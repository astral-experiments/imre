//! Recognizes sealed dataclass declarations and synthesizes ordinary structural initializers.

use std::collections::HashSet;

use ruff_python_ast as ast;
use ruff_text_size::Ranged;

use super::Field;
use crate::modules::{Binding, DefinitionId, Intrinsic, SourceGraph, SourceModule};

#[derive(Clone, Copy)]
pub(crate) struct DataclassOptions {
    pub(crate) value_struct: bool,
    pub(crate) init: bool,
    pub(crate) eq: bool,
    pub(crate) repr: bool,
    pub(crate) frozen: bool,
    pub(crate) kw_only: bool,
    pub(crate) slots: bool,
}

#[derive(Clone)]
pub(crate) enum FieldDefault {
    Scalar(ast::Expr),
    Factory(ContainerFactory),
    Record(DefinitionId),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ContainerFactory {
    Counter,
    List,
    Dict,
    Set,
}

/// Decorator identity follows imports and re-exports, never the spelling of an alias.
pub(super) fn options(
    graph: &SourceGraph,
    module_id: usize,
    node: &ast::StmtClassDef,
) -> Result<Option<DataclassOptions>, String> {
    let module = &graph.modules[module_id];
    let [decorator] = node.decorator_list.as_slice() else {
        return if node.decorator_list.is_empty() {
            Ok(None)
        } else {
            Err(module.error(node, "class decorators cannot be stacked"))
        };
    };
    let (target, arguments) = match &decorator.expression {
        ast::Expr::Call(call) => (&*call.func, Some(&call.arguments)),
        expression => (expression, None),
    };
    let binding = graph.resolve(module_id, target);
    let value_struct = binding == Some(Binding::Intrinsic(Intrinsic::Struct));
    if value_struct && arguments.is_some() {
        return Err(module.error(decorator, "use bare @imre.struct without options"));
    }
    if !value_struct && binding != Some(Binding::Intrinsic(Intrinsic::Dataclass)) {
        return Err(module.error(
            decorator,
            "only the sealed @dataclasses.dataclass decorator is supported",
        ));
    }
    let mut options = DataclassOptions {
        value_struct,
        init: true,
        eq: true,
        repr: true,
        frozen: value_struct,
        kw_only: false,
        slots: value_struct,
    };
    if let Some(arguments) = arguments {
        if !arguments.args.is_empty() {
            return Err(module.error(
                arguments,
                "dataclass decorator options must be keyword arguments",
            ));
        }
        let mut names = HashSet::new();
        for keyword in &arguments.keywords {
            let name = keyword_name(module, keyword, &mut names)?;
            let value = boolean(module, &keyword.value)?;
            match name {
                "init" => options.init = value,
                "eq" => options.eq = value,
                "repr" => options.repr = value,
                "frozen" => options.frozen = value,
                "kw_only" => options.kw_only = value,
                // Every Imre record already has a fixed layout. These options
                // do not opt into dynamic attributes or runtime pattern metadata.
                "slots" => options.slots = value,
                "match_args" => {}
                "order" | "unsafe_hash" | "weakref_slot" if !value => {}
                _ => {
                    return Err(
                        module.error(keyword, &format!("unsupported dataclass option {name:?}"))
                    );
                }
            }
        }
    }
    Ok(Some(options))
}

/// Mutable defaults are constructed per call; a declaration never owns a shared container.
pub(super) fn field(
    graph: &SourceGraph,
    module_id: usize,
    node: &ast::StmtAnnAssign,
    name: &str,
    options: Option<&DataclassOptions>,
    class_names: &HashSet<String>,
) -> Result<Field, String> {
    let module = &graph.modules[module_id];
    let mut field = Field {
        module: module_id,
        name: name.to_owned(),
        annotation: (*node.annotation).clone(),
        default: None,
        weak_proxy: false,
        kw_only: options.is_some_and(|options| options.kw_only),
        repr: true,
        compare: true,
    };
    let Some(value) = node.value.as_deref() else {
        return Ok(field);
    };
    if let ast::Expr::Call(call) = value
        && graph.resolve(module_id, &call.func)
            == Some(Binding::Intrinsic(Intrinsic::DataclassField))
        && !shadowed(&call.func, class_names)
    {
        if !call.arguments.args.is_empty() {
            return Err(module.error(call, "dataclass field options must be keyword arguments"));
        }
        let mut names = HashSet::new();
        for keyword in &call.arguments.keywords {
            let name = keyword_name(module, keyword, &mut names)?;
            match name {
                "default" => {
                    if field.default.is_some() {
                        return Err(module.error(
                            keyword,
                            "field cannot specify both default and default_factory",
                        ));
                    }
                    scalar(module, &keyword.value)?;
                    field.default = Some(FieldDefault::Scalar(keyword.value.clone()));
                }
                "default_factory" => {
                    if field.default.is_some() {
                        return Err(module.error(
                            keyword,
                            "field cannot specify both default and default_factory",
                        ));
                    }
                    field.default = Some(factory(graph, module_id, &keyword.value, class_names)?);
                }
                "kw_only" => field.kw_only = boolean(module, &keyword.value)?,
                "repr" => field.repr = boolean(module, &keyword.value)?,
                "compare" => field.compare = boolean(module, &keyword.value)?,
                "init" if boolean(module, &keyword.value)? => {}
                "hash" if matches!(keyword.value, ast::Expr::NoneLiteral(_)) => {}
                _ => {
                    return Err(module.error(
                        keyword,
                        &format!("unsupported dataclass field option {name:?}"),
                    ));
                }
            }
        }
    } else {
        scalar(module, value)?;
        field.default = Some(FieldDefault::Scalar(value.clone()));
    }
    Ok(field)
}

fn keyword_name<'a>(
    module: &SourceModule,
    keyword: &'a ast::Keyword,
    names: &mut HashSet<&'a str>,
) -> Result<&'a str, String> {
    let name = keyword
        .arg
        .as_ref()
        .ok_or_else(|| module.error(keyword, "dataclass option expansion is unsupported"))?
        .as_str();
    if !names.insert(name) {
        return Err(module.error(keyword, &format!("duplicate dataclass option {name:?}")));
    }
    Ok(name)
}

