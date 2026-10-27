//! Resolve one concrete layout lineage and infer straight-line constructor fields.

use super::*;
use ruff_text_size::Ranged;

pub(super) fn resolve(
    graph: &SourceGraph,
    nodes: &[(DefinitionId, &ast::StmtClassDef)],
    ids: &HashMap<DefinitionId, usize>,
    exceptions: &exceptions::Declarations,
) -> Result<Vec<Class>, String> {
    fn build(
        id: usize,
        graph: &SourceGraph,
        nodes: &[(DefinitionId, &ast::StmtClassDef)],
        ids: &HashMap<DefinitionId, usize>,
        exceptions: &exceptions::Declarations,
        classes: &mut [Option<Class>],
        path: &mut Vec<usize>,
    ) -> Result<(), String> {
        if classes[id].is_some() {
            return Ok(());
        }
        let (definition, node) = nodes[id];
        let module = &graph.modules[definition.module];
        if path.contains(&id) || path.len() >= 64 {
            return Err(module.error(node, "cyclic or too deeply nested class lineage"));
        }
        path.push(id);
        let mut base: Option<usize> = None;
        if let Some(args) = &node.arguments {
            if !args.keywords.is_empty() {
                return Err(module.error(
                    node,
                    "class metaclasses and keyword arguments are unsupported",
                ));
            }
            for argument in &args.args {
                if mapping_base(graph, definition.module, argument)
                    || (args.args.len() == 1
                        && graph
                            .session
                            .builtin_object_base(definition.module, argument))
                {
                    continue;
                }
                let origin = match argument {
                    ast::Expr::Subscript(subscript)
                        if matches!(graph.resolve(definition.module, &subscript.value),
                        Some(Binding::Definition(id)) if graph.is_abstract_context_manager(id)) =>
                    {
                        &*subscript.value
                    }
                    other => other,
                };
                match graph.resolve(definition.module, origin) {
                    Some(Binding::Intrinsic(Intrinsic::StrEnum | Intrinsic::StandardEnum))
                        if args.args.len() == 1 => {}
                    Some(Binding::Intrinsic(Intrinsic::Abc)) => {}
                    Some(Binding::Definition(source)) if ids.contains_key(&source) => {
                        if source.module == definition.module
                            && source.statement >= definition.statement
                        {
                            return Err(
                                module.error(argument, "a base class must already be defined")
                            );
                        }
                        let next = ids[&source];
                        build(next, graph, nodes, ids, exceptions, classes, path)?;
                        if let Some(first) = base {
                            let parent: &Class = classes[first].as_ref().unwrap();
                            if !parent.bases.contains(&next) {
                                return Err(module.error(
                                    argument,
                                    "multiple independent base classes are unsupported",
                                ));
                            }
                        } else {
                            base = Some(next);
                        }
                    }
                    _ => {
                        return Err(
                            module.error(argument, "the base must be a known fixed-layout class")
                        );
                    }
                }
            }
        }
        let mut class = collect_class(
            graph,
            module,
            definition,
            node,
            base.map(|i| (i, classes[i].as_ref().unwrap())),
            exceptions.layouts.contains(&definition),
            &exceptions.ids,
        )?;
        if exceptions.layouts.contains(&definition) {
            let index = (exceptions.ids[&definition] - exceptions::FIRST_USER_EXCEPTION) as usize;
            let mro = &exceptions.named[index].ancestors;
            if let Some(field) = class
                .fields
                .iter()
                .find(|field| exceptions::inherited_descriptor(&field.name, mro))
            {
                return Err(module.error(
                    node,
                    &format!(
                        "a record field cannot store the inherited exception descriptor '{}'",
                        field.name
                    ),
                ));
            }
            if base.is_none() {
                exceptions::install_argument_slot(&mut class, module);
            }
        }
        classes[id] = Some(class);
        path.pop();
        Ok(())
    }
    let mut classes = (0..nodes.len()).map(|_| None).collect::<Vec<_>>();
    let mut path = Vec::new();
    for id in 0..nodes.len() {
        build(id, graph, nodes, ids, exceptions, &mut classes, &mut path)?;
    }
    Ok(classes.into_iter().map(Option::unwrap).collect())
}

