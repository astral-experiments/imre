//! A stored generic assertion retains its receiver. Its operand types and
//! keyword names are bound by the same code as an ordinary TestCase call.

use super::*;

impl Checker<'_> {
    pub(super) fn bound_unittest_assertion(
        &mut self,
        attr: &ast::ExprAttribute,
        receiver: &Expr,
    ) -> Option<Expr> {
        let Type::Class(class) = receiver.ty else {
            return None;
        };
        let name = attr.attr.as_str();
        if !matches!(name, "assertEqual" | "assertListEqual" | "assertRaises")
            || !self.unittest_case(class)
        {
            return None;
        }
        // A user override was bound by normal method lookup before this point.
        let method = self.classes.methods[class].get(&format!("_bound_{name}"))?;
        self.calls.insert(method.id);
        Some(Expr {
            ty: method.returns,
            kind: ExprKind::Call(method.id, ir::CallArgs::positional(vec![receiver.clone()])),
        })
    }

    pub(super) fn stored_assertion_name(&self, ty: Type) -> Option<&'static str> {
        let Type::Class(class) = ty else { return None };
        let record = &self.classes.declarations.definitions[class];
        let module = &self.graph.modules[record.definition.module];
        if !module.bundled || module.name != "unittest" {
            return None;
        }
        match record.name.as_str() {
            "_BoundEqual" => Some("assertEqual"),
            "_BoundListEqual" => Some("assertListEqual"),
            "_BoundRaises" => Some("assertRaises"),
            _ => None,
        }
    }

    pub(super) fn invoke_bound_unittest(
        &mut self,
        call: &ast::ExprCall,
        callable: &Expr,
    ) -> Result<Option<Expr>, String> {
        let (class, name) = match (callable.ty, self.stored_assertion_name(callable.ty)) {
            (Type::Class(class), Some(name)) => (class, name),
            _ => return Ok(None),
        };
        let record = &self.classes.declarations.definitions[class];
        let field = record
            .fields
            .iter()
            .position(|f| f.name == "_receiver")
            .ok_or_else(|| error(self.source, call, "missing native assertion receiver"))?;
        let ty = self.classes.fields[class][field];
        let Type::Class(case) = ty else {
            return Err(error(
                self.source,
                call,
                "invalid native assertion receiver",
            ));
        };
        let receiver = Expr {
            ty,
            kind: ExprKind::Field(Box::new(callable.clone()), field),
        };
        let attribute = ast::ExprAttribute {
            node_index: ast::AtomicNodeIndex::NONE,
            range: call.range(),
            value: call.func.clone(),
            attr: ast::Identifier::new(name, call.range()),
            ctx: ast::ExprContext::Load,
        };
        self.class_method_call(call, &attribute, case, Some(receiver))
            .map(Some)
    }
}
