//! Collects fixed class shapes before nominal type resolution and executable lowering.

use std::collections::{HashMap, HashSet};

use ruff_python_ast as ast;
use ruff_python_ast::visitor::{Visitor, walk_expr};

use crate::ir::Type;
use crate::modules::{Binding, DefinitionId, Intrinsic, SourceGraph, SourceModule};

mod dataclasses;
mod enums;
pub(crate) use dataclasses::{ContainerFactory, DataclassOptions, FieldDefault};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MethodKind {
    Instance,
    Property,
    Class,
    Static,
}

pub(crate) struct Classes {
    pub(crate) definitions: Vec<Class>,
    pub(crate) ids: HashMap<DefinitionId, usize>,
}

pub(crate) struct Class {
    pub(crate) definition: DefinitionId,
    pub(crate) name: String,
    pub(crate) node: ast::StmtClassDef,
    pub(crate) fields: Vec<Field>,
    pub(crate) methods: Vec<ast::StmtFunctionDef>,
    pub(crate) method_kinds: Vec<MethodKind>,
    pub(crate) dataclass: Option<DataclassOptions>,
    pub(crate) enum_members: Option<enums::Members>,
    pub(crate) generated_initializer: bool,
    /// Index into `methods`, including the initializer itself.
    pub(crate) initializer: Option<usize>,
}

pub(crate) struct Field {
    pub(crate) name: String,
    pub(crate) annotation: ast::Expr,
    pub(crate) default: Option<FieldDefault>,
    pub(crate) kw_only: bool,
    pub(crate) repr: bool,
    pub(crate) compare: bool,
}

impl Classes {
    pub(crate) fn instance(&self, id: usize) -> Type {
        if self.definitions[id].enum_members.is_some() {
            Type::StrEnum(id)
        } else {
            Type::Class(id)
        }
    }

    /// Assigns nominal identities in module and source order, independently of imports' aliases.
    pub(crate) fn collect(graph: &SourceGraph) -> Result<Self, String> {
        let mut definitions = Vec::new();
        let mut ids = HashMap::new();
        for (module_id, module) in graph.modules.iter().enumerate() {
            for (statement, node) in module.parsed.syntax().body.iter().enumerate() {
                if let ast::Stmt::ClassDef(node) = node {
                    let definition = DefinitionId {
                        module: module_id,
                        statement,
                    };
                    let class = collect_class(graph, module, definition, node)?;
                    ids.insert(definition, definitions.len());
                    definitions.push(class);
                }
            }
        }
        Ok(Self { definitions, ids })
    }
}

