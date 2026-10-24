//! Declaration-only exception subclasses retain nominal catch identity while
//! sharing the builtin exception object's storage and constructor.

use super::*;
use crate::types::NamedException;

pub(crate) const FIRST_USER_EXCEPTION: u32 = 256;

pub(super) fn collect(
    graph: &SourceGraph,
) -> Result<(HashMap<DefinitionId, u32>, Vec<NamedException>), String> {
    let mut ids = HashMap::new();
    let mut declarations: Vec<NamedException> = Vec::new();
    loop {
        let before = ids.len();
        for (module_id, module) in graph.modules.iter().enumerate() {
            for (statement, syntax) in module.parsed.syntax().body.iter().enumerate() {
                let ast::Stmt::ClassDef(node) = syntax else {
                    continue;
                };
                let definition = DefinitionId {
                    module: module_id,
                    statement,
                };
                if !graph.includes(definition) || ids.contains_key(&definition) {
                    continue;
                }
                let Some(arguments) = &node.arguments else {
                    continue;
                };
                let [base] = arguments.args.as_ref() else {
                    continue;
                };
                let (base, mut ancestors) = match graph.resolve(module_id, base) {
                    None => {
                        let Some(kind) = base
                            .as_name_expr()
                            .and_then(|n| crate::frontend::exception_kind(n.id.as_str()))
                        else {
                            continue;
                        };
                        (kind, Vec::new())
                    }
                    Some(Binding::Definition(parent)) => {
                        let Some(id) = ids.get(&parent) else {
                            continue;
                        };
                        let parent = &declarations[(id - FIRST_USER_EXCEPTION) as usize];
                        (parent.base, parent.ancestors.clone())
                    }
                    _ => continue,
                };
                if !arguments.keywords.is_empty() || !node.decorator_list.is_empty() || node.type_params.is_some()
                    || !node.body.iter().all(|line| matches!(line, ast::Stmt::Pass(_)) || matches!(line, ast::Stmt::Expr(expr) if matches!(&*expr.value, ast::Expr::StringLiteral(_)))) {
                    return Err(module.error(node, "exception subclasses support a docstring or pass and inherit the builtin constructor"));
                }
                // These rich catch-only builtins require distinct payloads.
                if matches!(base, 18 | 25..=32 | 34) {
                    return Err(module.error(node, "unsupported exception base"));
                }
                let id = FIRST_USER_EXCEPTION + declarations.len() as u32;
                ancestors.push(id);
                ids.insert(definition, id);
                declarations.push(NamedException {
                    id,
                    name: node.name.to_string(),
                    base,
                    ancestors,
                });
            }
        }
        if before == ids.len() {
            break;
        }
    }
    Ok((ids, declarations))
}
