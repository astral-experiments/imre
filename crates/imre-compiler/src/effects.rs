//! Conservative escaping-error summaries over the final, rewritten typed IR.
//!
//! Some runtime operations report allocation failure. Fixed fields cannot raise:
//! the checker fixes their indices, prevents partially
//! initialized objects from escaping, and proves optional payloads present.
//! Container bounds, I/O, conversion, explicit raises and uncertain operations
//! remain fallible. Catching an error does not make its containing function
//! infallible here; every handler, else and finally body is included.

use crate::callables::Targets;
use crate::ir::{Expr, ExprKind, Module, Statement, Type};
use ruff_python_ast::CmpOp;

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

#[derive(Clone, Copy)]
struct Context<'a> {
    calls: &'a Targets,
    types: &'a crate::types::Types,
}

/// Removes fallibility only after every operation and callee is proved safe.
/// Starting pessimistically also leaves unexpected recursive cycles fallible;
/// the current frontend rejects them, and its acyclic graph reaches a fixed
/// point after at most one successful transition per function.
pub(crate) fn analyze(module: &Module, targets: &Targets) -> ExceptionEffects {
    let targets = Context {
        calls: targets,
        types: &module.types,
    };
    let mut functions = vec![true; module.functions.len()];
    loop {
        let mut changed = false;
        for (id, function) in module.functions.iter().enumerate() {
            if functions[id]
                && !module.source_frames.get(id).is_some_and(Option::is_some)
                && !block(&function.body, &functions, targets)
            {
                functions[id] = false;
                changed = true;
            }
        }
        if !changed {
            return ExceptionEffects { functions };
        }
    }
}

/// Check a fragment during lowering, when no callee is yet proven infallible.
pub(crate) fn may_raise_locally(body: &[Statement], types: &crate::types::Types) -> bool {
    block(
        body,
        &[],
        Context {
            calls: &Targets::default(),
            types,
        },
    )
}

/// Any potentially failing path is enough; unreachable branches and caught
/// errors deliberately remain conservative rather than requiring control flow.
fn block(body: &[Statement], functions: &[bool], targets: Context<'_>) -> bool {
    body.iter().any(|statement| match statement {
        Statement::Assign(_, value) | Statement::Expr(value) | Statement::Return(value) => {
            expression(value, functions, targets)
        }
        Statement::Move(..)
        | Statement::Clear(_)
        | Statement::Position(..)
        | Statement::Break(_)
        | Statement::Continue(_)
        | Statement::Exhaust => false,
        Statement::Scope { body, .. } => block(body, functions, targets),
        Statement::SetItem(..) => true,
        // The initial indexed read may fail even for a dictionary assignment.
        Statement::AugmentItem(..) => true,
        Statement::SetField(receiver, _, value) => {
            expression(receiver, functions, targets) || expression(value, functions, targets)
        }
        Statement::AugmentField(receiver, _, op, value) => {
            op.may_raise()
                || expression(receiver, functions, targets)
                || expression(value, functions, targets)
        }
        // Matching an object can call arbitrary equality and truth hooks.
        Statement::Match(..) => true,
        Statement::If(test, yes, no) => {
            expression(test, functions, targets)
                || block(yes, functions, targets)
                || block(no, functions, targets)
        }
        // Every native loop can propagate KeyboardInterrupt at its signal checkpoint.
        Statement::While(..) | Statement::For { .. } => true,
        Statement::With { .. } => true,
        Statement::IterNext { .. } | Statement::Raise(_) => true,
        Statement::Try {
            body,
            handlers,
            orelse,
            finalbody,
        } => {
            block(body, functions, targets)
                || handlers
                    .iter()
                    .any(|handler| block(&handler.body, functions, targets))
                || block(orelse, functions, targets)
                || block(finalbody, functions, targets)
        }
    })
}

