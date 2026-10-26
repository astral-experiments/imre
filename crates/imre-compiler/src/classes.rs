//! Collects fixed class shapes before nominal type resolution and executable lowering.

use std::collections::{HashMap, HashSet};

use ruff_python_ast as ast;
use ruff_python_ast::visitor::{Visitor, walk_expr};

use crate::ir::Type;
use crate::modules::{Binding, DefinitionId, Intrinsic, SourceGraph, SourceModule};

mod constants;
mod dataclasses;
mod enums;
mod exception_forwarders;
mod exceptions;
mod inheritance;
mod init_hooks;
mod record_hash;
pub(crate) use constants::ClassValue;
pub(crate) use dataclasses::{ContainerFactory, DataclassOptions, FieldDefault};
pub(crate) use exception_forwarders::ForwardingInitializer;
pub(crate) use exceptions::explicit_message_initializer;
pub(crate) use inheritance::{super_init, super_method};

pub(crate) const EXCEPTION_ARGS_SLOT: &str = "<exception arguments>";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MethodKind {
    Instance,
    Property,
    PropertySetter,
    Class,
    Static,
}

pub(crate) struct Classes {
    pub(crate) definitions: Vec<Class>,
    /// Data members only need rebindable storage when some source writes that attribute.
    pub(crate) attribute_writes: HashMap<String, HashSet<Type>>,
    pub(crate) exceptions: HashMap<DefinitionId, u32>,
    pub(crate) exception_declarations: Vec<crate::types::NamedException>,
    pub(crate) ids: HashMap<DefinitionId, usize>,
}

#[derive(Clone)]
pub(crate) enum ExceptionAlias {
    Builtin { kind: u32, name: String },
    Named(DefinitionId),
}

pub(crate) struct Class {
    pub(crate) definition: DefinitionId,
    pub(crate) name: String,
    pub(crate) node: ast::StmtClassDef,
    pub(crate) fields: Vec<Field>,
    pub(crate) methods: Vec<ast::StmtFunctionDef>,
    /// Field writes required of every override called during construction.
    pub(crate) init_hooks: init_hooks::Hooks,
    /// A base initializer that passes self to Python needs the entire derived layout ready.
    pub(crate) init_exposes_self: bool,
    pub(crate) method_kinds: Vec<MethodKind>,
    /// Declaring class for the corresponding method's imports and lexical super.
    pub(crate) method_origins: Vec<DefinitionId>,
    /// Single storage-bearing lineage, nearest base first.
    pub(crate) bases: Vec<usize>,
    pub(crate) abstract_methods: HashSet<String>,
    pub(crate) abc: bool,
    /// This layout inherits the collections.abc.Mapping primitives and mixins.
    pub(crate) mapping: bool,
    pub(crate) toml_hash: bool,
    pub(crate) dataclass: Option<DataclassOptions>,
    pub(crate) enum_members: Option<enums::Members>,
    pub(crate) exception_aliases: HashMap<String, ExceptionAlias>,
    pub(crate) forwarder: Option<ForwardingInitializer>,
    pub(crate) static_bindings: HashMap<String, ClassValue>,
    pub(crate) generated_initializer: bool,
    /// Index into `methods`, including the initializer itself.
    pub(crate) initializer: Option<usize>,
}

impl Class {
    /// The ABI slot stays in the root layout when a subclass adds Python fields.
    pub(crate) fn exception_argument_slot(&self) -> usize {
        self.fields
            .iter()
            .position(|field| {
                matches!(
                    field.name.as_str(),
                    EXCEPTION_ARGS_SLOT | "<exception message>"
                )
            })
            .expect("exception storage owns an argument slot")
    }
}

#[derive(Clone)]
pub(crate) struct Field {
    pub(crate) module: usize,
    pub(crate) name: String,
    pub(crate) annotation: ast::Expr,
    pub(crate) default: Option<FieldDefault>,
    pub(crate) kw_only: bool,
    pub(crate) repr: bool,
    pub(crate) compare: bool,
}

impl Classes {
    /// The entry module may need to execute class bodies even without module globals.
    pub(crate) fn has_class_values(&self, module: usize) -> bool {
        self.definitions.iter().any(|class| {
            class.definition.module == module && class.static_bindings.values().any(|value| {
                matches!(value, ClassValue::Runtime { .. })
                    || matches!(value, ClassValue::Strings { owner, .. } if *owner == class.definition)
            })
        })
    }

    pub(crate) fn instance(&self, id: usize) -> Type {
        if let Some(members) = &self.definitions[id].enum_members {
            if members.plain {
                Type::Enum(id)
            } else {
                Type::StrEnum(id)
            }
        } else {
            Type::Class(id)
        }
    }

