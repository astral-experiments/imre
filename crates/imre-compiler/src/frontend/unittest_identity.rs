//! TestCase identity assertions use the same typed operands as a Python `is` expression.

use super::*;

impl Checker<'_> {
    /// unittest.util.safe_repr falls back to object.__repr__ if user code raises Exception.
    fn unittest_identity_repr(&mut self, value: Expr, at: &impl Ranged) -> Result<Expr, String> {
        let rendered = self.record_repr(value.clone(), at)?;
        if !matches!(value.ty, Type::Class(_)) {
            return Ok(rendered);
        }
        let slot = self.hidden(Type::Str);
        let fallback = Expr {
            ty: Type::Str,
            kind: ExprKind::Builtin(Builtin::DefaultRecordRepr, vec![value]),
        };
        Ok(Expr {
            ty: Type::Str,
            kind: ExprKind::Scope {
                body: vec![Statement::Try {
                    body: vec![Statement::Assign(slot, rendered)],
                    handlers: vec![ir::ExceptHandler {
                        matcher: crate::ir::ExceptionMatcher::Categories(vec![
                            exception_kind("Exception").unwrap(),
                        ]),
                        name: None,
                        body: vec![Statement::Assign(slot, fallback)],
                    }],
                    orelse: Vec::new(),
                    finalbody: Vec::new(),
                }],
                value: Box::new(self.local(slot)),
                clear: vec![slot],
            },
        })
    }

    pub(super) fn unittest_identity(
        &mut self,
        call: &ast::ExprCall,
        receiver: &Expr,
        negated: bool,
    ) -> Result<Expr, String> {
        let failure = self
            .graph
            .modules
            .iter()
            .find(|m| m.bundled && m.name == "unittest")
            .and_then(|m| m.bindings.get("_raise_identity_failure"))
            .and_then(|binding| match binding {
                Binding::Definition(def) => self.functions.get(def),
                _ => None,
            })
            .cloned()
            .ok_or_else(|| {
                error(
                    self.source,
                    call,
                    "native TestCase identity formatter is unavailable",
                )
            })?;
        let params = &failure.params[1..];
        let both_classes = call
            .arguments
            .args
            .iter()
            .take(2)
            .chain(
                call.arguments
                    .keywords
                    .iter()
                    .filter(|kw| matches!(kw.arg.as_deref(), Some("expr1" | "expr2")))
                    .map(|kw| &kw.value),
            )
            .filter(|arg| self.class_reference(arg).is_some())
            .count()
            == 2;
        let mut class_operands = [None; 2];
        let args = self.arguments_with(
            call,
            params,
            Some(receiver.clone()),
            |checker, index, arg| {
                if index == 2 {
                    checker.direct_argument(arg, params[index].ty, false)
                } else if both_classes && let Some(class) = checker.class_reference(arg) {
                    // Like an ordinary "is" expression, fixed class bindings have a
                    // nominal identity. No first-class class object is required.
                    class_operands[index] = Some(class);
                    let decl = &checker.classes.declarations.definitions[class];
                    Ok(unpacking::string(format!(
                        "<class '{}.{}'>",
                        checker.runtime_module_name(decl.definition.module),
                        decl.name
                    )))
                } else {
                    // These fresh empty literals only expose their identity and repr.
                    match arg {
                        ast::Expr::List(list) if list.elts.is_empty() => {
                            checker.expr_expected(arg, Type::ListInt)
                        }
                        ast::Expr::Dict(dict) if dict.items.is_empty() => {
                            let ty = checker.types.dict(Type::Str, Type::Int);
                            checker.expr_expected(arg, ty)
                        }
                        _ => checker.expr(arg),
                    }
                }
            },
        )?;
        // All supplied arguments, including msg, run before the comparison. Only
        // the failing branch may call repr; it uses the same captured objects.
        let mut scope = calls::ArgumentScope::default();
        let values: Vec<_> = args
            .values
            .into_iter()
            .map(|v| scope.hold(self, v))
            .collect();
        let [case, first, second, msg] = args.order.as_slice() else {
            unreachable!()
        };
        let left = values[*first].clone();
        let right = values[*second].clone();
        let op = if negated {
            ast::CmpOp::Is
        } else {
            ast::CmpOp::IsNot
        };
        let mismatch = if let [Some(a), Some(b)] = class_operands {
            Expr::boolean((a == b) == negated)
        } else if left.ty == Type::None {
            Self::identity_is_none(right.clone(), op)
        } else if right.ty == Type::None {
            Self::identity_is_none(left.clone(), op)
        } else {
            self.identity_values(left.clone(), right.clone(), op, call)?
        };
        let none = Expr::none();
        // Each formatter may catch a Python exception. Finish both before
        // borrowing the case or either string as arguments to the failure call.
        let mut diagnostic = calls::ArgumentScope::default();
        let repr = if class_operands[0].is_some() {
            left
        } else {
            self.unittest_identity_repr(left, call)?
        };
        let first_repr = diagnostic.hold(self, repr);
        let second_repr = if negated {
            none.clone()
        } else if class_operands[1].is_some() {
            right
        } else {
            self.unittest_identity_repr(right, call)?
        };
        let formatted = self.coerce(second_repr, params[1].ty, call)?;
        let formatted = diagnostic.hold(self, formatted);
        self.calls.insert(failure.id);
        let fail = Expr {
            ty: Type::None,
            kind: ExprKind::Call(
                failure.id,
                ir::CallArgs::positional(vec![
                    self.coerce(values[*case].clone(), failure.params[0].ty, call)?,
                    first_repr,
                    formatted,
                    values[*msg].clone(),
                ]),
            ),
        };
        Ok(scope.finish(Expr {
            ty: Type::None,
            kind: ExprKind::Conditional(
                Box::new(mismatch),
                Box::new(diagnostic.finish(fail)),
                Box::new(none),
            ),
        }))
    }
}
