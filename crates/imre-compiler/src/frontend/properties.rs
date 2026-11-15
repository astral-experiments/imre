//! Assignment to a fixed property uses the concrete receiver's descriptor.

use super::*;

impl<'a> Checker<'a> {
    /// Frozen instances retain their existing assignment diagnostic.
    pub(super) fn writable_property(
        &self,
        ty: Type,
        attr: &ast::ExprAttribute,
    ) -> Option<(usize, &'a Signature)> {
        let (Type::Class(class) | Type::NamedException(class)) = ty else {
            return None;
        };
        let declaration = &self.classes.declarations.definitions[class];
        if declaration.dataclass.as_ref().is_some_and(|d| d.frozen) {
            return None;
        }
        Some((class, self.classes.setters[class].get(attr.attr.as_str())?))
    }

    /// Evaluate the RHS before the receiver, then call the concrete setter.
    pub(super) fn set_property(
        &mut self,
        (class, setter): (usize, &Signature),
        attr: &ast::ExprAttribute,
        receiver: Expr,
        value: Expr,
    ) -> Result<Statement, String> {
        let receiver = Self::exception_record(receiver);
        if !self.classes.has_concrete_subclasses(class) {
            return self.direct_setter(class, Some(setter), attr, receiver, value);
        }
        self.store_instance_data(class, attr, receiver, value)
    }

    pub(super) fn has_property(&self, class: usize, name: &str) -> bool {
        self.classes.methods[class].get(name).is_some_and(|s| {
            s.method.unwrap().kind(self.classes.declarations)
                == crate::classes::MethodKind::Property
        })
    }

    /// Discard the setter's return value. An override can remove the setter.
    pub(super) fn direct_setter(
        &mut self,
        class: usize,
        setter: Option<&Signature>,
        attr: &ast::ExprAttribute,
        receiver: Expr,
        value: Expr,
    ) -> Result<Statement, String> {
        let name = attr.attr.as_str();
        let Some(setter) = setter else {
            if self.has_property(class, name) {
                let owner = &self.classes.declarations.definitions[class].name;
                return Ok(Statement::raise(Expr {
                    ty: Type::Exception,
                    kind: ExprKind::Exception {
                        kind: exception_kind("AttributeError").unwrap(),
                        message: Some(Box::new(unpacking::string(format!(
                            "property '{name}' of '{owner}' object has no setter"
                        )))),
                    },
                }));
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
                    site: ir::CallSite::None,
                    exhaustive_enum_match: false,
                },
            ),
        }))
    }
}
