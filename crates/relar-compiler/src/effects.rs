//! Conservative escaping-error summaries over the final, rewritten typed IR.
//!
//! Allocation failure remains fatal under the current runtime contract. Fixed
//! fields cannot raise: the checker fixes their indices, prevents partially
//! initialized objects from escaping, and proves optional payloads present.
//! Container bounds, I/O, conversion, explicit raises and uncertain operations
//! remain fallible. Catching an error does not make its containing function
//! infallible here; every handler, else and finally body is included.

use crate::ir::{Binary, Builtin, Expr, ExprKind, Module, Statement, Type};

pub(crate) struct ExceptionEffects {
    functions: Vec<bool>,
}

impl ExceptionEffects {
    /// True includes uncertainty; false proves no pending error can escape a
    /// call entered without one. Callee arguments are evaluated by the caller
    /// and retain their own checks independently of this summary.
    pub(crate) fn may_raise(&self, function: usize) -> bool {
        self.functions.get(function).copied().unwrap_or(true)
    }
}

/// Removes fallibility only after every operation and callee is proved safe.
/// Starting pessimistically also leaves unexpected recursive cycles fallible;
/// the current frontend rejects them, and its acyclic graph reaches a fixed
/// point after at most one successful transition per function.
pub(crate) fn analyze(module: &Module) -> ExceptionEffects {
    let mut functions = vec![true; module.functions.len()];
    loop {
        let mut changed = false;
        for (id, function) in module.functions.iter().enumerate() {
            if functions[id] && !block(&function.body, &functions) {
                functions[id] = false;
                changed = true;
            }
        }
        if !changed {
            return ExceptionEffects { functions };
        }
    }
}

/// Any potentially failing path is enough; unreachable branches and caught
/// errors deliberately remain conservative rather than requiring control flow.
fn block(body: &[Statement], functions: &[bool]) -> bool {
    body.iter().any(|statement| match statement {
        Statement::Assign(_, value) | Statement::Expr(value) | Statement::Return(value) => {
            expression(value, functions)
        }
        Statement::Move(..)
        | Statement::Clear(_)
        | Statement::Break
        | Statement::Continue
        | Statement::Exhaust => false,
        Statement::Scope { body, .. } => block(body, functions),
        Statement::SetItem(receiver, index, value) => {
            !matches!(receiver.ty, Type::Dict(_))
                || expression(receiver, functions)
                || expression(index, functions)
                || expression(value, functions)
        }
        // The initial indexed read may fail even for a dictionary assignment.
        Statement::AugmentItem(..) => true,
        Statement::SetField(receiver, _, value) => {
            expression(receiver, functions) || expression(value, functions)
        }
        Statement::AugmentField(receiver, _, op, value) => {
            binary(*op) || expression(receiver, functions) || expression(value, functions)
        }
        Statement::If(test, yes, no) => {
            expression(test, functions) || block(yes, functions) || block(no, functions)
        }
        Statement::While(test, body, orelse) => {
            expression(test, functions) || block(body, functions) || block(orelse, functions)
        }
        Statement::For {
            args, body, orelse, ..
        } => {
            // The implicit step is exactly one. An explicit step is kept
            // fallible even when a separate range proof could exclude zero.
            !matches!(args.len(), 1 | 2)
                || args.iter().any(|arg| expression(arg, functions))
                || block(body, functions)
                || block(orelse, functions)
        }
        Statement::IterNext { .. } | Statement::Assert(_) | Statement::Raise(_) => true,
        Statement::Try {
            body,
            handlers,
            orelse,
            finalbody,
        } => {
            block(body, functions)
                || handlers
                    .iter()
                    .any(|handler| block(&handler.body, functions))
                || block(orelse, functions)
                || block(finalbody, functions)
        }
    })
}

