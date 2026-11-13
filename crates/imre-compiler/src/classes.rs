//! Collects fixed class shapes before nominal type resolution and executable lowering.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use ruff_python_ast as ast;
use ruff_python_ast::visitor::{Visitor, walk_expr};

use crate::frontend::phase::{Diagnostic, Kind};
use crate::ir::Type;
use crate::modules::{
    Binding, DefinitionId, Intrinsic, IntrinsicModule, SourceGraph, SourceModule,
};

mod constants;
mod dataclasses;
mod enums;
mod exception_forwarders;
mod exceptions;
mod inheritance;
mod init_hooks;
mod init_mappings;
mod record_hash;
pub(crate) use constants::ClassValue;
pub(crate) use dataclasses::{ContainerFactory, DataclassOptions, FieldDefault};
pub(crate) use exception_forwarders::{ForwardingInitializer, mangle_fields, private_field};
pub(crate) use exceptions::explicit_message_initializer;
pub(crate) use inheritance::{direct_base_init, super_init, super_method};

pub(crate) const EXCEPTION_ARGS_SLOT: &str = "<exception arguments>";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MethodKind {
    Instance,
    Property,
    PropertySetter,
    Class,
    Static,
}

/// A class can bind the same function under its method name and a property name.
#[derive(Clone)]
pub(crate) struct ClassMethod {
    pub(crate) name: ast::Identifier,
    pub(crate) function: Arc<ast::StmtFunctionDef>,
}

impl ruff_text_size::Ranged for ClassMethod {
    fn range(&self) -> ruff_text_size::TextRange {
        self.function.range
    }
}

impl From<ast::StmtFunctionDef> for ClassMethod {
    fn from(function: ast::StmtFunctionDef) -> Self {
        Self {
            name: function.name.clone(),
            function: Arc::new(function),
        }
    }
}

impl std::ops::Deref for ClassMethod {
    type Target = ast::StmtFunctionDef;

    fn deref(&self) -> &Self::Target {
        &self.function
    }
}

pub(crate) struct Classes<'syntax> {
    pub(crate) definitions: Vec<Class<'syntax>>,
    /// Data members only need rebindable storage when some source writes that attribute.
    pub(crate) attribute_writes: HashMap<String, HashSet<Type>>,
    pub(crate) exceptions: HashMap<DefinitionId, u32>,
    pub(crate) registered_classes: HashSet<DefinitionId>,
    pub(crate) exception_declarations: Vec<crate::types::NamedException>,
    pub(crate) ids: HashMap<DefinitionId, usize>,
}

#[derive(Clone)]
pub(crate) enum ExceptionAlias {
    Builtin { kind: u32, name: String },
    Named(DefinitionId),
}

/// Arguments to a sealed unittest class decorator, evaluated by the module initializer.
pub(crate) struct SkipDecorator {
    pub(crate) unless: bool,
    pub(crate) condition: ast::Expr,
    pub(crate) reason: ast::Expr,
}

pub(crate) struct Class<'syntax> {
    pub(crate) skip_decorators: Vec<SkipDecorator>,
    pub(crate) definition: DefinitionId,
    pub(crate) name: String,
    pub(crate) node: Cow<'syntax, ast::StmtClassDef>,
    pub(crate) fields: Vec<Field>,
    pub(crate) methods: Vec<ClassMethod>,
    /// Field writes required of every override called during construction.
    pub(crate) init_hooks: init_hooks::Hooks,
    /// A base initializer that passes self to Python needs the entire derived layout ready.
    pub(crate) init_exposes_self: bool,
    pub(crate) method_kinds: Vec<MethodKind>,
    /// Declaring class for the corresponding method's imports and lexical super.
    pub(crate) method_origins: Vec<DefinitionId>,
    /// The parent whose field layout and initializer the receiver inherits.
    pub(crate) storage_base: Option<usize>,
    /// Ancestors in Python method lookup order.
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

impl Class<'_> {
    pub(crate) fn is_uuid(&self, graph: &SourceGraph) -> bool {
        self.name == "UUID" && graph.modules[self.definition.module].is_bundled("uuid")
    }

    pub(crate) fn has_literal_hook(&self, name: &str) -> bool {
        self.init_hooks
            .get(name)
            .is_some_and(|hook| hook.writes.is_empty())
    }

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
    pub(crate) weak_proxy: bool,
    pub(crate) default: Option<FieldDefault>,
    pub(crate) kw_only: bool,
    pub(crate) repr: bool,
    pub(crate) compare: bool,
}

fn context_method(name: &str) -> bool {
    matches!(name, "__enter__" | "__exit__")
}

fn empty_slots(name: &str, value: &ast::Expr) -> bool {
    name == "__slots__" && matches!(value, ast::Expr::Tuple(t) if t.elts.is_empty())
}

impl<'syntax> Classes<'syntax> {
    pub(crate) fn has_empty_slots(&self, child: usize) -> bool {
        std::iter::once(&child)
            .chain(&self.definitions[child].bases)
            .all(|&id| {
                self.definitions[id].node.body.iter().any(|statement| {
                    let ast::Stmt::Assign(a) = statement else { return false };
                    matches!(a.targets.as_slice(), [ast::Expr::Name(n)] if empty_slots(n.id.as_str(), &a.value))
                })
            })
    }

    pub(crate) fn subclass(&self, graph: &SourceGraph, child: usize, parent: usize) -> bool {
        let bases = &self.definitions[child].bases;
        if child == parent || bases.contains(&parent) {
            return true;
        }
        graph.is_abstract_context_manager(self.definitions[parent].definition)
            && ["__enter__", "__exit__"].iter().all(|&name| {
                std::iter::once(&child).chain(bases).find_map(|&id| {
                    self.definitions[id]
                        .node
                        .body
                        .iter()
                        .rev()
                        .find_map(|stmt| match stmt {
                            ast::Stmt::FunctionDef(f) if f.name.as_str() == name => Some(true),
                            ast::Stmt::Assign(a)
                                if a.targets.iter().any(|target| {
                                    target.as_name_expr().is_some_and(|n| n.id.as_str() == name)
                                }) =>
                            {
                                Some(!matches!(&*a.value, ast::Expr::NoneLiteral(_)))
                            }
                            _ => None,
                        })
                }) == Some(true)
            })
    }

