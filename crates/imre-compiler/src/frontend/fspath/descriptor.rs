//! Bind the type's path protocol descriptor before calling it with no arguments.

use super::*;
use crate::classes::MethodKind;

/// The receiver is already evaluated; this AST supplies the normal binder's call site.
pub(in crate::frontend) fn protocol_call(at: &ast::Expr, name: &str) -> ast::ExprCall {
    let range = at.range();
    ast::ExprCall {
        node_index: ast::AtomicNodeIndex::NONE,
        range_start: range.start(),
        func: Box::new(ast::Expr::Attribute(ast::ExprAttribute {
            node_index: ast::AtomicNodeIndex::NONE,
            range,
            value: Box::new(at.clone()),
            attr: ast::Identifier::new(name, range),
            ctx: ast::ExprContext::Load,
        })),
        arguments: ast::Arguments {
            range,
            node_index: ast::AtomicNodeIndex::NONE,
            args: Default::default(),
            keywords: Default::default(),
        },
    }
}

impl Checker<'_> {
    /// A method may be static, class-bound, or use ordinary positional or keyword defaults.
    pub(super) fn call_fspath_descriptor(
        &mut self,
        class: usize,
        receiver: Expr,
        signature: &Signature,
        at: &ast::Expr,
        syscall: Option<(&str, &str)>,
    ) -> Result<Expr, String> {
        let call = protocol_call(at, "__fspath__");
        if signature.method.unwrap().kind(&self.classes.declarations) == MethodKind::Property {
            return self.call_fspath_property(class, receiver, &call, syscall);
        }
        self.call_path_method(&call, receiver, signature, Type::Str)
    }

    /// Bind an ordinary native method called by a path protocol, including defaults.
    pub(in crate::frontend) fn call_path_method(
        &mut self,
        call: &ast::ExprCall,
        receiver: Expr,
        signature: &Signature,
        on_error: Type,
    ) -> Result<Expr, String> {
        let method = signature.method.unwrap();
        let classes = &self.classes.declarations;
        let kind = method.kind(classes);
        let record = &classes.definitions[method.class];
        let owner = &classes.definitions[classes.ids[&record.method_origins[method.index]]].name;
        let declared = &record.methods[method.index];
        let mut scope = calls::ArgumentScope::default();
        if let Some(message) = call_binding_errors::arity_error(
            declared,
            Some(owner),
            usize::from(kind != MethodKind::Static),
            &[],
        ) {
            scope.hold(self, receiver);
            return Ok(scope.finish(self.sys_type_error(call, on_error, message)?));
        }
        let bound = if kind == MethodKind::Instance {
            Some(receiver)
        } else {
            scope.hold(self, receiver);
            None
        };
        let skip = usize::from(bound.is_some());
        Ok(scope.finish(self.source_call(call, signature, bound, skip)?))
    }

    /// CPython reports a missing protocol if descriptor lookup fails. Exceptions
    /// from calling a successfully retrieved descriptor propagate separately.
    fn call_fspath_property(
        &mut self,
        class: usize,
        receiver: Expr,
        call: &ast::ExprCall,
        syscall: Option<(&str, &str)>,
    ) -> Result<Expr, String> {
        let mut scope = calls::ArgumentScope::default();
        let receiver = scope.hold(self, receiver);
        let attribute = call.func.as_attribute_expr().unwrap();
        let getter = self.direct_class_attribute(attribute, class, Some(receiver.clone()), None)?;
        let slot = self.hidden(getter.ty);
        let failed = self.hidden(Type::Bool);
        let missing = self.fspath_type_error(receiver, None, syscall);
        let result = self.invoke_path_value(call, self.local(slot), Some(&missing), Type::Str)?;
        let flag = |value| Expr {
            ty: Type::Bool,
            kind: ExprKind::Bool(value),
        };
        Ok(scope.finish(Expr {
            ty: result.ty,
            kind: ExprKind::Scope {
                body: vec![
                    Statement::Assign(failed, flag(false)),
                    Statement::Try {
                        body: vec![Statement::Assign(slot, getter)],
                        handlers: vec![ir::ExceptHandler {
                            kinds: vec![],
                            name: None,
                            body: vec![Statement::Assign(failed, flag(true))],
                        }],
                        orelse: vec![],
                        finalbody: vec![],
                    },
                    Statement::If(self.local(failed), vec![Statement::Expr(missing)], vec![]),
                ],
                value: Box::new(result),
                clear: vec![slot, failed],
            },
        }))
    }

    /// Invoke a retrieved descriptor or instance field. Only the special path
    /// protocol treats None as an absent method.
    pub(in crate::frontend) fn invoke_path_value(
        &mut self,
        call: &ast::ExprCall,
        value: Expr,
        missing: Option<&Expr>,
        on_error: Type,
    ) -> Result<Expr, String> {
        if let Some(selected) =
            self.map_path_protocol_arms(&value, &call.func, |checker, arm| {
                checker.invoke_path_value(call, arm, missing, on_error)
            })?
        {
            return Ok(selected);
        }
        if value.ty == Type::None
            && let Some(missing) = missing
        {
            return Ok(missing.clone());
        }
        if self.types.signature(value.ty).is_some() {
            return self.invoke(call, value);
        }
        let actual = self.path_argument_type_name(value.ty);
        Ok(Self::path_operand_type_error(
            value,
            on_error,
            format!("'{actual}' object is not callable"),
        ))
    }
}
