//! Enforces the closed typed subset independently of gradual Python typing.

use std::collections::{HashMap, HashSet};

use ruff_python_ast as ast;
use ruff_text_size::{Ranged, TextRange};

use crate::classes::Classes;
use crate::ir::{self, Binary, Builtin, Compare, Expr, ExprKind, RegexCapture, Statement, Type};
use crate::modules::{Binding, DefinitionId, Intrinsic, ModuleId, SourceGraph};
use crate::narrowing;
use crate::types::Types;

mod annotations;
pub(crate) use annotations::{parameter_annotations, spelling as annotation_spelling};
mod argparse;
mod ascii;
mod assertion_syntax;
mod assignments;
mod asyncio;
mod binascii;
mod bytes;
mod cache_paths;
mod cached_properties;
mod calendar;
mod call_binding_errors;
mod callables;
mod caller_cache;
mod calls;
mod chainmap;
mod class_data;
mod closed_calls;
mod cmp_keys;
mod codecs;
mod comprehensions;
mod consuming_lists;
mod contexts;
mod contextvars;
mod copying;
mod count_elements;
mod counter;
mod csv;
mod dataclass_values;
mod defaultdict;
mod defaultdict_factory;
mod deque;
mod dispatch;
mod doctest;
mod documents;
mod enums;
mod errno;
mod exception_forwarders;
mod expandtabs;
mod expansions;
mod fixed_tuples;
mod fnmatch;
mod format_cursors;
mod fromkeys;
mod fspath;
mod function_caches;
mod function_text;
mod functools;
mod hashlib;
mod http_server;
mod import_errors;
mod inference;
mod inspect;
mod iterables;
mod itertools;
mod joined_lines;
mod json;
mod keyword_forwarding;
mod linecache;
mod literals;
mod local_classes;
mod local_functions;
mod logging;
mod logging_formatters;
mod logging_locks;
mod logging_records;
mod mapping_abc;
mod mapping_choice;
mod matching;
mod module_specs;
mod module_values;
mod namedtuples;
mod namespaces;
mod native_generators;
mod ord;
mod os;
mod partials;
mod patchset;
mod paths;
mod pipelines;
mod posixpath;
mod powers;
mod processes;
mod projected_equality;
mod properties;
mod random;
mod readiness;
mod record_inputs;
mod records;
mod regex;
mod sequences;
mod shutil;
mod sign_parameters;
mod single_dispatch;
mod socket;
mod stdlib;
mod string_io;
mod string_lists;
mod sys;
mod text;
mod time;
mod timed_handlers;
mod truthiness;
mod tuple_index;
mod typing;
mod unbound_locals;
mod unions;
mod unittest;
mod unittest_bound;
mod unittest_equality;
mod unittest_identity;
mod unittest_loader;
mod unittest_main;
mod unittest_mock;
mod unittest_patch;
mod unittest_setup;
mod unpacking;
mod urlsplit;
mod warnings;
mod weakref;
mod zip_sums;
mod zipfile;
use assignments::LocalState;
pub(crate) use caller_cache::CallerCache;
pub(crate) use calls::scalar_literal;
use calls::{Parameter, ParameterKind};
use comprehensions::Helpers;
pub(crate) use function_text::docstring;
use module_values::{Initialization, ModuleValues};

#[derive(Clone)]
struct Signature {
    id: usize,
    asynchronous: bool,
    params: Vec<Parameter>,
    returns: Type,
    method: Option<records::MethodContext>,
}

struct ClassInfo {
    parsers: argparse::Parsers,
    declarations: Classes,
    fields: Vec<Vec<Type>>,
    methods: Vec<HashMap<String, Signature>>,
    setters: Vec<HashMap<String, Signature>>,
    keyword_copies: HashSet<usize>,
    cached: HashMap<usize, function_caches::Policy>,
    cached_properties: cached_properties::Slots,
    setup_fields: unittest_setup::Slots,
    generators: HashSet<usize>,
    producer_items: HashMap<usize, Type>,
    contexts: HashSet<usize>,
}

impl ClassInfo {
    /// Resolves all nominal field types before checking constructors and methods.
    fn new(graph: &SourceGraph, types: &Types) -> Result<Self, String> {
        let mut declarations = Classes::collect(graph)?;
        types.set_exception_declarations(declarations.exception_declarations.clone());
        types.set_value_structs(
            declarations
                .definitions
                .iter()
                .enumerate()
                .filter_map(|(id, class)| {
                    class
                        .dataclass
                        .is_some_and(|d| d.value_struct)
                        .then_some(id)
                })
                .collect(),
        );
        types.set_pathlike_records(
            declarations
                .definitions
                .iter()
                .enumerate()
                .filter_map(|(id, class)| {
                    class
                        .methods
                        .iter()
                        .any(|method| method.name.as_str() == "__fspath__")
                        .then_some(id)
                })
                .collect(),
        );
        types.set_custom_truth(
            declarations
                .definitions
                .iter()
                .enumerate()
                .filter_map(|(id, class)| {
                    (class.mapping
                        || class
                            .methods
                            .iter()
                            .any(|method| method.name.as_str() == "__bool__"))
                    .then_some(id)
                })
                .collect(),
        );
        // Permit forward references while reading field annotations. Refine
        // these candidates once every record's concrete field types are known.
        let mut hashable = declarations
            .definitions
            .iter()
            .enumerate()
            .filter_map(|(id, class)| {
                class
                    .dataclass
                    .is_some_and(|options| options.frozen && options.eq)
                    .then_some(id)
            })
            .collect::<HashSet<_>>();
        types.set_hashable_records(hashable.clone());
        let mut fields = declarations
            .definitions
            .iter()
            .enumerate()
            .map(|(class_id, class)| {
                class
                    .fields
                    .iter()
                    .map(|field| {
                        if field.name == crate::classes::EXCEPTION_ARGS_SLOT
                            || class.forwarder.is_some()
                        {
                            return Ok(Type::ObjectValue);
                        }
                        annotation_in_class(
                            &field.annotation,
                            graph,
                            field.module,
                            &declarations,
                            types,
                            Some(class_id),
                        )
                        .and_then(|ty| {
                            if class.dataclass.is_some_and(|d| d.value_struct)
                                && !matches!(ty, Type::Float | Type::Bool)
                            {
                                return Err(error(
                                    &graph.modules[field.module].source,
                                    &field.annotation,
                                    "imre.struct fields must be float or bool",
                                ));
                            }
                            if field.weak_proxy {
                                let Type::Class(id) = ty else {
                                    return Err(error(
                                        &graph.modules[field.module].source,
                                        &field.annotation,
                                        "a weak proxy field requires a record annotation",
                                    ));
                                };
                                return Ok(Type::WeakProxy(id));
                            }
                            records::field_default(
                                field,
                                ty,
                                types,
                                &graph.modules[field.module].source,
                            )?;
                            if callables::contains_callable(ty, types) {
                                Err(error(
                                    &graph.modules[field.module].source,
                                    &field.annotation,
                                    "class fields cannot contain callable values",
                                ))
                            } else {
                                Ok(ty)
                            }
                        })
                        .map_err(|error| {
                            format!("{}:{error}", graph.modules[field.module].path.display())
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()
            })
            .collect::<Result<Vec<_>, _>>()?;
        for (id, class) in declarations.definitions.iter().enumerate() {
            if class.abstract_methods.is_empty() && !class.bases.is_empty() {
                types.add_concrete_subclass(id, &class.bases);
            }
            if let Some(&base) = class.bases.first()
                && !fields[id].starts_with(&fields[base])
            {
                return Err(graph.modules[class.definition.module].error(
                    &class.node,
                    "inherited fields must keep their exact types and offsets",
                ));
            }
        }
        let cached_properties = cached_properties::slots(graph, types, &declarations, &mut fields)?;
        let setup_fields = unittest_setup::slots(graph, types, &declarations, &mut fields);
        // Reference counting is sufficient only while no accepted field graph can
        // close a cycle. This concerns storage types, not method signatures.
        if let Some(id) = Self::cyclic_field_root(&fields, types) {
            let class = &declarations.definitions[id];
            return Err(graph.modules[class.definition.module].error(&class.node,
                "cyclic heap types are unsupported; class fields must form an acyclic ownership graph"));
        }
        loop {
            let before = hashable.len();
            hashable.retain(|&id| {
                declarations.definitions[id].toml_hash
                    || declarations.definitions[id]
                        .dataclass
                        .is_some_and(|d| d.value_struct)
                    || declarations.definitions[id]
                        .fields
                        .iter()
                        .zip(&fields[id])
                        .all(|(field, &ty)| !field.compare || types.immutable_key_field(ty))
            });
            if hashable.len() == before {
                break;
            }
            types.set_hashable_records(hashable.clone());
        }
        fn valid_keys(ty: Type, types: &Types) -> bool {
            types
                .mapping(ty)
                .map(|(key, _)| key)
                .or_else(|| types.set_element(ty))
                .is_none_or(|key| types.hashable_key(key))
                && types
                    .children(ty)
                    .into_iter()
                    .all(|child| valid_keys(child, types))
        }
        for (id, class) in declarations.definitions.iter().enumerate() {
            if class.toml_hash {
                if !fields[id]
                    .iter()
                    .all(|&ty| matches!(ty, Type::JsonObject | documents::MAP))
                {
                    return Err(graph.modules[class.definition.module].error(
                        &class.node,
                        "a TOML hash projection requires exact document object fields",
                    ));
                }
                types.mark_fallible_key(id);
            }
        }
        loop {
            let mut changed = false;
            for &id in &hashable {
                if declarations.definitions[id]
                    .fields
                    .iter()
                    .zip(&fields[id])
                    .any(|(field, &ty)| field.compare && types.fallible_key(ty))
                {
                    changed |= types.mark_fallible_key(id);
                }
            }
            if !changed {
                break;
            }
        }
        for (class, field_types) in declarations.definitions.iter().zip(&fields) {
            for (field, &ty) in class.fields.iter().zip(field_types) {
                if !valid_keys(ty, types) {
                    return Err(graph.modules[field.module].error(
                        &field.annotation,
                        "record keys require a frozen dataclass with immutable comparison fields",
                    ));
                }
            }
        }
        let parsers = argparse::Parsers::collect(graph, types, &mut declarations, &mut fields)?;
        Ok(Self {
            parsers,
            methods: vec![HashMap::new(); fields.len()],
            setters: vec![HashMap::new(); fields.len()],
            keyword_copies: HashSet::new(),
            cached: HashMap::new(),
            cached_properties,
            setup_fields,
            generators: HashSet::new(),
            producer_items: HashMap::new(),
            contexts: HashSet::new(),
            declarations,
            fields,
        })
    }

    fn cyclic_field_root(fields: &[Vec<Type>], types: &Types) -> Option<usize> {
        enum Step {
            Visit(Type),
            Finish(usize),
        }
        let mut states = vec![0; fields.len()];
        let mut pending = Vec::new();
        for root in 0..fields.len() {
            if states[root] != 0 {
                continue;
            }
            pending.push(Step::Visit(Type::Class(root)));
            while let Some(step) = pending.pop() {
                match step {
                    Step::Finish(id) => states[id] = 2,
                    Step::Visit(Type::Class(id)) => match states[id] {
                        1 => return Some(root),
                        2 => {}
                        _ => {
                            states[id] = 1;
                            pending.push(Step::Finish(id));
                            pending.extend(fields[id].iter().copied().map(Step::Visit));
                            // A base-typed field can own any concrete subclass.
                            pending.extend(
                                types
                                    .concrete_subclasses(id)
                                    .iter()
                                    .copied()
                                    .map(Step::Visit),
                            );
                        }
                    },
                    Step::Visit(ty) => {
                        pending.extend(types.children(ty).into_iter().map(Step::Visit));
                    }
                }
            }
        }
        None
    }
}

const RESERVED: &[&str] = &[
    "int",
    "float",
    "bool",
    "str",
    "repr",
    "property",
    "classmethod",
    "staticmethod",
    "bytes",
    "list",
    "tuple",
    "dict",
    "set",
    "len",
    "abs",
    "pow",
    "chr",
    "sorted",
    "divmod",
    "isinstance",
    "issubclass",
    "input",
    "print",
    "range",
    "enumerate",
    "map",
    "filter",
    "sum",
    "min",
    "max",
    "zip",
    "reversed",
    "any",
    "all",
    "open",
    "None",
    "True",
    "False",
    "__debug__",
    "__name__",
    "__doc__",
    "__file__",
];

/// Indices are runtime ABI categories. Empty entries belong to imported modules.
pub(crate) fn exception_kind(name: &str) -> Option<u32> {
    match name {
        "NotImplementedError" => return Some(37),
        "UnicodeEncodeError" => return Some(38),
        "NameError" => return Some(39),
        "UnboundLocalError" => return Some(40),
        "RecursionError" => return Some(43),
        "ReferenceError" => return Some(53),
        "BlockingIOError" => return Some(44),
        "ConnectionError" => return Some(45),
        "BrokenPipeError" => return Some(46),
        "ConnectionAbortedError" => return Some(47),
        "ConnectionRefusedError" => return Some(48),
        "ConnectionResetError" => return Some(49),
        "InterruptedError" => return Some(50),
        "TimeoutError" => return Some(51),
        "Warning" => return Some(80),
        "RuntimeWarning" => return Some(81),
        "DeprecationWarning" => return Some(82),
        _ => {}
    }
    if name == "ImportError" {
        return Some(41);
    }
    if name == "ModuleNotFoundError" {
        return Some(42);
    }
    if name == "StopIteration" {
        return Some(35);
    }
    if name == "ExceptionGroup" {
        return Some(34);
    }
    if name == "SystemExit" {
        return Some(32);
    }
    [
        "BaseException",
        "Exception",
        "ArithmeticError",
        "ZeroDivisionError",
        "OverflowError",
        "LookupError",
        "IndexError",
        "KeyError",
        "ValueError",
        "AssertionError",
        "RuntimeError",
        "EOFError",
        "OSError",
        "TypeError",
        "AttributeError",
        "MemoryError",
        "SystemError",
        "UnicodeError",
        "UnicodeDecodeError",
        // UnsupportedOperation belongs to io, not the builtin namespace.
        "",
        "FileNotFoundError",
        "PermissionError",
        "IsADirectoryError",
        "NotADirectoryError",
        "FileExistsError",
    ]
    .iter()
    .position(|candidate| !candidate.is_empty() && *candidate == name)
    .map(|kind| kind as u32)
}

fn reserved(name: &str) -> bool {
    RESERVED.contains(&name) || exception_kind(name).is_some()
}

fn pattern_elements(target: &ast::Expr) -> Option<&[ast::Expr]> {
    match target {
        ast::Expr::Tuple(tuple) => Some(&tuple.elts),
        ast::Expr::List(list) => Some(&list.elts),
        _ => None,
    }
}

/// Resolves the project-wide call graph without changing source-level namespaces.
#[cfg(test)]
pub(crate) fn lower(graph: &SourceGraph) -> Result<ir::Module, String> {
    lower_with_owners(graph).map(|(module, _)| module)
}

pub(crate) fn lower_with_owners(
    graph: &SourceGraph,
) -> Result<(ir::Module, Vec<ModuleId>), String> {
    lower_with_caller_cache(graph, None)
}

pub(crate) fn lower_with_caller_cache(
    graph: &SourceGraph,
    mut cache: Option<&mut CallerCache>,
) -> Result<(ir::Module, Vec<ModuleId>), String> {
    let types = Types::default();
    typing::collect_sites(graph, &types);
    namedtuples::collect(graph, &types)?;
    let mut classes = ClassInfo::new(graph, &types)?;
    dataclass_values::validate(&classes, graph, &types)?;
    let mut signatures = HashMap::new();
    let inference = inference::Inference::default();
    let mut definitions = Vec::new();
    let mut callable_definitions = Vec::new();
    for (module_id, module) in graph.modules.iter().enumerate() {
        for (name, binding) in &module.bindings {
            if reserved(name)
                && !matches!(
                    (name.as_str(), binding),
                    (
                        "open",
                        Binding::Intrinsic(
                            Intrinsic::GzipOpen | Intrinsic::Fd(ir::FdOperation::Open)
                        )
                    ) | (
                        "filter",
                        Binding::Intrinsic(Intrinsic::Fnmatch(ir::FnmatchOperation::Filter { .. }))
                    )
                )
            {
                return Err(module.error(
                    module.parsed.syntax(),
                    "a static binding uses a reserved builtin name",
                ));
            }
        }
        for (statement_id, statement) in module.parsed.syntax().body.iter().enumerate() {
            match statement {
                ast::Stmt::FunctionDef(function) => {
                    if !graph.includes(DefinitionId {
                        module: module_id,
                        statement: statement_id,
                    }) {
                        continue;
                    }
                    let cached = function_caches::policy(graph, module_id, function)?;
                    let mut function = function.clone();
                    if module.overloads.contains(&statement_id) {
                        graph.runtime_overload(module_id, function.name.range());
                        typing::strip_overload(&mut function);
                    }
                    if cached.is_some() {
                        function.decorator_list.clear();
                    }
                    let element = native_generators::item_type(
                        &function, graph, module_id, &classes, &types,
                    )?;
                    if element.is_some() {
                        if cached.is_some() {
                            return Err(
                                module.error(&function, "generator functions cannot be cached")
                            );
                        }
                        let range = function.returns.as_ref().unwrap().range();
                        function.returns =
                            Some(Box::new(ast::Expr::NoneLiteral(ast::ExprNoneLiteral {
                                range,
                                node_index: ast::AtomicNodeIndex::NONE,
                            })));
                    }
                    let defaults = if cached.is_some() {
                        // A cache can distinguish omitted keywords from explicitly supplied defaults.
                        keyword_forwarding::Defaults::local(module_id)
                    } else {
                        keyword_forwarding::prepare(graph, module_id, &mut function)
                    };
                    let signature = signature(
                        &function,
                        graph,
                        defaults,
                        &classes.declarations,
                        &types,
                        None,
                        &inference,
                    )
                    .map_err(|error| format!("{}:{error}", module.path.display()))?;
                    if let Some(policy) = cached {
                        if signature.asynchronous
                            || function.returns.is_none()
                            || !signature.params.iter().all(|p| {
                                types.hashable_key(p.ty)
                                    && p.default.is_none()
                                    && matches!(
                                        p.kind,
                                        ParameterKind::PositionalOnly
                                            | ParameterKind::PositionalOrKeyword
                                    )
                            })
                        {
                            return Err(module.error(&function, "cached functions require exact immutable positional keys and an annotated synchronous result"));
                        }
                        classes.cached.insert(signature.id, policy);
                    }
                    if let Some(element) = element {
                        classes.producer_items.insert(signature.id, element);
                    }
                    signatures.insert(
                        DefinitionId {
                            module: module_id,
                            statement: statement_id,
                        },
                        signature.clone(),
                    );
                    definitions.push((module_id, function.clone(), signature));
                    callable_definitions.push(DefinitionId {
                        module: module_id,
                        statement: statement_id,
                    });
                }
                ast::Stmt::Expr(value) => {
                    if let ast::Expr::StringLiteral(value) = &*value.value {
                        check_string(value, &module.source)
                            .map_err(|error| format!("{}:{error}", module.path.display()))?;
                    }
                }
                _ => {}
            }
        }
    }
    for (class_id, class) in classes.declarations.definitions.iter().enumerate() {
        let module_id = class.definition.module;
        let module = &graph.modules[module_id];
        for statement in &class.node.body {
            if let ast::Stmt::Expr(value) = statement
                && let ast::Expr::StringLiteral(value) = &*value.value
            {
                check_string(value, &module.source)
                    .map_err(|error| format!("{}:{error}", module.path.display()))?;
            }
        }
        let mut instance_signatures = HashMap::<_, Signature>::new();
        for (index, method) in class.methods.iter().enumerate() {
            if class.forwarder.is_some() && method.name.as_str() == "__init__" {
                continue;
            }
            let module_id = class.method_origins[index].module;
            let module = &graph.modules[module_id];
            let context = records::MethodContext {
                class: class_id,
                index,
            };
            let origin = (class.method_origins[index], method.range());
            let kind = class.method_kinds[index];
            // property(get, set) retains the original functions. Both spellings
            // must infer one signature even if only the property is used.
            if matches!(
                kind,
                crate::classes::MethodKind::Property | crate::classes::MethodKind::PropertySetter
            ) && let Some(original) = instance_signatures.get(&origin)
            {
                let mut alias = original.clone();
                alias.method = Some(context);
                let table = if kind == crate::classes::MethodKind::PropertySetter {
                    &mut classes.setters[class_id]
                } else {
                    &mut classes.methods[class_id]
                };
                table.insert(method.name.to_string(), alias);
                continue;
            }
            let mut method =
                records::executable_method(method, context, &classes.declarations, graph, &types)?;
            let keyword_copy =
                record_inputs::copies_keywords(&class.methods[index], graph, module_id);
            if keyword_copy {
                method.parameters.kwarg = None;
                // The signature and inference see its proved identity of result layout;
                // final lowering copies the concrete storage and runs its initializer.
                let receiver = method.parameters.iter_non_variadic_params().next().unwrap();
                let range = method.range;
                method.body = vec![ast::Stmt::Return(ast::StmtReturn {
                    range,
                    node_index: ast::AtomicNodeIndex::NONE,
                    value: Some(Box::new(ast::Expr::Name(ast::ExprName {
                        range,
                        node_index: ast::AtomicNodeIndex::NONE,
                        id: receiver.name().as_str().into(),
                        ctx: ast::ExprContext::Load,
                    }))),
                })]
                .into();
            }
            let defaults = keyword_forwarding::prepare(graph, module_id, &mut method);
            let mut signature = signature(
                &method,
                graph,
                defaults,
                &classes.declarations,
                &types,
                Some(context),
                &inference,
            )
            .map_err(|error| format!("{}:{error}", module.path.display()))?;
            if method.name.as_str() == "__init__" {
                if signature.returns != Type::None {
                    return Err(module.error(&method, "__init__ must return None"));
                }
                records::factory_defaults(&mut signature, class, &types, &module.source)?;
                if class.generated_initializer {
                    records::private_initializer_parameters(&mut method, module);
                }
            } else if method.name.as_str() == "__post_init__" && signature.returns != Type::None {
                return Err(module.error(&method, "__post_init__ must return None"));
            } else if matches!(method.name.as_str(), "__str__" | "__repr__")
                && signature.returns != Type::Str
            {
                return Err(module.error(&method, &format!("{} must return str", method.name)));
            }
            if keyword_copy {
                classes.keyword_copies.insert(signature.id);
            }
            if kind == crate::classes::MethodKind::Instance {
                instance_signatures.insert(origin, signature.clone());
            }
            let table = if class.method_kinds[index] == crate::classes::MethodKind::PropertySetter {
                &mut classes.setters[class_id]
            } else {
                &mut classes.methods[class_id]
            };
            table.insert(method.name.to_string(), signature.clone());
            if contexts::decorated(&class.methods[index], graph, module_id) {
                if !contexts::single_yield(&method) {
                    return Err(module.error(&method, "a context manager requires one bare yield on each successful path and an optional finally suite"));
                }
                if signature.returns != Type::None {
                    return Err(
                        module.error(&method, "a bare-yield context manager must return None")
                    );
                }
                classes.contexts.insert(signature.id);
            }
            if records::iterator_result(&method, graph, module_id) {
                classes.generators.insert(signature.id);
            }
            definitions.push((module_id, method, signature));
            callable_definitions.push(class.definition);
        }
    }
    records::record_factories(
        &mut classes,
        &mut definitions,
        &mut callable_definitions,
        &inference,
        graph,
        &types,
    )?;
    if closed_calls::has_keyword_displays(graph) {
        inference.discover_packed_arguments();
    }
    for (module, function, _) in &mut definitions {
        assignments::alias_builtin_parameters(function, &graph.modules[*module].bindings);
        if closed_calls::prepare(function, graph, *module) {
            inference.discover_packed_arguments();
        }
    }
    inference.solve(
        graph,
        &mut definitions,
        &mut signatures,
        &mut classes,
        &types,
    )?;
    for (id, methods) in classes.methods.iter().enumerate() {
        if let Some(method) = methods.get("__bool__")
            && method.returns != Type::Bool
        {
            let class = &classes.declarations.definitions[id];
            return Err(graph.modules[class.definition.module]
                .error(&class.node, "__bool__ must return bool"));
        }
    }
    for (module, function, signature) in &definitions {
        types.bind_function_site(
            *module,
            function.name.range().start().to_u32(),
            signature.id,
        );
        let annotated = function
            .parameters
            .iter_non_variadic_params()
            .zip(&signature.params)
            .filter_map(|(p, inferred)| p.annotation().map(|value| (value, inferred.ty)))
            .chain(
                function
                    .returns
                    .as_deref()
                    .map(|value| (value, signature.returns)),
            );
        for (annotation, ty) in annotated {
            if let Some(definition) = namedtuples::annotation(graph, *module, annotation, &types)
                && types.namedtuple_definition(ty) != Some(definition)
            {
                return Err(graph.modules[*module].error(
                    annotation,
                    "inferred value must be an instance of the annotated namedtuple",
                ));
            }
            if graph.resolve(*module, annotation)
                == Some(Binding::Intrinsic(Intrinsic::ArgparseNamespace))
                && !matches!(ty, Type::Class(id) if classes.parsers.namespace(id))
            {
                return Err(graph.modules[*module].error(
                    annotation,
                    "argparse.Namespace requires an inferred native parser namespace",
                ));
            }
        }
    }
    let initializer_start = definitions.len();
    let class_globals = classes.declarations.has_class_values(graph.entry);
    let helpers = Helpers::new(definitions.len() + graph.initializers(class_globals).count());
    let globals = ModuleValues::default();
    let mut initializations = Vec::new();
    let mut available = HashSet::new();
    for (module_id, range) in graph.initializers(class_globals) {
        let checker = Checker::new(
            graph,
            module_id,
            &signatures,
            &classes,
            &types,
            &helpers,
            &globals,
        );
        initializations.push(
            Initialization::lower(checker, &mut available, range)
                .map_err(|error| format!("{}:{error}", graph.modules[module_id].path.display()))?,
        );
    }
    if let Some(cache) = cache.as_deref_mut() {
        cache.validate_abi(&definitions, &globals);
    }
    let mut functions = Vec::new();
    let mut zip_sums = Vec::new();
    let mut owners = Vec::new();
    let mut call_graph = Vec::new();
    let mut class_reads = Vec::new();
    let mut recursion_errors = Vec::new();
    for (module_id, definition, signature) in &definitions {
        let module = &graph.modules[*module_id];
        let checker = Checker::new(
            graph,
            *module_id,
            &signatures,
            &classes,
            &types,
            &helpers,
            &globals,
        );
        let result = match cache.as_deref_mut() {
            Some(cache) if signature.method.is_none() => cache.body(checker, definition, signature),
            _ => lower_body(checker, definition, signature),
        };
        let (body, sums) = result.map_err(|error| format!("{}:{error}", module.path.display()))?;
        let LoweredBody {
            function,
            calls,
            class_reads: mut reads,
            ..
        } = body;
        graph.ordered_reads(*module_id, &definition.body, true, &mut reads);
        let (function, calls, reads) = if let Some(&policy) = classes.cached.get(&signature.id) {
            let raw = helpers.reserve();
            helpers.finish(
                raw,
                comprehensions::Helper {
                    module: *module_id,
                    function,
                    calls,
                    class_reads: reads,
                    source_frame: false,
                    recursion_error: module.error(
                        definition,
                        "recursive calls are unsupported in this prototype",
                    ),
                },
            );
            let wrapper = function_caches::wrapper(signature, raw, &types, policy);
            (wrapper, HashSet::from([raw]), HashSet::new())
        } else {
            (function, calls, reads)
        };
        if sums.len() >= 2 && !classes.cached.contains_key(&signature.id) {
            zip_sums.push((functions.len(), sums));
        }
        owners.push(*module_id);
        functions.push(function);
        call_graph.push(calls);
        class_reads.push(reads);
        recursion_errors.push(module.error(
            definition,
            "recursive calls are unsupported in this prototype",
        ));
    }
    let mut entry = None;
    for (module_id, module) in graph.modules.iter().enumerate() {
        if module_id != graph.entry {
            continue;
        }
        let mut checker = Checker::new(
            graph,
            module_id,
            &signatures,
            &classes,
            &types,
            &helpers,
            &globals,
        );
        checker.module_scope = true;
        let statements = module
            .executable
            .iter()
            .map(|&index| module.parsed.syntax().body[index].clone())
            .collect::<Vec<_>>();
        let (mut body, _) = checker
            .block(&statements)
            .map_err(|error| format!("{}:{error}", module.path.display()))?;
        if module_id == graph.entry {
            let mut startup = (0..initializations.len())
                .map(|index| {
                    Statement::Expr(Expr {
                        ty: Type::None,
                        kind: ExprKind::Call(
                            initializer_start + index,
                            ir::CallArgs::positional(Vec::new()),
                        ),
                    })
                })
                .collect::<Vec<_>>();
            startup.append(&mut body);
            entry = Some(ir::Function {
                params: Vec::new(),
                returns: Type::None,
                locals: checker.locals,
                body: startup,
            });
        }
    }
    for initialization in &initializations {
        let module = &graph.modules[initialization.module];
        let mut body = initialization.function.body.clone();
        if module.archive.is_some() {
            body.insert(
                0,
                Statement::Expr(Expr {
                    ty: Type::None,
                    kind: ExprKind::Builtin(
                        Builtin::ExtensionImport(
                            module.name.rsplit('.').next().unwrap().to_owned(),
                        ),
                        vec![],
                    ),
                }),
            );
        }
        owners.push(initialization.module);
        functions.push(ir::Function {
            params: Vec::new(),
            returns: Type::None,
            locals: initialization.function.locals.clone(),
            body,
        });
        call_graph.push(initialization.calls.clone());
        class_reads.push(HashSet::new());
        let module = &graph.modules[initialization.module];
        recursion_errors
            .push(module.error(module.parsed.syntax(), "recursive module initialization"));
    }
    // Entry code and inactive module guards can create helpers too. Validate the
    // complete graph only after every source expression has been lowered.
    let list_builds = helpers.list_builds.take();
    let mut python_frames: Vec<_> = owners
        .iter()
        .map(|&module| graph.modules[module].archive.is_none())
        .collect();
    for helper in helpers.into_functions() {
        python_frames.push(helper.source_frame);
        owners.push(helper.module);
        functions.push(helper.function);
        call_graph.push(helper.calls);
        class_reads.push(helper.class_reads);
        recursion_errors.push(helper.recursion_error);
    }
    let mut entry = entry.expect("source graph has an entry module");
    typing::validate_functions(&functions, &entry, &types)?;
    let callable_targets = callables::targets(&functions, &entry, &types)?;
    for (function, calls) in functions.iter().zip(&mut call_graph) {
        ir::walk_expressions(&function.body, &mut |value| {
            if let Some(signature) = callables::invoked_signature(value)
                && let Some(targets) = callable_targets.get(&signature)
            {
                calls.extend(targets);
            }
        });
    }
    for function in functions.iter().chain([&entry]) {
        callables::validate_storage(function, &types)?;
    }
    if let Some(id) = recursive_call_root(&call_graph) {
        return Err(recursion_errors.swap_remove(id));
    }
    for initialization in &initializations {
        initialization.validate(
            graph,
            &functions,
            &class_reads,
            &callable_definitions,
            &callable_targets,
        )?;
    }
    python_frames.push(true); // The program entry runs a Python module body.
    let mut names = false;
    let source_functions = python_frames.iter().filter(|&&frame| frame).count();
    let mut generator_sites = 0;
    let mut count = false;
    for function in functions.iter().chain([&entry]) {
        ir::walk_expressions(&function.body, &mut |value| match value.kind {
            ExprKind::Builtin(Builtin::SysGetFrameModuleName, _) => names = true,
            ExprKind::Builtin(Builtin::SysSetRecursionLimit, _) => count = true,
            ExprKind::Builtin(Builtin::GeneratorFrameEnter(_), _) => generator_sites += 1,
            _ => {}
        });
    }
    // The native call graph is acyclic. Programs that cannot inspect frames,
    // lower their limit, or reach the default depth need no tracking at all.
    let source_frames = if names || count || source_functions + generator_sites > 1000 {
        python_frames
            .into_iter()
            .zip(owners.iter().copied().chain([graph.entry]))
            .map(|(frame, module)| {
                frame.then(|| {
                    if names {
                        ir::SourceFrame::Module(if module == graph.entry {
                            "__main__".to_owned()
                        } else {
                            graph.modules[module].name.clone()
                        })
                    } else {
                        ir::SourceFrame::Depth
                    }
                })
            })
            .collect()
    } else {
        Vec::new()
    };
    if generator_sites != 0 && source_frames.is_empty() {
        for function in functions.iter_mut().chain([&mut entry]) {
            ir::walk_expressions_mut(&mut function.body, &mut |value| {
                if matches!(
                    value.kind,
                    ExprKind::Builtin(
                        Builtin::GeneratorFrameEnter(_) | Builtin::GeneratorFrameLeave,
                        _
                    )
                ) {
                    value.kind = ExprKind::None;
                }
            });
        }
    }
    let json_records = json::schemas(graph, &classes, &types, &functions, &entry)?;
    // Fusing adjacent sums also inlines their generator bodies and scalar calls.
    if source_frames.is_empty() {
        for (id, sums) in zip_sums {
            zip_sums::fuse(&mut functions, id, &sums);
        }
    }
    globals
        .class_data
        .borrow_mut()
        .extend_layouts(&mut classes.fields);
    let mut globals = globals.into_types();
    types.reserve_constructor_globals(&mut globals, &functions, &entry);
    if let Some(cache) = cache {
        cache.finish(&types);
    }
    Ok((
        ir::Module {
            ignore_python_environment: graph
                .command_startup
                .as_ref()
                .is_some_and(|(flags, _)| flags.isolated),
            command_utf8_mode: graph
                .command_startup
                .as_ref()
                .map(|(flags, _)| flags.utf8_mode),
            types,
            class_metadata: dataclass_values::metadata(&classes, graph),
            list_builds,
            class_fields: classes.fields,
            json_records,
            globals,
            source_frames,
            functions,
            entry,
        },
        owners,
    ))
}

#[derive(Clone)]
struct LoweredBody {
    function: ir::Function,
    calls: HashSet<usize>,
    class_reads: HashSet<DefinitionId>,
    default_reads: Option<Vec<caller_cache::DefaultId>>,
}

type BodyResult = Result<(LoweredBody, HashMap<usize, zip_sums::Sum>), String>;

fn lower_body(
    mut checker: Checker<'_>,
    definition: &ast::StmtFunctionDef,
    signature: &Signature,
) -> BodyResult {
    #[cfg(test)]
    caller_cache::record_lowering(definition.name.as_str());
    let module = &checker.graph.modules[checker.module];
    let classes = checker.classes;
    let types = checker.types;
    checker.positional_pack = definition
        .parameters
        .vararg
        .as_deref()
        .map(|p| p.name.to_string());
    checker.consuming_lists = consuming_lists::find(&definition.body);
    checker.joined_lines = joined_lines::find(&definition.body);
    checker.returns = signature.returns;
    checker.interactive_names = module.is_doctest_case() && definition.name.as_str() == "_run_case";
    checker.asynchronous = signature.asynchronous;
    checker.method_context(signature.method);
    checker.generator_item = classes.producer_items.get(&signature.id).copied();
    let mut params = Vec::new();
    for (parameter, signature) in
        closed_calls::parameters(&definition.parameters).zip(&signature.params)
    {
        let slot = checker.declare(parameter.name().as_str(), signature.ty, parameter)?;
        checker.assigned.insert(slot);
        params.push(slot);
    }
    if let Some(keyword) = &definition.parameters.kwarg {
        let parameter = signature.params.last().expect("keyword-rest signature");
        let slot = checker.declare(keyword.name.as_str(), parameter.ty, &**keyword)?;
        checker.assigned.insert(slot);
        params.push(slot);
    }
    let (mut body, falls_through) = if module.archive.is_some() {
        let args = params.iter().map(|&slot| checker.local(slot)).collect();
        let kind = |ty| match ty {
            Type::None => Ok(0),
            Type::Int => Ok(1),
            Type::Bytes => Ok(2),
            Type::ListInt => Ok(3),
            Type::Callable(_) if types.signature(ty) == Some((vec![Type::Int], Type::Int)) => Ok(4),
            _ => Err(error(
                &module.source,
                definition,
                "native declarations currently support None, int, bytes, list[int], and Callable[[int], int]",
            )),
        };
        let call = Builtin::ExtensionCall {
            module: module.name.rsplit('.').next().unwrap().to_owned(),
            method: definition.name.to_string(),
            kinds: signature
                .params
                .iter()
                .map(|p| kind(p.ty))
                .collect::<Result<_, _>>()?,
            result_kind: kind(signature.returns)?,
        };
        (
            vec![Statement::Return(Expr {
                ty: signature.returns,
                kind: ExprKind::Builtin(call, args),
            })],
            false,
        )
    } else if classes.keyword_copies.contains(&signature.id) {
        let class = signature.method.unwrap().class;
        let value = checker.dispatch_record(
            class,
            checker.local(params[0]),
            definition,
            |checker, class, receiver| checker.copy_with_keywords(class, receiver, &[], definition),
        )?;
        (vec![Statement::Return(value)], false)
    } else if signature.method.is_some_and(|m| {
        definition.name.as_str() == "__post_init__"
            && checker.normalized_record_input(m.class).is_some()
    }) {
        // The generated initializer stores the normalized field on all input paths.
        (vec![], true)
    } else {
        checker.function_body(&definition.body)?
    };
    if let Some(context) = signature.method
        && let class = &classes.declarations.definitions[context.class]
        && class.generated_initializer
        && class.initializer == Some(context.index)
        && let Some(post) = classes.methods[context.class].get("__post_init__")
    {
        checker.calls.insert(post.id);
        body.push(Statement::Expr(Expr {
            ty: Type::None,
            kind: ExprKind::Call(
                post.id,
                ir::CallArgs::positional(vec![checker.local(params[0])]),
            ),
        }));
    }
    if falls_through && types.payload(signature.returns).is_some() {
        body.push(Statement::Return(Expr {
            ty: signature.returns,
            kind: ExprKind::Absent(Box::new(Expr {
                ty: Type::None,
                kind: ExprKind::None,
            })),
        }));
    } else if falls_through && signature.returns != Type::None {
        return Err(error(
            &module.source,
            definition,
            "not every path returns the declared type",
        ));
    }
    if classes.generators.contains(&signature.id) {
        body = checker.generator_body(body);
    }
    Ok((
        LoweredBody {
            function: ir::Function {
                params,
                returns: signature.returns,
                locals: checker.locals,
                body,
            },
            calls: checker.calls,
            class_reads: checker.class_reads.into_inner(),
            default_reads: checker.default_reads,
        },
        checker.zip_sums,
    ))
}

fn signature(
    function: &ast::StmtFunctionDef,
    graph: &SourceGraph,
    defaults: keyword_forwarding::Defaults,
    classes: &Classes,
    types: &Types,
    method: Option<records::MethodContext>,
    inference: &inference::Inference,
) -> Result<Signature, String> {
    let module = defaults.module;
    let source = &graph.modules[module].source;
    let owner = method.map(|context| context.class);
    let receiver = method
        .filter(|context| {
            matches!(
                context.kind(classes),
                crate::classes::MethodKind::Instance
                    | crate::classes::MethodKind::Property
                    | crate::classes::MethodKind::PropertySetter
            )
        })
        .map(|context| classes.instance(context.class));
    if !function.decorator_list.is_empty() || function.type_params.is_some() {
        return Err(error(
            source,
            function,
            "decorators and type parameters are unsupported",
        ));
    }
    let mut params = Vec::new();
    let mut parameter_types = Vec::new();
    let mut none_defaults = Vec::new();
    for (index, parameter) in function.parameters.iter_non_variadic_params().enumerate() {
        let origin = defaults
            .forwarded
            .filter(|(first, _)| index >= *first)
            .map(|(_, id)| id);
        let module = origin
            .or_else(|| {
                owner.and_then(|id| {
                    let record = &classes.definitions[id];
                    (record.generated_initializer && function.name.as_str() == "__init__")
                        .then(|| {
                            record
                                .fields
                                .iter()
                                .find(|field| field.name == parameter.name().as_str())
                        })
                        .flatten()
                        .map(|field| field.module)
                })
            })
            .unwrap_or(module);
        let source = origin.map_or(source, |id| &graph.modules[id].source);
        let sequence = parameter.annotation().is_some_and(|value|
            matches!(value, ast::Expr::Subscript(subscript) if annotations::sequence_annotation(graph.resolve(module, &subscript.value))));
        let empty_sequence = sequence
            && matches!(parameter.default.as_deref(), Some(ast::Expr::Tuple(tuple)) if tuple.elts.is_empty());
        let wrapper_default =
            parameter
                .default
                .as_deref()
                .and_then(|value| match graph.resolve(module, value) {
                    Some(Binding::Intrinsic(Intrinsic::WrapperAttributes(fields))) => {
                        Some(functools::wrapper_attributes(fields, types))
                    }
                    _ => None,
                });
        let ty = if let Some(receiver) = receiver
            && index == 0
        {
            if let Some(declared) = parameter.annotation()
                && annotation_in_class(declared, graph, module, classes, types, owner)? != receiver
            {
                return Err(error(
                    source,
                    parameter,
                    "self annotation must name its exact class",
                ));
            }
            Some(receiver)
        } else {
            parameter
                .annotation()
                .filter(|value| {
                    graph.resolve(module, value)
                        != Some(Binding::Intrinsic(Intrinsic::ArgparseNamespace))
                        && namedtuples::annotation(graph, module, value, types).is_none()
                })
                .map(|value| {
                    if let ast::Expr::Subscript(sequence) = value
                        && annotations::sequence_annotation(graph.resolve(module, &sequence.value))
                    {
                        sequences::check_reads(
                            function,
                            parameter.name().as_str(),
                            empty_sequence,
                            graph,
                            module,
                            source,
                        )?;
                        let element = annotation_in_class(
                            &sequence.slice,
                            graph,
                            module,
                            classes,
                            types,
                            owner,
                        )?;
                        Ok(types.list(element))
                    } else {
                        annotation_in_class(value, graph, module, classes, types, owner)
                    }
                })
                .transpose()?
                .or(parameter
                    .default
                    .as_deref()
                    .filter(|value| !empty_sequence && !matches!(value, ast::Expr::Dict(dict) if dict.items.is_empty() && parameter.annotation().is_some()))
                    .map(|value| {
                        if inspect::is_sentinel(graph, module, value) {
                            Ok(Type::PlainObject)
                        } else if let Some(default) = &wrapper_default {
                            Ok(default.ty)
                        } else {
                            calls::scalar_literal(value, source).map(|value| value.ty)
                        }
                    })
                    .transpose()?)
        };
        if parameter.annotation().is_none()
            && matches!(
                parameter.default.as_deref(),
                Some(ast::Expr::NoneLiteral(_))
            )
        {
            none_defaults.push(index);
        }
        parameter_types.push(ty);
        // Unresolved types exist only in the discarded discovery pass. solve()
        // fills every slot before ordinary lowering can construct executable IR.
        let ty = ty.unwrap_or(Type::None);
        let kind = if index < function.parameters.posonlyargs.len() {
            ParameterKind::PositionalOnly
        } else if index < function.parameters.posonlyargs.len() + function.parameters.args.len() {
            ParameterKind::PositionalOrKeyword
        } else {
            ParameterKind::KeywordOnly
        };
        let default = parameter
            .default
            .as_deref()
            .map(|value| {
                if types.payload(ty).unwrap_or(ty) == Type::ReadOnlyObjectMapping
                    && matches!(value, ast::Expr::Dict(d) if d.items.is_empty())
                {
                    // The read-only surface exposes neither identity nor mutation.
                    let empty = Expr {
                        ty: types.dict(Type::Str, Type::ObjectValue),
                        kind: ExprKind::Dict {
                            key: Type::Str,
                            value: Type::ObjectValue,
                            entries: Vec::new(),
                        },
                    };
                    let view = Expr {
                        ty: Type::ReadOnlyObjectMapping,
                        kind: ExprKind::Builtin(Builtin::DictAsBase, vec![empty]),
                    };
                    Ok(if ty == view.ty {
                        view
                    } else {
                        Expr {
                            ty,
                            kind: ExprKind::Present(Box::new(view)),
                        }
                    })
                } else if empty_sequence {
                    // The read-only Sequence contract makes its empty backing list unobservable.
                    Ok(Expr {
                        ty,
                        kind: ExprKind::List(Vec::new()),
                    })
                } else if ty == Type::PlainObject && inspect::is_sentinel(graph, module, value) {
                    Ok(inspect::sentinel())
                } else if let Some(default) = wrapper_default {
                    if default.ty != ty {
                        return Err(error(
                            source,
                            value,
                            "wrapper attribute default requires its exact tuple type",
                        ));
                    }
                    Ok(default)
                } else {
                    calls::scalar_default(value, ty, types, source)
                }
            })
            .transpose()?;
        params.push(Parameter {
            name: parameter.name().to_string(),
            ty,
            kind,
            default,
            sign_only: ty == Type::Float
                && parameter.annotation().is_some()
                && sign_parameters::only_compares_zero(function, parameter.name().as_str()),
        });
    }
    if let Some(pack) = function.parameters.vararg.as_deref() {
        let annotation = pack.annotation().ok_or_else(|| {
            error(
                source,
                pack,
                "a variadic positional parameter requires an exact element annotation",
            )
        })?;
        let element = annotation_in_class(annotation, graph, module, classes, types, owner)?;
        sequences::check_pack_reads(function, pack.name.as_str(), graph, module, source)?;
        let ty = types.list(element);
        parameter_types.push(Some(ty));
        params.push(Parameter {
            name: pack.name.to_string(),
            ty,
            kind: ParameterKind::Variadic,
            default: None,
            sign_only: false,
        });
    }
    if let Some(keyword) = &function.parameters.kwarg {
        let element = keyword.annotation.as_deref().ok_or_else(|| {
            error(
                source,
                &**keyword,
                "keyword rest parameters need an element annotation",
            )
        })?;
        let element = annotation_in_class(element, graph, module, classes, types, owner)?;
        let ty = types.dict(Type::Str, element);
        params.push(Parameter {
            name: keyword.name.to_string(),
            ty,
            kind: ParameterKind::KeywordRest,
            default: None,
            sign_only: false,
        });
        parameter_types.push(Some(ty));
    }
    // A .pyi describes the linked C function. Its ellipsis is not an executable body.
    let inert = graph.modules[module].archive.is_none() && annotations::inert_body(function);
    if inert && let Some(annotation) = function.returns.as_deref() {
        graph.checked_inert_return(module, annotation.range());
        if let ast::Expr::Name(name) = annotation {
            graph.deferred_name(module, name.range());
        }
    }
    let returns = function
        .returns
        .as_deref()
        .filter(|_| !inert)
        .filter(|value| {
            graph.resolve(module, value) != Some(Binding::Intrinsic(Intrinsic::ArgparseNamespace))
                && namedtuples::annotation(graph, module, value, types).is_none()
        })
        .map(|value| {
            if let ast::Expr::Subscript(subscript) = value
                && annotations::sequence_annotation(graph.resolve(module, &subscript.value))
            {
                annotation_in_class(&subscript.slice, graph, module, classes, types, owner)
                    .map(|element| types.list(element))
            } else {
                annotation_in_class(value, graph, module, classes, types, owner)
            }
        })
        .transpose()?
        .or(inert.then_some(Type::None))
        .or_else(|| match (receiver, function.name.as_str()) {
            (Some(_), "__init__" | "__post_init__") => Some(Type::None),
            (Some(_), "__str__" | "__repr__") => Some(Type::Str),
            (Some(_), "__bool__") => Some(Type::Bool),
            _ => None,
        });
    let id = inference.register(parameter_types, returns, none_defaults);
    Ok(Signature {
        id,
        asynchronous: function.is_async,
        params,
        returns: returns.unwrap_or(Type::None),
        method,
    })
}

fn recursive_call_root(graph: &[HashSet<usize>]) -> Option<usize> {
    let mut states = vec![0; graph.len()];
    let mut path = Vec::new();
    for root in 0..graph.len() {
        if states[root] != 0 {
            continue;
        }
        states[root] = 1;
        path.push((root, graph[root].iter()));
        while let Some((id, callees)) = path.last_mut() {
            let Some(&callee) = callees.next() else {
                states[*id] = 2;
                path.pop();
                continue;
            };
            match states[callee] {
                1 => return Some(root),
                2 => {}
                _ => {
                    states[callee] = 1;
                    path.push((callee, graph[callee].iter()));
                }
            }
        }
    }
    None
}

pub(crate) fn error(source: &str, node: &impl Ranged, message: &str) -> String {
    let offset = usize::from(node.start());
    let prefix = &source[..offset];
    let line = prefix.bytes().filter(|&byte| byte == b'\n').count() + 1;
    let column = prefix.rsplit('\n').next().unwrap_or("").chars().count() + 1;
    format!("{line}:{column}: Imre: {message}")
}

fn annotation(
    expression: &ast::Expr,
    graph: &SourceGraph,
    module: ModuleId,
    classes: &Classes,
    types: &Types,
) -> Result<Type, String> {
    annotation_in_class(expression, graph, module, classes, types, None)
}

fn annotation_in_class(
    expression: &ast::Expr,
    graph: &SourceGraph,
    module: ModuleId,
    classes: &Classes,
    types: &Types,
    owner: Option<usize>,
) -> Result<Type, String> {
    annotation_at(
        expression,
        graph,
        module,
        classes,
        types,
        &graph.modules[module].source,
        owner,
    )
}

/// Quoted annotations are parsed as types, never executed. Keeping their source
/// separate preserves valid ranges even when the outer literal contains escapes.
fn annotation_at(
    expression: &ast::Expr,
    graph: &SourceGraph,
    module: ModuleId,
    classes: &Classes,
    types: &Types,
    source: &str,
    owner: Option<usize>,
) -> Result<Type, String> {
    annotation_with_aliases(
        expression,
        graph,
        (module, source),
        classes,
        types,
        owner,
        0,
    )
}

fn annotation_with_aliases(
    expression: &ast::Expr,
    graph: &SourceGraph,
    (module, source): (ModuleId, &str),
    classes: &Classes,
    types: &Types,
    owner: Option<usize>,
    alias_depth: usize,
) -> Result<Type, String> {
    if alias_depth >= 32 {
        return Err(error(
            source,
            expression,
            "type aliases nest more than 32 levels or form a cycle",
        ));
    }
    if let Some(Binding::TypeAlias(origin, index)) = graph.resolve_type(module, expression) {
        let module = &graph.modules[origin];
        return annotation_with_aliases(
            &module.aliases[index].value,
            graph,
            (origin, &module.source),
            classes,
            types,
            owner,
            alias_depth + 1,
        );
    }
    if let ast::Expr::StringLiteral(value) = expression {
        check_string(value, source)?;
        let contents = value.value.to_str();
        let parsed = ruff_python_parser::parse_expression(contents)
            .map_err(|_| error(source, expression, "invalid quoted type annotation"))?;
        return annotation_with_aliases(
            parsed.expr(),
            graph,
            (module, contents),
            classes,
            types,
            owner,
            alias_depth,
        )
        .map_err(|message| {
            error(
                source,
                expression,
                &format!("invalid quoted annotation: {message}"),
            )
        });
    }
    match graph.resolve_type(module, expression) {
        Some(Binding::Definition(definition)) => {
            // The integer-fd selector keeps its immutable key in both the mapping
            // and the ready list. The data field contains a boxed Python value.
            let owner = &graph.modules[definition.module];
            if owner.is_bundled("selectors")
                && types
                    .namedtuple(definition)
                    .is_some_and(|ty| ty.name == "SelectorKey")
            {
                return Ok(types.namedtuple_layout(
                    crate::types::TupleOrigin::Declaration(definition),
                    vec![Type::Int, Type::Int, Type::Int, Type::ObjectValue],
                ));
            }
            if owner.is_bundled("pkgutil")
                && types
                    .namedtuple(definition)
                    .is_some_and(|ty| ty.name == "ModuleInfo")
                && let Some(Binding::Definition(finder)) = owner.bindings.get("FileFinder")
                && let Some(&id) = classes.ids.get(finder)
            {
                return Ok(types.namedtuple_layout(
                    crate::types::TupleOrigin::Declaration(definition),
                    vec![classes.instance(id), Type::Str, Type::Bool],
                ));
            }
            if classes.exceptions.contains_key(&definition) {
                return Ok(classes
                    .ids
                    .get(&definition)
                    .copied()
                    .map_or(Type::Exception, Type::NamedException));
            }
            if let Some(&id) = classes.ids.get(&definition) {
                return Ok(classes.instance(id));
            }
        }
        Some(Binding::Intrinsic(Intrinsic::SelfType)) => {
            return owner.map(|id| classes.instance(id)).ok_or_else(|| {
                error(
                    source,
                    expression,
                    "Self is supported only inside a fixed class",
                )
            });
        }
        Some(Binding::Intrinsic(Intrinsic::Any)) if graph.mutable_documents => {
            return Ok(documents::ANY);
        }
        Some(Binding::Intrinsic(Intrinsic::Readiness(ir::Readiness::New))) => {
            return Ok(Type::Poll);
        }
        Some(Binding::Intrinsic(Intrinsic::Deque)) => return Ok(types.deque(Type::Int)),
        Some(Binding::Intrinsic(Intrinsic::Counter)) => return Ok(types.counter(Type::Str)),
        Some(Binding::Intrinsic(Intrinsic::ChainMap)) => {
            return Ok(types.chainmap(Type::Str, Type::Int));
        }
        Some(Binding::Intrinsic(Intrinsic::TextIO)) => return Ok(Type::TextIO),
        Some(Binding::Intrinsic(Intrinsic::BinaryIO)) => return Ok(Type::BinaryIO),
        Some(Binding::Intrinsic(Intrinsic::JsonDecoderClass)) => return Ok(Type::JsonDecoder),
        Some(Binding::Intrinsic(Intrinsic::Socket)) => return Ok(Type::Socket),
        Some(Binding::Intrinsic(Intrinsic::ZipFile)) => return Ok(Type::ZipFile),
        Some(Binding::Intrinsic(Intrinsic::ZipInfo)) => return Ok(Type::ZipInfo),
        Some(Binding::Intrinsic(Intrinsic::BytesIo)) => return Ok(Type::BytesIO),
        Some(Binding::Intrinsic(Intrinsic::StringIo)) => return Ok(Type::StringIO),
        Some(Binding::Intrinsic(Intrinsic::Context)) => return Ok(Type::Context),
        Some(Binding::Intrinsic(Intrinsic::ContextVar)) => {
            return Ok(types.context_var(Type::ObjectValue));
        }
        Some(Binding::Intrinsic(Intrinsic::LogFormatter)) => return Ok(Type::LogFormatter),
        Some(Binding::Intrinsic(Intrinsic::HandlerLevelType)) => return Ok(types.string_or_int()),
        Some(Binding::Intrinsic(Intrinsic::TextOutputType)) => return Ok(Type::TextOutput),
        Some(Binding::Intrinsic(Intrinsic::HandlerLock)) => return Ok(Type::HandlerLock),
        Some(Binding::Intrinsic(Intrinsic::LogRecord)) => return Ok(Type::LogRecord),
        Some(Binding::Intrinsic(Intrinsic::Logger)) => return Ok(Type::Logger),
        Some(Binding::Intrinsic(Intrinsic::LogManager)) => return Ok(Type::LogManager),
        Some(Binding::Intrinsic(Intrinsic::TimedFileHandler)) => return Ok(Type::TimedFileHandler),
        Some(Binding::Intrinsic(Intrinsic::ClockTime)) => return Ok(Type::ClockTime),
        Some(Binding::Intrinsic(Intrinsic::Calendar(kind))) => return Ok(Type::Calendar(kind)),
        Some(Binding::Intrinsic(Intrinsic::ModuleType)) => return Ok(Type::NativeModule),
        Some(Binding::Intrinsic(Intrinsic::PatchSet)) => {
            return Ok(Type::Patch(ir::PatchKind::Set));
        }
        Some(Binding::Intrinsic(Intrinsic::PatchedFile)) => {
            return Ok(Type::Patch(ir::PatchKind::File));
        }
        Some(Binding::Intrinsic(Intrinsic::Hunk)) => return Ok(Type::Patch(ir::PatchKind::Hunk)),
        Some(Binding::Intrinsic(Intrinsic::Path)) => return Ok(Type::Path),
        Some(Binding::Intrinsic(Intrinsic::StructTime)) => return Ok(types.struct_time()),
        Some(Binding::Intrinsic(Intrinsic::StatResult)) => return Ok(types.stat_result()),
        Some(Binding::Intrinsic(Intrinsic::SplitResult { bytes })) => {
            return Ok(types.url_split_result(bytes));
        }
        Some(Binding::Intrinsic(Intrinsic::PurePath(flavor))) => return Ok(Type::PurePath(flavor)),
        Some(Binding::Intrinsic(Intrinsic::CompletedProcess)) => {
            return Ok(Type::CompletedProcess(
                crate::ir::ProcessCapture::TEXT,
                None,
            ));
        }
        _ => {}
    }
    let nested = |expression: &ast::Expr| {
        annotation_with_aliases(
            expression,
            graph,
            (module, source),
            classes,
            types,
            owner,
            alias_depth,
        )
    };
    match expression {
        ast::Expr::Name(name) => match name.id.as_str() {
            "int" => Ok(Type::Int),
            "type" => Ok(Type::BuiltinClass),
            "float" => Ok(Type::Float),
            "bool" => Ok(Type::Bool),
            "str" => Ok(Type::Str),
            "bytes" => Ok(Type::Bytes),
            "BaseException" | "Exception" => Ok(Type::Exception),
            "ImportError" => Ok(Type::ImportException),
            "object" => Ok(Type::ObjectValue),
            _ => Err(error(
                source,
                expression,
                "unsupported exact type annotation",
            )),
        },
        ast::Expr::NoneLiteral(_) => Ok(Type::None),
        ast::Expr::Subscript(value)
            if matches!(&*value.value, ast::Expr::Name(name) if name.id.as_str() == "type")
                && graph.resolve_type(module, &value.value).is_none() =>
        {
            let Type::Class(base) = nested(&value.slice)? else {
                return Err(error(
                    source,
                    expression,
                    "type[T] requires a fixed record class",
                ));
            };
            let class = &classes.definitions[base];
            let index = class.initializer.ok_or_else(|| {
                error(
                    source,
                    expression,
                    "type[T] requires a declared initializer",
                )
            })?;
            let params = &class.methods[index].parameters;
            if params.vararg.is_some() || params.kwarg.is_some() || !params.kwonlyargs.is_empty() {
                return Err(error(
                    source,
                    expression,
                    "class factories require positional parameters",
                ));
            }
            let origin = class.method_origins[index].module;
            let args = params
                .iter_non_variadic_params()
                .skip(1)
                .map(|p| {
                    let annotation = p.annotation().ok_or_else(|| {
                        error(
                            source,
                            expression,
                            "class factory parameters require exact annotations",
                        )
                    })?;
                    annotation_with_aliases(
                        annotation,
                        graph,
                        (origin, &graph.modules[origin].source),
                        classes,
                        types,
                        Some(base),
                        alias_depth + 1,
                    )
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(types.record_constructor(base, args))
        }
        ast::Expr::Subscript(subscript)
            if matches!(
                graph.resolve_type(module, &subscript.value),
                Some(Binding::Intrinsic(Intrinsic::Mapping { .. }))
            ) =>
        {
            if let ast::Expr::Tuple(types) = &*subscript.slice
                && let [key, value] = types.elts.as_slice()
                && nested(key)? == Type::Str
                && nested(value)? == Type::ObjectValue
            {
                return Ok(Type::ReadOnlyObjectMapping);
            }
            Err(error(
                source,
                expression,
                "read-only mapping annotations currently require Mapping[str, object]",
            ))
        }
        ast::Expr::Subscript(subscript)
            if let Some(Binding::Intrinsic(
                kind @ (Intrinsic::ContextVar | Intrinsic::ContextToken),
            )) = graph.resolve_type(module, &subscript.value) =>
        {
            let Type::ContextVar(id) = types.context_var(nested(&subscript.slice)?) else {
                unreachable!()
            };
            Ok(if kind == Intrinsic::ContextVar {
                Type::ContextVar(id)
            } else {
                Type::ContextToken(id)
            })
        }
        ast::Expr::Subscript(subscript)
            if graph.resolve_type(module, &subscript.value)
                == Some(Binding::Intrinsic(Intrinsic::Literal)) =>
        {
            literals::annotation(&subscript.slice, types, source)
        }
        ast::Expr::Subscript(value)
            if graph.resolve_type(module, &value.value)
                == Some(Binding::Intrinsic(Intrinsic::AbstractSet))
                && nested(&value.slice)? == Type::Str =>
        {
            Ok(Type::StringKeys)
        }
        ast::Expr::Subscript(value)
            if let Some(Binding::Intrinsic(
                kind @ (Intrinsic::RegexPattern | Intrinsic::RegexMatch),
            )) = graph.resolve_type(module, &value.value) =>
        {
            if nested(&value.slice)? != Type::Str {
                return Err(error(source, expression, "regex annotations require str"));
            }
            Ok(if kind == Intrinsic::RegexPattern {
                Type::RegexPattern
            } else {
                Type::RegexMatch
            })
        }
        ast::Expr::Subscript(value)
            if graph.resolve_type(module, &value.value)
                == Some(Binding::Intrinsic(Intrinsic::Awaitable)) =>
        {
            Ok(types.coroutine(nested(&value.slice)?))
        }
        ast::Expr::Subscript(value)
            if matches!(
                graph.resolve_type(module, &value.value),
                Some(Binding::Intrinsic(Intrinsic::CompletedProcess))
            ) =>
        {
            let capture = match nested(&value.slice)? {
                Type::Str => crate::ir::ProcessCapture::TEXT,
                Type::Bytes => crate::ir::ProcessCapture::BYTES,
                _ => {
                    return Err(error(
                        source,
                        expression,
                        "CompletedProcess requires str or bytes output",
                    ));
                }
            };
            Ok(Type::CompletedProcess(capture, None))
        }
        ast::Expr::Subscript(value)
            if matches!(
                graph.resolve_type(module, &value.value),
                Some(Binding::Intrinsic(Intrinsic::Callable))
            ) =>
        {
            let ast::Expr::Tuple(arguments) = &*value.slice else {
                return Err(error(
                    source,
                    expression,
                    "Callable requires an exact parameter list and return type",
                ));
            };
            let [ast::Expr::List(parameters), returns] = arguments.elts.as_slice() else {
                return Err(error(
                    source,
                    expression,
                    "Callable requires Callable[[T, ...], R]; ellipsis is unsupported",
                ));
            };
            let parameters = parameters
                .elts
                .iter()
                .map(nested)
                .collect::<Result<Vec<_>, _>>()?;
            Ok(types.callable(parameters, nested(returns)?))
        }
        ast::Expr::Subscript(value)
            if matches!(
                graph.resolve_type(module, &value.value),
                Some(Binding::Intrinsic(Intrinsic::Union))
            ) =>
        {
            let members = if let ast::Expr::Tuple(arguments) = &*value.slice {
                arguments
                    .elts
                    .iter()
                    .map(nested)
                    .collect::<Result<Vec<_>, _>>()?
            } else {
                vec![nested(&value.slice)?]
            };
            types.union(&members).ok_or_else(|| {
                error(
                    source,
                    expression,
                    "finite unions support str | bytes and nullable exact types",
                )
            })
        }
        ast::Expr::Subscript(value) => match &*value.value {
            item if matches!(
                graph.resolve(module, item),
                Some(Binding::Intrinsic(
                    Intrinsic::Iterable | Intrinsic::Iterator
                ))
            ) =>
            {
                let element = nested(&value.slice)?;
                if owner.is_some_and(|id| classes.definitions[id].dataclass.is_some())
                    && element == Type::Str
                    && graph.resolve(module, item) == Some(Binding::Intrinsic(Intrinsic::Iterable))
                {
                    Ok(Type::StringIterable)
                } else {
                    Ok(types.list(element))
                }
            }
            item if graph.resolve_type(module, item)
                == Some(Binding::Intrinsic(Intrinsic::Deque)) =>
            {
                Ok(types.deque(nested(&value.slice)?))
            }
            item if graph.resolve_type(module, item)
                == Some(Binding::Intrinsic(Intrinsic::Counter)) =>
            {
                let key = nested(&value.slice)?;
                if !types.hashable_key(key) {
                    return Err(error(
                        source,
                        &*value.slice,
                        "Counter requires an immutable key type",
                    ));
                }
                Ok(types.counter(key))
            }
            item if graph.resolve_type(module, item)
                == Some(Binding::Intrinsic(Intrinsic::Final)) =>
            {
                nested(&value.slice)
            }
            ast::Expr::Name(name) if name.id == "list" => {
                let element = nested(&value.slice)?;
                Ok(if element == documents::ANY {
                    documents::LIST
                } else {
                    types.list(element)
                })
            }
            ast::Expr::Name(name)
                if name.id == "frozenset" && nested(&value.slice)? == Type::Str =>
            {
                Ok(Type::FrozenStrings)
            }
            ast::Expr::Name(name) if name.id == "set" => {
                let element = nested(&value.slice)?;
                if !types.hashable_key(element) {
                    return Err(error(
                        source,
                        &*value.slice,
                        "set elements must be int, str, bytes, Path, or immutable scalar tuples",
                    ));
                }
                Ok(types.set(element))
            }
            receiver
                if matches!(receiver, ast::Expr::Name(name) if name.id == "dict")
                    || matches!(
                        graph.resolve_type(module, receiver),
                        Some(Binding::Intrinsic(
                            Intrinsic::DefaultDict | Intrinsic::ChainMap | Intrinsic::OrderedDict
                        ))
                    ) =>
            {
                let ast::Expr::Tuple(arguments) = &*value.slice else {
                    return Err(error(
                        source,
                        expression,
                        "dict requires key and value types",
                    ));
                };
                let [key, value] = arguments.elts.as_slice() else {
                    return Err(error(
                        source,
                        expression,
                        "dict requires key and value types",
                    ));
                };
                let key_type = nested(key)?;
                if matches!(receiver, ast::Expr::Name(name) if name.id == "dict")
                    && key_type == Type::Str
                    && graph.resolve_type(module, value) == Some(Binding::Intrinsic(Intrinsic::Any))
                {
                    return Ok(if graph.mutable_documents {
                        documents::MAP
                    } else {
                        Type::JsonObject
                    });
                }
                if !types.hashable_key(key_type) {
                    return Err(error(
                        source,
                        key,
                        "dictionary keys must be int, str, bytes, Path, or immutable scalar tuples",
                    ));
                }
                let value_type = nested(value)?;
                if graph.resolve_type(module, receiver)
                    == Some(Binding::Intrinsic(Intrinsic::ChainMap))
                {
                    return Ok(types.chainmap(key_type, value_type));
                }
                if graph.resolve_type(module, receiver)
                    == Some(Binding::Intrinsic(Intrinsic::DefaultDict))
                {
                    if types.element(value_type).is_none() {
                        return Err(error(
                            source,
                            value,
                            "defaultdict requires a typed list value",
                        ));
                    }
                    Ok(types.default_list_dict(key_type, value_type))
                } else if graph.resolve_type(module, receiver)
                    == Some(Binding::Intrinsic(Intrinsic::OrderedDict))
                {
                    Ok(types.ordered_dict(key_type, value_type))
                } else {
                    Ok(types.dict(key_type, value_type))
                }
            }
            ast::Expr::Name(name) if name.id == "tuple" => {
                if let ast::Expr::Tuple(fields) = &*value.slice
                    && let [element, ast::Expr::EllipsisLiteral(_)] = fields.elts.as_slice()
                {
                    return Ok(types.var_tuple(nested(element)?));
                }
                let fields = if let ast::Expr::Tuple(fields) = &*value.slice {
                    fields
                        .elts
                        .iter()
                        .map(nested)
                        .collect::<Result<Vec<_>, _>>()?
                } else {
                    vec![nested(&value.slice)?]
                };
                Ok(types.tuple(fields))
            }
            _ => Err(error(
                source,
                expression,
                "only list[T], dict[K, V], set[T], and fixed tuple[T, ...] are supported",
            )),
        },
        ast::Expr::BinOp(union) if union.op == ast::Operator::BitOr => {
            // Check the complete annotation before deciding whether its arms have a layout.
            // An intermediate str | int alone has no general representation.
            let mut pending = vec![union.right.as_ref(), union.left.as_ref()];
            let mut members = Vec::new();
            while let Some(arm) = pending.pop() {
                if let ast::Expr::BinOp(part) = arm
                    && part.op == ast::Operator::BitOr
                {
                    pending.extend([part.right.as_ref(), part.left.as_ref()]);
                } else {
                    members.push(nested(arm)?);
                }
            }
            types.union(&members).ok_or_else(|| {
                error(
                    source,
                    expression,
                    "finite unions support str | bytes and nullable exact types",
                )
            })
        }
        _ => Err(error(source, expression, "unsupported type annotation")),
    }
}

/// Ruff replaces surrogate escapes with U+FFFD in its analysis AST. Reject those
/// source literals rather than silently compiling a different Python value.
pub(crate) fn check_string(value: &ast::ExprStringLiteral, source: &str) -> Result<(), String> {
    for part in value.value.iter() {
        check_string_literal(part, source)?;
    }
    Ok(())
}

/// Checks literal source ranges before boxed lowering consumes Ruff's decoded strings.
/// A field expression and each adjacent string have their own tokens and prefixes.
pub(crate) fn check_string_tokens(source: &str, tokens: &ast::token::Tokens) -> Result<(), String> {
    use ast::StringFlags;
    use ast::token::TokenKind;

    for token in tokens {
        if !matches!(token.kind(), TokenKind::String | TokenKind::FStringMiddle) {
            continue;
        }
        let flags = token.unwrap_string_flags();
        if flags.is_raw_string() || flags.is_byte_string() {
            continue;
        }
        let range = if token.kind() == TokenKind::String {
            TextRange::new(
                token.start() + flags.opener_len(),
                token.end() - flags.closer_len(),
            )
        } else {
            token.range()
        };
        check_string_escapes(range, source)?;
    }
    Ok(())
}

/// Checks each adjacent literal with its own raw prefix and content range.
fn check_string_literal(value: &ast::StringLiteral, source: &str) -> Result<(), String> {
    if matches!(
        value.flags.prefix(),
        ast::str_prefix::StringLiteralPrefix::Raw { .. }
    ) {
        Ok(())
    } else {
        check_string_escapes(value.content_range(), source)
    }
}

/// Scans only literal bytes; f-string field expressions have independent prefixes.
fn check_string_escapes(range: TextRange, source: &str) -> Result<(), String> {
    let bytes = &source.as_bytes()[usize::from(range.start())..usize::from(range.end())];
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] != b'\\' {
            i += 1;
            continue;
        }
        let width = match bytes[i + 1] {
            b'u' => 4,
            b'U' => 8,
            _ => 0,
        };
        if width != 0
            && let Some(hex) = bytes.get(i + 2..i + 2 + width)
            && let Ok(hex) = std::str::from_utf8(hex)
            && let Ok(codepoint) = u32::from_str_radix(hex, 16)
            && (0xd800..=0xdfff).contains(&codepoint)
        {
            return Err(error(
                source,
                &range,
                "surrogate string escapes are unsupported; strings must be valid UTF-8",
            ));
        }
        i += 2 + width;
    }
    Ok(())
}

/// Captured iteration inputs; no first-class Python iterator value is exposed.
enum Iteration {
    Range(Vec<Expr>),
    Container {
        iterable: Expr,
        items: bool,
        start: Option<Expr>,
    },
    /// A nonescaping iterator whose local state is resumed by its consumer.
    Prepared(pipelines::Prepared),
}

struct Checker<'a> {
    /// Doctest prompts share an exec-style namespace until the case ends.
    interactive_names: bool,
    /// A later local assignment shadows builtin iter throughout a function.
    local_iter: bool,
    positional_pack: Option<String>,
    context_method: bool,
    fixed_tuple: Option<fixed_tuples::Call>,
    generator_item: Option<Type>,
    asynchronous: bool,
    /// Capture only the outer sum of an assignment with a neighboring sum.
    zip_sum_rhs: Option<TextRange>,
    zip_sums: HashMap<usize, zip_sums::Sum>,
    source: &'a str,
    graph: &'a SourceGraph,
    module: ModuleId,
    functions: &'a HashMap<DefinitionId, Signature>,
    classes: &'a ClassInfo,
    types: &'a Types,
    helpers: &'a Helpers,
    globals: &'a ModuleValues,
    inference: Option<&'a inference::Inference>,
    inferred_returns: Option<Vec<Type>>,
    comprehension_targets: HashSet<String>,
    comprehension_depth: usize,
    comprehension_exports: HashMap<String, usize>,
    narrowed: assignments::Narrowed,
    regex_guards: Vec<(usize, regex::CaptureLayout)>,
    present_subscripts: Vec<ruff_text_size::TextRange>,
    regex_locals: HashMap<usize, regex::CaptureLayout>,
    /// Group dictionary locals whose complete set of reads resolves to capture slots.
    regex_fields: HashMap<usize, HashMap<String, usize>>,
    joined_lines: joined_lines::Plan,
    consuming_lists: HashSet<TextRange>,
    namespace_guards: Vec<(usize, usize)>,
    captured_process_errors: Vec<processes::CapturedFields>,
    returns: Type,
    nullable_assignments: HashMap<String, Type>,
    bound_flags: HashMap<String, usize>,
    omitted_assert_locals: HashMap<String, unbound_locals::Binding>,
    omitted_assert_free: HashMap<String, unbound_locals::Binding>,
    names: HashMap<String, usize>,
    local_functions: HashMap<String, local_functions::Binding>,
    local_classes: HashMap<String, local_classes::Binding>,
    locals: Vec<Type>,
    assigned: assignments::Assigned,
    calls: HashSet<usize>,
    class_reads: std::cell::RefCell<HashSet<DefinitionId>>,
    default_reads: Option<Vec<caller_cache::DefaultId>>,
    loop_breaks: Vec<Option<LocalState>>,
    block_depth: usize,
    in_finally: bool,
    reachable: bool,
    module_scope: bool,
    class: Option<usize>,
    class_receiver: Option<String>,
    /// Earlier assignments in the executing class body shadow module definitions.
    class_lexical: HashSet<String>,
    super_method_base: Option<usize>,
    iterator_source: Option<TextRange>,
    super_initializer: Option<records::BaseInitializer>,
    exception_formatter: Option<bool>,
    initializing: bool,
}

impl<'a> Checker<'a> {
    fn new(
        graph: &'a SourceGraph,
        module: ModuleId,
        functions: &'a HashMap<DefinitionId, Signature>,
        classes: &'a ClassInfo,
        types: &'a Types,
        helpers: &'a Helpers,
        globals: &'a ModuleValues,
    ) -> Self {
        Self {
            interactive_names: false,
            local_iter: false,
            positional_pack: None,
            zip_sum_rhs: None,
            asynchronous: false,
            context_method: false,
            fixed_tuple: None,
            generator_item: None,
            zip_sums: HashMap::new(),
            source: &graph.modules[module].source,
            graph,
            module,
            functions,
            classes,
            types,
            helpers,
            globals,
            inference: None,
            inferred_returns: None,
            comprehension_targets: HashSet::new(),
            comprehension_depth: 0,
            comprehension_exports: HashMap::new(),
            narrowed: assignments::Narrowed::new(),
            regex_guards: Vec::new(),
            present_subscripts: Vec::new(),
            regex_locals: HashMap::new(),
            regex_fields: HashMap::new(),
            joined_lines: joined_lines::Plan::default(),
            consuming_lists: HashSet::new(),
            namespace_guards: Vec::new(),
            captured_process_errors: Vec::new(),
            returns: Type::None,
            nullable_assignments: HashMap::new(),
            bound_flags: HashMap::new(),
            omitted_assert_locals: HashMap::new(),
            omitted_assert_free: HashMap::new(),
            names: HashMap::new(),
            local_functions: HashMap::new(),
            local_classes: HashMap::new(),
            locals: Vec::new(),
            assigned: assignments::Assigned::default(),
            calls: HashSet::new(),
            class_reads: Default::default(),
            default_reads: None,
            loop_breaks: Vec::new(),
            block_depth: 0,
            in_finally: false,
            reachable: true,
            module_scope: false,
            class: None,
            class_receiver: None,
            class_lexical: HashSet::new(),
            super_method_base: None,
            iterator_source: None,
            super_initializer: None,
            exception_formatter: None,
            initializing: false,
        }
    }

    /// Reuses a slot only at its original type and prevents rebinding fixed call targets.
    fn declare(&mut self, name: &str, ty: Type, node: &impl Ranged) -> Result<usize, String> {
        if (name != "__doc__" && reserved(name))
            || (self.graph.modules[self.module].bindings.contains_key(name)
                && (self.module_scope
                    || !self.mapping_class_import(name)
                    || (self.block_depth != 0 && !self.names.contains_key(name))))
            || self.local_functions.contains_key(name)
            || self.local_classes.contains_key(name)
            || self.class_receiver.as_deref() == Some(name)
        {
            return Err(error(
                self.source,
                node,
                "rebinding a builtin, definition, or imported module is unsupported",
            ));
        }
        if let Some(&slot) = self.names.get(name) {
            self.expect(self.locals[slot], ty, node)?;
            return Ok(slot);
        }
        let slot = self.hidden(ty);
        self.names.insert(name.to_owned(), slot);
        Ok(slot)
    }

    fn hidden(&mut self, ty: Type) -> usize {
        let slot = self.locals.len();
        self.locals.push(ty);
        slot
    }

    /// Reads a hidden binding at its storage type, without source-level narrowing.
    fn local(&self, slot: usize) -> Expr {
        Expr {
            ty: self.locals[slot],
            kind: ExprKind::Local(slot),
        }
    }

    fn expect(&self, expected: Type, actual: Type, node: &impl Ranged) -> Result<(), String> {
        if expected == actual {
            Ok(())
        } else {
            Err(error(
                self.source,
                node,
                &format!(
                    "expected {}, got {}; annotations are enforced contracts",
                    self.type_name(expected),
                    self.type_name(actual)
                ),
            ))
        }
    }

    fn type_name(&self, ty: Type) -> String {
        self.types.describe(ty, &|id| {
            let class = &self.classes.declarations.definitions[id];
            format!(
                "{}.{}",
                self.graph.modules[class.definition.module].name, class.name
            )
        })
    }

    /// Field accesses and Python methods use the existing fixed record machinery;
    /// every other reference retains the canonical exception object.
    fn exception_record(value: Expr) -> Expr {
        if let Type::NamedException(class) = value.ty {
            Expr {
                ty: Type::Class(class),
                kind: ExprKind::Builtin(Builtin::ExceptionRecord, vec![value]),
            }
        } else {
            value
        }
    }

    fn writable_field_value(
        &self,
        attribute: &ast::ExprAttribute,
        receiver: Expr,
    ) -> Result<(Expr, usize, Type), String> {
        let receiver = Self::exception_record(receiver);
        if let Type::ModuleSpec(id) = receiver.ty {
            let (field, ty) = self.module_spec_field(id, attribute)?;
            return Ok((receiver, field, ty));
        }
        if let Type::Class(class) = receiver.ty {
            let declaration = &self.classes.declarations.definitions[class];
            if self.classes.parsers.namespace(class) {
                return Err(error(
                    self.source,
                    attribute,
                    "argparse namespace fields are read-only",
                ));
            }
            if declaration
                .dataclass
                .as_ref()
                .is_some_and(|options| options.frozen)
                && !(self.initializing && self.class == Some(class))
            {
                return Err(error(
                    self.source,
                    attribute,
                    "frozen dataclass fields cannot be reassigned",
                ));
            }
            if self.classes.methods[class]
                .get(attribute.attr.as_str())
                .is_some_and(|signature| {
                    signature.method.unwrap().kind(&self.classes.declarations)
                        == crate::classes::MethodKind::Property
                })
            {
                return Err(error(self.source, attribute, "properties are read-only"));
            }
        }
        self.field_value(attribute, receiver)
    }

    fn field_value(
        &self,
        attribute: &ast::ExprAttribute,
        receiver: Expr,
    ) -> Result<(Expr, usize, Type), String> {
        if let Type::Class(class) = receiver.ty
            && self.native_mock(class)
            && self.module
                != self.classes.declarations.definitions[class]
                    .definition
                    .module
            && matches!(attribute.attr.as_str(), "_node" | "_nodes")
        {
            return Err(error(
                self.source,
                attribute,
                "private native mock storage is not a Python attribute",
            ));
        }
        let receiver = if let Type::Class(class) = receiver.ty
            && self.native_mock(class)
            && Self::mock_state_member(attribute.attr.as_str())
        {
            self.mock_node(receiver, class)
        } else {
            receiver
        };
        let Type::Class(class) = receiver.ty else {
            return Err(error(
                self.source,
                attribute,
                "field access requires a fixed class instance",
            ));
        };
        let field = self.classes.declarations.definitions[class]
            .fields
            .iter()
            .position(|field| field.name == attribute.attr.as_str())
            .ok_or_else(|| {
                error(
                    self.source,
                    attribute,
                    "unknown field; dynamic attributes and first-class methods are unsupported",
                )
            })?;
        Ok((receiver, field, self.classes.fields[class][field]))
    }

    /// Inferred writable storage keeps its established BinaryIO contract.
    /// Local assignments separately remember when the current owner is an exact BytesIO.
    fn inferred_stream(value: Expr) -> Expr {
        if value.ty == Type::BytesIO {
            Expr {
                ty: Type::BinaryIO,
                kind: ExprKind::Builtin(Builtin::RecordView, vec![value]),
            }
        } else {
            value
        }
    }

    /// Optional and finite-union promotions are explicit; containers stay invariant.
    fn coerce(&self, value: Expr, expected: Type, node: &impl Ranged) -> Result<Expr, String> {
        if value.ty == Type::FunctionRef && matches!(expected, Type::Callable(_)) {
            return Ok(typing::typed_function(value, expected));
        }
        if matches!(expected, Type::Document(_))
            || matches!(value.ty, Type::Document(_)) && expected == Type::Str
        {
            return self.document_coerce(value, expected, node);
        }
        if value.ty == Type::FrozenStrings && expected == Type::StringKeys {
            return Ok(Expr {
                ty: expected,
                kind: ExprKind::Builtin(Builtin::RecordView, vec![value]),
            });
        }
        if value.ty == expected {
            return Ok(value);
        }
        if value.ty == Type::BytesIO
            && self.types.payload(expected).unwrap_or(expected) == Type::BinaryIO
        {
            return self.coerce(Self::inferred_stream(value), expected, node);
        }
        if self.types.payload(value.ty) == Some(Type::BytesIO)
            && self.types.payload(expected) == Some(Type::BinaryIO)
        {
            return Ok(Expr {
                ty: expected,
                kind: ExprKind::Builtin(Builtin::RecordView, vec![value]),
            });
        }
        if let (Some(source), Some(target)) =
            (self.types.members(value.ty), self.types.members(expected))
            && source.iter().all(|member| target.contains(member))
        {
            // The closed unions share runtime tags. Widening preserves the
            // original member and its owner rather than converting an IntEnum.
            return Ok(Expr {
                ty: expected,
                kind: ExprKind::Builtin(Builtin::UnionProject, vec![value]),
            });
        }
        if self.types.payload(value.ty) == Some(Type::Calendar(ir::CalendarType::DateTime))
            && self.types.payload(expected) == Some(Type::Calendar(ir::CalendarType::Date))
        {
            return Ok(Expr {
                ty: expected,
                kind: ExprKind::Builtin(Builtin::RecordView, vec![value]),
            });
        }
        if value.ty == Type::Calendar(ir::CalendarType::DateTime)
            && self.types.payload(expected).unwrap_or(expected)
                == Type::Calendar(ir::CalendarType::Date)
        {
            let date = Expr {
                ty: Type::Calendar(ir::CalendarType::Date),
                kind: ExprKind::Builtin(Builtin::RecordView, vec![value]),
            };
            return self.coerce(date, expected, node);
        }
        if self.types.wrapped_signature(value.ty) == Some(expected) {
            return Ok(Expr {
                ty: expected,
                kind: ExprKind::Builtin(Builtin::RecordView, vec![value]),
            });
        }
        if self.types.payload(expected) == Some(Type::TextOutput)
            && matches!(
                self.types.payload(value.ty),
                Some(Type::CodecWriter | Type::StringIO | Type::TextIO)
            )
        {
            // Optional stream handles share a zero None word and the same rooted object payload.
            return Ok(Expr {
                ty: expected,
                kind: ExprKind::Builtin(Builtin::RecordView, vec![value]),
            });
        }
        if matches!(value.ty, Type::CodecWriter | Type::StringIO | Type::TextIO)
            && self.types.payload(expected).unwrap_or(expected) == Type::TextOutput
        {
            let stream = Expr {
                ty: Type::TextOutput,
                kind: ExprKind::Builtin(Builtin::RecordView, vec![value]),
            };
            return self.coerce(stream, expected, node);
        }
        if value.ty == Type::StringIO
            && self.types.payload(expected).unwrap_or(expected) == Type::TextIO
        {
            let stream = Expr {
                ty: Type::TextIO,
                kind: ExprKind::Builtin(Builtin::TextStreamView, vec![value]),
            };
            return self.coerce(stream, expected, node);
        }
        if expected == Type::StringIterable
            && (self.types.element(value.ty) == Some(Type::Str)
                || self
                    .types
                    .fields(value.ty)
                    .is_some_and(|fields| fields.iter().all(|t| *t == Type::Str)))
        {
            return Ok(Expr {
                ty: expected,
                kind: ExprKind::Builtin(Builtin::UnionInject, vec![value]),
            });
        }
        if let Some(element) = self.types.var_tuple_element(expected)
            && self
                .types
                .fields(value.ty)
                .is_some_and(|fields| fields.iter().all(|&ty| ty == element))
        {
            return Ok(Expr {
                ty: expected,
                kind: ExprKind::Builtin(Builtin::RecordView, vec![value]),
            });
        }
        if expected == Type::ObjectValue && self.object_value_supported(value.ty) {
            return Ok(Expr {
                ty: expected,
                kind: ExprKind::Builtin(Builtin::BoxValue, vec![value]),
            });
        }
        if (expected == Type::ReadOnlyObjectMapping
            || self.types.payload(expected) == Some(Type::ReadOnlyObjectMapping))
            && let Some((Type::Str, field)) = self.types.mapping(value.ty)
            && !self.types.is_counter(value.ty)
            && !self.types.is_default_list_dict(value.ty)
            && (field == Type::ObjectValue || self.boxable_value(field))
        {
            let view = Expr {
                ty: Type::ReadOnlyObjectMapping,
                kind: ExprKind::Builtin(Builtin::DictAsBase, vec![value]),
            };
            return if expected == view.ty {
                Ok(view)
            } else {
                Ok(Expr {
                    ty: expected,
                    kind: ExprKind::Present(Box::new(view)),
                })
            };
        }
        if let (Some(source), Some(destination)) = (
            self.types.directory_value(value.ty),
            self.types.directory_value(expected),
        ) && self.types.join(source, destination) == Some(destination)
        {
            return Ok(Expr {
                ty: expected,
                kind: ExprKind::Builtin(Builtin::DirectoryContextView, vec![value]),
            });
        }
        if matches!((value.ty, expected), (Type::Class(child), Type::Class(parent)) if self.classes.parsers.child_of(child, parent) || self.classes.declarations.definitions[child].bases.contains(&parent))
        {
            return Ok(Expr {
                ty: expected,
                kind: ExprKind::Builtin(Builtin::RecordView, vec![value]),
            });
        }
        let target = self.types.payload(expected).unwrap_or(expected);
        if matches!(value.ty, Type::JsonValue | Type::JsonRoot)
            && (matches!(
                target,
                Type::JsonObject | Type::Int | Type::Str | Type::Bool | Type::Float
            ) || self.types.element(target) == Some(Type::JsonObject))
        {
            return Ok(json::select(value, expected, None, None));
        }
        if (self.types.is_default_list_dict(value.ty)
            || self.types.is_counter(value.ty)
            || self.types.is_ordered_dict(value.ty))
            && !self.types.is_default_list_dict(expected)
            && !self.types.is_counter(expected)
            && !self.types.is_ordered_dict(expected)
            && self.types.mapping(value.ty) == self.types.mapping(expected)
        {
            return Ok(Expr {
                ty: expected,
                kind: ExprKind::Builtin(Builtin::DictAsBase, vec![value]),
            });
        }
        if matches!(value.ty, Type::StrEnum(_)) && expected == Type::Str {
            return Err(error(
                self.source,
                node,
                "passing a StrEnum as plain str requires str(member) or member.value",
            ));
        }
        if literals::accepts(self.types, expected, &value) {
            return Ok(literals::view(value, expected));
        }
        let payload = self.types.payload(value.ty).unwrap_or(value.ty);
        if matches!(payload, Type::StrLiteral(_))
            && self
                .types
                .members(self.types.payload(expected).unwrap_or(expected))
                .is_some_and(|members| members.contains(&Type::Str))
        {
            let string = if value.ty == payload {
                Type::Str
            } else {
                self.types.optional(Type::Str)
            };
            return self.coerce(literals::view(value, string), expected, node);
        }
        if matches!(
            value.ty,
            Type::ImportException | Type::TimeoutExpired | Type::NamedException(_)
        ) && expected == Type::Exception
        {
            return Ok(Expr {
                ty: expected,
                kind: ExprKind::Builtin(
                    Builtin::Process(ir::ProcessOperation::ExceptionView),
                    vec![value],
                ),
            });
        }
        if let Some(payload) = self.types.payload(expected) {
            if let Some(actual) = self.types.payload(value.ty)
                && literals::domain_fits(self.types, actual, payload)
            {
                return Ok(literals::view(value, expected));
            }
            if let Some(actual) = self.types.payload(value.ty)
                && self
                    .types
                    .members(payload)
                    .is_some_and(|members| members.contains(&actual))
            {
                return Ok(Expr {
                    ty: expected,
                    kind: ExprKind::Builtin(Builtin::UnionInject, vec![value]),
                });
            }
            let kind = if value.ty == Type::None {
                ExprKind::Absent(Box::new(value))
            } else {
                ExprKind::Present(Box::new(self.coerce(value, payload, node)?))
            };
            return Ok(Expr { ty: expected, kind });
        }
        if self
            .types
            .members(expected)
            .is_some_and(|members| members.contains(&value.ty))
        {
            return Ok(Expr {
                ty: expected,
                kind: ExprKind::Builtin(Builtin::UnionInject, vec![value]),
            });
        }
        self.expect(expected, value.ty, node)?;
        unreachable!("mismatched exact types return an error")
    }

    /// Adapt an evaluated argument to the target's concrete or private representation.
    fn coerce_argument_value(
        &mut self,
        value: Expr,
        expected: Type,
        site: &ast::Expr,
    ) -> Result<Expr, String> {
        if self.types.is_len_argument(expected) {
            self.pack_len_value(value, site)
        } else if self.types.is_callback_argument(expected) {
            self.pack_callback_value(value, site, expected)
        } else {
            self.coerce(value, expected, site)
        }
    }

    /// Context determines empty literals and optional elements at storage boundaries.
    fn expr_expected(&mut self, expression: &ast::Expr, expected: Type) -> Result<Expr, String> {
        self.expr_expected_or(expression, expected, None)
    }

    /// A partial can accept a sentinel in place of a parameter. Preserve its type
    /// while checking ordinary operands in the declared parameter's context.
    fn expr_expected_or(
        &mut self,
        expression: &ast::Expr,
        expected: Type,
        alternative: Option<Type>,
    ) -> Result<Expr, String> {
        self.expr_expected_if(expression, expected, |_, ty| alternative == Some(ty))
    }

    /// Preserve a permitted alternative for a caller that supplies its own conversion.
    fn expr_expected_if(
        &mut self,
        expression: &ast::Expr,
        expected: Type,
        alternative: impl Fn(&Self, Type) -> bool,
    ) -> Result<Expr, String> {
        if matches!(expected, Type::ContextVar(_))
            && let ast::Expr::Call(call) = expression
            && self.resolve(&call.func) == Some(Binding::Intrinsic(Intrinsic::ContextVar))
        {
            return self.context_constructor(call, self.types.context_value(expected));
        }
        if let Some(name) = self.unbound_assert_root(expression) {
            return self.omitted_assert_read(name, Some(expected), expression);
        }
        if self.present_subscripts.contains(&expression.range()) {
            let value = self.expr(expression)?;
            if self.types.payload(value.ty) == Some(expected) {
                return Ok(Expr {
                    ty: expected,
                    kind: ExprKind::Unwrap(Box::new(value)),
                });
            }
            return self.coerce(value, expected, expression);
        }
        if self.types.is_len_argument(expected) || self.types.is_callback_argument(expected) {
            let value = self.expr(expression)?;
            // A partial consumes its marker before the target's own argument ABI.
            // Keep the evaluated value so a lookup or callback runs exactly once.
            return if alternative(self, value.ty) {
                Ok(value)
            } else {
                self.coerce_argument_value(value, expected, expression)
            };
        }
        // A record can carry user fields with distinct types. An expected scalar type
        // permits reading such a field; the runtime checks its actual representation.
        if let ast::Expr::Attribute(attr) = expression
            && let ast::Expr::Name(owner) = &*attr.value
            && self
                .names
                .get(owner.id.as_str())
                .is_some_and(|&slot| self.locals[slot] == Type::LogRecord)
            && matches!(
                self.types.payload(expected).unwrap_or(expected),
                Type::Str | Type::Int | Type::Float | Type::Bool | Type::Bytes | Type::None
            )
        {
            let record = self.expr(&attr.value)?;
            return Ok(logging_records::op(
                ir::LogRecordOperation::Read,
                expected,
                vec![record, unpacking::string(attr.attr.as_str())],
            ));
        }
        if expected == Type::JsonObject
            && matches!(expression, ast::Expr::Dict(_))
            && documents::constant(expression)
        {
            let doc = self.document_literal(expression, documents::MAP)?.unwrap();
            return Ok(documents::op(
                ir::DocumentOp::FreezeMap,
                expected,
                vec![doc],
            ));
        }
        if matches!(expected, Type::Document(_))
            && let Some(value) = self.document_literal(expression, expected)?
        {
            return Ok(value);
        }
        self.reject_class_receiver_value(expression)?;
        if self.types.element(expected) == Some(Type::Str)
            && let Some(separator) = self.joined_lines.start_buffer(expression.range())
        {
            return Ok(Expr {
                ty: expected,
                kind: ExprKind::Builtin(
                    Builtin::JoinBufferNew,
                    vec![Expr {
                        ty: Type::Str,
                        kind: ExprKind::Str(separator),
                    }],
                ),
            });
        }
        let mut concrete = self.types.payload(expected).unwrap_or(expected);
        if let ast::Expr::Call(call) = expression
            && let Some(raised) = self.defaultdict_factory_error(call, expected)?
        {
            return Ok(raised);
        }
        if let ast::Expr::Call(call) = expression
            && let Some(shape) = self.types.mapping(concrete)
            && let Some(result) = self.fromkeys_call(call, Some(shape))?
        {
            return self.coerce(result, expected, expression);
        }
        if concrete == Type::StringIterable && matches!(expression, ast::Expr::List(_)) {
            concrete = self.types.list(Type::Str);
        }
        if matches!(expression, ast::Expr::List(_))
            && let Some(members) = self.types.members(concrete)
        {
            let mut lists = members
                .into_iter()
                .filter(|&ty| self.types.element(ty).is_some());
            if let (Some(list), None) = (lists.next(), lists.next()) {
                concrete = list;
            }
        }
        if matches!(concrete, Type::Callable(_)) {
            if let Some(base) = self.types.constructor_base(concrete) {
                let value = if let Some(class) = self.class_reference(expression) {
                    self.record_constructor_value(class, base, concrete, expression)?
                } else {
                    self.expr(expression)?
                };
                if value.ty != concrete {
                    return Err(error(
                        self.source,
                        expression,
                        "expected a compatible record class constructor",
                    ));
                }
                return self.coerce(value, expected, expression);
            }
            if self.resolve(expression) == Some(Binding::Intrinsic(Intrinsic::JsonLoads)) {
                let value = self.json_load_callable(None, Some(concrete), expression)?;
                return self.coerce(value, expected, expression);
            }
            if let Some(Binding::Intrinsic(
                op @ (Intrinsic::Copy(_)
                | Intrinsic::RmTree
                | Intrinsic::Move
                | Intrinsic::DestInSrc),
            )) = self.resolve(expression)
            {
                let value = self.shutil_function_value(op, Some(concrete), expression)?;
                return self.coerce(value, expected, expression);
            }
            if let Some(Binding::Definition(definition)) = self.resolve(expression)
                && let Some(signature) = self.functions.get(&definition).cloned()
            {
                let value = self.callable_value(&signature, None, Some(concrete), expression)?;
                return self.coerce(value, expected, expression);
            }
            if let ast::Expr::Attribute(attribute) = expression
                && self.resolve(expression).is_none()
            {
                if let Some(class) = self.class_reference(&attribute.value) {
                    let value = self.class_attribute(attribute, class, None, Some(concrete))?;
                    return self.coerce(value, expected, expression);
                }
                let receiver = Self::weak_target(self.expr(&attribute.value)?);
                if let Some(wrapped) = self.wrapped_function(&receiver, attribute.attr.as_str()) {
                    return self.coerce(wrapped, expected, expression);
                }
                if receiver.ty == Type::JsonDecoder && attribute.attr.as_str() == "decode" {
                    let value =
                        self.json_load_callable(Some(receiver), Some(concrete), expression)?;
                    return self.coerce(value, expected, expression);
                }
                if let Some(element) = self.types.deque_element(receiver.ty) {
                    let value = self.deque_attribute(attribute, receiver, element)?;
                    return self.coerce(value, expected, expression);
                }
                if let Some(field) = self.partial_attribute(&receiver, attribute.attr.as_str()) {
                    return self.coerce(field, expected, expression);
                }
                if let Type::Class(class)
                | Type::IntEnum(class)
                | Type::StrEnum(class)
                | Type::Enum(class) = receiver.ty
                {
                    let value =
                        self.class_attribute(attribute, class, Some(receiver), Some(concrete))?;
                    return self.coerce(value, expected, expression);
                }
                return Err(error(
                    self.source,
                    expression,
                    "expected a fixed bound method",
                ));
            }
        }
        let value = match expression {
            ast::Expr::Dict(dict) if concrete == Type::ReadOnlyObjectMapping => {
                let value = self.dictionary(dict, Some((Type::Str, Type::ObjectValue)))?;
                self.coerce(value, concrete, expression)?
            }
            ast::Expr::Call(call)
                if self.types.is_counter(concrete)
                    && self.resolve(&call.func) == Some(Binding::Intrinsic(Intrinsic::Counter))
                    && call.arguments.is_empty() =>
            {
                Expr {
                    ty: concrete,
                    kind: ExprKind::Builtin(Builtin::CounterNew, Vec::new()),
                }
            }
            ast::Expr::Lambda(lambda) => {
                let (params, returns) = self.types.signature(concrete).ok_or_else(|| {
                    error(
                        self.source,
                        lambda,
                        "a lambda requires an exact Callable context",
                    )
                })?;
                self.lambda_value(lambda, Some(params), Some(returns))?
            }
            ast::Expr::Named(value) => self.named_expression(value, Some(expected))?,
            ast::Expr::If(value) => self.conditional(value, Some(expected))?,
            ast::Expr::BoolOp(value) => self.boolean_value(value, Some(expected))?,
            ast::Expr::Subscript(subscript)
                if matches!(&*subscript.slice, ast::Expr::Slice(_))
                    && (matches!(concrete, Type::Str | Type::Bytes)
                        || self.types.element(concrete).is_some()) =>
            {
                let receiver = self.expr_expected(&subscript.value, concrete)?;
                let ast::Expr::Slice(slice) = &*subscript.slice else {
                    unreachable!("slice guard checked the node");
                };
                self.slice(receiver, slice)?
            }
            ast::Expr::ListComp(_) | ast::Expr::DictComp(_) | ast::Expr::SetComp(_) => {
                self.comprehension(expression, Some(concrete))?
            }
            ast::Expr::Dict(dict) if self.types.mapping(concrete).is_some() => {
                self.dictionary(dict, self.types.mapping(concrete))?
            }
            ast::Expr::Set(set) if self.types.set_element(concrete).is_some() => {
                self.set_literal(set, self.types.set_element(concrete))?
            }
            ast::Expr::Call(call)
                if self.types.element(concrete).is_some()
                    && matches!(&*call.func, ast::Expr::Name(name) if matches!(name.id.as_str(), "list" | "sorted")) =>
            {
                if matches!(&*call.func, ast::Expr::Name(name) if name.id == "sorted") {
                    self.sorted_call(call, self.types.element(concrete))?
                } else {
                    self.list_call(call, self.types.element(concrete))?
                }
            }
            ast::Expr::Attribute(attr)
                if attr.attr.as_str() == "maps"
                    && let Some(dict) = self.types.element(concrete)
                    && let Some((key, value)) = self.types.mapping(dict)
                    && self.types.dict(key, value) == dict
                    && let ast::Expr::Call(call) = &*attr.value
                    && self.resolve(&call.func)
                        == Some(Binding::Intrinsic(Intrinsic::ChainMap)) =>
            {
                // A typed list target supplies the dictionary shape for an
                // otherwise empty ChainMap constructor.
                let owner = self.chainmap(call, Some((key, value)))?;
                Expr {
                    ty: concrete,
                    kind: ExprKind::Builtin(Builtin::ChainMapMaps, vec![owner]),
                }
            }
            ast::Expr::Call(call)
                if self.resolve(&call.func) == Some(Binding::Intrinsic(Intrinsic::ChainMap))
                    && let Type::ChainMap(id) = concrete =>
            {
                self.chainmap(call, self.types.mapping(Type::Dict(id)))?
            }
            ast::Expr::Call(call)
                if self.resolve(&call.func) == Some(Binding::Intrinsic(Intrinsic::Deque))
                    && self.types.deque_element(concrete).is_some() =>
            {
                self.deque_new(call, self.types.deque_element(concrete))?
            }
            ast::Expr::Call(call)
                if self.resolve(&call.func) == Some(Binding::Intrinsic(Intrinsic::Counter))
                    && self.types.mapping(concrete).is_some() =>
            {
                self.counter(call, None, self.types.mapping(concrete).map(|(key, _)| key))?
            }
            ast::Expr::Call(call)
                if self.resolve(&call.func) == Some(Binding::Intrinsic(Intrinsic::OrderedDict)) =>
            {
                self.ordered_dict_constructor(call, self.types.mapping(concrete))?
            }
            ast::Expr::Call(call) if self.is_defaultdict_constructor(&call.func) => {
                let (key, value) = self.types.mapping(concrete).ok_or_else(|| {
                    error(
                        self.source,
                        call,
                        "defaultdict(list) requires a typed dictionary context",
                    )
                })?;
                if self.types.element(value).is_none()
                    || !matches!(call.arguments.args.as_slice(), [factory @ ast::Expr::Name(name)]
                        if name.id == "list" && !self.names.contains_key("list") && self.resolve(factory).is_none())
                    || !call.arguments.keywords.is_empty()
                {
                    return Err(error(
                        self.source,
                        call,
                        "only defaultdict(list) with a typed list value is supported",
                    ));
                }
                Expr {
                    ty: self.types.default_list_dict(key, value),
                    kind: ExprKind::Builtin(Builtin::DefaultListDict, Vec::new()),
                }
            }
            ast::Expr::Call(call)
                if self.types.mapping(concrete).is_some()
                    && self.unshadowed_builtin(&call.func, "dict") =>
            {
                self.dict_constructor(call, self.types.mapping(concrete))?
            }
            ast::Expr::Call(call)
                if self.types.set_element(concrete).is_some()
                    && matches!(&*call.func, ast::Expr::Name(name) if name.id == "set")
                    && call.arguments.args.is_empty()
                    && call.arguments.keywords.is_empty() =>
            {
                Expr {
                    ty: concrete,
                    kind: ExprKind::Set {
                        element: self.types.set_element(concrete).unwrap(),
                        values: Vec::new(),
                    },
                }
            }
            ast::Expr::List(list) if self.types.element(concrete).is_some() => {
                let element = self.types.element(concrete).unwrap();
                if list
                    .elts
                    .iter()
                    .any(|value| matches!(value, ast::Expr::Starred(_)))
                {
                    return self
                        .expanded_sequence(&list.elts, Some(element), false, list)
                        .and_then(|value| self.coerce(value, expected, expression));
                }
                let values = list
                    .elts
                    .iter()
                    .map(|value| self.expr_expected(value, element))
                    .collect::<Result<Vec<_>, _>>()?;
                Expr {
                    ty: concrete,
                    kind: ExprKind::List(values),
                }
            }
            ast::Expr::Tuple(tuple) if self.types.fields(concrete).is_some() => {
                let fields = self.types.fields(concrete).unwrap();
                if tuple
                    .elts
                    .iter()
                    .any(|v| matches!(v, ast::Expr::Starred(_)))
                {
                    let value = self.expanded_tuple(tuple, Some(&fields))?;
                    return self.coerce(value, expected, expression);
                }
                if tuple.elts.len() != fields.len() {
                    return Err(error(
                        self.source,
                        expression,
                        "tuple literal has the wrong number of fields",
                    ));
                }
                let values = tuple
                    .elts
                    .iter()
                    .zip(fields)
                    .map(|(value, ty)| self.expr_expected(value, ty))
                    .collect::<Result<Vec<_>, _>>()?;
                Expr {
                    ty: concrete,
                    kind: ExprKind::Tuple(values),
                }
            }
            ast::Expr::BinOp(binary)
                if binary.op == ast::Operator::Add && self.types.element(concrete).is_some() =>
            {
                let left = self.expr_expected(&binary.left, concrete)?;
                let right = self.expr_expected(&binary.right, concrete)?;
                self.concat_lists(left, right, expression)?
            }
            ast::Expr::BinOp(binary)
                if binary.op == ast::Operator::Mult && self.types.element(concrete).is_some() =>
            {
                let left = if matches!(&*binary.left, ast::Expr::List(_)) {
                    self.expr_expected(&binary.left, concrete)?
                } else {
                    self.expr(&binary.left)?
                };
                let right = if left.ty == Type::Int {
                    self.expr_expected(&binary.right, concrete)?
                } else {
                    self.expr_expected(&binary.right, Type::Int)?
                };
                self.binary_expr(binary.op, left, right, expression)?
            }
            _ => self.typed_expr(expression)?,
        };
        if alternative(self, value.ty) {
            return Ok(value);
        }
        self.coerce(value, expected, expression)
    }

    /// Checks both arms under their own narrowing facts, while contextual types
    /// reach literals before the branches join. Only optional promotion can
    /// widen a result; unrelated concrete storage types never share a value.
    fn conditional(&mut self, value: &ast::ExprIf, expected: Option<Type>) -> Result<Expr, String> {
        fn empty_display(checker: &Checker<'_>, arm: &ast::Expr) -> bool {
            matches!(arm, ast::Expr::Dict(d) if d.items.is_empty())
                || matches!(arm, ast::Expr::List(l) if l.elts.is_empty())
                || matches!(arm, ast::Expr::Call(c) if c.arguments.args.is_empty()
                    && c.arguments.keywords.is_empty()
                    && matches!(&*c.func, ast::Expr::Name(n) if n.id == "set"
                        && !checker.names.contains_key(n.id.as_str()) && checker.resolve(&c.func).is_none()))
        }
        // An empty display obtains its invariant element type from its sibling
        // when no surrounding annotation supplied one.
        let mut sibling = None;
        self.conditional_with(
            value,
            expected.is_none() && empty_display(self, &value.body),
            |checker, arm| {
                let context =
                    expected.or_else(|| empty_display(checker, arm).then_some(sibling).flatten());
                let checked = if let Some(ty) = context {
                    checker.expr_expected(arm, ty)?
                } else {
                    checker.expr(arm)?
                };
                sibling = Some(checked.ty);
                Ok(checked)
            },
        )
    }

    /// Checks branch-local facts once for value, contextual, and truth consumers.
    fn conditional_with(
        &mut self,
        value: &ast::ExprIf,
        reversed: bool,
        mut check: impl FnMut(&mut Self, &ast::Expr) -> Result<Expr, String>,
    ) -> Result<Expr, String> {
        let test = self.condition(&value.test)?;
        let mut facts = assignments::ConditionalFacts::capture(self, value);
        let mut arm = |checker: &mut Self, truth: bool| {
            checker.apply_facts(&value.test, truth);
            let expression = if truth { &*value.body } else { &*value.orelse };
            let guards = checker.namespace_guards.len();
            if narrowing::expression_assignments(expression).is_empty() {
                checker.enter_namespace_guards(&value.test, truth, &[]);
            }
            let result = check(checker, expression);
            checker.namespace_guards.truncate(guards);
            result
        };
        let first = arm(self, !reversed)?;
        facts.switch_arm(self);
        let second = arm(self, reversed)?;
        facts.join(self);
        let (yes, no) = if reversed {
            (second, first)
        } else {
            (first, second)
        };
        // Context can promote both concrete arms for optional storage. Keep the
        // expression's narrower result when those original values agree.
        let yes = assignments::unpromoted(yes);
        let no = assignments::unpromoted(no);
        let ty = self.types.join(yes.ty, no.ty).ok_or_else(|| {
            error(
                self.source,
                value,
                "conditional branches require the same concrete type or its optional form",
            )
        })?;
        let yes = self.coerce(yes, ty, &*value.body)?;
        let no = self.coerce(no, ty, &*value.orelse)?;
        Ok(Expr {
            ty,
            kind: ExprKind::Conditional(Box::new(test), Box::new(yes), Box::new(no)),
        })
    }

    fn apply_facts(&mut self, test: &ast::Expr, truth: bool) {
        for name in narrowing::guaranteed_assignments(test, Some(truth)) {
            if let Some(&slot) = self.names.get(&name) {
                self.assigned.insert(slot);
            }
        }
        let always_truthy = |name: &str| {
            self.names.get(name).is_some_and(|&slot| {
                self.types
                    .payload(self.locals[slot])
                    .is_some_and(|payload| self.types.truthiness(payload) == Some(true))
            })
        };
        for (name, present) in narrowing::facts(test, truth, &always_truthy) {
            if let Some(&slot) = self.names.get(&name)
                && let Some(payload) = self.types.payload(self.locals[slot])
            {
                let current = self
                    .narrowed
                    .get(&slot)
                    .copied()
                    .unwrap_or(self.locals[slot]);
                let narrowed = if !present {
                    Type::None
                } else if current == Type::None {
                    payload
                } else {
                    self.types.payload(current).unwrap_or(current)
                };
                self.narrowed.insert_mut(slot, narrowed);
            }
        }
        self.apply_instance_facts(test, truth);
        self.apply_literal_fact(test, truth);
    }

    /// Captures the receiver before ordered bounds. Nullable integer operands
    /// preserve omitted bounds without allocating a general slice object.
    fn slice(&mut self, receiver: Expr, slice: &ast::ExprSlice) -> Result<Expr, String> {
        if let Some(fields) = self.types.fields(receiver.ty) {
            return self.fixed_tuple_slice(receiver, &fields, slice);
        }
        if !matches!(receiver.ty, Type::Str | Type::Bytes)
            && self.types.element(receiver.ty).is_none()
        {
            return Err(error(
                self.source,
                slice,
                "slicing requires a concrete list, string, or bytes",
            ));
        }
        let ty = receiver.ty;
        let mut args = vec![receiver];
        let index_type = self.types.optional(Type::Int);
        for bound in [&slice.lower, &slice.upper, &slice.step] {
            args.push(if let Some(bound) = bound {
                self.expr_expected(bound, index_type)?
            } else {
                Expr {
                    ty: index_type,
                    kind: ExprKind::Absent(Box::new(Expr {
                        ty: Type::None,
                        kind: ExprKind::None,
                    })),
                }
            });
        }
        Ok(Expr {
            ty,
            kind: ExprKind::Builtin(Builtin::Slice, args),
        })
    }

    /// Loop backedges can observe stores from a previous iteration.
    fn invalidate_loop_facts(
        &mut self,
        body: &[ast::Stmt],
        source: Option<(&pipelines::Prepared, &ast::Expr)>,
    ) -> Result<(), String> {
        for name in narrowing::assigned_names(body) {
            if let Some(slot) = self.names.get(&name) {
                self.narrowed.remove_mut(slot);
            }
        }
        for name in narrowing::deleted_names(body) {
            if let Some(slot) = self.names.get(&name) {
                if let Some((input, node)) = source
                    && input.reads_on_pull(*slot)
                {
                    return Err(error(
                        self.source,
                        node,
                        &format!("iterator may read '{name}' after the loop body deletes it"),
                    ));
                }
                self.assigned.remove(slot);
            }
        }
        Ok(())
    }

    fn assigned_value(&mut self, slot: usize, value: &Expr) {
        self.regex_locals.remove(&slot);
        self.regex_fields.remove(&slot);
        self.assigned.insert(slot);
        self.narrowed.remove_mut(&slot);
        if let Some(payload) = self.types.payload(self.locals[slot]) {
            match value.kind {
                ExprKind::Present(_) => {
                    self.narrowed.insert_mut(slot, payload);
                }
                ExprKind::Absent(_) => {
                    self.narrowed.insert_mut(slot, Type::None);
                }
                _ => {}
            }
        }
        let value = match &value.kind {
            ExprKind::Present(value) => value.as_ref(),
            _ => value,
        };
        if let ExprKind::Builtin(Builtin::RecordView, args) = &value.kind
            && ((self.locals[slot] == documents::ANY
                && matches!(args[0].ty, documents::LIST | documents::MAP))
                || (value.ty == Type::BinaryIO && args[0].ty == Type::BytesIO)
                || (value.ty == Type::Calendar(ir::CalendarType::Date)
                    && args[0].ty == Type::Calendar(ir::CalendarType::DateTime))
                || self
                    .types
                    .partial_function(args[0].ty)
                    .is_some_and(|p| p.callable == self.locals[slot])
                || self.types.index_callable_signature(args[0].ty) == Some(self.locals[slot]))
        {
            self.narrowed.insert_mut(slot, args[0].ty);
        }
        if let ExprKind::Builtin(Builtin::UnionInject, args) = &value.kind
            && (matches!(args[0].ty, Type::Str | Type::Bytes | Type::Float)
                || args[0].ty == Type::Int && value.ty == self.types.string_or_int())
        {
            self.narrowed.insert_mut(slot, args[0].ty);
        }
    }

    /// Dictionary storage has one exact key/value pair of types, even when empty.
    fn dictionary(
        &mut self,
        dict: &ast::ExprDict,
        expected: Option<(Type, Type)>,
    ) -> Result<Expr, String> {
        if dict.items.iter().any(|item| item.key.is_none()) {
            return self.expanded_dictionary(dict, expected);
        }
        let mut types = expected.or_else(|| {
            if documents::mixed_scalar_fields(dict) {
                return Some((Type::Str, documents::SCALAR));
            }
            self.graph
                .empty_keywords
                .contains(self.module, dict)
                .then_some((Type::Str, Type::None))
        });
        let mut entries = Vec::with_capacity(dict.items.len());
        for item in &dict.items {
            let key = item
                .key
                .as_ref()
                .ok_or_else(|| error(self.source, dict, "dictionary expansion is unsupported"))?;
            let (key, value) = if let Some((key_type, value_type)) = types {
                (
                    self.expr_expected(key, key_type)?,
                    self.expr_expected(&item.value, value_type)?,
                )
            } else {
                let key_value = self.expr(key)?;
                if !self.types.hashable_key(key_value.ty) {
                    return Err(error(
                        self.source,
                        key,
                        "dictionary keys must be int, str, bytes, Path, or immutable scalar tuples",
                    ));
                }
                let value = Self::inferred_stream(self.expr(&item.value)?);
                types = Some((key_value.ty, value.ty));
                (key_value, value)
            };
            entries.push((key, value));
        }
        let (key, value) = types.ok_or_else(|| {
            error(
                self.source,
                dict,
                "an empty dictionary requires a concrete dict[K, V] context",
            )
        })?;
        Ok(Expr {
            ty: self.types.dict(key, value),
            kind: ExprKind::Dict {
                key,
                value,
                entries,
            },
        })
    }

    /// Exact scalar elements keep hashing and equality free of user callbacks.
    fn set_literal(&mut self, set: &ast::ExprSet, expected: Option<Type>) -> Result<Expr, String> {
        if set
            .elts
            .iter()
            .any(|value| matches!(value, ast::Expr::Starred(_)))
        {
            return self.expanded_sequence(&set.elts, expected, true, set);
        }
        let mut element = expected;
        let mut values = Vec::with_capacity(set.elts.len());
        for value in &set.elts {
            let checked = if let Some(element) = element {
                self.expr_expected(value, element)?
            } else {
                self.expr(value)?
            };
            if !self.types.hashable_key(checked.ty) {
                return Err(error(
                    self.source,
                    value,
                    "set elements must be int, str, bytes, Path, or immutable scalar tuples",
                ));
            }
            element = Some(checked.ty);
            values.push(checked);
        }
        let element = element.ok_or_else(|| {
            error(
                self.source,
                set,
                "an empty set requires a concrete set[T] context",
            )
        })?;
        Ok(Expr {
            ty: self.types.set(element),
            kind: ExprKind::Set { element, values },
        })
    }

    /// Recognizes items views only at a loop boundary; no view value escapes.
    fn iteration_source(&mut self, expression: &ast::Expr) -> Result<Iteration, String> {
        if let ast::Expr::Call(call) = expression
            && let ast::Expr::Attribute(attribute) = &*call.func
            && attribute.attr.as_str() == "items"
            && self.class_reference(&attribute.value).is_none()
        {
            let receiver = self.expr(&attribute.value)?;
            if let Type::Class(class) = receiver.ty {
                let value = self.iteration_method_call(call, attribute, class, receiver)?;
                return self.iteration_value(value, false, expression);
            }
            if self.dict_or_record(receiver.ty).is_some() {
                return self
                    .dict_or_record_items(receiver, attribute, call)
                    .map(Iteration::Prepared);
            }
            let receiver = if matches!(receiver.ty, Type::Document(_)) {
                self.document_map(receiver)
            } else {
                receiver
            };
            if self.types.mapping(receiver.ty).is_some() {
                if !call.arguments.args.is_empty() || !call.arguments.keywords.is_empty() {
                    return Err(error(self.source, call, "dict.items() takes no arguments"));
                }
                return self.iteration_value(receiver, true, expression);
            }
        }
        let value = self.iteration_expression(expression, None)?;
        self.iteration_value(self.json_iteration(value), false, expression)
    }

    /// Checks an iterable in its current lexical scope, before target binding.
    fn iteration_input(&mut self, expression: &ast::Expr) -> Result<Iteration, String> {
        if let Some(input) = self.pipeline_input(expression, None)? {
            return Ok(input);
        }
        if let ast::Expr::Call(call) = expression
            && matches!(&*call.func, ast::Expr::Name(name) if name.id == "range")
        {
            if !call.arguments.keywords.is_empty() || !(1..=3).contains(&call.arguments.args.len())
            {
                return Err(error(
                    self.source,
                    expression,
                    "range requires one to three positional integer arguments",
                ));
            }
            let args = call
                .arguments
                .args
                .iter()
                .map(|argument| self.expr_expected(argument, Type::Int))
                .collect::<Result<Vec<_>, _>>()?;
            return Ok(Iteration::Range(args));
        }
        self.iteration_source(expression)
    }

    /// Prepare cursors and generators for pulling while preserving concrete container iteration.
    fn iteration_value(
        &mut self,
        iterable: Expr,
        items: bool,
        node: &impl Ranged,
    ) -> Result<Iteration, String> {
        if let Some(prepared) = self.record_iteration(&iterable, node)? {
            return Ok(Iteration::Prepared(prepared));
        }
        Ok(Iteration::Container {
            iterable,
            items,
            start: None,
        })
    }

    /// Keeps the original container rooted and owns each item before user code runs.
    fn iteration(&mut self, loop_: &ast::StmtFor) -> Result<(Vec<Statement>, bool), String> {
        if let ast::Expr::Call(call) = loop_.iter.as_ref()
            && self.resolve(&call.func) == Some(Binding::Intrinsic(Intrinsic::GroupBy))
        {
            return self.groupby_iteration(loop_, call);
        }
        let mut input = match self.unpacked_list_enumeration(loop_)? {
            Some(input) => input,
            None => self.iteration_input(&loop_.iter)?,
        };
        input = self.fuse_splitlines(input);
        if let Iteration::Container {
            iterable,
            items: false,
            start: None,
        } = &input
            && let Some(fields) = self.types.fields(iterable.ty)
            && fields.windows(2).any(|pair| pair[0] != pair[1])
            && fields.len() <= 8
            && let ast::Expr::Name(target) = loop_.target.as_ref()
            && !self.names.contains_key(target.id.as_str())
            && let [ast::Stmt::If(guard)] = loop_.body.as_slice()
            && guard.elif_else_clauses.is_empty()
            && matches!(guard.body.as_slice(), [ast::Stmt::Return(_)])
        {
            return self.tuple_search(loop_, target, iterable.clone(), &fields);
        }
        if let Iteration::Container {
            iterable,
            items: false,
            start: None,
        } = &mut input
            && let ExprKind::List(values) = &mut iterable.kind
            && (1..=8).contains(&values.len())
            && values
                .iter()
                .all(|value| matches!(value.kind, ExprKind::Str(_)))
        {
            // This fresh, unaliased list has no observable mutations or element
            // effects. Prepared tuple iteration lets scalar replacement remove
            // the container while retaining ordinary loop ownership and exits.
            iterable.ty = self.types.tuple(vec![Type::Str; values.len()]);
            iterable.kind = ExprKind::Tuple(std::mem::take(values));
        }
        if matches!(&input, Iteration::Container { iterable, items: false, start: None }
            if matches!(iterable.ty, Type::Tuple(_)) || self.consumes_list(iterable.ty, loop_.iter.range()))
        {
            input =
                Iteration::Prepared(self.prepare_iteration(input, loop_.iter.as_ref(), false)?);
        }
        self.invalidate_loop_facts(
            &loop_.body,
            match &input {
                Iteration::Prepared(input) => Some((input, &loop_.iter)),
                _ => None,
            },
        )?;
        for name in narrowing::expression_assignments(&loop_.target) {
            if let Some(slot) = self.names.get(&name) {
                self.narrowed.remove_mut(slot);
            }
        }
        self.lower_iteration(&loop_.target, input, &loop_.orelse, |checker| {
            Ok(checker.block(&loop_.body)?.0)
        })
    }

    /// A guarded return can inspect each fixed tuple field at its own static type.
    /// Capture the entire tuple first, including effects from its later fields.
    fn tuple_search(
        &mut self,
        loop_: &ast::StmtFor,
        target: &ast::ExprName,
        iterable: Expr,
        fields: &[Type],
    ) -> Result<(Vec<Statement>, bool), String> {
        let captured = self.hidden(iterable.ty);
        let mut body = vec![Statement::Assign(captured, iterable)];
        let mut clear = vec![captured];
        for (index, &ty) in fields.iter().enumerate() {
            let slot = self.declare(target.id.as_str(), ty, target)?;
            self.assigned.insert(slot);
            body.push(Statement::Assign(
                slot,
                Expr {
                    ty,
                    kind: ExprKind::Field(Box::new(self.local(captured)), index),
                },
            ));
            body.extend(self.block(&loop_.body)?.0);
            if index + 1 < fields.len() {
                body.push(Statement::Clear(slot));
                clear.push(slot);
                self.names.remove(target.id.as_str());
                self.assigned.remove(&slot);
                self.narrowed.remove_mut(&slot);
            }
        }
        let mut result = vec![Statement::Scope { body, clear }];
        let (orelse, falls) = self.block(&loop_.orelse)?;
        result.extend(orelse);
        Ok((result, falls))
    }

    /// An unpacked enumerate over a known list needs only its live cursor.
    /// Keep the pipeline for escaping tuples, nested iterators and complex targets.
    fn unpacked_list_enumeration(
        &mut self,
        loop_: &ast::StmtFor,
    ) -> Result<Option<Iteration>, String> {
        let Some([ast::Expr::Name(_), ast::Expr::Name(_)]) = pattern_elements(&loop_.target) else {
            return Ok(None);
        };
        let ast::Expr::Call(call) = &*loop_.iter else {
            return Ok(None);
        };
        let [source @ ast::Expr::Name(name)] = call.arguments.args.as_ref() else {
            return Ok(None);
        };
        if !matches!(&*call.func, ast::Expr::Name(name) if name.id == "enumerate")
            || !call.arguments.keywords.is_empty()
        {
            return Ok(None);
        }
        let Some(&slot) = self.names.get(name.id.as_str()) else {
            return Ok(None);
        };
        let ty = self
            .narrowed
            .get(&slot)
            .copied()
            .unwrap_or(self.locals[slot]);
        if self.types.element(ty).is_none() {
            return Ok(None);
        }
        Ok(Some(Iteration::Container {
            iterable: self.expr(source)?,
            items: false,
            start: Some(Expr {
                ty: Type::Int,
                kind: ExprKind::Int("0".into()),
            }),
        }))
    }

    /// Shares iteration snapshots and cleanup between statements and eager displays.
    /// The callback runs with the target definitely assigned for one iteration.
    fn lower_iteration(
        &mut self,
        target: &ast::Expr,
        input: Iteration,
        orelse: &[ast::Stmt],
        lower_body: impl FnOnce(&mut Self) -> Result<Vec<Statement>, String>,
    ) -> Result<(Vec<Statement>, bool), String> {
        self.lower_iteration_after(target, input, orelse, Vec::new(), lower_body)
    }

    /// A consumer can evaluate its remaining arguments after acquiring an
    /// iterator, preserving `sum(map(int, values), start())` mutation checks.
    /// Supplying extra statements requires already validated range arguments.
    fn lower_iteration_after(
        &mut self,
        target: &ast::Expr,
        input: Iteration,
        orelse: &[ast::Stmt],
        before_iteration: Vec<Statement>,
        lower_body: impl FnOnce(&mut Self) -> Result<Vec<Statement>, String>,
    ) -> Result<(Vec<Statement>, bool), String> {
        if let Iteration::Prepared(input) = input {
            return self.lower_prepared_iteration(
                target,
                input,
                orelse,
                before_iteration,
                lower_body,
            );
        }
        if let Iteration::Range(args) = input {
            let ast::Expr::Name(name) = target else {
                return Err(error(self.source, target, "range target must be a name"));
            };
            let target = self.declare(name.id.as_str(), Type::Int, target)?;
            self.narrowed.remove_mut(&target);
            let facts = self.narrowed.clone();
            let before = self.assigned.clone();
            self.assigned.insert(target);
            self.loop_breaks.push(None);
            let body = lower_body(self)?;
            let breaks = self.loop_breaks.pop().unwrap();
            self.assigned = before;
            self.narrowed = facts;
            let (mut orelse, falls) = self.loop_else(orelse, breaks)?;
            let cursor = self.hidden(Type::Int);
            let stop = self.hidden(Type::Int);
            let step = self.hidden(Type::Int);
            let clear = vec![cursor, stop, step];
            if !orelse.is_empty() {
                orelse = clear
                    .iter()
                    .copied()
                    .map(Statement::Clear)
                    .chain(orelse)
                    .collect();
            }
            let mut output = before_iteration;
            output.push(Statement::For {
                target,
                cursor,
                stop,
                step,
                args,
                body,
                orelse,
            });
            return Ok((
                vec![Statement::Scope {
                    body: output,
                    clear,
                }],
                falls,
            ));
        }
        let Iteration::Container {
            iterable,
            items,
            start,
        } = input
        else {
            unreachable!("range iteration returned above")
        };
        if !items
            && start.is_none()
            && let Some(element) = self.types.cursor_element(iterable.ty)
        {
            let cursor = self.prepare_cursor(iterable, element);
            return self.lower_prepared_iteration(
                target,
                cursor,
                orelse,
                before_iteration,
                lower_body,
            );
        }
        let iterable = if items {
            iterable
        } else {
            self.json_iteration(iterable)
        };
        if matches!(
            iterable.ty,
            Type::TextIO | Type::StringIO | Type::BinaryIO | Type::BytesIO | Type::CsvReader
        ) {
            let input = if iterable.ty == Type::CsvReader {
                self.prepare_csv_reader(iterable)
            } else {
                self.prepare_file_lines(iterable)
            };
            return self.lower_prepared_iteration(
                target,
                input,
                orelse,
                before_iteration,
                lower_body,
            );
        }
        let mapping = self.types.mapping(iterable.ty);
        let set_element = self.types.readable_set_element(iterable.ty);
        let string = iterable.ty == Type::Str;
        let bytes = iterable.ty == Type::Bytes;
        let element = self
            .types
            .element(iterable.ty)
            .or_else(|| self.types.var_tuple_element(iterable.ty))
            .or(set_element)
            .or_else(|| self.types.deque_element(iterable.ty))
            .or(string.then_some(Type::Str))
            .or(bytes.then_some(Type::Int))
            .or_else(|| {
                mapping.map(|(key, value)| {
                    if items {
                        self.types.tuple(vec![key, value])
                    } else {
                        key
                    }
                })
            })
            .ok_or_else(|| {
                error(
                    self.source,
                    target,
                    "iteration requires a concrete list, dictionary, set, string, bytes, or range",
                )
            })?;
        let captured = self.hidden(iterable.ty);
        let cursor = if mapping.is_none()
            && set_element.is_none()
            && self.types.deque_element(iterable.ty).is_none()
            && !string
            && !bytes
        {
            Some(self.hidden(Type::Int))
        } else {
            None
        };
        let mut output = vec![Statement::Assign(captured, iterable)];
        let zero_based = cursor.is_some()
            && matches!(&start, Some(Expr { kind: ExprKind::Int(value), .. }) if value == "0");
        let counter = start.filter(|_| !zero_based).map(|start| {
            let slot = self.hidden(Type::Int);
            output.push(Statement::Assign(slot, start));
            slot
        });
        let mut body = Vec::new();
        let mut owned_items = Vec::new();
        let mut cleanup = vec![captured];
        let (item, test) = if let Some(cursor) = cursor {
            output.push(Statement::Assign(
                cursor,
                Expr {
                    ty: Type::Int,
                    kind: ExprKind::Int("0".into()),
                },
            ));
            cleanup.push(cursor);
            (
                Expr {
                    ty: element,
                    kind: ExprKind::Builtin(
                        if matches!(self.locals[captured], Type::VarTuple(_)) {
                            Builtin::TupleIndex
                        } else if matches!(self.locals[captured], Type::List(_)) {
                            Builtin::SequenceGetInBounds
                        } else {
                            Builtin::ListGet
                        },
                        vec![self.local(captured), self.local(cursor)],
                    ),
                },
                Expr {
                    ty: Type::Bool,
                    kind: ExprKind::Compare(
                        vec![
                            self.local(cursor),
                            Expr {
                                ty: Type::Int,
                                kind: ExprKind::Builtin(Builtin::Len, vec![self.local(captured)]),
                            },
                        ],
                        vec![Compare::Less],
                    ),
                },
            )
        } else {
            let (key_type, value_type) = mapping.unwrap_or((element, Type::None));
            let iterator = self.hidden(Type::Iterator);
            // enumerate evaluates its start before obtaining the actual iterator.
            // The earlier capture preserves the receiver through those effects.
            output.push(Statement::Assign(
                iterator,
                Expr {
                    ty: Type::Iterator,
                    kind: ExprKind::Builtin(
                        if string {
                            Builtin::StringIter
                        } else if bytes {
                            Builtin::BytesIter
                        } else if self.types.deque_element(self.locals[captured]).is_some() {
                            Builtin::DequeIter { reverse: false }
                        } else if set_element.is_some() {
                            Builtin::SetIter
                        } else {
                            Builtin::DictIter(items)
                        },
                        vec![self.local(captured)],
                    ),
                },
            ));
            cleanup.push(iterator);
            let key = self.hidden(key_type);
            let value = items.then(|| self.hidden(value_type));
            let ready = self.hidden(Type::Bool);
            body.push(Statement::IterNext {
                iterator,
                key,
                value,
                ready,
            });
            body.push(Statement::If(
                Expr {
                    ty: Type::Bool,
                    kind: ExprKind::Not(Box::new(self.local(ready))),
                },
                vec![Statement::Exhaust],
                Vec::new(),
            ));
            owned_items.push(key);
            owned_items.extend(value);
            let item = if let Some(value) = value {
                Expr {
                    ty: element,
                    kind: ExprKind::Tuple(vec![self.local(key), self.local(value)]),
                }
            } else {
                self.local(key)
            };
            (
                item,
                Expr {
                    ty: Type::Bool,
                    kind: ExprKind::Bool(true),
                },
            )
        };
        let mut facts = self.narrowed.clone();
        let before = self.assigned.clone();
        let bindings = if zero_based {
            let [index, target] = pattern_elements(target).unwrap() else {
                unreachable!()
            };
            vec![
                self.assign_value(index, self.local(cursor.unwrap()))?,
                self.assign_value(target, item)?,
            ]
        } else {
            let item = if let Some(counter) = counter {
                Expr {
                    ty: self.types.tuple(vec![Type::Int, element]),
                    kind: ExprKind::Tuple(vec![self.local(counter), item]),
                }
            } else {
                item
            };
            self.bind_pattern(target, item)?
        };
        fn remove_target_facts(
            target: &ast::Expr,
            names: &HashMap<String, usize>,
            facts: &mut assignments::Narrowed,
        ) {
            if let ast::Expr::Name(name) = target {
                if let Some(slot) = names.get(name.id.as_str()) {
                    facts.remove_mut(slot);
                }
            } else if let ast::Expr::Starred(starred) = target {
                remove_target_facts(&starred.value, names, facts);
            } else if let Some(elements) = pattern_elements(target) {
                for element in elements {
                    remove_target_facts(element, names, facts);
                }
            }
            for name in narrowing::expression_assignments(target) {
                if let Some(slot) = names.get(&name) {
                    facts.remove_mut(slot);
                }
            }
        }
        remove_target_facts(target, &self.names, &mut facts);
        body.extend(bindings);
        cleanup.extend(&owned_items);
        body.extend(owned_items.into_iter().map(Statement::Clear));
        // Advance before entering the body so continue and target rebinding do
        // not alter progression. Length is read anew on every loop condition.
        for slot in cursor.into_iter().chain(counter) {
            body.push(Statement::Assign(
                slot,
                Expr {
                    ty: Type::Int,
                    kind: ExprKind::Binary(
                        Binary::Add,
                        Box::new(self.local(slot)),
                        Box::new(Expr {
                            ty: Type::Int,
                            kind: ExprKind::Int("1".into()),
                        }),
                    ),
                },
            ));
        }
        self.loop_breaks.push(None);
        body.extend(lower_body(self)?);
        let breaks = self.loop_breaks.pop().unwrap();
        self.assigned = before;
        self.narrowed = facts;
        let (mut orelse, falls) = self.loop_else(orelse, breaks)?;
        cleanup.extend(counter);
        if !orelse.is_empty() {
            // CPython drops the exhausted iterator before entering else. A
            // temporary container may own buffered files whose drop flushes
            // writes that the else suite can observe. Scope cleanup remains
            // idempotent after these private roots have been cleared.
            orelse = cleanup
                .iter()
                .copied()
                .map(Statement::Clear)
                .chain(orelse)
                .collect();
        }
        output.extend(before_iteration);
        output.push(Statement::While(test, body, orelse));
        Ok((
            vec![Statement::Scope {
                body: output,
                clear: cleanup,
            }],
            falls,
        ))
    }

    // A transfer inside this suite can target only the innermost enclosing
    // loop. Temporarily isolate its exits when a handler or finally changes them.
    fn take_breaks(&mut self) -> Option<LocalState> {
        self.loop_breaks.last_mut().and_then(Option::take)
    }

    fn include_breaks(&mut self, previous: Option<LocalState>) {
        if let Some(mut previous) = previous {
            let current = self
                .loop_breaks
                .last_mut()
                .expect("break targets an active loop");
            if let Some(current) = current.as_ref() {
                previous.retain_common(&current.assigned, &current.narrowed);
            }
            *current = Some(previous);
        }
    }

    /// The else suite is outside this loop, so its transfers target an outer loop.
    /// Only normal completion enters else; break states join after it.
    fn loop_else(
        &mut self,
        orelse: &[ast::Stmt],
        mut breaks: Option<LocalState>,
    ) -> Result<(Vec<Statement>, bool), String> {
        let (orelse, falls) = self.block(orelse)?;
        if falls {
            self.merge_exit(&mut breaks);
        }
        let falls = breaks.is_some();
        if let Some(exit) = breaks {
            self.restore_state(exit);
        }
        Ok((orelse, falls))
    }

    /// Checks dead syntax too, but merges definite assignment only from reachable exits.
    fn block(&mut self, statements: &[ast::Stmt]) -> Result<(Vec<Statement>, bool), String> {
        if let Some(reason) = statements.first().and_then(|first| {
            self.graph
                .entry_profile
                .excluded(self.module, first.start())
        }) {
            let message = Expr {
                ty: Type::Str,
                kind: ExprKind::Str(reason.to_owned()),
            };
            let exception = Expr {
                ty: Type::Exception,
                kind: ExprKind::Exception {
                    kind: exception_kind("ValueError").unwrap(),
                    message: Some(Box::new(message)),
                },
            };
            return Ok((vec![Statement::Raise(Some(exception))], false));
        }

        if self.block_depth == 0 && !self.module_scope {
            if self.graph.python_optimization() > 0 && !self.interactive_names {
                let (omitted, live) = unbound_locals::omitted_assert_bindings(statements);
                self.omitted_assert_locals = omitted;
                self.omitted_assert_free
                    .retain(|name, _| !live.contains(name));
            }
            self.local_iter = narrowing::assigned_names(statements).contains("iter");
            self.nullable_assignments = assignments::nullable_assignments(self, statements);
            let mut names: Vec<_> =
                unbound_locals::candidates(statements, self.interactive_names, &|name| {
                    self.mapping_class_import(name)
                })
                .into_iter()
                .collect();
            names.sort_unstable();
            for name in names {
                // A later assignment shadows the imported ABC throughout this function.
                if self.mapping_class_import(&name) && !self.names.contains_key(&name) {
                    self.declare(&name, Type::BuiltinClass, &statements[0])?;
                }
                // Arguments enter already bound; deletion and nonordinary stores were excluded.
                if self
                    .names
                    .get(&name)
                    .is_some_and(|slot| self.assigned.contains(slot))
                {
                    continue;
                }
                let flag = self.hidden(Type::Bool);
                self.bound_flags.insert(name, flag);
            }
        }
        self.block_depth += 1;
        let mut body = Vec::new();
        let mut falls_through = true;
        let reachable = self.reachable;
        let enclosing_sum = self.zip_sum_rhs;
        let mut previous_sum = false;
        let mut length_guard = None;
        for (index, statement) in statements.iter().enumerate() {
            if let ast::Stmt::Assign(assignment) = statement
                && let [ast::Expr::Name(name)] = assignment.targets.as_slice()
                && !self.names.contains_key(name.id.as_str())
            {
                let later = &statements[index + 1..];
                let empty_list =
                    matches!(&*assignment.value, ast::Expr::List(list) if list.elts.is_empty());
                let empty_dict =
                    matches!(&*assignment.value, ast::Expr::Dict(dict) if dict.items.is_empty());
                let empty_set = matches!(&*assignment.value, ast::Expr::Call(call) if matches!(&*call.func, ast::Expr::Name(value) if value.id == "set" && !self.names.contains_key("set")) && self.resolve(&call.func).is_none() && call.arguments.args.is_empty() && call.arguments.keywords.is_empty());
                let returned = || {
                    later.iter().any(|statement| matches!(statement, ast::Stmt::Return(ret) if matches!(ret.value.as_deref(), Some(ast::Expr::Name(value)) if value.id == name.id)))
                };
                let context = if (empty_list && self.types.element(self.returns).is_some()
                    || empty_dict && self.types.mapping(self.returns).is_some())
                    && returned()
                {
                    Some(self.returns)
                } else if empty_list && later.iter().any(|s| matches!(s, ast::Stmt::Assign(a)
                    if matches!(&*a.value, ast::Expr::Name(value) if value.id == name.id)
                    && a.targets.iter().any(|target| matches!(target, ast::Expr::Subscript(sub)
                        if matches!(&*sub.slice, ast::Expr::Slice(_)) && self.assignment_context(target) == Some(self.types.list(documents::ANY)))))) {
                    Some(self.types.list(documents::ANY))
                } else if (empty_list || empty_set)
                    && let Some(value) = inference::first_insertion(name.id.as_str(), if empty_list { "append" } else { "add" }, later)
                {
                    let element = match value {
                        ast::Expr::Call(call)
                            if let Some(class) = self.class_reference(&call.func) =>
                        {
                            Some(Type::Class(class))
                        }
                        _ => self.graph.session.scalar_type(self.module, value),
                    };
                    element.map(|ty| if empty_list { self.types.list(ty) } else { self.types.set(ty) })
                } else {
                    None
                };
                if let Some(ty) = context {
                    self.declare(name.id.as_str(), ty, name)?;
                }
            }
            self.reachable = reachable && falls_through;
            let sum = zip_sums::rhs(statement);
            self.zip_sum_rhs = sum.filter(|_| {
                self.inference.is_none()
                    && (previous_sum || statements.get(index + 1).and_then(zip_sums::rhs).is_some())
            });
            previous_sum = sum.is_some();
            let tuple = length_guard.and_then(|guard| self.guarded_tuple(statement, guard));
            let enclosing_tuple = std::mem::replace(&mut self.fixed_tuple, tuple);
            let (mut lowered, continues) = self.statement(statement)?;
            self.fixed_tuple = enclosing_tuple;
            length_guard = self
                .list_length_guard(statement)
                .or_else(|| length_guard.filter(|guard| fixed_tuples::preserves(*guard, &lowered)));
            self.remember_regex_assignment(statement, &mut lowered, &statements[index + 1..]);
            if let Some(flag) =
                unbound_locals::assigned(statement).and_then(|n| self.bound_flags.get(n))
            {
                lowered.push(Statement::Assign(
                    *flag,
                    Expr {
                        ty: Type::Bool,
                        kind: ExprKind::Bool(true),
                    },
                ));
            }
            body.extend(lowered);
            falls_through &= continues;
        }
        self.reachable = reachable;
        self.zip_sum_rhs = enclosing_sum;
        self.block_depth -= 1;
        Ok((body, falls_through))
    }

    fn statement(&mut self, statement: &ast::Stmt) -> Result<(Vec<Statement>, bool), String> {
        if let ast::Stmt::ImportFrom(import) = statement
            && let Some(bindings) = self.local_sys_import(import)?
        {
            return Ok((bindings, true));
        }
        if self.module_scope && local_functions::type_variable(self.graph, self.module, statement) {
            return Ok((Vec::new(), true));
        }
        if self.classes.parsers.declaration(self.module, statement) {
            return Ok((Vec::new(), true));
        }
        let result = match statement {
            ast::Stmt::ClassDef(class) if !self.module_scope => {
                self.define_local_class(class)?;
                None
            }
            ast::Stmt::FunctionDef(function) => self.define_local_function(function)?,
            ast::Stmt::Assign(assign) => {
                return Ok((self.assignment_statement(assign)?, true));
            }
            ast::Stmt::AnnAssign(assign) => {
                let ast::Expr::Name(name) = &*assign.target else {
                    return Err(error(self.source, statement, "only names may be annotated"));
                };
                let ty = annotation_in_class(
                    &assign.annotation,
                    self.graph,
                    self.module,
                    &self.classes.declarations,
                    self.types,
                    self.class,
                )?;
                // A new outermost annotation dominates every subsequent read.
                // Evaluate its RHS with the old binding; closures and nested
                // suites keep their fixed slots across calls and control flow.
                if !self.module_scope
                    && self.block_depth == 1
                    && self.local_functions.is_empty()
                    && let Some(&old) = self.names.get(name.id.as_str())
                    && self.locals[old] != ty
                    && let Some(rhs) = &assign.value
                {
                    let value = self.expr_expected(rhs, ty)?;
                    let slot = self.hidden(ty);
                    self.names.insert(name.id.to_string(), slot);
                    self.assigned_value(slot, &value);
                    self.assigned.remove(&old);
                    self.narrowed.remove_mut(&old);
                    return Ok((
                        vec![Statement::Assign(slot, value), Statement::Clear(old)],
                        true,
                    ));
                }
                let slot = self.declare(name.id.as_str(), ty, statement)?;
                if let Some(value) = &assign.value {
                    let value = self.expr_expected(value, ty)?;
                    self.assigned_value(slot, &value);
                    Some(Statement::Assign(slot, value))
                } else {
                    None
                }
            }
            ast::Stmt::AugAssign(assign) => {
                if let ast::Expr::Subscript(target) = &*assign.target {
                    let list = self.expr(&target.value)?;
                    let list = self.json_mutation_receiver(list, target)?;
                    let (key, element) = if let Some(mapping) = self.types.mapping(list.ty) {
                        mapping
                    } else {
                        let element = self.types.indexed_element(list.ty).ok_or_else(|| {
                            error(
                                self.source,
                                target,
                                "augmented item assignment requires a typed list or dictionary",
                            )
                        })?;
                        (Type::Int, element)
                    };
                    if !matches!(element, Type::Int | Type::Float) {
                        return Err(error(
                            self.source,
                            target,
                            "augmented items require int or float storage",
                        ));
                    }
                    let index = if matches!(list.ty, Type::Deque(_)) {
                        self.integer_index(&target.slice)?
                    } else {
                        self.expr_expected(&target.slice, key)?
                    };
                    let value = self.expr(&assign.value)?;
                    let value = Self::integer_addend(assign.op, value, element);
                    let (op, result) = self.binary_op(assign.op, element, value.ty, statement)?;
                    self.expect(element, result, statement)?;
                    Some(Statement::AugmentItem(list, index, op, value))
                } else if let ast::Expr::Attribute(target) = &*assign.target {
                    let receiver = Self::weak_target(self.expr(&target.value)?);
                    if let Some(class) = self.writable_property(receiver.ty, target) {
                        let slot = self.hidden(receiver.ty);
                        let old =
                            self.class_attribute(target, class, Some(self.local(slot)), None)?;
                        let rhs = self.expr(&assign.value)?;
                        let combined = self.binary_expr(assign.op, old, rhs, statement)?;
                        if self.types.element(combined.ty).is_some() {
                            return Err(error(
                                self.source,
                                target,
                                "in-place list operations are unsupported",
                            ));
                        }
                        let put = self.set_property(class, target, self.local(slot), combined)?;
                        return Ok((
                            vec![Statement::Scope {
                                body: vec![Statement::Assign(slot, receiver), put],
                                clear: vec![slot],
                            }],
                            true,
                        ));
                    }
                    let (receiver, field, ty) = self.writable_field_value(target, receiver)?;
                    if !matches!(ty, Type::Int | Type::Float) {
                        return Err(error(
                            self.source,
                            target,
                            "augmented fields require int or float storage",
                        ));
                    }
                    let value = self.expr(&assign.value)?;
                    let value = Self::integer_addend(assign.op, value, ty);
                    let (op, result) = self.binary_op(assign.op, ty, value.ty, statement)?;
                    self.expect(ty, result, statement)?;
                    // AugmentField keeps its old value implicit. Expose the
                    // checked integer conversion in the RHS so effect analysis
                    // preserves OverflowError through callers.
                    let value = if ty == Type::Float && value.ty == Type::Int {
                        Expr {
                            ty: Type::Float,
                            kind: ExprKind::Builtin(Builtin::Float, vec![value]),
                        }
                    } else {
                        value
                    };
                    Some(Statement::AugmentField(receiver, field, op, value))
                } else if let ast::Expr::Name(_) = &*assign.target {
                    let left = self.expr(&assign.target)?;
                    let right = self.expr(&assign.value)?;
                    let value = if self.types.is_counter(left.ty) {
                        self.counter_math(assign.op, left, right, true, statement)?
                    } else if assign.op == ast::Operator::Div
                        && (left.ty.is_path() || right.ty.is_path())
                    {
                        self.path_join(left, right, statement)?
                    } else {
                        self.binary_expr(assign.op, left, right, statement)?
                    };
                    if self.types.element(value.ty).is_some() {
                        return Err(error(
                            self.source,
                            statement,
                            "in-place list operations are unsupported",
                        ));
                    }
                    Some(self.assign(&assign.target, value)?)
                } else {
                    return Err(error(
                        self.source,
                        statement,
                        "unsupported assignment target",
                    ));
                }
            }
            ast::Stmt::Expr(expression) => {
                if matches!(&*expression.value, ast::Expr::EllipsisLiteral(_)) {
                    return Ok((Vec::new(), true));
                }
                if let ast::Expr::Yield(yielded) = &*expression.value
                    && let Some(element) = self.generator_item
                    && let Some(value) = &yielded.value
                {
                    let item = self.expr_expected(value, element)?;
                    let resume =
                        contexts::op(ir::ProcessOperation::GeneratorYield, Type::Bool, vec![item]);
                    return Ok((
                        vec![Statement::If(
                            resume,
                            Vec::new(),
                            vec![Statement::Return(Expr {
                                ty: Type::None,
                                kind: ExprKind::None,
                            })],
                        )],
                        true,
                    ));
                }
                if let ast::Expr::Yield(yielded) = &*expression.value
                    && yielded.value.is_none()
                    && self.context_method
                {
                    return Ok((
                        vec![Statement::Expr(contexts::op(
                            ir::ProcessOperation::ContextYield,
                            Type::None,
                            Vec::new(),
                        ))],
                        true,
                    ));
                }
                if let ast::Expr::Call(call) = &*expression.value
                    && self.resolve(&call.func)
                        == Some(Binding::Intrinsic(Intrinsic::SubprocessRun))
                    && call
                        .arguments
                        .keywords
                        .iter()
                        .any(|kw| kw.arg.as_ref().is_some_and(|name| name == "stdout"))
                {
                    Some(Statement::Expr(self.process_to_stderr(call)?))
                } else if let ast::Expr::Call(call) = &*expression.value
                    && self.resolve(&call.func) == Some(Binding::Intrinsic(Intrinsic::AssertNever))
                {
                    return Ok((vec![self.assert_never(call, false)?], false));
                } else if let ast::Expr::Call(call) = &*expression.value
                    && self.resolve(&call.func) == Some(Binding::Intrinsic(Intrinsic::SysExit))
                {
                    return Ok((vec![Statement::Expr(self.expr(&expression.value)?)], false));
                } else {
                    Some(Statement::Expr(self.expr(&expression.value)?))
                }
            }
            ast::Stmt::Delete(delete) => {
                let mut statements = Vec::new();
                for target in &delete.targets {
                    if matches!(target, ast::Expr::Attribute(_))
                        && self.resolve(target) == Some(Binding::Intrinsic(Intrinsic::Stdout))
                    {
                        statements.push(Statement::Expr(Expr {
                            ty: Type::None,
                            kind: ExprKind::Builtin(Builtin::SysStdoutDelete, vec![]),
                        }));
                        continue;
                    }
                    if matches!(target, ast::Expr::Attribute(_))
                        && self.resolve(target) == Some(Binding::Intrinsic(Intrinsic::BuiltinsLast))
                    {
                        statements.push(Statement::Expr(Expr {
                            ty: Type::None,
                            kind: ExprKind::Builtin(Builtin::BuiltinsDeleteLast, vec![]),
                        }));
                        continue;
                    }
                    if let ast::Expr::Name(name) = target
                        && let Some(&slot) = self.names.get(name.id.as_str())
                        && matches!(
                            self.locals[slot],
                            Type::Cursor(_) | Type::SimpleNamespace(_)
                        )
                    {
                        if !self.assigned.contains(&slot) {
                            return Err(error(
                                self.source,
                                target,
                                "deletion requires a bound local",
                            ));
                        }
                        self.assigned.remove(&slot);
                        self.narrowed.remove_mut(&slot);
                        statements.push(Statement::Clear(slot));
                        continue;
                    }
                    if let ast::Expr::Attribute(attr) = target {
                        let receiver = self.expr(&attr.value)?;
                        if matches!(receiver.ty, Type::Popen(..)) && attr.attr.as_str() == "poll" {
                            statements.push(Statement::Expr(Expr {
                                ty: Type::None,
                                kind: ExprKind::Builtin(
                                    Builtin::Process(ir::ProcessOperation::PopenDeletePoll),
                                    vec![receiver],
                                ),
                            }));
                            continue;
                        }
                        if let Some(class) =
                            self.mutable_instance_data(receiver.ty, attr.attr.as_str())
                        {
                            statements.push(self.delete_instance_data(class, attr, receiver)?);
                            continue;
                        }
                        if matches!(receiver.ty, Type::SimpleNamespace(_)) {
                            let name = self.namespace_attr_name(attr)?;
                            statements.push(Statement::Expr(Expr {
                                ty: Type::None,
                                kind: ExprKind::Builtin(
                                    Builtin::NamespaceDelete,
                                    vec![receiver, name],
                                ),
                            }));
                            continue;
                        }
                    }
                    let ast::Expr::Subscript(target) = target else {
                        return Err(error(
                            self.source,
                            target,
                            "deletion requires a dictionary subscript",
                        ));
                    };
                    let receiver = self.expr(&target.value)?;
                    if matches!(receiver.ty, Type::Deque(_)) {
                        let index = self.integer_index(&target.slice)?;
                        statements.push(Statement::Expr(Expr {
                            ty: Type::None,
                            kind: ExprKind::Builtin(
                                Builtin::DequeDelete { method: false },
                                vec![receiver, index],
                            ),
                        }));
                        continue;
                    }
                    let (key, _) = self.types.mapping(receiver.ty).ok_or_else(|| {
                        error(
                            self.source,
                            target,
                            "deletion requires a dictionary subscript",
                        )
                    })?;
                    let key = self.expr_expected(&target.slice, key)?;
                    self.graph
                        .checked_dict_deletion(self.module, target.range());
                    statements.push(Statement::Expr(Expr {
                        ty: Type::None,
                        kind: ExprKind::Builtin(Builtin::DictDelete, vec![receiver, key]),
                    }));
                }
                return Ok((statements, true));
            }
            ast::Stmt::If(branch) => return self.if_chain(branch),
            ast::Stmt::Match(branch) => return self.match_statement(branch),
            ast::Stmt::While(loop_) => {
                self.invalidate_loop_facts(&loop_.body, None)?;
                for name in narrowing::expression_assignments(&loop_.test) {
                    if let Some(slot) = self.names.get(&name) {
                        self.narrowed.remove_mut(slot);
                    }
                }
                let test = self.condition(&loop_.test)?;
                let facts = self.narrowed.clone();
                let before = self.assigned.clone();
                self.apply_facts(&loop_.test, true);
                self.loop_breaks.push(None);
                let (body, _) = self.block(&loop_.body)?;
                let breaks = self.loop_breaks.pop().unwrap();
                self.assigned = before;
                self.narrowed = facts;
                self.apply_facts(&loop_.test, false);
                let (orelse, falls) = self.loop_else(&loop_.orelse, breaks)?;
                return Ok((vec![Statement::While(test, body, orelse)], falls));
            }
            ast::Stmt::For(loop_) if !loop_.is_async => {
                return self.iteration(loop_);
            }
            ast::Stmt::Return(return_) if !self.module_scope => {
                let value = if let Some(value) = &return_.value {
                    if let Some(repr) = self.exception_formatter {
                        let value = self.expr(value)?;
                        if value.ty == Type::ObjectValue {
                            Expr {
                                ty: Type::Str,
                                kind: ExprKind::Builtin(
                                    Builtin::ExceptionFormatterResult(repr),
                                    vec![value],
                                ),
                            }
                        } else {
                            self.coerce(value, Type::Str, statement)?
                        }
                    } else if self.inferred_returns.is_some() {
                        self.expr(value)?
                    } else {
                        self.expr_expected(value, self.returns)?
                    }
                } else {
                    Expr {
                        ty: Type::None,
                        kind: ExprKind::None,
                    }
                };
                let value = if let Some(types) = &mut self.inferred_returns {
                    types.push(value.ty);
                    value
                } else {
                    self.coerce(value, self.returns, statement)?
                };
                return Ok((vec![Statement::Return(value)], false));
            }
            ast::Stmt::Break(_) | ast::Stmt::Continue(_) if !self.loop_breaks.is_empty() => {
                if self.reachable && matches!(statement, ast::Stmt::Break(_)) {
                    let mut exits = self.take_breaks();
                    self.merge_exit(&mut exits);
                    *self.loop_breaks.last_mut().unwrap() = exits;
                }
                return Ok((
                    vec![if matches!(statement, ast::Stmt::Break(_)) {
                        Statement::Break
                    } else {
                        Statement::Continue
                    }],
                    false,
                ));
            }
            ast::Stmt::Assert(assertion) if self.graph.python_optimization() > 0 => {
                assertion_syntax::validate(self.source, assertion)?;
                None
            }
            ast::Stmt::Assert(assert) => {
                let test = self.condition(&assert.test)?;
                let successful = self.local_state();
                let result = if let Some(message) = &assert.msg {
                    let message = self.expr_expected(message, Type::Str)?;
                    Statement::If(
                        test,
                        Vec::new(),
                        vec![Statement::Raise(Some(Expr {
                            ty: Type::Exception,
                            kind: ExprKind::Exception {
                                kind: 9,
                                message: Some(Box::new(message)),
                            },
                        }))],
                    )
                } else {
                    Statement::Assert(test)
                };
                self.restore_state(successful);
                self.apply_facts(&assert.test, true);
                Some(result)
            }
            ast::Stmt::Try(try_) if !try_.is_star => {
                return self.try_statement(try_);
            }
            ast::Stmt::With(with_) if !with_.is_async => {
                if !with_.body.iter().any(ast::Stmt::is_function_def_stmt) {
                    return self.with_items(&with_.items, &with_.body);
                }
                let enclosing = self.local_functions.clone();
                self.collect_suite_definitions(&with_.body, false)?;
                let result = self.with_items(&with_.items, &with_.body);
                self.local_functions = enclosing;
                return result;
            }
            ast::Stmt::With(with_) if self.asynchronous => {
                return self.async_with(&with_.items, &with_.body);
            }
            ast::Stmt::Raise(raise) => {
                let mut value = raise
                    .exc
                    .as_deref()
                    .map(|value| self.raised_exception(value))
                    .transpose()?;
                if let Some(cause) = &raise.cause {
                    let raised = value.take().expect("raise from has an exception");
                    if let ExprKind::Builtin(
                        Builtin::Process(ir::ProcessOperation::ExceptionAt),
                        args,
                    ) = &raised.kind
                        && matches!(&args[1].kind, ExprKind::Int(_) | ExprKind::Negate(_))
                        && let ExprKind::Local(slot) = args[0].kind
                        && let Some(name) = cause.as_name_expr()
                        && self.names.get(name.id.as_str()) == Some(&slot)
                    {
                        let group = self.expr_expected(cause, Type::Exception)?;
                        let member = Expr {
                            ty: Type::Exception,
                            kind: ExprKind::Builtin(
                                Builtin::Process(ir::ProcessOperation::MemberCause),
                                vec![raised, group],
                            ),
                        };
                        let member = self.exception_origin(member, raise);
                        return Ok((vec![Statement::Raise(Some(member))], false));
                    }
                    if !matches!(
                        raised.kind,
                        ExprKind::Exception { .. }
                            | ExprKind::Builtin(
                                Builtin::NamedException { .. }
                                    | Builtin::ImportErrorNew { .. }
                                    | Builtin::ExceptionArgument(_),
                                _
                            )
                    ) && !matches!(&raised.kind,
                        ExprKind::Builtin(Builtin::Process(ir::ProcessOperation::ExceptionView),args)
                            if matches!(args[0].kind, ExprKind::Builtin(Builtin::NamedException { .. } | Builtin::ImportErrorNew { .. }, _)))
                    {
                        return Err(error(
                            self.source,
                            raise,
                            "explicit causes require a freshly constructed builtin exception",
                        ));
                    }
                    let expected = self.types.optional(Type::Exception);
                    let cause = if self.exception_category(cause).is_some() {
                        let cause = self.raised_exception(cause)?;
                        self.coerce(cause, expected, raise)?
                    } else {
                        self.expr_expected(cause, expected)?
                    };
                    value = Some(Expr {
                        ty: Type::Exception,
                        kind: ExprKind::Builtin(Builtin::ExceptionWithCause, vec![raised, cause]),
                    });
                }
                let value = value.map(|exception| self.exception_origin(exception, raise));
                return Ok((vec![Statement::Raise(value)], false));
            }
            ast::Stmt::Pass(_) => None,
            _ => {
                return Err(error(
                    self.source,
                    statement,
                    "unsupported statement in the Imre subset",
                ));
            }
        };
        Ok((result.into_iter().collect(), true))
    }

    /// Sealed files and direct temporary-directory scopes share normal finally
    /// transfers. An independent root keeps cleanup tied to the entered resource
    /// when the visible target is rebound or a later manager fails to enter.
    fn with_items(
        &mut self,
        items: &[ast::WithItem],
        body: &[ast::Stmt],
    ) -> Result<(Vec<Statement>, bool), String> {
        let Some((item, remaining)) = items.split_first() else {
            return self.block(body);
        };
        let temporary = if let ast::Expr::Call(call) = &item.context_expr
            && matches!(
                self.resolve(&call.func),
                Some(Binding::Intrinsic(Intrinsic::TemporaryDirectory))
            ) {
            Some(call)
        } else {
            None
        };
        let manager = if let Some(call) = temporary {
            self.temporary_directory(call)?
        } else {
            self.expr(&item.context_expr)?
        };
        if let Type::Class(class) = manager.ty
            && self.unittest_context(class)
        {
            return self.assert_raises_scope(item, remaining, body, class, manager);
        }
        if self.types.payload(manager.ty).unwrap_or(manager.ty) == Type::HandlerLock {
            return self.handler_lock_scope(item, remaining, body, manager);
        }
        if let Type::Class(class) = manager.ty
            && self.bundled_mock_class(class, "_DictionaryPatch")
        {
            return self.mock_dictionary_scope(item, remaining, body, class, manager);
        }
        if manager.ty == Type::GeneratorContext {
            return self.generator_scope(item, remaining, body, manager);
        }
        let stored = self.types.directory_value(manager.ty);
        let archive = manager.ty == Type::ZipFile;
        let token = matches!(manager.ty, Type::ContextToken(_));
        let socket = manager.ty == Type::Socket;
        let server = match manager.ty {
            Type::Class(class) if self.is_tcpserver(class) => Some(class),
            _ => None,
        };
        let popen = matches!(manager.ty, Type::Popen(..));
        let context_methods = if let Type::Class(class) = manager.ty {
            let owner = &self.classes.declarations.definitions[class];
            self.graph.modules[owner.definition.module]
                .closes_context(&owner.name)
                .then(|| {
                    let methods = &self.classes.methods[class];
                    (methods["__enter__"].id, methods["close"].id)
                })
        } else {
            None
        };
        if temporary.is_none()
            && stored.is_none()
            && !archive
            && !token
            && !socket
            && server.is_none()
            && !popen
            && context_methods.is_none()
            && !matches!(
                manager.ty,
                Type::TextIO | Type::StringIO | Type::BinaryIO | Type::BytesIO
            )
        {
            return Err(error(
                self.source,
                &item.context_expr,
                "with requires a supported file, socket, ZIP archive, or directory context",
            ));
        }
        let captured = self.hidden(manager.ty);
        let mut protected = Vec::new();
        if let Some(target) = &item.optional_vars {
            if !matches!(&**target, ast::Expr::Name(_)) {
                return Err(error(
                    self.source,
                    target.as_ref(),
                    "with targets must be local names",
                ));
            }
            let value = if let Some(ty) = stored {
                paths::operation(
                    ir::PathOperation::ContextEnter,
                    ty,
                    vec![self.local(captured)],
                )
            } else {
                self.local(captured)
            };
            protected.push(self.assign(target, value)?);
        }
        let (nested, falls) = self.with_items(remaining, body)?;
        protected.extend(nested);
        let mut statements = vec![Statement::Assign(captured, manager)];
        if let Some((enter, _)) = context_methods {
            self.calls.insert(enter);
            let receiver = self.local(captured);
            statements.push(Statement::Expr(Expr {
                ty: receiver.ty,
                kind: ExprKind::Call(enter, ir::CallArgs::positional(vec![receiver])),
            }));
        } else if temporary.is_none()
            && stored.is_none()
            && !archive
            && !token
            && !socket
            && server.is_none()
            && !popen
        {
            statements.push(Statement::Expr(Expr {
                ty: Type::None,
                kind: ExprKind::Builtin(Builtin::TextEnter, vec![self.local(captured)]),
            }));
        }
        let cleanup = if stored.is_some() {
            Builtin::Path(ir::PathOperation::ContextExit)
        } else if temporary.is_some() {
            Builtin::Path(ir::PathOperation::CleanupTemporaryDirectory)
        } else if popen {
            Builtin::Process(ir::ProcessOperation::PopenExit)
        } else if token {
            Builtin::ContextVar(ir::ContextOperation::ExitToken)
        } else if socket {
            Builtin::Socket(ir::SocketOp::Close)
        } else if archive {
            Builtin::ZipClose
        } else {
            Builtin::TextClose
        };
        let cleanup = if let Some(class) = server {
            self.tcpserver_context_exit(class, self.local(captured), &item.context_expr)?
        } else {
            let kind = if let Some((_, id)) = context_methods {
                self.calls.insert(id);
                ExprKind::Call(id, ir::CallArgs::positional(vec![self.local(captured)]))
            } else {
                ExprKind::Builtin(cleanup, vec![self.local(captured)])
            };
            Expr {
                ty: Type::None,
                kind,
            }
        };
        statements.push(Statement::Try {
            body: protected,
            handlers: Vec::new(),
            orelse: Vec::new(),
            finalbody: vec![Statement::Expr(cleanup)],
        });
        Ok((
            vec![Statement::Scope {
                body: statements,
                clear: vec![captured],
            }],
            falls,
        ))
    }

    /// Imported exception aliases resolve by identity; a spelling alone cannot
    /// turn an arbitrary first-party definition into a builtin category.
    /// Class-form raises construct a new instance, just like an explicit call.
    fn simple_exception(&mut self, call: &ast::ExprCall, kind: u32) -> Result<Expr, String> {
        if self.is_import_error(kind) {
            return self.import_error(call, kind, None);
        }
        if kind == 32 {
            return self.system_exit(call, false);
        }
        if matches!(kind, 0 | 18 | 25..=31 | 34) {
            return Err(error(
                self.source,
                call,
                "this exception category is catch-only",
            ));
        }
        if !call.arguments.keywords.is_empty() || call.arguments.args.len() > 1 {
            return Err(error(
                self.source,
                call,
                "builtin exceptions accept zero arguments or one typed argument",
            ));
        }
        let arg = call
            .arguments
            .args
            .first()
            .map(|v| self.expr(v))
            .transpose()?;
        let arg = match arg {
            Some(v) if matches!(v.ty, Type::StrLiteral(_)) => {
                Some(self.coerce(v, Type::Str, call)?)
            }
            other => other,
        };
        if arg.as_ref().is_some_and(|v| v.ty != Type::Str) {
            let v = arg.unwrap();
            if !matches!(
                v.ty,
                Type::Bool
                    | Type::Int
                    | Type::Float
                    | Type::Bytes
                    | Type::None
                    | Type::ListInt
                    | Type::List(_)
                    | Type::Tuple(_)
                    | Type::Dict(_)
                    | Type::Set(_)
            ) || !dataclass_values::can_format(v.ty, self.types, self.classes)
            {
                return Err(error(
                    self.source,
                    call,
                    "exception argument has no supported Python representation",
                ));
            }
            return Ok(Expr {
                ty: Type::Exception,
                kind: ExprKind::Builtin(Builtin::ExceptionArgument(kind), vec![v]),
            });
        }
        Ok(Expr {
            ty: Type::Exception,
            kind: ExprKind::Exception {
                kind,
                message: arg.map(Box::new),
            },
        })
    }

    fn exception_origin(&self, exception: Expr, site: &impl Ranged) -> Expr {
        if self.graph.command_startup.is_none() {
            return exception;
        }
        let filename = if self.graph.virtual_command && self.module == self.graph.entry {
            "<string>".to_owned()
        } else {
            self.graph
                .module_filename(self.module)
                .to_string_lossy()
                .into_owned()
        };
        let line = self.source[..usize::from(site.start())]
            .bytes()
            .filter(|&byte| byte == b'\n')
            .count() as u64
            + 1;
        Expr {
            ty: Type::Exception,
            kind: ExprKind::Builtin(
                Builtin::ExceptionOrigin(line),
                vec![exception, unpacking::string(filename)],
            ),
        }
    }

    fn raised_exception(&mut self, value: &ast::Expr) -> Result<Expr, String> {
        if let Some(kind) = self.exception_category(value) {
            if self.class_reference(value).is_some() {
                return Err(error(
                    self.source,
                    value,
                    "raise a constructed exception payload with its required arguments",
                ));
            }
            if self.is_import_error(kind) {
                return Ok(self.import_error_value(kind, Vec::new(), vec![], [None; 3]));
            }
            if kind == 32 {
                return Ok(Expr {
                    ty: Type::Exception,
                    kind: ExprKind::Builtin(
                        Builtin::SystemExitNew { omit_none: false },
                        Vec::new(),
                    ),
                });
            }
            if matches!(kind, 0 | 18 | 25..=31 | 34) {
                return Err(error(
                    self.source,
                    value,
                    "this exception category is catch-only",
                ));
            }
            Ok(Expr {
                ty: Type::Exception,
                kind: ExprKind::Exception {
                    kind,
                    message: None,
                },
            })
        } else {
            self.expr_expected(value, Type::Exception)
        }
    }

    fn exception_category(&self, expression: &ast::Expr) -> Option<u32> {
        if self.is_class_receiver(expression) {
            return None;
        }
        match self.resolve(expression) {
            Some(Binding::Definition(def))
                if self.classes.declarations.exceptions.contains_key(&def) =>
            {
                Some(self.classes.declarations.exceptions[&def])
            }
            Some(Binding::Intrinsic(Intrinsic::ShutilError)) => Some(64),
            Some(Binding::Intrinsic(Intrinsic::SameFileError)) => Some(65),
            Some(Binding::Intrinsic(Intrinsic::SpecialFileError)) => Some(66),
            Some(Binding::Intrinsic(Intrinsic::CancelledError)) => Some(33),
            Some(Binding::Intrinsic(Intrinsic::UnsupportedOperation)) => Some(19),
            Some(Binding::Intrinsic(Intrinsic::SubprocessError)) => Some(25),
            Some(Binding::Intrinsic(Intrinsic::CalledProcessError)) => Some(26),
            Some(Binding::Intrinsic(Intrinsic::TimeoutExpired)) => Some(27),
            Some(Binding::Intrinsic(Intrinsic::BadGzipFile)) => Some(28),
            Some(Binding::Intrinsic(Intrinsic::BadZipFile)) => Some(36),
            Some(Binding::Intrinsic(Intrinsic::CsvError)) => Some(67),
            Some(Binding::Intrinsic(Intrinsic::GaiError)) => Some(68),
            Some(Binding::Intrinsic(Intrinsic::BinasciiError)) => Some(69),
            Some(Binding::Intrinsic(Intrinsic::BinasciiIncomplete)) => Some(70),
            Some(Binding::Intrinsic(Intrinsic::ZlibError)) => Some(29),
            Some(Binding::Intrinsic(Intrinsic::TomlDecodeError)) => Some(30),
            Some(Binding::Intrinsic(Intrinsic::JsonDecodeError)) => Some(31),
            Some(Binding::Intrinsic(Intrinsic::UnidiffParseError)) => Some(52),
            Some(_) => None,
            None => match expression {
                ast::Expr::Name(name) => exception_kind(name.id.as_str()),
                _ => None,
            },
        }
    }

    /// A handler may run after any failing operation, before its assignment has
    /// committed. Only incoming definite assignments survive; writes invalidate
    /// incoming optional facts. Finally uses the same conservative entry for all
    /// normal, exceptional, return, and loop exits.
    fn try_statement(&mut self, try_: &ast::StmtTry) -> Result<(Vec<Statement>, bool), String> {
        let before = self.assigned.clone();
        let before_facts = self.narrowed.clone();
        let earlier_breaks = if try_.finalbody.is_empty() {
            None
        } else {
            self.take_breaks()
        };
        let (body, body_falls) = self.block(&try_.body)?;
        let reachable = self.reachable;
        self.reachable &= body_falls;
        let (orelse, else_falls) = self.block(&try_.orelse)?;
        self.reachable = reachable;
        let mut normal_exit = (body_falls && else_falls).then(|| self.local_state());

        let mut handler_assigned = before.clone();
        // Releasing a definitely bound cursor cannot raise. A handler can only
        // be reached before a trailing deletion of plain locals takes effect.
        let protected_prefix = if let [prefix @ .., ast::Stmt::Delete(last)] = try_.body.as_slice()
            && last.targets.iter().all(ast::Expr::is_name_expr)
        {
            prefix
        } else {
            try_.body.as_slice()
        };
        for name in narrowing::deleted_names(protected_prefix) {
            if let Some(slot) = self.names.get(&name) {
                handler_assigned.remove(slot);
            }
        }
        let mut handler_facts = before_facts.clone();
        for name in narrowing::assigned_names(&try_.body) {
            if let Some(slot) = self.names.get(&name) {
                handler_facts.remove_mut(slot);
            }
        }
        let mut handlers = Vec::new();
        for (index, handler) in try_.handlers.iter().enumerate() {
            let ast::ExceptHandler::ExceptHandler(handler) = handler;
            let kinds = if let Some(target) = &handler.type_ {
                let expressions = if let ast::Expr::Tuple(tuple) = &**target {
                    tuple.elts.as_slice()
                } else {
                    std::slice::from_ref(&**target)
                };
                if expressions.is_empty() {
                    return Err(error(
                        self.source,
                        target.as_ref(),
                        "an exception handler requires at least one builtin category",
                    ));
                }
                expressions
                    .iter()
                    .map(|target| {
                        if let Some(kind) = self
                            .exception_category(target)
                            .or_else(|| self.fixed_testcase_failure_kind(target))
                        {
                            return Ok(kind);
                        }
                        Err(error(
                            self.source,
                            target,
                            "exception handlers require fixed builtin categories",
                        ))
                    })
                    .collect::<Result<Vec<_>, _>>()?
            } else {
                if index + 1 != try_.handlers.len() {
                    return Err(error(self.source, handler, "a bare handler must be last"));
                }
                Vec::new()
            };
            self.assigned = handler_assigned.clone();
            self.narrowed = handler_facts.clone();
            let name = handler
                .name
                .as_ref()
                .map(|name| {
                    let slot = self.declare(name.as_str(), Type::Exception, handler)?;
                    self.assigned.insert(slot);
                    self.narrowed.remove_mut(&slot);
                    if let [kind] = kinds.as_slice()
                        && let Some((&def, _)) = self
                            .classes
                            .declarations
                            .exceptions
                            .iter()
                            .find(|(_, id)| *id == kind)
                        && let Some(&class) = self.classes.declarations.ids.get(&def)
                    {
                        self.narrowed.insert_mut(slot, Type::NamedException(class));
                    }
                    if !kinds.is_empty() && kinds.iter().all(|kind| self.is_import_error(*kind)) {
                        self.narrowed.insert_mut(slot, Type::ImportException);
                    }
                    if !kinds.is_empty() && kinds.iter().all(|kind| *kind == 27) {
                        self.narrowed.insert_mut(slot, Type::TimeoutExpired);
                    }
                    Ok::<_, String>(slot)
                })
                .transpose()?;
            let before_handler = name.and_then(|_| self.take_breaks());
            let capture_depth = self.captured_process_errors.len();
            if let (Some(slot), Some(target)) = (name, &handler.name)
                && !narrowing::assigned_names(&handler.body).contains(target.as_str())
            {
                let view =
                    processes::process_error_view(slot, &try_.body, &body, self.types, &kinds);
                self.captured_process_errors.push(view);
            }
            let (handler_body, falls) = self.block(&handler.body)?;
            self.captured_process_errors.truncate(capture_depth);
            if let Some(name) = name {
                self.assigned.remove(&name);
                self.narrowed.remove_mut(&name);
                // The exception target is deleted even when a handler breaks.
                if let Some(Some(exits)) = self.loop_breaks.last_mut() {
                    exits.assigned.remove(&name);
                    exits.narrowed.remove_mut(&name);
                }
                self.include_breaks(before_handler);
            }
            if falls {
                self.merge_exit(&mut normal_exit);
            }
            handlers.push(ir::ExceptHandler {
                kinds,
                name,
                body: handler_body,
            });
        }

        let has_normal_exit = normal_exit.is_some();
        let (mut normal_assigned, normal_facts) = normal_exit
            .map(|exit| (exit.assigned, exit.narrowed))
            .unwrap_or_default();

        self.assigned = before;
        self.narrowed = before_facts;
        let mut protected = try_.body.to_vec();
        protected.extend(try_.orelse.iter().cloned());
        for handler in &try_.handlers {
            let ast::ExceptHandler::ExceptHandler(handler) = handler;
            protected.extend(handler.body.iter().cloned());
            if let Some(name) = &handler.name
                && let Some(slot) = self.names.get(name.as_str())
            {
                self.assigned.remove(slot);
                self.narrowed.remove_mut(slot);
            }
        }
        for name in narrowing::deleted_names(&protected) {
            if let Some(slot) = self.names.get(&name) {
                self.assigned.remove(slot);
            }
        }
        for name in narrowing::assigned_names(&protected) {
            if let Some(slot) = self.names.get(&name) {
                self.narrowed.remove_mut(slot);
            }
        }
        // Pending breaks must pass through finally. Its own transfers replace
        // them; successful completion adds its assignments and invalidates any
        // optional facts it may have changed, rather than recording an early
        // "broken" flag before the final completion is known.
        let pending_breaks = if try_.finalbody.is_empty() {
            None
        } else {
            self.take_breaks()
        };
        let outer_finally = self.in_finally;
        self.in_finally = true;
        let (finalbody, final_falls) = self.block(&try_.finalbody)?;
        self.in_finally = outer_finally;
        if final_falls && let Some(mut pending) = pending_breaks {
            let final_state = self.local_state();
            let written = narrowing::assigned_names(&try_.finalbody)
                .into_iter()
                .filter_map(|name| self.names.get(&name).copied())
                .collect::<HashSet<_>>();
            let deleted = narrowing::deleted_names(&try_.finalbody)
                .into_iter()
                .filter_map(|name| self.names.get(&name).copied())
                .collect::<HashSet<_>>();
            for slot in &deleted {
                pending.assigned.remove(slot);
            }
            for slot in written.iter().chain(&deleted) {
                pending.narrowed.remove_mut(slot);
            }
            pending.assigned.extend(&final_state.assigned);
            for (&slot, &ty) in &final_state.narrowed {
                pending.narrowed.insert_mut(slot, ty);
            }
            self.include_breaks(Some(pending));
        }
        self.include_breaks(earlier_breaks);
        if try_.finalbody.is_empty() {
            self.assigned = normal_assigned;
            self.narrowed = normal_facts;
        } else {
            for name in narrowing::deleted_names(&try_.finalbody) {
                if let Some(slot) = self.names.get(&name) {
                    normal_assigned.remove(slot);
                }
            }
            self.assigned.extend(&normal_assigned);
            // Facts from the successful body cannot describe an interrupted one.
            // Preserve only facts established while checking the shared finally.
        }
        Ok((
            vec![Statement::Try {
                body,
                handlers,
                orelse,
                finalbody,
            }],
            has_normal_exit && final_falls,
        ))
    }

    fn if_chain(&mut self, branch: &ast::StmtIf) -> Result<(Vec<Statement>, bool), String> {
        let clauses = &branch.elif_else_clauses;
        let conditions = std::iter::once((branch.test.as_ref(), branch.body.as_slice())).chain(
            clauses
                .iter()
                .filter_map(|clause| Some((clause.test.as_ref()?, clause.body.as_slice()))),
        );
        let mut arms = Vec::with_capacity(clauses.len() + 1);
        let mut joined = None;
        let mut falls_at_cutoff = None;
        let mut exit_fixed = false;
        for (index, (test, body)) in conditions.enumerate() {
            self.branch_layouts(body, &clauses[index..])?;
            let condition = self.condition(test)?;
            let known = self.instance_condition(&condition);
            let before = self.local_state();
            self.apply_facts(test, true);
            let guard_depth = self.regex_guards.len();
            if let Some(guard) = self.regex_guard(&condition, body) {
                self.regex_guards.push(guard);
            }
            let subscript_depth = self.present_subscripts.len();
            if let Some(read) = self.immediate_present_subscript(test, body) {
                self.present_subscripts.push(read);
            }
            let namespace_depth = self.namespace_guards.len();
            self.enter_namespace_guards(test, true, body);
            // In a reachable-only profile, an impossible compatibility guard's
            // expression-only body contains no local declarations to preserve.
            let (yes, yes_falls) = if self.graph.reachable_only()
                && known == Some(false)
                && body.iter().all(|stmt| matches!(stmt, ast::Stmt::Expr(s) if narrowing::expression_assignments(&s.value).is_empty()))
            {
                (Vec::new(), false)
            } else {
                self.block(body)?
            };
            self.namespace_guards.truncate(namespace_depth);
            self.regex_guards.truncate(guard_depth);
            self.present_subscripts.truncate(subscript_depth);
            if yes_falls && known != Some(false) && !exit_fixed {
                self.merge_exit(&mut joined);
            }
            self.restore_state(before);
            self.apply_facts(test, false);
            if known == Some(true) {
                // Later clauses are still checked. Their facts matter for checking
                // dead code only when this prefix has no normal exit of its own.
                falls_at_cutoff.get_or_insert(joined.is_some());
                exit_fixed = joined.is_some();
            }
            arms.push((condition, yes));
        }
        let (mut no, no_falls) = if let Some(tail) = clauses.last().filter(|c| c.test.is_none()) {
            let guards = self.namespace_guards.len();
            let last_test = clauses
                .iter()
                .rev()
                .find_map(|clause| clause.test.as_ref())
                .unwrap_or(&branch.test);
            self.enter_namespace_guards(last_test, false, &tail.body);
            let result = self.block(&tail.body);
            self.namespace_guards.truncate(guards);
            result?
        } else {
            (Vec::new(), true)
        };
        if no_falls && !exit_fixed {
            self.merge_exit(&mut joined);
        }
        let falls = falls_at_cutoff.unwrap_or(joined.is_some());
        if let Some(joined) = joined {
            self.restore_state(joined);
        }
        for (condition, yes) in arms.into_iter().rev() {
            no = vec![Statement::If(condition, yes, no)];
        }
        Ok((no, falls))
    }

    fn assign(&mut self, target: &ast::Expr, value: Expr) -> Result<Statement, String> {
        let ast::Expr::Name(name) = target else {
            return Err(error(
                self.source,
                target,
                "local assignment requires a name",
            ));
        };
        let value = if let Some(&slot) = self.names.get(name.id.as_str()) {
            self.coerce(value, self.locals[slot], target)?
        } else if let Some(partial) = self.types.partial_function(value.ty) {
            // Different partials can share a call signature without sharing a capture frame.
            self.coerce(value, partial.callable, target)?
        } else if let Some(&nullable) = self.nullable_assignments.get(name.id.as_str())
            && self.types.payload(nullable) == Some(value.ty)
        {
            self.coerce(value, nullable, target)?
        } else if let Some(signature) = self.types.index_callable_signature(value.ty) {
            self.coerce(value, signature, target)?
        } else {
            Self::inferred_stream(value)
        };
        let slot = self.declare(name.id.as_str(), value.ty, target)?;
        self.assigned_value(slot, &value);
        Ok(Statement::Assign(slot, value))
    }

    fn integer_addend(op: ast::Operator, value: Expr, other: Type) -> Expr {
        if matches!(op, ast::Operator::Add | ast::Operator::Sub)
            && matches!(other, Type::Int | Type::Bool)
        {
            powers::integer(value)
        } else {
            value
        }
    }

    fn binary_operation(&mut self, binary: &ast::ExprBinOp) -> Result<Expr, String> {
        let (mut left, mut right) = if matches!(
            binary.op,
            ast::Operator::BitOr | ast::Operator::BitAnd | ast::Operator::Sub
        ) {
            let (left, left_key) = self.union_operand(&binary.left)?;
            let (right, right_key) = self.union_operand(&binary.right)?;
            if let (Some(left_key), Some(right_key)) = (left_key, right_key) {
                self.expect(left_key, right_key, binary)?;
                return Ok(Expr {
                    ty: self.types.set(left_key),
                    kind: ExprKind::Builtin(
                        match binary.op {
                            ast::Operator::Sub => Builtin::KeyDifference,
                            ast::Operator::BitAnd => Builtin::KeyIntersection,
                            _ => Builtin::KeyUnion,
                        },
                        vec![left, right],
                    ),
                });
            }
            (left, right)
        } else {
            (self.expr(&binary.left)?, self.expr(&binary.right)?)
        };
        if binary.op == ast::Operator::Mod
            && left.ty == Type::Str
            && self.types.element(right.ty) == Some(Type::ObjectValue)
            && binary
                .right
                .as_name_expr()
                .is_some_and(|name| self.positional_pack.as_deref() == Some(name.id.as_str()))
        {
            return Ok(Expr {
                ty: Type::Str,
                kind: ExprKind::Builtin(Builtin::PercentFormat, vec![left, right]),
            });
        }
        if binary.op == ast::Operator::Add
            && (self.types.payload(left.ty) == Some(Type::Int)
                || self.types.payload(right.ty) == Some(Type::Int))
            && self.types.payload(left.ty).unwrap_or(left.ty) == Type::Int
            && self.types.payload(right.ty).unwrap_or(right.ty) == Type::Int
        {
            return Ok(self.nullable_int_sum(left, right));
        }
        if let (Type::Calendar(first), Type::Calendar(second)) = (left.ty, right.ty) {
            return calendar::binary(first, second, left, right, binary, self.source);
        }
        if binary.op == ast::Operator::Mod
            && matches!(left.ty, Type::Str | Type::StrLiteral(_))
            && matches!(right.ty, Type::Str | Type::StrLiteral(_))
        {
            left = Self::string_value(left);
            right = Self::string_value(right);
        }
        if binary.op == ast::Operator::Add
            && left.ty == Type::JsonValue
            && right.ty == Type::JsonValue
        {
            return Ok(json::add(left, right));
        }
        if binary.op == ast::Operator::Add
            && matches!(left.ty, Type::Str | Type::StrEnum(_) | Type::StrLiteral(_))
            && matches!(right.ty, Type::Str | Type::StrEnum(_) | Type::StrLiteral(_))
        {
            left = Self::string_value(left);
            right = Self::string_value(right);
        }
        if binary.op == ast::Operator::Div && (left.ty.is_path() || right.ty.is_path()) {
            return self.path_join(left, right, binary);
        }
        if binary.op == ast::Operator::Add && self.types.element(left.ty).is_some() {
            if matches!(&left.kind, ExprKind::List(items) if items.is_empty())
                && self.types.element(right.ty).is_some()
            {
                left.ty = right.ty;
            } else if matches!(&right.kind, ExprKind::List(items) if items.is_empty()) {
                right.ty = left.ty;
            }
            self.expect(left.ty, right.ty, binary)?;
            return self.concat_lists(left, right, binary);
        }
        if self.types.is_counter(left.ty) && self.types.is_counter(right.ty) {
            return self.counter_math(binary.op, left, right, false, binary);
        }
        self.binary_expr(binary.op, left, right, binary)
    }

    fn binary_expr(
        &self,
        op: ast::Operator,
        left: Expr,
        right: Expr,
        node: &impl Ranged,
    ) -> Result<Expr, String> {
        let optional_str = |ty| self.types.payload(ty) == Some(Type::Str);
        if op == ast::Operator::Add
            && (optional_str(left.ty) || optional_str(right.ty))
            && [left.ty, right.ty]
                .into_iter()
                .all(|ty| matches!(ty, Type::Str | Type::None) || optional_str(ty))
        {
            return Ok(Expr {
                ty: Type::Str,
                kind: ExprKind::Builtin(Builtin::OptionalStrConcat, vec![left, right]),
            });
        }
        if op == ast::Operator::Mod
            && let Some(formatted) = single_dispatch::format_argument(self.types, &left, &right)
        {
            return Ok(formatted);
        }
        if op == ast::Operator::Mod
            && left.ty == Type::Str
            && (matches!(right.ty, Type::Int | Type::Str)
                || self.types.percent_tuple(right.ty)
                || (matches!(right.ty, Type::Dict(_))
                    && self.types.mapping(right.ty) == Some((Type::Str, Type::ObjectValue))))
        {
            return Ok(Expr {
                ty: Type::Str,
                kind: ExprKind::Builtin(Builtin::PercentFormat, vec![left, right]),
            });
        }
        if op == ast::Operator::Pow {
            let (operation, ty) = self.integer_power(&left, &right, false, node)?;
            return Ok(Expr {
                ty,
                kind: ExprKind::Builtin(
                    operation,
                    vec![powers::integer(left), powers::integer(right)],
                ),
            });
        }
        let left = Self::integer_addend(op, left, right.ty);
        let right = Self::integer_addend(op, right, left.ty);
        let (op, ty) = self.binary_op(op, left.ty, right.ty, node)?;
        Ok(Expr {
            ty,
            kind: ExprKind::Binary(op, Box::new(left), Box::new(right)),
        })
    }

    fn binary_op(
        &self,
        op: ast::Operator,
        left: Type,
        right: Type,
        node: &impl Ranged,
    ) -> Result<(Binary, Type), String> {
        let bitwise = match op {
            ast::Operator::BitAnd => Some(Binary::BitAnd),
            ast::Operator::BitOr => Some(Binary::BitOr),
            ast::Operator::BitXor => Some(Binary::BitXor),
            ast::Operator::LShift => Some(Binary::LeftShift),
            ast::Operator::RShift => Some(Binary::RightShift),
            _ => None,
        };
        if let Some(op) = bitwise {
            self.expect(Type::Int, left, node)?;
            self.expect(Type::Int, right, node)?;
            return Ok((op, Type::Int));
        }
        if op == ast::Operator::Mult {
            if (self.types.element(left).is_some() || matches!(left, Type::Bytes | Type::Str))
                && right == Type::Int
            {
                return Ok((Binary::Repeat, left));
            }
            if left == Type::Int
                && (self.types.element(right).is_some() || matches!(right, Type::Bytes | Type::Str))
            {
                return Ok((Binary::Repeat, right));
            }
        }
        if op == ast::Operator::Add && matches!(left, Type::Str | Type::Bytes) && right == left {
            return Ok((Binary::Concat, left));
        }
        if !matches!(left, Type::Int | Type::Float) || !matches!(right, Type::Int | Type::Float) {
            return Err(error(
                self.source,
                node,
                "arithmetic requires int or float operands",
            ));
        }
        let result = if left == Type::Float || right == Type::Float || op == ast::Operator::Div {
            Type::Float
        } else {
            Type::Int
        };
        let op = match op {
            ast::Operator::Add => Binary::Add,
            ast::Operator::Sub => Binary::Subtract,
            ast::Operator::Mult => Binary::Multiply,
            ast::Operator::Div => Binary::Divide,
            ast::Operator::FloorDiv => Binary::FloorDivide,
            ast::Operator::Mod => Binary::Modulo,
            _ => return Err(error(self.source, node, "unsupported numeric operator")),
        };
        Ok((op, result))
    }

    /// MIME module tables are rebound by init(). Qualified loads and stores
    /// use the same owned state fields as the bundled implementation.
    fn mime_attribute(&self, attr: &ast::ExprAttribute) -> Option<ast::ExprAttribute> {
        let Binding::Module(module) = self.resolve(&attr.value)? else {
            return None;
        };
        if !self.graph.modules[module].is_bundled("mimetypes")
            || !matches!(
                attr.attr.as_str(),
                "knownfiles"
                    | "inited"
                    | "suffix_map"
                    | "encodings_map"
                    | "types_map"
                    | "common_types"
            )
        {
            return None;
        }
        let mut state = attr.clone();
        state.attr.id = "_state".into();
        state.ctx = ast::ExprContext::Load;
        let mut field = attr.clone();
        field.value = Box::new(ast::Expr::Attribute(state));
        Some(field)
    }

    fn expr(&mut self, expression: &ast::Expr) -> Result<Expr, String> {
        let value = self.typed_expr(expression)?;
        Ok(if matches!(value.ty, Type::StrLiteral(_)) {
            literals::view(value, Type::Str)
        } else {
            value
        })
    }

    fn typed_expr(&mut self, expression: &ast::Expr) -> Result<Expr, String> {
        if let ast::Expr::Attribute(attribute) = expression
            && let Some(field) = self.mime_attribute(attribute)
        {
            return self.typed_expr(&ast::Expr::Attribute(field));
        }
        if let Some(name) = self.unbound_assert_root(expression) {
            return self.omitted_assert_read(name, None, expression);
        }
        // Calls and binary operators recursively check their operands. Keep
        // the large general expression frame off both paths in debug builds.
        match expression {
            ast::Expr::Name(name) if self.class_lexical.contains(name.id.as_str()) => {
                Ok(self.local(self.names[name.id.as_str()]))
            }
            ast::Expr::Call(call) => self.call(call),
            ast::Expr::BinOp(binary) => self.binary_operation(binary),
            _ => self.non_call_expr(expression),
        }
    }

    /// Keep the large general frame off recursive left- and right-hand arithmetic.
    fn non_call_expr(&mut self, expression: &ast::Expr) -> Result<Expr, String> {
        if self.unshadowed_len(expression) {
            return Ok(self.builtin_len());
        }
        if self.resolve(expression) == Some(Binding::Intrinsic(Intrinsic::AttrGetter)) {
            return Err(self.runtime_attrgetter(expression));
        }
        if self.resolve(expression) == Some(Binding::Intrinsic(Intrinsic::PopenActive)) {
            return Ok(Expr {
                ty: self.types.list(Type::ActivePopen),
                kind: ExprKind::Builtin(
                    Builtin::Process(ir::ProcessOperation::PopenActive),
                    Vec::new(),
                ),
            });
        }
        if typing::is_dummy(self.graph, self.resolve(expression)) {
            return Ok(typing::dummy());
        }

        if let Some(name) = self.collection_class_name(expression) {
            return Ok(annotations::class_value(name));
        }
        if let ast::Expr::Attribute(attr) = expression
            && let Some(Binding::Intrinsic(Intrinsic::AnnotationFormat(number))) =
                self.resolve(&attr.value)
        {
            match attr.attr.as_str() {
                "value" => return Ok(pipelines::integer(number)),
                "name" => {
                    return Ok(unpacking::string(
                        ["VALUE", "VALUE_WITH_FAKE_GLOBALS", "FORWARDREF", "STRING"]
                            [(number - 1) as usize],
                    ));
                }
                _ => {}
            }
        }
        let selector = match self.resolve(expression) {
            Some(Binding::Intrinsic(Intrinsic::PopenSelector)) => Some(None),
            Some(Binding::Intrinsic(Intrinsic::SelectorClass(select))) => Some(Some(select)),
            Some(binding @ Binding::Definition(def))
                if self.graph.modules[def.module].is_bundled("selectors")
                    && self.graph.modules[def.module].bindings.get("PollSelector")
                        == Some(&binding) =>
            {
                // As a value this class selects the poll backend for Popen. Calls
                // still construct the bundled PollSelector with its own fd map.
                Some(Some(false))
            }
            _ => None,
        };
        if let Some(mode) = selector {
            return Ok(Expr {
                ty: Type::ProcessSelectorClass,
                kind: ExprKind::Builtin(
                    Builtin::Process(ir::ProcessOperation::SelectorClass(mode)),
                    Vec::new(),
                ),
            });
        }
        if let Some(Binding::Intrinsic(Intrinsic::Declaration(symbol))) = self.resolve(expression) {
            return Err(error(
                self.source,
                expression,
                &format!("no native implementation for {symbol}"),
            ));
        }
        if let ast::Expr::Attribute(attr) = expression
            && let Some(value) = self.calendar_class_attribute(attr)
        {
            return Ok(value);
        }
        if let Some(target) = self.resolve(expression) {
            if target == Binding::Intrinsic(Intrinsic::CalendarUtc) {
                return Ok(calendar::utc());
            }
            if matches!(target, Binding::Module(_) | Binding::IntrinsicModule(_))
                && let Some(module) = self.module_docs(target, expression)?
            {
                return Ok(module);
            }
            if let Binding::Module(id) = target {
                return Ok(Expr {
                    ty: Type::SourceModule,
                    kind: ExprKind::Builtin(
                        Builtin::SourceModule,
                        vec![
                            unpacking::string(&self.graph.modules[id].name),
                            unpacking::string(self.graph.module_filename(id).to_string_lossy()),
                        ],
                    ),
                });
            }
            if let Some(value) = self.errno_value(target, expression) {
                return Ok(value);
            }
        }
        if let Some(Binding::Intrinsic(Intrinsic::SocketConstant(name))) = self.resolve(expression)
        {
            return Ok(socket::constant(name));
        }
        if let Some(Binding::Intrinsic(
            Intrinsic::LogLevel(level)
            | Intrinsic::IntConstant(level)
            | Intrinsic::AnnotationFormat(level),
        )) = self.resolve(expression)
        {
            return Ok(Expr {
                ty: Type::Int,
                kind: ExprKind::Int(level.to_string()),
            });
        }
        if let Some(prefix) = self.cache_config(expression) {
            return Ok(cache_paths::op(
                ir::CachePathOperation::Get(prefix),
                self.types.optional(Type::Str),
                vec![],
            ));
        }
        self.reject_class_receiver_value(expression)?;
        if let Some(binding) = self.local_function_binding(expression) {
            if let Some(slot) = binding.value_slot {
                return Ok(self.local(slot));
            }
            return Err(error(
                self.source,
                expression,
                "local functions require a direct call or a noncapturing map/filter callback",
            ));
        }
        if let Some(Binding::Definition(definition)) = self.resolve(expression)
            && let Some(signature) = self.functions.get(&definition).cloned()
        {
            return self.callable_value(&signature, None, None, expression);
        }
        if let Some(Binding::Intrinsic(
            op @ (Intrinsic::Copy(_) | Intrinsic::RmTree | Intrinsic::Move | Intrinsic::DestInSrc),
        )) = self.resolve(expression)
        {
            return self.shutil_function_value(op, None, expression);
        }
        if let Some(Binding::Value(definition)) = self.resolve(expression) {
            let (slot, ty) = self.globals.get(definition).ok_or_else(|| {
                error(
                    self.source,
                    expression,
                    "module value is not initialized on this path",
                )
            })?;
            return Ok(Expr {
                ty,
                kind: ExprKind::Builtin(Builtin::GlobalGet(slot), Vec::new()),
            });
        }
        if let ast::Expr::Attribute(attr) = expression
            && attr.attr.as_str() == "avoids_symlink_attacks"
            && self.resolve(&attr.value) == Some(Binding::Intrinsic(Intrinsic::RmTree))
        {
            return Ok(Expr {
                ty: Type::Bool,
                kind: ExprKind::Bool(true),
            });
        }
        if let Some(Binding::Intrinsic(Intrinsic::AccessMode(mode))) = self.resolve(expression) {
            return Ok(Expr {
                ty: Type::Int,
                kind: ExprKind::Int(mode.to_string()),
            });
        }
        if let Some(Binding::Intrinsic(symbol @ (Intrinsic::Argv | Intrinsic::OrigArgv))) =
            self.resolve(expression)
        {
            return Ok(Expr {
                ty: self.types.list(Type::Str),
                kind: ExprKind::Builtin(
                    if symbol == Intrinsic::Argv {
                        Builtin::SysArgv
                    } else {
                        Builtin::SysOrigArgv
                    },
                    Vec::new(),
                ),
            });
        }
        if let Some(Binding::Intrinsic(symbol)) = self.resolve(expression) {
            if let Some((flags, executable)) = &self.graph.command_startup {
                match symbol {
                    Intrinsic::SysExecutable => return Ok(unpacking::string(executable.clone())),
                    Intrinsic::SysDontWriteBytecode => {
                        return Ok(Expr {
                            ty: Type::Bool,
                            kind: ExprKind::Bool(flags.dont_write_bytecode),
                        });
                    }
                    _ => {}
                }
            }
            match symbol {
                Intrinsic::BuiltinsLen => return Ok(self.builtin_len()),
                Intrinsic::CodecBom(bytes) => {
                    return Ok(Expr {
                        ty: Type::Bytes,
                        kind: ExprKind::Bytes(bytes.to_vec()),
                    });
                }
                Intrinsic::Stdout => return Ok(self.stdout_value()),
                Intrinsic::OriginalStdout | Intrinsic::Stderr => {
                    return Ok(Expr {
                        ty: Type::TextIO,
                        kind: ExprKind::Builtin(
                            Builtin::SysOriginalOutput {
                                stderr: symbol == Intrinsic::Stderr,
                            },
                            vec![],
                        ),
                    });
                }
                Intrinsic::BuiltinsLast => {
                    return Ok(Expr {
                        ty: Type::ObjectValue,
                        kind: ExprKind::Builtin(Builtin::BuiltinsLast, vec![]),
                    });
                }
                Intrinsic::JsonLoads => return self.json_load_callable(None, None, expression),
                Intrinsic::DescriptorConstant(name) => {
                    return Ok(Expr {
                        ty: Type::Int,
                        kind: ExprKind::Builtin(
                            Builtin::Process(ir::ProcessOperation::DescriptorConstant(name)),
                            vec![],
                        ),
                    });
                }
                Intrinsic::HashAvailable => {
                    return Ok(Expr {
                        ty: self.types.set(Type::Str),
                        kind: ExprKind::Builtin(Builtin::HashAvailable, vec![]),
                    });
                }
                Intrinsic::Linecache(ir::LinecacheOperation::Cache) => {
                    return Ok(self.linecache_mapping());
                }
                Intrinsic::InspectSentinel => return Ok(inspect::sentinel()),
                Intrinsic::Placeholder => {
                    return Ok(partials::singleton(ir::PlaceholderKind::Value));
                }
                Intrinsic::WrapperAttributes(fields) => {
                    return Ok(functools::wrapper_attributes(fields, self.types));
                }
                Intrinsic::OsSeparator { alternate, posix } => {
                    if posix {
                        return Ok(if alternate {
                            Expr {
                                ty: Type::None,
                                kind: ExprKind::None,
                            }
                        } else {
                            unpacking::string("/")
                        });
                    }
                    return Ok(Expr {
                        ty: if alternate {
                            self.types.optional(Type::Str)
                        } else {
                            Type::Str
                        },
                        kind: ExprKind::Builtin(Builtin::OsSeparator { alternate }, vec![]),
                    });
                }
                Intrinsic::OsDevnull => {
                    return Ok(Expr {
                        ty: Type::Str,
                        kind: ExprKind::Str("/dev/null".into()),
                    });
                }
                Intrinsic::LogRecordFlag(mask) => {
                    return Ok(logging_records::op(
                        ir::LogRecordOperation::Flag,
                        Type::Bool,
                        vec![logging::number(mask)],
                    ));
                }
                Intrinsic::LogRoot => return Ok(self.root_logger()),
                Intrinsic::LevelMap { names } => return Ok(self.logging_level_map(names)),
                Intrinsic::SysStdlibModuleNames => {
                    return Ok(Expr {
                        ty: Type::FrozenStrings,
                        kind: ExprKind::Builtin(Builtin::SysStdlibModuleNames, vec![]),
                    });
                }
                Intrinsic::SysImplementation => {
                    return Ok(Expr {
                        ty: Type::SysImplementation,
                        kind: ExprKind::Builtin(Builtin::SysImplementation, vec![]),
                    });
                }
                Intrinsic::SysVersionInfo => return Ok(self.sys_version_info(false)),
                Intrinsic::SysThreadInfo => return Ok(self.sys_thread_info()),
                Intrinsic::SysHexversion => {
                    return Ok(crate::version::integer(crate::version::hex(
                        crate::version::components(false),
                    )));
                }
                Intrinsic::SysVersion => {
                    let [major, minor, micro] = crate::version::components(false);
                    return Ok(unpacking::string(format!(
                        "{major}.{minor}.{micro} (Imre {})",
                        env!("CARGO_PKG_VERSION")
                    )));
                }
                Intrinsic::SysExecutable | Intrinsic::SysPlatform | Intrinsic::OsName => {
                    let operation = match symbol {
                        Intrinsic::SysExecutable => {
                            Builtin::Process(ir::ProcessOperation::Executable)
                        }
                        Intrinsic::OsName => Builtin::OsName,
                        _ => Builtin::SysPlatform,
                    };
                    return Ok(Expr {
                        ty: Type::Str,
                        kind: ExprKind::Builtin(operation, Vec::new()),
                    });
                }
                _ => {}
            }
        }
        let (ty, kind) = match expression {
            ast::Expr::NumberLiteral(value) => match &value.value {
                ast::Number::Int(value) => (Type::Int, ExprKind::Int(value.to_string())),
                ast::Number::Float(value) => (Type::Float, ExprKind::Float(*value)),
                _ => {
                    return Err(error(
                        self.source,
                        expression,
                        "complex numeric literals are unsupported",
                    ));
                }
            },
            ast::Expr::BooleanLiteral(value) => (Type::Bool, ExprKind::Bool(value.value)),
            ast::Expr::StringLiteral(value) => {
                check_string(value, self.source)?;
                (Type::Str, ExprKind::Str(value.value.to_string()))
            }
            ast::Expr::BytesLiteral(value) => {
                (Type::Bytes, ExprKind::Bytes(value.value.bytes().collect()))
            }
            ast::Expr::FString(value) => return self.fstring(value),
            ast::Expr::Lambda(lambda) => return self.lambda_value(lambda, None, None),
            ast::Expr::Named(value) => return self.named_expression(value, None),
            ast::Expr::If(value) => return self.conditional(value, None),
            ast::Expr::NoneLiteral(_) => (Type::None, ExprKind::None),
            ast::Expr::Name(name) if name.id == "__debug__" => (
                Type::Bool,
                ExprKind::Bool(self.graph.python_optimization() == 0),
            ),
            ast::Expr::Name(name)
                if name.id == "__doc__" && !self.names.contains_key("__doc__") =>
            {
                let module = &self.graph.modules[self.module];
                if self.graph.python_optimization() < 2
                    && let Some(ast::Stmt::Expr(doc)) = module.parsed.syntax().body.first()
                    && let ast::Expr::StringLiteral(text) = &*doc.value
                {
                    check_string(text, self.source)?;
                    (Type::Str, ExprKind::Str(text.value.to_string()))
                } else {
                    (Type::None, ExprKind::None)
                }
            }
            ast::Expr::Name(name)
                if name.id == "__file__"
                    && self.module == self.graph.entry
                    && self.graph.virtual_command =>
            {
                let exception = Expr {
                    ty: Type::Exception,
                    kind: ExprKind::Exception {
                        kind: exception_kind("NameError").unwrap(),
                        message: Some(Box::new(unpacking::string(
                            "name '__file__' is not defined",
                        ))),
                    },
                };
                (
                    Type::Str,
                    ExprKind::Builtin(Builtin::RaiseException, vec![exception]),
                )
            }
            ast::Expr::Name(name) if name.id == "__file__" => (
                Type::Str,
                ExprKind::Str(
                    self.graph
                        .module_filename(self.module)
                        .to_string_lossy()
                        .into_owned(),
                ),
            ),
            ast::Expr::Name(name)
                if name.id == "__package__"
                    && !self.names.contains_key("__package__")
                    && self.resolve(expression).is_none() =>
            {
                match self.graph.module_package(self.module) {
                    Some(package) => (Type::Str, ExprKind::Str(package.to_owned())),
                    None => (Type::None, ExprKind::None),
                }
            }
            ast::Expr::Name(name)
                if name.id == "__name__" && !self.names.contains_key("__name__") =>
            {
                let name = if self.module == self.graph.entry {
                    "__main__"
                } else {
                    &self.graph.modules[self.module].name
                };
                (Type::Str, ExprKind::Str(name.to_owned()))
            }
            ast::Expr::Name(name) => {
                if !self.names.contains_key(name.id.as_str())
                    && self.resolve(expression).is_none()
                    && let Some(value) = annotations::builtin_type(name.id.as_str())
                {
                    return Ok(value);
                }
                if self.comprehension_targets.contains(name.id.as_str())
                    && !self.names.contains_key(name.id.as_str())
                {
                    return Err(error(
                        self.source,
                        expression,
                        "comprehension local may be uninitialized on this path",
                    ));
                }
                let slot = self.names.get(name.id.as_str()).copied().ok_or_else(|| error(self.source, expression, "name is not a local value; globals and first-class functions are unsupported"))?;
                if !self.assigned.contains(&slot) {
                    if let Some(&flag) = self.bound_flags.get(name.id.as_str()) {
                        self.graph
                            .checked_unbound_local_read(self.module, expression.range());
                        return Ok(self.checked_bound_read(slot, flag, name.id.as_str()));
                    }
                    return Err(error(
                        self.source,
                        expression,
                        "local may be uninitialized on this path",
                    ));
                }
                return Ok(self.read_local(slot));
            }
            ast::Expr::List(list) => {
                if list
                    .elts
                    .iter()
                    .any(|value| matches!(value, ast::Expr::Starred(_)))
                {
                    return self.expanded_sequence(&list.elts, None, false, list);
                }
                let mut values = Vec::new();
                let mut element = Type::Int;
                let mut mixed = false;
                for value in &list.elts {
                    let item = Self::inferred_stream(self.expr(value)?);
                    if values.is_empty() {
                        element = item.ty;
                    }
                    if !self.module_scope
                        && matches!(element, Type::Int | Type::Str)
                        && matches!(item.ty, Type::Int | Type::Str)
                    {
                        mixed |= item.ty != element;
                    } else {
                        self.expect(element, item.ty, value)?;
                    }
                    values.push(item);
                }
                if mixed {
                    // Local storage can infer this closed layout. Module globals and
                    // lists with a declared element type retain their storage contract.
                    element = self.types.string_or_int();
                    values = values
                        .into_iter()
                        .zip(&list.elts)
                        .map(|(item, at)| self.coerce(item, element, at))
                        .collect::<Result<_, _>>()?;
                }
                (self.types.list(element), ExprKind::List(values))
            }
            ast::Expr::ListComp(_) | ast::Expr::DictComp(_) | ast::Expr::SetComp(_) => {
                return self.comprehension(expression, None);
            }
            ast::Expr::Dict(dict) => {
                if self.graph.mutable_documents
                    && dict
                        .items
                        .iter()
                        .any(|item| item.key.is_none() && self.probe_document(&item.value))
                {
                    return Ok(self.document_literal(expression, documents::MAP)?.unwrap());
                }
                return self.dictionary(dict, None);
            }
            ast::Expr::Set(set) => return self.set_literal(set, None),
            ast::Expr::Tuple(tuple) => {
                if tuple
                    .elts
                    .iter()
                    .any(|v| matches!(v, ast::Expr::Starred(_)))
                {
                    return self.expanded_tuple(tuple, None);
                }
                let values = tuple
                    .elts
                    .iter()
                    .map(|value| self.expr(value))
                    .collect::<Result<Vec<_>, _>>()?;
                let values = if values
                    .iter()
                    .all(|value| matches!(value.ty, Type::Document(_)))
                    && values.windows(2).any(|v| v[0].ty != v[1].ty)
                {
                    values
                        .into_iter()
                        .map(|v| self.document_coerce(v, documents::ANY, tuple))
                        .collect::<Result<Vec<_>, _>>()?
                } else {
                    values
                };
                let ty = self
                    .types
                    .tuple(values.iter().map(|value| value.ty).collect());
                (ty, ExprKind::Tuple(values))
            }
            ast::Expr::UnaryOp(unary) => {
                let value = if unary.op == ast::UnaryOp::Not {
                    self.condition(&unary.operand)?
                } else {
                    self.expr(&unary.operand)?
                };
                match unary.op {
                    ast::UnaryOp::Not => (Type::Bool, ExprKind::Not(Box::new(value))),
                    ast::UnaryOp::Invert => {
                        self.expect(Type::Int, value.ty, expression)?;
                        (
                            Type::Int,
                            ExprKind::Binary(
                                Binary::BitXor,
                                Box::new(value),
                                Box::new(Expr {
                                    ty: Type::Int,
                                    kind: ExprKind::Int("-1".into()),
                                }),
                            ),
                        )
                    }
                    ast::UnaryOp::USub | ast::UnaryOp::UAdd => {
                        if value.ty == Type::Calendar(ir::CalendarType::Delta)
                            && unary.op == ast::UnaryOp::USub
                        {
                            return Ok(calendar::op(ir::CalendarOp::Negate, value.ty, vec![value]));
                        }
                        if self.types.is_counter(value.ty) {
                            let op = if unary.op == ast::UnaryOp::UAdd {
                                ir::CounterOp::Positive
                            } else {
                                ir::CounterOp::Negative
                            };
                            return Ok(Expr {
                                ty: value.ty,
                                kind: ExprKind::Builtin(
                                    Builtin::CounterMath { op, inplace: false },
                                    vec![value],
                                ),
                            });
                        }
                        if !matches!(value.ty, Type::Int | Type::Float) {
                            return Err(error(
                                self.source,
                                expression,
                                "unary signs require int or float",
                            ));
                        }
                        if unary.op == ast::UnaryOp::UAdd {
                            return Ok(value);
                        }
                        (value.ty, ExprKind::Negate(Box::new(value)))
                    }
                }
            }
            ast::Expr::BoolOp(boolean) => {
                return self.boolean_value(boolean, None);
            }
            ast::Expr::Compare(compare) => {
                if let [op @ (ast::CmpOp::Is | ast::CmpOp::IsNot)] = compare.ops.as_ref()
                    && let [left, right] = compare.operands.as_ref()
                    && let (Some(left), Some(right)) =
                        (self.class_reference(left), self.class_reference(right))
                {
                    return Ok(Expr {
                        ty: Type::Bool,
                        kind: ExprKind::Bool((left == right) == (*op == ast::CmpOp::Is)),
                    });
                }
                if compare.ops.len() == 1
                    && matches!(compare.ops[0], ast::CmpOp::In | ast::CmpOp::NotIn)
                {
                    let key = self.expr(&compare.operands[0])?;
                    let receiver = Self::string_value(self.expr(&compare.operands[1])?);
                    if receiver.ty == Type::Context && !matches!(key.ty, Type::ContextVar(_)) {
                        return self.context_key_error(
                            receiver,
                            key,
                            None,
                            true,
                            Type::Bool,
                            expression,
                        );
                    }
                    if receiver.ty == Type::Context && matches!(key.ty, Type::ContextVar(_)) {
                        let found = contextvars::op(
                            ir::ContextOperation::ContextContains,
                            Type::Bool,
                            vec![key, receiver],
                        );
                        return Ok(if compare.ops[0] == ast::CmpOp::NotIn {
                            Expr {
                                ty: Type::Bool,
                                kind: ExprKind::Not(Box::new(found)),
                            }
                        } else {
                            found
                        });
                    }
                    let receiver = if matches!(receiver.ty, Type::Document(_)) {
                        self.document_map(receiver)
                    } else {
                        receiver
                    };
                    if self.mapping_class(receiver.ty).is_some() {
                        let found = self.mapping_membership(key, receiver, expression)?;
                        return Ok(if compare.ops[0] == ast::CmpOp::NotIn {
                            Expr {
                                ty: Type::Bool,
                                kind: ExprKind::Not(Box::new(found)),
                            }
                        } else {
                            found
                        });
                    }
                    if receiver.ty == Type::Bytes {
                        return self.bytes_membership(key, receiver, compare.ops[0], expression);
                    }
                    if let Some(fields) = self.types.fields(receiver.ty) {
                        return self.tuple_contains(key, receiver, &fields, compare);
                    }
                    let receiver = if receiver.ty == Type::ModuleNamespace {
                        errno::op(ir::ErrnoOperation::Keys, Type::StringKeys, vec![receiver])
                    } else {
                        receiver
                    };
                    let (key_type, builtin) = if let Some((target_key, _)) =
                        self.types.mapping(receiver.ty)
                    {
                        if target_key == Type::Int && key.ty == Type::ObjectValue {
                            (key.ty, Builtin::Errno(ir::ErrnoOperation::MemberContains))
                        } else if matches!(target_key, Type::IntEnum(_)) && key.ty == Type::Int {
                            (Type::Int, Builtin::DictContains)
                        } else {
                            (target_key, Builtin::DictContains)
                        }
                    } else if receiver.ty == Type::ReadOnlyObjectMapping {
                        (Type::Str, Builtin::DictContains)
                    } else if let Some((key, _)) = self.types.chainmap_entries(receiver.ty) {
                        (key, Builtin::ChainMapContains)
                    } else if receiver.ty == Type::Str {
                        (Type::Str, Builtin::StringContains)
                    } else if let Some(element @ (Type::Int | Type::Str | Type::Bool)) =
                        self.types.deque_element(receiver.ty)
                    {
                        (element, Builtin::DequeContains)
                    } else if let Some(element) = self.types.readable_set_element(receiver.ty) {
                        (element, Builtin::SetContains)
                    } else if let Some(ty) = self.types.element(receiver.ty)
                        && self.types.is_value_struct(ty)
                    {
                        (ty, Builtin::StructListContains)
                    } else if receiver.ty == Type::ListInt {
                        (self.types.optional(Type::Int), Builtin::IntListContains)
                    } else if self.types.element(receiver.ty) == Some(Type::Str) {
                        (Type::Str, Builtin::StringListContains)
                    } else {
                        return Err(error(
                            self.source,
                            expression,
                            "membership requires a concrete dictionary or set",
                        ));
                    };
                    let key = if receiver.ty == Type::ListInt {
                        self.coerce(key, key_type, expression)?
                    } else if key_type == Type::Str {
                        Self::string_value(key)
                    } else {
                        key
                    };
                    self.expect(key_type, key.ty, expression)?;
                    let mut result = Expr {
                        ty: Type::Bool,
                        kind: ExprKind::Builtin(builtin, vec![key, receiver]),
                    };
                    if compare.ops[0] == ast::CmpOp::NotIn {
                        result = Expr {
                            ty: Type::Bool,
                            kind: ExprKind::Not(Box::new(result)),
                        };
                    }
                    return Ok(result);
                }
                if compare.ops.len() == 1
                    && matches!(compare.ops[0], ast::CmpOp::Is | ast::CmpOp::IsNot)
                {
                    let value = match compare.operands.as_ref() {
                        [ast::Expr::NoneLiteral(_), value] | [value, ast::Expr::NoneLiteral(_)] => {
                            self.expr(value)?
                        }
                        [left, right] => {
                            return self.nominal_identity(left, right, compare.ops[0], expression);
                        }
                        _ => unreachable!(),
                    };
                    return Ok(Self::identity_is_none(value, compare.ops[0]));
                }
                let mut values = Vec::new();
                let mut exit = None;
                for (index, value) in compare.operands.iter().enumerate() {
                    values.push(self.expr(value)?);
                    if index != 0 {
                        self.merge_exit(&mut exit);
                    }
                }
                self.restore_state(exit.expect("comparisons have at least two operands"));
                if values.iter().any(|v| matches!(v.ty, Type::WeakProxy(_))) {
                    return self.weak_proxy_comparison(values, &compare.ops, expression);
                }
                if values
                    .iter()
                    .any(|value| matches!(value.ty, Type::Class(id) if self.decimal_class(id)))
                {
                    return Err(error(
                        self.source,
                        expression,
                        "native Decimal numeric comparisons are unsupported",
                    ));
                }
                if let [op @ (ast::CmpOp::Eq | ast::CmpOp::NotEq)] = compare.ops.as_ref()
                    && values
                        .iter()
                        .any(|v| self.types.mapping(v.ty) == Some((Type::Str, documents::SCALAR)))
                    && values.iter().all(|v| {
                        matches!(
                            self.types.mapping(v.ty),
                            Some((Type::Str, documents::ANY | documents::SCALAR))
                        )
                    })
                {
                    // Both dictionaries store tagged values. One has only exact scalars, so
                    // two identity-sensitive values (such as NaNs) cannot face each other.
                    // This view is for the comparison only; mutation retains its value contract.
                    let ty = self.types.dict(Type::Str, documents::ANY);
                    let values = values
                        .into_iter()
                        .map(|v| {
                            if v.ty == ty {
                                v
                            } else {
                                Expr {
                                    ty,
                                    kind: ExprKind::Builtin(Builtin::RecordView, vec![v]),
                                }
                            }
                        })
                        .collect();
                    let op = if *op == ast::CmpOp::Eq {
                        Compare::Equal
                    } else {
                        Compare::NotEqual
                    };
                    return Ok(Expr {
                        ty: Type::Bool,
                        kind: ExprKind::Compare(values, vec![op]),
                    });
                }
                if let [op @ (ast::CmpOp::Eq | ast::CmpOp::NotEq)] = compare.ops.as_ref()
                    && values.iter().all(|v| v.ty == Type::SourceModule)
                {
                    return Ok(Self::identity_result(values, *op == ast::CmpOp::NotEq));
                }
                if values.iter().all(|v| matches!(v.ty, Type::ForwardRef(_))) {
                    let ops = compare
                        .ops
                        .iter()
                        .map(|op| match op {
                            ast::CmpOp::Eq => Ok(Compare::Equal),
                            ast::CmpOp::NotEq => Ok(Compare::NotEqual),
                            _ => Err(error(self.source, expression, "ForwardRef has no ordering")),
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    return Ok(Expr {
                        ty: Type::Bool,
                        kind: ExprKind::Compare(values, ops),
                    });
                }
                let mut values = self.numeric_union_comparison(values);
                if let Some(result) =
                    calendar::equality(self.types, &mut values, compare.ops.as_ref())
                {
                    return Ok(result);
                }
                if values
                    .iter()
                    .any(|value| matches!(value.ty, Type::CmpKey(..)))
                {
                    return self.cmp_key_chain(values, &compare.ops, expression);
                }
                if let ([left, right], [op]) = (values.as_slice(), compare.ops.as_ref())
                    && let Some(result) = inspect::equality(left, right, *op)
                {
                    return Ok(result);
                }
                if compare
                    .ops
                    .iter()
                    .all(|op| matches!(op, ast::CmpOp::Eq | ast::CmpOp::NotEq))
                {
                    for value in &mut values {
                        if let Type::Class(class) = value.ty
                            && self.native_mock(class)
                        {
                            *value = self.mock_node(value.clone(), class);
                        }
                    }
                }
                if values.iter().any(|value| {
                    matches!(
                        value.ty,
                        Type::JsonRoot | Type::JsonValue | Type::JsonObject
                    )
                }) {
                    return json::compare(values, &compare.ops, self.source, expression);
                }
                if let [op @ (ast::CmpOp::Eq | ast::CmpOp::NotEq)] = compare.ops.as_ref()
                    && let [left, right] = values.as_slice()
                    && (matches!(left.ty, Type::Enum(_) | Type::BuiltinClass)
                        || matches!(right.ty, Type::Enum(_) | Type::BuiltinClass))
                    && left.ty != right.ty
                    && [left.ty, right.ty].into_iter().all(|ty| {
                        self.types.payload(ty).is_none() && self.types.members(ty).is_none()
                    })
                {
                    return Ok(Expr {
                        ty: Type::Bool,
                        kind: ExprKind::Scope {
                            body: values.into_iter().map(Statement::Expr).collect(),
                            value: Box::new(Expr {
                                ty: Type::Bool,
                                kind: ExprKind::Bool(*op == ast::CmpOp::NotEq),
                            }),
                            clear: Vec::new(),
                        },
                    });
                }
                let scalar = |ty| match self.types.payload(ty).unwrap_or(ty) {
                    Type::StrLiteral(_) => Type::Str,
                    ty => ty,
                };
                if let [op @ (ast::CmpOp::Eq | ast::CmpOp::NotEq)] = compare.ops.as_ref()
                    && let [left, right] = values.as_slice()
                    && (self.types.payload(left.ty).is_some()
                        || self.types.payload(right.ty).is_some())
                    && matches!(scalar(left.ty), Type::Str | Type::Int)
                    && scalar(left.ty) == scalar(right.ty)
                {
                    let operation = if *op == ast::CmpOp::Eq {
                        Compare::Equal
                    } else {
                        Compare::NotEqual
                    };
                    return Ok(Expr {
                        ty: Type::Bool,
                        kind: ExprKind::Compare(values, vec![operation]),
                    });
                }
                if let [left, right] = values.as_slice()
                    && matches!(compare.ops.as_ref(), [ast::CmpOp::Eq | ast::CmpOp::NotEq])
                    && (self.types.payload(left.ty).is_some()
                        || self.types.payload(right.ty).is_some())
                    && let ty @ (Type::IntEnum(_) | Type::StrEnum(_) | Type::Enum(_)) =
                        self.types.payload(left.ty).unwrap_or(left.ty)
                    && self.types.payload(right.ty).unwrap_or(right.ty) == ty
                {
                    return Ok(Self::identity_result(
                        values,
                        compare.ops[0] == ast::CmpOp::NotEq,
                    ));
                }
                if values.iter().any(|value| value.ty == Type::ObjectValue) {
                    if !matches!(compare.ops.as_ref(), [ast::CmpOp::Eq | ast::CmpOp::NotEq])
                        || !values.iter().all(|v| {
                            self.forwarding_value_supported(v.ty)
                                || self.object_value_supported(v.ty)
                                || self.types.mapping(v.ty) == Some((Type::Int, Type::Str))
                        })
                    {
                        return Err(error(
                            self.source,
                            expression,
                            "object values support equality with scalars, tuples, typed lists, eligible dataclasses, or integer-to-string dictionaries",
                        ));
                    }
                    let reversed = values[0].ty != Type::ObjectValue;
                    let eq = Expr {
                        ty: Type::Bool,
                        kind: ExprKind::Builtin(Builtin::SystemExitCodeEqual { reversed }, values),
                    };
                    return Ok(
                        if matches!(compare.ops[0], ast::CmpOp::NotEq | ast::CmpOp::IsNot) {
                            Expr {
                                ty: Type::Bool,
                                kind: ExprKind::Not(Box::new(eq)),
                            }
                        } else {
                            eq
                        },
                    );
                }
                // Python StrEnum compares by its underlying str value, even
                // against raw str or a member of another enum. A same-class
                // equality chain can keep canonical native tags throughout.
                if values.iter().any(|v| matches!(v.ty, Type::StrEnum(_)))
                    && values
                        .iter()
                        .all(|v| matches!(v.ty, Type::Str | Type::StrEnum(_) | Type::StrLiteral(_)))
                    && !values.iter().all(|v| v.ty == values[0].ty)
                {
                    values = values.into_iter().map(Self::string_value).collect();
                }
                if values.iter().any(|v| matches!(v.ty, Type::IntEnum(_)))
                    && values
                        .iter()
                        .all(|v| matches!(v.ty, Type::IntEnum(_) | Type::Int | Type::Float))
                {
                    values = values.into_iter().map(Self::enum_integer).collect();
                }
                let mut ops = Vec::new();
                for (&op, pair) in compare.ops.iter().zip(values.windows(2)) {
                    let (ty, right) = (pair[0].ty, pair[1].ty);
                    if ty == Type::Context || right == Type::Context {
                        return Err(error(
                            self.source,
                            expression,
                            "Context mapping equality requires comparing arbitrary bound Python values",
                        ));
                    }
                    if self.types.contains_module_spec(ty) || self.types.contains_module_spec(right)
                    {
                        return Err(error(
                            self.source,
                            expression,
                            "ModuleSpec rich comparison requires dynamic attribute protocol support",
                        ));
                    }
                    if let Some(order) = counter::comparison(self.types, ty, right, op) {
                        ops.push(order);
                        continue;
                    }
                    if (self.types.is_ordered_dict(ty) || self.types.is_ordered_dict(right))
                        && self.types.mapping(ty) == self.types.mapping(right)
                        && matches!(op, ast::CmpOp::Eq | ast::CmpOp::NotEq)
                    {
                        ops.push(if op == ast::CmpOp::Eq {
                            Compare::Equal
                        } else {
                            Compare::NotEqual
                        });
                        continue;
                    }
                    let numeric = matches!(ty, Type::Int | Type::Float)
                        && matches!(right, Type::Int | Type::Float);
                    let records = matches!(op, ast::CmpOp::Eq | ast::CmpOp::NotEq)
                        && matches!((ty, right), (Type::Class(_), Type::Class(_)));
                    let tuples = matches!(op, ast::CmpOp::Eq | ast::CmpOp::NotEq)
                        && matches!((ty, right), (Type::Tuple(_), Type::Tuple(_)))
                        && self.types.fields(ty) == self.types.fields(right);
                    let paths = ty.is_path() && right.is_path();
                    let variable_tuple = matches!(op, ast::CmpOp::Eq | ast::CmpOp::NotEq)
                        && matches!(
                            (ty, right),
                            (Type::VarTuple(_), Type::Tuple(_) | Type::VarTuple(_))
                                | (Type::Tuple(_), Type::VarTuple(_))
                        )
                        && self
                            .types
                            .var_tuple_element(ty)
                            .or_else(|| self.types.var_tuple_element(right))
                            .is_some_and(|elem| {
                                self.types.hashable_key(elem)
                                    && [ty, right].into_iter().all(|t| {
                                        self.types.var_tuple_element(t) == Some(elem)
                                            || self
                                                .types
                                                .fields(t)
                                                .is_some_and(|fs| fs.iter().all(|&f| f == elem))
                                    })
                            });
                    let dates = [ty, right].iter().all(|t| {
                        matches!(
                            t,
                            Type::Calendar(ir::CalendarType::Date | ir::CalendarType::DateTime)
                        )
                    });
                    if !numeric && !records && !paths && !tuples && !variable_tuple && !dates {
                        self.expect(ty, right, expression)?;
                    }
                    let ordered = numeric
                        || matches!(
                            ty,
                            Type::Bytes
                                | Type::ClockTime
                                | Type::Calendar(
                                    ir::CalendarType::Delta
                                        | ir::CalendarType::Date
                                        | ir::CalendarType::DateTime
                                )
                        )
                        || matches!(ty, Type::Tuple(_)) && self.types.ordered_key(ty);
                    let op = match op {
                        ast::CmpOp::Eq => Compare::Equal,
                        ast::CmpOp::NotEq => Compare::NotEqual,
                        ast::CmpOp::Lt if ordered => Compare::Less,
                        ast::CmpOp::LtE if ordered => Compare::LessEqual,
                        ast::CmpOp::Gt if ordered => Compare::Greater,
                        ast::CmpOp::GtE if ordered => Compare::GreaterEqual,
                        _ => {
                            return Err(error(
                                self.source,
                                expression,
                                "unsupported comparison for these types",
                            ));
                        }
                    };
                    if matches!(ty, Type::Patch(_)) {
                        return Err(error(
                            self.source,
                            expression,
                            "comparison of unified-diff objects is unsupported",
                        ));
                    }
                    if matches!(ty, Type::RegexPattern | Type::RegexMatch) {
                        return Err(error(
                            self.source,
                            expression,
                            "regex objects do not support value comparisons in Imre",
                        ));
                    }
                    let comparable_list = self.types.element(ty).is_some_and(|item| {
                        item == Type::Str
                            || self.types.is_value_struct(item)
                            || matches!(item, Type::Tuple(_)) && self.types.hashable_key(item)
                    });
                    // Ordinary dictionaries compare their entries independently of insertion order.
                    // Only enable value kinds whose Python equality has no user callbacks or boxed NaN identity.
                    let scalar_dict = self.types.mapping(ty).is_some_and(|(_, value)| {
                        matches!(
                            value,
                            Type::None
                                | Type::Bool
                                | Type::Int
                                | Type::Str
                                | Type::Bytes
                                | Type::BuiltinClass
                        )
                    });
                    let comparable = match ty {
                        Type::List(_) => comparable_list,
                        Type::Dict(_) => self.types.is_counter(ty) || scalar_dict,
                        Type::Tuple(_) => self.types.hashable_key(ty),
                        Type::VarTuple(_) => variable_tuple,
                        Type::ListInt
                        | Type::ChainMap(_)
                        | Type::StringKeys
                        | Type::Optional(_)
                        | Type::Union(_) => false,
                        Type::ForwardRef(_)
                        | Type::AnnotationValue(_)
                        | Type::Int
                        | Type::Float
                        | Type::StrEnum(_)
                        | Type::Enum(_)
                        | Type::Str
                        | Type::StrLiteral(_)
                        | Type::BuiltinClass
                        | Type::Path
                        | Type::PurePath(_)
                        | Type::FrozenStrings
                        | Type::Set(_)
                        | Type::Deque(_)
                        | Type::SimpleNamespace(_)
                        | Type::Bytes
                        | Type::Calendar(_)
                        | Type::ClockTime
                        | Type::Class(_)
                        | Type::Bool
                        | Type::None
                        | Type::TimedFileHandler
                        | Type::StrDescriptor(_)
                        | Type::PlainObject
                        | Type::PathInfo
                        | Type::Poll
                        | Type::Socket
                        | Type::ProcessSelectorClass
                        | Type::Hash
                        | Type::JsonDecoder
                        | Type::ZipFile
                        | Type::ZipInfo
                        | Type::CsvReader
                        | Type::Exception
                        | Type::NamedException(_)
                        | Type::ObjectValue
                        | Type::ReadOnlyObjectMapping
                        | Type::ImportException
                        | Type::TimeoutExpired
                        | Type::TextIO
                        | Type::StringIO
                        | Type::HandlerLock
                        | Type::CompletedProcess(..)
                        | Type::SourceModule
                        | Type::BytesIO
                        | Type::BinaryIO => true,
                        _ => {
                            return Err(error(
                                self.source,
                                expression,
                                "unsupported comparison for these types",
                            ));
                        }
                    };
                    if !comparable {
                        return Err(error(
                            self.source,
                            expression,
                            "container and optional value comparisons are unsupported",
                        ));
                    }
                    ops.push(op);
                }
                (Type::Bool, ExprKind::Compare(values, ops))
            }
            ast::Expr::Subscript(subscript) => {
                if let Some(member) = self.enum_name_lookup(subscript)? {
                    return Ok(member);
                }
                if let ast::Expr::Attribute(attr) = &*subscript.value
                    && attr.attr.as_str() == "exceptions"
                {
                    let value = self.expr(&attr.value)?;
                    if value.ty == Type::Exception {
                        let index = self.expr_expected(&subscript.slice, Type::Int)?;
                        return Ok(Expr {
                            ty: Type::Exception,
                            kind: ExprKind::Builtin(
                                Builtin::Process(ir::ProcessOperation::ExceptionAt),
                                vec![value, index],
                            ),
                        });
                    }
                }
                let list = Self::enum_text(self.expr(&subscript.value)?);
                if self.dict_or_record(list.ty).is_some() {
                    return self.dict_or_record_item(list, &subscript.slice, expression);
                }
                if self.mapping_class(list.ty).is_some()
                    || matches!(list.ty, Type::Class(class) if self.classes.methods[class].contains_key("__getitem__"))
                {
                    let key = self.expr(&subscript.slice)?;
                    return self.record_primitive(list, "__getitem__", &[key], expression);
                }
                if let Some(element) = self.types.deque_element(list.ty) {
                    let index = self.integer_index(&subscript.slice)?;
                    return Ok(Expr {
                        ty: element,
                        kind: ExprKind::Builtin(
                            Builtin::DequeGet { method: false },
                            vec![list, index],
                        ),
                    });
                }
                if list.ty == Type::BuiltinClass {
                    // Python evaluates type arguments before invoking __class_getitem__.
                    self.expr(&subscript.slice)?;
                    return Err(error(
                        self.source,
                        subscript,
                        "runtime generic aliases of builtin classes are unsupported",
                    ));
                }
                if list.ty == Type::Context {
                    return self.context_subscript(list, &subscript.slice);
                }
                if list.ty == Type::ModuleNamespace {
                    let key = self.expr_expected(&subscript.slice, Type::Str)?;
                    return Ok(errno::op(
                        ir::ErrnoOperation::Items,
                        Type::ObjectValue,
                        vec![list, key],
                    ));
                }
                if list.ty == Type::ReadOnlyObjectMapping {
                    let key = self.expr_expected(&subscript.slice, Type::Str)?;
                    return Ok(Expr {
                        ty: Type::ObjectValue,
                        kind: ExprKind::Builtin(Builtin::MappingObjectGet, vec![list, key]),
                    });
                }
                if list.ty == Type::ObjectValue {
                    let index = self.expr_expected(&subscript.slice, Type::Int)?;
                    // The runtime dispatches this exact integer access to the value's
                    // Python protocol and raises TypeError when it cannot be indexed.
                    self.graph
                        .checked_index_read(self.module, subscript.range());
                    return Ok(Expr {
                        ty: Type::ObjectValue,
                        kind: ExprKind::Builtin(Builtin::ExceptionFieldIndex, vec![list, index]),
                    });
                }
                let list = match list.ty {
                    documents::LIST => self.document_list(list),
                    Type::Document(_) => self.document_map(list),
                    _ => list,
                };
                if matches!(list.ty, Type::JsonRoot | Type::JsonValue | Type::JsonObject) {
                    let ast::Expr::StringLiteral(key) = subscript.slice.as_ref() else {
                        let index = self.expr(&subscript.slice)?;
                        if index.ty == Type::Int {
                            return Ok(Expr {
                                ty: Type::JsonValue,
                                kind: ExprKind::Builtin(Builtin::JsonIndex, vec![list, index]),
                            });
                        }
                        return Err(error(
                            self.source,
                            expression,
                            "readonly JSON lookups require a literal string key",
                        ));
                    };
                    check_string(key, self.source)?;
                    return Ok(json::select(
                        list,
                        Type::JsonValue,
                        Some(key.value.to_string()),
                        None,
                    ));
                }
                if let ast::Expr::Slice(slice) = &*subscript.slice {
                    return self.slice(list, slice);
                }
                if matches!(list.ty, Type::Str | Type::Bytes) {
                    let index = self.expr_expected(&subscript.slice, Type::Int)?;
                    let (ty, builtin) = if list.ty == Type::Str {
                        (Type::Str, Builtin::StringGet)
                    } else {
                        (Type::Int, Builtin::BytesGet)
                    };
                    return Ok(Expr {
                        ty,
                        kind: ExprKind::Builtin(builtin, vec![list, index]),
                    });
                }
                if let Some((key, value)) = self.types.chainmap_entries(list.ty) {
                    let key = self.expr_expected(&subscript.slice, key)?;
                    return Ok(Expr {
                        ty: value,
                        kind: ExprKind::Builtin(Builtin::ChainMapGetItem, vec![list, key]),
                    });
                }
                if let Some((key, value)) = self.types.mapping(list.ty) {
                    // Reading an ordinary enum-keyed dictionary may probe with an equal integer.
                    // A default factory can insert the supplied key, which changes its stored type.
                    let integer = (matches!(key, Type::IntEnum(_))
                        && !self.types.is_default_list_dict(list.ty)
                        && !self.types.is_counter(list.ty))
                    .then_some(Type::Int);
                    let key = self.expr_expected_or(&subscript.slice, key, integer)?;
                    let required = self.required_group_key(&list, &key);
                    let expression = self.projected_group(&list, &key).unwrap_or_else(|| Expr {
                        ty: value,
                        kind: ExprKind::Builtin(Builtin::DictGet, vec![list, key]),
                    });
                    return Ok(if required {
                        Expr {
                            ty: Type::Str,
                            kind: ExprKind::Unwrap(Box::new(expression)),
                        }
                    } else {
                        expression
                    });
                }
                if matches!(list.ty, Type::VarTuple(_)) {
                    let (literal, negative) = match &*subscript.slice {
                        ast::Expr::UnaryOp(u) if u.op == ast::UnaryOp::USub => (&*u.operand, true),
                        node => (node, false),
                    };
                    let position = match literal {
                        ast::Expr::NumberLiteral(n) if let ast::Number::Int(v) = &n.value => v
                            .to_string()
                            .parse::<i64>()
                            .ok()
                            .and_then(|i| if negative { i.checked_neg() } else { Some(i) }),
                        _ => None,
                    };
                    let element = self
                        .types
                        .display_tuple_index(list.ty, position)
                        .ok_or_else(|| {
                            error(
                                self.source,
                                &*subscript.slice,
                                "this tuple index can select different field types",
                            )
                        })?;
                    let index = self.expr_expected(&subscript.slice, Type::Int)?;
                    return Ok(Expr {
                        ty: element,
                        kind: ExprKind::Builtin(Builtin::TupleIndex, vec![list, index]),
                    });
                }
                if let Some(fields) = self.types.fields(list.ty) {
                    return self.fixed_tuple_item(list, &fields, &subscript.slice, expression);
                }
                let element = self.types.element(list.ty).ok_or_else(|| {
                    error(
                        self.source,
                        expression,
                        "indexing requires a typed list or fixed tuple",
                    )
                })?;
                let index = self.expr(&subscript.slice)?;
                self.expect(Type::Int, index.ty, expression)?;
                if let ExprKind::Field(tuple, _) = &list.kind
                    && self.types.fields(tuple.ty).is_some()
                    && self.graph.session.has_tuple_spreads(self.module)
                {
                    self.graph
                        .checked_index_read(self.module, subscript.range());
                }
                (
                    element,
                    ExprKind::Builtin(Builtin::ListGet, vec![list, index]),
                )
            }
            ast::Expr::Await(awaited) => {
                if !self.asynchronous {
                    return Err(error(
                        self.source,
                        awaited,
                        "await outside an async function",
                    ));
                }
                if let ast::Expr::Call(call) = &*awaited.value
                    && self.resolve(&call.func) == Some(Binding::Intrinsic(Intrinsic::Gather))
                {
                    return self.async_gather(call);
                }
                if let ast::Expr::Call(call) = &*awaited.value
                    && let Some(process) = self.async_process_await(call)?
                {
                    return Ok(process);
                }
                let value = self.expr(&awaited.value)?;
                return self.task_await(value, false, awaited);
            }
            ast::Expr::Attribute(attribute) => {
                if attribute.attr.as_str() == "MISSING"
                    && self.resolve(&attribute.value)
                        == Some(Binding::Intrinsic(Intrinsic::ContextToken))
                {
                    return Ok(contextvars::op(
                        ir::ContextOperation::Missing,
                        Type::ContextMissing,
                        vec![],
                    ));
                }
                if matches!(attribute.attr.as_str(), "machinery" | "util")
                    && self.resolve(&attribute.value)
                        == Some(Binding::IntrinsicModule(
                            crate::modules::IntrinsicModule::Importlib,
                        ))
                    && self.resolve(expression).is_none()
                {
                    return Err(error(
                        self.source,
                        attribute,
                        &format!(
                            "import importlib.{} before reading the {} attribute",
                            attribute.attr, attribute.attr
                        ),
                    ));
                }
                if let Some(class) = self.local_class(&attribute.value)? {
                    let value = match attribute.attr.as_str() {
                        "__name__" => class.name.as_str(),
                        "__qualname__" => class.qualname.as_str(),
                        "__module__" => self.runtime_module_name(self.module),
                        _ => {
                            return Err(error(
                                self.source,
                                attribute,
                                "local class attribute requires supported native metadata",
                            ));
                        }
                    };
                    return Ok(unpacking::string(value));
                }
                if let Some(flag) = self.command_flag(attribute)? {
                    return Ok(flag);
                }
                if let Some(text) = self.testcase_class_text(attribute)? {
                    return Ok(text);
                }
                if self.resolve(&attribute.value) == Some(Binding::Intrinsic(Intrinsic::StructTime))
                {
                    return self.struct_time_class_field(attribute);
                }
                if let Some((_, declaration)) = self.namedtuple_factory(&attribute.value) {
                    return self.namedtuple_class_attribute(attribute, &declaration);
                }
                if let Some(method) = ir::StrDescriptor::named(attribute.attr.as_str())
                    && matches!(&*attribute.value, ast::Expr::Name(n) if n.id == "str")
                    && !self.names.contains_key("str")
                    && self.resolve(&attribute.value).is_none()
                {
                    return Ok(inspect::descriptor(method));
                }
                if attribute.attr.as_str() == "join"
                    && matches!(&*attribute.value, ast::Expr::Name(n) if n.id == "str")
                    && !self.names.contains_key("str")
                    && self.resolve(&attribute.value).is_none()
                {
                    return Ok(self.string_join_value(None, attribute));
                }
                if let Some(Binding::Intrinsic(
                    stream @ (Intrinsic::Stdin | Intrinsic::OriginalStdout | Intrinsic::Stderr),
                )) = self.resolve(&attribute.value)
                    && matches!(attribute.attr.as_str(), "encoding" | "errors")
                {
                    let descriptor = match stream {
                        Intrinsic::Stdin => 0,
                        Intrinsic::OriginalStdout => 1,
                        Intrinsic::Stderr => 2,
                        _ => unreachable!(),
                    };
                    return Ok(Expr {
                        ty: Type::Str,
                        kind: ExprKind::Builtin(
                            Builtin::SysStreamText {
                                descriptor,
                                errors: attribute.attr.as_str() == "errors",
                            },
                            vec![],
                        ),
                    });
                }
                if let Some(class) = self.class_reference(&attribute.value) {
                    if let Some(member) = self.enum_member(class, attribute.attr.as_str()) {
                        return Ok(member);
                    }
                    return self.class_attribute(attribute, class, None, None);
                }
                let receiver = self.expr(&attribute.value)?;
                if let Type::WeakProxy(class) = receiver.ty
                    && attribute.attr.as_str() == "__class__"
                {
                    return self.dispatch_record(
                        class,
                        Self::weak_target(receiver),
                        attribute,
                        |checker, layout, _| Ok(checker.record_class_object(layout)),
                    );
                }
                if let Some(layout) = self.types.dispatcher(receiver.ty)
                    && let Some(value) =
                        self.dispatch_attribute(&receiver, layout, attribute.attr.as_str())
                {
                    return Ok(value);
                }
                if self.types.has_reflected_functions()
                    && matches!(receiver.ty, Type::FunctionRef | Type::Callable(_))
                    && let Some(index) = typing::attribute_index(attribute.attr.as_str())
                {
                    return Ok(Expr {
                        ty: Type::Str,
                        kind: ExprKind::Builtin(
                            Builtin::Typing(ir::TypingOperation::Attr(index)),
                            vec![receiver],
                        ),
                    });
                }
                if matches!(receiver.ty, Type::AnnotationValue(_)) {
                    return self.annotation_attribute(receiver, attribute);
                }
                if let Type::ChainMap(id) = receiver.ty
                    && attribute.attr.as_str() == "maps"
                {
                    return Ok(Expr {
                        ty: self.types.list(Type::Dict(id)),
                        kind: ExprKind::Builtin(Builtin::ChainMapMaps, vec![receiver]),
                    });
                }
                if receiver.ty == Type::SourceModule
                    && matches!(attribute.attr.as_str(), "__name__" | "__file__")
                {
                    return Ok(Expr {
                        ty: Type::Str,
                        kind: ExprKind::Builtin(
                            Builtin::SourceModuleAttribute {
                                filename: attribute.attr.as_str() == "__file__",
                            },
                            vec![receiver],
                        ),
                    });
                }
                if attribute.attr.as_str() == "default_factory"
                    && self.types.is_default_list_dict(receiver.ty)
                {
                    return self.default_factory(receiver, attribute);
                }
                if let Type::ForwardRef(layout) = receiver.ty {
                    return self.forward_ref_attribute(layout, receiver, attribute);
                }
                if let Some(wrapped) = self.wrapped_function(&receiver, attribute.attr.as_str()) {
                    return Ok(wrapped);
                }
                if let Some(field) = self.partial_attribute(&receiver, attribute.attr.as_str()) {
                    return Ok(field);
                }
                if let Type::Placeholder(kind) = receiver.ty {
                    let result = match (kind, attribute.attr.as_str()) {
                        (ir::PlaceholderKind::Value, "__class__") => {
                            partials::singleton(ir::PlaceholderKind::Class)
                        }
                        (ir::PlaceholderKind::Class, "__class__") => {
                            annotations::builtin_type("type").unwrap()
                        }
                        (ir::PlaceholderKind::Class, "__name__" | "__qualname__") => {
                            unpacking::string("_PlaceholderType")
                        }
                        (ir::PlaceholderKind::Class, "__module__") => {
                            unpacking::string("functools")
                        }
                        _ => {
                            return Err(error(
                                self.source,
                                attribute,
                                "unsupported Placeholder attribute",
                            ));
                        }
                    };
                    return Ok(partials::after(receiver, result));
                }
                if let Type::CmpKey(frame, bound) = receiver.ty
                    && attribute.attr.as_str() == "obj"
                {
                    return Ok(self.cmp_key_obj(receiver, frame, bound));
                }
                if receiver.ty == Type::BuiltinClass
                    && matches!(
                        attribute.attr.as_str(),
                        "__name__" | "__qualname__" | "__module__"
                    )
                {
                    return Ok(Expr {
                        ty: Type::Str,
                        kind: ExprKind::Builtin(
                            Builtin::ClassNamePart {
                                module: attribute.attr.as_str() == "__module__",
                            },
                            vec![receiver],
                        ),
                    });
                }
                if let Type::StrDescriptor(method) = receiver.ty {
                    return self.descriptor_attribute(receiver, method, attribute);
                }
                if receiver.ty == Type::SysImplementation {
                    let index = crate::version::IMPLEMENTATION
                        .iter()
                        .position(|&name| name == attribute.attr.as_str())
                        .ok_or_else(|| {
                            error(
                                self.source,
                                attribute,
                                "unknown sys.implementation attribute",
                            )
                        })?;
                    return Ok(Expr {
                        ty: crate::version::implementation_fields(self.types)[index],
                        kind: ExprKind::Field(Box::new(receiver), index),
                    });
                }
                if let Some(field) = self.url_property(&receiver, attribute.attr.as_str()) {
                    return Ok(field);
                }
                if let Some(declaration) = self.types.tuple_metadata(receiver.ty) {
                    if self.types.time_attributes(receiver.ty).is_some() {
                        return self.struct_time_field(receiver, attribute, &declaration);
                    }
                    if self.types.is_stat_result(receiver.ty) {
                        return self.stat_result_attribute(receiver, attribute, &declaration);
                    }
                    return self.namedtuple_instance_attribute(attribute, receiver, &declaration);
                }
                if receiver.ty == Type::Str && attribute.attr.as_str() == "join" {
                    return Ok(self.string_join_value(Some(receiver), attribute));
                }
                if receiver.ty == Type::JsonDecoder && attribute.attr.as_str() == "decode" {
                    return self.json_load_callable(Some(receiver), None, attribute);
                }
                if attribute.attr.as_str() == "reverse" && self.types.element(receiver.ty).is_some()
                {
                    return Ok(self.bound_builtin(
                        attribute,
                        receiver,
                        Builtin::ListReverse,
                        Vec::new(),
                        Type::None,
                    ));
                }
                if let Some(element) = self.types.deque_element(receiver.ty) {
                    return self.deque_attribute(attribute, receiver, element);
                }
                if let Type::NamedException(class) = receiver.ty
                    && attribute.attr.as_str() == "args"
                {
                    let field =
                        self.classes.declarations.definitions[class].exception_argument_slot();
                    if self.exception_tuple_slot(class) {
                        return Ok(Expr {
                            ty: Type::ObjectValue,
                            kind: ExprKind::Field(
                                Box::new(Self::exception_record(receiver)),
                                field,
                            ),
                        });
                    }
                    let message = Expr {
                        ty: self.types.optional(Type::Str),
                        kind: ExprKind::Field(
                            Box::new(Self::exception_record(receiver)),
                            self.classes.fields[class].len() - 1,
                        ),
                    };
                    return Ok(Expr {
                        ty: self.types.tuple(vec![Type::Str]),
                        kind: ExprKind::Tuple(vec![Expr {
                            ty: Type::Str,
                            kind: ExprKind::Unwrap(Box::new(message)),
                        }]),
                    });
                }
                let receiver = if matches!(
                    attribute.attr.as_str(),
                    "__cause__" | "__suppress_context__"
                ) && matches!(receiver.ty, Type::NamedException(_))
                {
                    self.coerce(receiver, Type::Exception, attribute)?
                } else {
                    Self::exception_record(receiver)
                };
                let receiver = self.checked_record_receiver(Self::weak_target(receiver), attribute);
                if let Type::IntEnum(class) | Type::StrEnum(class) | Type::Enum(class) = receiver.ty
                {
                    match attribute.attr.as_str() {
                        "name" => {
                            return Ok(Expr {
                                ty: Type::Str,
                                kind: ExprKind::Builtin(Builtin::StrEnumName, vec![receiver]),
                            });
                        }
                        "value" if matches!(receiver.ty, Type::IntEnum(_)) => {
                            return Ok(Self::enum_integer(receiver));
                        }
                        "phrase" if matches!(receiver.ty, Type::IntEnum(_)) => {
                            return Ok(Expr {
                                ty: Type::Str,
                                kind: ExprKind::Builtin(Builtin::HttpEnumPhrase, vec![receiver]),
                            });
                        }
                        "description"
                            if self.classes.declarations.definitions[class]
                                .enum_members
                                .as_ref()
                                .is_some_and(|m| m.http.is_some()) =>
                        {
                            return Ok(Expr {
                                ty: Type::Str,
                                kind: ExprKind::Builtin(
                                    Builtin::HttpEnumDescription,
                                    vec![receiver],
                                ),
                            });
                        }
                        "value" => {
                            return Ok(Expr {
                                ty: Type::Str,
                                kind: ExprKind::Builtin(Builtin::StrEnumValue, vec![receiver]),
                            });
                        }
                        _ => return self.class_attribute(attribute, class, Some(receiver), None),
                    }
                }
                if receiver.ty == Type::NativeModule {
                    return Ok(self.errno_attribute(receiver, attribute.attr.as_str()));
                }
                if matches!(receiver.ty, Type::ZipFile | Type::ZipInfo) {
                    return self.zip_attribute(receiver, attribute);
                }
                if let Type::ModuleSpec(id) = receiver.ty {
                    return self.module_spec_attribute(receiver, id, attribute);
                }
                if let Type::SimpleNamespace(id) = receiver.ty {
                    return self.namespace_attribute(receiver, id, attribute);
                }
                if receiver.ty == Type::CsvReader && attribute.attr.as_str() == "line_num" {
                    return Ok(Expr {
                        ty: Type::Int,
                        kind: ExprKind::Builtin(Builtin::CsvLineNumber, vec![receiver]),
                    });
                }
                if receiver.ty == Type::CodecInfo && attribute.attr.as_str() == "name" {
                    return Ok(Expr {
                        ty: Type::Str,
                        kind: ExprKind::Builtin(Builtin::CodecName, vec![receiver]),
                    });
                }
                if receiver.ty == Type::CodecInfo && attribute.attr.as_str() == "streamwriter" {
                    return Ok(codecs::writer(
                        ir::WriterOperation::Class,
                        Type::CodecWriterClass,
                        vec![receiver],
                    ));
                }
                if matches!(receiver.ty, Type::CodecWriterClass | Type::CodecWriter) {
                    let (op, ty) = match (receiver.ty, attribute.attr.as_str()) {
                        (_, "encoding") => (ir::WriterOperation::Encoding, Type::Str),
                        (Type::CodecWriter, "errors") => (ir::WriterOperation::Errors, Type::Str),
                        (Type::CodecWriter, "stream") => {
                            (ir::WriterOperation::Stream, Type::BinaryIO)
                        }
                        _ => {
                            return Err(error(
                                self.source,
                                attribute,
                                "unsupported stream writer attribute",
                            ));
                        }
                    };
                    return Ok(codecs::writer(op, ty, vec![receiver]));
                }
                if receiver.ty == Type::Hash {
                    return self.hash_attribute(receiver, attribute);
                }
                if receiver.ty == Type::JsonDecoder && attribute.attr.as_str() == "memo" {
                    return Ok(Expr {
                        ty: self.types.dict(Type::Str, Type::Str),
                        kind: ExprKind::Builtin(Builtin::JsonDecoderMemo, vec![receiver]),
                    });
                }
                if let Type::Calendar(kind) = receiver.ty {
                    return self.calendar_attribute(kind, receiver, attribute);
                }
                if matches!(receiver.ty, Type::TimedFileHandler | Type::ClockTime) {
                    return self.timed_attribute(receiver, attribute);
                }
                if receiver.ty == Type::LogFormatter {
                    return self.formatter_attr(receiver, attribute);
                }
                if receiver.ty == Type::LogRecord {
                    return self.log_record_field(receiver, attribute);
                }
                if let Type::Patch(kind) = receiver.ty {
                    return self.patch_attribute(kind, receiver, attribute);
                }
                if receiver.ty == Type::LogManager {
                    return self.log_manager_attribute(receiver, attribute);
                }
                if receiver.ty == Type::Logger {
                    return self.logger_attribute(receiver, attribute);
                }
                if receiver.ty.is_path() {
                    return self.path_attribute(receiver, attribute);
                }
                if matches!(receiver.ty, Type::AsyncProcess(_)) {
                    return self.async_returncode(receiver, attribute);
                }
                if matches!(receiver.ty, Type::Popen(..) | Type::ActivePopen) {
                    return self.popen_attribute(receiver, attribute);
                }
                if matches!(receiver.ty, Type::CompletedProcess(..)) {
                    return self.process_attribute(receiver, attribute);
                }
                if matches!(
                    receiver.ty,
                    Type::Exception | Type::ImportException | Type::TimeoutExpired
                ) {
                    if attribute.attr.as_str() == "args"
                        && let ExprKind::Local(slot) = receiver.kind
                        && let Some(issue) = self
                            .captured_process_errors
                            .iter()
                            .rev()
                            .find(|capture| capture.slot == slot)
                            .and_then(|capture| capture.tree_issue)
                    {
                        return Ok(Expr {
                            ty: self.types.tuple(vec![self.types.list(issue)]),
                            kind: ExprKind::Builtin(
                                Builtin::Process(ir::ProcessOperation::TreeErrorArgs),
                                vec![receiver],
                            ),
                        });
                    }
                    let field = if receiver.ty == Type::TimeoutExpired {
                        self.timeout_error_attribute(&receiver, attribute)?
                    } else {
                        self.process_error_attribute(&receiver, attribute)
                    };
                    if let Some(value) = field {
                        return Ok(value);
                    }
                    let (ty, builtin) = match attribute.attr.as_str() {
                        "characters_written" => (
                            Type::Int,
                            Builtin::Process(ir::ProcessOperation::OsErrorField(4)),
                        ),
                        name @ ("errno" | "strerror" | "filename" | "filename2") => {
                            let (index, ty) = match name {
                                "errno" => (0, Type::Int),
                                "strerror" => (1, Type::Str),
                                _ => (
                                    if name == "filename" { 2 } else { 3 },
                                    self.types
                                        .union(&[Type::Str, Type::Bytes, Type::Path])
                                        .unwrap(),
                                ),
                            };
                            (
                                self.types.optional(ty),
                                Builtin::Process(ir::ProcessOperation::OsErrorField(index)),
                            )
                        }
                        field @ ("args" | "msg" | "name" | "path" | "name_from")
                            if receiver.ty == Type::ImportException =>
                        {
                            (
                                Type::ObjectValue,
                                Builtin::ImportErrorField(match field {
                                    "args" => 0,
                                    "msg" => 1,
                                    "name" => 2,
                                    "path" => 3,
                                    _ => 4,
                                }),
                            )
                        }
                        "args" => (Type::ObjectValue, Builtin::ExceptionArgs),
                        "code" => (Type::ObjectValue, Builtin::SystemExitCode),
                        "msg" => (Type::Str, Builtin::JsonErrorField(0)),
                        "doc" => (Type::Str, Builtin::JsonErrorField(1)),
                        "pos" => (Type::Int, Builtin::JsonErrorField(2)),
                        "lineno" => (Type::Int, Builtin::JsonErrorField(3)),
                        "colno" => (Type::Int, Builtin::JsonErrorField(4)),
                        "__cause__" => (
                            self.types.optional(Type::Exception),
                            Builtin::ExceptionCause,
                        ),
                        "__suppress_context__" => (Type::Bool, Builtin::ExceptionSuppressContext),
                        _ => {
                            return Err(error(
                                self.source,
                                attribute,
                                "unsupported exception attribute",
                            ));
                        }
                    };
                    return Ok(Expr {
                        ty,
                        kind: ExprKind::Builtin(builtin, vec![receiver]),
                    });
                }
                if let Some(value) = self.context_attribute(&receiver, attribute.attr.as_str()) {
                    return Ok(value);
                }
                if receiver.ty == Type::StringIO
                    && let Some(value) =
                        self.string_io_attribute(&receiver, attribute.attr.as_str())
                {
                    return Ok(value);
                }
                if matches!(attribute.attr.as_str(), "encoding" | "errors")
                    && matches!(
                        self.types.payload(receiver.ty).unwrap_or(receiver.ty),
                        Type::TextIO | Type::TextOutput
                    )
                {
                    return Ok(Expr {
                        ty: self.types.optional(Type::Str),
                        kind: ExprKind::Builtin(
                            Builtin::TextMetadata {
                                errors: attribute.attr.as_str() == "errors",
                            },
                            vec![self.optional_stream(receiver, attribute.attr.as_str())],
                        ),
                    });
                }
                if receiver.ty == Type::TextOutput && attribute.attr.as_str() == "closed" {
                    let file = Expr {
                        ty: Type::TextIO,
                        kind: ExprKind::Builtin(
                            Builtin::Logging(ir::LogOperation::StreamFile),
                            vec![receiver],
                        ),
                    };
                    return Ok(Expr {
                        ty: Type::Bool,
                        kind: ExprKind::Builtin(Builtin::TextClosed, vec![file]),
                    });
                }
                if matches!(
                    receiver.ty,
                    Type::TextIO | Type::StringIO | Type::BinaryIO | Type::BytesIO
                ) && attribute.attr.as_str() == "closed"
                {
                    return Ok(Expr {
                        ty: Type::Bool,
                        kind: ExprKind::Builtin(Builtin::TextClosed, vec![receiver]),
                    });
                }
                if let Some(field) = self.setup_field(receiver.ty, attribute.attr.as_str()) {
                    return self.read_setup_field(receiver, attribute, field);
                }
                if let Type::Class(class) = receiver.ty
                    && (self.classes.methods[class].contains_key(attribute.attr.as_str())
                        || matches!(
                            self.classes.declarations.definitions[class]
                                .static_bindings
                                .get(attribute.attr.as_str()),
                            Some(
                                crate::classes::ClassValue::Runtime { .. }
                                    | crate::classes::ClassValue::String(_)
                                    | crate::classes::ClassValue::Strings { .. }
                            )
                        ))
                {
                    return self.class_attribute(attribute, class, Some(receiver), None);
                }
                if let Type::Class(class) = receiver.ty
                    && self.native_mock(class)
                    && !Self::mock_state_member(attribute.attr.as_str())
                    && !self.classes.declarations.definitions[class]
                        .fields
                        .iter()
                        .any(|field| field.name == attribute.attr.as_str())
                {
                    return self.mock_child(class, receiver, attribute);
                }
                if let Some(bound) = self.bound_unittest_assertion(attribute, &receiver) {
                    return Ok(bound);
                }
                let (receiver, field, ty) = self.field_value(attribute, receiver)?;
                if let Some(name) = attribute.value.as_name_expr()
                    && let Some(&slot) = self.names.get(name.id.as_str())
                    && self.namespace_guards.contains(&(slot, field))
                    && let Some(payload) = self.types.payload(ty)
                {
                    return Ok(Expr {
                        ty: payload,
                        kind: ExprKind::Unwrap(Box::new(Expr {
                            ty,
                            kind: ExprKind::Field(Box::new(receiver), field),
                        })),
                    });
                }
                (ty, ExprKind::Field(Box::new(receiver), field))
            }
            ast::Expr::Call(_) | ast::Expr::BinOp(_) => unreachable!("dispatched by typed_expr"),
            ast::Expr::Generator(_) => {
                return Err(error(
                    self.source,
                    expression,
                    "generator expressions require an immediate for, comprehension, list, sum, min, max, any, or all consumer",
                ));
            }
            _ => {
                return Err(error(
                    self.source,
                    expression,
                    "unsupported expression in the Imre subset",
                ));
            }
        };
        let kind = match kind {
            ExprKind::Builtin(Builtin::RaiseException, mut operands) => {
                let exception = operands
                    .pop()
                    .expect("a failing expression carries an exception");
                operands.push(self.exception_origin(exception, expression));
                ExprKind::Builtin(Builtin::RaiseException, operands)
            }
            kind => kind,
        };
        Ok(Expr { ty, kind })
    }

    /// Preserves the complete adjacent-part sequence and converts each field
    /// before evaluating the next. Sealed conversions never resolve a local `str`.
    fn fstring(&mut self, value: &ast::ExprFString) -> Result<Expr, String> {
        let mut parts = Vec::new();
        for part in value.value.iter() {
            match part {
                ast::FStringPartRef::Literal(literal) => {
                    check_string_literal(literal, self.source)?;
                    parts.push(Expr {
                        ty: Type::Str,
                        kind: ExprKind::Str(literal.value.to_string()),
                    });
                }
                ast::FStringPartRef::FString(fstring) => {
                    for element in &fstring.elements {
                        let part = match element {
                            ast::InterpolatedStringElement::Literal(literal) => {
                                if !matches!(
                                    fstring.flags.prefix(),
                                    ast::str_prefix::FStringPrefix::Raw { .. }
                                ) {
                                    check_string_escapes(literal.range(), self.source)?;
                                }
                                Expr {
                                    ty: Type::Str,
                                    kind: ExprKind::Str(literal.value.to_string()),
                                }
                            }
                            ast::InterpolatedStringElement::Interpolation(field) => {
                                self.formatted_scalar(field)?
                            }
                        };
                        parts.push(part);
                    }
                }
            }
        }
        // A formatted string evaluates every interpolation before joining them.
        Ok(parts
            .into_iter()
            .rev()
            .reduce(|right, left| Expr {
                ty: Type::Str,
                kind: ExprKind::Binary(Binary::Concat, Box::new(left), Box::new(right)),
            })
            .unwrap_or_else(|| Expr {
                ty: Type::Str,
                kind: ExprKind::Str(String::new()),
            }))
    }

    fn call(&mut self, mut call: &ast::ExprCall) -> Result<Expr, String> {
        if let Some(value) = self.literal_attrgetter_call(call)? {
            return Ok(value);
        }
        if let Some(method) = self.request_method_lookup(call)? {
            return Ok(method);
        }

        if self.resolve(&call.func) == Some(Binding::Intrinsic(Intrinsic::BuiltinsAscii)) {
            return self.ascii_call(call);
        }
        if let Some(registration) = self.immediate_registration(call)? {
            return Ok(registration);
        }
        if call
            .func
            .as_name_expr()
            .is_some_and(|n| n.id == "object" && !self.names.contains_key("object"))
            && self.resolve(&call.func).is_none()
            && call.arguments.args.is_empty()
            && call.arguments.keywords.is_empty()
        {
            return Ok(Expr {
                ty: Type::PlainObject,
                kind: ExprKind::Builtin(Builtin::NewPlainObject, vec![]),
            });
        }
        // A method result becomes the next receiver before its arguments are
        // evaluated. Check such chains in that order without retaining a full
        // expression and call-dispatch frame for every intermediate receiver.
        let mut chain = Vec::new();
        while let ast::Expr::Attribute(method) = &*call.func
            && let ast::Expr::Call(inner) = &*method.value
            && inner.func.is_attribute_expr()
            && self.resolve(&inner.func).is_none()
            && !call.arguments.args.iter().any(ast::Expr::is_starred_expr)
            && call.arguments.keywords.iter().all(|kw| kw.arg.is_some())
        {
            chain.push(call);
            call = inner;
        }
        let mut value = self.call_on(call, None)?;
        for call in chain.into_iter().rev() {
            if matches!(value.ty, Type::StrLiteral(_)) {
                value = literals::view(value, Type::Str);
            }
            value = self.call_on(call, Some(value))?;
        }
        Ok(value)
    }

    fn call_on(
        &mut self,
        call: &ast::ExprCall,
        mut receiver: Option<Expr>,
    ) -> Result<Expr, String> {
        if self.resolve(&call.func) == Some(Binding::Intrinsic(Intrinsic::BuiltinsLen))
            || self.graph.mutable_builtin_len && self.unshadowed_len(&call.func)
        {
            let callable = self.builtin_len();
            return self.invoke(call, callable);
        }
        if receiver.is_none()
            && let ast::Expr::Call(config) = &*call.func
            && self.resolve(&config.func) == Some(Binding::Intrinsic(Intrinsic::Wraps))
        {
            return self.apply_wraps(config, call);
        }
        if receiver.is_none()
            && let Some(raised) = self.defaultdict_factory_error(call, Type::None)?
        {
            return Ok(raised);
        }
        if receiver.is_none()
            && let Some(result) = self.fromkeys_call(call, None)?
        {
            return Ok(result);
        }
        if receiver.is_none()
            && matches!(&*call.func,ast::Expr::Name(n) if n.id.as_str()=="len")
            && !self.names.contains_key("len")
            && self.resolve(&call.func).is_none()
            && call.arguments.keywords.is_empty()
            && let [arg] = call.arguments.args.as_ref()
            && self.resolve(arg) == Some(Binding::Intrinsic(Intrinsic::OverloadRegistry))
        {
            return Ok(Expr {
                ty: Type::Int,
                kind: ExprKind::Builtin(Builtin::Typing(ir::TypingOperation::RegistryLen), vec![]),
            });
        }
        if let Some(tuple) = self.fixed_tuple.filter(|proof| proof.at == call.range()) {
            return Ok(self.materialize_fixed_tuple(tuple));
        }
        if let Some(value) = self.cache_constructor(call)? {
            return Ok(value);
        }
        if let Some(value) = self.cache_control(call, receiver.as_ref())? {
            return Ok(value);
        }
        if let Some(Binding::Intrinsic(Intrinsic::MockCallList { mark })) = self.resolve(&call.func)
        {
            self.reject_builtin_keywords(call)?;
            let [values] = call.arguments.args.as_ref() else {
                return Err(error(self.source, call, "expected a mock call list"));
            };
            let values = self.expr(values)?;
            if !matches!(self.types.element(values.ty), Some(Type::Class(c)) if self.bundled_mock_class(c, "_Call"))
            {
                return Err(error(self.source, call, "expected a mock call list"));
            }
            return Ok(Expr {
                ty: Type::Bool,
                kind: ExprKind::Builtin(Builtin::MockCallList { mark }, vec![values]),
            });
        }
        if let Some(suite) = self.doctest_suite(call)? {
            return Ok(suite);
        }
        if let Some(value) = self.sys_call(call)? {
            return Ok(value);
        }
        if call.arguments.keywords.iter().any(|kw| kw.arg.is_none()) {
            let mut normalized = call.clone();
            normalized.arguments.keywords.retain(|kw| {
                if kw.arg.is_some() {
                    return true;
                }
                match &kw.value {
                    ast::Expr::Dict(d) if d.items.is_empty() => false,
                    ast::Expr::Name(n)
                        if self.graph.empty_keywords.contains(self.module, n)
                            && self
                                .names
                                .get(n.id.as_str())
                                .is_some_and(|slot| self.assigned.contains(slot)) =>
                    {
                        false
                    }
                    _ => true,
                }
            });
            if normalized.arguments.keywords.len() != call.arguments.keywords.len() {
                return self.call_on(&normalized, receiver);
            }
        }
        if self.unshadowed_builtin(&call.func, "dict") {
            return self.dict_constructor(call, None);
        }
        if call.arguments.keywords.iter().any(|kw| kw.arg.is_none())
            && !call.arguments.args.iter().any(ast::Expr::is_starred_expr)
        {
            // An import exception binds each display before initializing its fields.
            if let Some(kind) = self.exception_category(&call.func)
                && self.is_import_error(kind)
                && self.class_reference(&call.func).is_none()
            {
                return self.import_error(call, kind, None);
            }
            if let ast::Expr::Attribute(method) = &*call.func
                && method.attr.as_str() == "__init__"
                && crate::classes::super_method(call).is_none()
                && self.class_reference(&method.value).is_none()
                && self.exception_category(&method.value).is_none()
                && self.resolve(&call.func).is_none()
            {
                let target = match receiver.take() {
                    Some(value) => value,
                    None => self.expr(&method.value)?,
                };
                if target.ty == Type::ImportException {
                    return self.import_error(call, 41, Some(target));
                }
                receiver = Some(target);
            }
        }
        if let Some(expanded) = self.expand_closed_call(call) {
            return self.call_on(&expanded, receiver);
        }
        if self.resolve(&call.func) == Some(Binding::Intrinsic(Intrinsic::OrderedDict)) {
            return self.ordered_dict_constructor(call, None);
        }
        if let Some(Binding::Intrinsic(
            kind @ (Intrinsic::TomliLoads
            | Intrinsic::TomliLoad
            | Intrinsic::TomlDumps
            | Intrinsic::TomliDump),
        )) = self.resolve(&call.func)
        {
            return self.document_call(kind, call);
        }
        if let Some(expression) = self.parse_args(call)? {
            return Ok(expression);
        }
        self.call_with_key_view(call, None, receiver)
    }

    /// Evaluate the receiver, key and default before the mapping probes its table.
    fn dict_setdefault(
        &mut self,
        call: &ast::ExprCall,
        receiver: Expr,
        args: &[ast::Expr],
    ) -> Result<Expr, String> {
        self.reject_builtin_keywords(call)?;
        let Some((key_type, value_type)) = self.types.mapping(receiver.ty) else {
            return Err(error(
                self.source,
                call,
                "dict.setdefault requires a dictionary",
            ));
        };
        let [key, rest @ ..] = args else {
            return Err(error(self.source, call, "dict.setdefault requires a key"));
        };
        let key = self.expr_expected(key, key_type)?;
        let value = match rest {
            [default] => self.expr_expected(default, value_type)?,
            [] => self.coerce(
                Expr {
                    ty: Type::None,
                    kind: ExprKind::None,
                },
                value_type,
                call,
            )?,
            _ => {
                return Err(error(
                    self.source,
                    call,
                    "dict.setdefault takes at most two arguments",
                ));
            }
        };
        Ok(Expr {
            ty: value_type,
            kind: ExprKind::Builtin(Builtin::DictSetDefault, vec![receiver, key, value]),
        })
    }

    /// A direct `dict.keys()` operand roots the mapping until the operation.
    /// Other methods called `keys` still use ordinary class/module dispatch.
    fn union_operand(&mut self, source: &ast::Expr) -> Result<(Expr, Option<Type>), String> {
        let mut view = false;
        let value = match source {
            ast::Expr::Call(call) if matches!(&*call.func, ast::Expr::Attribute(attr) if attr.attr.as_str() == "keys") => {
                self.call_with_key_view(call, Some(&mut view), None)?
            }
            _ => self.expr(source)?,
        };
        let key = if view {
            self.types.mapping(value.ty).map(|(key, _)| key)
        } else {
            self.types.set_element(value.ty)
        };
        Ok((value, key))
    }

    fn call_with_key_view(
        &mut self,
        call: &ast::ExprCall,
        key_view: Option<&mut bool>,
        receiver: Option<Expr>,
    ) -> Result<Expr, String> {
        if let Some((origin, declaration)) = self.namedtuple_factory(&call.func) {
            return self.namedtuple_call(origin, &declaration, call);
        }
        if let Some((method, name)) = crate::classes::super_method(call)
            && self.unshadowed_builtin(name, "super")
        {
            if method.attr.as_str() == "__init__" {
                return self.call_super_initializer(call);
            }
            if let Some(base) = self.super_method_base {
                return self.call_super_method(call, method, base);
            }
        }
        if let Some(expr) = self.explicit_exception_initializer(call)? {
            return Ok(expr);
        }
        if let ast::Expr::Attribute(method) = &*call.func
            && method.attr.as_str() == "__missing__"
            && self.resolve(&method.value) == Some(Binding::Intrinsic(Intrinsic::DefaultDict))
        {
            let Some((owner, rest)) = call.arguments.args.split_first() else {
                return Err(error(
                    self.source,
                    call,
                    "defaultdict.__missing__ requires a receiver",
                ));
            };
            let receiver = self.expr(owner)?;
            return self.defaultdict_missing(call, receiver, rest);
        }
        if let ast::Expr::Attribute(method) = &*call.func
            && method.attr.as_str() == "most_common"
            && self.resolve(&method.value) == Some(Binding::Intrinsic(Intrinsic::Counter))
        {
            return self.counter_most_common(call, None);
        }
        if let ast::Expr::Attribute(method) = &*call.func
            && self.resolve(&method.value) == Some(Binding::Intrinsic(Intrinsic::Counter))
            && matches!(method.attr.as_str(), "subtract" | "update")
            && !calls::starred(call)
        {
            let op = if method.attr.as_str() == "subtract" {
                Binary::Subtract
            } else {
                Binary::Add
            };
            return self.counter_unbound_update(call, op, method.attr.as_str());
        }
        if let ast::Expr::Attribute(method) = &*call.func
            && method.attr.as_str() == "elements"
            && self.resolve(&method.value) == Some(Binding::Intrinsic(Intrinsic::Counter))
        {
            return self.counter_elements(call, None);
        }
        if let ast::Expr::Attribute(method) = &*call.func
            && method.attr.as_str() == "reverse"
        {
            if self.unshadowed_builtin(&method.value, "list") {
                return self.sequence_reverse(stdlib::ReverseSequence::List, None, call);
            }
            if self.resolve(&method.value) == Some(Binding::Intrinsic(Intrinsic::Deque)) {
                return self.sequence_reverse(stdlib::ReverseSequence::Deque, None, call);
            }
        }
        if let ast::Expr::Attribute(method) = &*call.func
            && matches!(method.attr.as_str(), "clear" | "setdefault")
            && (self.unshadowed_builtin(&method.value, "dict")
                || (method.attr.as_str() == "clear"
                    && self.unshadowed_builtin(&method.value, "list"))
                || matches!(
                    self.resolve(&method.value),
                    Some(Binding::Intrinsic(
                        Intrinsic::Counter | Intrinsic::DefaultDict | Intrinsic::OrderedDict
                    ))
                ))
        {
            self.reject_builtin_keywords(call)?;
            if method.attr.as_str() == "setdefault" {
                let Some((owner, args)) = call.arguments.args.split_first() else {
                    return Err(error(
                        self.source,
                        call,
                        "unbound dict.setdefault requires a dictionary",
                    ));
                };
                let receiver = self.expr(owner)?;
                return self.dict_setdefault(call, receiver, args);
            }
            let [owner] = call.arguments.args.as_ref() else {
                return Err(error(
                    self.source,
                    call,
                    "unbound clear requires one receiver",
                ));
            };
            let receiver = self.expr(owner)?;
            if self.unshadowed_builtin(&method.value, "list") {
                let receiver = self.json_mutation_receiver(receiver, owner)?;
                if self.types.element(receiver.ty).is_none() {
                    return Err(error(self.source, owner, "list.clear requires a list"));
                }
                return Ok(Expr {
                    ty: Type::None,
                    kind: ExprKind::Builtin(Builtin::ListClear, vec![receiver]),
                });
            }
            if self.types.mapping(receiver.ty).is_none() {
                return Err(error(
                    self.source,
                    owner,
                    "dict.clear requires a dictionary",
                ));
            }
            return Ok(Expr {
                ty: Type::None,
                kind: ExprKind::Builtin(Builtin::DictClear, vec![receiver]),
            });
        }
        if let Some(value) = self.construct_receiver_type(call)? {
            return Ok(value);
        }
        if let Some(copy) = self.dataclass_copy(call)? {
            return Ok(copy);
        }
        if self.resolve(&call.func) == Some(Binding::Intrinsic(Intrinsic::Partial)) {
            return self.partial(call);
        }
        if calls::starred(call) {
            match self.resolve(&call.func) {
                Some(Binding::Intrinsic(Intrinsic::Credential(name))) => {
                    return self.credential_call(call, name);
                }
                Some(Binding::Intrinsic(Intrinsic::Fd(op @ ir::FdOperation::Pipe2))) => {
                    return self.fd_call(call, op);
                }
                _ => {}
            }
            if let Some(Binding::Definition(definition)) = self.resolve(&call.func)
                && let Some(signature) = self.functions.get(&definition)
            {
                return self.source_call(call, &signature.clone(), None, 0);
            }
            if let ast::Expr::Attribute(method) = &*call.func
                && self.resolve(&call.func).is_none()
            {
                if let Some(class) = self.class_reference(&method.value) {
                    return self.class_method_call(call, method, class, None);
                }
                let object = match receiver {
                    Some(value) => value,
                    None => self.expr(&method.value)?,
                };
                if let Type::Class(class)
                | Type::IntEnum(class)
                | Type::StrEnum(class)
                | Type::Enum(class) = object.ty
                    && self.classes.methods[class].contains_key(method.attr.as_str())
                {
                    return self.class_method_call(call, method, class, Some(object));
                }
            }
            return Err(error(
                self.source,
                call,
                "argument unpacking requires a source-defined function or method",
            ));
        }
        if let Some(Binding::Intrinsic(Intrinsic::CachePath { reverse })) = self.resolve(&call.func)
        {
            return self.cache_path_call(call, reverse);
        }
        if self.resolve(&call.func) == Some(Binding::Intrinsic(Intrinsic::ModuleSpec)) {
            return self.module_spec(call);
        }
        if self.resolve(&call.func) == Some(Binding::Intrinsic(Intrinsic::SimpleNamespace)) {
            return self.simple_namespace(call);
        }
        calls::reject_unpacking(call, self.source)?;
        if matches!(&*call.func, ast::Expr::Name(name) if name.id == "dict" && !self.names.contains_key(name.id.as_str()))
            && self.resolve(&call.func).is_none()
            && call.arguments.args.is_empty()
            && !call.arguments.keywords.is_empty()
        {
            return self.dict_constructor(call, None);
        }
        if self.unshadowed_builtin(&call.func, "set")
            && let [input] = call.arguments.args.as_ref()
            && call.arguments.keywords.is_empty()
        {
            if pipelines::is_pipeline_expression(self, input)
                || matches!(input, ast::Expr::Call(producer) if self.unshadowed_builtin(&producer.func, "range"))
            {
                let iterable = self.expansion_input(input, None)?;
                let source = self.prepare_iteration(iterable, input, false)?;
                return self.collect_set(source, input);
            }
            let (source, key) = self.union_operand(input)?;
            if let Some(element) = self.types.cursor_element(source.ty) {
                let cursor = self.prepare_cursor(source, element);
                return self.collect_set(cursor, input);
            }
            let key = key.ok_or_else(|| {
                error(
                    self.source,
                    input,
                    "set() requires a dictionary key view or a typed set",
                )
            })?;
            return Ok(Expr {
                ty: self.types.set(key),
                kind: ExprKind::Builtin(Builtin::KeySet, vec![source]),
            });
        }
        if let Some(cached) = self.local_cached_value(&call.func) {
            return self.invoke(call, cached);
        }
        if let Some(binding) = self.local_function_binding(&call.func) {
            if binding.overload_stub {
                let callable = self.local(
                    binding
                        .value_slot
                        .expect("an overloaded definition has a stub object"),
                );
                return self.invoke(call, callable);
            }
            if let Some(slot) = binding.value_slot
                && self.types.dispatcher(self.locals[slot]).is_some()
            {
                return self.invoke(call, self.local(slot));
            }
            if let Some(definition) = binding.generic.clone() {
                return self.call_generic_awaitable(call, &definition);
            }
            let function = binding.target(self.source, &*call.func)?.clone();
            return self.call_local_function(call, &function);
        }
        if let ast::Expr::Attribute(method) = &*call.func
            && method.attr.as_str() == "keys"
            && let ast::Expr::Call(load) = &*method.value
            && self.resolve(&load.func) == Some(Binding::Intrinsic(Intrinsic::TomllibLoad))
        {
            self.reject_builtin_keywords(call)?;
            self.reject_builtin_keywords(load)?;
            let [stream] = load.arguments.args.as_ref() else {
                return Err(error(
                    self.source,
                    load,
                    "tomllib.load requires one binary stream",
                ));
            };
            if !call.arguments.args.is_empty() {
                return Err(error(self.source, call, "keys() takes no arguments"));
            }
            let stream = self.expr_expected(stream, Type::BinaryIO)?;
            let bytes = Expr {
                ty: Type::Bytes,
                kind: ExprKind::Builtin(Builtin::BinaryRead, vec![stream]),
            };
            let text = Expr {
                ty: Type::Str,
                kind: ExprKind::Builtin(Builtin::Decode, vec![bytes]),
            };
            return Ok(Expr {
                ty: Type::StringKeys,
                kind: ExprKind::Builtin(Builtin::TomlKeys, vec![text]),
            });
        }
        if let ast::Expr::Subscript(s) = &*call.func
            && self.resolve(&s.value) == Some(Binding::Intrinsic(Intrinsic::Deque))
        {
            let ty = annotation(
                &s.slice,
                self.graph,
                self.module,
                &self.classes.declarations,
                self.types,
            )?;
            return self.deque_new(call, Some(ty));
        }
        if let ast::Expr::Subscript(s) = &*call.func
            && self.resolve(&s.value) == Some(Binding::Intrinsic(Intrinsic::Counter))
        {
            let ty = annotation(
                &s.slice,
                self.graph,
                self.module,
                &self.classes.declarations,
                self.types,
            )?;
            return self.counter(call, None, Some(ty));
        }
        match self.resolve(&call.func) {
            Some(Binding::Intrinsic(Intrinsic::ForwardRef)) => return self.forward_ref(call),
            binding if typing::is_dummy(self.graph, binding) => {
                return self.invoke(call, typing::dummy());
            }
            Some(Binding::Intrinsic(
                op @ (Intrinsic::Overload | Intrinsic::GetOverloads | Intrinsic::ClearOverloads),
            )) => return self.typing_overload(call, op),
            Some(Binding::Intrinsic(Intrinsic::GetAnnotations)) => {
                return self.get_annotations(call);
            }
            Some(Binding::Intrinsic(Intrinsic::AnnotationTypeRepr)) => {
                return self.annotation_type_repr(call);
            }
            Some(Binding::Intrinsic(
                kind @ (Intrinsic::RunLoop
                | Intrinsic::NewLoop
                | Intrinsic::SetLoop
                | Intrinsic::EnsureTask
                | Intrinsic::TaskGroup
                | Intrinsic::Semaphore),
            )) => {
                return self.async_builtin(kind, call);
            }
            Some(Binding::Intrinsic(Intrinsic::Declaration(symbol))) => {
                return Err(error(
                    self.source,
                    call,
                    &format!("no native implementation for {symbol}"),
                ));
            }
            Some(Binding::TypeAlias(module, index))
                if self.graph.modules[module].aliases[index].constructor =>
            {
                self.reject_builtin_keywords(call)?;
                let [value] = call.arguments.args.as_ref() else {
                    return Err(error(
                        self.source,
                        call,
                        "NewType constructors take one positional value",
                    ));
                };
                return self.typed_expr(value);
            }
            Some(Binding::Intrinsic(Intrinsic::Placeholder)) => {
                return self.placeholder_call(
                    call,
                    partials::singleton(ir::PlaceholderKind::Value),
                    ir::PlaceholderKind::Value,
                );
            }
            Some(Binding::Intrinsic(Intrinsic::SingleDispatch)) => {
                return self.single_dispatch(call);
            }
            Some(Binding::Intrinsic(Intrinsic::Reduce)) => return self.reduce_call(call),
            Some(Binding::Intrinsic(Intrinsic::UpdateWrapper)) => return self.update_wrapper(call),
            Some(Binding::Intrinsic(Intrinsic::CmpToKey)) => return self.cmp_to_key(call),
            Some(Binding::Intrinsic(Intrinsic::Cast)) => {
                self.reject_builtin_keywords(call)?;
                if let [target, value] = call.arguments.args.as_ref()
                    && matches!(
                        self.resolve(target),
                        Some(Binding::Intrinsic(Intrinsic::BytesIo | Intrinsic::StringIo))
                    )
                {
                    // Python cast() returns its operand, including on a wrong assertion.
                    return self.typed_expr(value);
                }
                let [ast::Expr::StringLiteral(target), value] = call.arguments.args.as_ref() else {
                    return Err(error(
                        self.source,
                        call,
                        "typing.cast requires a quoted type and a value",
                    ));
                };
                // cast returns its second argument verbatim. The quoted type is a
                // runtime string constant; it does not check or change the value.
                check_string(target, self.source)?;
                return self.typed_expr(value);
            }
            Some(Binding::Intrinsic(Intrinsic::WeakProxy)) => {
                return self.weak_proxy(call);
            }
            Some(Binding::Intrinsic(Intrinsic::UnicodeNormalize { check })) => {
                return self.unicode_normalize(call, check);
            }
            Some(Binding::Intrinsic(Intrinsic::CodecLookup)) => return self.codec_lookup(call),
            Some(Binding::Intrinsic(Intrinsic::SocketGetFqdn)) => {
                let params = [Parameter {
                    name: "name".into(),
                    ty: Type::Str,
                    kind: ParameterKind::PositionalOrKeyword,
                    default: Some(Expr {
                        ty: Type::Str,
                        kind: ExprKind::Str(String::new()),
                    }),
                    sign_only: false,
                }];
                let args = self.arguments(call, &params, None)?;
                return Ok(self.ordered_builtin(
                    Builtin::Socket(ir::SocketOp::GetFqdn),
                    Type::Str,
                    args,
                ));
            }
            Some(Binding::Intrinsic(Intrinsic::Socket)) => return self.socket_constructor(call),
            Some(Binding::Intrinsic(Intrinsic::CodecGetWriter)) => {
                return self.codec_getwriter(call);
            }
            Some(Binding::Intrinsic(
                op @ (Intrinsic::Sha1 | Intrinsic::Sha256 | Intrinsic::HashNew),
            )) => {
                return self.hash_constructor(call, op);
            }
            Some(Binding::Intrinsic(Intrinsic::HmacCompareDigest)) => {
                return self.hmac_compare_digest(call);
            }
            Some(Binding::Intrinsic(Intrinsic::StringIo)) => return self.string_io(call),
            Some(Binding::Intrinsic(Intrinsic::ContextVar)) => {
                return self.context_constructor(call, None);
            }
            Some(Binding::Intrinsic(kind @ (Intrinsic::Context | Intrinsic::CopyContext))) => {
                if !call.arguments.args.is_empty() || !call.arguments.keywords.is_empty() {
                    return Err(error(
                        self.source,
                        call,
                        "Context() and copy_context() take no arguments",
                    ));
                }
                return Ok(contextvars::op(
                    if kind == Intrinsic::Context {
                        ir::ContextOperation::New
                    } else {
                        ir::ContextOperation::CopyCurrent
                    },
                    Type::Context,
                    vec![],
                ));
            }
            Some(Binding::Intrinsic(Intrinsic::BytesIo)) => {
                let args = self.arguments(
                    call,
                    &[Parameter {
                        name: "initial_bytes".into(),
                        ty: Type::Bytes,
                        kind: ParameterKind::PositionalOrKeyword,
                        default: Some(Expr {
                            ty: Type::Bytes,
                            kind: ExprKind::Bytes(Vec::new()),
                        }),
                        sign_only: false,
                    }],
                    None,
                )?;
                return Ok(self.ordered_builtin(Builtin::BytesIo, Type::BytesIO, args));
            }
            Some(Binding::Intrinsic(Intrinsic::ZipFile)) => return self.zipfile_constructor(call),
            Some(Binding::Intrinsic(Intrinsic::Deque)) => return self.deque_new(call, None),
            Some(Binding::Intrinsic(Intrinsic::Counter)) => return self.counter(call, None, None),
            Some(Binding::Intrinsic(Intrinsic::ChainMap)) => return self.chainmap(call, None),
            Some(Binding::Intrinsic(Intrinsic::CountElements)) => return self.count_elements(call),
            Some(Binding::Intrinsic(Intrinsic::AttrGetter)) => {
                return Err(self.runtime_attrgetter(call));
            }
            Some(Binding::Intrinsic(Intrinsic::LengthHint)) => {
                return self.cursor_length_hint(call);
            }
            Some(Binding::Intrinsic(Intrinsic::MathProd)) => return self.integer_product(call),
            Some(Binding::Intrinsic(Intrinsic::ShallowCopy)) => return self.shallow_copy(call),
            Some(Binding::Intrinsic(Intrinsic::DeepCopy)) => return self.deep_copy(call),
            Some(Binding::Intrinsic(Intrinsic::Repeat)) => return self.repeat_cursor(call),
            Some(Binding::Intrinsic(Intrinsic::Tee)) => return self.tee_cursor(call),
            Some(Binding::Intrinsic(Intrinsic::Count)) => return self.count_cursor(call),
            Some(Binding::Intrinsic(Intrinsic::Islice | Intrinsic::Cycle)) => {
                return self.source_adapter(call);
            }
            Some(Binding::Intrinsic(Intrinsic::GroupBy)) => {
                return Err(error(
                    self.source,
                    call,
                    "groupby requires direct for iteration with an immediate list(group)",
                ));
            }
            Some(Binding::Intrinsic(Intrinsic::GetEnv)) => return self.environ_get(call),
            Some(Binding::Intrinsic(Intrinsic::Strerror)) => {
                self.reject_builtin_keywords(call)?;
                let [code] = call.arguments.args.as_ref() else {
                    return Err(error(self.source, call, "os.strerror requires one integer"));
                };
                return Ok(Expr {
                    ty: Type::Str,
                    kind: ExprKind::Builtin(
                        Builtin::Process(ir::ProcessOperation::Strerror),
                        vec![self.expr_expected(code, Type::Int)?],
                    ),
                });
            }
            Some(Binding::Intrinsic(Intrinsic::Readiness(op))) => {
                return self.readiness_call(call, op);
            }
            Some(Binding::Intrinsic(Intrinsic::Fd(op))) => return self.fd_call(call, op),
            Some(Binding::Intrinsic(Intrinsic::Credential(name))) => {
                return self.credential_call(call, name);
            }
            Some(Binding::Intrinsic(Intrinsic::GetCwd)) => {
                self.arguments(call, &[], None)?;
                return Ok(Expr {
                    ty: Type::Str,
                    kind: ExprKind::Builtin(Builtin::Process(ir::ProcessOperation::GetCwd), vec![]),
                });
            }
            Some(Binding::Intrinsic(Intrinsic::Popen)) => return self.popen(call, None),
            Some(Binding::Intrinsic(Intrinsic::PopenCleanup)) => {
                self.arguments(call, &[], None)?;
                return Ok(Expr {
                    ty: Type::None,
                    kind: ExprKind::Builtin(
                        Builtin::Process(ir::ProcessOperation::PopenCleanup),
                        vec![],
                    ),
                });
            }
            Some(Binding::Intrinsic(Intrinsic::SubprocessCall)) => {
                return self.popen(call, Some(false));
            }
            Some(Binding::Intrinsic(Intrinsic::SubprocessCheckCall)) => {
                return self.popen(call, Some(true));
            }
            Some(Binding::Intrinsic(Intrinsic::SubprocessList2Cmdline)) => {
                return self.subprocess_list2cmdline(call);
            }
            Some(Binding::Intrinsic(Intrinsic::Random(op))) => return self.random_call(op, call),
            Some(Binding::Intrinsic(Intrinsic::RandomChoice { secure })) => {
                return self.random_choice(call, secure);
            }
            Some(Binding::Intrinsic(Intrinsic::Which)) => return self.shutil_which(call),
            Some(Binding::Intrinsic(Intrinsic::RmTree)) => return self.shutil_rmtree(call),
            Some(Binding::Intrinsic(Intrinsic::CopyTree)) => return self.shutil_copytree(call),
            Some(Binding::Intrinsic(Intrinsic::Move)) => return self.shutil_move(call, false),
            Some(Binding::Intrinsic(Intrinsic::DestInSrc)) => return self.shutil_move(call, true),
            Some(Binding::Intrinsic(Intrinsic::IgnorePatterns)) => {
                return self.shutil_ignore_patterns(call);
            }
            Some(Binding::Intrinsic(Intrinsic::Copy(op))) => return self.shutil_copy(op, call),
            Some(Binding::Intrinsic(
                symbol @ (Intrinsic::FsPath
                | Intrinsic::FsDecode
                | Intrinsic::FsEncode
                | Intrinsic::PosixNormcase),
            )) => {
                return self.public_fspath(call, symbol);
            }
            Some(Binding::Intrinsic(Intrinsic::LogFormatter)) => return self.log_formatter(call),
            Some(Binding::Intrinsic(Intrinsic::HandlerLock)) => {
                let args = self.arguments(call, &[], None)?;
                return Ok(self.ordered_builtin(
                    Builtin::Logging(ir::LogOperation::LockNew),
                    Type::HandlerLock,
                    args,
                ));
            }
            Some(Binding::Intrinsic(
                op @ (Intrinsic::HandlerRename
                | Intrinsic::HandlerLookup
                | Intrinsic::FileEncodingUsesUtf8
                | Intrinsic::HandlerFallback
                | Intrinsic::PrintActiveException),
            )) => {
                return self.handler_operation(op, call);
            }
            Some(Binding::Intrinsic(Intrinsic::MakeLogRecord)) => {
                return self.make_log_record(call);
            }
            Some(Binding::Intrinsic(Intrinsic::LogRecord)) => return self.log_record(call),
            Some(Binding::Intrinsic(Intrinsic::GetLogger)) => return self.get_logger(call),
            Some(Binding::Intrinsic(Intrinsic::Logger)) => return self.new_logger(call),
            Some(Binding::Intrinsic(Intrinsic::LogManager)) => return self.new_log_manager(call),
            Some(Binding::Intrinsic(
                symbol @ (Intrinsic::AddLevelName
                | Intrinsic::GetLevelName
                | Intrinsic::LoggingDisable
                | Intrinsic::LevelNamesMapping),
            )) => return self.logging_levels(symbol, call),
            Some(Binding::Intrinsic(Intrinsic::TimedFileHandler)) => {
                return self.timed_handler(call);
            }
            Some(Binding::Intrinsic(Intrinsic::ClockTime)) => return self.clock_time(call),
            Some(Binding::Intrinsic(Intrinsic::Calendar(kind))) => {
                return self.calendar_constructor(kind, call);
            }
            Some(Binding::Intrinsic(
                which @ (Intrinsic::EmailParseDate
                | Intrinsic::EmailFormatDateTime
                | Intrinsic::EmailFormatDate),
            )) => {
                return self.email_date(which, call);
            }
            Some(Binding::Intrinsic(Intrinsic::BasicConfig)) => {
                return self.configure_logging(call);
            }
            Some(Binding::Intrinsic(Intrinsic::DictConfig)) => {
                return self.logging_dictionary_config(call);
            }
            Some(Binding::Intrinsic(Intrinsic::FileConfig)) => {
                return self.logging_file_config(call);
            }
            Some(Binding::Intrinsic(Intrinsic::LogEmit(level))) => {
                return self.root_log(level, call);
            }
            Some(Binding::Intrinsic(Intrinsic::Linecache(op)))
                if op != ir::LinecacheOperation::Cache =>
            {
                return self.linecache_call(op, call);
            }
            Some(Binding::Intrinsic(Intrinsic::InspectIsMethodDescriptor)) => {
                return self.is_method_descriptor(call);
            }
            Some(Binding::Intrinsic(Intrinsic::InspectGetattrStatic)) => {
                return self.static_attribute(call);
            }
            Some(Binding::Intrinsic(Intrinsic::CsvReader)) => return self.csv_reader(call),
            Some(Binding::Intrinsic(Intrinsic::FormatterParser)) => {
                return self.format_cursor(call, false);
            }
            Some(Binding::Intrinsic(Intrinsic::FormatterFieldNameSplit)) => {
                return self.format_cursor(call, true);
            }
            Some(Binding::Intrinsic(Intrinsic::RegexCompile)) => return self.compile_regex(call),
            Some(Binding::Intrinsic(Intrinsic::WarningFilter { simple })) => {
                return self.warning_filter(call, simple);
            }
            Some(Binding::Intrinsic(Intrinsic::WarningDisplay { show })) => {
                return self.warning_display(call, show);
            }
            Some(Binding::Intrinsic(Intrinsic::WarningReset)) => {
                self.arguments(call, &[], None)?;
                return Ok(Expr {
                    ty: Type::None,
                    kind: ExprKind::Builtin(Builtin::WarningReset, vec![]),
                });
            }
            Some(Binding::Intrinsic(Intrinsic::ClockConverter(utc))) => {
                return self.clock_parts(call, utc);
            }
            Some(Binding::Intrinsic(Intrinsic::Mktime)) => return self.mktime(call),
            Some(Binding::Intrinsic(Intrinsic::StructTime)) => return self.struct_time_new(call),
            Some(Binding::Intrinsic(Intrinsic::TimeSleep)) => {
                self.reject_builtin_keywords(call)?;
                let [arg] = call.arguments.args.as_ref() else {
                    return Err(error(self.source, call, "time.sleep takes one number"));
                };
                let value = self.expr(arg)?;
                let seconds = match value.ty {
                    Type::Float => value,
                    Type::Int | Type::Bool => Expr {
                        ty: Type::Float,
                        kind: ExprKind::Builtin(Builtin::Float, vec![value]),
                    },
                    _ => {
                        return Err(error(
                            self.source,
                            arg,
                            "time.sleep requires an integer or float",
                        ));
                    }
                };
                return Ok(Expr {
                    ty: Type::None,
                    kind: ExprKind::Builtin(Builtin::TimeSleep, vec![seconds]),
                });
            }
            Some(Binding::Intrinsic(Intrinsic::WallTime)) if call.arguments.is_empty() => {
                return Ok(Expr {
                    ty: Type::Float,
                    kind: ExprKind::Builtin(Builtin::WallTime, vec![]),
                });
            }
            Some(Binding::Intrinsic(Intrinsic::Ndiff)) => {
                return Err(error(
                    self.source,
                    call,
                    "ndiff requires immediate materialization",
                ));
            }
            Some(Binding::Intrinsic(Intrinsic::PatchSet)) => return self.parse_patch(call),
            Some(Binding::Intrinsic(Intrinsic::RegexSearch)) => return self.search_regex(call),
            Some(Binding::Intrinsic(Intrinsic::Fnmatch(operation))) => {
                return self.fnmatch_call(operation, call);
            }
            Some(Binding::Intrinsic(Intrinsic::RegexSub)) => return self.sub_regex(call),
            Some(Binding::Intrinsic(
                intrinsic @ (Intrinsic::GetSysconfigPath | Intrinsic::OsPathExists),
            )) => {
                return self.system_path_call(intrinsic, call);
            }
            Some(Binding::Intrinsic(Intrinsic::Dedent)) => {
                return self.dedent_call(call);
            }
            Some(Binding::Intrinsic(Intrinsic::Adler32)) => return self.adler32(call),
            Some(Binding::Intrinsic(
                op @ (Intrinsic::BinasciiDecode | Intrinsic::BinasciiEncode),
            )) => {
                return self.binascii_base64(call, op == Intrinsic::BinasciiDecode);
            }
            Some(Binding::Intrinsic(op @ (Intrinsic::Base64Encode | Intrinsic::Base64Decode))) => {
                return self.base64_codec(call, op == Intrinsic::Base64Decode);
            }
            Some(Binding::Intrinsic(Intrinsic::UrlsafeB64encode)) => {
                let args = self.arguments(
                    call,
                    &[Parameter {
                        name: "s".into(),
                        ty: Type::Bytes,
                        kind: ParameterKind::PositionalOrKeyword,
                        default: None,
                        sign_only: false,
                    }],
                    None,
                )?;
                return Ok(self.ordered_builtin(Builtin::UrlsafeB64encode, Type::Bytes, args));
            }
            Some(Binding::Intrinsic(Intrinsic::UrlJoin)) => return self.url_join(call),
            Some(Binding::Intrinsic(Intrinsic::ParseQsl)) => return self.parse_qsl(call),
            Some(Binding::Intrinsic(Intrinsic::UrlSplit)) => return self.url_split(call),
            Some(Binding::Intrinsic(Intrinsic::UrlUnsplit)) => return self.url_unsplit(call),
            Some(Binding::Intrinsic(Intrinsic::UrlEncode)) => return self.url_encode(call),
            Some(Binding::Intrinsic(
                op @ (Intrinsic::Quote | Intrinsic::QuotePlus | Intrinsic::Unquote),
            )) => {
                return self.url_quoting(call, op);
            }
            Some(Binding::Intrinsic(Intrinsic::ShlexSplit)) => {
                return self.shlex_split(call);
            }
            Some(Binding::Intrinsic(op @ (Intrinsic::ShlexQuote | Intrinsic::ShlexJoin))) => {
                return self.shlex_call(call, op);
            }
            Some(Binding::Intrinsic(Intrinsic::TemporaryFile)) => return self.temporary_file(call),
            _ => {}
        }
        if let Some(Binding::Intrinsic(
            kind @ (Intrinsic::TemporaryDirectory | Intrinsic::NullContext),
        )) = self.resolve(&call.func)
        {
            return self.directory_manager(kind, call);
        }
        if matches!(
            self.resolve(&call.func),
            Some(Binding::Intrinsic(Intrinsic::SubprocessRun))
        ) {
            return self.process_run(call);
        }
        if let Some(Binding::Intrinsic(Intrinsic::ShellOutput { status })) =
            self.resolve(&call.func)
        {
            return self.shell_output(call, status);
        }
        if self.resolve(&call.func) == Some(Binding::Intrinsic(Intrinsic::CheckOutput)) {
            return self.process_check_output(call);
        }
        if let ast::Expr::Attribute(attribute) = &*call.func
            && matches!(
                self.resolve(&attribute.value),
                Some(Binding::Intrinsic(Intrinsic::Environ))
            )
        {
            return self.environ_method(attribute, call);
        }
        if let ast::Expr::Attribute(attr) = call.func.as_ref()
            && self.resolve(&attr.value)
                == Some(Binding::Intrinsic(Intrinsic::Calendar(
                    ir::CalendarType::DateTime,
                )))
        {
            match attr.attr.as_str() {
                "now" => return self.calendar_now(call),
                "combine" => return self.calendar_combine(call),
                _ => {}
            }
        }
        if let Some(class) = self.class_reference(&call.func) {
            return self.construct(call, class);
        }
        if let Some(Binding::Intrinsic(Intrinsic::PosixPath(operation))) = self.resolve(&call.func)
        {
            return self.posix_path(call, operation);
        }
        if matches!(
            self.resolve(&call.func),
            Some(Binding::Intrinsic(Intrinsic::Path))
        ) {
            return self.path_constructor(call, Type::Path);
        }
        if let Some(Binding::Intrinsic(Intrinsic::PurePath(flavor))) = self.resolve(&call.func) {
            return self.path_constructor(call, Type::PurePath(flavor));
        }
        if matches!(
            self.resolve(&call.func),
            Some(Binding::Intrinsic(Intrinsic::JsonDecode))
        ) {
            return self.json_decode(call);
        }
        if self.resolve(&call.func) == Some(Binding::Intrinsic(Intrinsic::JsonDecoderClass)) {
            self.reject_builtin_keywords(call)?;
            if !call.arguments.args.is_empty() {
                return Err(error(
                    self.source,
                    call,
                    "JSONDecoder expects keyword-only options",
                ));
            }
            return Ok(Expr {
                ty: Type::JsonDecoder,
                kind: ExprKind::Builtin(Builtin::JsonDecoderNew, vec![]),
            });
        }
        if let Some(Binding::Intrinsic(operation @ (Intrinsic::JsonLoads | Intrinsic::JsonLoad))) =
            self.resolve(&call.func)
        {
            return self.json_loads(call, operation == Intrinsic::JsonLoad);
        }
        if matches!(
            self.resolve(&call.func),
            Some(Binding::Intrinsic(Intrinsic::TomlDecode))
        ) {
            return self.toml_decode(call);
        }
        if let Some(Binding::Intrinsic(
            op @ (Intrinsic::JsonEncode | Intrinsic::JsonDumps | Intrinsic::JsonDump),
        )) = self.resolve(&call.func)
        {
            return self.json_encode(call, op);
        }
        let gzip = matches!(
            self.resolve(&call.func),
            Some(Binding::Intrinsic(Intrinsic::GzipOpen))
        );
        if gzip || matches!(&*call.func, ast::Expr::Name(name) if name.id == "open") {
            return self.open(call, gzip);
        }
        if self.unshadowed_builtin(&call.func, "issubclass") {
            let builtin_error = |expr: &ast::Expr| match self.collection_binding(expr) {
                Some(Binding::Intrinsic(_)) => self.exception_category(expr),
                None if let ast::Expr::Name(name) = expr
                    && self.unshadowed_builtin(expr, name.id.as_str()) =>
                {
                    exception_kind(name.id.as_str())
                }
                _ => None,
            };
            if call.arguments.keywords.is_empty()
                && let [child, parent] = call.arguments.args.as_slice()
                && let (Some(child), Some(parent)) = (builtin_error(child), builtin_error(parent))
            {
                return Ok(Expr {
                    ty: Type::Bool,
                    kind: ExprKind::Builtin(Builtin::ExceptionSubclass(child, parent), vec![]),
                });
            }
            if call.arguments.keywords.is_empty()
                && let [child, parent] = call.arguments.args.as_slice()
                && let (Some(child), Some(parent)) =
                    (self.class_reference(child), self.class_reference(parent))
            {
                let result = child == parent
                    || self.classes.declarations.definitions[child]
                        .bases
                        .contains(&parent);
                return Ok(Expr {
                    ty: Type::Bool,
                    kind: ExprKind::Bool(result),
                });
            }
            if let Some(answer) = self.mapping_issubclass(call)? {
                return Ok(answer);
            }
        }
        if matches!(&*call.func, ast::Expr::Name(name) if name.id == "isinstance") {
            return self.isinstance(call);
        }
        if matches!(&*call.func, ast::Expr::Name(name) if name.id == "issubclass") {
            return self.issubclass(call);
        }
        if let Some(kind) = self.exception_category(&call.func) {
            return self.simple_exception(call, kind);
        }
        if let Some(binding) = self.resolve(&call.func) {
            if let Binding::Value(_) = binding {
                let callable = self.expr(&call.func)?;
                return self.invoke(call, callable);
            }
            let Binding::Definition(definition) = binding else {
                return Err(error(self.source, call, "a module is not callable"));
            };
            if self.graph.is_unittest_main(definition) {
                return self.unittest_main(call, definition.module);
            }
            let signature = self
                .functions
                .get(&definition)
                .ok_or_else(|| error(self.source, call, "definition is not a callable function"))?
                .clone();
            if self.classes.producer_items.contains_key(&signature.id) {
                return Err(error(
                    self.source,
                    call,
                    "generator functions require direct iteration",
                ));
            }
            return self.source_call(call, &signature, None, 0);
        }
        if matches!(&*call.func, ast::Expr::Name(name) if self.names.contains_key(name.id.as_str()))
            || !matches!(&*call.func, ast::Expr::Name(_) | ast::Expr::Attribute(_))
        {
            let callable = self.expr(&call.func)?;
            if callable.ty == Type::CodecWriterClass {
                return self.codec_writer_new(callable, call);
            }
            return self.invoke(call, callable);
        }
        if let ast::Expr::Attribute(attribute) = &*call.func {
            if let Some(method) = ir::StrDescriptor::named(attribute.attr.as_str())
                && matches!(&*attribute.value, ast::Expr::Name(n) if n.id == "str")
                && !self.names.contains_key("str")
                && self.resolve(&attribute.value).is_none()
            {
                return self.invoke_descriptor(call, inspect::descriptor(method), method);
            }
            if attribute.attr.as_str() == "count"
                && let ast::Expr::Attribute(parts) = &*attribute.value
                && parts.attr.as_str() == "parts"
                && let Ok(path) = self.expr(&parts.value)
                && path.ty.is_path()
            {
                self.reject_builtin_keywords(call)?;
                let [component] = call.arguments.args.as_ref() else {
                    return Err(error(
                        self.source,
                        call,
                        "parts.count expects one component",
                    ));
                };
                let component = self.expr_expected(component, Type::Str)?;
                return Ok(paths::operation(
                    ir::PathOperation::CountPart,
                    Type::Int,
                    vec![path, component],
                ));
            }
            if let Some(class) = self.class_reference(&attribute.value) {
                if let Some(e) = self.exception_alias_call(call, attribute, class, None)? {
                    return Ok(e);
                }
                return self.class_method_call(call, attribute, class, None);
            }
            let receiver = match receiver {
                Some(value) => value,
                None => self.expr(&attribute.value)?,
            };
            if let Some(result) =
                self.url_result_method(&receiver, attribute.attr.as_str(), call)?
            {
                return Ok(result);
            }
            if let Some(layout) = self.types.dispatcher(receiver.ty) {
                return self.single_dispatch_method(call, attribute, receiver, layout);
            }
            if let Some(wrapped) = self.wrapped_function(&receiver, attribute.attr.as_str()) {
                return self.invoke(call, wrapped);
            }
            if attribute.attr.as_str() == "func"
                && let Some(func) = self.partial_attribute(&receiver, "func")
            {
                return self.invoke(call, func);
            }
            if receiver.ty == Type::ModuleNamespace
                && attribute.attr.as_str() == "keys"
                && call.arguments.is_empty()
            {
                return Ok(errno::op(
                    ir::ErrnoOperation::Keys,
                    Type::StringKeys,
                    vec![receiver],
                ));
            }
            if receiver.ty == Type::FrozenStrings
                && attribute.attr.as_str() == "copy"
                && call.arguments.is_empty()
            {
                return Ok(receiver);
            }
            if receiver.ty == Type::ImportException && attribute.attr.as_str() == "__init__" {
                return self.import_error(call, 41, Some(receiver));
            }
            let receiver = self.optional_stream(receiver, attribute.attr.as_str());
            if matches!(receiver.ty, Type::NamedException(_))
                && matches!(attribute.attr.as_str(), "__init__" | "__post_init__")
            {
                return Err(error(
                    self.source,
                    call,
                    "reinitializing an existing exception payload is unsupported",
                ));
            }
            let receiver = Self::exception_record(receiver);
            let receiver = self.checked_record_receiver(Self::weak_target(receiver), attribute);
            if attribute.attr.as_str() == "get" && self.dict_or_record(receiver.ty).is_some() {
                return self.dict_or_record_get(receiver, attribute, call);
            }
            if let Type::Class(class) = receiver.ty {
                if self.classes.declarations.definitions[class]
                    .exception_aliases
                    .contains_key(attribute.attr.as_str())
                {
                    return self
                        .exception_alias_call(call, attribute, class, Some(receiver))
                        .map(Option::unwrap);
                }
                if let Some(field) = self.classes.declarations.definitions[class]
                    .fields
                    .iter()
                    .position(|field| field.name == attribute.attr.as_str())
                    && let ty @ (Type::Class(target) | Type::WeakProxy(target)) =
                        self.classes.fields[class][field]
                    && (matches!(ty, Type::WeakProxy(_))
                        || self.classes.methods[target].contains_key("__call__"))
                {
                    return self.invoke(
                        call,
                        Expr {
                            ty,
                            kind: ExprKind::Field(Box::new(receiver), field),
                        },
                    );
                }
                return self.class_method_call(call, attribute, class, Some(receiver));
            }
            if let Type::IntEnum(class) | Type::StrEnum(class) | Type::Enum(class) = receiver.ty
                && self.classes.methods[class].contains_key(attribute.attr.as_str())
            {
                return self.class_method_call(call, attribute, class, Some(receiver));
            }
            let receiver = Self::enum_text(receiver);
            if matches!(receiver.ty, Type::Document(_)) {
                return self.document_method(call, attribute, receiver);
            }
            if let Some((key, value)) = self.types.chainmap_entries(receiver.ty)
                && attribute.attr.as_str() == "get"
            {
                return self.chainmap_get(call, receiver, key, value);
            }
            if matches!(
                receiver.ty,
                Type::JsonRoot | Type::JsonValue | Type::JsonObject
            ) && attribute.attr.as_str() == "get"
            {
                return self.json_get(receiver, call);
            }
            let receiver = if matches!(
                attribute.attr.as_str(),
                "append" | "pop" | "extend" | "sort" | "clear" | "reverse"
            ) {
                self.json_mutation_receiver(receiver, call)?
            } else {
                receiver
            };
            if receiver.ty.is_path() || receiver.ty == Type::PathInfo {
                return self.path_method(receiver, attribute, call);
            }
            if matches!(receiver.ty, Type::Popen(..) | Type::ActivePopen) {
                return self.popen_method(receiver, attribute, call);
            }
            if matches!(receiver.ty, Type::CompletedProcess(..)) {
                return self.process_method(receiver, attribute, call);
            }
            if matches!(
                receiver.ty,
                Type::TaskGroup | Type::EventLoop | Type::Task(_)
            ) {
                return self.task_method(receiver, attribute, call);
            }
            if let Some(element) = self.types.deque_element(receiver.ty) {
                return self.deque_method(call, attribute, receiver, element);
            }
            if matches!(receiver.ty, Type::RegexPattern | Type::RegexMatch) {
                return self.regex_method(receiver, attribute, call);
            }
            if matches!(receiver.ty, Type::Str | Type::StrLiteral(_))
                && attribute.attr.as_str() == "encode"
            {
                return self.str_encode(Self::string_value(receiver), call);
            }
            if receiver.ty == Type::CodecInfo {
                return self.codec_method(receiver, attribute, call);
            }
            if receiver.ty == Type::Poll {
                return self.poll_method(receiver, attribute, call);
            }
            if receiver.ty == Type::Socket {
                return self.socket_method(receiver, attribute, call);
            }
            if receiver.ty == Type::CodecWriter {
                return self.codec_writer_method(receiver, attribute, call);
            }
            if receiver.ty == Type::Hash {
                return self.hash_method(receiver, attribute, call);
            }
            if receiver.ty == Type::JsonDecoder && attribute.attr.as_str() == "decode" {
                let args = self.arguments(
                    call,
                    &[Parameter {
                        name: "s".into(),
                        ty: Type::Str,
                        kind: ParameterKind::PositionalOrKeyword,
                        default: None,
                        sign_only: false,
                    }],
                    Some(receiver),
                )?;
                return Ok(self.ordered_builtin(Builtin::JsonDecoderDecode, Type::JsonRoot, args));
            }
            if receiver.ty == Type::ZipInfo
                && attribute.attr.as_str() == "is_dir"
                && call.arguments.is_empty()
            {
                return Ok(Expr {
                    ty: Type::Bool,
                    kind: ExprKind::Builtin(Builtin::ZipField(ir::ZipField::IsDir), vec![receiver]),
                });
            }
            if receiver.ty == Type::ZipFile {
                return self.zipfile_method(receiver, attribute, call);
            }
            if let Type::Calendar(kind) = receiver.ty {
                return self.calendar_method(kind, receiver, attribute, call);
            }
            if receiver.ty == Type::TimedFileHandler {
                return self.timed_method(receiver, attribute, call);
            }
            if receiver.ty == Type::LogFormatter {
                return self.formatter_method(receiver, attribute, call);
            }
            if receiver.ty == Type::LogRecord {
                return self.log_record_method(receiver, attribute, call);
            }
            if receiver.ty == Type::HandlerLock {
                return self.handler_lock_method(receiver, attribute.attr.as_str(), call);
            }
            if receiver.ty == Type::Logger {
                return self.logger_method(receiver, attribute, call);
            }
            if matches!(
                attribute.attr.as_str(),
                "splitlines" | "isalnum" | "isspace" | "isupper"
            ) && matches!(receiver.ty, Type::Str | Type::StrLiteral(_))
            {
                return self.splitlines_or_predicate(
                    Self::string_value(receiver),
                    attribute.attr.as_str(),
                    call,
                );
            }
            if attribute.attr.as_str() == "expandtabs"
                && matches!(receiver.ty, Type::Str | Type::StrLiteral(_))
            {
                return self.expandtabs(Self::string_value(receiver), call);
            }
            if matches!(attribute.attr.as_str(), "lower" | "upper" | "capitalize")
                && matches!(receiver.ty, Type::Str | Type::StrLiteral(_))
            {
                return self.text_method(
                    Self::string_value(receiver),
                    attribute.attr.as_str(),
                    call,
                );
            }
            if attribute.attr.as_str() == "format"
                && matches!(receiver.ty, Type::Str | Type::StrLiteral(_))
            {
                return self.named_format_call(Self::string_value(receiver), call);
            }
            if attribute.attr.as_str() == "translate"
                && matches!(receiver.ty, Type::Str | Type::StrLiteral(_))
            {
                return self.translate_method(Self::string_value(receiver), call);
            }
            if receiver.ty == Type::Str {
                match attribute.attr.as_str() {
                    "join" => return self.join_call(receiver, call),
                    "replace" => return self.replace_call(receiver, call),
                    "split" | "rsplit" | "strip" | "lstrip" | "rstrip" | "startswith"
                    | "endswith" | "find" | "partition" => {
                        return self.text_method(receiver, attribute.attr.as_str(), call);
                    }
                    _ => {}
                }
            }
            if attribute.attr.as_str() == "__missing__"
                && self.types.is_default_list_dict(receiver.ty)
            {
                return self.defaultdict_missing(call, receiver, &call.arguments.args);
            }
            if self.types.is_counter(receiver.ty) {
                match attribute.attr.as_str() {
                    "elements" => return self.counter_elements(call, Some(receiver)),
                    "most_common" => return self.counter_most_common(call, Some(receiver)),
                    "update" => return self.counter(call, Some(receiver), None),
                    "subtract" => {
                        return self.counter_counts(call, Some(receiver), None, Binary::Subtract);
                    }
                    "total"
                        if call.arguments.args.is_empty() && call.arguments.keywords.is_empty() =>
                    {
                        return Ok(Expr {
                            ty: Type::Int,
                            kind: ExprKind::Builtin(Builtin::CounterTotal, vec![receiver]),
                        });
                    }
                    _ => {}
                }
            }
            if attribute.attr.as_str() == "update"
                && let Some((key, value)) = self.types.mapping(receiver.ty)
            {
                return self.dict_update(call, receiver, key, value);
            }
            if attribute.attr.as_str() == "reverse" && self.types.element(receiver.ty).is_some() {
                return self.sequence_reverse(stdlib::ReverseSequence::List, Some(receiver), call);
            }
            if attribute.attr.as_str() == "sort" && self.types.element(receiver.ty).is_some() {
                return self.list_sort(receiver, call);
            }
            if attribute.attr.as_str() == "union"
                && let Some(key) = self.types.set_element(receiver.ty)
            {
                self.reject_builtin_keywords(call)?;
                let [arg] = call.arguments.args.as_ref() else {
                    return Err(error(
                        self.source,
                        call,
                        "set.union requires one set or mapping key view",
                    ));
                };
                let (other, other_key) = self.union_operand(arg)?;
                if other_key != Some(key) {
                    return Err(error(
                        self.source,
                        arg,
                        "set.union requires matching sets or mapping keys",
                    ));
                }
                return Ok(Expr {
                    ty: receiver.ty,
                    kind: ExprKind::Builtin(Builtin::KeyUnion, vec![receiver, other]),
                });
            }
            if matches!(
                receiver.ty,
                Type::Context | Type::ContextVar(_) | Type::ContextToken(_)
            ) {
                return self.context_method(receiver, attribute.attr.as_str(), call);
            }
            if receiver.ty == Type::TextOutput {
                return self.text_output_method(receiver, attribute, call);
            }
            if receiver.ty == Type::StringIO
                && matches!(
                    attribute.attr.as_str(),
                    "getvalue" | "read" | "readline" | "readlines" | "seek" | "tell" | "truncate"
                )
            {
                return self.string_io_method(receiver, attribute.attr.as_str(), call);
            }
            self.reject_builtin_keywords(call)?;
            if receiver.ty == Type::StringIO
                && let Some(value) =
                    self.string_io_protocol(&receiver, attribute.attr.as_str(), call)?
            {
                return Ok(value);
            }
            if matches!(
                receiver.ty,
                Type::TextIO | Type::StringIO | Type::BinaryIO | Type::BytesIO
            ) {
                let binary = matches!(receiver.ty, Type::BinaryIO | Type::BytesIO);
                let data_type = if binary { Type::Bytes } else { Type::Str };
                if binary
                    && attribute.attr.as_str() == "seek"
                    && let [offset, rest @ ..] = call.arguments.args.as_ref()
                    && rest.len() <= 1
                {
                    let offset = self.expr_expected(offset, Type::Int)?;
                    let whence = match rest {
                        [whence] => self.expr_expected(whence, Type::Int)?,
                        _ => Expr {
                            ty: Type::Int,
                            kind: ExprKind::Int("0".into()),
                        },
                    };
                    return Ok(Expr {
                        ty: Type::Int,
                        kind: ExprKind::Builtin(
                            Builtin::BinarySeek,
                            vec![receiver, offset, whence],
                        ),
                    });
                }
                let (builtin, ty) = match (attribute.attr.as_str(), call.arguments.args.len()) {
                    ("read", 0) => (
                        if binary {
                            Builtin::BinaryRead
                        } else {
                            Builtin::TextRead
                        },
                        data_type,
                    ),
                    ("read", 1) if binary => (Builtin::BinaryReadSize, Type::Bytes),
                    ("tell", 0) if binary => (Builtin::BinaryTell, Type::Int),
                    ("getvalue", 0) if binary => (Builtin::BytesIoValue, Type::Bytes),
                    ("readline", 0) => (Builtin::FileReadline, data_type),
                    ("write", 1) => (
                        if binary {
                            Builtin::BinaryWrite
                        } else {
                            Builtin::TextWrite
                        },
                        Type::Int,
                    ),
                    ("flush", 0) => (Builtin::TextFlush, Type::None),
                    ("close", 0) => (Builtin::TextClose, Type::None),
                    ("fileno", 0) => (Builtin::FileFileno, Type::Int),
                    _ => {
                        return Err(error(
                            self.source,
                            call,
                            "supported file methods are read(), binary read(size), readline(), write(data), flush(), and close()",
                        ));
                    }
                };
                let mut args = vec![receiver];
                if let Some(value) = call.arguments.args.first() {
                    let parameter = if matches!(builtin, Builtin::BinaryReadSize) {
                        Type::Int
                    } else {
                        data_type
                    };
                    args.push(self.expr_expected(value, parameter)?);
                }
                return Ok(Expr {
                    ty,
                    kind: ExprKind::Builtin(builtin, args),
                });
            }
            if receiver.ty == Type::Bytes
                && attribute.attr.as_str() == "decode"
                && matches!(call.arguments.args.as_ref(), [ast::Expr::StringLiteral(s)] if s.value.to_str().eq_ignore_ascii_case("ascii"))
            {
                return Ok(Expr {
                    ty: Type::Str,
                    kind: ExprKind::Builtin(Builtin::DecodeAscii, vec![receiver]),
                });
            }
            if receiver.ty == Type::Bytes
                && attribute.attr.as_str() == "decode"
                && (call.arguments.args.is_empty()
                    || matches!(call.arguments.args.as_ref(), [ast::Expr::StringLiteral(s)] if matches!(s.value.to_str(), "utf-8" | "utf8" | "UTF-8" | "UTF8")))
            {
                return Ok(Expr {
                    ty: Type::Str,
                    kind: ExprKind::Builtin(Builtin::Decode, vec![receiver]),
                });
            }
            if let Some((key_type, value_type)) = self.types.mapping(receiver.ty) {
                if attribute.attr.as_str() == "fromkeys" {
                    return self.dict_fromkeys(call, None, Some(receiver));
                }
                if attribute.attr.as_str() == "copy" {
                    if !call.arguments.is_empty() {
                        return Err(error(self.source, call, "dict.copy() takes no arguments"));
                    }
                    return Ok(Expr {
                        ty: receiver.ty,
                        kind: ExprKind::Builtin(Builtin::DictCopy, vec![receiver]),
                    });
                }
                if attribute.attr.as_str() == "setdefault" {
                    return self.dict_setdefault(call, receiver, &call.arguments.args);
                }
                if attribute.attr.as_str() == "clear"
                    && call.arguments.args.is_empty()
                    && call.arguments.keywords.is_empty()
                {
                    return Ok(Expr {
                        ty: Type::None,
                        kind: ExprKind::Builtin(Builtin::DictClear, vec![receiver]),
                    });
                }
                if attribute.attr.as_str() == "keys"
                    && call.arguments.args.is_empty()
                    && let Some(view) = key_view
                {
                    *view = true;
                    return Ok(receiver);
                }
                let pop = attribute.attr.as_str() == "pop";
                if (!pop && attribute.attr.as_str() != "get")
                    || !(1..=2).contains(&call.arguments.args.len())
                {
                    return Err(error(
                        self.source,
                        call,
                        "supported dictionary methods are clear(), copy(), update(), get(key[, default]), pop(key[, default]), loop-head items(), and keys() in a key union",
                    ));
                }
                let integer = (!pop && matches!(key_type, Type::IntEnum(_))).then_some(Type::Int);
                let key = self.expr_expected_or(&call.arguments.args[0], key_type, integer)?;
                if !pop
                    && call.arguments.args.len() == 1
                    && let Some(capture) = self.projected_group(&receiver, &key)
                {
                    return Ok(capture);
                }
                let mut args = vec![receiver, key];
                let optional = self.types.optional(value_type);
                let optional_lookup = || {
                    if pop {
                        Builtin::DictTryPop(value_type)
                    } else {
                        Builtin::DictTryGet(value_type)
                    }
                };
                let with_default = || {
                    if pop {
                        Builtin::DictPop
                    } else {
                        Builtin::DictGetDefault
                    }
                };
                let (builtin, ty) = if let Some(default) = call.arguments.args.get(1) {
                    let default = self.expr_expected(default, optional)?;
                    if optional == value_type {
                        args.push(default);
                        (with_default(), value_type)
                    } else if let ExprKind::Present(default) = default.kind {
                        args.push(*default);
                        (with_default(), value_type)
                    } else {
                        args.push(default);
                        (optional_lookup(), optional)
                    }
                } else if pop {
                    (Builtin::DictPop, value_type)
                } else {
                    (optional_lookup(), optional)
                };
                return Ok(Expr {
                    ty,
                    kind: ExprKind::Builtin(builtin, args),
                });
            }
            if let Some(element) = self.types.set_element(receiver.ty) {
                let builtin = match attribute.attr.as_str() {
                    "add" if call.arguments.args.len() == 1 => Builtin::SetAdd,
                    "discard" if call.arguments.args.len() == 1 => Builtin::SetDiscard,
                    _ => {
                        return Err(error(
                            self.source,
                            call,
                            "supported set methods are add(value) and discard(value)",
                        ));
                    }
                };
                let value = self.expr_expected(&call.arguments.args[0], element)?;
                return Ok(Expr {
                    ty: Type::None,
                    kind: ExprKind::Builtin(builtin, vec![receiver, value]),
                });
            }
            if receiver.ty == Type::ObjectValue
                && matches!(
                    attribute.attr.as_str(),
                    "read" | "write" | "seek" | "tell" | "close" | "getvalue"
                )
            {
                return Err(error(
                    self.source,
                    call,
                    "stream methods on an object value require an isinstance guard for io.BytesIO or io.StringIO",
                ));
            }
            let element = self.types.element(receiver.ty).ok_or_else(|| {
                error(
                    self.source,
                    call,
                    "supported value methods are typed-list append/pop, dict.get, set.add/discard, and str.split()",
                )
            })?;
            let append = if self.joined_lines.writes_buffer(call.range()) {
                Builtin::JoinBufferAppend
            } else {
                Builtin::Append
            };
            if attribute.attr.as_str() == "extend" && call.arguments.args.len() == 1 {
                return self.extend_list(receiver, &call.arguments.args[0], call, append);
            }
            let mut args = vec![receiver];
            let (builtin, ty) = match attribute.attr.as_str() {
                "append" if call.arguments.args.len() == 1 => {
                    args.push(self.expr_expected(&call.arguments.args[0], element)?);
                    (append, Type::None)
                }
                "pop" if call.arguments.args.is_empty() => (Builtin::Pop, element),
                "clear" if call.arguments.args.is_empty() => (Builtin::ListClear, Type::None),
                _ => {
                    return Err(error(
                        self.source,
                        call,
                        "supported list methods are append(value), extend(iterable), pop(), and clear()",
                    ));
                }
            };
            return Ok(Expr {
                ty,
                kind: ExprKind::Builtin(builtin, args),
            });
        }
        let ast::Expr::Name(name) = &*call.func else {
            return Err(error(
                self.source,
                call,
                "only statically bound direct calls are supported",
            ));
        };
        // Passing stderr as a positional value displays the open text stream.
        if matches!(name.id.as_str(), "print" | "repr" | "str")
            && !self.names.contains_key(name.id.as_str())
            && self.resolve(&call.func).is_none()
            && call.arguments.keywords.is_empty()
            && call
                .arguments
                .args
                .iter()
                .any(|arg| self.resolve(arg) == Some(Binding::Intrinsic(Intrinsic::Stderr)))
            && (name.id == "print" || call.arguments.args.len() == 1)
        {
            let mut pieces = Vec::new();
            for arg in &call.arguments.args {
                if !pieces.is_empty() {
                    pieces.push(unpacking::string(" "));
                }
                pieces.push(
                    if self.resolve(arg) == Some(Binding::Intrinsic(Intrinsic::Stderr)) {
                        unpacking::concat(
                            unpacking::concat(
                                unpacking::string(
                                    "<_io.TextIOWrapper name='<stderr>' mode='w' encoding='",
                                ),
                                Expr {
                                    ty: Type::Str,
                                    kind: ExprKind::Builtin(
                                        Builtin::SysStreamText {
                                            descriptor: 2,
                                            errors: false,
                                        },
                                        vec![],
                                    ),
                                },
                            ),
                            unpacking::string("'>"),
                        )
                    } else {
                        self.expr_expected(arg, Type::Str)?
                    },
                );
            }
            let text = pieces.into_iter().reduce(unpacking::concat).unwrap();
            return Ok(if name.id == "print" {
                Expr {
                    ty: Type::None,
                    kind: ExprKind::Builtin(Builtin::Print, vec![text]),
                }
            } else {
                text
            });
        }
        if name.id == "print"
            && let [field] = call.arguments.keywords.as_slice()
            && field
                .arg
                .as_ref()
                .is_some_and(|name| name.as_str() == "file")
            && self.resolve(&field.value) == Some(Binding::Intrinsic(Intrinsic::Stderr))
            && call.arguments.args.len() <= 1
        {
            let text = match call.arguments.args.first() {
                Some(text) => self.expr_expected(text, Type::Str)?,
                None => unpacking::string(""),
            };
            return Ok(Expr {
                ty: Type::None,
                kind: ExprKind::Builtin(Builtin::StderrLine, vec![text]),
            });
        }
        match name.id.as_str() {
            "list" => return self.list_call(call, None),
            "sorted" => return self.sorted_call(call, None),
            "abs" => return self.abs_call(call),
            "pow" => return self.pow_call(call),
            "next" => return self.next_cursor(call),
            "iter" if call.arguments.args.len() == 1 => return self.iter_cursor(call),
            "sum" => return self.sum_call(call),
            "min" | "max" => return self.extremum_call(call, name.id == "min"),
            "any" | "all" => return self.quantifier_call(call, name.id == "all"),
            "map" | "filter" | "zip" | "reversed" | "enumerate" | "range" => {
                return Err(error(
                    self.source,
                    call,
                    "iterator values require an immediate for, comprehension, list, sum, any, or all consumer",
                ));
            }
            _ => {}
        }
        if name.id == "ord" && !self.names.contains_key("ord") && self.resolve(&call.func).is_none()
        {
            return self.ordinal_call(call);
        }
        if name.id == "ascii"
            && !self.names.contains_key("ascii")
            && self.resolve(&call.func).is_none()
        {
            return self.ascii_call(call);
        }
        self.reject_builtin_keywords(call)?;
        if name.id == "bool" {
            return match call.arguments.args.as_ref() {
                [] => Ok(Expr {
                    ty: Type::Bool,
                    kind: ExprKind::Bool(false),
                }),
                [value] => self.condition(value),
                _ => Err(error(
                    self.source,
                    call,
                    "bool accepts zero or one positional argument",
                )),
            };
        }
        if name.id == "bytes" {
            let value = match call.arguments.args.as_ref() {
                [] => {
                    return Ok(Expr {
                        ty: Type::Bytes,
                        kind: ExprKind::Bytes(Vec::new()),
                    });
                }
                [value @ (ast::Expr::List(_) | ast::Expr::ListComp(_))] => {
                    self.expr_expected(value, Type::ListInt)?
                }
                [value] => self.expr(value)?,
                _ => {
                    return Err(error(
                        self.source,
                        call,
                        "bytes accepts zero arguments or one int, bytes, integer list, or integer tuple argument",
                    ));
                }
            };
            let value = if self.types.var_tuple_element(value.ty) == Some(Type::Int)
                || self
                    .types
                    .fields(value.ty)
                    .is_some_and(|fields| fields.iter().all(|&f| f == Type::Int))
            {
                let input = self.iteration_value(value, false, call)?;
                self.collect_list(input, false, Some(Type::Int), call)?
            } else {
                value
            };
            if !matches!(value.ty, Type::Int | Type::ListInt | Type::Bytes) {
                return Err(error(
                    self.source,
                    call,
                    "bytes expects an int, bytes, integer list, or integer tuple",
                ));
            }
            return Ok(Expr {
                ty: Type::Bytes,
                kind: ExprKind::Builtin(Builtin::Bytes, vec![value]),
            });
        }
        let args = call
            .arguments
            .args
            .iter()
            .map(|arg| self.expr(arg))
            .collect::<Result<Vec<_>, _>>()?;
        if let [proxy] = args.as_slice()
            && matches!(proxy.ty, Type::WeakProxy(_))
            && let Some(value) = self.weak_proxy_builtin(name.id.as_str(), proxy.clone())
        {
            return Ok(value);
        }
        if name.id == "tuple"
            && self.unshadowed_builtin(&call.func, "tuple")
            && let [value] = args.as_slice()
            && let Some(fields) = self.types.library_tuple_fields(value.ty)
        {
            let mut scope = calls::ArgumentScope::default();
            let source = scope.hold(self, value.clone());
            let items = fields
                .iter()
                .enumerate()
                .map(|(i, &ty)| Expr {
                    ty,
                    kind: ExprKind::Field(Box::new(source.clone()), i),
                })
                .collect();
            return Ok(scope.finish(Expr {
                ty: self.types.tuple(fields),
                kind: ExprKind::Tuple(items),
            }));
        }
        if let [value] = args.as_slice()
            && let Type::Placeholder(kind) = value.ty
        {
            let result = match name.id.as_str() {
                "type" if kind == ir::PlaceholderKind::Value => {
                    Some(partials::singleton(ir::PlaceholderKind::Class))
                }
                "type" => annotations::builtin_type("type"),
                "callable" => Some(Expr {
                    ty: Type::Bool,
                    kind: ExprKind::Bool(kind == ir::PlaceholderKind::Class),
                }),
                _ => None,
            };
            if let Some(result) = result {
                return Ok(partials::after(value.clone(), result));
            }
        }
        if args.len() == 1
            && matches!(args[0].ty, Type::Calendar(_))
            && matches!(name.id.as_str(), "str" | "print" | "repr")
        {
            return Ok(calendar::format_builtin(name.id.as_str(), args));
        }
        if let [value] = args.as_slice()
            && let Some(result) = self.patch_builtin(name.id.as_str(), value)
        {
            return Ok(result);
        }
        if name.id == "hash" {
            let [value] = args.as_slice() else {
                return Err(error(self.source, call, "hash requires one argument"));
            };
            if matches!(value.ty, Type::CmpKey(..)) {
                return Ok(self.cmp_key_hash(value.clone()));
            }
            let builtin = match value.ty {
                Type::Str | Type::StrLiteral(_) => Builtin::HashStr,
                Type::Class(id)
                    if self.classes.declarations.definitions[id].toml_hash
                        || self.classes.declarations.definitions[id]
                            .dataclass
                            .is_some_and(|d| d.value_struct) =>
                {
                    Builtin::HashRecord
                }
                _ => {
                    return Err(error(
                        self.source,
                        call,
                        "hash requires a string or a known frozen hash projection",
                    ));
                }
            };
            return Ok(Expr {
                ty: Type::Int,
                kind: ExprKind::Builtin(builtin, args),
            });
        }
        if args.len() == 1
            && (((args[0].ty.is_path() || self.types.payload(args[0].ty) == Some(Type::Str))
                && matches!(name.id.as_str(), "str" | "print"))
                || (args[0].ty == Type::PathInfo
                    && matches!(name.id.as_str(), "str" | "repr" | "print")))
        {
            let text = if self.types.payload(args[0].ty) == Some(Type::Str) {
                self.optional_text(args.into_iter().next().unwrap())
            } else {
                let op = if args[0].ty == Type::PathInfo {
                    ir::PathOperation::InfoRepr
                } else {
                    ir::PathOperation::Text
                };
                paths::operation(op, Type::Str, args)
            };
            return Ok(if name.id != "print" {
                text
            } else {
                Expr {
                    ty: Type::None,
                    kind: ExprKind::Builtin(Builtin::Print, vec![text]),
                }
            });
        }
        if name.id == "int"
            && args.len() == 1
            && matches!(args[0].ty, Type::Union(_))
            && let Some(value) = self.int_enum_union_value(args[0].clone())
        {
            return Ok(value);
        }
        if args.len() == 1 && matches!(args[0].ty, Type::IntEnum(_)) && name.id == "int" {
            return Ok(Self::enum_integer(args.into_iter().next().unwrap()));
        }
        if let [class] = args.as_slice()
            && class.ty == Type::BuiltinClass
            && name.id == "type"
        {
            return Ok(
                self.evaluated_receiver(Some(class.clone()), annotations::class_value("type"))
            );
        }
        if args.len() == 1
            && matches!(args[0].ty, Type::Enum(_) | Type::BuiltinClass)
            && (matches!(name.id.as_str(), "str" | "print")
                || (args[0].ty == Type::BuiltinClass && name.id == "repr"))
        {
            let builtin = if args[0].ty == Type::BuiltinClass {
                Builtin::Repr
            } else {
                Builtin::EnumString
            };
            let text = Expr {
                ty: Type::Str,
                kind: ExprKind::Builtin(builtin, args),
            };
            return Ok(if name.id != "print" {
                text
            } else {
                Expr {
                    ty: Type::None,
                    kind: ExprKind::Builtin(Builtin::Print, vec![text]),
                }
            });
        }
        if args.len() == 1
            && (matches!(args[0].ty, Type::StrEnum(_))
                && matches!(name.id.as_str(), "str" | "print" | "len")
                || matches!(args[0].ty, Type::IntEnum(_))
                    && matches!(name.id.as_str(), "str" | "print"))
        {
            let text = Expr {
                ty: Type::Str,
                kind: ExprKind::Builtin(Builtin::StrEnumValue, args),
            };
            return Ok(match name.id.as_str() {
                "str" => text,
                "print" => Expr {
                    ty: Type::None,
                    kind: ExprKind::Builtin(Builtin::Print, vec![text]),
                },
                _ => Expr {
                    ty: Type::Int,
                    kind: ExprKind::Builtin(Builtin::Len, vec![text]),
                },
            });
        }
        if name.id == "frozenset"
            && let [value] = args.as_slice()
            && value.ty == Type::FrozenStrings
        {
            return Ok(value.clone());
        }
        if name.id == "frozenset"
            && args.len() == 1
            && self.types.element(args[0].ty) == Some(Type::Str)
        {
            return Ok(Expr {
                ty: Type::FrozenStrings,
                kind: ExprKind::Builtin(Builtin::FreezeStrings, args),
            });
        }
        if name.id == "divmod" && args.len() == 2 {
            let mut body = Vec::with_capacity(2);
            let mut slots = Vec::with_capacity(2);
            for arg in args {
                self.expect(Type::Int, arg.ty, call)?;
                let slot = self.hidden(Type::Int);
                body.push(Statement::Assign(slot, arg));
                slots.push(slot);
            }
            let values = [Binary::FloorDivide, Binary::Modulo]
                .into_iter()
                .map(|op| Expr {
                    ty: Type::Int,
                    kind: ExprKind::Binary(
                        op,
                        Box::new(self.local(slots[0])),
                        Box::new(self.local(slots[1])),
                    ),
                })
                .collect();
            let ty = self.types.tuple(vec![Type::Int, Type::Int]);
            return Ok(Expr {
                ty,
                kind: ExprKind::Scope {
                    body,
                    value: Box::new(Expr {
                        ty,
                        kind: ExprKind::Tuple(values),
                    }),
                    clear: slots,
                },
            });
        }
        if args.len() == 1 && args[0].ty == Type::ObjectValue {
            let op = match name.id.as_str() {
                "repr" => Some((Type::Str, Builtin::ExceptionFieldRepr)),
                "len" => Some((Type::Int, Builtin::ExceptionFieldLen)),
                _ => None,
            };
            if let Some((ty, op)) = op {
                return Ok(Expr {
                    ty,
                    kind: ExprKind::Builtin(op, args),
                });
            }
        }
        if matches!(name.id.as_str(), "str" | "print")
            && args.len() == 1
            && args[0].ty == Type::ObjectValue
        {
            let text = Expr {
                ty: Type::Str,
                kind: ExprKind::Builtin(Builtin::Str, args),
            };
            return Ok(if name.id == "str" {
                text
            } else {
                Expr {
                    ty: Type::None,
                    kind: ExprKind::Builtin(Builtin::Print, vec![text]),
                }
            });
        }
        if name.id == "print"
            && args.len() == 1
            && matches!(
                args[0].ty,
                Type::Exception
                    | Type::NamedException(_)
                    | Type::ImportException
                    | Type::TimeoutExpired
            )
        {
            return Ok(Expr {
                ty: Type::None,
                kind: ExprKind::Builtin(
                    Builtin::Print,
                    vec![Expr {
                        ty: Type::Str,
                        kind: ExprKind::Builtin(Builtin::ExceptionMessage, args),
                    }],
                ),
            });
        }
        if name.id == "repr"
            && args.len() == 1
            && matches!(
                args[0].ty,
                Type::Exception | Type::ImportException | Type::NamedException(_)
            )
        {
            return Ok(Expr {
                ty: Type::Str,
                kind: ExprKind::Builtin(Builtin::ExceptionRepr, args),
            });
        }
        if args.len() == 1
            && args[0].ty == Type::LogRecord
            && matches!(name.id.as_str(), "repr" | "str" | "print")
        {
            let text = logging_records::op(ir::LogRecordOperation::Repr, Type::Str, args);
            return Ok(if name.id == "print" {
                Expr {
                    ty: Type::None,
                    kind: ExprKind::Builtin(Builtin::Print, vec![text]),
                }
            } else {
                text
            });
        }
        if let [value] = args.as_slice()
            && matches!(name.id.as_str(), "repr" | "str")
            && let Some(rendered) = self.mock_call_repr(value, call)?
        {
            return Ok(rendered);
        }
        if args.len() == 1
            && matches!(
                args[0].ty,
                Type::PlainObject | Type::StrDescriptor(_) | Type::Placeholder(_)
            )
            && matches!(name.id.as_str(), "repr" | "str" | "print")
        {
            let rendered = Expr {
                ty: Type::Str,
                kind: ExprKind::Builtin(Builtin::Repr, args),
            };
            return Ok(if name.id == "print" {
                Expr {
                    ty: Type::None,
                    kind: ExprKind::Builtin(Builtin::Print, vec![rendered]),
                }
            } else {
                rendered
            });
        }
        if name.id == "hasattr"
            && let [receiver, key] = args.as_slice()
            && receiver.ty == Type::SysImplementation
            && self.resolve(&call.func).is_none()
            && !self.names.contains_key("hasattr")
        {
            self.reject_builtin_keywords(call)?;
            let key = self.coerce(key.clone(), Type::Str, &call.arguments.args[1])?;
            return Ok(self.implementation_hasattr(receiver.clone(), key));
        }
        if matches!(name.id.as_str(), "getattr" | "hasattr" | "vars")
            && args.first().is_some_and(|arg| arg.ty == Type::NativeModule)
            && self.resolve(&call.func).is_none()
            && !self.names.contains_key(name.id.as_str())
        {
            return self.errno_builtin(name.id.as_str(), call, &args);
        }
        if matches!(name.id.as_str(), "vars" | "hasattr" | "getattr")
            && self.unshadowed_builtin(&call.func, name.id.as_str())
            && let Some(value) = self.record_namespace_builtin(name.id.as_str(), &args, call)?
        {
            return Ok(value);
        }
        if name.id == "vars"
            && let [receiver] = args.as_slice()
            && let Type::SimpleNamespace(id) = receiver.ty
        {
            return Ok(namespaces::backing(receiver.clone(), id));
        }
        if name.id == "repr" && args.len() == 1 && matches!(args[0].ty, Type::Class(_)) {
            return self.record_repr(args.into_iter().next().unwrap(), call);
        }
        if args.len() == 1 && dataclass_values::repr_supported(args[0].ty, self.types) {
            if name.id == "repr" || (name.id == "str" && self.types.element(args[0].ty).is_some()) {
                if !dataclass_values::can_format(args[0].ty, self.types, self.classes) {
                    return Err(error(
                        self.source,
                        call,
                        "generated repr cannot format resource or callable fields",
                    ));
                }
                return Ok(Expr {
                    ty: Type::Str,
                    kind: ExprKind::Builtin(Builtin::Repr, args),
                });
            }
            if matches!(name.id.as_str(), "str" | "print")
                && (matches!(
                    args[0].ty,
                    Type::Class(_)
                        | Type::SysImplementation
                        | Type::CompletedProcess(..)
                        | Type::Context
                        | Type::ContextMissing
                        | Type::ContextVar(_)
                        | Type::ContextToken(_)
                        | Type::ForwardRef(_)
                        | Type::ModuleSpec(_)
                        | Type::SimpleNamespace(_)
                        | Type::Dict(_)
                        | Type::Tuple(_)
                        | Type::VarTuple(_)
                        | Type::StringIterable
                ) || self.types.payload(args[0].ty) == Some(Type::Int)
                    || (name.id == "print" && self.types.element(args[0].ty).is_some()))
            {
                let text = self.record_text(args.into_iter().next().unwrap(), call)?;
                return Ok(if name.id == "str" {
                    text
                } else {
                    Expr {
                        ty: Type::None,
                        kind: ExprKind::Builtin(Builtin::Print, vec![text]),
                    }
                });
            }
        }
        if name.id == "len"
            && let [value] = args.as_slice()
            && self.mapping_class(value.ty).is_some()
        {
            return self.mapping_length(value.clone(), call);
        }
        if name.id.as_str() == "len"
            && matches!(args.as_slice(), [m] if m.ty == Type::ReadOnlyObjectMapping)
        {
            return Ok(Expr {
                ty: Type::Int,
                kind: ExprKind::Builtin(Builtin::Len, args),
            });
        }
        if name.id.as_str() == "len"
            && let ([source], [set]) = (call.arguments.args.as_ref(), args.as_slice())
            && let Some(length) = self.short_set_len(source, set)
        {
            return Ok(length);
        }
        let (builtin, ty) = match name.id.as_str() {
            "input" if args.is_empty() => (Builtin::Input, Type::Str),
            "chr" if args.len() == 1 && matches!(args[0].ty, Type::Int | Type::Bool) => {
                let value = args.into_iter().next().unwrap();
                let integer = if value.ty == Type::Int {
                    value
                } else {
                    Expr {
                        ty: Type::Int,
                        kind: ExprKind::Builtin(Builtin::Int, vec![value]),
                    }
                };
                return Ok(Expr {
                    ty: Type::Str,
                    kind: ExprKind::Builtin(Builtin::Chr, vec![integer]),
                });
            }
            "str" if args.is_empty() => {
                return Ok(Expr {
                    ty: Type::Str,
                    kind: ExprKind::Str(String::new()),
                });
            }
            "id" if args.len() == 1
                && !self.names.contains_key("id")
                && self.resolve(&call.func).is_none()
                && matches!(args[0].ty, Type::Popen(..) | Type::ActivePopen) =>
            {
                (
                    Builtin::Process(ir::ProcessOperation::PopenIdentity),
                    Type::Int,
                )
            }
            "str"
                if args.len() == 1
                    && matches!(
                        args[0].ty,
                        Type::Int
                            | Type::Float
                            | Type::Bool
                            | Type::Str
                            | Type::Bytes
                            | Type::None
                            | Type::Exception
                            | Type::NamedException(_)
                            | Type::ImportException
                            | Type::TimeoutExpired
                    ) =>
            {
                (Builtin::Str, Type::Str)
            }
            "int"
                if args.len() == 1
                    && matches!(args[0].ty, Type::Union(_))
                    && args[0].ty == self.types.string_or_int() =>
            {
                return Ok(self.integer_from_text_or_int(args.into_iter().next().unwrap()));
            }
            "int"
                if let [text, base] = args.as_slice()
                    && text.ty == Type::Str
                    && matches!(&base.kind, ExprKind::Int(n) if n == "16") =>
            {
                (Builtin::Int, Type::Int)
            }
            "int"
                if args.len() == 1
                    && matches!(args[0].ty, Type::Int | Type::Float | Type::Bool | Type::Str) =>
            {
                (Builtin::Int, Type::Int)
            }
            "float" if args.is_empty() => {
                return Ok(Expr {
                    ty: Type::Float,
                    kind: ExprKind::Float(0.0),
                });
            }
            "float"
                if args.len() == 1
                    && matches!(
                        args[0].ty,
                        Type::Int | Type::Float | Type::Bool | Type::Str | Type::Bytes
                    ) =>
            {
                (Builtin::Float, Type::Float)
            }
            "len" if args.len() == 1 && args[0].ty == Type::Context => {
                (Builtin::ContextVar(ir::ContextOperation::Len), Type::Int)
            }
            "len"
                if args.len() == 1
                    && matches!(
                        args[0].ty,
                        Type::JsonRoot | Type::JsonValue | Type::JsonObject
                    ) =>
            {
                (Builtin::JsonLen, Type::Int)
            }
            "len"
                if args.len() == 1
                    && matches!(
                        args[0].ty,
                        Type::Str
                            | Type::Bytes
                            | Type::StringIterable
                            | Type::ListInt
                            | Type::List(_)
                            | Type::Dict(_)
                            | Type::Set(_)
                            | Type::Deque(_)
                            | Type::FrozenStrings
                            | Type::StringKeys
                            | Type::CodecInfo
                            | Type::Tuple(_)
                            | Type::VarTuple(_)
                    ) =>
            {
                (Builtin::Len, Type::Int)
            }
            "print"
                if args.len() == 1
                    && matches!(
                        args[0].ty,
                        Type::Int | Type::Float | Type::Bool | Type::Str | Type::Bytes | Type::None
                    ) =>
            {
                (Builtin::Print, Type::None)
            }
            _ => {
                return Err(error(
                    self.source,
                    call,
                    "unknown function or unsupported builtin arguments",
                ));
            }
        };
        Ok(Expr {
            ty,
            kind: ExprKind::Builtin(builtin, args),
        })
    }

    /// Text uses UTF-8 and defaults to reading; explicit modes stay literal.
    fn open(&mut self, call: &ast::ExprCall, gzip: bool) -> Result<Expr, String> {
        let (path, mode) = match call.arguments.args.as_ref() {
            [path] if !gzip => (path, "r"),
            [path, ast::Expr::StringLiteral(mode)] => {
                check_string(mode, self.source)?;
                (path, mode.value.to_str())
            }
            _ => {
                return Err(error(
                    self.source,
                    call,
                    "open requires a path and a literal supported text or binary mode",
                ));
            }
        };
        let path = self.path_argument(path)?;
        let (ty, mut builtin) = match (gzip, mode) {
            (true, "rt") => (Type::TextIO, Builtin::GzipOpen),
            (true, "rb") => (Type::BinaryIO, Builtin::GzipOpen),
            (true, _) => {
                return Err(error(
                    self.source,
                    call,
                    "gzip.open supports literal 'rb' or 'rt' mode",
                ));
            }
            (false, mode) => paths::file_open_mode(mode).ok_or_else(|| {
                error(
                    self.source,
                    call,
                    "open supports literal 'r', 'w', 'rb', 'wb', 'ab', or 'xb' mode",
                )
            })?,
        };
        let mut args = vec![path];
        if ty == Type::BinaryIO {
            self.reject_builtin_keywords(call)?;
        } else if gzip {
            self.utf8_encoding(call)?;
        } else {
            let (mut encoding, mut newline) = (false, false);
            for keyword in &call.arguments.keywords {
                match keyword.arg.as_deref() {
                    Some("encoding") if !encoding => {
                        self.utf8_encoding_keyword(keyword)?;
                        args.push(self.expr(&keyword.value)?);
                        encoding = true;
                    }
                    Some("newline") if !newline => {
                        builtin = self.text_newline(&keyword.value, builtin)?;
                        newline = true;
                    }
                    _ => {
                        return Err(error(
                            self.source,
                            keyword,
                            "unexpected or repeated text open keyword",
                        ));
                    }
                }
            }
        }
        Ok(Expr {
            ty,
            kind: ExprKind::Builtin(builtin, args),
        })
    }

    /// POSIX writers already preserve newlines. Readers must retain CR and
    /// CRLF while still recognizing each as a physical line terminator.
    fn text_newline(&self, value: &ast::Expr, builtin: Builtin) -> Result<Builtin, String> {
        if !matches!(builtin, Builtin::TextOpen(_)) {
            return Err(error(
                self.source,
                value,
                "binary open does not accept newline",
            ));
        }
        match value {
            ast::Expr::NoneLiteral(_) => Ok(builtin),
            ast::Expr::StringLiteral(s) if s.value.to_str().is_empty() => {
                Ok(if matches!(builtin, Builtin::TextOpen(0)) {
                    Builtin::TextOpen(4)
                } else {
                    builtin
                })
            }
            _ => Err(error(
                self.source,
                value,
                "expected newline=None or an empty literal string",
            )),
        }
    }

    /// Validate an explicit encoding for native text I/O.
    fn utf8_encoding(&self, call: &ast::ExprCall) -> Result<(), String> {
        let [encoding] = call.arguments.keywords.as_ref() else {
            return Err(error(
                self.source,
                call,
                "text file I/O requires encoding='utf-8'",
            ));
        };
        self.utf8_encoding_keyword(encoding)
    }

    fn utf8_encoding_keyword(&self, encoding: &ast::Keyword) -> Result<(), String> {
        if encoding
            .arg
            .as_ref()
            .is_none_or(|name| name.as_str() != "encoding")
        {
            return Err(error(
                self.source,
                encoding,
                "text file I/O accepts only encoding='utf-8'",
            ));
        }
        let ast::Expr::StringLiteral(value) = &encoding.value else {
            return Err(error(
                self.source,
                encoding,
                "encoding must be the literal 'utf-8'",
            ));
        };
        check_string(value, self.source)?;
        if value.value.to_str() != "utf-8" {
            return Err(error(
                self.source,
                encoding,
                "encoding must be the literal 'utf-8'",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn long_call_chains_can_be_checked_with_or_without_a_cycle() {
        use std::collections::HashSet;

        let mut graph: Vec<_> = (1..=40_000).map(|id| HashSet::from([id])).collect();
        graph.push(HashSet::new());
        assert_eq!(super::recursive_call_root(&graph), None);
        graph.last_mut().unwrap().insert(0);
        assert_eq!(super::recursive_call_root(&graph), Some(0));
    }

    #[test]
    fn literal_string_comprehensions_keep_inline_iteration() {
        crate::compile(
            "values = [outer + inner for outer in [\"a\", \"b\"] for inner in [\"1\", \"2\"]]\nprint(len(values))\n",
            &crate::CompileOptions::default(),
        )
        .unwrap();
    }

    #[test]
    fn splitlines_fuses_only_the_immediately_consumed_list() {
        let llvm = crate::compile(
            r#"
from pathlib import Path

def scan(text: str, path: Path, start: str) -> None:
    for line in text.splitlines():
        print(line)
    for i, line in enumerate(path.read_text().splitlines(), int(start)):
        print(i)
        print(line)
    lines = text.splitlines()
    for line in lines:
        print(line)
    print(len(lines))
scan("a\nb", Path("lines.txt"), "4")
"#,
            &crate::CompileOptions::default(),
        )
        .unwrap();
        assert_eq!(
            llvm.matches("call ptr @imre_str_splitlines_iter(").count(),
            2
        );
        assert_eq!(llvm.matches("call ptr @imre_str_splitlines(").count(), 1);
        let (_, after_read) = llvm.split_once("call ptr @imre_path_read_text(").unwrap();
        let (_, after_cursor) = after_read
            .split_once("call ptr @imre_str_splitlines_iter(")
            .unwrap();
        assert!(after_cursor.contains("call i64 @imre_int_parse("));
    }

    #[test]
    fn small_literal_string_loops_elide_container_allocations() {
        for length in [1, 2, 8] {
            let values = vec!["\"value\""; length].join(", ");
            let source = format!(
                "def measure() -> int:\n    count = 0\n    for value in [{values}]:\n        count += len(value)\n    return count\nprint(measure())\n"
            );
            let llvm = crate::compile(&source, &crate::CompileOptions::default()).unwrap();
            assert!(!llvm.contains("call ptr @imre_seq_new("));
            assert!(!llvm.contains("call i32 @imre_seq_append("));
            assert!(!llvm.contains("call ptr @imre_record_try_new("));
        }
    }

    #[test]
    fn other_string_list_loops_keep_their_containers() {
        for source in [
            "values = [\"a\", \"b\"]\nfor value in values:\n    print(value)\n",
            "for value in (values := [\"a\", \"b\"]):\n    print(value)\n",
            "suffix = \"b\"\nfor value in [\"a\", suffix]:\n    print(value)\n",
            "for value in [\"1\", \"2\", \"3\", \"4\", \"5\", \"6\", \"7\", \"8\", \"9\"]:\n    print(value)\n",
        ] {
            let llvm = crate::compile(source, &crate::CompileOptions::default()).unwrap();
            assert!(llvm.contains("call ptr @imre_seq_new("), "{source}");
        }
    }

    #[test]
    fn fields_can_own_a_concrete_subclass_of_the_declared_type() {
        use super::{ClassInfo, Type, Types};

        let types = Types::default();
        types.add_concrete_subclass(1, &[0]);
        let mut fields = vec![vec![], vec![], vec![types.list(Type::Class(0))]];
        assert_eq!(ClassInfo::cyclic_field_root(&fields, &types), None);
        fields[1].push(Type::Class(2));
        assert!(ClassInfo::cyclic_field_root(&fields, &types).is_some());
    }

    #[test]
    fn deeply_nested_field_graphs_preserve_cycle_detection() {
        use super::{ClassInfo, Type, Types};

        let types = Types::default();
        let mut fields: Vec<_> = (1..=20_000)
            .map(|id| vec![types.list(Type::Class(id))])
            .collect();
        fields.push(vec![Type::Int]);
        assert_eq!(ClassInfo::cyclic_field_root(&fields, &types), None);
        fields.last_mut().unwrap().push(types.list(Type::Class(0)));
        assert_eq!(ClassInfo::cyclic_field_root(&fields, &types), Some(0));
    }
}