    /// Assigns nominal identities in module and source order, independently of imports' aliases.
    pub(crate) fn collect(graph: &SourceGraph) -> Result<Self, String> {
        let resolved = exceptions::collect(graph)?;
        let mut source_nodes = Vec::new();
        let mut ids = HashMap::new();
        for (module_id, module) in graph.modules.iter().enumerate() {
            for (statement, node) in module.parsed.syntax().body.iter().enumerate() {
                if let ast::Stmt::ClassDef(node) = node {
                    let definition = DefinitionId {
                        module: module_id,
                        statement,
                    };
                    if !graph.includes(definition)
                        || (resolved.ids.contains_key(&definition)
                            && !resolved.layouts.contains(&definition))
                    {
                        continue;
                    }
                    ids.insert(definition, source_nodes.len());
                    let mut record = std::borrow::Cow::Borrowed(node);
                    if resolved.layouts.contains(&definition) {
                        // Exception inheritance is resolved above; only the additional fixed
                        // fields and methods take part in record layout and initialization.
                        if let Some(parent) = resolved.storage_parent.get(&definition) {
                            let arguments = record.to_mut().arguments.as_mut().unwrap();
                            let base = arguments
                                .args
                                .iter()
                                .find(|base| {
                                    graph.resolve(module_id, base)
                                        == Some(Binding::Definition(*parent))
                                })
                                .unwrap()
                                .clone();
                            arguments.args = std::iter::once(base).collect();
                        } else {
                            record.to_mut().arguments = None;
                        }
                    }
                    source_nodes.push((definition, record));
                }
            }
        }
        let nodes = source_nodes
            .iter()
            .map(|(def, node)| (*def, node.as_ref()))
            .collect::<Vec<_>>();
        let definitions = inheritance::resolve(graph, &nodes, &ids, &resolved)?;
        let data_names: HashSet<_> = definitions
            .iter()
            .flat_map(|class| {
                class.static_bindings.iter().filter_map(|(name, value)| {
                    matches!(value, ClassValue::Runtime { .. }).then_some(name.as_str())
                })
            })
            .collect();
        let attribute_writes = constants::attribute_writes(graph, &data_names);
        Ok(Self {
            definitions,
            attribute_writes,
            ids,
            exceptions: resolved.ids,
            exception_declarations: resolved.named,
        })
    }
}

/// Resolve the origin of a Mapping base, including aliases and its two static type arguments.
pub(crate) fn mapping_base(graph: &SourceGraph, module: usize, expression: &ast::Expr) -> bool {
    let origin = match expression {
        ast::Expr::Subscript(subscript) => {
            let ast::Expr::Tuple(args) = &*subscript.slice else {
                return false;
            };
            if args.elts.len() != 2 || !args.elts.iter().all(|arg| match graph.resolve(module, arg) {
                Some(Binding::Definition(id)) => matches!(graph.modules[id.module].parsed.syntax().body[id.statement], ast::Stmt::ClassDef(_)),
                None => matches!(arg, ast::Expr::Name(n) if matches!(n.id.as_str(), "int" | "bool" | "float" | "str" | "bytes" | "list" | "tuple" | "dict" | "set" | "object")),
                _ => false,
            })
            {
                return false;
            }
            subscript.value.as_ref()
        }
        origin => origin,
    };
    matches!(
        graph.resolve(module, origin),
        Some(Binding::Intrinsic(Intrinsic::Mapping { .. }))
    )
}

