//! Text copied from a Python function declaration when a wrapper is created.
use super::*;

pub(super) struct FunctionText {
    pub module: usize,
    pub name: String,
    pub doc: Option<String>,
}

impl Checker<'_> {
    /// Read the docstring emitted at the active Python optimization level.
    pub(super) fn function_text(
        &self,
        module: usize,
        definition: &ast::StmtFunctionDef,
    ) -> FunctionText {
        let doc = if self.graph.python_optimization() < 2 {
            docstring(&definition.body)
        } else {
            None
        };
        FunctionText {
            module,
            name: definition.name.to_string(),
            doc,
        }
    }

    /// Resolve the declaration of an unbound source function before a wrapper replaces its binding.
    pub(super) fn declared_function_text(&self, value: &ast::Expr) -> Option<FunctionText> {
        if let Some(binding) = self.local_function_binding(value) {
            return Some(self.function_text(
                self.module,
                &binding.target(self.source, value).ok()?.definition,
            ));
        }
        if let Some(Binding::Definition(id)) = self.resolve(value)
            && let ast::Stmt::FunctionDef(def) =
                &self.graph.modules[id.module].parsed.syntax().body[id.statement]
        {
            return Some(self.function_text(id.module, def));
        }
        if let ast::Expr::Attribute(attribute) = value
            && let Some(class) = self.class_reference(&attribute.value)
            && let Some(method) = self.classes.methods[class]
                .get(attribute.attr.as_str())?
                .method
        {
            let owner = &self.classes.declarations.definitions[method.class];
            return Some(self.function_text(owner.definition.module, &owner.methods[method.index]));
        }
        matches!(value, ast::Expr::Lambda(_)).then(|| FunctionText {
            module: self.module,
            name: "<lambda>".into(),
            doc: None,
        })
    }
}

/// Return the normalized value of a namespace's leading string literal.
pub(crate) fn docstring(body: &[ast::Stmt]) -> Option<String> {
    Some(imre_ast::strings::clean_doc(
        docstring_literal(body)?.value.to_str(),
    ))
}

pub(crate) fn docstring_literal(body: &[ast::Stmt]) -> Option<&ast::ExprStringLiteral> {
    let ast::Stmt::Expr(stmt) = body.first()? else {
        return None;
    };
    let ast::Expr::StringLiteral(text) = &*stmt.value else {
        return None;
    };
    Some(text)
}
