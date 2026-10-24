//! Resolve one concrete layout lineage and infer straight-line constructor fields.

use super::*;

pub(super) fn resolve(
    graph: &SourceGraph,
    nodes: &[(DefinitionId, &ast::StmtClassDef)],
    ids: &HashMap<DefinitionId, usize>,
) -> Result<Vec<Class>, String> {
    fn build(
        id: usize,
        graph: &SourceGraph,
        nodes: &[(DefinitionId, &ast::StmtClassDef)],
        ids: &HashMap<DefinitionId, usize>,
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
                match graph.resolve(definition.module, argument) {
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
                        build(next, graph, nodes, ids, classes, path)?;
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
        classes[id] = Some(collect_class(
            graph,
            module,
            definition,
            node,
            base.map(|i| (i, classes[i].as_ref().unwrap())),
        )?);
        path.pop();
        Ok(())
    }
    let mut classes = (0..nodes.len()).map(|_| None).collect::<Vec<_>>();
    let mut path = Vec::new();
    for id in 0..nodes.len() {
        build(id, graph, nodes, ids, &mut classes, &mut path)?;
    }
    Ok(classes.into_iter().map(Option::unwrap).collect())
}

/// Matches only zero-argument super bound by the surrounding instance initializer.
pub(crate) fn super_init(call: &ast::ExprCall) -> bool {
    matches!(&*call.func, ast::Expr::Attribute(method) if method.attr.as_str() == "__init__"
        && matches!(&*method.value, ast::Expr::Call(s) if s.arguments.is_empty()
            && matches!(&*s.func, ast::Expr::Name(name) if name.id == "super")))
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
            _ => None,
        };
        if let Some(annotation) = ann {
            fields.push(Field {
                module,
                name: target.attr.to_string(),
                annotation,
                default: None,
                kw_only: false,
                repr: true,
                compare: true,
            });
        }
    }
    Ok(())
}