fn collect_class(
    graph: &SourceGraph,
    module: &SourceModule,
    definition: DefinitionId,
    node: &ast::StmtClassDef,
    base: Option<(usize, &Class)>,
    exception_layout: bool,
    exceptions: &HashMap<DefinitionId, u32>,
) -> Result<Class, String> {
    let is_enum = node.arguments.as_ref().is_some_and(|arguments| {
        matches!(arguments.args.as_ref(), [base] if matches!(graph.resolve(definition.module, base), Some(Binding::Intrinsic(Intrinsic::StrEnum | Intrinsic::StandardEnum))))
            && arguments.keywords.is_empty()
    });
    let direct_mapping = node.arguments.as_ref().is_some_and(|args| {
        args.args
            .iter()
            .any(|arg| mapping_base(graph, definition.module, arg))
    });
    let mapping = direct_mapping || base.is_some_and(|(_, parent)| parent.mapping);
    let abc = mapping
        || base.is_some_and(|(_, parent)| parent.abc)
        || node.arguments.as_ref().is_some_and(|args| {
            args.args.iter().any(|arg| {
                graph.resolve(definition.module, arg) == Some(Binding::Intrinsic(Intrinsic::Abc))
            })
        });
    if node.type_params.is_some()
        || !is_enum
            && base.is_none()
            && !abc
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
    if let Some(members) = &mut enum_members {
        members.plain = graph.resolve(definition.module, &node.arguments.as_ref().unwrap().args[0])
            == Some(Binding::Intrinsic(Intrinsic::StandardEnum));
    }
    let dataclass = dataclasses::options(graph, definition.module, node)?;
    let value_struct = dataclass.is_some_and(|options| options.value_struct);
    if base.is_some_and(|(_, parent)| parent.dataclass.is_some_and(|options| options.value_struct))
        || value_struct && node.arguments.as_ref().is_some_and(|args| !args.is_empty())
    {
        return Err(module.error(node, "an imre.struct is final and cannot inherit fields"));
    }
    if value_struct {
        for statement in &node.body {
            match statement {
                ast::Stmt::AnnAssign(field) if field.value.is_none() => {}
                ast::Stmt::FunctionDef(method)
                    if !method.name.starts_with("__") && method.decorator_list.is_empty() => {}
                ast::Stmt::Pass(_) => {}
                ast::Stmt::Expr(stmt) if matches!(&*stmt.value, ast::Expr::StringLiteral(_)) => {}
                _ => {
                    return Err(module.error(
                        statement,
                        "imre.struct supports required fields and ordinary instance methods",
                    ));
                }
            }
        }
    }
    let forwarder = exception_layout
        .then(|| ForwardingInitializer::collect(definition, node, base.map(|(_, c)| c), module))
        .flatten();
    if base.is_some_and(|(_, c)| c.forwarder.is_some()) && forwarder.is_none() {
        return Err(module.error(
            node,
            "a subclass of a variadic exception must forward a fixed set of constructor parameters",
        ));
    }
    let mut names = HashSet::new();
    let mut class_bindings = HashSet::new();
    let mut abstract_methods = base
        .map(|(_, parent)| parent.abstract_methods.clone())
        .unwrap_or_default();
    if direct_mapping {
        for primitive in ["__getitem__", "__iter__", "__len__"] {
            if !base.is_some_and(|(_, parent)| {
                parent.methods.iter().any(|m| m.name.as_str() == primitive)
            }) {
                abstract_methods.insert(primitive.to_owned());
            }
        }
    }
    let mut exception_aliases =
        base.map_or_else(HashMap::new, |(_, b)| b.exception_aliases.clone());
    let mut static_bindings = base.map_or_else(HashMap::new, |(_, b)| b.static_bindings.clone());
    let mut hash = None;
    let mut fields = base
        .map(|(_, base)| base.fields.clone())
        .unwrap_or_default();
    if let Some((_, base)) = base
        && (base.enum_members.is_some()
            || base
                .dataclass
                .is_some_and(|b| dataclass.is_none_or(|d| d.frozen != b.frozen)))
    {
        return Err(module.error(node, "incompatible base for a fixed-layout class"));
    }
    let mut methods: Vec<ast::StmtFunctionDef> = Vec::new();
    let mut method_kinds = Vec::new();
    let mut initializer = None;
    for (ordinal, statement) in node.body.iter().enumerate() {
        // A call through an upcast uses its declared layout. Do not erase an
        // override which could select a different callable at runtime.
        let member = match statement {
            ast::Stmt::FunctionDef(f) => Some(f.name.as_str()),
            ast::Stmt::AnnAssign(a) => a.target.as_name_expr().map(|n| n.id.as_str()),
            ast::Stmt::Assign(a) => a
                .targets
                .first()
                .and_then(ast::Expr::as_name_expr)
                .map(|n| n.id.as_str()),
            _ => None,
        };
        if member.is_some_and(|name| {
            base.and_then(|(_, b)| b.static_bindings.get(name))
                .is_some_and(|binding| {
                    !matches!(
                        (binding, statement),
                        (ClassValue::Runtime { .. }, ast::Stmt::Assign(_))
                    )
                })
        }) {
            return Err(module.error(statement, "overriding a fixed class binding is unsupported"));
        }
        match statement {
            ast::Stmt::Assign(member) if let Some(members) = &mut enum_members => {
                let name =
                    members.declare(graph, module, definition.module, member, &class_bindings)?;
                names.insert(name.clone());
                class_bindings.insert(name);
            }
            ast::Stmt::AnnAssign(field)
                if let (Some(name), Some(value)) =
                    (field.target.as_name_expr(), field.value.as_deref())
                    && exceptions::constructor_annotation(
                        graph,
                        definition.module,
                        &field.annotation,
                    )
                    && let Some(target) =
                        exceptions::alias(graph, definition.module, value, exceptions) =>
            {
                if !names.insert(name.id.to_string()) {
                    return Err(module.error(name, "duplicate class binding"));
                }
                class_bindings.insert(name.id.to_string());
                exception_aliases.insert(name.id.to_string(), target);
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
                if base
                    .is_some_and(|(_, base)| base.methods.iter().any(|m| m.name.as_str() == name))
                {
                    return Err(
                        module.error(field, "a fixed field cannot replace an inherited method")
                    );
                }
                let declared_field = dataclasses::field(
                    graph,
                    definition.module,
                    field,
                    name,
                    dataclass.as_ref(),
                    &class_bindings,
                )?;
                if let Some(index) = fields.iter().position(|inherited| inherited.name == name) {
                    if dataclass.is_none() {
                        return Err(module.error(node, "an inherited field cannot be redeclared"));
                    }
                    fields[index] = declared_field;
                } else {
                    fields.push(declared_field);
                }
                names.insert(name.to_owned());
                if field.value.is_some() {
                    class_bindings.insert(name.to_owned());
                }
            }
            ast::Stmt::FunctionDef(method) => {
                let abstract_method = matches!(method.decorator_list.as_slice(), [decorator]
                    if graph.resolve(definition.module, &decorator.expression) == Some(Binding::Intrinsic(Intrinsic::AbstractMethod)));
                if !abstract_method && !graph.includes_method(definition.module, method) {
                    abstract_methods.remove(method.name.as_str());
                    continue;
                }
                if method.type_params.is_some() {
                    return Err(module.error(method, "method type parameters are unsupported"));
                }
                let name = method.name.as_str();
                if base.is_some_and(|(_, base)| base.fields.iter().any(|field| field.name == name))
                {
                    return Err(
                        module.error(method, "a method cannot replace an inherited fixed field")
                    );
                }
                if abstract_method {
                    if !abc || !method.body.iter().all(|stmt| matches!(stmt, ast::Stmt::Pass(_))
                        || matches!(stmt, ast::Stmt::Expr(value) if matches!(&*value.value, ast::Expr::StringLiteral(_) | ast::Expr::EllipsisLiteral(_)))) {
                        return Err(module.error(method, "an abstract record method must be a declaration on an ABC"));
                    }
                    if !names.insert(name.to_owned()) {
                        return Err(module.error(method, "duplicate abstract method"));
                    }
                    abstract_methods.insert(name.to_owned());
                    class_bindings.insert(name.to_owned());
                    continue;
                }
                abstract_methods.remove(name);
                if name == "__hash__" {
                    if hash.is_some() || !dataclass.is_some_and(|d| d.frozen && d.eq) {
                        return Err(module
                            .error(method, "a custom hash requires a frozen equality dataclass"));
                    }
                    hash = Some(method);
                    names.insert(name.to_owned());
                    class_bindings.insert(name.to_owned());
                    continue;
                }
                if is_enum && (name.starts_with('_') || matches!(name, "name" | "value")) {
                    return Err(module.error(
                        method,
                        "StrEnum methods cannot override member metadata or special behavior",
                    ));
                }
                let unary_protocol = matches!(
                    name,
                    "__str__" | "__repr__" | "__bool__" | "__index__" | "__iter__" | "__copy__"
                ) || (mapping && name == "__len__");
                let selector_context = module.is_bundled("selectors")
                    && node.name.as_str() == "PollSelector"
                    && matches!(name, "__enter__" | "__exit__");
                if name.starts_with("__")
                    && !matches!(name, "__init__" | "__fspath__" | "__setitem__")
                    && !unary_protocol
                    && !selector_context
                    && !(mapping && matches!(name, "__getitem__" | "__contains__"))
                    && !(name == "__post_init__" && dataclass.is_some())
                    && !(module.is_bundled("socketserver")
                        && node.name.as_str() == "TCPServer"
                        && matches!(name, "__enter__" | "__exit__"))
                {
                    return Err(
                        module.error(method, "unsupported special method on a fixed-layout class")
                    );
                }
                let kind = method_kind(graph, definition.module, method, &class_bindings)?;
                if name == "__setitem__"
                    && (kind != MethodKind::Instance
                        || method.parameters.len() != 3
                        || method.parameters.posonlyargs.len() + method.parameters.args.len() != 3)
                {
                    return Err(module.error(method, "__setitem__ requires self, key and value"));
                }
                if kind == MethodKind::PropertySetter {
                    let mut accessors = methods
                        .iter()
                        .zip(&method_kinds)
                        .filter(|(m, _)| m.name.as_str() == name);
                    let getter = accessors.next().is_some_and(|(m, kind)| {
                        *kind == MethodKind::Property
                            && !m.decorator_list.iter().any(|d| {
                                graph.resolve(definition.module, &d.expression)
                                    == Some(Binding::Intrinsic(Intrinsic::CachedProperty))
                            })
                    });
                    if !getter || accessors.next().is_some() {
                        return Err(
                            module.error(method, "a property setter needs its preceding getter")
                        );
                    }
                } else if !names.insert(name.to_owned()) {
                    return Err(module.error(method, &format!("duplicate class member {name:?}")));
                }
                let first = method
                    .parameters
                    .posonlyargs
                    .first()
                    .or_else(|| method.parameters.args.first());
                let receiver = match kind {
                    MethodKind::Instance
                        if method.name.as_str() == "__init__" && forwarder.is_some() =>
                    {
                        forwarder.as_ref().map(|plan| plan.receiver.as_str())
                    }
                    MethodKind::Instance | MethodKind::Property | MethodKind::PropertySetter => {
                        Some("self")
                    }
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
                if kind == MethodKind::PropertySetter
                    && (method.parameters.posonlyargs.len() + method.parameters.args.len() != 2
                        || method.parameters.len() != 2)
                {
                    return Err(
                        module.error(method, "a property setter must take self and a value")
                    );
                }
                if (unary_protocol || name == "__post_init__")
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
                let mut method = method.clone();
                if forwarder.is_some() {
                    exception_forwarders::mangle_fields(node.name.as_str(), &mut method);
                }
                methods.push(method);
                method_kinds.push(kind);
                class_bindings.insert(name.to_owned());
            }
            ast::Stmt::Assign(assign)
                if let [ast::Expr::Name(name)] = assign.targets.as_slice()
                    && !class_bindings.contains("property")
                    && let Some((read, write)) =
                        property_accessors(graph, definition.module, &assign.value) =>
            {
                if !names.insert(name.id.to_string()) {
                    return Err(module.error(name, "duplicate property binding"));
                }
                for (accessor, kind, arity) in [
                    (Some(read), MethodKind::Property, 1),
                    (write, MethodKind::PropertySetter, 2),
                ] {
                    let Some(accessor) = accessor else { continue };
                    let Some((method, _)) = methods
                        .iter()
                        .zip(&method_kinds)
                        .find(|(m, k)| m.name.as_str() == accessor && **k == MethodKind::Instance)
                    else {
                        return Err(module.error(
                            assign,
                            "property accessors must be preceding instance methods",
                        ));
                    };
                    if method.parameters.len() != arity
                        || method.parameters.posonlyargs.len() + method.parameters.args.len()
                            != arity
                    {
                        return Err(module.error(
                            assign,
                            "property accessor has the wrong number of positional parameters",
                        ));
                    }
                    let mut alias = method.clone();
                    alias.name = ast::Identifier::new(name.id.as_str(), name.range);
                    methods.push(alias);
                    method_kinds.push(kind);
                }
                class_bindings.insert(name.id.to_string());
            }
            ast::Stmt::Assign(assign)
                if let [ast::Expr::Name(name)] = assign.targets.as_slice()
                    && let Some(binding) = constants::value(
                        graph,
                        definition,
                        ordinal,
                        &assign.value,
                        &class_bindings,
                        &static_bindings,
                    )?
                    .or_else(|| {
                        (dataclass.is_none()
                            && !exception_layout
                            && constants::runtime_expression(&assign.value)
                            && exceptions::alias(
                                graph,
                                definition.module,
                                &assign.value,
                                exceptions,
                            )
                            .is_none())
                        .then_some(ClassValue::Runtime {
                            owner: definition,
                            ordinal,
                        })
                    }) =>
            {
                if !names.insert(name.id.to_string())
                    || name.id.starts_with("__")
                    || base.is_some_and(|(_, b)| {
                        b.fields.iter().any(|f| f.name == name.id.as_str())
                            || b.methods
                                .iter()
                                .any(|m| m.name.as_str() == name.id.as_str())
                            || b.exception_aliases.contains_key(name.id.as_str())
                    })
                {
                    return Err(module.error(name, "duplicate or reserved static class binding"));
                }
                class_bindings.insert(name.id.to_string());
                static_bindings.insert(name.id.to_string(), binding);
            }
            ast::Stmt::Assign(assign)
                if let [ast::Expr::Name(name)] = assign.targets.as_slice()
                    && let Some(target) =
                        exceptions::alias(graph, definition.module, &assign.value, exceptions) =>
            {
                if !names.insert(name.id.to_string()) {
                    return Err(module.error(name, "duplicate class binding"));
                }
                class_bindings.insert(name.id.to_string());
                exception_aliases.insert(name.id.to_string(), target);
            }
            ast::Stmt::Expr(value) if matches!(&*value.value, ast::Expr::StringLiteral(_)) => {}
            ast::Stmt::Pass(_) => {}
            _ => {
                return Err(module.error(
                    statement,
                    "class bodies support field declarations, plain data assignments, methods, pass, and docstrings; annotated field defaults require @dataclass",
                ));
            }
        }
    }
    let mut method_origins = vec![definition; methods.len()];
    if let Some(method) = hash
        && !record_hash::toml_fields(method, &fields, graph, definition.module)
    {
        return Err(module.error(
            method,
            "a frozen custom hash must hash a TOML serialization of its exact ordered JSON fields",
        ));
    }
    let inherited_fields = base.map_or(&[][..], |(_, base)| base.fields.as_slice());
    if let Some(plan) = &forwarder {
        let annotation = ruff_python_parser::parse_expression("object")
            .expect("static annotation")
            .expr()
            .clone();
        for (name, _) in &plan.stores {
            if !fields.iter().any(|field| &field.name == name) {
                fields.push(Field {
                    module: definition.module,
                    name: name.clone(),
                    annotation: annotation.clone(),
                    default: None,
                    kw_only: false,
                    repr: true,
                    compare: true,
                });
            }
        }
    }
    if !is_enum
        && dataclass.is_none()
        && fields.len() == inherited_fields.len()
        && let Some(init) = initializer
    {
        inheritance::infer_fields(graph, definition.module, &methods[init], &mut fields)?;
        // A write to inherited or local class data creates an instance shadow.
        fields.retain(|f| {
            !matches!(
                static_bindings.get(&f.name),
                Some(ClassValue::Runtime { .. })
            )
        });
    }
    if fields
        .iter()
        .any(|field| static_bindings.contains_key(&field.name))
    {
        return Err(module.error(
            node,
            "an instance field cannot shadow a fixed class binding",
        ));
    }
    if value_struct && fields.is_empty() {
        return Err(module.error(node, "imre.struct needs at least one field"));
    }
    let generated_initializer =
        initializer.is_none() && dataclass.is_some_and(|options| options.init);
    let initializer = if generated_initializer {
        let index = methods.len();
        methods.push(dataclasses::initializer(module, node, &fields)?);
        method_kinds.push(MethodKind::Instance);
        method_origins.push(definition);
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
        if initializer.is_some() {
            initializer
        } else if let Some((_, base)) = base
            && fields.len() == base.fields.len()
            && let Some(init) = base.initializer
        {
            let index = methods.len();
            methods.push(base.methods[init].clone());
            method_kinds.push(MethodKind::Instance);
            method_origins.push(base.method_origins[init]);
            Some(index)
        } else if fields.is_empty() {
            let index = methods.len();
            methods.push(dataclasses::initializer(module, node, &[])?);
            method_kinds.push(MethodKind::Instance);
            method_origins.push(definition);
            Some(index)
        } else {
            return Err(module.error(
                node,
                "a fixed class requires an explicit or generated __init__ method",
            ));
        }
    };
    let mut bases = Vec::new();
    if let Some((id, base)) = base {
        bases.push(id);
        bases.extend_from_slice(&base.bases);
        for (index, method) in base.methods.iter().enumerate() {
            if method.name.as_str() != "__init__" && !names.contains(method.name.as_str()) {
                if base.method_kinds[index] == MethodKind::Class
                    && !graph.inherits_class_method(definition, method.name.as_str())
                {
                    continue;
                }
                methods.push(method.clone());
                method_kinds.push(base.method_kinds[index]);
                method_origins.push(base.method_origins[index]);
            }
        }
    }
    let mut init_hooks = base
        .map(|(_, parent)| parent.init_hooks.clone())
        .unwrap_or_default();
    let hook_writes = |name: &str, inherited: Option<&init_hooks::Hook>| {
        init_hooks::method_writes(
            graph,
            &methods,
            &method_kinds,
            &method_origins,
            name,
            inherited,
        )
    };
    for (name, expected) in &init_hooks {
        if names.contains(name) {
            let actual = hook_writes(name, Some(expected));
            if actual.transpose()?.as_ref() != Some(&expected.writes) {
                return Err(module.error(
                    node,
                    "an initializer hook override must initialize the same fields",
                ));
            }
        }
    }
    let base_exposes_self = base.is_some_and(|(_, parent)| parent.init_exposes_self);
    let mut init_exposes_self = base_exposes_self;
    if let Some(initializer) = initializer
        && method_origins[initializer] == definition
        // A proven forwarding exception constructor writes each field through its plan.
        && forwarder.is_none()
    {
        init_exposes_self |= check_initializer(
            module,
            &methods[initializer],
            InitializerFields {
                declared: &fields,
                bindings: &static_bindings,
                inherited: if generated_initializer {
                    &[]
                } else {
                    inherited_fields
                },
                exposes_self: base_exposes_self,
            },
            exception_layout,
            &mut init_hooks,
            |name| hook_writes(name, None),
        )?;
    }
    Ok(Class {
        definition,
        forwarder,
        name: node.name.as_str().to_owned(),
        init_hooks,
        init_exposes_self,
        node: node.clone(),
        fields,
        toml_hash: hash.is_some(),
        methods,
        method_kinds,
        method_origins,
        bases,
        abstract_methods,
        abc,
        mapping,
        dataclass,
        enum_members,
        exception_aliases,
        static_bindings,
        generated_initializer,
        initializer,
    })
}

/// Recognize a fixed property declared from locally named accessors.
pub(crate) fn property_accessors<'a>(
    graph: &SourceGraph,
    module: usize,
    value: &'a ast::Expr,
) -> Option<(&'a str, Option<&'a str>)> {
    let ast::Expr::Call(call) = value else {
        return None;
    };
    if !matches!(&*call.func, ast::Expr::Name(n) if n.id == "property")
        || graph.resolve(module, &call.func).is_some()
        || !call.arguments.keywords.is_empty()
    {
        return None;
    }
    match call.arguments.args.as_ref() {
        [ast::Expr::Name(get)] => Some((get.id.as_str(), None)),
        [ast::Expr::Name(get), ast::Expr::Name(set)] => {
            Some((get.id.as_str(), Some(set.id.as_str())))
        }
        _ => None,
    }
}

