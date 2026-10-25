//! Bind variadic mock calls at their actual call sites. The native source owns
//! the mock tree, call history, exception side effects and assertion decisions.

use super::*;

impl Checker<'_> {
    pub(super) fn mock_implementation_method(&self, class: usize, name: &str) -> bool {
        self.native_mock(class)
            && self.module
                != self.classes.declarations.definitions[class]
                    .definition
                    .module
            && matches!(
                name,
                "_at"
                    | "_child"
                    | "_attach"
                    | "_record"
                    | "_name"
                    | "_assert_called_with"
                    | "_assert_any_call"
                    | "_assert_called_once_with"
            )
    }

    pub(super) fn native_mock(&self, class: usize) -> bool {
        self.bundled_mock_class(class, "Mock")
    }

    pub(super) fn bundled_mock_class(&self, class: usize, name: &str) -> bool {
        let record = &self.classes.declarations.definitions[class];
        let module = &self.graph.modules[record.definition.module];
        module.bundled && module.name == "unittest.mock" && record.name == name
    }

    /// A recorded call has Python's tuple representation, including ordered keywords.
    pub(super) fn mock_call_repr(
        &mut self,
        value: &Expr,
        at: &impl Ranged,
    ) -> Result<Option<Expr>, String> {
        let (ty, name) = match value.ty {
            Type::Class(_) => (Some(value.ty), "_represent"),
            Type::Optional(_) => (self.types.payload(value.ty), "_represent"),
            Type::List(_) => (self.types.element(value.ty), "_represent_list"),
            _ => return Ok(None),
        };
        let Some(Type::Class(class)) = ty else {
            return Ok(None);
        };
        if !self.bundled_mock_class(class, "_Call") {
            return Ok(None);
        }
        let method = self.classes.methods[class]
            .get(name)
            .ok_or_else(|| error(self.source, at, "missing native mock call formatter"))?;
        self.calls.insert(method.id);
        Ok(Some(Expr {
            ty: Type::Str,
            kind: ExprKind::Call(
                method.id,
                ir::CallArgs::positional(vec![self.coerce(
                    value.clone(),
                    method.params[0].ty,
                    at,
                )?]),
            ),
        }))
    }

    pub(super) fn mock_state_member(name: &str) -> bool {
        matches!(
            name,
            "called" | "call_count" | "call_args" | "call_args_list" | "side_effect" | "_mock_name"
        )
    }

    /// Each alias owns the same node; the table uses integer child indices so
    /// dropping the last alias releases even deeply nested mocks without cycles.
    pub(super) fn mock_node(&self, value: Expr, class: usize) -> Expr {
        let field = self.classes.declarations.definitions[class]
            .fields
            .iter()
            .position(|f| f.name == "_node")
            .unwrap();
        Expr {
            ty: self.classes.fields[class][field],
            kind: ExprKind::Field(Box::new(value), field),
        }
    }

    pub(super) fn mock_child(
        &mut self,
        class: usize,
        receiver: Expr,
        attr: &ast::ExprAttribute,
    ) -> Result<Expr, String> {
        let name = attr.attr.as_str();
        // An unimplemented real member must never become a successful child mock.
        if name.starts_with('_')
            || Self::mock_state_member(name)
            || matches!(
                name,
                "assert_any_call"
                    | "assert_has_calls"
                    | "assert_called_once_with"
                    | "assert_called_with"
                    | "attach_mock"
                    | "configure_mock"
                    | "mock_add_spec"
                    | "mock_calls"
                    | "method_calls"
                    | "reset_mock"
            )
            || self.classes.declarations.definitions[class]
                .fields
                .iter()
                .any(|field| field.name == name)
        {
            return Err(error(
                self.source,
                attr,
                "unsupported native unittest.mock member",
            ));
        }
        let child = &self.classes.methods[class]["_child"];
        self.calls.insert(child.id);
        Ok(Expr {
            ty: Type::Class(class),
            kind: ExprKind::Call(
                child.id,
                ir::CallArgs::positional(vec![receiver, unpacking::string(name)]),
            ),
        })
    }

    fn mock_argument(&mut self, node: &ast::Expr, argument: usize) -> Result<Expr, String> {
        let mut value = self.expr(node)?;
        let kind = match value.ty {
            Type::Int => "int",
            Type::Bool => "bool",
            Type::Str => "str",
            Type::StrLiteral(_) => {
                value = literals::view(value, Type::Str);
                "str"
            }
            Type::Bytes => "bytes",
            Type::None => "None",
            _ => {
                return Err(error(
                    self.source,
                    node,
                    "native mock call arguments require None, bool, int, str or bytes",
                ));
            }
        };
        let constructor = self.classes.methods[argument]["__init__"].id;
        self.calls.insert(constructor);
        Ok(Expr {
            ty: Type::Class(argument),
            kind: ExprKind::Construct {
                initializer: constructor,
                args: ir::CallArgs::positional(vec![
                    Expr {
                        ty: Type::Str,
                        kind: ExprKind::Builtin(Builtin::Repr, vec![value]),
                    },
                    unpacking::string(kind),
                ]),
            },
        })
    }

    pub(super) fn mock_arguments(
        &mut self,
        call: &ast::ExprCall,
        class: usize,
        receiver: Expr,
        method: &str,
    ) -> Result<Expr, String> {
        let target = self.classes.methods[class]
            .get(method)
            .ok_or_else(|| error(self.source, call, "missing native mock call recorder"))?;
        let (function, returns, positional, keywords) = (
            target.id,
            target.returns,
            target.params[1].ty,
            target.params[2].ty,
        );
        let Type::Class(argument) = self
            .types
            .element(positional)
            .expect("bundled mock argument list")
        else {
            unreachable!()
        };
        let args = call
            .arguments
            .args
            .iter()
            .map(|expr| self.mock_argument(expr, argument))
            .collect::<Result<_, _>>()?;
        let kwargs = call
            .arguments
            .keywords
            .iter()
            .map(|kw| {
                let name = kw.arg.as_ref().ok_or_else(|| {
                    error(
                        self.source,
                        kw,
                        "native mock keyword unpacking requires fixed names",
                    )
                })?;
                Ok((
                    unpacking::string(name.as_str()),
                    self.mock_argument(&kw.value, argument)?,
                ))
            })
            .collect::<Result<_, String>>()?;
        self.calls.insert(function);
        Ok(Expr {
            ty: returns,
            kind: ExprKind::Call(
                function,
                ir::CallArgs::positional(vec![
                    receiver,
                    Expr {
                        ty: positional,
                        kind: ExprKind::List(args),
                    },
                    Expr {
                        ty: keywords,
                        kind: ExprKind::Dict {
                            key: Type::Str,
                            value: Type::Class(argument),
                            entries: kwargs,
                        },
                    },
                ]),
            ),
        })
    }
}