    /// The entry module may need to execute class bodies even without module globals.
    pub(crate) fn has_class_values(&self, module: usize) -> bool {
        self.definitions.iter().any(|class| {
            class.definition.module == module && (!class.skip_decorators.is_empty() || class.static_bindings.values().any(|value| {
                matches!(value, ClassValue::Runtime { .. })
                    || matches!(value, ClassValue::Strings { owner, .. } if *owner == class.definition)
            }))
        })
    }

    pub(crate) fn instance(&self, id: usize) -> Type {
        if let Some(members) = &self.definitions[id].enum_members {
            if members.http == Some(enums::HttpEnum::Status) {
                Type::IntEnum(id)
            } else if members.plain {
                Type::Enum(id)
            } else {
                Type::StrEnum(id)
            }
        } else {
            Type::Class(id)
        }
    }

    #[cfg(test)]
    pub(crate) fn collect(graph: &'syntax SourceGraph) -> Result<Self, Diagnostic> {
        Self::collect_layouts(graph, None)
    }

    /// Assigns nominal identities in module and source order. A registered binder may
    /// retain completed declarations when a fixed layout reports its first error.
    pub(crate) fn collect_layouts(
        graph: &'syntax SourceGraph,
        first_layout: Option<&mut Option<Diagnostic>>,
    ) -> Result<Self, Diagnostic> {
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
                    // Select the boxed base before inferring fixed layouts for any mixins.
                    if let Some((base, name)) = node
                        .arguments
                        .iter()
                        .flat_map(|args| &args.args)
                        .find_map(|base| {
                            if graph.session.is_known_class(
                                module_id,
                                base,
                                ty_python_semantic::types::KnownClass::Str,
                            ) {
                                return Some((base, "str subclasses"));
                            }
                            let Binding::Intrinsic(intrinsic) = graph.resolve(module_id, base)?
                            else {
                                return None;
                            };
                            Some((base, intrinsic.scoped_class_base()?))
                        })
                    {
                        return Err(module
                            .diagnostic(base, &format!("{name} requires object lowering"))
                            .classified(Kind::ScopedClassLayout));
                    }
                    ids.insert(definition, source_nodes.len());
                    let mut record = Cow::Borrowed(node);
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
                    // An inactive unittest skip returns the original function. Normalize only
                    // calls whose arguments and namespace lookups cannot execute Python code.
                    for (index, statement) in node.body.iter().enumerate() {
                        if let ast::Stmt::FunctionDef(method) = statement
                            && !method.decorator_list.is_empty()
                            && method.decorator_list.iter().all(|decorator| {
                                identity_skip_decorator(graph, module_id, decorator)
                            })
                            && let ast::Stmt::FunctionDef(normalized) =
                                &mut record.to_mut().body[index]
                        {
                            normalized.decorator_list.clear();
                        }
                    }
                    source_nodes.push((definition, record));
                }
            }
        }
        let (definitions, mut registered_classes) =
            inheritance::resolve(graph, &source_nodes, &mut ids, &resolved, first_layout)?;
        registered_classes.extend(resolved.registered);
        // An inner class uses the object runtime, but every fixed class must validate first.
        if let Some(error) = graph.class_layout_error.borrow().as_ref() {
            return Err(error.clone());
        }
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
            registered_classes,
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
        Some(Binding::Intrinsic(
            Intrinsic::Mapping | Intrinsic::TYPING_MAPPING
        ))
    )
}

/// Recognize a factory on an imported source module while the class namespace is still being built.
fn source_method_factory(
    graph: &SourceGraph,
    module: usize,
    decorator: &ast::Decorator,
    class_names: &HashSet<String>,
) -> bool {
    let ast::Expr::Call(call) = &decorator.expression else {
        return false;
    };
    let mut root = call.func.as_ref();
    while let ast::Expr::Attribute(attr) = root {
        root = &attr.value;
    }
    let Some(owner) = root.as_name_expr() else {
        return false;
    };
    let Some(Binding::Definition(id)) = graph.resolve(module, &call.func) else {
        return false;
    };
    let source = &graph.modules[id.module];
    id.module != module
        && !class_names.contains(owner.id.as_str())
        && !source.bundled
        && matches!(
            source.parsed.syntax().body[id.statement],
            ast::Stmt::FunctionDef(_)
        )
}

fn unittest_skip_decorator(
    graph: &SourceGraph,
    module: usize,
    decorator: &ast::Decorator,
    class_names: Option<&HashSet<String>>,
) -> Result<Option<SkipDecorator>, String> {
    let ast::Expr::Call(call) = &decorator.expression else {
        return Ok(None);
    };
    let ast::Expr::Attribute(attr) = call.func.as_ref() else {
        return Ok(None);
    };
    if class_names.is_some_and(|names| {
        attr.value
            .as_name_expr()
            .is_none_or(|owner| names.contains(owner.id.as_str()))
    }) {
        return Ok(None);
    }
    let Some(Binding::Module(owner)) = graph.resolve(module, &attr.value) else {
        return Ok(None);
    };
    if !graph.modules[owner].is_bundled("unittest") {
        return Ok(None);
    }
    let unless = match attr.attr.as_str() {
        "skipIf" => false,
        "skipUnless" => true,
        _ => return Ok(None),
    };
    let ([condition, reason], []) = (
        call.arguments.args.as_ref(),
        call.arguments.keywords.as_ref(),
    ) else {
        return Err(graph.modules[module].error(
            call,
            "native unittest skip decorators require condition and reason as positional arguments",
        ));
    };
    Ok(Some(SkipDecorator {
        unless,
        condition: condition.clone(),
        reason: reason.clone(),
    }))
}