/// Return the member and the name whose binding must be the builtin `super`.
pub(crate) fn super_method(call: &ast::ExprCall) -> Option<(&ast::ExprAttribute, &ast::Expr)> {
    let method = call.func.as_attribute_expr()?;
    let proxy = method.value.as_call_expr()?;
    (proxy.arguments.is_empty() && proxy.func.as_name_expr()?.id == "super")
        .then_some((method, &proxy.func))
}

pub(crate) fn super_init(call: &ast::ExprCall) -> bool {
    super_method(call).is_some_and(|(method, _)| method.attr.as_str() == "__init__")
}

/// Infer storage from an exact builtin value. The constructor still evaluates
/// the original expression, including every format argument and its errors.
fn literal_field_annotation(
    graph: &SourceGraph,
    module: usize,
    value: &ast::Expr,
) -> Option<ast::Expr> {
    let name = match value {
        ast::Expr::BooleanLiteral(_) => "bool",
        ast::Expr::StringLiteral(_) => "str",
        ast::Expr::Call(call)
            if let ast::Expr::Attribute(method) = &*call.func
                && method.attr.as_str() == "format"
                && matches!(&*method.value, ast::Expr::StringLiteral(_)) =>
        {
            "str"
        }
        _ => return None,
    };
    let annotation = ast::Expr::Name(ast::ExprName {
        node_index: ast::AtomicNodeIndex::NONE,
        range: value.range(),
        id: name.into(),
        ctx: ast::ExprContext::Load,
    });
    // An inferred builtin must not resolve to a user-defined class or type alias.
    graph
        .resolve_type(module, &annotation)
        .is_none()
        .then_some(annotation)
}

/// Fold a constructor attribute annotation into the fixed layout. Python does not
/// evaluate this annotation, so the checked method subsequently performs a normal store.
pub(super) fn annotated_fields(
    graph: &SourceGraph,
    module: usize,
    init: &mut ast::StmtFunctionDef,
    fields: &mut Vec<Field>,
) {
    let Some(receiver) = init.parameters.iter_non_variadic_params().next() else {
        return;
    };
    let params = init
        .parameters
        .iter_non_variadic_params()
        .map(|p| (p.name().as_str(), p.annotation()))
        .collect::<HashMap<_, _>>();
    let locals = crate::narrowing::assigned_names(&init.body);
    for statement in &mut init.body {
        let (target, value, annotation) = match &*statement {
            ast::Stmt::AnnAssign(a)
                if let (ast::Expr::Attribute(target), Some(value)) = (&*a.target, &a.value) =>
            {
                (target, &**value, Some(&*a.annotation))
            }
            ast::Stmt::Assign(a) if let [ast::Expr::Attribute(target)] = a.targets.as_slice() => {
                (target, &*a.value, None)
            }
            _ => continue,
        };
        if !matches!(&*target.value, ast::Expr::Name(n) if n.id.as_str() == receiver.name().as_str())
        {
            continue;
        }
        let proxy_arg = if let ast::Expr::Call(call) = value
            && graph.resolve(module, &call.func) == Some(Binding::Intrinsic(Intrinsic::WeakProxy))
        {
            let mut root = &*call.func;
            while let ast::Expr::Attribute(attr) = root {
                root = &attr.value;
            }
            root.as_name_expr()
                .filter(|n| !params.contains_key(n.id.as_str()) && !locals.contains(n.id.as_str()))
                .and_then(|_| call.arguments.args.first())
        } else {
            None
        };
        if let Some(field) = fields.iter_mut().find(|f| f.name == target.attr.as_str()) {
            field.weak_proxy |= proxy_arg.is_some();
        } else if let Some(annotation) = annotation.or_else(|| {
            let name = proxy_arg?.as_name_expr()?.id.as_str();
            params.get(name).copied().flatten()
        }) {
            fields.push(Field {
                module,
                name: target.attr.to_string(),
                annotation: annotation.clone(),
                weak_proxy: proxy_arg.is_some(),
                default: None,
                kw_only: false,
                repr: true,
                compare: true,
            });
        }
        if let ast::Stmt::AnnAssign(assign) = statement
            && let Some(value) = &assign.value
        {
            *statement = ast::Stmt::Assign(ast::StmtAssign {
                node_index: ast::AtomicNodeIndex::NONE,
                range: assign.range,
                targets: vec![(*assign.target).clone()],
                value: value.clone(),
            });
        }
    }
}

