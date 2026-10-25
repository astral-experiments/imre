//! Lower copy.copy only for types whose Python copy behavior is known.

use super::*;

impl Checker<'_> {
    pub(super) fn shallow_copy(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        let source = match (
            call.arguments.args.as_ref(),
            call.arguments.keywords.as_ref(),
        ) {
            ([value], []) => value,
            ([], [kw]) if kw.arg.as_ref().is_some_and(|name| name.as_str() == "x") => &kw.value,
            _ => {
                return Err(error(
                    self.source,
                    call,
                    "copy.copy requires one argument, x",
                ));
            }
        };
        let source = self.expr(source)?;
        let ty = source.ty;
        match ty {
            Type::None
            | Type::Bool
            | Type::Int
            | Type::Float
            | Type::Str
            | Type::StrLiteral(_)
            | Type::Bytes
            | Type::StrEnum(_)
            | Type::Enum(_) => Ok(source),
            Type::Tuple(_)
                if self
                    .types
                    .fields(ty)
                    .is_some_and(|fields| self.types.tuple(fields) == ty) =>
            {
                Ok(source)
            }
            Type::Class(class) if self.classes.has_concrete_subclasses(class) => self
                .dispatch_record(class, source, call, |checker, actual, obj| {
                    checker.copy_method(actual, obj, call)
                }),
            Type::Class(class) => self.copy_method(class, source, call),
            Type::ListInt | Type::List(_) => Ok(self.list_snapshot(source)),
            Type::Dict(_) | Type::Set(_) => Ok(Expr {
                ty,
                kind: ExprKind::Builtin(
                    if matches!(ty, Type::Dict(_)) {
                        Builtin::DictCopy
                    } else {
                        Builtin::KeySet
                    },
                    vec![source],
                ),
            }),
            _ => Err(error(
                self.source,
                call,
                "copy.copy requires a supported atomic type or container",
            )),
        }
    }

    /// Resolve a class's inherited or overridden shallow copy protocol method.
    fn copy_method(
        &mut self,
        class: usize,
        receiver: Expr,
        call: &ast::ExprCall,
    ) -> Result<Expr, String> {
        let Some(signature) = self.classes.methods[class].get("__copy__") else {
            let declarations = &self.classes.declarations.definitions;
            if self
                .classes
                .declarations
                .exceptions
                .contains_key(&declarations[class].definition)
                || std::iter::once(class)
                    .chain(declarations[class].bases.iter().copied())
                    .any(|id| declarations[id].node.body.iter().any(custom_reconstruction))
            {
                return Err(error(
                    self.source,
                    call,
                    "copying this class requires an unsupported reconstruction protocol",
                ));
            }
            return Ok(Expr {
                ty: receiver.ty,
                kind: ExprKind::Builtin(Builtin::RecordCopy, vec![receiver]),
            });
        };
        self.calls.insert(signature.id);
        Ok(Expr {
            ty: signature.returns,
            kind: ExprKind::Call(signature.id, ir::CallArgs::positional(vec![receiver])),
        })
    }

    /// A full slice duplicates storage while acquiring each existing element.
    pub(super) fn list_snapshot(&self, list: Expr) -> Expr {
        let absent = Expr {
            ty: self.types.optional(Type::Int),
            kind: ExprKind::Absent(Box::new(Expr {
                ty: Type::None,
                kind: ExprKind::None,
            })),
        };
        Expr {
            ty: list.ty,
            kind: ExprKind::Builtin(
                Builtin::Slice,
                vec![list, absent.clone(), absent.clone(), absent],
            ),
        }
    }
}

/// Reachability may omit an unused method from the native table. Check the
/// original class declarations before treating a record as plain Python state.
fn custom_reconstruction(statement: &ast::Stmt) -> bool {
    fn protocol(name: &str) -> bool {
        matches!(
            name,
            "__copy__"
                | "__reduce__"
                | "__reduce_ex__"
                | "__getstate__"
                | "__setstate__"
                | "__getnewargs__"
                | "__getnewargs_ex__"
                | "__new__"
                | "__getattr__"
                | "__getattribute__"
                | "__setattr__"
                | "__delattr__"
                | "__slots__"
                | "__dict__"
        )
    }
    let binding =
        |target: &ast::Expr| matches!(target, ast::Expr::Name(n) if protocol(n.id.as_str()));
    match statement {
        ast::Stmt::FunctionDef(f) => protocol(f.name.as_str()),
        ast::Stmt::Assign(a) => a.targets.iter().any(binding),
        ast::Stmt::AnnAssign(a) => binding(&a.target),
        _ => false,
    }
}