fn identity_skip_decorator(graph: &SourceGraph, module: usize, decorator: &ast::Decorator) -> bool {
    let ast::Expr::Call(call) = &decorator.expression else {
        return false;
    };
    let ast::Expr::Attribute(attr) = &*call.func else {
        return false;
    };
    let Ok(Some(skip)) = unittest_skip_decorator(graph, module, decorator, None) else {
        return false;
    };
    if !matches!(skip.reason, ast::Expr::StringLiteral(_))
        || !graph
            .session
            .is_stdlib_module(module, &attr.value, "unittest")
    {
        return false;
    }
    let truth = match &skip.condition {
        ast::Expr::BooleanLiteral(literal) => literal.value,
        ast::Expr::Attribute(attr) if graph.session.is_stdlib_module(module, &attr.value, "os") => {
            let Some(Binding::Intrinsic(Intrinsic::BoolConstant(value))) =
                graph.resolve(module, &skip.condition)
            else {
                return false;
            };
            value
        }
        ast::Expr::Call(probe)
            if probe
                .func
                .as_name_expr()
                .is_some_and(|name| name.id == "hasattr")
                && graph.resolve(module, &probe.func).is_none()
                && probe.arguments.keywords.is_empty()
                && let [object @ ast::Expr::Name(_), ast::Expr::StringLiteral(name)] =
                    probe.arguments.args.as_ref()
                && graph
                    .session
                    .is_stdlib_function(module, &probe.func, "builtins", "hasattr")
                && graph.session.is_stdlib_module(module, object, "os")
                && graph.resolve(module, object)
                    == Some(Binding::IntrinsicModule(IntrinsicModule::Os))
                && matches!(
                    IntrinsicModule::Os.symbol(name.value.to_str()),
                    Some(Binding::Intrinsic(Intrinsic::Credential(_)))
                ) =>
        {
            true
        }
        _ => return false,
    };
    truth == skip.unless
}

pub(crate) fn defines_pickle_reducer(node: &ast::StmtClassDef) -> bool {
    let reducer = |name: &str| matches!(name, "__reduce__" | "__reduce_ex__" | "__getstate__");
    let binding = |target: &ast::Expr| {
        target
            .as_name_expr()
            .is_some_and(|n| reducer(n.id.as_str()))
    };
    node.body.iter().any(|stmt| match stmt {
        ast::Stmt::FunctionDef(f) => reducer(f.name.as_str()),
        ast::Stmt::Assign(a) => a.targets.iter().any(binding),
        ast::Stmt::AnnAssign(a) => a.value.is_some() && binding(&a.target),
        _ => false,
    })
}

// Keep a decorator error local to its class until we know how that class is built.
#[derive(Default)]
struct DeferredMethodDecorators {
    has_runtime_method: bool,
    unsupported: Option<Diagnostic>,
}

