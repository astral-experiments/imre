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

/// The native record store cannot call a descriptor inherited from a builtin base.
pub(super) fn inherited_descriptor(name: &str, mro: &[u32]) -> bool {
    let inherits =
        |base| crate::frontend::exception_kind(base).is_some_and(|kind| mro.contains(&kind));
    match name {
        "args"
        | "__cause__"
        | "__context__"
        | "__traceback__"
        | "__suppress_context__"
        | "__dict__"
        | "__class__"
        | "__weakref__" => true,
        "value" => inherits("StopIteration"),
        "name" => inherits("NameError") || inherits("AttributeError") || inherits("ImportError"),
        "obj" => inherits("AttributeError"),
        "errno" | "strerror" | "filename" | "filename2" | "characters_written" => {
            inherits("OSError")
        }
        "msg" | "path" | "name_from" => inherits("ImportError"),
        "encoding" | "object" | "start" | "end" | "reason" => inherits("UnicodeEncodeError"),
        _ => false,
    }
}

fn builtin_mro(kind: u32) -> Vec<u32> {
    let parent = match kind {
        0 => return vec![0],
        1 | 32 => 0,
        3 | 4 => 2,
        6 | 7 => 5,
        17 => 8,
        18 => 17,
        20..=24 | 44 | 45 | 50 | 51 | 68 => 12,
        46..=49 => 45,
        37 => 10,
        40 => 39, // UnboundLocalError derives from NameError.
        42 => 41,
        81 | 82 => 80, // RuntimeWarning and DeprecationWarning derive from Warning.
        _ => 1,
    };
    let mut out = vec![kind];
    out.extend(builtin_mro(parent));
    out
}

/// Exception inherits BaseException's initializer. A direct call passes the
/// receiver explicitly, and must resolve to the unshadowed builtin.
pub(crate) fn explicit_message_initializer<'a>(
    module: &SourceModule,
    call: &'a ast::ExprCall,
) -> Option<(&'a str, &'a [ast::Expr])> {
    let ast::Expr::Attribute(method) = call.func.as_ref() else {
        return None;
    };
    let base = method.value.as_name_expr()?.id.as_str();
    let (receiver, values) = call.arguments.args.split_first()?;
    (method.attr.as_str() == "__init__"
        && matches!(base, "Exception" | "BaseException")
        && !module.bindings.contains_key(base)
        && receiver
            .as_name_expr()
            .is_some_and(|receiver| receiver.id == "self")
        && call.arguments.keywords.is_empty())
    .then_some((base, values))
}

/// A fixed payload must establish its base arguments exactly once before it can escape.
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
    let calls = init.body.iter().filter(|stmt| {
        matches!(stmt, ast::Stmt::Expr(e) if matches!(&*e.value, ast::Expr::Call(c)
            if super_init(c) || init.name.as_str() == "__init__" && explicit_message_initializer(module, c).is_some()))
    }).count();
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
    /// A concrete descendant shares one compatible record prefix with its parent.
    pub storage_parent: HashMap<DefinitionId, DefinitionId>,
}
pub(super) fn collect(graph: &SourceGraph) -> Result<Declarations, String> {
    let mut ids: HashMap<DefinitionId, u32> = HashMap::new();
    let mut named: Vec<NamedException> = Vec::new();
    let mut layouts = HashSet::new();
    let mut storage_parent = HashMap::new();
    let mut implementation = HashMap::<DefinitionId, DefinitionId>::new();
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
                let mut storage = None;
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
                                match storage {
                                    None => storage = Some(parent),
                                    // Two pass-only branches can share exactly the same layout and methods.
                                    Some(earlier) if implementation[&earlier] == implementation[&parent] => {}
                                    Some(earlier) if named[(ids[&earlier] - FIRST_USER_EXCEPTION) as usize]
                                        .ancestors.contains(&id) => {}
                                    Some(earlier) if named[(id - FIRST_USER_EXCEPTION) as usize]
                                        .ancestors.contains(&ids[&earlier]) => storage = Some(parent),
                                    Some(_) => return Err(module.error(base, "multiple independent exception payload layouts are unsupported")),
                                }
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
                    if storage.is_none()
                        || node
                            .body
                            .iter()
                            .filter_map(ast::Stmt::as_function_def_stmt)
                            .any(|f| matches!(f.name.as_str(), "__init__" | "__post_init__"))
                    {
                        validate_layout(module, node)?;
                    }
                    layouts.insert(def);
                    implementation.insert(def, def);
                }
                if let Some(parent) = storage {
                    storage_parent.insert(def, parent);
                    layouts.insert(def);
                    let origin = implementation[&parent];
                    implementation.entry(def).or_insert(origin);
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
        storage_parent,
    })
}

/// Reserve the base exception arguments in the common prefix of every concrete descendant.
pub(super) fn install_argument_slot(c: &mut Class, module: &SourceModule) {
    // This ABI slot is not a Python identifier, constructor parameter,
    // dataclass comparison field, or repr field. The frontend gives
    // the tuple slot its native exception field type during layout.
    let tuple_args = c.forwarder.is_some() || c.methods.iter().filter(|m| matches!(m.name.as_str(), "__init__" | "__post_init__"))
                .flat_map(|m| &m.body).any(|stmt| matches!(stmt, ast::Stmt::Expr(e)
                    if matches!(&*e.value, ast::Expr::Call(call) if explicit_message_initializer(module, call).is_some()
                        || super_init(call) && call.arguments.args.len() != 1)));
    let annotation = ruff_python_parser::parse_expression("str | None")
        .expect("static annotation")
        .expr()
        .clone();
    c.fields.push(Field {
        module: c.definition.module,
        name: if tuple_args {
            EXCEPTION_ARGS_SLOT
        } else {
            "<exception message>"
        }
        .into(),
        annotation,
        default: None,
        kw_only: false,
        repr: false,
        compare: false,
    });
}