pub(super) fn infer_fields(
    graph: &SourceGraph,
    module: usize,
    init: &ast::StmtFunctionDef,
    fields: &mut Vec<Field>,
) -> Result<(), String> {
    let params = init
        .parameters
        .iter_non_variadic_params()
        .filter_map(|p| p.annotation().map(|ann| (p.name().as_str(), ann.clone())))
        .collect::<HashMap<_, _>>();
    for stmt in &init.body {
        let ast::Stmt::Assign(assign) = stmt else {
            continue;
        };
        let [ast::Expr::Attribute(target)] = assign.targets.as_slice() else {
            continue;
        };
        if !matches!(&*target.value, ast::Expr::Name(name) if name.id == "self")
            || fields
                .iter()
                .any(|field| field.name == target.attr.as_str())
        {
            continue;
        }
        let ann = match &*assign.value {
            ast::Expr::Name(name) => params.get(name.id.as_str()).cloned(),
            ast::Expr::Call(call)
                if matches!(&*call.func, ast::Expr::Name(name) if name.id == "list")
                    && graph.resolve(module, &call.func).is_none()
                    && call.arguments.keywords.is_empty() =>
            {
                let [ast::Expr::Name(input)] = call.arguments.args.as_ref() else {
                    continue;
                };
                let Some(ast::Expr::Subscript(annotated)) = params.get(input.id.as_str()) else {
                    continue;
                };
                let mut list = annotated.clone();
                list.value.clone_from(&call.func);
                Some(ast::Expr::Subscript(list))
            }
            ast::Expr::ListComp(comp) => {
                let [generator] = comp.generators.as_slice() else {
                    continue;
                };
                let (ast::Expr::Attribute(input), ast::Expr::Name(item)) =
                    (&generator.iter, &generator.target)
                else {
                    continue;
                };
                if !matches!(&*input.value, ast::Expr::Name(name) if name.id == "self") {
                    continue;
                }
                let Some(field) = fields
                    .iter()
                    .find(|field| field.name == input.attr.as_str())
                else {
                    continue;
                };
                let ast::Expr::Subscript(list) = &field.annotation else {
                    continue;
                };
                let preserving = matches!(&*comp.elt, ast::Expr::Name(name) if name.id == item.id)
                    || matches!(&*comp.elt, ast::Expr::Subscript(slice) if matches!(&*slice.slice, ast::Expr::Slice(_))
                        && matches!(&*slice.value, ast::Expr::Name(name) if name.id == item.id)
                        && matches!(&*list.slice, ast::Expr::Name(ty) if matches!(ty.id.as_str(), "str" | "bytes")));
                preserving.then(|| field.annotation.clone())
            }
            value => literal_field_annotation(graph, module, value),
        };
        if let Some(annotation) = ann {
            fields.push(Field {
                module,
                name: target.attr.to_string(),
                annotation,
                weak_proxy: false,
                default: None,
                kw_only: false,
                repr: true,
                compare: true,
            });
        }
    }
    Ok(())
}