/// Resolve declarations whose descriptor cannot bind an instance: a sealed
/// module or an explicit staticmethod over a known function.
fn class_static_binding(
    graph: &SourceGraph,
    module: usize,
    expr: &ast::Expr,
    locals: &HashSet<String>,
) -> Option<Binding> {
    let (value, wrapped) = if let ast::Expr::Call(c) = expr {
        if !matches!(&*c.func, ast::Expr::Name(n) if n.id == "staticmethod")
            || locals.contains("staticmethod")
            || graph.resolve(module, &c.func).is_some()
            || !c.arguments.keywords.is_empty()
        {
            return None;
        }
        let [value] = c.arguments.args.as_slice() else {
            return None;
        };
        (value, true)
    } else {
        (expr, false)
    };
    let mut root = value;
    while let ast::Expr::Attribute(a) = root {
        root = &a.value;
    }
    if !matches!(root, ast::Expr::Name(n) if !locals.contains(n.id.as_str())) {
        return None;
    }
    let binding = graph.resolve(module, value)?;
    match (wrapped, binding) {
        (false, Binding::Value(id)) => {
            // A compiled regex is an immutable object in a fixed module binding.
            let stmt = &graph.modules[id.module].parsed.syntax().body[id.statement];
            let init = match stmt {
                ast::Stmt::Assign(a) => Some(a.value.as_ref()),
                ast::Stmt::AnnAssign(a) => a.value.as_deref(),
                _ => None,
            };
            if let Some(ast::Expr::Call(call)) = init
                && graph.resolve(id.module, &call.func)
                    == Some(Binding::Intrinsic(Intrinsic::RegexCompile))
            {
                Some(binding)
            } else {
                None
            }
        }
        (false, Binding::IntrinsicModule(_)) | (true, Binding::Intrinsic(_)) => Some(binding),
        (true, Binding::Definition(id))
            if matches!(
                graph.modules[id.module].parsed.syntax().body[id.statement],
                ast::Stmt::FunctionDef(_)
            ) =>
        {
            Some(binding)
        }
        _ => None,
    }
}