fn collect_class<'syntax>(
    graph: &SourceGraph,
    module: &SourceModule,
    definition: DefinitionId,
    source: &Cow<'syntax, ast::StmtClassDef>,
    parents: [Option<(usize, &Class)>; 2],
    declarations: &exceptions::Declarations,
    decorators: &mut DeferredMethodDecorators,
) -> Result<Option<Class<'syntax>>, Diagnostic> {
    let node = source.as_ref();
    let exception_layout = declarations.layouts.contains(&definition);
    let exceptions = &declarations.ids;
    let base = parents[0];
    if defines_pickle_reducer(node) {
        graph.needs_object_runtime.set(true);
        return Err(module.diagnostic(node, "pickle reducers require the object runtime"));
    }
    let http = enums::HttpEnum::bundled(module, node);
    let is_enum = http.is_some() || node.arguments.as_ref().is_some_and(|arguments| {
        matches!(arguments.args.as_ref(), [base] if matches!(graph.resolve(definition.module, base), Some(Binding::Intrinsic(Intrinsic::StrEnum | Intrinsic::StandardEnum))))
            && arguments.keywords.is_empty()
    });
    let direct_mapping = node.arguments.as_ref().is_some_and(|args| {
        args.args
            .iter()
            .any(|arg| mapping_base(graph, definition.module, arg))
    });
    let mapping = direct_mapping || base.is_some_and(|(_, parent)| parent.mapping);
    let keywords = node
        .arguments
        .as_ref()
        .map_or(&[][..], |args| args.keywords.as_ref());
    let abc_meta = match keywords {
        [] => false,
        [key]
            if key.arg.as_deref() == Some("metaclass")
                && graph.resolve(definition.module, &key.value)
                    == Some(Binding::Intrinsic(Intrinsic::AbcMeta))
                && graph.session.is_known_class(
                    definition.module,
                    &key.value,
                    ty_python_semantic::types::KnownClass::ABCMeta,
                ) =>
        {
            true
        }
        _ => {
            return Err(module.diagnostic(
                node,
                "class metaclasses and keyword arguments are unsupported",
            ));
        }
    };
    let abc = abc_meta
        || mapping
        || base.is_some_and(|(_, parent)| parent.abc)
        || node.arguments.as_ref().is_some_and(|args| {
            args.args.iter().any(|arg| {
                graph.resolve(definition.module, arg) == Some(Binding::Intrinsic(Intrinsic::Abc))
            })
        });
    if node.type_params.is_some() {
        graph.needs_object_runtime.set(true);
        return Err(module.diagnostic(node, "class type parameters require the object runtime"));
    }
    if is_enum && http.is_none() && !node.decorator_list.is_empty() {
        return Err(module.diagnostic(node, "decorated StrEnum declarations are unsupported"));
    }
    let mut enum_members = is_enum.then(enums::Members::default);
    if let Some(members) = &mut enum_members {
        members.http = http;
        members.plain = node.arguments.as_ref().is_some_and(|args| {
            graph.resolve(definition.module, &args.args[0])
                == Some(Binding::Intrinsic(Intrinsic::StandardEnum))
        });
    }
    let mut skip_decorators = Vec::new();
    for decorator in &node.decorator_list {
        let Some(skip) = unittest_skip_decorator(graph, definition.module, decorator, None)? else {
            skip_decorators.clear();
            break;
        };
        skip_decorators.push(skip);
    }
    let dataclass = if http.is_some() || !skip_decorators.is_empty() {
        None
    } else {
        dataclasses::options(graph, definition.module, node)?
    };
    let value_struct = dataclass.is_some_and(|options| options.value_struct);
    if base.is_some_and(|(_, parent)| parent.dataclass.is_some_and(|options| options.value_struct))
        || value_struct && node.arguments.as_ref().is_some_and(|args| !args.is_empty())
    {
        return Err(module.diagnostic(node, "an imre.struct is final and cannot inherit fields"));
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
                    return Err(module.diagnostic(
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
        return Err(module.diagnostic(
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
        return Err(module.diagnostic(node, "incompatible base for a fixed-layout class"));
    }
    let mut methods: Vec<ClassMethod> = Vec::new();
    let mut method_kinds = Vec::new();
    let mut needs_object_layout = false;
    let mut builds_python_namespace = false;
    let mut initializer = None;
    for (ordinal, statement) in node.body.iter().enumerate() {
        let statement = graph.runtime_class_item(definition.module, statement);
        // A call through an upcast uses its declared layout. Do not erase an
        // override which could select a different callable at runtime.
        let member = match statement {
            ast::Stmt::FunctionDef(f) => Some(f.name.as_str()),
            ast::Stmt::ClassDef(c) => Some(c.name.as_str()),
            ast::Stmt::AnnAssign(a) => a.target.as_name_expr().map(|n| n.id.as_str()),
            ast::Stmt::Assign(a) => a
                .targets
                .first()
                .and_then(ast::Expr::as_name_expr)
                .map(|n| n.id.as_str()),
            _ => None,
        };
        if let Some(name) = member
            && let Some(binding) = base.and_then(|(_, parent)| parent.static_bindings.get(name))
        {
            match (binding, statement) {
                (_, ast::Stmt::Assign(a)) if empty_slots(name, &a.value) => {}
                (_, ast::Stmt::FunctionDef(_)) if context_method(name) => {
                    static_bindings.remove(name);
                }
                (ClassValue::Runtime { .. }, ast::Stmt::Assign(_)) => {}
                (ClassValue::String(_), ast::Stmt::Assign(assign))
                    if matches!(&*assign.value, ast::Expr::StringLiteral(_)) => {}
                (
                    ClassValue::String(_) | ClassValue::Runtime { .. },
                    ast::Stmt::FunctionDef(method),
                ) if method_kind(graph, definition.module, method, &class_bindings)?
                    == MethodKind::Property =>
                {
                    static_bindings.remove(name);
                }
                _ => {
                    return Err(module
                        .diagnostic(statement, "overriding a fixed class binding is unsupported"));
                }
            }
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
                    return Err(module.diagnostic(name, "duplicate class binding"));
                }
                abstract_methods.remove(name.id.as_str());
                class_bindings.insert(name.id.to_string());
                exception_aliases.insert(name.id.to_string(), target);
            }
            ast::Stmt::AnnAssign(field)
                if !is_enum && (field.value.is_none() || dataclass.is_some()) =>
            {
                if dataclass.is_some() && !field.simple {
                    return Err(module.diagnostic(
                        field,
                        "dataclass fields require an unparenthesized name annotation",
                    ));
                }
                let ast::Expr::Name(name) = &*field.target else {
                    return Err(
                        module.diagnostic(field, "a field declaration must annotate one name")
                    );
                };
                let name = name.id.as_str();
                if name.starts_with("__") {
                    return Err(
                        module.diagnostic(field, "field names starting with '__' are reserved")
                    );
                }
                if names.contains(name) {
                    return Err(
                        module.diagnostic(field, &format!("duplicate class member {name:?}"))
                    );
                }
                if base
                    .is_some_and(|(_, base)| base.methods.iter().any(|m| m.name.as_str() == name))
                {
                    return Err(module
                        .diagnostic(field, "a fixed field cannot replace an inherited method"));
                }
                let declared_field = dataclasses::field(
                    graph,
                    definition.module,
                    field,
                    name,
                    dataclass.as_ref(),
                    &class_bindings,
                )?;
                // A scalar default remains on the class. A default factory initializes
                // only the instance, so it cannot implement an inherited abstract method.
                if matches!(declared_field.default, Some(FieldDefault::Scalar(_))) {
                    abstract_methods.remove(name);
                } else if dataclass.is_some_and(|options| options.slots)
                    && abstract_methods.contains(name)
                {
                    return Err(module.diagnostic(
                        field,
                        "a slot replacing an abstract method requires native class construction",
                    ));
                }
                if let Some(index) = fields.iter().position(|inherited| inherited.name == name) {
                    if dataclass.is_none() {
                        return Err(
                            module.diagnostic(node, "an inherited field cannot be redeclared")
                        );
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
            // The immutable CPython constructors initialize the member table, including
            // HTTPMethod's custom repr. Category properties keep their original bodies.
            ast::Stmt::FunctionDef(method)
                if http.is_some() && method.name.as_str() == "__new__"
                    || http == Some(enums::HttpEnum::Method)
                        && method.name.as_str() == "__repr__" => {}
            ast::Stmt::FunctionDef(method) => {
                let abstract_method = matches!(method.decorator_list.as_slice(), [decorator]
                    if graph.resolve(definition.module, &decorator.expression) == Some(Binding::Intrinsic(Intrinsic::AbstractMethod)));
                if !abstract_method && !graph.includes_method(definition.module, method) {
                    abstract_methods.remove(method.name.as_str());
                    continue;
                }
                if method.type_params.is_some() {
                    let postponed = imre_ast::future_annotations(&module.parsed.syntax().body);
                    use crate::frontend::function_scope::{
                        TypeParameterScope, type_parameter_scope,
                    };
                    match type_parameter_scope(method, postponed, true, None) {
                        Some(TypeParameterScope::MethodClosure) => {
                            let name = method.name.to_string();
                            if !names.insert(name.clone()) {
                                return Err(module.diagnostic(method, "duplicate class binding"));
                            }
                            class_bindings.insert(name);
                            graph.class_layout_error.borrow_mut().get_or_insert_with(|| {
                                module.diagnostic(method, "generic method requires a registered type-parameter scope")
                                    .classified(Kind::ScopedClassLayout)
                            });
                            continue;
                        }
                        Some(TypeParameterScope::Function) if postponed => {
                            graph.needs_object_runtime.set(true);
                        }
                        _ => {}
                    }
                    return Err(module.diagnostic(method, "method type parameters are unsupported"));
                }
                let name = method.name.as_str();
                if base.is_some_and(|(_, base)| base.fields.iter().any(|field| field.name == name))
                {
                    return Err(module
                        .diagnostic(method, "a method cannot replace an inherited fixed field"));
                }
                if abstract_method {
                    if name.starts_with("__") && !name.ends_with("__") {
                        return Err(module.diagnostic(
                            method,
                            "a private abstract method requires name mangling",
                        ));
                    }
                    if crate::modules::definition_effect(graph, definition.module, method) {
                        return Err(module.diagnostic(
                            method,
                            "an abstract method default requires executable initialization",
                        ));
                    }
                    if !abc || !method.body.iter().all(|stmt| matches!(stmt, ast::Stmt::Pass(_))
                        || matches!(stmt, ast::Stmt::Expr(value) if matches!(&*value.value, ast::Expr::StringLiteral(_) | ast::Expr::EllipsisLiteral(_)))) {
                        return Err(module.diagnostic(method, "an abstract record method must be a declaration on an ABC"));
                    }
                    abstract_methods.insert(name.to_owned());
                    if !graph.is_abstract_context_manager(definition) {
                        if !names.insert(name.to_owned()) {
                            return Err(module.diagnostic(method, "duplicate abstract method"));
                        }
                        class_bindings.insert(name.to_owned());
                        continue;
                    }
                } else {
                    abstract_methods.remove(name);
                }
                if name == "__hash__" {
                    if hash.is_some() || !dataclass.is_some_and(|d| d.frozen && d.eq) {
                        return Err(module.diagnostic(
                            method,
                            "a custom hash requires a frozen equality dataclass",
                        ));
                    }
                    hash = Some(method);
                    names.insert(name.to_owned());
                    class_bindings.insert(name.to_owned());
                    continue;
                }
                if is_enum && (name.starts_with('_') || matches!(name, "name" | "value")) {
                    return Err(module.diagnostic(
                        method,
                        "StrEnum methods cannot override member metadata or special behavior",
                    ));
                }
                if matches!(name, "__dir__" | "__next__") {
                    // These hooks use the Python class namespace. Continue checking the
                    // other declarations before selecting its object representation.
                    graph
                        .class_layout_error
                        .borrow_mut()
                        .get_or_insert_with(|| {
                            module
                                .diagnostic(
                                    method,
                                    &format!("custom {name} requires native class construction"),
                                )
                                .classified(Kind::ScopedClassLayout)
                        });
                    needs_object_layout = true;
                    class_bindings.insert(name.to_owned());
                    continue;
                }
                if name == "__del__" && graph.command_startup.is_some() {
                    let error =
                        module.diagnostic(method, "class finalizers require the object runtime");
                    return Err(error.classified(Kind::ClassLayout));
                }
                let async_iterator = name == "__aiter__"
                    && method.is_async
                    && imre_ast::code_flags::is_generator(&method.body);
                let unary_protocol = async_iterator
                    || matches!(
                        name,
                        "__str__" | "__repr__" | "__bool__" | "__index__" | "__iter__" | "__copy__"
                    )
                    || (mapping && name == "__len__");
                if name.starts_with("__")
                    && !matches!(
                        name,
                        "__init__" | "__call__" | "__fspath__" | "__getitem__" | "__setitem__"
                    )
                    && !(name == "__getattr__" && graph.is_http_handler(definition))
                    && !unary_protocol
                    && !context_method(name)
                    && !(mapping && name == "__contains__")
                    && !(name == "__post_init__" && dataclass.is_some())
                {
                    return Err(module
                        .diagnostic(method, "unsupported special method on a fixed-layout class"));
                }
                let kind = if abstract_method {
                    MethodKind::Instance
                } else if let [decorator] = method.decorator_list.as_slice()
                    && unittest_skip_decorator(
                        graph,
                        definition.module,
                        decorator,
                        Some(&class_bindings),
                    )?
                    .is_some()
                {
                    // The public decorator can wrap even a direct method call and evaluates
                    // its arguments while the class namespace is being constructed.
                    let error = module.diagnostic(
                        decorator,
                        "unittest method skips require native class construction",
                    );
                    graph
                        .class_layout_error
                        .borrow_mut()
                        .get_or_insert(error.classified(Kind::ClassLayout));
                    graph.require_fallback_calls();
                    decorators.has_runtime_method = true;
                    MethodKind::Instance
                } else {
                    match method_kind(graph, definition.module, method, &class_bindings) {
                        Err(error)
                            if graph.command_startup.is_none() && !name.starts_with("__") =>
                        {
                            // An arbitrary decorator may bind a descriptor or a non-callable.
                            // Preserve the class name without inventing a fixed method slot.
                            if method.decorator_list.iter().any(|decorator| {
                                source_method_factory(
                                    graph,
                                    definition.module,
                                    decorator,
                                    &class_bindings,
                                )
                            }) {
                                // The class builder evaluates the factory and binds its result.
                                graph
                                    .class_layout_error
                                    .borrow_mut()
                                    .get_or_insert(error.clone().classified(Kind::ClassLayout));
                                graph.check_method_factories.set(true);
                                decorators.has_runtime_method = true;
                            }
                            decorators.unsupported.get_or_insert(error);
                            if !names.insert(name.to_owned()) {
                                return Err(module.diagnostic(
                                    method,
                                    &format!("duplicate class member {name:?}"),
                                ));
                            }
                            class_bindings.insert(name.to_owned());
                            continue;
                        }
                        kind => kind?,
                    }
                };
                if name == "__call__" && kind != MethodKind::Instance {
                    return Err(module.diagnostic(
                        method,
                        "a fixed callable record requires an instance __call__ method",
                    ));
                }
                if name == "__setitem__"
                    && (kind != MethodKind::Instance
                        || method.parameters.len() != 3
                        || method.parameters.posonlyargs.len() + method.parameters.args.len() != 3)
                {
                    return Err(
                        module.diagnostic(method, "__setitem__ requires self, key and value")
                    );
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
                        return Err(module
                            .diagnostic(method, "a property setter needs its preceding getter"));
                    }
                } else if !names.insert(name.to_owned()) {
                    return Err(
                        module.diagnostic(method, &format!("duplicate class member {name:?}"))
                    );
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
                    return Err(module.diagnostic(
                        method,
                        &format!(
                            "this method's first positional parameter must be named {receiver:?}"
                        ),
                    ));
                }
                if kind == MethodKind::Property
                    && (method.parameters.len() != 1 || first.unwrap().default.is_some())
                {
                    return Err(module.diagnostic(method, "a property getter must take only self"));
                }
                if kind == MethodKind::PropertySetter
                    && (method.parameters.posonlyargs.len() + method.parameters.args.len() != 2
                        || method.parameters.len() != 2)
                {
                    return Err(
                        module.diagnostic(method, "a property setter must take self and a value")
                    );
                }
                if (unary_protocol || name == "__post_init__")
                    && (kind != MethodKind::Instance
                        || method.parameters.len() != 1
                        || first.is_none_or(|parameter| parameter.default.is_some()))
                {
                    return Err(module.diagnostic(
                        method,
                        &format!("{name} must be an instance method taking only self"),
                    ));
                }
                builds_python_namespace |= async_iterator;
                if name == "__init__" {
                    if kind != MethodKind::Instance {
                        return Err(
                            module.diagnostic(method, "__init__ must be an instance method")
                        );
                    }
                    initializer = Some(methods.len());
                }
                let mut method = method.clone();
                if forwarder.is_some() {
                    mangle_fields(node.name.as_str(), &mut method, |_| true);
                }
                methods.push(method.into());
                method_kinds.push(kind);
                class_bindings.insert(name.to_owned());
            }
            ast::Stmt::Assign(assign)
                if let [ast::Expr::Name(name)] = assign.targets.as_slice()
                    && !class_bindings.contains("property")
                    && let Some(accessors) =
                        property_accessors(graph, definition.module, &assign.value) =>
            {
                if !names.insert(name.id.to_string()) {
                    return Err(module.diagnostic(name, "duplicate property binding"));
                }
                for (index, operand) in accessors.iter().enumerate() {
                    let Some(accessor) = operand.as_name_expr() else {
                        // Reading a write-only property raises through the Python descriptor.
                        builds_python_namespace |= index == 0;
                        continue;
                    };
                    let accessor = accessor.id.as_str();
                    let (kind, arity) = if index == 0 {
                        (MethodKind::Property, 1)
                    } else {
                        (MethodKind::PropertySetter, 2)
                    };
                    let Some((method, _)) = methods
                        .iter()
                        .zip(&method_kinds)
                        .find(|(m, k)| m.name.as_str() == accessor && **k == MethodKind::Instance)
                    else {
                        if !class_bindings.contains(accessor)
                            && let Some(Binding::Definition(id)) =
                                graph.resolve(definition.module, operand)
                            && graph.modules[id.module].parsed.syntax().body[id.statement]
                                .is_function_def_stmt()
                        {
                            // The class captures the existing function from its defining module.
                            builds_python_namespace = true;
                            continue;
                        }
                        return Err(module.diagnostic(
                            assign,
                            "property accessors must be preceding instance methods",
                        ));
                    };
                    if method.parameters.len() != arity
                        || method.parameters.posonlyargs.len() + method.parameters.args.len()
                            != arity
                    {
                        return Err(module.diagnostic(
                            assign,
                            "property accessor has the wrong number of positional parameters",
                        ));
                    }
                    methods.push(ClassMethod {
                        name: ast::Identifier::new(name.id.as_str(), name.range),
                        function: Arc::clone(&method.function),
                    });
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
                    // Named exceptions use exception_aliases so subclasses can
                    // override a ClassVar[type[Exception]] constructor.
                    .filter(|value| match value {
                        ClassValue::Binding(Binding::Definition(id)) => {
                            !exceptions.contains_key(id)
                        }
                        _ => true,
                    })
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
                let member = name.id.as_str();
                let protocol = context_method(member);
                let slots = empty_slots(member, &assign.value);
                if !names.insert(member.to_owned())
                    || (member.starts_with("__") && !protocol && !slots)
                    || base.is_some_and(|(_, b)| {
                        b.fields.iter().any(|f| f.name == member)
                            || (!protocol && b.methods.iter().any(|m| m.name.as_str() == member))
                            || b.exception_aliases.contains_key(member)
                    })
                {
                    return Err(
                        module.diagnostic(name, "duplicate or reserved static class binding")
                    );
                }
                class_bindings.insert(member.to_owned());
                static_bindings.insert(name.id.to_string(), binding);
            }
            ast::Stmt::Assign(assign)
                if let [ast::Expr::Name(name)] = assign.targets.as_slice()
                    && let Some(target) =
                        exceptions::alias(graph, definition.module, &assign.value, exceptions) =>
            {
                if !names.insert(name.id.to_string()) {
                    return Err(module.diagnostic(name, "duplicate class binding"));
                }
                class_bindings.insert(name.id.to_string());
                exception_aliases.insert(name.id.to_string(), target);
            }
            ast::Stmt::Expr(value) if matches!(&*value.value, ast::Expr::StringLiteral(_)) => {}
            ast::Stmt::ClassDef(nested) if !is_enum && dataclass.is_none() && !exception_layout => {
                let name = nested.name.as_str();
                if !names.insert(name.to_owned()) {
                    return Err(module.diagnostic(nested, "duplicate class binding"));
                }
                class_bindings.insert(name.to_owned());
                graph
                    .class_layout_error
                    .borrow_mut()
                    .get_or_insert_with(|| {
                        module
                            .diagnostic(nested, "nested classes require native class construction")
                            .classified(Kind::ClassLayout)
                    });
            }
            ast::Stmt::Pass(_) => {}
            ast::Stmt::If(branch)
                if branch
                    .body
                    .iter()
                    .chain(branch.elif_else_clauses.iter().flat_map(|arm| &arm.body))
                    .all(|statement| {
                        matches!(statement, ast::Stmt::FunctionDef(method)
                        if !method.is_async && method.type_params.is_none())
                    }) =>
            {
                // Class layout cannot select a method until the class suite runs.
                // The object emitter evaluates the test and binds only the chosen definitions.
                let error = module.diagnostic(
                    branch,
                    "conditional method definitions require native class construction",
                );
                return Err(error.classified(Kind::ClassLayout));
            }
            _ => {
                return Err(module.diagnostic(
                    statement,
                    "class bodies support field declarations, plain data assignments, methods, pass, and docstrings; annotated field defaults require @dataclass",
                ));
            }
        }
        // Every accepted plain assignment binds a concrete class attribute,
        // including property() and exception constructor aliases.
        if matches!(statement, ast::Stmt::Assign(_))
            && let Some(name) = member
        {
            abstract_methods.remove(name);
        }
    }
    // A dataclass updates abstract methods after installing its generated attributes.
    if let Some(options) = dataclass
        && abstract_methods.iter().any(|name| match name.as_str() {
            "__init__" => options.init,
            "__repr__" => options.repr,
            "__eq__" | "__hash__" => options.eq,
            "__setattr__" | "__delattr__" => options.frozen,
            "__slots__" => options.slots,
            "__getstate__" | "__setstate__" => options.frozen && options.slots,
            "__match_args__" | "__replace__" | "__dataclass_fields__" | "__dataclass_params__" => {
                true
            }
            _ => false,
        })
    {
        return Err(module.diagnostic(node, "a dataclass-generated override of an abstract method requires native class construction"));
    }
    let mut method_origins = vec![definition; methods.len()];
    if let Some(method) = hash
        && !record_hash::toml_fields(method, &fields, graph, definition.module)
    {
        return Err(module.diagnostic(
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
                    weak_proxy: false,
                    default: None,
                    kw_only: false,
                    repr: true,
                    compare: true,
                });
            }
        }
    }
    let mut open_parameter_field = false;
    if !is_enum
        && dataclass.is_none()
        && let Some(init) = initializer
    {
        let infer = fields.len() == inherited_fields.len();
        inheritance::annotated_fields(
            graph,
            definition.module,
            Arc::make_mut(&mut methods[init].function),
            &mut fields,
        );
        if infer {
            init_mappings::infer(graph, definition.module, init, &mut methods, &mut fields);
            open_parameter_field =
                inheritance::infer_fields(graph, definition.module, &methods[init], &mut fields);
            // A write to inherited or local class data creates an instance shadow.
            fields.retain(|f| {
                !matches!(
                    static_bindings.get(&f.name),
                    Some(ClassValue::Runtime { .. })
                )
            });
        }
    }
    if fields
        .iter()
        .any(|field| static_bindings.contains_key(&field.name))
    {
        return Err(module.diagnostic(
            node,
            "an instance field cannot shadow a fixed class binding",
        ));
    }
    if value_struct && fields.is_empty() {
        return Err(module.diagnostic(node, "imre.struct needs at least one field"));
    }
    let generated_initializer =
        initializer.is_none() && dataclass.is_some_and(|options| options.init);
    let initializer = if generated_initializer {
        let index = methods.len();
        methods.push(dataclasses::initializer(module, node, &fields)?.into());
        method_kinds.push(MethodKind::Instance);
        method_origins.push(definition);
        Some(index)
    } else if is_enum {
        if enum_members.as_ref().unwrap().values.is_empty() {
            return Err(module.diagnostic(node, "a StrEnum requires at least one declared member"));
        }
        None
    } else {
        if dataclass.is_some_and(|options| options.frozen) {
            return Err(
                module.diagnostic(node, "frozen dataclasses require a generated initializer")
            );
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
            methods.push(dataclasses::initializer(module, node, &[])?.into());
            method_kinds.push(MethodKind::Instance);
            method_origins.push(definition);
            Some(index)
        } else {
            return Err(module.diagnostic(
                node,
                "a fixed class requires an explicit or generated __init__ method",
            ));
        }
    };
    let mut bases = Vec::new();
    for (position, (id, base)) in parents.into_iter().flatten().enumerate() {
        // A trailing independent root follows the entire primary lineage in the MRO.
        bases.push(id);
        bases.extend_from_slice(&base.bases);
        for (index, method) in base.methods.iter().enumerate() {
            if method.name.as_str() != "__init__" && !names.contains(method.name.as_str()) {
                if base.method_kinds[index] == MethodKind::Class
                    && !graph.inherits_class_method(definition, method.name.as_str())
                {
                    continue;
                }
                if position > 0 {
                    let name = method.name.as_str();
                    if methods.iter().any(|earlier| earlier.name.as_str() == name) {
                        continue;
                    }
                    if fields.iter().any(|field| field.name == name)
                        || static_bindings.contains_key(name)
                        || exception_aliases.contains_key(name)
                    {
                        return Err(module.diagnostic(
                            node,
                            "a mixin method conflicts with a fixed class member",
                        ));
                    }
                }
                methods.push(method.clone());
                method_kinds.push(base.method_kinds[index]);
                method_origins.push(base.method_origins[index]);
            }
        }
    }
    // ABCMeta initializes a fresh registry, shadowing even an inherited class binding.
    if abc
        && (class_bindings.contains("_abc_impl")
            || static_bindings.contains_key("_abc_impl")
            || exception_aliases.contains_key("_abc_impl")
            || methods
                .iter()
                .any(|method| method.name.as_str() == "_abc_impl")
            || abstract_methods.contains("_abc_impl")
            || fields.iter().any(|field| {
                field.name == "_abc_impl" && matches!(field.default, Some(FieldDefault::Scalar(_)))
            }))
    {
        return Err(module.diagnostic(
            node,
            "the ABCMeta registry shadows the _abc_impl class binding",
        ));
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
                return Err(module.diagnostic(
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
        // An object-backed class can create new instance fields in its initializer.
        && !needs_object_layout
        // A proven forwarding exception constructor writes each field through its plan.
        && forwarder.is_none()
    {
        if open_parameter_field && fields.is_empty() && base.is_none() && !exception_layout {
            return Ok(None);
        }
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
            |call| {
                base.is_some_and(|(_, parent)| {
                    direct_base_init(
                        graph,
                        definition.module,
                        parent.definition,
                        &methods[initializer],
                        call,
                    )
                })
            },
            |name| hook_writes(name, None),
        )?;
    }
    // Finish validating the declaration before constructing its Python namespace.
    if builds_python_namespace {
        return Ok(None);
    }
    Ok(Some(Class {
        definition,
        skip_decorators,
        forwarder,
        name: node.name.as_str().to_owned(),
        init_hooks,
        init_exposes_self,
        node: source.clone(),
        fields,
        toml_hash: hash.is_some(),
        methods,
        method_kinds,
        method_origins,
        storage_base: base.map(|(id, _)| id),
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
    }))
}

/// Recognize a fixed property declared from locally named accessors.
pub(crate) fn property_accessors<'a>(
    graph: &SourceGraph,
    module: usize,
    value: &'a ast::Expr,
) -> Option<&'a [ast::Expr]> {
    let ast::Expr::Call(call) = value else {
        return None;
    };
    if !matches!(&*call.func, ast::Expr::Name(n) if n.id == "property")
        || graph.resolve(module, &call.func).is_some()
        || !call.arguments.keywords.is_empty()
    {
        return None;
    }
    let args = call.arguments.args.as_ref();
    ((1..=2).contains(&args.len())
        && args
            .iter()
            .all(|arg| arg.is_name_expr() || arg.is_none_literal_expr()))
    .then_some(args)
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
        (false, Binding::Definition(id))
            if matches!(
                graph.modules[id.module].parsed.syntax().body[id.statement],
                ast::Stmt::ClassDef(_)
            ) =>
        {
            Some(binding)
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
) -> Result<MethodKind, Diagnostic> {
    let module = &graph.modules[module_id];
    let unsupported = |error: Diagnostic| {
        Err(if graph.command_startup.is_some() {
            error.classified(Kind::ClassLayout)
        } else {
            error
        })
    };
    let [decorator] = method.decorator_list.as_slice() else {
        return if method.decorator_list.is_empty() {
            Ok(MethodKind::Instance)
        } else {
            unsupported(module.diagnostic(method, "method decorators cannot be stacked"))
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
    unsupported(module.diagnostic(
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
    direct_base: impl Fn(&ast::ExprCall) -> bool,
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
            // The bundled method-name check reads the record's layout tag.
            // It neither publishes the receiver nor reads a subclass field.
            let metadata_only = module.is_bundled("unittest")
                && matches!(&*call.func, ast::Expr::Name(name) if name.id == "_validateCaseMethod");
            return Ok(!metadata_only);
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
                    && (super_init(call) || direct_base(call)) =>
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
                    .skip(usize::from(!super_init(call)))
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
            if let Some(name) = init_hooks::receiver_call(&assignment.value, receiver, true)
                && matches!(lookup(name), Some(Ok(writes)) if writes.is_empty())
            {
                hooks.entry(name.to_owned()).or_default();
            } else {
                return Err(module.error(
                    expression,
                    "__init__ cannot expose self before construction or read an uninitialized field",
                ));
            }
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
            if let ast::Expr::Subscript(item) = target
                && let ast::Expr::Attribute(field) = &*item.value
                && matches!(&*field.value, ast::Expr::Name(name) if name.id == receiver)
                && declared.contains(field.attr.as_str())
            {
                // Storing an item reads the container and its index. It initializes no field.
                let mut use_self = SelfUse(receiver, None, &initialized);
                use_self.visit_expr(target);
                if let Some(expr) = use_self.1 {
                    return Err(module.error(
                        expr,
                        "a subscript assignment cannot expose self or read an uninitialized field",
                    ));
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
            // A completed base initializer owns these fields. A subclass assignment
            // replaces their values through the same field store as an ordinary method.
            if !initialized.insert(name)
                && !(called_super && inherited.iter().any(|field| field.name == name))
            {
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
        if let Some(plan) = imre_ast::comprehension::Plan::from_expr(expression)
            && plan.binds_name(self.0)
        {
            self.visit_expr(plan.outer);
            return;
        }
        if matches!(expression, ast::Expr::Name(name) if name.id == self.0) {
            self.1 = Some(expression);
        } else {
            walk_expr(self, expression);
        }
    }
}
