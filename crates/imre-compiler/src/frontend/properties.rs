//! Assignment to a fixed property uses the concrete receiver's descriptor.

use super::*;

impl Checker<'_> {
    /// Frozen instances retain their existing assignment diagnostic.
    pub(super) fn writable_property(&self, ty: Type, attr: &ast::ExprAttribute) -> Option<usize> {
        let Type::Class(class) = ty else { return None };
        let declaration = &self.classes.declarations.definitions[class];
        (self.classes.setters[class].contains_key(attr.attr.as_str())
            && !declaration.dataclass.as_ref().is_some_and(|d| d.frozen))
        .then_some(class)
    }

    /// Evaluate the RHS before the receiver, then call the concrete setter.
    pub(super) fn set_property(
        &mut self,
        class: usize,
        attr: &ast::ExprAttribute,
        receiver: Expr,
        value: Expr,
    ) -> Result<Statement, String> {
        if !self.classes.has_concrete_subclasses(class) {
            return self.direct_setter(class, attr, receiver, value);
        }
        let (rhs, object) = (self.hidden(value.ty), self.hidden(receiver.ty));
        let mut branches = None;
        for layout in self.classes.concrete_layouts(class).into_iter().rev() {
            let view = Expr {
                ty: Type::Class(layout),
                kind: ExprKind::Builtin(Builtin::RecordView, vec![self.local(object)]),
            };
            let put = self.direct_setter(layout, attr, view, self.local(rhs))?;
            branches = Some(match branches {
                None => put,
                Some(other) => Statement::If(
                    Expr {
                        ty: Type::Bool,
                        kind: ExprKind::Builtin(
                            Builtin::ExactRecord(layout),
                            vec![self.local(object)],
                        ),
                    },
                    vec![put],
                    vec![other],
                ),
            });
        }
        Ok(Statement::Scope {
            body: vec![
                Statement::Assign(rhs, value),
                Statement::Assign(object, receiver),
                branches.expect("dispatch has a concrete subclass"),
            ],
            clear: vec![rhs, object],
        })
    }

    /// Discard the setter's return value. An override can remove the setter.
    fn direct_setter(
        &mut self,
        class: usize,
        attr: &ast::ExprAttribute,
        receiver: Expr,
        value: Expr,
    ) -> Result<Statement, String> {
        let name = attr.attr.as_str();
        let Some(setter) = self.classes.setters[class].get(name) else {
            if self.classes.methods[class].get(name).is_some_and(|s| {
                s.method.unwrap().kind(&self.classes.declarations)
                    == crate::classes::MethodKind::Property
            }) {
                let owner = &self.classes.declarations.definitions[class].name;
                return Ok(Statement::Raise(Some(Expr {
                    ty: Type::Exception,
                    kind: ExprKind::Exception {
                        kind: exception_kind("AttributeError").unwrap(),
                        message: Some(Box::new(unpacking::string(format!(
                            "property '{name}' of '{owner}' object has no setter"
                        )))),
                    },
                })));
            }
            let (receiver, index, ty) = self.writable_field_value(attr, receiver)?;
            return Ok(Statement::SetField(
                receiver,
                index,
                self.coerce(value, ty, attr)?,
            ));
        };
        let expected = if let Some(inference) = self.inference {
            inference.learn_parameter(setter.id, 1, value.ty);
            inference.parameter(setter.id, 1).unwrap()
        } else {
            setter.params[1].ty
        };
        let value = self.coerce(value, expected, attr)?;
        self.calls.insert(setter.id);
        Ok(Statement::Expr(Expr {
            ty: setter.returns,
            kind: ExprKind::Call(
                setter.id,
                ir::CallArgs {
                    values: vec![value, receiver],
                    order: vec![1, 0],
                    exhaustive_enum_match: false,
                },
            ),
        }))
    }
}
