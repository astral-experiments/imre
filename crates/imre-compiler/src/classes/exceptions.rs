//! Resolve exception identities independently of ordinary class storage.
//! A user class can carry one sealed record layout and multiple marker bases.
use super::*;
use crate::types::NamedException;

pub(crate) const FIRST_USER_EXCEPTION: u32 = 256;

pub(super) fn alias(
    graph: &SourceGraph,
    module: usize,
    value: &ast::Expr,
    ids: &HashMap<DefinitionId, u32>,
) -> Option<ExceptionAlias> {
    match graph.resolve(module, value) {
        Some(Binding::Definition(def)) if ids.contains_key(&def) => {
            Some(ExceptionAlias::Named(def))
        }
        None => value.as_name_expr().and_then(|n| {
            let kind = crate::frontend::exception_kind(n.id.as_str())?;
            Some(ExceptionAlias::Builtin {
                kind,
                name: n.id.to_string(),
            })
        }),
        _ => None,
    }
}

/// Only a resolved ClassVar[type[Exception]] represents this static alias;
/// ordinary annotated fields remain part of the instance layout.
pub(super) fn constructor_annotation(graph: &SourceGraph, module: usize, ann: &ast::Expr) -> bool {
    let ast::Expr::Subscript(outer) = ann else {
        return false;
    };
    let ast::Expr::Subscript(inner) = outer.slice.as_ref() else {
        return false;
    };
    graph.resolve_type(module, &outer.value) == Some(Binding::Intrinsic(Intrinsic::ClassVar))
        && inner.value.as_name_expr().is_some_and(|v| v.id == "type")
        && graph.resolve_type(module, &inner.value).is_none()
        && inner
            .slice
            .as_name_expr()
            .is_some_and(|v| v.id == "Exception" || v.id == "BaseException")
        && graph.resolve_type(module, &inner.slice).is_none()
}

fn marker(node: &ast::StmtClassDef) -> bool {
    node.decorator_list.is_empty() && node.body.iter().all(|line| {
        matches!(line, ast::Stmt::Pass(_))
            || matches!(line,ast::Stmt::Expr(e) if matches!(&*e.value,ast::Expr::StringLiteral(_)))
    })
}

fn builtin_mro(kind: u32) -> Vec<u32> {
    let parent = match kind {
        0 => return vec![0],
        1 | 32 => 0,
        3 | 4 => 2,
        6 | 7 => 5,
        17 => 8,
        18 => 17,
        20..=24 => 12,
        37 => 10,
        42 => 41,
        _ => 1,
    };
    let mut out = vec![kind];
    out.extend(builtin_mro(parent));
    out
}

/// Every admitted payload constructor explicitly establishes its string argument.
/// BaseException.__new__ also accepts arbitrary positional arguments, whose
/// heterogeneous tuple is outside this fixed-layout subset.
fn validate_layout(module: &SourceModule, node: &ast::StmtClassDef) -> Result<(), String> {
    let methods = node
        .body
        .iter()
        .filter_map(ast::Stmt::as_function_def_stmt)
        .collect::<Vec<_>>();
    let initialization = methods
        .iter()
        .find(|m| m.name.as_str() == "__init__")
        .or_else(|| methods.iter().find(|m| m.name.as_str() == "__post_init__"));
    let Some(init) = initialization else {
        return Err(module.error(node,"an exception payload needs an explicit initializer or dataclass post-init which calls super().__init__(message)"));
    };
    let calls=init.body.iter().filter(|stmt|matches!(stmt,ast::Stmt::Expr(e) if matches!(&*e.value,ast::Expr::Call(c) if super_init(c)))).count();
    if calls != 1
        || init.body.iter().any(|s| {
            !matches!(
                s,
                ast::Stmt::Expr(_) | ast::Stmt::Assign(_) | ast::Stmt::Pass(_)
            )
        })
    {
        return Err(module.error(init,"an exception payload must initialize its message exactly once in a straight-line constructor"));
    }
    Ok(())
}