fn boolean(module: &SourceModule, value: &ast::Expr) -> Result<bool, String> {
    if let ast::Expr::BooleanLiteral(value) = value {
        Ok(value.value)
    } else {
        Err(module.error(value, "dataclass flags require literal True or False"))
    }
}

/// Class-body assignments can shadow imported decorators and container builtins.
fn shadowed(expression: &ast::Expr, names: &HashSet<String>) -> bool {
    match expression {
        ast::Expr::Name(name) => names.contains(name.id.as_str()),
        ast::Expr::Attribute(attribute) => shadowed(&attribute.value, names),
        _ => false,
    }
}

fn factory(
    graph: &SourceGraph,
    module_id: usize,
    value: &ast::Expr,
    class_names: &HashSet<String>,
) -> Result<FieldDefault, String> {
    let constructor = if let ast::Expr::Lambda(lambda) = value
        && lambda
            .parameters
            .as_ref()
            .is_none_or(|params| params.is_empty())
        && let ast::Expr::Call(call) = &*lambda.body
        && call.arguments.is_empty()
    {
        Some(&*call.func)
    } else {
        (!shadowed(value, class_names)).then_some(value)
    };
    if let Some(Binding::Definition(id)) =
        constructor.and_then(|target| graph.resolve(module_id, target))
        && matches!(
            graph.modules[id.module].parsed.syntax().body[id.statement],
            ast::Stmt::ClassDef(_)
        )
    {
        if id.module == module_id
            && !matches!(value, ast::Expr::Lambda(_))
            && graph.modules[id.module].parsed.syntax().body[id.statement].start() >= value.start()
        {
            return Err(graph.modules[module_id]
                .error(value, "a direct default factory must already be defined"));
        }
        return Ok(FieldDefault::Record(id));
    }
    if !shadowed(value, class_names)
        && graph.resolve(module_id, value) == Some(Binding::Intrinsic(Intrinsic::Counter))
    {
        return Ok(FieldDefault::Factory(ContainerFactory::Counter));
    }
    if graph.resolve(module_id, value).is_none()
        && !shadowed(value, class_names)
        && let ast::Expr::Name(name) = value
    {
        match name.id.as_str() {
            "list" => return Ok(FieldDefault::Factory(ContainerFactory::List)),
            "dict" => return Ok(FieldDefault::Factory(ContainerFactory::Dict)),
            "set" => return Ok(FieldDefault::Factory(ContainerFactory::Set)),
            _ => {}
        }
    }
    Err(graph.modules[module_id].error(
        value,
        "default_factory requires a supported container or a zero-argument dataclass constructor",
    ))
}

