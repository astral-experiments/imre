//! A sealed string enum has one canonical member for each distinct value.

use super::*;

#[derive(Default)]
pub(crate) struct Members {
    pub(crate) plain: bool,
    pub(crate) values: Vec<(String, String)>,
    pub(crate) names: HashMap<String, usize>,
    indices: HashMap<String, usize>,
}

impl Members {
    pub(super) fn declare(
        &mut self,
        graph: &SourceGraph,
        module: &SourceModule,
        module_id: usize,
        member: &ast::StmtAssign,
        bindings: &HashSet<String>,
    ) -> Result<String, String> {
        let [ast::Expr::Name(target)] = member.targets.as_slice() else {
            return Err(module.error(member, "a StrEnum member requires one name"));
        };
        let name = target.id.as_str();
        if name.starts_with('_')
            || matches!(name, "name" | "value" | "mro")
            || bindings.contains(name)
        {
            return Err(module.error(target, "duplicate or reserved StrEnum member name"));
        }
        let value = match &*member.value {
            ast::Expr::StringLiteral(literal) => {
                crate::frontend::check_string(literal, &module.source)?;
                literal.value.to_str().to_owned()
            }
            ast::Expr::Name(previous)
                if let Some(&index) = self.names.get(previous.id.as_str()) =>
            {
                self.values[index].1.clone()
            }
            ast::Expr::Call(call)
                if call.arguments.args.is_empty()
                    && call.arguments.keywords.is_empty()
                    && !shadowed(&call.func, bindings)
                    && !self.plain
                    && graph.resolve(module_id, &call.func)
                        == Some(Binding::Intrinsic(Intrinsic::EnumAuto)) =>
            {
                if !name.is_ascii() {
                    return Err(
                        module.error(target, "StrEnum auto() requires an ASCII member name")
                    );
                }
                name.to_ascii_lowercase()
            }
            _ => {
                return Err(module.error(
                    &*member.value,
                    "StrEnum members require a literal string, auto(), or an earlier member",
                ));
            }
        };
        let index = *self.indices.entry(value.clone()).or_insert_with(|| {
            let index = self.values.len();
            self.values.push((name.to_owned(), value));
            index
        });
        self.names.insert(name.to_owned(), index);
        Ok(name.to_owned())
    }
}

fn shadowed(mut expression: &ast::Expr, bindings: &HashSet<String>) -> bool {
    while let ast::Expr::Attribute(attribute) = expression {
        expression = &attribute.value;
    }
    matches!(expression, ast::Expr::Name(name) if bindings.contains(name.id.as_str()))
}