/// Includes every evaluated child; fixed-layout reads and writes themselves
/// cannot fail, but their receiver, replacement or initializer still can.
fn expression(value: &Expr, functions: &[bool]) -> bool {
    match &value.kind {
        ExprKind::Int(_)
        | ExprKind::Float(_)
        | ExprKind::Bool(_)
        | ExprKind::Str(_)
        | ExprKind::Bytes(_)
        | ExprKind::None
        | ExprKind::Local(_) => false,
        ExprKind::List(values)
        | ExprKind::Tuple(values)
        | ExprKind::Set { values, .. }
        | ExprKind::Compare(values, _)
        | ExprKind::Boolean(_, values) => values.iter().any(|value| expression(value, functions)),
        ExprKind::Dict { entries, .. } => entries
            .iter()
            .any(|(key, value)| expression(key, functions) || expression(value, functions)),
        ExprKind::Scope { body, value, .. } => {
            block(body, functions) || expression(value, functions)
        }
        ExprKind::Bind(_, value)
        | ExprKind::Present(value)
        | ExprKind::Absent(value)
        | ExprKind::Unwrap(value)
        | ExprKind::Not(value)
        | ExprKind::Negate(value)
        | ExprKind::Field(value, _) => expression(value, functions),
        ExprKind::Binary(op, left, right) => {
            (matches!(op, Binary::Concat) && left.ty == Type::Bytes)
                || binary(*op)
                || matches!(
                    (left.ty, right.ty),
                    (Type::Int, Type::Float) | (Type::Float, Type::Int)
                )
                || expression(left, functions)
                || expression(right, functions)
        }
        ExprKind::Conditional(test, yes, no) => {
            expression(test, functions) || expression(yes, functions) || expression(no, functions)
        }
        ExprKind::BooleanValue(_, left, right) => {
            expression(left, functions) || expression(right, functions)
        }
        ExprKind::Call(id, args) => {
            functions.get(*id).copied().unwrap_or(true)
                || args.values.iter().any(|arg| expression(arg, functions))
        }
        ExprKind::Construct {
            initializer, args, ..
        } => {
            functions.get(*initializer).copied().unwrap_or(true)
                || args.values.iter().any(|arg| expression(arg, functions))
        }
        ExprKind::Exception { message, .. } => message
            .as_ref()
            .is_some_and(|message| expression(message, functions)),
        ExprKind::Builtin(builtin, args) => {
            builtin_raises(builtin, args) || args.iter().any(|arg| expression(arg, functions))
        }
    }
}

fn binary(op: Binary) -> bool {
    match op {
        Binary::Divide
        | Binary::FloorDivide
        | Binary::Modulo
        | Binary::Repeat
        | Binary::LeftShift
        | Binary::RightShift => true,
        Binary::Add
        | Binary::Subtract
        | Binary::Multiply
        | Binary::Concat
        | Binary::BitAnd
        | Binary::BitOr
        | Binary::BitXor => false,
    }
}