/// Validate even defaults hidden behind an explicit initializer; later type
/// checking verifies that the literal exactly matches the declared field type.
fn scalar(module: &SourceModule, value: &ast::Expr) -> Result<(), String> {
    let valid = match value {
        ast::Expr::NumberLiteral(value) => {
            matches!(value.value, ast::Number::Int(_) | ast::Number::Float(_))
        }
        ast::Expr::BooleanLiteral(_)
        | ast::Expr::StringLiteral(_)
        | ast::Expr::BytesLiteral(_)
        | ast::Expr::NoneLiteral(_) => true,
        ast::Expr::UnaryOp(value)
            if matches!(value.op, ast::UnaryOp::UAdd | ast::UnaryOp::USub) =>
        {
            matches!(&*value.operand, ast::Expr::NumberLiteral(value) if matches!(value.value, ast::Number::Int(_) | ast::Number::Float(_)))
        }
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err(module.error(value, "dataclass defaults must be scalar literals; use field(default_factory=list, dict, or set) for fresh containers"))
    }
}

/// Synthesized nodes retain declaration ranges so ordinary checker diagnostics
/// always point into the user's file. Keyword-only parameters move after all
/// positional parameters without changing field initialization order.
pub(super) fn initializer(
    module: &SourceModule,
    node: &ast::StmtClassDef,
    fields: &[Field],
) -> Result<ast::StmtFunctionDef, String> {
    let range = node.range;
    // Match dataclasses' collision avoidance for a user field named self.
    let receiver = if fields.iter().any(|field| field.name == "self") {
        "__dataclass_self__"
    } else {
        "self"
    };
    let mut parameters = ast::Parameters {
        range,
        ..Default::default()
    };
    parameters.args.push(ast::ParameterWithDefault {
        range,
        node_index: ast::AtomicNodeIndex::NONE,
        parameter: ast::Parameter {
            range,
            node_index: ast::AtomicNodeIndex::NONE,
            name: ast::Identifier::new(receiver, range),
            annotation: None,
        },
        default: None,
    });
    let mut default_seen = false;
    for field in fields {
        if !field.kw_only {
            if default_seen && field.default.is_none() {
                return Err(module.error(
                    &field.annotation,
                    "a required positional dataclass field cannot follow a field with a default",
                ));
            }
            default_seen |= field.default.is_some();
        }
        let range = field.annotation.range();
        let parameter = ast::ParameterWithDefault {
            range,
            node_index: ast::AtomicNodeIndex::NONE,
            parameter: ast::Parameter {
                range,
                node_index: ast::AtomicNodeIndex::NONE,
                name: ast::Identifier::new(field.name.as_str(), range),
                annotation: Some(Box::new(field.annotation.clone())),
            },
            // Factories become fresh-container defaults in the closed signature.
            default: match &field.default {
                Some(FieldDefault::Scalar(value)) => Some(Box::new(value.clone())),
                _ => None,
            },
        };
        if field.kw_only {
            parameters.kwonlyargs.push(parameter);
        } else {
            parameters.args.push(parameter);
        }
    }
    let body = fields
        .iter()
        .map(|field| {
            let range = field.annotation.range();
            let name = |value: &str| {
                ast::Expr::Name(ast::ExprName {
                    node_index: ast::AtomicNodeIndex::NONE,
                    range,
                    id: value.into(),
                    ctx: ast::ExprContext::Load,
                })
            };
            ast::Stmt::Assign(ast::StmtAssign {
                node_index: ast::AtomicNodeIndex::NONE,
                range,
                targets: vec![ast::Expr::Attribute(ast::ExprAttribute {
                    node_index: ast::AtomicNodeIndex::NONE,
                    range,
                    value: Box::new(name(receiver)),
                    attr: ast::Identifier::new(field.name.as_str(), range),
                    ctx: ast::ExprContext::Store,
                })],
                value: Box::new(name(&field.name)),
            })
        })
        .collect();
    Ok(ast::StmtFunctionDef {
        node_index: ast::AtomicNodeIndex::NONE,
        range,
        is_async: false,
        decorator_list: Default::default(),
        name: ast::Identifier::new("__init__", range),
        type_params: None,
        parameters: Box::new(parameters),
        returns: Some(Box::new(ast::Expr::NoneLiteral(ast::ExprNoneLiteral {
            node_index: ast::AtomicNodeIndex::NONE,
            range,
        }))),
        body,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::classes::{Classes, MethodKind};

    #[test]
    fn imported_decorator_aliases_preserve_keyword_only_and_factory_contracts() {
        let source = r#"from dataclasses import dataclass as record, field as member
@record(slots=True, kw_only=True)
class Item:
    title: str
    count: int = member(default=3, kw_only=False)
    values: list[int] = member(default_factory=list, repr=False, compare=False)
"#;
        let graph = SourceGraph::single(source).unwrap();
        let classes = Classes::collect(&graph).unwrap();
        let class = &classes.definitions[0];
        assert!(class.generated_initializer);
        assert_eq!(class.fields[0].name, "title");
        assert_eq!(class.fields[1].name, "count");
        assert!(matches!(
            class.fields[2].default,
            Some(FieldDefault::Factory(ContainerFactory::List))
        ));
        assert!(!class.fields[2].repr);
        assert!(!class.fields[2].compare);
        let parameters = &class.methods[class.initializer.unwrap()].parameters;
        assert_eq!(parameters.args.len(), 2);
        assert_eq!(parameters.args[1].name().as_str(), "count");
        assert_eq!(parameters.kwonlyargs[0].name().as_str(), "title");
        assert_eq!(parameters.kwonlyargs[1].name().as_str(), "values");
    }

    #[test]
    fn generated_initializer_can_bind_a_field_named_self() {
        let graph = SourceGraph::single(
            "from dataclasses import dataclass\n@dataclass\nclass Item:\n    self: int\n",
        )
        .unwrap();
        let classes = Classes::collect(&graph).unwrap();
        let class = &classes.definitions[0];
        let parameters = &class.methods[class.initializer.unwrap()].parameters;
        assert_eq!(parameters.args[0].name().as_str(), "__dataclass_self__");
        assert_eq!(parameters.args[1].name().as_str(), "self");
    }

    #[test]
    fn parenthesized_annotations_do_not_silently_become_dataclass_fields() {
        let graph = SourceGraph::single(
            "from dataclasses import dataclass\n@dataclass\nclass Item:\n    (value): int = 3\n",
        )
        .unwrap();
        let error = Classes::collect(&graph).err().unwrap();
        assert!(error.contains("unparenthesized name annotation"), "{error}");
    }

    #[test]
    fn descriptors_are_classified_without_erasing_source_decorators() {
        let source = r#"from dataclasses import dataclass
@dataclass
class Item:
    value: int
    @property
    def doubled(self) -> int:
        return self.value * 2
    @classmethod
    def make(cls, value: int):
        return cls(value)
    @staticmethod
    def identity(value: int) -> int:
        return value
"#;
        let graph = SourceGraph::single(source).unwrap();
        let classes = Classes::collect(&graph).unwrap();
        let class = &classes.definitions[0];
        assert_eq!(
            class.method_kinds,
            [
                MethodKind::Property,
                MethodKind::Class,
                MethodKind::Static,
                MethodKind::Instance,
            ]
        );
        assert_eq!(class.methods[0].decorator_list.len(), 1);
    }
}
