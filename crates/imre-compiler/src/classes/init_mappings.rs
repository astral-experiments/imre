//! Infer the layout of a dictionary copied from a literal factory in __init__.

use super::*;

fn self_attribute<'a>(expr: &'a ast::Expr, receiver: &str) -> Option<&'a str> {
    let attr = expr.as_attribute_expr()?;
    (attr.value.as_name_expr()?.id == receiver).then(|| attr.attr.as_str())
}

fn called_attribute(expr: &ast::Expr) -> Option<&ast::ExprAttribute> {
    let call = expr.as_call_expr()?;
    call.arguments.is_empty().then_some(())?;
    call.func.as_attribute_expr()
}

fn copied_factory<'a>(expr: &'a ast::Expr, receiver: &str) -> Option<&'a str> {
    let copy = called_attribute(expr)?;
    if copy.attr.as_str() != "copy" {
        return None;
    }
    let call = copy.value.as_call_expr()?;
    call.arguments.is_empty().then_some(())?;
    self_attribute(&call.func, receiver)
}

fn literal_type(expr: &ast::Expr) -> Option<String> {
    let name = match expr {
        ast::Expr::StringLiteral(_) => "str",
        ast::Expr::BytesLiteral(_) => "bytes",
        ast::Expr::BooleanLiteral(_) => "bool",
        ast::Expr::NoneLiteral(_) => "None",
        ast::Expr::NumberLiteral(number) => match number.value {
            ast::Number::Int(_) => "int",
            ast::Number::Float(_) => "float",
            ast::Number::Complex { .. } => return None,
        },
        ast::Expr::Tuple(tuple) if !tuple.elts.is_empty() => {
            let fields = tuple
                .elts
                .iter()
                .map(literal_type)
                .collect::<Option<Vec<_>>>()?;
            return Some(format!("tuple[{}]", fields.join(", ")));
        }
        _ => return None,
    };
    Some(name.to_owned())
}

/// The key representation must be uniform. Values that disagree need individual tags.
fn string_mapping(dict: &ast::ExprDict, locals: &HashMap<&str, String>) -> Option<String> {
    let element = |expr: &ast::Expr| {
        expr.as_name_expr()
            .and_then(|n| locals.get(n.id.as_str()).cloned())
            .or_else(|| literal_type(expr))
    };
    let mut common = None;
    for item in &dict.items {
        if element(item.key.as_ref()?)? != "str" {
            return None;
        }
        let value = element(&item.value)?;
        common = Some(match common {
            Some(previous) if previous != value => "object".to_owned(),
            _ => value,
        });
    }
    common
}

fn literal_mapping(method: &ast::StmtFunctionDef) -> Option<String> {
    if method.is_async
        || method.parameters.len() != 1
        || !method.decorator_list.is_empty()
        || !init_hooks::returns_literal(method)
    {
        return None;
    }
    let ast::Stmt::Return(last) = method.body.last()? else {
        return None;
    };
    string_mapping(last.value.as_ref()?.as_dict_expr()?, &HashMap::new())
}

/// Recover the value type from the factory's literals, independently of override return types.
/// Only the newly allocated copy occupies this field. A heterogeneous factory keeps tagged
/// values in the base class, while homogeneous overrides retain their precise dictionaries.
pub(super) fn infer(
    graph: &SourceGraph,
    module: usize,
    init: usize,
    methods: &mut [ClassMethod],
    fields: &mut Vec<Field>,
) {
    let initializer = &methods[init];
    let Some(param) = initializer.parameters.iter_non_variadic_params().next() else {
        return;
    };
    let receiver = param.name().as_str();
    let mut inferred_returns = Vec::new();
    let mut mappings: HashMap<&str, String> = HashMap::new();
    let mut locals = HashMap::new();
    for stmt in &initializer.body {
        let ast::Stmt::Assign(assign) = stmt else {
            locals.clear();
            continue;
        };
        // Do not reuse a pair's types if either local has been rebound.
        let mut targets = Vec::new();
        for t in &assign.targets {
            initializer_targets(t, &mut targets);
        }
        for t in targets {
            if let Some(n) = t.as_name_expr() {
                locals.remove(n.id.as_str());
            }
        }
        if ast::helpers::any_over_body(std::slice::from_ref(stmt), ast::Expr::is_named_expr) {
            locals.clear();
            continue;
        }
        let [target] = assign.targets.as_slice() else {
            continue;
        };
        if let Some(pair) = target.as_tuple_expr()
            && let [key, value] = pair.elts.as_slice()
            && let (Some(key), Some(value)) = (key.as_name_expr(), value.as_name_expr())
            && key.id != value.id
            && let Some(pop) = called_attribute(&assign.value)
            && pop.attr.as_str() == "popitem"
            && let Some(source) = self_attribute(&pop.value, receiver)
            && let Some(element) = mappings.get(source)
        {
            locals.insert(key.id.as_str(), "str".to_owned());
            locals.insert(value.id.as_str(), element.clone());
            continue;
        }
        let Some(name) = self_attribute(target, receiver) else {
            continue;
        };
        if fields.iter().any(|f| f.name == name) {
            continue;
        }
        let factory = copied_factory(&assign.value, receiver)
            .and_then(|factory| methods.iter().position(|m| m.name.as_str() == factory));
        let element = if let Some(index) = factory {
            let method = &methods[index];
            if method.returns.is_some() {
                continue;
            }
            literal_mapping(method)
        } else if let Some(dict) = assign.value.as_dict_expr()
            && dict.items.iter().all(|entry| {
                entry.key.as_ref().is_some_and(ast::Expr::is_name_expr)
                    && entry.value.is_name_expr()
            })
        {
            string_mapping(dict, &locals)
        } else {
            None
        };
        let Some(element) = element else { continue };
        let spelling = format!("dict[str, {element}]");
        let annotation = ruff_python_parser::parse_expression(&spelling)
            .expect("builtin dictionary annotation")
            .expr()
            .clone();
        if ast::helpers::any_over_expr(&annotation, |part| {
            part.is_name_expr() && graph.resolve_type(module, part).is_some()
        }) {
            continue;
        }
        if element == "object"
            && let Some(index) = factory
        {
            inferred_returns.push((index, annotation.clone()));
        }
        mappings.insert(name, element);
        fields.push(Field {
            module,
            name: name.to_owned(),
            annotation,
            weak_proxy: false,
            default: None,
            kw_only: false,
            repr: true,
            compare: true,
        });
    }
    for (index, annotation) in inferred_returns {
        Arc::make_mut(&mut methods[index].function).returns = Some(Box::new(annotation));
    }
}