/// Classifies only the builtin's own work. Argument evaluation is accounted
/// separately, including eager defaults on otherwise infallible dictionary lookups.
fn builtin_raises(builtin: &Builtin, args: &[Expr]) -> bool {
    match builtin {
        Builtin::Print
        | Builtin::Input
        | Builtin::ListGet
        | Builtin::ListGetI32
        | Builtin::StringGet
        | Builtin::BytesGet
        | Builtin::Decode
        | Builtin::BytesValidateUtf8
        | Builtin::BinaryOpen(_)
        | Builtin::BinaryRead
        | Builtin::BinaryWrite
        | Builtin::Slice
        | Builtin::Pop
        | Builtin::DictGet
        | Builtin::DictGatherStrInt
        | Builtin::DictGatherBytesInt
        | Builtin::DictDelete
        | Builtin::SysArgv
        | Builtin::TextOpen(_)
        | Builtin::TextEnter
        | Builtin::TextRead
        | Builtin::TextWrite
        | Builtin::TextFlush
        | Builtin::TextClose => true,
        Builtin::Bytes => args.first().is_none_or(|arg| arg.ty != Type::Bytes),
        Builtin::Int => args
            .first()
            .is_none_or(|arg| !matches!(arg.ty, Type::Int | Type::Bool)),
        Builtin::Float => args
            .first()
            .is_none_or(|arg| !matches!(arg.ty, Type::Float | Type::Bool)),
        Builtin::GlobalGet(_)
        | Builtin::GlobalSet(_)
        | Builtin::Bool
        | Builtin::UnionInject
        | Builtin::UnionProject
        | Builtin::IsInstance(_)
        | Builtin::Len
        | Builtin::ListAllI32
        | Builtin::ListReserveRange
        | Builtin::ListDistinct
        | Builtin::Append
        | Builtin::IsNone
        | Builtin::Split
        | Builtin::SplitLines
        | Builtin::RStrip
        | Builtin::Str
        | Builtin::TextClosed
        | Builtin::DictGetDefault
        | Builtin::DictTryGet(_)
        | Builtin::DictContains
        | Builtin::DictIter(_)
        | Builtin::SetContains
        | Builtin::SetAdd
        | Builtin::SetDiscard
        | Builtin::SetIter
        | Builtin::StringIter
        | Builtin::BytesIter
        | Builtin::Encode
        | Builtin::ExceptionMessage
        | Builtin::ExceptionWithCause
        | Builtin::ExceptionCause
        | Builtin::ExceptionSuppressContext => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{CallArgs, ExceptHandler, Function};

    fn integer() -> Expr {
        Expr {
            ty: Type::Int,
            kind: ExprKind::Int("1".into()),
        }
    }

    fn function(body: Vec<Statement>) -> Function {
        Function {
            params: Vec::new(),
            returns: Type::None,
            locals: Vec::new(),
            body,
        }
    }

    fn call(id: usize, args: Vec<Expr>) -> Expr {
        Expr {
            ty: Type::Int,
            kind: ExprKind::Call(id, CallArgs::positional(args)),
        }
    }

    #[test]
    fn transitive_calls_converge_and_uncertain_cycles_stay_fallible() {
        let module = Module {
            types: Default::default(),
            class_fields: Vec::new(),
            globals: Vec::new(),
            functions: vec![
                function(vec![Statement::Return(call(1, Vec::new()))]),
                function(vec![Statement::Return(call(2, Vec::new()))]),
                function(vec![Statement::Return(integer())]),
                function(vec![Statement::Return(call(4, Vec::new()))]),
                function(vec![Statement::Return(call(3, Vec::new()))]),
            ],
            entry: function(Vec::new()),
        };
        assert_eq!(
            analyze(&module).functions,
            [false, false, false, true, true]
        );
    }

    #[test]
    fn fixed_fields_preserve_receiver_argument_and_initializer_errors() {
        let field = |receiver| Expr {
            ty: Type::Int,
            kind: ExprKind::Field(Box::new(receiver), 0),
        };
        let construct = |initializer, args| Expr {
            ty: Type::Class(0),
            kind: ExprKind::Construct {
                initializer,
                args: CallArgs::positional(args),
            },
        };
        let module = Module {
            types: Default::default(),
            class_fields: Vec::new(),
            globals: Vec::new(),
            functions: vec![
                function(vec![Statement::Return(integer())]),
                function(vec![Statement::Raise(None)]),
                function(vec![Statement::Return(field(construct(0, Vec::new())))]),
                function(vec![Statement::Return(field(call(1, Vec::new())))]),
                function(vec![Statement::Return(call(0, vec![call(1, Vec::new())]))]),
                function(vec![Statement::Return(construct(1, Vec::new()))]),
                function(vec![Statement::Return(construct(
                    0,
                    vec![call(1, Vec::new())],
                ))]),
                function(vec![Statement::SetField(integer(), 0, call(1, Vec::new()))]),
            ],
            entry: function(Vec::new()),
        };
        assert_eq!(
            analyze(&module).functions,
            [false, true, false, true, true, true, true, true]
        );
    }

    #[test]
    fn scopes_handlers_finally_and_iterator_steps_remain_fallible() {
        let raises = vec![Statement::Raise(None)];
        for part in 0..4 {
            let statement = Statement::Try {
                body: if part == 0 {
                    raises.clone()
                } else {
                    Vec::new()
                },
                handlers: vec![ExceptHandler {
                    kinds: Vec::new(),
                    name: None,
                    body: if part == 1 {
                        raises.clone()
                    } else {
                        Vec::new()
                    },
                }],
                orelse: if part == 2 {
                    raises.clone()
                } else {
                    Vec::new()
                },
                finalbody: if part == 3 {
                    raises.clone()
                } else {
                    Vec::new()
                },
            };
            assert!(block(&[statement], &[]));
        }
        assert!(block(
            &[Statement::Scope {
                body: vec![Statement::IterNext {
                    iterator: 0,
                    key: 1,
                    value: None,
                    ready: 2,
                }],
                clear: vec![0],
            }],
            &[]
        ));
    }

    #[test]
    fn range_steps_and_default_arguments_keep_their_errors() {
        let range = |args| Statement::For {
            target: 0,
            cursor: 1,
            stop: 2,
            step: 3,
            args,
            body: Vec::new(),
            orelse: Vec::new(),
        };
        assert!(!block(&[range(vec![integer()])], &[]));
        assert!(block(&[range(vec![integer(), integer(), integer()])], &[]));
        assert!(block(&[range(vec![call(0, Vec::new())])], &[true]));
        let value = Expr {
            ty: Type::Int,
            kind: ExprKind::Builtin(
                Builtin::DictGetDefault,
                vec![integer(), integer(), call(0, Vec::new())],
            ),
        };
        assert!(expression(&value, &[true]));
    }

    #[test]
    fn codegen_checks_fallible_calls_but_not_fixed_field_call_chains() {
        let source = r#"
class Record:
    value: int
    def __init__(self, value: int) -> None:
        self.value = value
    def replace(self, value: int) -> int:
        self.value = value
        return self.value

def pure(record: Record, value: int) -> int:
    return record.replace(value)

def parse(value: str) -> int:
    return int(value)

def indirect(value: str) -> int:
    return parse(value)
"#;
        let llvm = crate::compile(source, &crate::CompileOptions::default()).unwrap();
        assert!(llvm.contains("getelementptr inbounds %class_0"));
        // Parsing, its fallible caller, and the process-entry flush stay checked.
        assert_eq!(llvm.matches("call i8 @relar_error_pending()").count(), 3);
    }

    #[test]
    fn loop_else_errors_are_part_of_the_function_summary() {
        let source = r#"
def range_error() -> None:
    for ignored in range(0):
        pass
    else:
        raise ValueError("range exhausted")

def while_error() -> None:
    while False:
        pass
    else:
        raise ValueError("condition failed")
"#;
        let graph = crate::modules::SourceGraph::single(source).unwrap();
        let module = crate::frontend::lower(&graph).unwrap();
        let effects = analyze(&module);
        assert!(effects.may_raise(0));
        assert!(effects.may_raise(1));
    }
}