/// Only these sealed descriptors affect binding; arbitrary decorators still cannot run.
pub(crate) fn method_kind(
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
    if graph.resolve(module_id, &decorator.expression)
        == Some(Binding::Intrinsic(Intrinsic::CachedProperty))
        && !method.is_async
    {
        return Ok(MethodKind::Property);
    }
    if graph.resolve(module_id, &decorator.expression)
        == Some(Binding::Intrinsic(Intrinsic::ContextManager))
        && !method.is_async
    {
        return Ok(MethodKind::Instance);
    }
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
    if let ast::Expr::Attribute(attr) = &decorator.expression
        && attr.attr.as_str() == "setter"
        && let ast::Expr::Name(name) = &*attr.value
        && name.id == method.name.id
        && class_names.contains(name.id.as_str())
    {
        return Ok(MethodKind::PropertySetter);
    }
    Err(module.error(
        decorator,
        "unsupported method decorator on a fixed-layout class",
    ))
}

struct InitializerFields<'a> {
    declared: &'a [Field],
    exposes_self: bool,
    inherited: &'a [Field],
    bindings: &'a HashMap<String, ClassValue>,
}

/// Checks that a constructor initializes the layout before user code can see it,
/// and reports whether the initialized instance escapes to a callback.
fn check_initializer(
    module: &SourceModule,
    initializer: &ast::StmtFunctionDef,
    layout: InitializerFields<'_>,
    exception_layout: bool,
    hooks: &mut init_hooks::Hooks,
    lookup: impl Fn(&str) -> Option<Result<Vec<String>, String>>,
) -> Result<bool, String> {
    let InitializerFields {
        declared: fields,
        exposes_self: base_exposes_self,
        inherited,
        bindings,
    } = layout;
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
    // Plain class values are available even while the instance is under construction.
    // A write shadows them; it does not initialize a mandatory physical field.
    let mut initialized: HashSet<&str> = bindings
        .iter()
        .filter_map(|(n, v)| matches!(v, ClassValue::Runtime { .. }).then_some(n.as_str()))
        .collect();
    let mut called_super = false;
    let mut called_exception_base = false;
    for (index, statement) in initializer.body.iter().enumerate() {
        if index + 1 == initializer.body.len()
            && (inherited.is_empty() || called_super)
            && let ast::Stmt::Expr(s) = statement
            && let ast::Expr::Call(call) = &*s.value
            && matches!(&*call.func, ast::Expr::Name(_))
            && matches!(call.arguments.args.first(), Some(ast::Expr::Name(n)) if n.id == receiver)
        {
            // Initialized also contains readable class attributes, which cannot
            // stand in for a missing field when Python receives the instance.
            if !declared.is_subset(&initialized) {
                return Err(module.error(
                    statement,
                    "initialize all instance fields before passing self to a callback",
                ));
            }
            return Ok(true);
        }
        if exception_layout
            && let ast::Stmt::Expr(stmt) = statement
            && let ast::Expr::Call(call) = &*stmt.value
            && let Some((_, args)) = explicit_message_initializer(module, call)
        {
            let mut use_self = SelfUse(receiver, None, &initialized);
            for arg in args {
                use_self.visit_expr(arg);
            }
            if let Some(expr) = use_self.1 {
                return Err(module.error(
                    expr,
                    "the incomplete instance cannot escape during construction",
                ));
            }
            // Calling BaseException directly bypasses any user base initializer.
            // Its ordinary fields still have to be initialized by this method.
            called_exception_base = true;
            continue;
        }
        if let Some(name) = init_hooks::called_method(statement, receiver)
            && name != "__init__"
            && let Some(writes) = lookup(name)
        {
            let writes = writes?;
            let before = initialized.iter().copied().map(str::to_owned).collect();
            for field in &writes {
                // The checked hook may also create an ordinary instance shadow.
                // That write does not satisfy any of the required physical fields.
                if matches!(bindings.get(field), Some(ClassValue::Runtime { .. })) {
                    continue;
                }
                let Some(&declared_field) = declared.get(field.as_str()) else {
                    return Err(
                        module.error(statement, "initializer hook writes an undeclared field")
                    );
                };
                if !initialized.insert(declared_field) {
                    return Err(module.error(
                        statement,
                        "initializer hook writes an already initialized field",
                    ));
                }
            }
            hooks.insert(
                name.to_owned(),
                init_hooks::Hook {
                    initialized: before,
                    writes,
                },
            );
            continue;
        }
        let assignment = match statement {
            ast::Stmt::Assign(assignment) => assignment,
            ast::Stmt::Expr(value) if matches!(&*value.value, ast::Expr::StringLiteral(_)) => {
                continue;
            }
            ast::Stmt::Expr(value)
                if let ast::Expr::Call(call) = &*value.value
                    && super_init(call) =>
            {
                if called_super
                    || inherited
                        .iter()
                        .any(|f| initialized.contains(f.name.as_str()))
                {
                    return Err(
                        module.error(call, "base initializer fields may only be initialized once")
                    );
                }
                if base_exposes_self
                    && fields
                        .iter()
                        .skip(inherited.len())
                        .any(|f| !initialized.contains(f.name.as_str()))
                {
                    return Err(module.error(call, "initialize subclass fields before calling a base initializer that exposes self"));
                }
                for argument in call
                    .arguments
                    .args
                    .iter()
                    .chain(call.arguments.keywords.iter().map(|kw| &kw.value))
                {
                    let mut use_self = SelfUse(receiver, None, &initialized);
                    use_self.visit_expr(argument);
                    if let Some(expr) = use_self.1 {
                        return Err(module.error(
                            expr,
                            "the incomplete instance cannot escape during construction",
                        ));
                    }
                }
                initialized.extend(inherited.iter().map(|f| f.name.as_str()));
                called_super = true;
                continue;
            }
            ast::Stmt::Assert(assertion) => {
                let mut use_self = SelfUse(receiver, None, &initialized);
                use_self.visit_expr(&assertion.test);
                if let Some(message) = &assertion.msg {
                    use_self.visit_expr(message);
                }
                if let Some(expr) = use_self.1 {
                    return Err(
                        module.error(expr, "an assertion cannot expose an incomplete instance")
                    );
                }
                continue;
            }
            ast::Stmt::Pass(_) => continue,
            _ => {
                return Err(module.error(
                    statement,
                    "__init__ must contain only straight-line local and self.field assignments, assertions, docstrings, or pass",
                ));
            }
        };
        let mut self_use = SelfUse(receiver, None, &initialized);
        self_use.visit_expr(&assignment.value);
        if let Some(expression) = self_use.1 {
            return Err(module.error(
                expression,
                "__init__ cannot expose self before construction or read an uninitialized field",
            ));
        }
        let mut targets = Vec::new();
        for target in &assignment.targets {
            initializer_targets(target, &mut targets);
        }
        for target in targets {
            if let ast::Expr::Name(name) = target {
                if name.id == receiver {
                    return Err(module.error(target, "an initializer cannot rebind its receiver"));
                }
                continue;
            }
            let ast::Expr::Attribute(attribute) = target else {
                return Err(module.error(target, "__init__ must assign a declared self.field"));
            };
            if !matches!(&*attribute.value, ast::Expr::Name(name) if name.id == receiver) {
                return Err(module.error(target, "__init__ must assign a declared self.field"));
            }
            let name = attribute.attr.as_str();
            if matches!(bindings.get(name), Some(ClassValue::Runtime { .. })) {
                continue;
            }
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
    }
    if !inherited.is_empty() && !called_super && !called_exception_base {
        return Err(module.error(initializer, "an explicit subclass initializer must call super().__init__ to initialize inherited fields"));
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
    Ok(false)
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

struct SelfUse<'a, 'b>(&'a str, Option<&'a ast::Expr>, &'b HashSet<&'a str>);

impl<'a> Visitor<'a> for SelfUse<'a, '_> {
    fn visit_expr(&mut self, expression: &'a ast::Expr) {
        if self.1.is_some() {
            return;
        }
        if let ast::Expr::Attribute(field) = expression
            && matches!(&*field.value, ast::Expr::Name(name) if name.id == self.0)
            && self.2.contains(field.attr.as_str())
        {
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