fn collect_class(
    graph: &SourceGraph,
    module: &SourceModule,
    definition: DefinitionId,
    node: &ast::StmtClassDef,
) -> Result<Class, String> {
    let is_enum = node.arguments.as_ref().is_some_and(|arguments| {
        matches!(arguments.args.as_ref(), [base] if graph.resolve(definition.module, base) == Some(Binding::Intrinsic(Intrinsic::StrEnum)))
            && arguments.keywords.is_empty()
    });
    if node.type_params.is_some()
        || !is_enum
            && node
                .arguments
                .as_ref()
                .is_some_and(|args| !args.args.is_empty() || !args.keywords.is_empty())
    {
        return Err(module.error(
            node,
            "class inheritance, metaclasses, and type parameters are unsupported",
        ));
    }
    if is_enum && !node.decorator_list.is_empty() {
        return Err(module.error(node, "decorated StrEnum declarations are unsupported"));
    }
    let mut enum_members = is_enum.then(enums::Members::default);
    let dataclass = dataclasses::options(graph, definition.module, node)?;
    let mut names = HashSet::new();
    let mut class_bindings = HashSet::new();
    let mut fields = Vec::new();
    let mut methods = Vec::new();
    let mut method_kinds = Vec::new();
    let mut initializer = None;
    for statement in &node.body {
        match statement {
            ast::Stmt::Assign(member) if let Some(members) = &mut enum_members => {
                let name =
                    members.declare(graph, module, definition.module, member, &class_bindings)?;
                names.insert(name.clone());
                class_bindings.insert(name);
            }
            ast::Stmt::AnnAssign(field)
                if !is_enum && (field.value.is_none() || dataclass.is_some()) =>
            {
                if dataclass.is_some() && !field.simple {
                    return Err(module.error(
                        field,
                        "dataclass fields require an unparenthesized name annotation",
                    ));
                }
                let ast::Expr::Name(name) = &*field.target else {
                    return Err(module.error(field, "a field declaration must annotate one name"));
                };
                let name = name.id.as_str();
                if name.starts_with("__") {
                    return Err(module.error(field, "field names starting with '__' are reserved"));
                }
                if names.contains(name) {
                    return Err(module.error(field, &format!("duplicate class member {name:?}")));
                }
                fields.push(dataclasses::field(
                    graph,
                    definition.module,
                    field,
                    name,
                    dataclass.as_ref(),
                    &class_bindings,
                )?);
                names.insert(name.to_owned());
                if field.value.is_some() {
                    class_bindings.insert(name.to_owned());
                }
            }
            ast::Stmt::FunctionDef(method) => {
                if method.is_async || method.type_params.is_some() {
                    return Err(module.error(
                        method,
                        "async methods and method type parameters are unsupported",
                    ));
                }
                let name = method.name.as_str();
                if is_enum && (name.starts_with('_') || matches!(name, "name" | "value")) {
                    return Err(module.error(
                        method,
                        "StrEnum methods cannot override member metadata or special behavior",
                    ));
                }
                if name.starts_with("__")
                    && !matches!(name, "__init__" | "__str__")
                    && !(name == "__post_init__" && dataclass.is_some())
                {
                    return Err(
                        module.error(method, "unsupported special method on a fixed-layout class")
                    );
                }
                let kind = method_kind(graph, definition.module, method, &class_bindings)?;
                if !names.insert(name.to_owned()) {
                    return Err(module.error(method, &format!("duplicate class member {name:?}")));
                }
                let first = method
                    .parameters
                    .posonlyargs
                    .first()
                    .or_else(|| method.parameters.args.first());
                let receiver = match kind {
                    MethodKind::Instance | MethodKind::Property => Some("self"),
                    MethodKind::Class => Some("cls"),
                    MethodKind::Static => None,
                };
                if let Some(receiver) = receiver
                    && first.is_none_or(|parameter| parameter.name().as_str() != receiver)
                {
                    return Err(module.error(
                        method,
                        &format!(
                            "this method's first positional parameter must be named {receiver:?}"
                        ),
                    ));
                }
                if kind == MethodKind::Property
                    && (method.parameters.len() != 1 || first.unwrap().default.is_some())
                {
                    return Err(module.error(method, "a property getter must take only self"));
                }
                if matches!(name, "__str__" | "__post_init__")
                    && (kind != MethodKind::Instance
                        || method.parameters.len() != 1
                        || first.is_none_or(|parameter| parameter.default.is_some()))
                {
                    return Err(module.error(
                        method,
                        &format!("{name} must be an instance method taking only self"),
                    ));
                }
                if name == "__init__" {
                    if kind != MethodKind::Instance {
                        return Err(module.error(method, "__init__ must be an instance method"));
                    }
                    initializer = Some(methods.len());
                }
                methods.push(method.clone());
                method_kinds.push(kind);
                class_bindings.insert(name.to_owned());
            }
            ast::Stmt::Expr(value) if matches!(&*value.value, ast::Expr::StringLiteral(_)) => {}
            ast::Stmt::Pass(_) => {}
            _ => {
                return Err(module.error(
                    statement,
                    "class bodies allow only declared fields, methods, pass, and docstrings; field defaults require @dataclass",
                ));
            }
        }
    }
    let generated_initializer =
        initializer.is_none() && dataclass.is_some_and(|options| options.init);
    let initializer = if generated_initializer {
        let index = methods.len();
        methods.push(dataclasses::initializer(module, node, &fields)?);
        method_kinds.push(MethodKind::Instance);
        Some(index)
    } else if is_enum {
        if enum_members.as_ref().unwrap().values.is_empty() {
            return Err(module.error(node, "a StrEnum requires at least one declared member"));
        }
        None
    } else {
        if dataclass.is_some_and(|options| options.frozen) {
            return Err(module.error(node, "frozen dataclasses require a generated initializer"));
        }
        Some(initializer.ok_or_else(|| {
            module.error(
                node,
                "a fixed class requires an explicit or generated __init__ method",
            )
        })?)
    };
    if let Some(initializer) = initializer {
        check_initializer(module, &methods[initializer], &fields)?;
    }
    Ok(Class {
        definition,
        name: node.name.as_str().to_owned(),
        node: node.clone(),
        fields,
        methods,
        method_kinds,
        dataclass,
        enum_members,
        generated_initializer,
        initializer,
    })
}

/// Only these sealed descriptors affect binding; arbitrary decorators still cannot run.
fn method_kind(
    graph: &SourceGraph,
    module_id: usize,
    method: &ast::StmtFunctionDef,
    class_names: &HashSet<String>,
) -> Result<MethodKind, String> {
    let module = &graph.modules[module_id];
    let [decorator] = method.decorator_list.as_slice() else {
        return if method.decorator_list.is_empty() {
            Ok(MethodKind::Instance)
        } else {
            Err(module.error(method, "method decorators cannot be stacked"))
        };
    };
    if let ast::Expr::Name(name) = &decorator.expression
        && graph.resolve(module_id, &decorator.expression).is_none()
        && !class_names.contains(name.id.as_str())
    {
        match name.id.as_str() {
            "property" => return Ok(MethodKind::Property),
            "classmethod" => return Ok(MethodKind::Class),
            "staticmethod" => return Ok(MethodKind::Static),
            _ => {}
        }
    }
    Err(module.error(
        decorator,
        "method decorators must be unshadowed @property, @classmethod, or @staticmethod; properties are read-only",
    ))
}