/// Includes every evaluated child; fixed-layout reads and writes themselves
/// cannot fail, but their receiver, replacement or initializer still can.
fn expression(value: &Expr, functions: &[bool], targets: Context<'_>) -> bool {
    match &value.kind {
        ExprKind::Int(_)
        | ExprKind::Float(_)
        | ExprKind::Bool(_)
        | ExprKind::Str(_)
        | ExprKind::Bytes(_)
        | ExprKind::None
        | ExprKind::Local(_) => false,
        ExprKind::Unary(..) if value.uses_python_unary_protocol() => true,
        ExprKind::Compare(_, ops) if crate::ir::Compare::uses_python_protocol(value.ty, ops) => {
            true
        }
        // Fixed-offset datetime ordering raises on a mix of naive and aware operands.
        // Preserve this effect across a direct call even when both operands are simple locals.
        ExprKind::Compare(values, ops)
            if ops.iter().zip(values).any(|(op, value)| {
                matches!(
                    value.ty,
                    crate::ir::Type::Calendar(crate::ir::CalendarType::DateTime)
                ) && !matches!(op, CmpOp::Eq | CmpOp::NotEq)
            }) =>
        {
            true
        }
        // Even an existing point needs space when appended to a packed list literal.
        ExprKind::List(values)
            if !values.is_empty()
                && targets
                    .types
                    .element(value.ty)
                    .is_some_and(|ty| targets.types.is_value_struct(ty)) =>
        {
            true
        }
        ExprKind::Compare(values, _) => {
            // A structural comparison may allocate a traversal stack, including
            // when a set or Counter looks up an equal record key.
            values.windows(2).any(|pair| {
                matches!(
                    pair[0].ty,
                    Type::Class(_)
                        | Type::Tuple(_)
                        | Type::List(_)
                        | Type::Dict(_)
                        | Type::Set(_)
                        | Type::Deque(_)
                        | Type::SimpleNamespace(_)
                )
            }) || values
                .iter()
                .any(|value| expression(value, functions, targets))
        }
        ExprKind::List(values) | ExprKind::Boolean(_, values) => values
            .iter()
            .any(|value| expression(value, functions, targets)),
        ExprKind::Dict(entries) => value.ty == Type::PythonObject || !entries.is_empty(),
        ExprKind::Set { values, .. } => !values.is_empty(),
        ExprKind::Scope { body, value, .. } => {
            block(body, functions, targets) || expression(value, functions, targets)
        }
        ExprKind::Bind(_, value)
        | ExprKind::Present(value)
        | ExprKind::Absent(value)
        | ExprKind::Unwrap(value)
        | ExprKind::Unary(_, value)
        | ExprKind::Field(value, _) => expression(value, functions, targets),
        ExprKind::Binary(op, left, right) => {
            op.may_raise_with_operands(left.ty, right.ty)
                || expression(left, functions, targets)
                || expression(right, functions, targets)
        }
        ExprKind::Conditional(test, yes, no) => {
            expression(test, functions, targets)
                || expression(yes, functions, targets)
                || expression(no, functions, targets)
        }
        ExprKind::BooleanValue(_, left, right, predicate) => {
            targets.types.truth_may_materialize(left.ty)
                || predicate
                    .as_ref()
                    .is_some_and(|p| expression(&p.condition, functions, targets))
                || expression(left, functions, targets)
                || expression(right, functions, targets)
        }
        ExprKind::Call(id, args) => {
            functions.get(*id).copied().unwrap_or(true)
                || args
                    .values
                    .iter()
                    .any(|arg| expression(arg, functions, targets))
        }
        ExprKind::Callable { receiver, .. } => receiver
            .as_ref()
            .is_some_and(|receiver| expression(receiver, functions, targets)),
        ExprKind::Invoke { callable, args } => {
            let candidates = targets.calls.get(callable.ty);
            candidates.is_empty()
                || candidates
                    .iter()
                    .any(|target| functions.get(target.function).copied().unwrap_or(true))
                || expression(callable, functions, targets)
                || args
                    .values
                    .iter()
                    .any(|arg| expression(arg, functions, targets))
        }
        ExprKind::Tuple(_) | ExprKind::Construct { .. } => true,
        ExprKind::Exception { message, .. } => message
            .as_ref()
            .is_some_and(|message| expression(message, functions, targets)),
        ExprKind::Builtin(builtin, args) => {
            // An owned struct read can need a new box even at a proven index.
            (matches!(builtin, crate::ir::Builtin::SequenceGetInBounds)
                && targets.types.is_value_struct(value.ty))
                || builtin.contract(args).may_raise
                || args.iter().any(|arg| expression(arg, functions, targets))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{Builtin, CallArgs, ExceptHandler, Function, Type};

    fn analyze(module: &Module) -> ExceptionEffects {
        super::analyze(module, &Targets::analyze(module))
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
    fn local_and_final_analysis_agree_on_packed_list_allocations() {
        let types = crate::types::Types::default();
        types.set_value_structs([0].into());
        let mut functions = Vec::new();
        for (item, packed) in [(Type::Class(0), true), (Type::Float, false)] {
            let list = types.list(item);
            let literal = Expr {
                ty: list,
                kind: ExprKind::List(vec![Expr::local(0, item)]),
            };
            let read = Expr {
                ty: item,
                kind: ExprKind::Builtin(
                    Builtin::SequenceGetInBounds,
                    vec![Expr::local(1, list), Expr::integer(1)],
                ),
            };
            for value in [literal, read] {
                let mut f = function(vec![Statement::Expr(value)]);
                f.params = vec![0, 1];
                f.locals = vec![item, list];
                assert_eq!(may_raise_locally(&f.body, &types), packed);
                functions.push(f);
            }
        }
        let module = Module {
            list_builds: Default::default(),
            command_flags: None,
            builtin_module_sources: None,
            types,
            functions,
            class_fields: Vec::new(),
            class_metadata: Vec::new(),
            json_records: Vec::new(),
            globals: Vec::new(),
            source_frames: Vec::new(),
            entry: function(Vec::new()),
        };
        assert_eq!(analyze(&module).functions, [true, true, false, false]);
    }

    #[test]
    fn unknown_callbacks_keep_escaping_error_checks() {
        let context = Context {
            calls: &Targets::default(),
            types: &Default::default(),
        };
        let callback = Expr::local(0, Type::Callable(0));
        let invocation = Expr {
            ty: Type::Int,
            kind: ExprKind::Invoke {
                callable: Box::new(callback),
                args: CallArgs::positional(vec![Expr::integer(1)]),
            },
        };
        assert!(expression(&invocation, &[], context));
    }

    #[test]
    fn callbacks_require_every_target_and_evaluated_child_to_be_infallible() {
        let callback = Type::Callable(0);
        let reference = |function| Expr {
            ty: callback,
            kind: ExprKind::Callable {
                function,
                receiver: None,
            },
        };
        let invocation = |callable, args| Expr {
            ty: Type::Int,
            kind: ExprKind::Invoke {
                callable: Box::new(callable),
                args: CallArgs::positional(args),
            },
        };
        let local = Expr::local(0, callback);
        let mut module = Module {
            types: Default::default(),
            class_fields: Vec::new(),
            list_builds: Default::default(),
            class_metadata: Vec::new(),
            command_flags: None,
            builtin_module_sources: None,
            json_records: Vec::new(),
            globals: Vec::new(),
            source_frames: Vec::new(),
            functions: vec![
                function(vec![Statement::Return(Expr::integer(1))]),
                function(vec![Statement::Return(Expr::integer(1))]),
                function(vec![Statement::Return(invocation(
                    local.clone(),
                    vec![Expr::integer(1)],
                ))]),
                function(vec![Statement::Raise(None)]),
            ],
            entry: function(vec![
                Statement::Expr(reference(1)),
                Statement::Expr(reference(0)),
            ]),
        };
        let targets = Targets::analyze(&module);
        let context = Context {
            calls: &targets,
            types: &module.types,
        };
        assert_eq!(
            super::analyze(&module, &targets).functions,
            [false, false, false, true]
        );
        let functions = [false, false, false, true];
        assert!(expression(
            &invocation(local.clone(), vec![call(3, Vec::new())]),
            &functions,
            context,
        ));
        assert!(expression(
            &invocation(
                Expr {
                    ty: callback,
                    kind: ExprKind::Scope {
                        body: vec![Statement::Raise(None)],
                        value: Box::new(local),
                        clear: Vec::new(),
                    },
                },
                vec![Expr::integer(1)]
            ),
            &functions,
            context,
        ));
        // One failing alternative poisons the whole exact signature, even if
        // no local value-flow analysis would select it at this particular call.
        module.entry.body.push(Statement::Expr(reference(3)));
        assert!(analyze(&module).may_raise(2));
    }

    #[test]
    fn transitive_calls_converge_and_uncertain_cycles_stay_fallible() {
        let module = Module {
            types: Default::default(),
            class_fields: Vec::new(),
            list_builds: Default::default(),
            class_metadata: Vec::new(),
            command_flags: None,
            builtin_module_sources: None,
            json_records: Vec::new(),
            globals: Vec::new(),
            source_frames: Vec::new(),
            functions: vec![
                function(vec![Statement::Return(call(1, Vec::new()))]),
                function(vec![Statement::Return(call(2, Vec::new()))]),
                function(vec![Statement::Return(Expr::integer(1))]),
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
    fn proven_sequence_pulls_preserve_deferred_read_errors() {
        let source = crate::modules::SourceGraph::single(
            r#"
def checked(values: list[str], index: int) -> str:
    return values[index]

def forward(values: list[str]) -> bool:
    return any(value == "" for value in values)

def backward(values: list[str]) -> bool:
    return all(value != "" for value in reversed(values))

def conversion(values: list[str]) -> bool:
    return any(int(value) for value in values)

def integers(values: list[int]) -> int:
    return len(values)
"#,
        )
        .unwrap();
        let module = crate::frontend::lower(&source).unwrap();
        assert_eq!(analyze(&module).functions, [true, true, true, true, false]);
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
            list_builds: Default::default(),
            class_metadata: Vec::new(),
            command_flags: None,
            builtin_module_sources: None,
            json_records: Vec::new(),
            globals: Vec::new(),
            source_frames: Vec::new(),
            functions: vec![
                function(vec![Statement::Return(Expr::integer(1))]),
                function(vec![Statement::Raise(None)]),
                function(vec![Statement::Return(field(construct(0, Vec::new())))]),
                function(vec![Statement::Return(field(call(1, Vec::new())))]),
                function(vec![Statement::Return(call(0, vec![call(1, Vec::new())]))]),
                function(vec![Statement::Return(construct(1, Vec::new()))]),
                function(vec![Statement::Return(construct(
                    0,
                    vec![call(1, Vec::new())],
                ))]),
                function(vec![Statement::SetField(
                    Expr::integer(1),
                    0,
                    call(1, Vec::new()),
                )]),
            ],
            entry: function(Vec::new()),
        };
        assert_eq!(
            analyze(&module).functions,
            [false, true, true, true, true, true, true, true]
        );
    }

    #[test]
    fn scopes_handlers_finally_and_iterator_steps_remain_fallible() {
        let context = Context {
            calls: &Targets::default(),
            types: &Default::default(),
        };
        let raises = vec![Statement::Raise(None)];
        for part in 0..4 {
            let statement = Statement::Try {
                body: if part == 0 {
                    raises.clone()
                } else {
                    Vec::new()
                },
                handlers: vec![ExceptHandler {
                    matcher: crate::ir::ExceptionMatcher::Categories(Vec::new()),
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
            assert!(block(&[statement], &[], context));
        }
        assert!(block(
            &[Statement::Scope {
                body: vec![Statement::IterNext {
                    kind: crate::ir::IterNextKind::Generic,
                    iterator: 0,
                    key: 1,
                    value: None,
                    ready: 2,
                }],
                clear: vec![0],
            }],
            &[],
            context
        ));
    }

    #[test]
    fn native_loops_and_default_arguments_keep_their_errors() {
        let context = Context {
            calls: &Targets::default(),
            types: &Default::default(),
        };
        let range = |args| Statement::For {
            target: 0,
            cursor: 1,
            stop: 2,
            step: 3,
            args,
            body: Vec::new(),
            orelse: Vec::new(),
        };
        assert!(block(&[range(vec![Expr::integer(1)])], &[], context));
        assert!(block(
            &[range(vec![
                Expr::integer(1),
                Expr::integer(1),
                Expr::integer(1)
            ])],
            &[],
            context
        ));
        assert!(block(&[range(vec![call(0, Vec::new())])], &[true], context));
        let value = Expr {
            ty: Type::Int,
            kind: ExprKind::Builtin(
                Builtin::DictGetDefault,
                vec![Expr::integer(1), Expr::integer(1), call(0, Vec::new())],
            ),
        };
        assert!(expression(&value, &[true], context));
    }

    #[test]
    fn builtin_module_names_preserves_checked_calls() {
        let source = r#"
import sys
from sys import builtin_module_names as names

def get_names() -> tuple[str, ...]:
    return sys.builtin_module_names

def take(value: int) -> int:
    return value

print(take(2), 'errno' in sys.builtin_module_names)
print(type(names) is tuple, type(sys.builtin_module_names) is tuple, type(get_names()) is tuple)
"#;
        let llvm = crate::compile(source, &crate::CompileOptions::default()).unwrap();
        assert!(llvm.contains("call ptr @imre_sys_builtin_module_names("));
        assert!(llvm.contains("call i1 @imre_str_tuple_contains("));
        assert!(!llvm.contains("call ptr @imre_dyn_new("));

        let source = "import sys\ndef take(value: int) -> int:\n    return value\nif False:\n    print(sys.builtin_module_names)\nprint(take('x'))\n";
        let error = crate::compile(source, &crate::CompileOptions::default()).unwrap_err();
        assert!(error.contains("expected int, got str"), "{error}");
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
        // Parsing and its fallible caller stay checked. Finalization returns the exit status.
        assert_eq!(llvm.matches("call i32 @imre_error_pending()").count(), 2);
        assert!(llvm.contains("call i32 @imre_finish_stdio()"));
    }

    #[test]
    fn allocation_errors_escape_user_functions() {
        let cases = [
            (
                "def lines(text: str) -> list[str]:\n    return text.splitlines()\ndef count(text: str) -> int:\n    return len(lines(text))\n",
                "call ptr @imre_str_splitlines(",
            ),
            (
                "import sys\ndef names() -> tuple[str, ...]:\n    return sys.builtin_module_names\ndef count() -> int:\n    return len(names())\n",
                "call ptr @imre_sys_builtin_module_names(",
            ),
            (
                "import sys\ndef names() -> frozenset[str]:\n    return sys.stdlib_module_names\ndef count() -> int:\n    return len(names())\n",
                "call ptr @imre_sys_stdlib_module_names(",
            ),
        ];
        for (source, allocation) in cases {
            let llvm = crate::compile(source, &crate::CompileOptions::default()).unwrap();
            for call in [allocation, "call ptr @fn_0("] {
                let (_, following) = llvm.split_once(call).unwrap();
                let (body, _) = following.split_once("\n}").unwrap();
                assert!(body.contains("call i32 @imre_error_pending()"), "{call}");
            }
        }
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