/// C3 merge rejects inconsistent or repeated bases before any descriptor is emitted.
fn linearize(direct: &[u32], mut paths: Vec<Vec<u32>>) -> Option<Vec<u32>> {
    if direct.iter().collect::<HashSet<_>>().len() != direct.len() {
        return None;
    }
    paths.push(direct.to_vec());
    let mut out = Vec::new();
    while paths.iter().any(|p| !p.is_empty()) {
        let id = paths.iter().filter_map(|p| p.first()).find(|id| {
            paths
                .iter()
                .all(|p| !p.get(1..).unwrap_or_default().contains(id))
        })?;
        let id = *id;
        out.push(id);
        for path in &mut paths {
            if path.first() == Some(&id) {
                path.remove(0);
            }
        }
    }
    Some(out)
}

pub(super) struct Declarations {
    pub ids: HashMap<DefinitionId, u32>,
    pub named: Vec<NamedException>,
    pub layouts: HashSet<DefinitionId>,
}
pub(super) fn collect(graph: &SourceGraph) -> Result<Declarations, String> {
    let mut ids: HashMap<DefinitionId, u32> = HashMap::new();
    let mut named: Vec<NamedException> = Vec::new();
    let mut layouts = HashSet::new();
    loop {
        let before = ids.len();
        for (module_id, module) in graph.modules.iter().enumerate() {
            for (statement, syntax) in module.parsed.syntax().body.iter().enumerate() {
                let ast::Stmt::ClassDef(node) = syntax else {
                    continue;
                };
                let def = DefinitionId {
                    module: module_id,
                    statement,
                };
                if !graph.includes(def) || ids.contains_key(&def) {
                    continue;
                }
                let Some(arguments) = &node.arguments else {
                    continue;
                };
                if arguments.args.is_empty() {
                    continue;
                }
                let mut bases = Vec::new();
                let mut mros = Vec::new();
                for base in &arguments.args {
                    let (id, mro) = match graph.resolve(module_id, base) {
                        None => {
                            let Some(id) = base
                                .as_name_expr()
                                .and_then(|n| crate::frontend::exception_kind(n.id.as_str()))
                            else {
                                break;
                            };
                            if matches!(id, 18 | 25..=32 | 34) {
                                return Err(module.error(base, "unsupported exception base"));
                            }
                            (id, builtin_mro(id))
                        }
                        Some(Binding::Definition(parent)) => {
                            let Some(&id) = ids.get(&parent) else {
                                break;
                            };
                            if layouts.contains(&parent) {
                                return Err(module.error(
                                    base,
                                    "inheriting a user exception payload layout is unsupported",
                                ));
                            }
                            (
                                id,
                                named[(id - FIRST_USER_EXCEPTION) as usize]
                                    .ancestors
                                    .clone(),
                            )
                        }
                        _ => break,
                    };
                    bases.push(id);
                    mros.push(mro);
                }
                if bases.len() != arguments.args.len() {
                    continue;
                }
                if !arguments.keywords.is_empty() || node.type_params.is_some() {
                    return Err(module.error(
                        node,
                        "exception metaclasses and type parameters are unsupported",
                    ));
                }
                let mro = linearize(&bases, mros).ok_or_else(|| {
                    module.error(node, "inconsistent exception method resolution order")
                })?;
                let base = *mro
                    .iter()
                    .find(|&&kind| kind < FIRST_USER_EXCEPTION)
                    .unwrap();
                let id = FIRST_USER_EXCEPTION + named.len() as u32;
                let mut ancestors = vec![id];
                ancestors.extend(mro);
                if !marker(node) {
                    if matches!(base, 41 | 42) {
                        return Err(module.error(
                            node,
                            "custom import exception payload layouts are unsupported",
                        ));
                    }
                    validate_layout(module, node)?;
                    layouts.insert(def);
                }
                ids.insert(def, id);
                named.push(NamedException {
                    id,
                    name: node.name.to_string(),
                    base,
                    ancestors,
                });
            }
        }
        if ids.len() == before {
            break;
        }
    }
    Ok(Declarations {
        ids,
        named,
        layouts,
    })
}