/// Initializes each field once before `self` can be read or passed to another operation.
/// This permits allocation before argument assignment without exposing a partial object.
fn check_initializer(
    module: &SourceModule,
    initializer: &ast::StmtFunctionDef,
    fields: &[Field],
) -> Result<(), String> {
    let receiver = initializer
        .parameters
        .iter_non_variadic_params()
        .next()
        .expect("initializers have a receiver")
        .name()
        .as_str();
    let declared = fields
        .iter()
        .map(|field| field.name.as_str())
        .collect::<HashSet<_>>();
    let mut initialized = HashSet::new();
    for statement in &initializer.body {
        let assignment = match statement {
            ast::Stmt::Assign(assignment) => assignment,
            ast::Stmt::Expr(value) if matches!(&*value.value, ast::Expr::StringLiteral(_)) => {
                continue;
            }
            ast::Stmt::Pass(_) => continue,
            _ => {
                return Err(module.error(
                    statement,
                    "__init__ must contain only straight-line self.field assignments, docstrings, or pass",
                ));
            }
        };
        let mut targets = Vec::new();
        for target in &assignment.targets {
            initializer_targets(target, &mut targets);
        }
        for target in targets {
            let ast::Expr::Attribute(attribute) = target else {
                return Err(module.error(target, "__init__ must assign a declared self.field"));
            };
            if !matches!(&*attribute.value, ast::Expr::Name(name) if name.id == receiver) {
                return Err(module.error(target, "__init__ must assign a declared self.field"));
            }
            let name = attribute.attr.as_str();
            if !declared.contains(name) {
                return Err(module.error(
                    target,
                    &format!("field {name:?} is not declared in the class"),
                ));
            }
            if !initialized.insert(name) {
                return Err(module.error(
                    target,
                    &format!("field {name:?} is initialized more than once"),
                ));
            }
        }
        let mut self_use = SelfUse(receiver, None);
        self_use.visit_expr(&assignment.value);
        if let Some(expression) = self_use.1 {
            return Err(module.error(
                expression,
                "__init__ assignment values cannot read or expose self before construction finishes",
            ));
        }
    }
    let missing = fields
        .iter()
        .filter(|field| !initialized.contains(field.name.as_str()))
        .map(|field| field.name.as_str())
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        return Err(module.error(
            initializer,
            &format!(
                "__init__ does not initialize fields: {}",
                missing.join(", ")
            ),
        ));
    }
    Ok(())
}

/// Chained and nested assignments must still initialize only declared fields.
/// A starred target receives one list, so it has the same field requirement.
fn initializer_targets<'a>(target: &'a ast::Expr, targets: &mut Vec<&'a ast::Expr>) {
    match target {
        ast::Expr::Tuple(tuple) => {
            for target in &tuple.elts {
                initializer_targets(target, targets);
            }
        }
        ast::Expr::List(list) => {
            for target in &list.elts {
                initializer_targets(target, targets);
            }
        }
        ast::Expr::Starred(starred) => initializer_targets(&starred.value, targets),
        _ => targets.push(target),
    }
}

struct SelfUse<'a>(&'a str, Option<&'a ast::Expr>);

impl<'a> Visitor<'a> for SelfUse<'a> {
    fn visit_expr(&mut self, expression: &'a ast::Expr) {
        if self.1.is_some() {
            return;
        }
        if let ast::Expr::Lambda(lambda) = expression
            && lambda.parameters.as_ref().is_some_and(|parameters| {
                parameters
                    .iter_non_variadic_params()
                    .any(|parameter| parameter.name().as_str() == self.0)
            })
        {
            // Defaults are rejected, and this self belongs only to the lambda.
            return;
        }
        // A comprehension target named self is distinct from the incomplete
        // receiver. Its leftmost iterable still belongs to the enclosing scope.
        let clauses = match expression {
            ast::Expr::ListComp(value) => Some(value.generators.as_slice()),
            ast::Expr::SetComp(value) => Some(value.generators.as_slice()),
            ast::Expr::DictComp(value) => Some(value.generators.as_ref()),
            ast::Expr::Generator(value) => Some(value.generators.as_slice()),
            _ => None,
        };
        fn binds_self(target: &ast::Expr, receiver: &str) -> bool {
            match target {
                ast::Expr::Name(name) => name.id == receiver,
                ast::Expr::Tuple(value) => {
                    value.elts.iter().any(|target| binds_self(target, receiver))
                }
                ast::Expr::List(value) => {
                    value.elts.iter().any(|target| binds_self(target, receiver))
                }
                ast::Expr::Starred(value) => binds_self(&value.value, receiver),
                _ => false,
            }
        }
        if let Some(clauses) = clauses
            && clauses
                .iter()
                .any(|clause| binds_self(&clause.target, self.0))
        {
            self.visit_expr(&clauses[0].iter);
            return;
        }
        if matches!(expression, ast::Expr::Name(name) if name.id == self.0) {
            self.1 = Some(expression);
        } else {
            walk_expr(self, expression);
        }
    }
}
