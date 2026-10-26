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
        let doc = if self.graph.python_optimization() < 2
            && let Some(ast::Stmt::Expr(stmt)) = definition.body.first()
            && let ast::Expr::StringLiteral(text) = &*stmt.value
        {
            Some(clean_doc(text.value.to_str()))
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

/// CPython expands tabs at eight-character stops, then removes the common
/// margin after the first line. Blank lines and the final newline survive.
fn clean_doc(doc: &str) -> String {
    let mut expanded = String::with_capacity(doc.len());
    let mut column = 0;
    for c in doc.chars() {
        if c == '\t' {
            let width = 8 - column % 8;
            expanded.extend(std::iter::repeat_n(' ', width));
            column += width;
        } else {
            expanded.push(c);
            column = if matches!(c, '\r' | '\n') {
                0
            } else {
                column + 1
            };
        }
    }
    let mut lines = expanded.split('\n');
    let first = lines.next().unwrap_or_default().trim_start_matches(' ');
    let margin = lines
        .clone()
        .filter_map(|line| {
            let content = line.trim_start_matches(' ');
            (!content.is_empty()).then_some(line.len() - content.len())
        })
        .min()
        .unwrap_or(0);
    let mut cleaned = first.to_owned();
    for line in lines {
        cleaned.push('\n');
        let indent = line.len() - line.trim_start_matches(' ').len();
        cleaned.push_str(&line[margin.min(indent)..]);
    }
    cleaned
}
