//! Enforces the closed typed subset independently of gradual Python typing.

use ruff_python_ast::CmpOp;
use ruff_python_ast::UnaryOp;
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::result::Result as StdResult;
use std::sync::Arc;

pub(crate) use imre_ast::exceptions::builtin_id as exception_kind;
use imre_ast::source_encoding::is_utf8_codec;
use ruff_python_ast as ast;
use ruff_text_size::{Ranged, TextRange, TextSize};

use crate::classes::Classes;
use crate::ir::{self, Binary, Builtin, Compare, Expr, ExprKind, RegexCapture, Statement, Type};
use crate::modules::{Binding, DefinitionId, Intrinsic, ModuleId, SourceGraph};
use crate::narrowing;
use crate::types::Types;

pub(crate) mod phase;
// Checkers and their expression modules propagate signature dependencies with errors.
// Terminal-only validation and the graph driver use the standard Result.
use phase::{Diagnostic, Failure, Kind, Result, TypeDeclaration};

mod annotation_operands;
mod annotations;
pub(crate) use annotation_operands::{AnnotationBody, AnnotationCode};
pub(crate) use annotations::{parameter_annotations, spelling as annotation_spelling};
pub(crate) use function_scope::ClassAnnotations;
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
mod fixed_body_source;
pub(crate) use fixed_body_source::{FixedAttribute, FixedBodySource, FixedNode};
mod fixed_tuples;
mod fnmatch;
mod format_cursors;
mod fractions;
mod fromkeys;
mod fstrings;
use fstrings::FStringLiteral;
mod fspath;
mod function_caches;
pub(crate) mod function_scope;
mod function_text;
mod functools;
mod hash_file;
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
pub(crate) mod module_source;
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
mod registered;
mod required_sites;
#[cfg(test)]
pub(crate) use registered::booleans::{ExpressionInput, SourceOperand};
pub(crate) use registered::booleans::{ExpressionSites, Selection as ExpressionSelection};
pub(crate) use registered::type_alias_body;
#[cfg(test)]
pub(crate) use registered::values::registered_value;
pub(crate) use registered::values::{
    ModuleSource as RegisteredModuleSource, RegisteredSource, exec_literal_text,
    exec_source_operand, packed_arguments, push_codepoints, scan_literals,
    validate_docstring_literals,
};
pub(crate) use registered::{
    AnnotationProvider, CheckedProvider, DeclarationSite, FixedFunctions, FrameArguments,
    FrameInputs, FrameSite, FrameSlots, LexicalOperands, NamedBody, ParameterAnnotations,
    ParameterEvaluator, PhysicalOperand, PreparedRefusal, RegisteredBody, RegisteredFunction,
    RegisteredGenerator, RegisteredLambda, RegisteredScope, RegisteredSuite, SuitePreparation,
    TypeParameterValues, annotation_expression, registered_sites,
};
pub(crate) use registered::{LocalBinding, RegisteredClass, RegisteredOperand, RegisteredRegion};
#[cfg(test)]
pub(crate) use registered::{StatementSites, booleans::OwnedExpression};
pub(crate) use required_sites::RequiredSite;
mod sequences;
mod shutil;
mod sign_parameters;
pub(crate) use sign_parameters::only_compares_zero;
mod single_dispatch;
mod socket;
mod statements;
mod stdlib;
pub(crate) use stdlib::literal_replace_mismatch;
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
use assignments::{LocalState, ValueSource};
pub(crate) use caller_cache::CallerCache;
use calls::{Parameter, ParameterKind, parameter};
pub(crate) use calls::{fixed_call_arguments, known_declared_argument, scalar_literal};
use comprehensions::Helpers;
pub(crate) use function_text::{docstring, docstring_literal};
use module_values::{Initialization, ModuleValues};

#[derive(Clone)]
struct Signature {
    id: usize,
    physical_frame: bool,
    may_have_display: bool,
    asynchronous: bool,
    parameter_declarations: Arc<[Option<TypeDeclaration>]>,
    params: Vec<Parameter>,
    returns: Type,
    method: Option<records::MethodContext>,
}

impl Signature {
    fn parameter_declaration(&self, index: usize) -> Option<TypeDeclaration> {
        self.parameter_declarations.get(index).copied().flatten()
    }

    /// An eager source frame reads its declared pack from the tuple bound by the Python caller.
    fn takes_positional_tuple(
        &self,
        source: &Cow<'_, ast::StmtFunctionDef>,
        session: &crate::analysis::Session,
    ) -> bool {
        matches!(source, Cow::Borrowed(def) if !def.is_async)
            && matches!(self.params.as_slice(), [param] if param.kind == ParameterKind::Variadic)
            && self.parameter_declaration(0).is_some_and(|declaration| {
                !session.is_generator_definition(declaration.module, source)
            })
    }

    fn registered_frame(&self) -> bool {
        self.method.is_none()
            && self
                .params
                .iter()
                .any(|param| param.ty == Type::PythonObject)
    }
}

type InitializerDefaults = HashMap<usize, Vec<(usize, Expr)>>;

struct ClassInfo<'a> {
    parsers: argparse::Parsers,
    declarations: &'a Classes<'a>,
    fields: Vec<Vec<Type>>,
    methods: Vec<HashMap<String, Signature>>,
    setters: Vec<HashMap<String, Signature>>,
    /// Declared record types whose truth must be selected by concrete layout.
    truth_dispatch: HashSet<usize>,
    keyword_copies: HashSet<usize>,
    /// Original base called by a constructor with statically forwarded packs.
    forwarded_initializers: HashMap<usize, DefinitionId>,
    nonreturning_functions: HashSet<usize>,
    cached: HashMap<usize, function_caches::Policy>,
    cached_properties: cached_properties::Slots,
    setup_fields: unittest_setup::Slots,
    generators: HashSet<usize>,
    producer_items: HashMap<usize, Type>,
    contexts: HashSet<usize>,
}

impl<'a> ClassInfo<'a> {
    /// Resolves all nominal field types before checking constructors and methods.
    fn new(
        graph: &SourceGraph,
        types: &Types,
        declarations: &'a mut Classes<'a>,
    ) -> StdResult<(Self, InitializerDefaults), Diagnostic> {
        let mut initializer_defaults = InitializerDefaults::new();
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
        let mut custom_truth = HashSet::new();
        let mut truth_dispatch = HashSet::new();
        for (id, class) in declarations.definitions.iter().enumerate() {
            if class.mapping
                || class
                    .methods
                    .iter()
                    .any(|method| method.name.as_str() == "__bool__")
            {
                custom_truth.insert(id);
                if class.mapping {
                    truth_dispatch.insert(id);
                }
                if class.abstract_methods.is_empty() {
                    // A concrete subclass can inhabit a variable annotated with any base.
                    truth_dispatch.extend(class.bases.iter().copied());
                }
            }
        }
        types.set_custom_truth(custom_truth);
        // Permit forward references while reading field annotations. Refine
        // these candidates once every record's concrete field types are known.
        let mut hashable = declarations
            .definitions
            .iter()
            .enumerate()
            .filter_map(|(id, class)| {
                (class.is_uuid(graph)
                    || class
                        .dataclass
                        .is_some_and(|options| options.frozen && options.eq))
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
                    .enumerate()
                    .map(|(field_index, field)| {
                        if field.name == crate::classes::EXCEPTION_ARGS_SLOT
                            || class.forwarder.is_some()
                        {
                            return Ok(Type::ObjectValue);
                        }
                        annotation_in_class(
                            &field.annotation,
                            graph,
                            field.module,
                            declarations,
                            types,
                            Some(class_id),
                        )
                        .and_then(|ty| {
                            if class.dataclass.is_some_and(|d| d.value_struct)
                                && !matches!(ty, Type::Float | Type::Bool)
                            {
                                return Err(diagnostic_message(
                                    &graph.modules[field.module].source,
                                    &field.annotation,
                                    "imre.struct fields must be float or bool",
                                ));
                            }
                            if field.weak_proxy {
                                let Type::Class(id) = ty else {
                                    return Err(diagnostic_message(
                                        &graph.modules[field.module].source,
                                        &field.annotation,
                                        "a weak proxy field requires a record annotation",
                                    ));
                                };
                                return Ok(Type::WeakProxy(id));
                            }
                            if let Some(default) = records::field_default(
                                field,
                                ty,
                                types,
                                &graph.modules[field.module].source,
                            )? && class.generated_initializer
                                && matches!(
                                    field.default,
                                    Some(crate::classes::FieldDefault::Factory(_))
                                )
                            {
                                initializer_defaults
                                    .entry(class_id)
                                    .or_default()
                                    .push((field_index, default));
                            }
                            if callables::contains_callable(ty, types) {
                                Err(diagnostic_message(
                                    &graph.modules[field.module].source,
                                    &field.annotation,
                                    "class fields cannot contain callable values",
                                ))
                            } else if class.toml_hash && ty == Type::JsonObject {
                                // The constructor accepts its declared default. The hash reads
                                // these fields together using the stored TOML representation.
                                Ok(documents::MAP)
                            } else {
                                Ok(ty)
                            }
                        })
                        .map_err(|error| {
                            format!("{}:{error}", graph.modules[field.module].path.display())
                        })
                    })
                    .collect::<StdResult<Vec<_>, _>>()
            })
            .collect::<StdResult<Vec<_>, _>>()?;
        for (id, class) in declarations.definitions.iter().enumerate() {
            if class.abstract_methods.is_empty() && !class.bases.is_empty() {
                types.add_concrete_subclass(id, &class.bases);
            }
            if let Some(base) = class.storage_base
                && !fields[id].starts_with(&fields[base])
            {
                return Err(graph.modules[class.definition.module].diagnostic(
                    class.node.as_ref(),
                    "inherited fields must keep their exact types and offsets",
                ));
            }
        }
        let cached_properties = cached_properties::slots(graph, types, declarations, &mut fields)?;
        let setup_fields = unittest_setup::slots(graph, types, declarations, &mut fields)?;
        // Reference counting is sufficient only while no accepted field graph can
        // close a cycle. This concerns storage types, not method signatures.
        if let Some(id) = Self::cyclic_field_root(&fields, types) {
            let class = &declarations.definitions[id];
            return Err(graph.modules[class.definition.module].diagnostic(class.node.as_ref(),
                "cyclic heap types are unsupported; class fields must form an acyclic ownership graph"));
        }
        loop {
            let before = hashable.len();
            hashable.retain(|&id| {
                // A UUID-typed key may hold a subclass with different equality or hashing.
                if declarations.definitions[id].is_uuid(graph)
                    && !types.concrete_subclasses(id).is_empty()
                {
                    return false;
                }
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
                    return Err(graph.modules[class.definition.module].diagnostic(
                        class.node.as_ref(),
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
                    return Err(graph.modules[field.module].diagnostic(
                        &field.annotation,
                        "record keys require a frozen dataclass with immutable comparison fields",
                    ));
                }
            }
        }
        let parsers = argparse::Parsers::collect(graph, types, declarations, &mut fields);
        let classes = Self {
            parsers,
            truth_dispatch,
            methods: vec![HashMap::new(); fields.len()],
            setters: vec![HashMap::new(); fields.len()],
            keyword_copies: HashSet::new(),
            forwarded_initializers: HashMap::new(),
            nonreturning_functions: HashSet::new(),
            cached: HashMap::new(),
            cached_properties,
            setup_fields,
            generators: HashSet::new(),
            producer_items: HashMap::new(),
            contexts: HashSet::new(),
            declarations,
            fields,
        };
        Ok((classes, initializer_defaults))
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

/// Recognize a builtin name when no declaration in the module binds it.
fn module_builtin_name(module: &crate::modules::SourceModule, name: &str) -> bool {
    !module.bindings.contains_key(name)
        && (reserved_builtin_function(name) || imre_ast::providers::BUILTIN_TYPES.contains(&name))
}

pub(crate) fn reserved_builtin_function(name: &str) -> bool {
    RESERVED.contains(&name) && imre_ast::providers::BUILTIN_FUNCTIONS.contains(&name)
}

fn reserved(name: &str) -> bool {
    RESERVED.contains(&name) || exception_kind(name).is_some()
}

fn debug_operand(name: &ast::ExprName, python_optimization: i32) -> Option<ExprKind> {
    (name.id == "__debug__").then(|| ExprKind::Bool(python_optimization == 0))
}

fn pattern_elements(target: &ast::Expr) -> Option<&[ast::Expr]> {
    match target {
        ast::Expr::Tuple(tuple) => Some(&tuple.elts),
        ast::Expr::List(list) => Some(&list.elts),
        _ => None,
    }
}

fn fault_frame_statement(file: String, name: &str, line: u32) -> Statement {
    let frame = Builtin::FaultFrame {
        file,
        name: name.to_owned(),
        line,
    };
    Statement::Expr(Expr {
        ty: Type::None,
        kind: ExprKind::Builtin(frame, vec![]),
    })
}

/// Resolves the project-wide call graph without changing source-level namespaces.
#[cfg(test)]
pub(crate) fn lower(graph: &SourceGraph) -> StdResult<ir::Module, String> {
    lower_current(graph, None, None, false)
        .0
        .map_err(|report| report.to_string())?
        .expect("a fixed set of methods cannot request another lowering")
        .into_native(graph)
        .map(|(module, _)| module)
}

#[cfg(test)]
pub(crate) fn lower_with_owners(
    graph: &mut SourceGraph,
) -> StdResult<crate::CheckedIr, Diagnostic> {
    lower_with_caller_cache(graph, None, None)?
        .into_native(graph)
        .map_err(Into::into)
}

/// Cached layout hints can keep a shared handle to the graph's quiet Salsa storage.
pub(crate) enum SourceCheckSession<'a> {
    Borrowed(&'a mut crate::analysis::Session),
    SharedGraph,
}

pub(crate) fn lower_with_caller_cache(
    graph: &mut SourceGraph,
    mut cache: Option<&mut CallerCache>,
    source_checks: Option<SourceCheckSession<'_>>,
) -> StdResult<crate::CheckedProgram, Diagnostic> {
    let mut key_reprs = HashSet::new();
    loop {
        let before = key_reprs.len();
        let (lowered, checks) = lower_current(
            graph,
            cache.take(),
            Some(&mut key_reprs),
            !matches!(source_checks, Some(SourceCheckSession::SharedGraph)),
        );
        if let Err(reason) = &lowered
            && let Kind::MissingFormatter(owner) = reason.kind
        {
            key_reprs.insert(owner);
        }
        if let Some(lowered) = lowered.transpose()
            && key_reprs.len() == before
        {
            let session = match source_checks {
                Some(SourceCheckSession::SharedGraph) => {
                    for &owner in &checks.unresolved {
                        let _ = registered::retain_generator(graph, owner, &checks);
                    }
                    return lowered;
                }
                Some(SourceCheckSession::Borrowed(session)) => Some(session),
                None => None,
            };
            registered::finish_source_checks(graph, checks, session);
            return lowered;
        }
        // Only the typed subscript knows the key's class. Include its missing
        // formatter and its dependencies without admitting unrelated methods.
        graph.include_key_reprs(&key_reprs);
    }
}

fn lower_current(
    graph: &SourceGraph,
    cache: Option<&mut CallerCache>,
    key_reprs: Option<&mut HashSet<DefinitionId>>,
    defer_solved: bool,
) -> (
    StdResult<Option<crate::CheckedProgram>, Diagnostic>,
    registered::SourceChecks,
) {
    let mut inference = inference::Inference::default();
    inference.source_checks.solved = defer_solved.then(Default::default);
    let mut layout_error = None;
    let result = lower_graph(graph, cache, key_reprs, &mut inference, &mut layout_error);
    (layout_error.map_or(result, Err), inference.source_checks)
}

/// The file driver checks this graph again before it can place a registered body.
fn retries_failed_layout(graph: &SourceGraph) -> bool {
    graph.layout_hint.get() == crate::modules::LayoutHint::Applied
        && !graph.reachable_only()
}

/// A body without its native representation still has checked neighbors. Keep its
/// original diagnostic for backend selection after their source contracts have run.
fn checked_region<T>(
    result: Result<T, String>,
    graph: &SourceGraph,
    module: ModuleId,
    unavailable: &mut Option<Diagnostic>,
    capture_on_object: impl FnOnce(&Failure),
) -> StdResult<Option<T>, Diagnostic> {
    let failure = match result {
        Ok(body) => return Ok(Some(body)),
        Err(failure) => failure,
    };
    // A failed layout trial is discarded. Finish checking and capture any
    // registered siblings in the ordinary pass. Reachable key formatters
    // have their own retry inside the layout trial.
    if retries_failed_layout(graph) {
        return Err(failure.terminal_in(graph, module));
    }
    capture_on_object(&failure);
    if graph.modules[module].is_program_source() && failure.is_representation() {
        unavailable.get_or_insert_with(|| failure.terminal_in(graph, module));
        Ok(None)
    } else {
        Err(failure.terminal_in(graph, module))
    }
}

pub(crate) fn fixed_frame_entry(function: &ast::StmtFunctionDef) -> bool {
    !function.is_async
        && function.decorator_list.is_empty()
        && function.type_params.is_none()
        && function.parameters.vararg.is_none()
        && function.parameters.kwarg.is_none()
}

/// A newly reachable key formatter changes the method table. Return no program until it is included.
fn lower_graph(
    graph: &SourceGraph,
    mut cache: Option<&mut CallerCache>,
    key_reprs: Option<&mut HashSet<DefinitionId>>,
    inference: &mut inference::Inference,
    layout_error: &mut Option<Diagnostic>,
) -> StdResult<Option<crate::CheckedProgram>, Diagnostic> {
    graph.clear_registered_functions();
    let types = Types::default();
    typing::collect_sites(graph, &types);
    namedtuples::collect(graph, &types)?;
    let can_bind = graph.command_startup.is_some() && !retries_failed_layout(graph);
    let mut declarations = Classes::collect_layouts(graph, can_bind.then_some(&mut *layout_error))?;
    let (mut classes, mut initializer_defaults) = ClassInfo::new(graph, &types, &mut declarations)?;
    dataclass_values::validate(&classes, graph, &types)?;
    let mut signatures = HashMap::new();
    let mut definitions = Vec::new();
    let mut callable_definitions = Vec::new();
    // A source method needs its original binder and operand sites if its
    // callable layout requires a registered frame.
    let can_transfer_frames = !classes.declarations.definitions.is_empty();
    for (module_id, module) in graph.modules.iter().enumerate() {
        if !module.executable.is_empty() {
            inference.discover_list_elements(graph, module_id, None);
        }
        for (name, binding) in &module.bindings {
            if reserved(name)
                && graph.builtin_function_shadow(name, *binding).is_none()
                && !matches!(
                    (name.as_str(), binding),
                    (
                        "open",
                        Binding::Intrinsic(
                            Intrinsic::BuiltinsOpen
                                | Intrinsic::GzipOpen
                                | Intrinsic::Fd(ir::FdOperation::Open)
                        )
                    ) | ("print", Binding::Intrinsic(Intrinsic::BuiltinsPrint))
                        | (
                            "filter",
                            Binding::Intrinsic(Intrinsic::Fnmatch(
                                ir::FnmatchOperation::Filter { .. }
                            ))
                        )
                )
            {
                return Err(module.diagnostic(
                    module.parsed.syntax(),
                    "a static binding uses a reserved builtin name",
                ));
            }
        }
        for (statement_id, statement) in module.parsed.syntax().body.iter().enumerate() {
            let owner = DefinitionId {
                module: module_id,
                statement: statement_id,
            };
            match statement {
                ast::Stmt::Assign(assign)
                    if assign
                        .value
                        .as_list_expr()
                        .is_some_and(|list| list.elts.is_empty())
                        && let [target] = assign.targets.as_slice()
                        && target.as_name_expr().is_some_and(|name| {
                            module.bindings.get(name.id.as_str()) == Some(&Binding::Value(owner))
                        })
                        && !module.rebound_values.contains(&statement_id)
                        && graph.session.unannotated_module_store(module_id, target) =>
                {
                    inference.discover_module_list(&types, owner);
                }
                ast::Stmt::FunctionDef(function) => {
                    if !graph.includes(owner) {
                        continue;
                    }
                    if layout_error.is_some()
                        && !(module.is_program_source()
                            && fixed_frame_entry(function)
                            && function
                                .parameters
                                .iter_non_variadic_params()
                                .all(|p| p.default.is_none())
                            && graph.session.function_uses_fixed_names(
                                module_id,
                                function,
                                false,
                                |_| false,
                            ))
                    {
                        continue;
                    }
                    let cached = function_caches::policy(graph, module_id, function)?;
                    let physical_entry = (can_transfer_frames
                        || graph.registered_startup
                        || module_id == graph.entry)
                        && module.is_program_source()
                        && fixed_frame_entry(function);
                    let mut function = Cow::Borrowed(function);
                    if module.overloads.contains(&statement_id) {
                        graph.runtime_overload(module_id, function.name.range());
                        typing::strip_overload(function.to_mut());
                    }
                    if cached.is_some() || inspect::marks_function(graph, module_id, &function) {
                        function.to_mut().decorator_list.clear();
                    }
                    let element = native_generators::item_type(
                        &function, graph, module_id, &classes, &types,
                    )?;
                    if element.is_some() {
                        if cached.is_some() {
                            return Err(module.diagnostic(
                                function.as_ref(),
                                "generator functions cannot be cached",
                            ));
                        }
                        let range = function.returns.as_ref().unwrap().range();
                        function.to_mut().returns =
                            Some(Box::new(ast::Expr::NoneLiteral(ast::ExprNoneLiteral {
                                range,
                                node_index: ast::AtomicNodeIndex::NONE,
                            })));
                    }
                    let defaults = if cached.is_some() || !function.decorator_list.is_empty() {
                        // A wrapper can distinguish omitted keywords from explicitly supplied defaults.
                        keyword_forwarding::Defaults::local(module_id)
                    } else {
                        keyword_forwarding::prepare(graph, module_id, &mut function)
                    };
                    let mut signature = match signature(
                        &function,
                        graph,
                        defaults,
                        classes.declarations,
                        &types,
                        None,
                        inference,
                    ) {
                        Ok(signature) => signature,
                        Err(_) if layout_error.is_some() => continue,
                        Err(error) => {
                            return Err(format!("{}:{error}", module.path.display()).into());
                        }
                    };
                    // Rewriting a top-level function takes ownership of its declaration.
                    signature.physical_frame =
                        physical_entry && matches!(function, Cow::Borrowed(_));
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
                            return Err(module.diagnostic(function.as_ref(), "cached functions require exact immutable positional keys and an annotated synchronous result"));
                        }
                        classes.cached.insert(signature.id, policy);
                    }
                    if let Some(element) = element {
                        classes.producer_items.insert(signature.id, element);
                    }
                    signatures.insert(owner, signature.clone());
                    definitions.push((module_id, function, signature));
                    callable_definitions.push(owner);
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
    if let Some(error) = layout_error.take() {
        registered::capture(
            graph,
            &signatures,
            &definitions,
            &callable_definitions,
            &classes,
            &types,
            inference,
            registered::Capture::Declared,
            definitions.iter().filter_map(|(_, _, signature)| {
                signature.physical_frame.then_some((signature.id, None))
            }),
        );
        return Err(error);
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
            let kind = class.method_kinds[index];
            // property(get, set) retains the original functions. Both spellings
            // must infer one signature even if only the property is used.
            if matches!(
                kind,
                crate::classes::MethodKind::Property | crate::classes::MethodKind::PropertySetter
            ) && let Some(original) =
                classes.methods[class_id].get(method.function.name.as_str())
                && let Some(owner) = original.method
                && class.method_kinds[owner.index] == crate::classes::MethodKind::Instance
                && class.method_origins[owner.index] == class.method_origins[index]
                && class.methods[owner.index].range() == method.range()
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
            let (mut method, returns_iterator) =
                records::executable_method(method, context, classes.declarations, graph, &types)?;
            let keyword_copy =
                record_inputs::copies_keywords(&class.methods[index], graph, module_id);
            if keyword_copy {
                let method = method.to_mut();
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
            let (defaults, forwarded_base) = keyword_forwarding::prepare_method(
                graph,
                module_id,
                &mut method,
                context,
                classes.declarations,
            );
            // A synthesized initializer has the class statement's range. Only declared
            // methods have their own source site for a Python entry.
            let physical_entry = kind == crate::classes::MethodKind::Instance
                && class.method_origins[index] == class.definition
                && method.start() != class.node.start()
                && class.forwarder.is_none()
                && module.is_program_source()
                && defaults.forwarded.is_none()
                && fixed_frame_entry(&class.methods[index]);
            let mut signature = signature(
                &method,
                graph,
                defaults,
                classes.declarations,
                &types,
                Some(context),
                inference,
            )
            .map_err(|error| format!("{}:{error}", module.path.display()))?;
            if method.name.as_str() == "__init__" {
                if signature.returns != Type::None {
                    return Err(module.diagnostic(method.as_ref(), "__init__ must return None"));
                }
                if let Some(defaults) = initializer_defaults.remove(&class_id) {
                    for (index, value) in defaults {
                        let field = &class.fields[index];
                        let parameter = signature
                            .params
                            .iter_mut()
                            .skip(1)
                            .find(|parameter| parameter.name == field.name)
                            .unwrap();
                        parameter.default = if parameter.ty == value.ty {
                            Some(value)
                        } else {
                            records::field_default(field, parameter.ty, &types, &module.source)?
                        };
                    }
                }
                if class.generated_initializer {
                    records::private_initializer_parameters(method.to_mut(), module);
                }
            } else if method.name.as_str() == "__post_init__" && signature.returns != Type::None {
                return Err(module.diagnostic(method.as_ref(), "__post_init__ must return None"));
            } else if matches!(method.name.as_str(), "__str__" | "__repr__")
                && signature.returns != Type::Str
            {
                return Err(
                    module.diagnostic(method.as_ref(), &format!("{} must return str", method.name))
                );
            }
            if let Some(base) = forwarded_base {
                classes.forwarded_initializers.insert(signature.id, base);
            }
            if keyword_copy {
                classes.keyword_copies.insert(signature.id);
            }
            if kind == crate::classes::MethodKind::Instance {
                let after_doc = usize::from(docstring_literal(&method.body).is_some());
                if module.archive.is_none()
                    && class.methods[index].decorator_list.is_empty()
                    && !signature.asynchronous
                    && matches!(&method.body[after_doc..], [ast::Stmt::Raise(_)])
                    && !imre_ast::code_flags::is_generator(&method.body)
                {
                    classes.nonreturning_functions.insert(signature.id);
                }
            }
            let table = if class.method_kinds[index] == crate::classes::MethodKind::PropertySetter {
                &mut classes.setters[class_id]
            } else {
                &mut classes.methods[class_id]
            };
            table.insert(method.name.to_string(), signature.clone());
            if contexts::decorated(&class.methods[index], graph, module_id) {
                if !contexts::single_yield(&method) {
                    return Err(module.diagnostic(method.as_ref(), "a context manager requires one bare yield on each successful path and an optional finally suite"));
                }
                if signature.returns != Type::None {
                    return Err(module.diagnostic(
                        method.as_ref(),
                        "a bare-yield context manager must return None",
                    ));
                }
                classes.contexts.insert(signature.id);
            }
            if returns_iterator {
                classes.generators.insert(signature.id);
            } else {
                signature.physical_frame = physical_entry;
            }
            definitions.push((module_id, method, signature));
            callable_definitions.push(class.definition);
        }
    }
    let record_factories = records::record_factories(
        &mut classes,
        &mut definitions,
        &mut callable_definitions,
        inference,
        graph,
        &types,
    )?;
    if closed_calls::has_keyword_displays(graph) {
        inference.discover_packed_arguments();
    }
    for (module, function, signature) in &mut definitions {
        let renamed = assignments::alias_builtin_parameters(function, graph, *module);
        let (dispatch, packed, may_have_display) = closed_calls::prepare(function, graph, *module);
        signature.may_have_display = may_have_display;
        if renamed || dispatch || packed {
            signature.physical_frame = false;
        }
        if packed {
            inference.discover_packed_arguments();
        }
        // Initializers check calls before the ordinary body pass. A raise in the
        // function's outer suite prevents a normal exit unless a return bypasses it.
        if let Cow::Borrowed(source) = function
            && graph.modules[*module].is_program_source()
            && source.decorator_list.is_empty()
            && source.body.iter().any(ast::Stmt::is_raise_stmt)
            && !graph.session.call_may_return_implicitly(*module, source)
            && crate::analysis::explicit_returns(source).is_empty()
        {
            classes.nonreturning_functions.insert(signature.id);
        }
    }
    let entry_statements = graph.modules[graph.entry].executable_statements();
    let inference::Solution {
        registered_parameters,
        method_returns,
        mut checked_bodies,
        suspended_bodies,
        needs_registered_entry,
    } = inference.solve(
        graph,
        &entry_statements,
        &callable_definitions,
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
                .diagnostic(class.node.as_ref(), "__bool__ must return bool"));
        }
    }
    if needs_registered_entry {
        checked_bodies.retain(|&id| {
            let (module, definition, signature) = &definitions[id];
            if signature.method.is_some() {
                return registered::physical_method(signature, definition, &classes).is_some();
            }
            // Top-level functions borrow the parsed declaration until a transform edits it.
            if matches!(definition, Cow::Borrowed(_)) {
                return true;
            }
            graph.modules[*module].parsed.syntax().body[callable_definitions[id].statement]
                .as_function_def_stmt()
                == Some(definition)
        });
    }
    // Fixed FILE compilations need no secondary body. A registered launch or an
    // actual representation failure can still use functions whose signatures solved.
    let pending_capture = std::cell::RefCell::new(Some(method_returns));
    let capture_slots = |prepared: &mut HashMap<_, _>, ids: &mut dyn Iterator<Item = usize>| {
        registered::capture(
            graph,
            &signatures,
            &definitions,
            &callable_definitions,
            &classes,
            &types,
            inference,
            registered::Capture::Solved(needs_registered_entry.then_some(&checked_bodies)),
            ids.map(|id| (id, prepared.remove(&id))),
        );
    };
    let slots = || definitions.iter().map(|(_, _, signature)| signature.id);
    let capture = || {
        if let Some(mut prepared) = pending_capture.take() {
            capture_slots(&mut prepared, &mut slots());
        }
    };
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
            .filter_map(|(p, inferred)| p.annotation().map(|value| (value, inferred.ty, false)))
            .chain(
                function
                    .returns
                    .as_deref()
                    .map(|value| (value, signature.returns, true)),
            );
        for (annotation, ty, is_return) in annotated {
            let mismatch = if let Some(definition) =
                namedtuples::annotation(graph, *module, annotation, &types)
                && types.namedtuple_definition(ty) != Some(definition)
            {
                "inferred value must be an instance of the annotated namedtuple"
            } else if graph.resolve(*module, annotation)
                == Some(Binding::Intrinsic(Intrinsic::ArgparseNamespace))
                && !matches!(ty, Type::Class(id) if classes.parsers.namespace(id))
            {
                "argparse.Namespace requires an inferred native parser namespace"
            } else {
                continue;
            };
            // The registered frame can return a handle even when the fixed layout fails.
            // Keep the original diagnostic for callers that require the fixed contract.
            if is_return && ty == Type::PythonObject && graph.modules[*module].is_program_source() {
                capture();
            }
            return Err(graph.modules[*module].diagnostic(annotation, mismatch));
        }
    }
    let entry_declarations = classes.declarations.has_class_values(graph.entry)
        || graph.modules[graph.entry].parsed.syntax().body.iter().any(|stmt| {
            matches!(stmt, ast::Stmt::FunctionDef(def) if inspect::marks_function(graph, graph.entry, def))
        });
    let helpers = Helpers::new(inference.len() + graph.initializers(entry_declarations).count());
    let globals = ModuleValues::for_initializers(needs_registered_entry);
    let named = graph.registered_startup && !needs_registered_entry;
    let recover_completed = std::cell::Cell::new(named);
    let defer_capture = !named && !needs_registered_entry && cache.is_none();
    let capture_on_object = |failure: &Failure| {
        if failure.is_object() || needs_registered_entry {
            if defer_capture {
                // A settled body needs no further contract check. Keep its existing
                // recovery path while the remaining procedures finish validation.
                if !recover_completed.replace(true)
                    && let Some(prepared) = pending_capture.borrow_mut().as_mut()
                {
                    capture_slots(
                        prepared,
                        &mut slots().filter(|id| checked_bodies.contains(id)),
                    );
                }
            } else {
                capture();
            }
        }
    };
    if needs_registered_entry {
        // A checked body or a real function-value read can establish the registered binding.
        globals.missing_bindings.borrow_mut().extend(
            registered_parameters
                .iter()
                .filter(|(_, original)| !original.kind.is_object())
                .map(|(&id, original)| (id, original.clone())),
        );
    }
    let mut initializations = Vec::new();
    let mut unavailable = None;
    let mut available = HashSet::new();
    for (module_id, range) in graph.initializers(entry_declarations) {
        let checker = Checker::new(
            graph,
            module_id,
            &signatures,
            &classes,
            &types,
            &helpers,
            &globals,
        );
        let result = Initialization::lower(
            checker,
            &mut available,
            range,
            (!needs_registered_entry).then_some(&mut initializations),
        );
        checked_region(
            result,
            graph,
            module_id,
            &mut unavailable,
            capture_on_object,
        )
        .inspect_err(|_| {
            if defer_capture && recover_completed.get() {
                capture();
            }
        })?;
    }
    // After an initializer requests recovery, a checked procedure can supply both
    // its source contracts and its Python frame, just as it does on a named entry.
    let place_procedures = recover_completed.get();
    if needs_registered_entry {
        capture();
    }
    if let Some(cache) = cache.as_deref_mut() {
        cache.validate_abi(&definitions, &globals);
    }
    let mut completed = FixedFunctions::default();
    let register = |completed: FixedFunctions, owners: &[ModuleId]| {
        completed.register(graph, owners, |module, range| {
            let checks = &inference.source_checks;
            if let Some(accepted) = registered::SourceChecks::cached(graph, module, range) {
                return accepted;
            }
            if let Some(solved) = &checks.solved {
                solved
                    .borrow_mut()
                    .push((module, range, registered::SourceFrame::Fixed));
                true
            } else {
                checks.accepts(graph, module, range)
            }
        });
    };
    let place_completed = |completed: &mut FixedFunctions, owners: &[ModuleId]| {
        if recover_completed.get() {
            register(std::mem::take(completed), owners);
            capture();
        }
    };
    let checker_for = |module| {
        Checker::new(
            graph,
            module,
            &signatures,
            &classes,
            &types,
            &helpers,
            &globals,
        )
    };
    // Other fixed definitions can still complete after a body requests an object
    // frame. Place those operations before recovery selects a second body.
    let request_recovery = |failure: &Failure| {
        if !needs_registered_entry && failure.is_object() {
            recover_completed.set(true);
        }
    };
    let mut has_wrapper_link = false;
    let mut completed_calls = classes.nonreturning_functions.clone();
    let mut body_checks = Some(&checked_bodies);
    let mut zip_sums = Vec::new();
    let mut owners = Vec::new();
    let mut call_graph = Vec::new();
    let mut class_reads = Vec::new();
    let mut recursion_errors = Vec::new();
    let mut body_reports = Vec::new();
    for (module_id, source, signature) in &definitions {
        let definition = source.as_ref();
        let module = &graph.modules[*module_id];
        let mut checker = checker_for(*module_id);
        checker.nonreturning_functions = &completed_calls;
        // A refusal in a named body leaves other definitions available to the launcher.
        let mut refused = |mut report: Diagnostic| {
            if signature.method.is_none() {
                report.in_function(
                    graph,
                    callable_definitions[signature.id],
                    phase::BodyCheck::Lowering,
                );
            }
            let check_siblings = !body_reports.is_empty() || report.function_site().is_some();
            body_reports.push(report);
            check_siblings
        };
        let original = registered_parameters.get(&signature.id);
        if suspended_bodies.contains(&signature.id) {
            continue;
        }
        let accepted = signature.registered_frame()
            && inference
                .source_checks
                .accepts(graph, *module_id, definition.range());
        if accepted || original.is_some() {
            // A checked Python frame binds its imports when the function runs. A
            // callable read also establishes this namespace for a recovered signature.
            checker.bound_function_imports =
                accepted || signature.registered_frame() && inference.used_as_value(signature.id);
            match registered::lower_definition(checker, definition, signature) {
                Ok(body) if accepted => {
                    graph.register_function(*module_id, definition.start(), body);
                    globals.missing_bindings.borrow_mut().remove(&signature.id);
                }
                Ok(_) => {
                    if let Some(original) = original {
                        globals
                            .missing_bindings
                            .borrow_mut()
                            .insert(signature.id, original.clone());
                    }
                }
                Err(error) if error.is_object() => {}
                Err(error) => {
                    if !refused(error.terminal_in(graph, *module_id)) {
                        break;
                    }
                }
            }
            continue;
        }
        // This graph will emit registered frames. Inference already checked this exact
        // definition with the settled signature; recovery owns its executable checked body.
        if (needs_registered_entry || unavailable.is_some())
            && body_checks.is_some_and(|ids| ids.contains(&signature.id))
            && graph
                .registered_functions()
                .get(module_id)
                .is_some_and(|bodies| bodies.contains_key(&definition.start()))
        {
            continue;
        }
        // A bound wrapper with an unknown owner may need the method's Python
        // frame. Keep the operand sites until its specialized layout is known.
        let python_method = graph.registered_startup || has_wrapper_link;
        let prepare_sites = !needs_registered_entry
            && (signature.method.is_none() || python_method)
            && signature.physical_frame;
        let inferred = if signature.method.is_some() && prepare_sites {
            pending_capture
                .borrow_mut()
                .as_mut()
                .and_then(|prepared| prepared.remove(&signature.id))
                .and_then(StdResult::ok)
        } else {
            None
        };
        checker.fixed_sites = (prepare_sites && inferred.is_none())
            .then(|| fixed_body_source::SourceSites::new(python_method));
        checker.placement_only = (named || unavailable.is_some()) && cache.is_none();
        let mut attributes = HashMap::new();
        if signature.method.is_none()
            && checker.fixed_sites.is_some()
            && cache.is_none()
            && pending_capture.borrow().is_some()
        {
            checker.settled_attributes = Some(&mut attributes);
        }
        let source_function = cache.is_none() && signature.method.is_none();
        let physical_function = source_function && signature.physical_frame;
        let named_scalar = named
            && source_function
            && definition.returns.is_some()
            && signature.returns.builtin_scalar_name().is_some();
        let mut callable_inputs = pending_capture.borrow_mut().as_mut().and_then(|prepared| {
            if named && physical_function && matches!(prepared.get(&signature.id), Some(Err(_))) {
                prepared.remove(&signature.id)?.err()
            } else {
                None
            }
        });
        let result = if let Some(body) = inferred {
            Ok(body)
        } else if place_procedures
            && physical_function
            && signature
                .params
                .iter()
                .all(|p| p.default.is_none() && !p.sign_only)
            && ((named_scalar
                && unavailable.is_none()
                && signature.may_have_display
                // Eager displays cannot supply a fixed source operand to this frame.
                // Keep first-party calls and method bindings on their fixed contract.
                && graph.session.function_reads_only(
                    *module_id,
                    definition,
                    crate::analysis::EagerDisplays::Required,
                    |_| true,
                    |name| module_builtin_name(module, name),
                ))
                || (signature.returns == Type::None
                    && definition.returns.is_some()
                    && !signature.params.is_empty()
                    && signature.parameter_declarations.len() == signature.params.len()
                    && signature.parameter_declarations.iter().all(Option::is_some))
                || (named
                    && definition.returns.is_none()
                    && (callable_inputs.is_some()
                        || inference.has_inferred_protocol_argument(signature, &types)
                            && (unavailable.is_none()
                                || registered::SourceChecks::cached(
                                    graph,
                                    *module_id,
                                    definition.range(),
                                ) == Some(true)))))
            || named_scalar && signature.takes_positional_tuple(source, &graph.session)
        {
            registered::place_checked_definition(
                &checker,
                definition,
                signature,
                &inference,
                callable_inputs.as_mut(),
            )
            .ok()
            .flatten()
            .map(|body| (body, Default::default()))
            .map_or_else(|| lower_body(checker, definition, signature), Ok)
        } else if named_scalar && signature.physical_frame && unavailable.is_none() {
            lower_function(checker, definition, signature, None, Some(&inference))
        } else if let Some(cache) = cache.as_deref_mut().filter(|_| signature.method.is_none()) {
            cache.body(checker, definition, signature)
        } else {
            lower_body(checker, definition, signature)
        };
        if !attributes.is_empty()
            && let Some(prepared) = pending_capture.borrow_mut().as_mut()
        {
            prepared.insert(signature.id, Err(attributes));
        }
        let cannot_return = match &result {
            Ok((body, _)) => body.cannot_return,
            Err(refusal) => refusal.cannot_return,
        };
        let placed = match checked_region(
            result.map_err(|refusal| refusal.failure),
            graph,
            *module_id,
            &mut unavailable,
            request_recovery,
        ) {
            Ok(body) => body,
            Err(report) => {
                if refused(report) {
                    continue;
                }
                break;
            }
        };
        if !body_reports.is_empty() {
            continue;
        }
        if cannot_return && completed_calls.insert(signature.id) && !needs_registered_entry {
            body_checks = None;
        }
        let Some((body, sums)) = placed else { continue };
        has_wrapper_link |= body.wrapper_link;
        if needs_registered_entry || (unavailable.is_some() && body.source.is_none()) {
            // After a fixed layout fails, only bodies with a source frame can be placed.
            // Later definitions still receive the checked transfer and wrapper facts above.
            continue;
        }
        let recursion_error = module.error(
            definition,
            "recursive calls are unsupported in this prototype",
        );
        // This completed call edge already prevents the trial from producing an acyclic program.
        if retries_failed_layout(graph) && body.calls.contains(&signature.id) {
            return Err(recursion_error.into());
        }
        let LoweredBody {
            function,
            calls,
            class_reads: mut reads,
            ..
        } = body;
        if let Some(source) = body.source
            && signature.physical_frame
        {
            let slot = completed.functions.len();
            completed.frames.push((slot, definition.range(), source));
        }
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
            types.register_cache_body(signature.id, raw, policy);
            let wrapper = function_caches::wrapper(signature, raw, &types, policy);
            (wrapper, HashSet::from([raw]), HashSet::new())
        } else {
            (function, calls, reads)
        };
        if sums.len() >= 2 && !classes.cached.contains_key(&signature.id) {
            zip_sums.push((completed.functions.len(), sums));
        }
        owners.push(*module_id);
        completed.functions.push(function);
        call_graph.push(calls);
        class_reads.push(reads);
        recursion_errors.push(recursion_error);
    }
    let mut reports = body_reports.into_iter();
    if let Some(primary) = reports.next() {
        place_completed(&mut completed, &owners);
        return Err(primary.with_following(reports));
    }
    let mut factories = Vec::with_capacity(record_factories.len());
    for class in record_factories {
        let module = classes.declarations.definitions[class].definition.module;
        let checker = checker_for(module);
        let body = checked_region(
            checker.record_factory_body(class),
            graph,
            module,
            &mut unavailable,
            request_recovery,
        )
        .inspect_err(|_| place_completed(&mut completed, &owners))?;
        if let Some(helper) = body {
            factories.push(helper);
        }
    }
    let mut startup = Vec::new();
    let mut entry_source = None;
    let entry = {
        let module_id = graph.entry;
        let mut checker = checker_for(module_id);
        checker.module_scope = true;
        checker.nonreturning_functions = &completed_calls;
        let mut fixed_entry = (!needs_registered_entry && unavailable.is_none()).then(|| {
            if let Some((prefix, tail)) = graph.modules[module_id].executable_suffix() {
                checker.fixed_sites = Some(fixed_body_source::SourceSites::default());
                // Leave room for the declarations and their Python statement checkpoints.
                Vec::with_capacity(tail.len() + 2 * prefix.len())
            } else {
                Vec::new()
            }
        });
        let result = checker.lower_block(&entry_statements, fixed_entry.as_mut());
        let result = checker.finish_region(result);
        if checked_region(result, graph, module_id, &mut unavailable, |failure| {
            // A FILE entry can select the same recovery as a checked function.
            if pending_capture.borrow().is_some() {
                request_recovery(failure);
            }
        })
        .inspect_err(|_| place_completed(&mut completed, &owners))?
        .is_some()
            && let Some(body) = fixed_entry
        {
            entry_source = checker.fixed_sites.take().and_then(|sites| {
                sites.finish(
                    entry_statements.len(),
                    &body,
                    std::mem::take(&mut checker.names),
                    &checker.locals,
                    None,
                    FrameArguments::Python,
                )
            });
            checker.fault_frame(&mut startup, "<module>", TextSize::default());
            Some(ir::Function {
                params: Vec::new(),
                returns: Type::None,
                locals: checker.locals,
                body,
            })
        } else {
            None
        }
    };
    if let Some(reason) = unavailable.or_else(|| {
        globals
            .missing_bindings
            .borrow_mut()
            .pop_first()
            .map(|(_, reason)| reason)
    }) {
        place_completed(&mut completed, &owners);
        return Err(reason);
    }
    if needs_registered_entry {
        return Ok(Some(crate::CheckedProgram::Registered(None)));
    }
    if let Some(error) = graph.deferred_object_error.take() {
        register(completed, &owners);
        if named || error.kind.is_object() {
            capture();
        }
        return Err(error);
    }
    let functions = &mut completed.functions;
    let fault_enabled = graph
        .command_startup
        .as_ref()
        .is_some_and(|(flags, _)| flags.faulthandler);
    let first_initializer = functions.len() + factories.len();
    let initializer_ids = first_initializer..first_initializer + initializations.len();
    let mut initialization_order = Vec::with_capacity(initializations.len());
    let module_bodies = initializations.into_iter().map(|initialization| {
        let module_id = initialization.order.module;
        let module = &graph.modules[module_id];
        initialization_order.push(initialization.order);
        comprehensions::Helper {
            module: module_id,
            function: initialization.function,
            calls: initialization.calls,
            class_reads: HashSet::new(),
            recursion_error: module
                .error(module.parsed.syntax(), "recursive module initialization"),
            source_frame: module.archive.is_none(),
        }
    });
    // Entry code and inactive module guards can create helpers too. Validate the
    // complete graph only after every source expression has been lowered.
    let list_builds = helpers.list_builds.take();
    let mut python_frames: Vec<_> = owners
        .iter()
        .map(|&module| graph.modules[module].archive.is_none())
        .collect();
    for helper in factories
        .into_iter()
        .chain(module_bodies)
        .chain(helpers.take_functions())
    {
        python_frames.push(helper.source_frame);
        owners.push(helper.module);
        functions.push(helper.function);
        call_graph.push(helper.calls);
        class_reads.push(helper.class_reads);
        recursion_errors.push(helper.recursion_error);
    }
    let mut entry = entry.expect("source graph has an entry module");
    let mut entry_operations = EntryOperations::default();
    let mut callable_targets = crate::callables::TargetCollector::default();
    let mut names = false;
    let mut uses_builtin_module_names = false;
    let mut generator_sites = 0;
    let mut count = false;
    let mut missing_key_types = HashSet::new();
    let mut decoded = Vec::new();
    let mut encoded = Vec::new();
    let mut record_builtin = |value: &Expr| match value.kind {
        ExprKind::Builtin(Builtin::JsonDecode(ty) | Builtin::TomlDecode(ty), _) => decoded.push(ty),
        ExprKind::Builtin(Builtin::JsonEncode(ty), _) => encoded.push(ty),
        ExprKind::Builtin(
            Builtin::DictGet | Builtin::DeleteItem | Builtin::DictPop | Builtin::ChainMapGetItem,
            ref args,
        ) if graph.reachable_only() => {
            if let [_, key] = args.as_slice() {
                missing_key_types.insert(key.ty);
            }
        }
        ExprKind::Builtin(Builtin::SysGetFrameModuleName, _) => names = true,
        ExprKind::Builtin(Builtin::SysBuiltinModuleNames, _) => uses_builtin_module_names = true,
        ExprKind::Builtin(Builtin::SysSetRecursionLimit, _) => count = true,
        ExprKind::Builtin(Builtin::GeneratorFrameEnter(_), _) => generator_sites += 1,
        _ => {}
    };
    let mut validate_calls = |functions: &[ir::Function]| -> StdResult<(), Diagnostic> {
        typing::validate_functions(functions, &entry, &types, graph, &callable_definitions)?;
        let mut indirect = Vec::new();
        let mut storage = Ok(());
        for (slot, function) in functions.iter().chain([&entry]).enumerate() {
            let checked = callables::validate_storage(function, &types, |value| {
                callable_targets.inspect(slot, &types, value);
                record_builtin(value);
                entry_operations.record(value);
                if let Some(calls) = call_graph.get_mut(slot) {
                    if let Some(targets) = globals.invoked_functions(value) {
                        calls.extend(targets.iter().copied());
                    } else if let Some(signature) = callables::invoked_signature(value) {
                        indirect.push((slot, signature));
                    }
                }
            });
            storage = storage.and(checked);
        }
        // An invalid coroutine adapter precedes a storage error anywhere in the program.
        callable_targets.deduplicate();
        callables::validate_targets(functions, &types, &callable_targets.constructed)?;
        storage?;
        for (slot, signature) in indirect {
            if let Some(targets) = callable_targets.constructed.get(&signature) {
                call_graph[slot].extend(targets.iter().map(|target| target.function));
            }
        }
        if let Some(id) = recursive_call_root(&call_graph) {
            return Err(recursion_errors.swap_remove(id).into());
        }
        globals.validate_initializers(
            initializer_ids.clone().zip(&initialization_order),
            graph,
            functions,
            &class_reads,
            &callable_definitions,
            &callable_targets.constructed,
        )?;
        Ok(())
    };
    validate_calls(functions).inspect_err(|_| {
        // A failed layout trial goes through ordinary lowering before registration.
        if graph.layout_hint.get() != crate::modules::LayoutHint::Applied {
            place_completed(&mut completed, &owners);
        }
    })?;
    let functions = &mut completed.functions;
    python_frames.push(true); // The program entry runs a Python module body.
    let source_functions = python_frames.iter().filter(|&&frame| frame).count();
    // The native call graph is acyclic. Programs that cannot inspect frames,
    // lower their limit, or reach the default depth need no tracking at all.
    let track_frames = fault_enabled || names || count || source_functions + generator_sites > 1000;
    if generator_sites != 0 && !track_frames {
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
    if !missing_key_types.is_empty()
        && let Some(key_reprs) = key_reprs
    {
        let before = key_reprs.len();
        classes.missing_key_reprs(missing_key_types, graph, &types, key_reprs);
        if key_reprs.len() != before && cache.is_none() {
            return Ok(None);
        }
    }
    let json_records = json::schemas(graph, &classes, &types, &decoded, &encoded)
        .inspect_err(|_| place_completed(&mut completed, &owners))?;
    let functions = &mut completed.functions;
    // Fusing adjacent sums also inlines their generator bodies and scalar calls.
    if !track_frames && !zip_sums.is_empty() {
        for (id, sums) in zip_sums {
            zip_sums::fuse(functions, id, &sums);
        }
        // Fusion can replace checked operands. Frame markers carry no operands.
        entry_operations = EntryOperations::default();
        callable_targets = crate::callables::TargetCollector::default();
        for (id, function) in functions.iter().chain([&entry]).enumerate() {
            ir::walk_expressions(&function.body, &mut |expr| {
                entry_operations.record(expr);
                callable_targets.inspect(id, &types, expr);
            });
        }
    }
    if let Some(cache) = cache {
        cache.finish(&types);
    }
    let selected = registered_entry(graph, functions, &entry, entry_operations)
        .inspect_err(|_| place_completed(&mut completed, &owners))?;
    if let Some(registered) = selected {
        for (order, function) in initialization_order
            .into_iter()
            .zip(completed.functions.drain(initializer_ids))
        {
            if let Some(source) = order.source
                && let Ok(frame) = source.place(function)
            {
                graph.register_initializer(order.module, frame);
            }
        }
        if let Some(frame) = entry_source.and_then(|source| source.place(entry).ok()) {
            graph.register_initializer(graph.entry, frame);
        }
        register(completed, &owners);
        if recover_completed.get() {
            capture();
        }
        return Ok(Some(registered));
    }
    if !can_transfer_frames && !graph.has_object_constructor_fallback.get() {
        completed.frames = Vec::new();
    }
    let mut functions = completed.functions;
    // Checkpoints above refer to the source operations. Insert native startup
    // bookkeeping into these same bodies only after their source order is checked.
    for id in initializer_ids {
        let module_id = owners[id];
        let module = &graph.modules[module_id];
        let prefix = if module.archive.is_some() {
            Some(Statement::Expr(Expr {
                ty: Type::None,
                kind: ExprKind::Builtin(
                    Builtin::ExtensionImport(module.name.rsplit('.').next().unwrap().to_owned()),
                    vec![],
                ),
            }))
        } else if fault_enabled && module_id != graph.entry {
            Some(fault_frame_statement(
                graph
                    .module_filename(module_id)
                    .to_string_lossy()
                    .into_owned(),
                "<module>",
                1,
            ))
        } else {
            None
        };
        if let Some(prefix) = prefix {
            functions[id].body.insert(0, prefix);
        }
        startup.push(Statement::Expr(Expr {
            ty: Type::None,
            kind: ExprKind::Call(id, ir::CallArgs::positional(Vec::new())),
        }));
    }
    entry.body.splice(..0, startup);
    let source_frames = if track_frames {
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
    globals
        .class_data
        .borrow_mut()
        .extend_layouts(&mut classes.fields);
    let mut globals = globals.into_types();
    types.reserve_constructor_globals(&mut globals, &callable_targets.constructors);
    let lowered = (
        ir::Module {
            builtin_module_sources: None,
            command_flags: graph
                .command_startup
                .as_ref()
                .map(|(flags, _)| flags.clone()),
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
    );
    Ok(Some(crate::CheckedProgram::Native(Box::new(
        crate::FixedProgram {
            ir: lowered,
            callables: callable_targets,
            frame_sources: completed.frames,
            uses_builtin_module_names,
        },
    ))))
}

#[derive(Default)]
struct EntryOperations {
    first_object: Option<(ir::ObjectCall, usize, TextRange)>,
    orders_strings: bool,
    creates_character: bool,
}

impl EntryOperations {
    fn record(&mut self, expr: &Expr) {
        match &expr.kind {
            ExprKind::Builtin(Builtin::ObjectCall(call, module, span), _)
                if self.first_object.is_none_or(|(kind, id, range)| {
                    (*call, *module, span.start()) < (kind, id, range.start())
                }) =>
            {
                self.first_object = Some((*call, *module, *span));
            }
            ExprKind::Builtin(Builtin::Chr, _) => self.creates_character = true,
            ExprKind::Compare(values, operators) => {
                self.orders_strings |= values.iter().zip(operators).any(|(value, op)| {
                    matches!(value.ty, Type::Str | Type::StrLiteral(_))
                        && !matches!(op, CmpOp::Eq | CmpOp::NotEq)
                });
            }
            _ => {}
        }
    }
}

/// Decide startup from the checked operations before inserting native instructions.
fn registered_entry(
    graph: &SourceGraph,
    functions: &[ir::Function],
    entry: &ir::Function,
    operations: EntryOperations,
) -> StdResult<Option<crate::CheckedProgram>, Diagnostic> {
    // Native text is UTF-8. A possible lone surrogate has priority over a
    // registered initializer or another operation in this graph.
    if operations.orders_strings && operations.creates_character {
        let mut code_points = Vec::new();
        for function in functions.iter().chain([entry]) {
            ir::walk_expressions(&function.body, &mut |expr| {
                if let ExprKind::Builtin(Builtin::Chr, args) = &expr.kind {
                    code_points.push(&args[0]);
                }
            });
        }
        let ranges = crate::ranges::analyze_functions(functions.iter().chain([entry]));
        if code_points.into_iter().any(|value| {
            ranges
                .integer(value)
                .is_none_or(|range| range.min <= 0xdfff && range.max >= 0xd800)
        }) {
            graph.needs_object_runtime.set(true);
            return Err(Diagnostic::plain(
                " Imre: ordering strings from chr() with possible surrogate values requires native Python object operations".to_owned(),
            )
            .with_path(&graph.modules[graph.entry].path)
            .classified(Kind::ScopedObject));
        }
    }
    if graph.registered_startup {
        return Ok(Some(crate::CheckedProgram::Registered(None)));
    }
    let reason = graph
        .runtime_initializer
        .clone()
        .map(Into::into)
        .or_else(|| {
            let (call, id, operand) = operations.first_object?;
            let (name, kind) = match call {
                ir::ObjectCall::ExecText => ("exec", Kind::ScopedObject),
                ir::ObjectCall::FormatField => ("this f-string field", Kind::Ordinary),
                ir::ObjectCall::SocketSendfile => ("socket.sendfile", Kind::Object),
            };
            let source = &graph.modules[id];
            Some(
                Diagnostic::at(
                    &source.source,
                    &operand,
                    format!("{name} requires native Python object operations"),
                )
                .with_path(&source.path)
                .classified(kind),
            )
        });
    Ok(reason.map(|reason| crate::CheckedProgram::Registered(Some(reason))))
}

#[derive(Clone)]
struct LoweredBody {
    cannot_return: bool,
    frame_arguments: FrameArguments,
    wrapper_link: bool,
    function: ir::Function,
    source: Option<FixedBodySource>,
    calls: HashSet<usize>,
    class_reads: HashSet<DefinitionId>,
    default_reads: Option<Vec<caller_cache::DefaultId>>,
}

type CompletedBody = (LoweredBody, HashMap<usize, zip_sums::Sum>);
struct BodyRefusal {
    failure: Failure,
    cannot_return: bool,
}

impl From<Failure> for BodyRefusal {
    fn from(failure: Failure) -> Self {
        Self {
            failure,
            cannot_return: false,
        }
    }
}

type BodyResult = StdResult<CompletedBody, BodyRefusal>;
type PreparedBody = StdResult<CompletedBody, HashMap<TextRange, FixedAttribute>>;

type FunctionDefinition<'a> = (ModuleId, Cow<'a, ast::StmtFunctionDef>, Signature);

fn lower_body(
    checker: Checker<'_>,
    definition: &ast::StmtFunctionDef,
    signature: &Signature,
) -> BodyResult {
    lower_function(checker, definition, signature, None, None)
}

fn lower_function(
    mut checker: Checker<'_>,
    definition: &ast::StmtFunctionDef,
    signature: &Signature,
    registered_locals: Option<&mut HashMap<usize, registered::LocalBinding>>,
    placement: Option<&inference::Inference>,
) -> BodyResult {
    debug_assert_eq!(checker.registered.is_some(), registered_locals.is_some());
    #[cfg(test)]
    caller_cache::record_lowering(definition.name.as_str());
    let module = &checker.graph.modules[checker.module];
    let classes = checker.classes;
    let types = checker.types;
    checker.frame = LexicalFrame::Named(definition.name.id.clone());
    checker.positional_pack = definition
        .parameters
        .vararg
        .as_deref()
        .filter(|p| p.annotation().is_some())
        .map(|p| p.name.to_string());
    checker.interactive_names = module.is_doctest_case() && definition.name.as_str() == "_run_case";
    let mut params = Vec::new();
    checker.bind_function_signature(definition, signature, |slot| params.push(slot))?;
    if checker.checked_call_contracts && definition.returns.is_none() {
        // The Python frame returns the caller's live result. Inference keeps the fixed result type.
        checker.returns = Type::PythonObject;
    }
    // A live Python frame calls the original string methods and iterates the original list.
    if checker.registered_local_start.is_none() {
        checker.joined_lines = joined_lines::find(&definition.body);
        checker.consuming_lists = consuming_lists::find(&definition.body, |func| {
            checker.unshadowed_builtin(func, "sorted")
        });
    }
    // A function header owns its leading docstring. Keep a sole statement for the
    // implicit return and keep instrumented bodies on their ordinary line path.
    let statements =
        if checker.fixed_sites.is_some() && definition.body.len() > 1 && !checker.fault_enabled() {
            ast::helpers::body_without_leading_docstring(&definition.body)
        } else {
            &definition.body
        };
    let mut cannot_return = false;
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
                .collect::<StdResult<_, _>>()?,
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
        let mut body = Vec::new();
        let mut placed = None;
        let result = if let Some(inference) = placement {
            let helpers = checker.helpers.len();
            checker.function_body(
                statements,
                Some(&mut body),
                Some(&mut |cx| {
                    if cx.helpers.len() != helpers || cx.unrepresented.borrow().is_some() {
                        return false;
                    }
                    placed = registered::place_checked_definition(
                        cx, definition, signature, inference, None,
                    )
                    .ok()
                    .flatten();
                    placed.is_some()
                }),
            )
        } else {
            checker.function_body(statements, Some(&mut body), None)
        };
        if let Some(body) = placed {
            return Ok((body, HashMap::new()));
        }
        match result {
            Ok(falls) => {
                cannot_return = checker.checked_call_cannot_return(definition, falls);
                (body, falls)
            }
            Err(failure) => return checker.finish_region(Err(failure)).map_err(Into::into),
        }
    };
    if let Some(context) = signature.method
        && context.generated_initializer(classes.declarations)
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
    if falls_through {
        match checker.returns {
            Type::None => {}
            // A registered frame returns the None handle on fallthrough.
            Type::PythonObject if checker.registered.is_some() => {}
            ty @ Type::Optional(_) => body.push(Statement::Return(Expr::absent(ty))),
            _ => {
                return Err(error(
                    &module.source,
                    definition,
                    "not every path returns the declared type",
                )
                .into());
            }
        }
    }
    if classes.generators.contains(&signature.id) {
        body = checker.generator_body(body);
    }
    // Placement can require a Python frame after the entire suite has been checked.
    checker
        .finish_region(Ok(()))
        .map_err(|failure| BodyRefusal {
            cannot_return,
            failure,
        })?;
    let mut source = None;
    if let Some(names) = registered_locals {
        names.extend(checker.namespace_bindings.drain());
        names.extend(checker.names.drain().map(|(name, slot)| {
            (
                slot,
                registered::LocalBinding {
                    name,
                    namespace: None,
                },
            )
        }));
        // The Python binder installs the public name even when the fixed checker renamed a local.
        for (slot, parameter) in params.iter().zip(&signature.params) {
            if let Some(binding) = names.get_mut(slot) {
                binding.name.clone_from(&parameter.name);
            }
        }
    } else if module.archive.is_none() {
        checker.fault_frame(&mut body, definition.name.as_str(), definition.start());
        source = checker.fixed_body_source(definition, signature, statements.len(), &body);
    }
    Ok(checker.completed_body(params, body, source, cannot_return))
}

fn signature(
    function: &ast::StmtFunctionDef,
    graph: &SourceGraph,
    defaults: keyword_forwarding::Defaults,
    classes: &Classes,
    types: &Types,
    method: Option<records::MethodContext>,
    inference: &inference::Inference,
) -> StdResult<Signature, String> {
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
    if method.is_none() && function.type_params.is_some() {
        let postponed = imre_ast::future_annotations(&graph.modules[module].parsed.syntax().body);
        if crate::dynamic::function_type_params(function, postponed, false, None).is_some() {
            graph.needs_object_runtime.set(true);
        }
    }
    // The class collector has already resolved method decorators to their descriptor kinds.
    if function.type_params.is_some() || (method.is_none() && !function.decorator_list.is_empty()) {
        if function.decorator_list.iter().any(|decorator| {
            decorator.expression.as_call_expr().is_some_and(|call| {
                graph.resolve(module, &call.func) == Some(Binding::Intrinsic(Intrinsic::Wraps))
                    && graph
                        .session
                        .is_stdlib_function(module, &call.func, "functools", "wraps")
            })
        }) {
            graph.needs_object_runtime.set(true);
        }
        return Err(diagnostic_message(
            source,
            function,
            "decorators and type parameters are unsupported",
        ));
    }
    let mut params = Vec::new();
    let mut parameter_types = Vec::new();
    let mut parameter_declarations = Vec::new();
    let mut declare = |index: usize, module: ModuleId, annotation: Option<&ast::Expr>| {
        if let Some(annotation) = annotation
            && graph.modules[module].checks_source_contracts()
        {
            parameter_declarations.resize(index + 1, None);
            parameter_declarations[index] = Some(TypeDeclaration {
                module,
                annotation: annotation.range(),
            });
        }
    };
    let mut none_defaults = Vec::new();
    let mut document_parameters = Vec::new();
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
        let mut fixed_default =
            parameter
                .default
                .as_deref()
                .and_then(|value| match graph.resolve(module, value) {
                    Some(Binding::Intrinsic(Intrinsic::Fnmatch(ir::FnmatchOperation::Match {
                        normcase,
                    }))) => Some(fnmatch::function(types, normcase)),
                    Some(Binding::Intrinsic(Intrinsic::WrapperAttributes(fields))) => {
                        Some(functools::wrapper_attributes(fields, types))
                    }
                    _ => None,
                });
        let annotation = parameter.annotation().filter(|value| {
            graph.resolve(module, value) != Some(Binding::Intrinsic(Intrinsic::ArgparseNamespace))
                && namedtuples::annotation(graph, module, value, types).is_none()
        });
        let ty = if index == 0
            && method.is_some_and(|m| m.kind(classes) == crate::classes::MethodKind::Class)
        {
            Some(Type::BuiltinClass)
        } else if let Some(receiver) = receiver
            && index == 0
        {
            if let Some(declared) = parameter.annotation()
                && annotation_in_class(declared, graph, module, classes, types, owner)? != receiver
            {
                return Err(diagnostic_message(
                    source,
                    parameter,
                    "self annotation must name its exact class",
                ));
            }
            Some(receiver)
        } else {
            annotation.map(|value| {
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
                    } else if let Some(layout) = documents::annotation(graph, module, value) {
                        document_parameters.push(index);
                        Ok(if layout == documents::LIST {
                            types.list(Type::ObjectValue)
                        } else {
                            Type::ObjectValue
                        })
                    } else {
                        annotation_in_class(value, graph, module, classes, types, owner)
                    }
                })
                .or_else(|| parameter
                    .default
                    .as_deref()
                    .filter(|value| !empty_sequence && !matches!(value, ast::Expr::Dict(dict) if dict.items.is_empty() && parameter.annotation().is_some()))
                    .map(|value| {
                        if inspect::is_sentinel(graph, module, value) {
                            Ok(Type::PlainObject)
                        } else if let Some(default) = &fixed_default {
                            Ok(default.ty)
                        } else {
                            Ok(fixed_default.insert(calls::scalar_literal(value, source)?).ty)
                        }
                    }))
                .transpose()?
        };
        if parameter.annotation().is_none()
            && matches!(
                parameter.default.as_deref(),
                Some(ast::Expr::NoneLiteral(_))
            )
        {
            none_defaults.push(index);
        }
        if ty == Some(Type::JsonObject) {
            document_parameters.push(index);
        }
        parameter_types.push(ty);
        // Unresolved types exist only in the discarded discovery pass. solve()
        // fills every slot before ordinary lowering can construct executable IR.
        let ty = ty.unwrap_or(Type::None);
        let kind = ParameterKind::fixed(&function.parameters, index);
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
                        kind: ExprKind::Dict(Vec::new()),
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
                } else if let Some(default) = fixed_default {
                    if default.ty != ty {
                        return Err(diagnostic_message(
                            source,
                            value,
                            "default requires its exact type",
                        ));
                    }
                    Ok(default)
                } else {
                    calls::scalar_default(value, ty, types, source)
                }
            })
            .transpose()?;
        declare(params.len(), module, annotation);
        params.push(Parameter {
            name: parameter.name().to_string(),
            ty,
            kind,
            default,
            sign_only: ty == Type::Float
                && parameter.annotation().is_some()
                && only_compares_zero(&function.body, parameter.name().as_str()),
        });
    }
    let pack_element = |annotation: &ast::Expr| match annotation_in_class(
        annotation, graph, module, classes, types, owner,
    ) {
        Err(_)
            if graph.resolve_type(module, annotation)
                == Some(Binding::Intrinsic(Intrinsic::Any)) =>
        {
            Ok(Type::ObjectValue)
        }
        layout => layout,
    };
    if let Some(pack) = function.parameters.vararg.as_deref() {
        // Annotated packs use homogeneous storage. Without an annotation a direct
        // call can instead determine the exact (possibly heterogeneous) tuple.
        let ty = if let Some(annotation) = pack.annotation() {
            let element = pack_element(annotation)?;
            sequences::check_pack_reads(function, pack.name.as_str(), graph, module, source)?;
            Some(types.list(element))
        } else {
            None
        };
        parameter_types.push(ty);
        declare(params.len(), module, pack.annotation());
        params.push(Parameter {
            name: pack.name.to_string(),
            ty: ty.unwrap_or(Type::None),
            kind: ParameterKind::Variadic,
            default: None,
            sign_only: false,
        });
    }
    if let Some(keyword) = &function.parameters.kwarg {
        let element = keyword.annotation.as_deref().ok_or_else(|| {
            diagnostic_message(
                source,
                &**keyword,
                "keyword rest parameters need an element annotation",
            )
        })?;
        let element = pack_element(element)?;
        let ty = types.dict(Type::Str, element);
        declare(params.len(), module, keyword.annotation());
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
    let document_return = function
        .returns
        .as_deref()
        .and_then(|value| documents::annotation(graph, module, value));
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
            } else if document_return == Some(documents::ANY) {
                Ok(Type::ObjectValue)
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
    let document_return = if inert {
        None
    } else if returns == Some(Type::JsonObject) {
        Some(documents::MAP)
    } else {
        document_return
    };
    inference.document_annotations(id, document_parameters, document_return);
    if !inert && graph.modules[module].archive.is_none() {
        inference.discover_list_elements(graph, module, Some(function));
    }
    Ok(Signature {
        id,
        physical_frame: false,
        may_have_display: false,
        asynchronous: function.is_async,
        parameter_declarations: parameter_declarations.into(),
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

fn error(source: &str, node: &impl Ranged, message: &str) -> Failure {
    Diagnostic::at(source, node, message.to_owned()).into()
}

pub(crate) fn diagnostic_message(source: &str, node: &impl Ranged, message: &str) -> String {
    Diagnostic::at(source, node, message).to_string()
}

fn annotation(
    expression: &ast::Expr,
    graph: &SourceGraph,
    module: ModuleId,
    classes: &Classes,
    types: &Types,
) -> StdResult<Type, String> {
    annotation_in_class(expression, graph, module, classes, types, None)
}

fn annotation_in_class(
    expression: &ast::Expr,
    graph: &SourceGraph,
    module: ModuleId,
    classes: &Classes,
    types: &Types,
    owner: Option<usize>,
) -> StdResult<Type, String> {
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
) -> StdResult<Type, String> {
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

/// Return the checked layout of an annotation naming this physical declaration.
fn declaration_annotation(
    definition: DefinitionId,
    graph: &SourceGraph,
    classes: &Classes,
    types: &Types,
) -> Option<Type> {
    // The integer-fd selector keeps its immutable key in both the mapping
    // and the ready list. The data field contains a boxed Python value.
    let owner = &graph.modules[definition.module];
    if owner.is_bundled("selectors")
        && types
            .namedtuple(definition)
            .is_some_and(|ty| ty.name == "SelectorKey")
    {
        return Some(types.namedtuple_layout(
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
        return Some(types.namedtuple_layout(
            crate::types::TupleOrigin::Declaration(definition),
            vec![classes.instance(id), Type::Str, Type::Bool],
        ));
    }
    if classes.exceptions.contains_key(&definition) {
        return Some(
            classes
                .ids
                .get(&definition)
                .copied()
                .map_or(Type::Exception, Type::NamedException),
        );
    }
    if classes.registered_classes.contains(&definition) {
        return Some(Type::PythonObject);
    }
    classes.ids.get(&definition).map(|&id| classes.instance(id))
}

fn annotation_with_aliases(
    expression: &ast::Expr,
    graph: &SourceGraph,
    (module, source): (ModuleId, &str),
    classes: &Classes,
    types: &Types,
    owner: Option<usize>,
    alias_depth: usize,
) -> StdResult<Type, String> {
    if alias_depth >= 32 {
        return Err(diagnostic_message(
            source,
            expression,
            "type aliases nest more than 32 levels or form a cycle",
        ));
    }
    let binding = graph.resolve_type(module, expression);
    if let Some(Binding::TypeAlias(origin, index)) = binding {
        let module = &graph.modules[origin];
        if let Some(value) = module.aliases[index].annotation_value(module) {
            return annotation_with_aliases(
                value,
                graph,
                (origin, &module.source),
                classes,
                types,
                owner,
                alias_depth + 1,
            );
        }
    }
    if let ast::Expr::StringLiteral(value) = expression {
        check_string(value, source)?;
    }
    if let Some(class) = graph.session.annotation_class(module, expression) {
        let (origin, name) = match class {
            crate::analysis::AnnotationClass::Linked(origin, name) => (origin, name),
            crate::analysis::AnnotationClass::Declaration(location) => {
                return Err(diagnostic_message(
                    source,
                    expression,
                    &format!("class declared at {location} has no linked native layout"),
                ));
            }
        };
        let body = &graph.modules[origin].parsed.syntax().body;
        let statement = body.partition_point(|stmt| stmt.end() <= name.start());
        if body
            .get(statement)
            .and_then(ast::Stmt::as_class_def_stmt)
            .is_some_and(|class| class.name.range() == name)
            && let Some(ty) = declaration_annotation(
                DefinitionId {
                    module: origin,
                    statement,
                },
                graph,
                classes,
                types,
            )
        {
            return Ok(ty);
        }
    }
    if let ast::Expr::StringLiteral(value) = expression {
        let contents = value.value.to_str();
        let parsed = ruff_python_parser::parse_expression(contents).map_err(|_| {
            diagnostic_message(source, expression, "invalid quoted type annotation")
        })?;
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
            diagnostic_message(
                source,
                expression,
                &format!("invalid quoted annotation: {message}"),
            )
        });
    }
    match binding {
        Some(Binding::Definition(definition)) => {
            if let Some(ty) = declaration_annotation(definition, graph, classes, types) {
                return Ok(ty);
            }
        }
        Some(Binding::Intrinsic(Intrinsic::SelfType)) => {
            return owner.map(|id| classes.instance(id)).ok_or_else(|| {
                diagnostic_message(
                    source,
                    expression,
                    "Self is supported only inside a fixed class",
                )
            });
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
        Some(Binding::Intrinsic(Intrinsic::JsonEncoderClass)) => return Ok(Type::JsonEncoder),
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
        Some(Binding::Intrinsic(Intrinsic::ObjectRuntimeExport(
            name @ ("urllib.parse.DefragResult" | "urllib.parse.DefragResultBytes"),
        ))) => {
            graph.needs_object_runtime.set(true);
            return Err(diagnostic_message(
                source,
                expression,
                &format!("{name} requires native Python object operations"),
            ));
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
            "range" => Ok(Type::Range),
            "type" => Ok(Type::BuiltinClass),
            "float" => Ok(Type::Float),
            "bool" => Ok(Type::Bool),
            "str" => Ok(Type::Str),
            "bytes" => Ok(Type::Bytes),
            "bytearray" => Ok(Type::ByteArray),
            "BaseException" | "Exception" => Ok(Type::Exception),
            "ImportError" => Ok(Type::ImportException),
            "object" => Ok(Type::ObjectValue),
            _ => Err(diagnostic_message(
                source,
                expression,
                "unsupported exact type annotation",
            )),
        },
        ast::Expr::NoneLiteral(_) => Ok(Type::None),
        ast::Expr::Subscript(value) if matches!(graph.resolve_type(module, &value.value), Some(Binding::Definition(id)) if graph.is_abstract_context_manager(id)) => {
            nested(&value.value)
        }
        ast::Expr::Subscript(value)
            if matches!(&*value.value, ast::Expr::Name(name) if name.id.as_str() == "type")
                && graph.resolve_type(module, &value.value).is_none() =>
        {
            let Type::Class(base) = nested(&value.slice)? else {
                return Err(diagnostic_message(
                    source,
                    expression,
                    "type[T] requires a fixed record class",
                ));
            };
            let class = &classes.definitions[base];
            let index = class.initializer.ok_or_else(|| {
                diagnostic_message(
                    source,
                    expression,
                    "type[T] requires a declared initializer",
                )
            })?;
            let params = &class.methods[index].parameters;
            if params.vararg.is_some() || params.kwarg.is_some() || !params.kwonlyargs.is_empty() {
                return Err(diagnostic_message(
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
                        diagnostic_message(
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
                .collect::<StdResult<Vec<_>, _>>()?;
            Ok(types.record_constructor(base, args))
        }
        ast::Expr::Subscript(subscript)
            if matches!(
                graph.resolve_type(module, &subscript.value),
                Some(Binding::Intrinsic(
                    Intrinsic::Mapping | Intrinsic::TYPING_MAPPING
                ))
            ) =>
        {
            if let ast::Expr::Tuple(types) = &*subscript.slice
                && let [key, value] = types.elts.as_slice()
                && nested(key)? == Type::Str
                && nested(value)? == Type::ObjectValue
            {
                return Ok(Type::ReadOnlyObjectMapping);
            }
            Err(diagnostic_message(
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
                return Err(diagnostic_message(
                    source,
                    expression,
                    "regex annotations require str",
                ));
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
                    return Err(diagnostic_message(
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
                return Err(diagnostic_message(
                    source,
                    expression,
                    "Callable requires an exact parameter list and return type",
                ));
            };
            let [ast::Expr::List(parameters), returns] = arguments.elts.as_slice() else {
                return Err(diagnostic_message(
                    source,
                    expression,
                    "Callable requires Callable[[T, ...], R]; ellipsis is unsupported",
                ));
            };
            let parameters = parameters
                .elts
                .iter()
                .map(nested)
                .collect::<StdResult<Vec<_>, _>>()?;
            Ok(types.callable(parameters, nested(returns)?))
        }
        ast::Expr::Subscript(value)
            if graph.resolve_type(module, &value.value)
                == Some(Binding::Intrinsic(Intrinsic::Optional)) =>
        {
            Ok(types.optional(nested(&value.slice)?))
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
                    .collect::<StdResult<Vec<_>, _>>()?
            } else {
                vec![nested(&value.slice)?]
            };
            types.union(&members).ok_or_else(|| {
                diagnostic_message(
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
                == Some(Binding::Intrinsic(Intrinsic::TYPING_IO)) =>
            {
                match nested(&value.slice)? {
                    Type::Str => Ok(Type::TextIO),
                    Type::Bytes => Ok(Type::BinaryIO),
                    _ => Err(diagnostic_message(
                        source,
                        &*value.slice,
                        "checked IO requires str or bytes",
                    )),
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
                    return Err(diagnostic_message(
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
                let element = if graph.resolve_type(module, &value.slice)
                    == Some(Binding::Intrinsic(Intrinsic::Any))
                {
                    Type::ObjectValue
                } else {
                    nested(&value.slice)?
                };
                Ok(types.list(element))
            }
            ast::Expr::Name(name)
                if name.id == "frozenset" && nested(&value.slice)? == Type::Str =>
            {
                Ok(Type::FrozenStrings)
            }
            ast::Expr::Name(name) if name.id == "set" => {
                let element = nested(&value.slice)?;
                if !types.hashable_key(element) {
                    return Err(diagnostic_message(
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
                    return Err(diagnostic_message(
                        source,
                        expression,
                        "dict requires key and value types",
                    ));
                };
                let [key, value] = arguments.elts.as_slice() else {
                    return Err(diagnostic_message(
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
                    return Ok(Type::JsonObject);
                }
                if !types.hashable_key(key_type) {
                    return Err(diagnostic_message(
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
                        return Err(diagnostic_message(
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
                        .collect::<StdResult<Vec<_>, _>>()?
                } else {
                    vec![nested(&value.slice)?]
                };
                Ok(types.tuple(fields))
            }
            _ => Err(diagnostic_message(
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
                diagnostic_message(
                    source,
                    expression,
                    "finite unions support str | bytes and nullable exact types",
                )
            })
        }
        _ => Err(diagnostic_message(
            source,
            expression,
            "unsupported type annotation",
        )),
    }
}

/// Ruff replaces surrogate escapes with U+FFFD in its analysis AST. Reject those
/// source literals rather than silently compiling a different Python value.
pub(crate) fn check_string(value: &ast::ExprStringLiteral, source: &str) -> StdResult<(), String> {
    for part in value.value.iter() {
        check_string_literal(part, source)?;
    }
    Ok(())
}

/// Checks each adjacent literal with its own raw prefix and content range.
fn check_string_literal(value: &ast::StringLiteral, source: &str) -> StdResult<(), String> {
    if value.flags.prefix().is_raw() {
        Ok(())
    } else {
        check_string_escapes(value, value.content_range(), source)
    }
}

/// Ruff decodes a lone surrogate to U+FFFD. Only that decoded character needs its spelling.
fn has_surrogate_escape(decoded: &str, range: TextRange, source: &str) -> bool {
    decoded.contains(char::REPLACEMENT_CHARACTER)
        && imre_ast::strings::replacements(&source[range])
            .any(|point| point != u32::from(char::REPLACEMENT_CHARACTER))
}

fn check_string_escapes(decoded: &str, range: TextRange, source: &str) -> StdResult<(), String> {
    if has_surrogate_escape(decoded, range, source) {
        return Err(diagnostic_message(
            source,
            &range,
            "surrogate string escapes are unsupported; strings must be valid UTF-8",
        ));
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

/// Materialize the physical filename only when a checked operation needs a traceback.
enum LexicalFrame {
    Named(ast::name::Name),
    Positioned(Arc<ir::FixedFrame>),
}

/// The compiler only retains these facts while the handler's binding is unchanged.
struct CaughtException {
    slot: usize,
    builtin_class: Option<&'static str>,
    fields: processes::CapturedFields,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FinallyScope {
    Outside,
    Protected,
    Cleanup,
}

struct Checker<'a> {
    frame: LexicalFrame,
    unhighlighted_call: Option<TextRange>,
    wrapper_link: std::cell::Cell<bool>,
    fixed_sites: Option<fixed_body_source::SourceSites>,
    /// After a layout refusal the outer fixed body can only be used with its source sites.
    placement_only: bool,
    /// The graph keeps this physical function's declaration and first-party call edges.
    checked_call_contracts: bool,
    /// Resolved operations from a fixed source that the live frame can still use.
    settled_attributes: Option<&'a mut HashMap<TextRange, FixedAttribute>>,
    nonreturning_functions: &'a HashSet<usize>,
    /// Locals in this body are bound in an existing Python activation.
    registered: Option<&'a registered::values::ModuleSource>,
    bound_function_imports: bool,
    checked_callback_effects: bool,
    /// First non-parameter slot of a solved-signature registered body, when being tried.
    registered_local_start: Option<usize>,
    /// An operation on a live parameter can require the Python binder during lowering.
    frame_arguments: FrameArguments,
    /// The first operation whose execution requires a different representation. All
    /// later source operations still see the facts from the ordinary checker.
    unrepresented: std::cell::RefCell<Option<Failure>>,
    namespace_calls: HashSet<ruff_text_size::TextRange>,
    module_namespace: assignments::ModuleNamespace,
    /// The actual namespace supplied to Python calls in the current lexical region.
    namespace: Option<usize>,
    namespace_bindings: HashMap<usize, registered::LocalBinding>,
    /// Doctest prompts share an exec-style namespace until the case ends.
    interactive_names: bool,
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
    classes: &'a ClassInfo<'a>,
    types: &'a Types,
    helpers: &'a Helpers,
    globals: &'a ModuleValues,
    inference: Option<&'a inference::Inference>,
    inferred_returns: Option<Vec<Type>>,
    return_declaration: Option<phase::TypeDeclaration>,
    /// An explicit return can still escape the enclosing cleanup.
    explicit_return: bool,
    discover_document_return: Option<Type>,
    comprehension_targets: HashSet<String>,
    comprehension_exports: Option<HashMap<String, usize>>,
    narrowed: assignments::Narrowed,
    regex_guards: Vec<(usize, regex::CaptureLayout)>,
    present_subscripts: Vec<ruff_text_size::TextRange>,
    regex_locals: HashMap<usize, regex::CaptureLayout>,
    /// Group dictionary locals whose complete set of reads resolves to capture slots.
    regex_fields: HashMap<usize, HashMap<String, usize>>,
    joined_lines: joined_lines::Plan,
    consuming_lists: consuming_lists::Plan,
    /// Stored fields known to be present at the current read, by receiver slot and field index.
    present_fields: Vec<(usize, usize)>,
    caught_exceptions: Vec<CaughtException>,
    returns: Type,
    /// Inferred containers can acquire a new storage layout at an outermost plain assignment.
    inferred_containers: HashSet<usize>,
    /// Annotations constrain later assignments to the same lexical slot.
    declared_slots: HashMap<usize, TypeDeclaration>,
    /// Fresh annotated lists whose inner lists may come from a document.
    document_list_sites: HashMap<usize, u32>,
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
    finally_scope: FinallyScope,
    reachable: bool,
    suite_imports: bool,
    module_scope: bool,
    class: Option<usize>,
    class_receiver: Option<String>,
    /// Earlier assignments in the executing class body shadow module definitions.
    class_lexical: HashSet<String>,
    super_method_owner: Option<usize>,
    iterator_source: Option<TextRange>,
    super_initializer: Option<records::BaseInitializer>,
    exception_formatter: Option<bool>,
    initializing: bool,
}

impl<'a> Checker<'a> {
    /// Move the function and the dependencies established while checking its operations together.
    fn completed_body(
        self,
        params: Vec<usize>,
        body: Vec<Statement>,
        source: Option<FixedBodySource>,
        cannot_return: bool,
    ) -> CompletedBody {
        (
            LoweredBody {
                cannot_return,
                frame_arguments: self.frame_arguments,
                wrapper_link: self.wrapper_link.get(),
                source,
                function: ir::Function {
                    params,
                    returns: self.returns,
                    locals: self.locals,
                    body,
                },
                calls: self.calls,
                class_reads: self.class_reads.into_inner(),
                default_reads: self.default_reads,
            },
            self.zip_sums,
        )
    }

    fn new(
        graph: &'a SourceGraph,
        module: ModuleId,
        functions: &'a HashMap<DefinitionId, Signature>,
        classes: &'a ClassInfo<'a>,
        types: &'a Types,
        helpers: &'a Helpers,
        globals: &'a ModuleValues,
    ) -> Self {
        Self {
            frame: LexicalFrame::Named(ast::name::Name::new_static("<module>")),
            unhighlighted_call: None,
            wrapper_link: Default::default(),
            fixed_sites: None,
            placement_only: false,
            checked_call_contracts: false,
            settled_attributes: None,
            nonreturning_functions: &classes.nonreturning_functions,
            registered: None,
            bound_function_imports: false,
            checked_callback_effects: false,
            registered_local_start: None,
            frame_arguments: FrameArguments::Checked,
            unrepresented: Default::default(),
            namespace_calls: HashSet::new(),
            module_namespace: Default::default(),
            namespace: None,
            namespace_bindings: HashMap::new(),
            interactive_names: false,
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
            return_declaration: None,
            explicit_return: false,
            discover_document_return: None,
            comprehension_targets: HashSet::new(),
            comprehension_exports: None,
            narrowed: assignments::Narrowed::new(),
            regex_guards: Vec::new(),
            present_subscripts: Vec::new(),
            regex_locals: HashMap::new(),
            regex_fields: HashMap::new(),
            joined_lines: joined_lines::Plan::default(),
            consuming_lists: consuming_lists::Plan::default(),
            present_fields: Vec::new(),
            caught_exceptions: Vec::new(),
            returns: Type::None,
            inferred_containers: HashSet::new(),
            declared_slots: HashMap::new(),
            document_list_sites: HashMap::new(),
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
            finally_scope: FinallyScope::Outside,
            reachable: true,
            suite_imports: true,
            module_scope: false,
            class: None,
            class_receiver: None,
            class_lexical: HashSet::new(),
            super_method_owner: None,
            iterator_source: None,
            super_initializer: None,
            exception_formatter: None,
            initializing: false,
        }
    }

    fn parameter_shadows_binding(&self, name: &str, ty: Type, bound: bool) -> bool {
        self.mapping_class_import(name)
            || match self.graph.modules[self.module].bindings.get(name) {
                Some(Binding::Value(_) | Binding::ImportedValue(..) | Binding::TypeAlias(..)) => {
                    bound && !self.module_scope
                }
                Some(Binding::Definition(id)) => {
                    (bound || matches!(ty, Type::Callable(_))) && self.functions.contains_key(id)
                }
                _ => false,
            }
    }

    fn fixed_body_source(
        &mut self,
        definition: &ast::StmtFunctionDef,
        signature: &Signature,
        source_statements: usize,
        body: &[Statement],
    ) -> Option<FixedBodySource> {
        let sites = self.fixed_sites.take()?;
        let method = match signature.method {
            Some(_) => Some(registered::physical_method(
                signature,
                definition,
                self.classes,
            )?),
            None => None,
        };
        sites.finish(
            source_statements,
            body,
            std::mem::take(&mut self.names),
            &self.locals,
            method,
            self.frame_arguments,
        )
    }

    /// Bind the declared parameter slots in native ABI order for inference and final lowering.
    fn bind_function_signature(
        &mut self,
        definition: &ast::StmtFunctionDef,
        signature: &Signature,
        mut bound: impl FnMut(usize),
    ) -> Result<(), String> {
        self.returns = signature.returns;
        self.asynchronous = signature.asynchronous;
        self.method_context(signature.method);
        self.context_method = self.classes.contexts.contains(&signature.id);
        self.generator_item = self.classes.producer_items.get(&signature.id).copied();
        self.return_declaration = definition
            .returns
            .as_deref()
            .filter(|_| {
                self.generator_item.is_none()
                    && self.graph.modules[self.module].checks_source_contracts()
            })
            .map(|annotation| phase::TypeDeclaration {
                module: self.module,
                annotation: annotation.range(),
            });
        // The signature builder appends **kwargs after the ordinary parameters and *args.
        let parameters = closed_calls::parameters(&definition.parameters)
            .chain(definition.parameters.kwarg.as_deref());
        for (index, (parameter, layout)) in parameters.zip(&signature.params).enumerate() {
            // Static class operations still resolve through the specialized method body.
            // The ordinary bound adapter passes its owned class in the first ABI slot.
            let slot = if self.class_receiver.as_deref() == Some(parameter.name().as_str()) {
                self.hidden(layout.ty)
            } else {
                self.bind_parameter(parameter, layout.ty, signature.parameter_declaration(index))?
            };
            bound(slot);
        }
        Ok(())
    }

    fn bind_parameter(
        &mut self,
        parameter: &ast::Parameter,
        ty: Type,
        declaration: Option<TypeDeclaration>,
    ) -> Result<usize, String> {
        let slot = self.declare_binding(parameter.name().as_str(), ty, parameter, true)?;
        self.assigned.insert(slot);
        if let Some(declaration) = declaration {
            self.bind_declared_type(slot, declaration);
        }
        Ok(slot)
    }

    /// Reuses a slot only at its original type and prevents rebinding fixed call targets.
    fn declare(&mut self, name: &str, ty: Type, node: &impl Ranged) -> Result<usize, String> {
        self.declare_binding(name, ty, node, self.names.contains_key(name))
    }

    fn declare_binding(
        &mut self,
        name: &str,
        ty: Type,
        node: &impl Ranged,
        bound: bool,
    ) -> Result<usize, String> {
        if (name != "__doc__" && reserved(name))
            || (self.graph.modules[self.module].bindings.contains_key(name)
                && (self.module_scope
                    || !self.parameter_shadows_binding(name, ty, bound)
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
        if let Some(namespace) = self.namespace
            && self.comprehension_targets.contains(name)
        {
            self.namespace_bindings.insert(
                slot,
                registered::LocalBinding {
                    name: name.to_owned(),
                    namespace: Some(namespace),
                },
            );
        }
        Ok(slot)
    }

    fn hidden(&mut self, ty: Type) -> usize {
        let slot = self.locals.len();
        self.locals.push(ty);
        slot
    }

    /// Reads a hidden binding at its storage type, without source-level narrowing.
    fn local(&self, slot: usize) -> Expr {
        Expr::local(slot, self.locals[slot])
    }

    fn classified_error(&self, kind: Kind, node: &impl Ranged, message: &str) -> Failure {
        error(self.source, node, message).classify(kind)
    }

    fn expect(&self, expected: Type, actual: Type, node: &impl Ranged) -> Result<(), String> {
        if expected == actual || expected == Type::PythonObject {
            Ok(())
        } else if actual == Type::PythonObject {
            Err(self.classified_error(
                Kind::ScopedObject,
                node,
                "operand needs a runtime type check",
            ))
        } else {
            Err(self.classified_error(
                Kind::ExactType,
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
            if self.native_mock(class) && attribute.attr.as_str() == "return_value" {
                return Err(
                    self.object_runtime_export("unittest.mock return value assignment", attribute)
                );
            }
            let declaration = &self.classes.declarations.definitions[class];
            if self.classes.parsers.namespace(class) {
                return Err(error(
                    self.source,
                    attribute,
                    "argparse namespace fields are read-only",
                ));
            }
            if declaration.fields.iter().any(|field| {
                field.name == attribute.attr.as_str()
                    && field.module != self.module
                    && self.graph.modules[field.module].is_bundled("uuid")
            }) {
                return Err(error(self.source, attribute, "UUID fields are read-only"));
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
                    signature.method.unwrap().kind(self.classes.declarations)
                        == crate::classes::MethodKind::Property
                })
            {
                return Err(error(self.source, attribute, "properties are read-only"));
            }
        }
        self.field_value(attribute, receiver)
    }

    /// Obtain the exact integer payload for a checked integer method.
    fn int_method_receiver(&self, value: Expr, node: &impl Ranged) -> Result<Expr, String> {
        if self.types.is_bool_or_int(value.ty) {
            Ok(Expr {
                ty: Type::Int,
                kind: ExprKind::Builtin(Builtin::UnionProject, vec![value]),
            })
        } else {
            self.coerce(powers::integer(value), Type::Int, node)
        }
    }

    fn boxed_set_method(
        &self,
        attribute: &ast::ExprAttribute,
        receiver: Option<Type>,
    ) -> Option<&'static str> {
        let operation = matches!(
            attribute.attr.as_str(),
            "__and__"
                | "__rand__"
                | "__or__"
                | "__ror__"
                | "__sub__"
                | "__rsub__"
                | "__xor__"
                | "__rxor__"
        );
        let name = match attribute.attr.as_str() {
            _ if operation => "set binary operator",
            "__iand__" | "__ior__" | "__isub__" | "__ixor__" => "set in-place operator",
            "union" => "set.union",
            "symmetric_difference" => "set.symmetric_difference",
            "difference_update" => "set.difference_update",
            "intersection_update" => "set.intersection_update",
            "symmetric_difference_update" => "set.symmetric_difference_update",
            "add" if self.unshadowed_builtin(&attribute.value, "set") => "set.add",
            "copy" => "set.copy",
            "issuperset" => "set.issuperset",
            "__iter__" => "set.__iter__",
            "__len__" => "set.__len__",
            "__class_getitem__" => "set.__class_getitem__",
            "clear" => "set.clear",
            "pop" => "set.pop",
            "remove" => "set.remove",
            "update" => "set.update",
            "isdisjoint" => "set.isdisjoint",
            "issubset" => "set.issubset",
            _ => return None,
        };
        if matches!(receiver, Some(Type::Set(_)))
            || self.unshadowed_builtin(&attribute.value, "set")
        {
            return Some(name);
        }
        if receiver == Some(Type::FrozenStrings)
            || self.unshadowed_builtin(&attribute.value, "frozenset")
        {
            return Some(match attribute.attr.as_str() {
                _ if operation => name,
                "__class_getitem__" => "frozenset.__class_getitem__",
                "__iter__" => "frozenset.__iter__",
                "__len__" => "frozenset.__len__",
                "copy" => "frozenset.copy",
                "isdisjoint" => "frozenset.isdisjoint",
                "issubset" => "frozenset.issubset",
                "issuperset" => "frozenset.issuperset",
                "symmetric_difference" => "frozenset.symmetric_difference",
                "union" => "frozenset.union",
                _ => return None,
            });
        }
        None
    }

    fn boxed_float_attribute(
        &self,
        attribute: &ast::ExprAttribute,
        receiver: Type,
    ) -> Option<&'static str> {
        if receiver != Type::Float && !self.unshadowed_builtin(&attribute.value, "float") {
            return None;
        }
        match attribute.attr.as_str() {
            "real" | "imag" => Some("float components"),
            "fromhex" => Some("float.fromhex"),
            "hex" => Some("float.hex"),
            _ => None,
        }
    }

    fn field_value(
        &self,
        attribute: &ast::ExprAttribute,
        receiver: Expr,
    ) -> Result<(Expr, usize, Type), String> {
        if let Some(method) = self.boxed_set_method(attribute, Some(receiver.ty)) {
            return Err(self.object_builtin_error(method, attribute));
        }
        if receiver.ty == Type::CodecInfo
            && matches!(
                attribute.attr.as_str(),
                "_is_text_encoding" | "incrementaldecoder" | "incrementalencoder"
            )
        {
            return Err(self.object_builtin_error(attribute.attr.as_str(), attribute));
        }
        if matches!(receiver.ty, Type::Coroutine(_)) && attribute.attr.as_str() == "cr_origin" {
            return Err(self.object_builtin_error("coroutine.cr_origin", attribute));
        }
        if let Some(name) = self.boxed_float_attribute(attribute, receiver.ty) {
            return Err(self.scoped_builtin_error(name, attribute));
        }
        if receiver.ty == Type::BinaryIO && matches!(attribute.attr.as_str(), "name" | "mode") {
            return Err(self.object_builtin_error("binary file metadata", attribute));
        }
        if attribute.attr.as_str() == "readinto"
            && matches!(receiver.ty, Type::BinaryIO | Type::BytesIO)
        {
            return Err(self.object_builtin_error("binary readinto", attribute));
        }
        if receiver.ty == Type::BytesIO && attribute.attr.as_str() == "getbuffer" {
            return Err(self.object_builtin_error("BytesIO.getbuffer", attribute));
        }
        if receiver.ty == Type::BinaryIO && attribute.attr.as_str() == "writelines" {
            return Err(self.object_builtin_error("binary writelines", attribute));
        }
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
                self.missing_class_member(
                    class,
                    attribute.attr.as_str(),
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
        self.keep_module_list_layout(&value);
        if matches!(&value.kind, ExprKind::Builtin(Builtin::Unrepresented, _)) {
            return Ok(Expr {
                ty: expected,
                ..value
            });
        }
        // PythonObject describes the selected binder's storage, never an annotation.
        // The operand retains its checked type while the registered call supplies a handle.
        if expected == Type::PythonObject || value.ty == Type::PythonObject {
            return match self.expect(expected, value.ty, node) {
                Ok(()) => Ok(value),
                Err(failure) if self.missing_operation(&failure) => Ok(Expr {
                    ty: expected,
                    ..Expr::unrepresented()
                }),
                Err(failure) => Err(failure),
            };
        }
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
            && (target == value.ty
                || matches!(
                    target,
                    Type::JsonObject | Type::Int | Type::Str | Type::Bool | Type::Float
                )
                || self.types.element(target) == Some(Type::JsonObject))
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
        let exception_upcast = match (value.ty, expected) {
            (Type::NamedException(child), Type::NamedException(parent)) => self
                .classes
                .declarations
                .subclass(self.graph, child, parent),
            (
                Type::ImportException | Type::TimeoutExpired | Type::NamedException(_),
                Type::Exception,
            ) => true,
            _ => false,
        };
        if exception_upcast {
            return Ok(Expr {
                ty: expected,
                kind: ExprKind::Builtin(
                    Builtin::process(ir::ProcessOperation::ExceptionView),
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
        // Tagged spelling unions share the same representation for each member.
        if let (Some(actual), Some(target)) =
            (self.types.members(value.ty), self.types.members(expected))
            && actual
                .iter()
                .all(|ty| matches!(ty, Type::Str | Type::Bytes | Type::Path) && target.contains(ty))
        {
            return Ok(Expr {
                ty: expected,
                kind: ExprKind::Builtin(Builtin::RecordView, vec![value]),
            });
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

    /// A terminating call supplies no value to the destination that follows it.
    fn coerce_supplied_result(
        &self,
        value: Expr,
        expected: Type,
        node: &impl Ranged,
    ) -> Result<Expr, String> {
        if self.terminating_call(&value) {
            Ok(value)
        } else {
            self.coerce(value, expected, node)
        }
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
        self.expr_expected_from(expression, expected, None)
    }

    fn expr_expected_from(
        &mut self,
        expression: &ast::Expr,
        expected: Type,
        declaration: Option<TypeDeclaration>,
    ) -> Result<Expr, String> {
        self.expr_in_context(
            expression,
            expected,
            |_, _| false,
            Self::coerce,
            declaration,
        )
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
        self.expr_in_context(expression, expected, alternative, Self::coerce, None)
    }

    /// Lower operands normally and let the destination check the resulting value.
    fn expr_in_context(
        &mut self,
        expression: &ast::Expr,
        expected: Type,
        alternative: impl Fn(&Self, Type) -> bool,
        coerce: impl Fn(&Self, Expr, Type, &ast::Expr) -> Result<Expr, String>,
        declaration: Option<TypeDeclaration>,
    ) -> Result<Expr, String> {
        let coerce = |checker: &Self, value, ty, site: &ast::Expr| {
            coerce(checker, value, ty, site).map_err(|e| e.expected_from(declaration, site.range()))
        };
        let mut concrete = self.types.payload(expected).unwrap_or(expected);
        // An empty list gets its element type from its destination in either frame.
        if self.registered.is_some()
            && !(matches!(expression, ast::Expr::List(list) if list.elts.is_empty() || self.checked_call_contracts)
                && self.types.element(concrete).is_some())
        {
            let concrete_display = match expression {
                ast::Expr::List(_) => self.types.element(concrete).is_some(),
                ast::Expr::Tuple(_) => self.types.fields(concrete).is_some(),
                ast::Expr::Set(_) => self.types.set_element(concrete).is_some(),
                _ => false,
            };
            let value = if concrete_display {
                self.fixed_expr(expression)?
            } else {
                self.expr(expression)?
            };
            if value.ty == Type::PythonObject
                && self.types.set_element(concrete).is_some()
                && let ast::Expr::Call(call) = expression
                && call.arguments.is_empty()
                && self.graph.session.is_known_class(
                    self.module,
                    &call.func,
                    ty_python_semantic::types::KnownClass::Set,
                )
            {
                return Ok(Expr {
                    ty: concrete,
                    ..value
                });
            }
            return if alternative(self, value.ty) {
                Ok(value)
            } else {
                coerce(self, value, expected, expression)
            };
        }
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
            return coerce(self, value, expected, expression);
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
        if expected == Type::BuiltinClass
            && let Some(class) = self.class_reference(expression)
        {
            return Ok(self.record_class_object(class));
        }
        if self.types.constructor_base(concrete).is_none() {
            self.reject_class_receiver_value(expression)?;
        }
        if self.registered.is_none()
            && self.types.element(expected) == Some(Type::Str)
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
        if let ast::Expr::Call(call) = expression
            && self.is_defaultdict_constructor(&call.func)
            && let Some(raised) = self.defaultdict_factory_error(call, expected)?
        {
            return Ok(raised);
        }
        if let ast::Expr::Call(call) = expression
            && let Some(shape) = self.types.mapping(concrete)
            && concrete == self.types.dict(shape.0, shape.1)
            && let Some(result) = self.copied_factory_mapping(call, shape)?
        {
            return coerce(self, result, expected, expression);
        }
        if let ast::Expr::Call(call) = expression
            && let Some(shape) = self.types.mapping(concrete)
            && let Some(result) = self.fromkeys_call(call, Some(shape))?
        {
            return coerce(self, result, expected, expression);
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
                return coerce(self, value, expected, expression);
            }
            let binding = self.resolve(expression);
            if binding == Some(Binding::Intrinsic(Intrinsic::JsonLoads)) {
                let value = self.json_load_callable(None, Some(concrete), expression)?;
                return coerce(self, value, expected, expression);
            }
            if let Some(Binding::Intrinsic(
                op @ (Intrinsic::Copy(_)
                | Intrinsic::RmTree
                | Intrinsic::Move
                | Intrinsic::DestInSrc),
            )) = binding
            {
                let value = self.shutil_function_value(op, Some(concrete), expression)?;
                return coerce(self, value, expected, expression);
            }
            if let Some(Binding::Definition(definition)) = binding
                && let Some(signature) = self.functions.get(&definition).cloned()
            {
                let value = self.callable_value(&signature, None, Some(concrete), expression)?;
                let value = coerce(self, value, expected, expression)?;
                if let Some(sites) = self
                    .fixed_sites
                    .as_mut()
                    .map(|sites| &mut sites.attributes)
                    .or(self.settled_attributes.as_deref_mut())
                    && let Some(name) = expression.as_name_expr()
                    && self.namespace.is_none()
                    && !self.names.contains_key(name.id.as_str())
                {
                    // The native callable carries a definition ID. The Python frame
                    // loads the written name after evaluating the preceding arguments.
                    sites.insert(name.range(), FixedAttribute::ModuleName(name.id.clone()));
                }
                return Ok(value);
            }
            if let ast::Expr::Attribute(attribute) = expression
                && self.resolve(expression).is_none()
            {
                if let Some(class) = self.class_reference(&attribute.value) {
                    let value = self.class_attribute(attribute, class, None, Some(concrete))?;
                    return coerce(self, value, expected, expression);
                }
                let receiver = Self::weak_target(self.expr(&attribute.value)?);
                if let Some(wrapped) = self.callable_attribute(&receiver, attribute)? {
                    return self.coerce(wrapped, expected, expression);
                }
                if receiver.ty == Type::JsonDecoder && attribute.attr.as_str() == "decode" {
                    let value =
                        self.json_load_callable(Some(receiver), Some(concrete), expression)?;
                    return coerce(self, value, expected, expression);
                }
                if let Some(element) = self.types.deque_element(receiver.ty) {
                    let value = self.deque_attribute(attribute, receiver, element)?;
                    return coerce(self, value, expected, expression);
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
                    return coerce(self, value, expected, expression);
                }
                return Err(error(
                    self.source,
                    expression,
                    "expected a fixed bound method",
                ));
            }
        }
        let value = match expression {
            ast::Expr::Dict(dict) if concrete == Type::PythonObject && dict.items.is_empty() => {
                self.require_registered_operation(expression)?;
                Expr::unrepresented()
            }
            ast::Expr::Dict(dict) if concrete == Type::ReadOnlyObjectMapping => {
                let value = self.dictionary(dict, Some((Type::Str, Type::ObjectValue)), None)?;
                self.coerce(value, concrete, expression)?
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
            ast::Expr::Named(named) => self.with_expression_site(expression, |checker| {
                checker.named_expression(named, Some(expected))
            })?,
            ast::Expr::If(value) => self.conditional(value, Some(expected))?,
            ast::Expr::BoolOp(value) => self.boolean_value(value, Some(expected))?,
            ast::Expr::Subscript(subscript)
                if matches!(&*subscript.slice, ast::Expr::Slice(_))
                    && (matches!(concrete, Type::Str | Type::Bytes)
                        || self.types.element(concrete).is_some()) =>
            {
                self.with_expression_site(expression, |checker| {
                    let receiver = checker.expr_expected(&subscript.value, concrete)?;
                    let slice = subscript.slice.as_slice_expr().expect("checked slice");
                    checker.slice(receiver, slice, subscript.range())
                })?
            }
            ast::Expr::ListComp(_) | ast::Expr::DictComp(_) | ast::Expr::SetComp(_) => {
                self.comprehension(expression, Some(concrete))?
            }
            ast::Expr::Dict(dict) if self.types.mapping(concrete).is_some() => {
                self.dictionary(dict, self.types.mapping(concrete), declaration)?
            }
            ast::Expr::Set(set) if self.types.set_element(concrete).is_some() => {
                self.set_literal(set, self.types.set_element(concrete), declaration)?
            }
            ast::Expr::Call(call)
                if self.types.element(concrete).is_some()
                    && (self.unshadowed_builtin(&call.func, "list")
                        || self.unshadowed_builtin(&call.func, "sorted")) =>
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
                if let Some(value) = self.contextual_collection(call, concrete)? =>
            {
                value
            }
            ast::Expr::Call(call)
                if let Some((key, value)) = self.types.mapping(concrete)
                    && self.is_defaultdict_constructor(&call.func) =>
            {
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
                if let Some(element) = self.types.set_element(concrete)
                    && matches!(&*call.func, ast::Expr::Name(name) if name.id == "set")
                    && call.arguments.args.is_empty()
                    && call.arguments.keywords.is_empty() =>
            {
                Expr {
                    ty: concrete,
                    kind: ExprKind::Set {
                        element,
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
                        .and_then(|value| coerce(self, value, expected, expression));
                }
                self.contextual_display(
                    expression,
                    concrete,
                    &list.elts,
                    std::iter::repeat(element),
                    declaration,
                    ExprKind::List,
                )?
            }
            ast::Expr::Tuple(tuple) if self.types.fields(concrete).is_some() => {
                let fields = self.types.fields(concrete).unwrap();
                if tuple
                    .elts
                    .iter()
                    .any(|v| matches!(v, ast::Expr::Starred(_)))
                {
                    let value = self.expanded_tuple(tuple, Some(&fields))?;
                    return coerce(self, value, expected, expression);
                }
                if tuple.elts.len() != fields.len() {
                    return Err(error(
                        self.source,
                        expression,
                        "tuple literal has the wrong number of fields",
                    ));
                }
                self.contextual_display(
                    expression,
                    concrete,
                    &tuple.elts,
                    fields.into_iter(),
                    declaration,
                    ExprKind::Tuple,
                )?
            }
            ast::Expr::BinOp(binary)
                if binary.op == ast::Operator::Add && self.types.element(concrete).is_some() =>
            {
                self.with_expression_site(expression, |checker| {
                    let left = checker.expr_expected(&binary.left, concrete)?;
                    let right = checker.expr_expected(&binary.right, concrete)?;
                    checker.binary_expr(binary.op, left, right, binary)
                })?
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
        coerce(self, value, expected, expression)
    }

    fn contextual_collection(
        &mut self,
        call: &ast::ExprCall,
        expected: Type,
    ) -> Result<Option<Expr>, String> {
        let Some(Binding::Intrinsic(constructor)) = self.resolve(&call.func) else {
            return Ok(None);
        };
        let value = match constructor {
            Intrinsic::ChainMap if let Type::ChainMap(id) = expected => {
                self.chainmap(call, self.types.mapping(Type::Dict(id)))?
            }
            Intrinsic::Deque if let Some(element) = self.types.deque_element(expected) => {
                self.deque_new(call, Some(element))?
            }
            Intrinsic::Counter if self.types.is_counter(expected) && call.arguments.is_empty() => {
                Expr {
                    ty: expected,
                    kind: ExprKind::Builtin(Builtin::CounterNew, Vec::new()),
                }
            }
            Intrinsic::Counter if let Some((key, _)) = self.types.mapping(expected) => {
                self.counter(call, None, Some(key))?
            }
            Intrinsic::OrderedDict => {
                self.ordered_dict_constructor(call, self.types.mapping(expected))?
            }
            _ => return Ok(None),
        };
        Ok(Some(value))
    }

    /// The display and its elements use the same source positions in either frame.
    fn contextual_display(
        &mut self,
        expression: &ast::Expr,
        ty: Type,
        items: &[ast::Expr],
        fields: impl Iterator<Item = Type>,
        declaration: Option<TypeDeclaration>,
        build: fn(Vec<Expr>) -> ExprKind,
    ) -> Result<Expr, String> {
        self.with_expression_site(expression, |checker| {
            let values = items
                .iter()
                .zip(fields)
                .map(|(value, ty)| checker.expr_expected_from(value, ty, declaration))
                .collect::<StdResult<Vec<_>, _>>()?;
            Ok(Expr {
                ty,
                kind: build(values),
            })
        })
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
            let guards = checker.present_fields.len();
            if narrowing::expression_assignments(expression).is_empty() {
                checker.enter_namespace_guards(&value.test, truth, &[]);
            }
            checker.guard_field_read(expression, records::present_field(&test, truth));
            let result = check(checker, expression);
            checker.present_fields.truncate(guards);
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
        let ty = self.types.join_results(yes.ty, no.ty).ok_or_else(|| {
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
                    .is_some_and(|payload| self.known_truth(payload) == Some(true))
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
    fn slice(
        &mut self,
        receiver: Expr,
        slice: &ast::ExprSlice,
        range: TextRange,
    ) -> Result<Expr, String> {
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
        // A full slice of a general list has only one source operand. The fixed
        // ABI supplies its omitted bounds when the receiver is emitted.
        let bounds = registered::values::slice_bounds(slice);
        if matches!(ty, Type::List(_)) && bounds.iter().all(Option::is_none) {
            let value = Expr {
                ty,
                kind: ExprKind::Builtin(Builtin::Slice, args),
            };
            return Ok(self.expression_at(range, value));
        }
        let index_type = self.types.optional(Type::Int);
        for bound in bounds {
            args.push(if let Some(bound) = bound {
                self.expr_expected(bound, index_type)?
            } else {
                Expr::absent(index_type)
            });
        }
        Ok(Expr {
            ty,
            kind: ExprKind::Builtin(Builtin::Slice, args),
        })
    }

    fn registered_in_place_local(&self, name: &str) -> Option<usize> {
        let slot = *self.names.get(name)?;
        (slot >= self.registered_local_start? && !self.declared_slots.contains_key(&slot))
            .then_some(slot)
    }

    /// Loop backedges can observe stores from a previous iteration.
    fn invalidate_loop_facts(
        &mut self,
        body: &[ast::Stmt],
        source: Option<(&pipelines::Prepared, &ast::Expr)>,
    ) -> Result<(), String> {
        let names = narrowing::assigned_names_with_updates(body, &mut |name| {
            // The next iteration can receive the replacement returned by __iadd__ and its peers.
            if let Some(slot) = self.registered_in_place_local(name) {
                self.locals[slot] = Type::PythonObject;
            }
        });
        for name in names {
            if let Some(slot) = self.names.get(&name) {
                self.narrowed.remove_mut(slot);
                self.assigned
                    .set_type_source(*slot, self.declared_slots.get(slot).copied());
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

    fn bind_declared_type(&mut self, slot: usize, declaration: TypeDeclaration) {
        self.declared_slots.insert(slot, declaration);
        self.assigned.set_type_source(slot, Some(declaration));
    }

    fn value_source(&self, value: &Expr) -> Option<ValueSource> {
        match &value.kind {
            ExprKind::Local(slot) => self.assigned.value_source(*slot),
            ExprKind::Builtin(Builtin::StaticFunction(id), _) => Some(ValueSource::Function(*id)),
            ExprKind::Bind(_, inner) | ExprKind::Present(inner) | ExprKind::Unwrap(inner) => {
                self.value_source(inner)
            }
            ExprKind::Conditional(_, yes, no) => {
                let source = self.value_source(yes)?;
                (self.value_source(no) == Some(source)).then_some(source)
            }
            _ => self
                .globals
                .list_origin(value)
                .map(ValueSource::InferredList),
        }
    }

    /// A checked mutator or an escaping alias can write elements outside
    /// the observed appends. Keep the original layout for that allocation.
    fn keep_module_list_layout(&self, value: &Expr) {
        if let Some(inference) = self.inference
            && matches!(value.ty, Type::List(_))
            && let Some(ValueSource::InferredList(owner)) = self.value_source(value)
            && self.types.observe_module_list_use(owner, false)
        {
            inference.layout_changed();
        }
    }

    fn value_declaration(&self, value: &Expr) -> Option<TypeDeclaration> {
        match self.value_source(value)? {
            ValueSource::Declaration(declaration) => Some(declaration),
            ValueSource::InferredList(_)
            | ValueSource::Function(_)
            | ValueSource::ReboundMethod => None,
        }
    }

    fn assigned_value(&mut self, slot: usize, value: &Expr) {
        let source = self
            .declared_slots
            .get(&slot)
            .copied()
            .map(ValueSource::Declaration)
            .or_else(|| self.value_source(value));
        self.assigned.set_value_source(slot, source);
        if self.module_scope {
            self.namespace_assignment(slot, value);
        }
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
                || self.types.method_signature(args[0].ty) == Some(self.locals[slot]))
        {
            self.narrowed.insert_mut(slot, args[0].ty);
        }
        if let ExprKind::Builtin(Builtin::UnionInject, args) = &value.kind
            && (self.types.element(args[0].ty) == Some(Type::Str)
                || matches!(args[0].ty, Type::Str | Type::Bytes | Type::Float)
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
        declaration: Option<TypeDeclaration>,
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
                    self.expr_expected_from(key, key_type, declaration)?,
                    self.expr_expected_from(&item.value, value_type, declaration)?,
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
            entries.push(ir::DictEntry {
                key: Some(key),
                value,
            });
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
            kind: ExprKind::Dict(entries),
        })
    }

    /// Exact scalar elements keep hashing and equality free of user callbacks.
    fn set_literal(
        &mut self,
        set: &ast::ExprSet,
        expected: Option<Type>,
        declaration: Option<TypeDeclaration>,
    ) -> Result<Expr, String> {
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
                self.expr_expected_from(value, element, declaration)?
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

    /// An object can override the view method and return any iterable.
    fn object_method_iteration(
        &mut self,
        call: &ast::ExprCall,
        method: &ast::ExprAttribute,
        receiver: Expr,
    ) -> Result<Iteration, String> {
        let value = self.call_object_method(call, method, receiver)?;
        self.iteration_value(value, false, call)
    }

    /// Recognizes items views only at a loop boundary; no view value escapes.
    fn iteration_source(&mut self, expression: &ast::Expr) -> Result<Iteration, String> {
        if let ast::Expr::Call(call) = expression
            && let ast::Expr::Attribute(attribute) = &*call.func
            && attribute.attr.as_str() == "items"
            && self.class_reference(&attribute.value).is_none()
        {
            let receiver = self.expr(&attribute.value)?;
            if receiver.ty == Type::PythonObject
                || (receiver.ty == Type::JsonObject && call.arguments.is_empty())
            {
                return self.object_method_iteration(call, attribute, receiver);
            }
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

    /// Raise a statically known range binding error after evaluating its operands.
    fn range_argument_error(
        &mut self,
        call: &ast::ExprCall,
        returns: Type,
    ) -> Result<Option<Expr>, String> {
        if calls::has_unpacking(call) {
            return Ok(None);
        }
        let Some(message) = imre_ast::range_constructor_error(
            call.arguments.args.len(),
            !call.arguments.keywords.is_empty(),
        ) else {
            return Ok(None);
        };
        self.graph.checked_overload(self.module, call.range());
        self.positional_argument_error(
            call,
            &["start", "stop", "step"],
            returns,
            unpacking::string(message),
        )
        .map(Some)
    }

    /// Checks an iterable in its current lexical scope, before target binding.
    fn iteration_input(
        &mut self,
        expression: &ast::Expr,
        split_needle: Option<&str>,
        pull: Option<ir::AsyncPull>,
    ) -> Result<Iteration, String> {
        if pull.is_some() {
            if !self.asynchronous {
                return Err(error(
                    self.source,
                    expression,
                    "asynchronous iteration outside an async function",
                ));
            }
            if self.registered.is_none() {
                return Err(self.classified_error(
                    Kind::FrameSuspension,
                    expression,
                    "asynchronous iteration requires a suspended frame",
                ));
            }
        }
        if self.registered.is_some() {
            // The Python frame supplies the actual object, including subclass overrides.
            // Prepare the same pull/bind/cleanup IR used by the fixed-layout iterators.
            let value = self.expr(expression)?;
            // A Python handle still carries any established element contract. Only an
            // unknown object defers the validity of iteration to its live protocol.
            let element = if pull.is_some() || value.ty == Type::PythonObject {
                Type::PythonObject
            } else {
                self.types
                    .iteration_element(value.ty, false)
                    .or_else(|| self.types.cursor_element(value.ty))
                    .or_else(|| {
                        self.types.fields(value.ty).map(|fields| {
                            self.types
                                .tuple_iteration_element(&fields)
                                .unwrap_or(Type::PythonObject)
                        })
                    })
                    .ok_or_else(|| {
                        error(
                            self.source,
                            expression,
                            "iteration requires an iterable value",
                        )
                    })?
            };
            let input = registered::cursor_input(value, Some(expression.range()), pull, |ty, _| {
                self.hidden(ty)
            });
            // The private item owns the Python handle returned by next(). Its
            // checked element type belongs to the subsequent target binding.
            if element != Type::PythonObject {
                self.narrowed.insert_mut(input.value, element);
            }
            return Ok(Iteration::Prepared(input));
        }
        if let Some(input) = self.pipeline_input(expression, None, split_needle)? {
            return Ok(input);
        }
        if let ast::Expr::Call(call) = expression
            && self.unshadowed_builtin(&call.func, "range")
        {
            if let Some(failure) = self.range_argument_error(call, Type::Int)? {
                return Ok(Iteration::Range(vec![failure]));
            }
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
                .collect::<StdResult<Vec<_>, _>>()?;
            return Ok(Iteration::Range(args));
        }
        self.iteration_source(expression)
    }

    /// Acquire the live Python iterator and own each item until the next pull.
    fn python_cursor(&mut self, value: Expr, range: TextRange) -> pipelines::Prepared {
        registered::cursor_input(value, Some(range), None, |ty, _| self.hidden(ty))
    }

    /// Prepare cursors and generators for pulling while preserving concrete container iteration.
    fn iteration_value(
        &mut self,
        iterable: Expr,
        items: bool,
        node: &impl Ranged,
    ) -> Result<Iteration, String> {
        if iterable.ty == Type::PythonObject && self.graph.modules[self.module].is_program_source()
        {
            self.require_registered_operation(node)?;
            return Ok(Iteration::Prepared(
                self.python_cursor(iterable, node.range()),
            ));
        }
        if let Some(element) = self.types.cursor_element(iterable.ty) {
            return Ok(Iteration::Prepared(self.prepare_cursor(iterable, element)));
        }
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
        let mut input = if loop_.is_async || self.registered.is_some() {
            self.iteration_input(&loop_.iter, None, registered::for_pull(loop_))?
        } else if let ast::Expr::Call(call) = loop_.iter.as_ref()
            && self.resolve(&call.func) == Some(Binding::Intrinsic(Intrinsic::GroupBy))
        {
            return self.groupby_iteration(loop_, call);
        } else if let Some(fields) = self.prepared_split_iteration(&loop_.iter)? {
            fields
        } else if let Some(items) = self.environ_items_loop(&loop_.iter)? {
            items
        } else if let Some(input) = self.unpacked_list_enumeration(loop_)? {
            input
        } else {
            let input = self.iteration_input(&loop_.iter, self.split_guard(loop_), None)?;
            self.fuse_split(input)
        };
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
        let is_range = matches!(&input, Iteration::Range(_));
        self.lower_iteration_after(
            &loop_.target,
            input,
            &loop_.orelse,
            Vec::new(),
            |checker, body| {
                let discard_range_body = is_range
                    && checker.inference.is_some()
                    && checker.discarded_function_body()
                    && checker.registered.is_none()
                    && !checker.fault_enabled();
                checker.lower_block(&loop_.body, (!discard_range_body).then_some(body))?;
                // The loop body can select a Python frame while its source contracts are checked.
                Ok(!checker.discarded_function_body())
            },
        )
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
            start: Some(Expr::integer(0)),
        }))
    }

    /// A consumer can evaluate its remaining arguments after acquiring an
    /// iterator, preserving `sum(map(int, values), start())` mutation checks.
    /// Supplying extra statements requires already validated range arguments.
    /// The callback returns whether its checked body needs an executable loop.
    fn lower_iteration_after(
        &mut self,
        target: &ast::Expr,
        input: Iteration,
        orelse: &[ast::Stmt],
        before_iteration: Vec<Statement>,
        lower_body: impl FnOnce(&mut Self, &mut Vec<Statement>) -> Result<bool, String>,
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
            let mut body = Vec::new();
            let emit = lower_body(self, &mut body)?;
            let breaks = self.loop_breaks.pop().unwrap();
            self.assigned.after_iteration(before);
            self.narrowed = facts;
            let (orelse, falls) = self.loop_else(orelse, breaks, true)?;
            if !emit {
                return Ok((Vec::new(), falls));
            }
            let cursor = self.hidden(Type::Int);
            let stop = self.hidden(Type::Int);
            let step = self.hidden(Type::Int);
            let clear = vec![cursor, stop, step];
            let output =
                pipelines::scoped_loop(before_iteration, clear, orelse, |orelse| Statement::For {
                    target,
                    cursor,
                    stop,
                    step,
                    args,
                    body,
                    orelse,
                });
            return Ok((output, falls));
        }
        let Iteration::Container {
            iterable,
            items,
            start,
        } = input
        else {
            unreachable!("range iteration returned above")
        };
        if !items && start.is_none() && matches!(iterable.ty, Type::Tuple(_)) {
            let input = self.prepare_container(iterable, false, target, false)?;
            return self.lower_prepared_iteration(
                target,
                input,
                orelse,
                before_iteration,
                lower_body,
            );
        }
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
            .iteration_element(iterable.ty, items)
            .ok_or_else(|| {
                let reason = error(
                    self.source,
                    target,
                    "iteration requires a concrete list, dictionary, set, string, bytes, or range",
                );
                if iterable.ty == Type::PythonObject {
                    reason.classify(Kind::ScopedObject)
                } else {
                    reason
                }
            })?;
        // A private forward cursor can borrow the UTF-8 bytes of an exact string.
        let (iterable, encoded_bytes) = match iterable {
            Expr {
                kind: ExprKind::Builtin(Builtin::Encode, mut args),
                ..
            } => (args.pop().expect("encode receiver"), true),
            iterable => (iterable, false),
        };
        let captured = self.hidden(iterable.ty);
        let cursor = if mapping.is_none()
            && set_element.is_none()
            && self.types.deque_element(iterable.ty).is_none()
            && !string
            && !encoded_bytes
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
        let mut cleanup = vec![captured];
        let (item, pull) = if let Some(cursor) = cursor {
            output.push(Statement::Assign(cursor, Expr::integer(0)));
            cleanup.push(cursor);
            (
                Expr {
                    ty: element,
                    kind: ExprKind::Builtin(
                        if bytes {
                            Builtin::BytesGet
                        } else if matches!(self.locals[captured], Type::VarTuple(_)) {
                            Builtin::TupleIndex
                        } else if matches!(self.locals[captured], Type::List(_)) {
                            Builtin::SequenceGetInBounds
                        } else {
                            Builtin::ListGet(None)
                        },
                        vec![self.local(captured), self.local(cursor)],
                    ),
                },
                None,
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
                        if string || encoded_bytes {
                            Builtin::StringIter
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
                kind: if encoded_bytes {
                    ir::IterNextKind::Utf8Byte
                } else if string {
                    ir::IterNextKind::String
                } else {
                    ir::IterNextKind::Generic
                },
                iterator,
                key,
                value,
                ready,
            });
            let item = if let Some(value) = value {
                Expr {
                    ty: element,
                    kind: ExprKind::Tuple(vec![self.local(key), self.local(value)]),
                }
            } else {
                self.local(key)
            };
            (item, Some((ready, key, value)))
        };
        let mut facts = self.narrowed.clone();
        let before = self.assigned.clone();
        if let Some((ready, _, _)) = pull {
            body.push(pipelines::Prepared::exhaust_if_empty(self.local(ready)));
        }
        let start = body.len();
        if zero_based {
            let [index, target] = pattern_elements(target).unwrap() else {
                unreachable!()
            };
            self.store_value(index, self.local(cursor.unwrap()), None, Some(&mut body))?;
            self.store_value(target, item, None, Some(&mut body))?;
        } else {
            let item = if let Some(counter) = counter {
                Expr {
                    ty: self.types.tuple(vec![Type::Int, element]),
                    kind: ExprKind::Tuple(vec![self.local(counter), item]),
                }
            } else {
                item
            };
            unpacking::bind_input(self, target, item, None, &mut body)?;
        }
        assignments::forget_binding_writes(&mut facts, &body[start..]);
        if let Some((_, key, value)) = pull {
            let slots = std::iter::once(key).chain(value);
            cleanup.extend(slots.clone());
            body.extend(slots.map(Statement::Clear));
        }
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
                        Box::new(Expr::integer(1)),
                    ),
                },
            ));
        }
        self.loop_breaks.push(None);
        let emit = lower_body(self, &mut body)?;
        let breaks = self.loop_breaks.pop().unwrap();
        self.assigned.after_iteration(before);
        self.narrowed = facts;
        let (orelse, falls) = self.loop_else(orelse, breaks, true)?;
        if !emit {
            return Ok((Vec::new(), falls));
        }
        cleanup.extend(counter);
        output.extend(before_iteration);
        let test = match cursor {
            Some(cursor) => pipelines::compare(
                Compare::Less,
                self.local(cursor),
                Expr {
                    ty: Type::Int,
                    kind: ExprKind::Builtin(Builtin::Len, vec![self.local(captured)]),
                },
            ),
            None => Expr::boolean(true),
        };
        let output = pipelines::scoped_loop(output, cleanup, orelse, |orelse| {
            Statement::While(test, body, orelse)
        });
        Ok((output, falls))
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
                previous.retain_common(current);
            }
            *current = Some(previous);
        }
    }

    fn discarded_function_body(&self) -> bool {
        self.placement_only && self.block_depth == 1 && self.fixed_sites.is_none()
    }

    /// A discarded outer loop still needs each suite's binding and exit facts.
    /// Nested handlers inspect their protected operations.
    fn loop_suite(&mut self, statements: &[ast::Stmt]) -> Result<(Vec<Statement>, bool), String> {
        if self.discarded_function_body() {
            let falls = self.lower_block(statements, None)?;
            Ok((Vec::new(), falls))
        } else {
            self.block(statements)
        }
    }

    /// The else suite is outside this loop, so its transfers target an outer loop.
    /// Only normal completion enters else; break states join after it.
    fn loop_else(
        &mut self,
        orelse: &[ast::Stmt],
        mut breaks: Option<LocalState>,
        can_exhaust: bool,
    ) -> Result<(Vec<Statement>, bool), String> {
        let reachable = self.reachable;
        self.reachable &= can_exhaust;
        let (orelse, falls) = self.loop_suite(orelse)?;
        self.reachable = reachable;
        if can_exhaust && falls {
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
        let mut body = Vec::new();
        let falls_through = self.lower_block(statements, Some(&mut body))?;
        Ok((body, falls_through))
    }

    /// Keep binding, flow and diagnostic evidence for every statement. Only the
    /// owner of an executable body collects instructions and frame checkpoints.
    fn lower_block(
        &mut self,
        statements: &[ast::Stmt],
        output: Option<&mut Vec<Statement>>,
    ) -> Result<bool, String> {
        self.lower_block_with_placement(statements, output, None)
    }

    /// Let a complete Python body finish a function whose fixed positions no longer match.
    fn lower_block_with_placement(
        &mut self,
        statements: &[ast::Stmt],
        mut output: Option<&mut Vec<Statement>>,
        mut place: Option<&mut dyn FnMut(&Self) -> bool>,
    ) -> Result<bool, String> {
        if let Some(reason) = statements.first().and_then(|first| {
            self.graph
                .entry_profile
                .excluded(self.module, first.start())
        }) {
            if let Some(body) = output {
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
                body.push(Statement::raise(exception));
            }
            return Ok(false);
        }

        if self.block_depth == 0 && !self.module_scope {
            if self.graph.python_optimization() > 0 && !self.interactive_names {
                let (omitted, live) = unbound_locals::omitted_assert_bindings(statements);
                self.omitted_assert_locals = omitted;
                self.omitted_assert_free
                    .retain(|name, _| !live.contains(name));
            }
            let (guarded, nullable) = unbound_locals::local_assignments(self, statements);
            self.nullable_assignments = nullable;
            let mut names: Vec<_> = guarded.into_iter().collect();
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
        let mut falls_through = true;
        let reachable = self.reachable;
        let enclosing_sum = self.zip_sum_rhs;
        let mut previous_sum = false;
        let mut length_guard = None;
        let mut fallthrough_field = None;
        // Checking an unplaced body still needs the current statement for narrowing.
        let mut discarded = Vec::new();
        for (index, statement) in statements.iter().enumerate() {
            let mut fresh = None;
            if let ast::Stmt::Assign(assignment) = statement
                && let [ast::Expr::Name(name)] = assignment.targets.as_slice()
                && !self.names.contains_key(name.id.as_str())
            {
                fresh = Some((name, &*assignment.value));
                let later = &statements[index + 1..];
                let empty_list =
                    matches!(&*assignment.value, ast::Expr::List(list) if list.elts.is_empty());
                let empty_dict =
                    matches!(&*assignment.value, ast::Expr::Dict(dict) if dict.items.is_empty());
                // A registered call constructs its set through the live Python binding.
                let empty_set = self.registered.is_none()
                    && matches!(&*assignment.value, ast::Expr::Call(call) if matches!(&*call.func, ast::Expr::Name(value) if value.id == "set" && !self.names.contains_key("set")) && self.resolve(&call.func).is_none() && call.arguments.args.is_empty() && call.arguments.keywords.is_empty());
                let returned = || {
                    later.iter().any(|statement| matches!(statement, ast::Stmt::Return(ret) if matches!(ret.value.as_deref(), Some(ast::Expr::Name(value)) if value.id == name.id)))
                };
                let context = if assignment.value.is_lambda_expr() && !self.module_scope {
                    self.local_lambda_context(name.id.as_str(), later)
                } else if (empty_list && self.types.element(self.returns).is_some()
                    || empty_dict && self.types.mapping(self.returns).is_some())
                    && returned()
                {
                    Some(self.returns)
                } else if empty_list && later.iter().any(|s| matches!(s, ast::Stmt::Assign(a)
                    if matches!(&*a.value, ast::Expr::Name(value) if value.id == name.id)
                    && a.targets.iter().any(|target| matches!(target, ast::Expr::Subscript(sub)
                        if matches!(&*sub.slice, ast::Expr::Slice(_)) && self.target_context(target, None) == Some(self.types.list(documents::ANY)))))) {
                    Some(self.types.list(documents::ANY))
                } else if empty_list || empty_set {
                    self.inserted_element(name.id.as_str(), empty_set, later)
                        .map(|ty| if empty_list { self.types.list(ty) } else { self.types.set(ty) })
                } else {
                    None
                };
                if let Some(ty) = context {
                    let slot = self.declare(name.id.as_str(), ty, name)?;
                    if empty_list || empty_set {
                        self.remember_inferred_container(slot);
                    }
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
            let parent = self
                .fixed_sites
                .as_mut()
                .and_then(|sites| sites.enter(statement.range()));
            let emitting = output.is_some();
            let body = output.as_deref_mut().unwrap_or(&mut discarded);
            let start = body.len();
            if emitting && self.registered.is_some() && !statement.is_assert_stmt() {
                registered::push_checkpoint(body, statement);
            }
            let ops = body.len();
            let continues = if !self.module_scope
                && self.block_depth == 1
                && fresh.is_some()
                && let Some((target, parser)) = self
                    .saved_datetime_parser(statement, calendar::ParserBinding::Local(statements))
            {
                self.assign(target, parser, Some(body))?;
                true
            } else {
                let depth = self.present_fields.len();
                self.guard_return_field(std::slice::from_ref(statement), fallthrough_field);
                // A later statement can inspect these instructions for narrowing.
                let output = (emitting
                    || !statement.is_if_stmt()
                    || index + 1 < statements.len()
                    || self.fixed_sites.is_some())
                .then_some(&mut *body);
                let result = self.statement(statement, output);
                self.present_fields.truncate(depth);
                result?
            };
            let lowered = &mut body[ops..];
            fallthrough_field = records::fallthrough_field(lowered);
            self.fixed_tuple = enclosing_tuple;
            if !self.module_scope
                && let Some((name, rhs)) = fresh
            {
                self.widen_reassigned_text(name, rhs, lowered, &statements[index + 1..])?;
            }
            length_guard = self
                .list_length_guard(statement)
                .or_else(|| length_guard.filter(|guard| fixed_tuples::preserves(*guard, lowered)));
            self.remember_regex_assignment(statement, body, ops, &statements[index + 1..]);
            if let Some(flag) =
                unbound_locals::assigned(statement).and_then(|n| self.bound_flags.get(n))
            {
                body.push(Statement::Assign(*flag, Expr::boolean(true)));
            }
            if emitting && self.registered.is_none() && body.len() > ops && self.fault_enabled() {
                body.insert(start, self.fault_line(statement));
            }
            self.leave_fixed_site(parent);
            if !emitting {
                body.clear();
            } else if self.discarded_function_body() {
                // The fixed body has lost its physical operands. Continue the source
                // checks here if the Python frame cannot validate the whole declaration.
                if place.take().is_some_and(|place| place(self)) {
                    break;
                }
                *body = Vec::new();
                output = None;
            }
            falls_through &= continues;
        }
        self.reachable = reachable;
        self.zip_sum_rhs = enclosing_sum;
        self.block_depth -= 1;
        Ok(falls_through)
    }

    fn require_registered_operation(&self, site: &impl Ranged) -> Result<(), String> {
        let mut missing = self.unrepresented.borrow_mut();
        let failure = missing.get_or_insert_with(|| {
            self.classified_error(
                Kind::ScopedObject,
                site,
                "operation requires its compiled object representation",
            )
        });
        if self.registered_local_start.is_some() {
            return Err(failure.clone());
        }
        Ok(())
    }

    fn missing_operation(&self, failure: &Failure) -> bool {
        if !failure.is_representation() || !self.graph.modules[self.module].is_program_source() {
            return false;
        }
        self.unrepresented
            .borrow_mut()
            .get_or_insert_with(|| failure.clone());
        self.registered_local_start.is_none()
    }

    /// Check-only operands cannot leave the source region in an executable body.
    fn finish_region<T>(&self, result: Result<T, String>) -> Result<T, String> {
        match (result, self.unrepresented.take()) {
            (Ok(_), Some(failure)) => Err(failure),
            (Err(later), Some(first)) if later.is_representation() => Err(first),
            (
                Err(Failure::PendingSignature { .. }),
                Some(first @ Failure::PendingSignature { .. }),
            ) => Err(first),
            (result, _) => result,
        }
    }

    /// Defer an opaque operation only when its result need not enter typed storage.
    fn defer_object_error(&self, error: &Failure) -> bool {
        let Failure::Diagnostic(diagnostic) = error else {
            return false;
        };
        if !error.is_object() {
            return false;
        }
        if self.registered.is_some() {
            self.unrepresented
                .borrow_mut()
                .get_or_insert_with(|| error.clone());
        } else if self.inference.is_none() {
            self.graph
                .deferred_object_error
                .borrow_mut()
                .get_or_insert_with(|| diagnostic.clone().in_module(self.graph, self.module));
        }
        true
    }

    fn ordinary_entry(&self) -> Option<ModuleId> {
        (self.module_scope
            && self.module == self.graph.entry
            && (self.inference.is_none() || self.module_namespace.is_unexposed())
            && self.block_depth <= 1
            && self.reachable)
            .then_some(self.module)
    }

    fn statement(
        &mut self,
        statement: &ast::Stmt,
        output: Option<&mut Vec<Statement>>,
    ) -> Result<bool, String> {
        if self.bound_function_imports
            && self.graph.session.sole_local_import(self.module, statement)
        {
            let mut discarded = Vec::new();
            let body = output.unwrap_or(&mut discarded);
            let start = body.len();
            let falls_through = self.lower_statement(statement, Some(body))?;
            // A native intrinsic may have consumed the import without producing its
            // Python namespace operation. Keep its original prepared function in that case.
            if !matches!(
                &body[start..],
                [Statement::Expr(Expr {
                    kind: ExprKind::Builtin(Builtin::RegisteredImport(_), _),
                    ..
                })]
            ) {
                self.require_registered_operation(statement)?;
            }
            return Ok(falls_through);
        }
        if self.registered.is_some() {
            // Fixed layouts can erase Python stores (for example, a local += becomes +).
            // Keep every operation in this frame explicit until it has an object-ABI lowering.
            let supported = match statement {
                ast::Stmt::AnnAssign(assign) => assign.target.is_name_expr(),
                ast::Stmt::Assign(assign) => {
                    assign.targets.iter().all(ast::Expr::is_name_expr)
                        || matches!(
                            assign.targets.as_slice(),
                            [ast::Expr::Attribute(_)
                                | ast::Expr::Subscript(_)
                                | ast::Expr::Tuple(_)
                                | ast::Expr::List(_)]
                        )
                }
                ast::Stmt::AugAssign(assign) => assign
                    .target
                    .as_name_expr()
                    .and_then(|name| self.registered_in_place_local(name.id.as_str()))
                    .is_some(),
                ast::Stmt::Assert(_) | ast::Stmt::FunctionDef(_) => {
                    self.registered_local_start.is_some()
                }
                ast::Stmt::With(scope) => !scope.is_async,
                ast::Stmt::For(_) | ast::Stmt::While(_) => true,
                ast::Stmt::If(_) => true,
                ast::Stmt::Try(protected) => !protected.is_star,
                ast::Stmt::Break(_) | ast::Stmt::Continue(_) => true,
                ast::Stmt::Return(_) | ast::Stmt::Raise(_) => true,
                ast::Stmt::Expr(_) | ast::Stmt::Pass(_) => true,
                _ => false,
            };
            if !supported {
                self.require_registered_operation(statement)?;
            }
        }
        let entry = self.ordinary_entry();
        let enclosing = std::mem::replace(
            &mut self.unhighlighted_call,
            imre_ast::unhighlighted_call(statement),
        );
        let result = self.lower_statement(statement, output);
        self.unhighlighted_call = enclosing;
        if let Err(error) = &result {
            // A boxed operation in a checked source file must not hide a bad call in
            // another function, whether this operation is deferred or stops lowering.
            if self.inference.is_none()
                && self.registered.is_none()
                && !self.graph.modules[self.module].bundled
                && error.is_object()
            {
                self.graph.require_fallback_calls();
            }
            match statement {
                ast::Stmt::Expr(expression)
                    if matches!(error, Failure::PendingSignature { .. })
                        && self.inference.is_some()
                        && self.graph.modules[self.module].is_program_source()
                        && narrowing::expression_assignments(&expression.value).is_empty() =>
                {
                    // The value is discarded. Check independent later statements, but keep
                    // this dependency for the next inference round.
                    let mut blocked = self.unrepresented.borrow_mut();
                    if !matches!(*blocked, Some(Failure::PendingSignature { .. })) {
                        *blocked = Some(error.clone());
                    }
                    return Ok(true);
                }
                ast::Stmt::Expr(_) | ast::Stmt::Assert(_) if self.missing_operation(error) => {
                    return Ok(true);
                }
                ast::Stmt::Assign(assign)
                    if let [ast::Expr::Name(target)] = assign.targets.as_slice()
                        && self.missing_operation(error) =>
                {
                    // Only the representation of the store failed. Normal successors see
                    // its binding, including the type required by a prior declaration.
                    let name = target.id.as_str();
                    let ty = self
                        .names
                        .get(name)
                        .map_or(Type::PythonObject, |&slot| self.locals[slot]);
                    let slot = self.declare(name, ty, target)?;
                    self.assigned_value(slot, &Expr::unrepresented());
                    return Ok(true);
                }
                _ => {}
            }
        }
        result.map_err(|error| error.in_entry_statement(entry, statement.start()))
    }

    fn lower_statement(
        &mut self,
        statement: &ast::Stmt,
        output: Option<&mut Vec<Statement>>,
    ) -> Result<bool, String> {
        let checking = output.is_none();
        let mut discarded = Vec::new();
        let body = output.unwrap_or(&mut discarded);
        if self.graph.local_static_import(self.module, statement) {
            return Ok(true);
        }
        if let ast::Stmt::ImportFrom(import) = statement
            && let Some(bindings) = self.local_sys_import(import)?
        {
            statements::append_block(body, bindings);
            return Ok(true);
        }
        if self.bound_function_imports
            && let Some(import) = registered::registered_import(statement, None)
        {
            body.push(Statement::Expr(import));
            return Ok(true);
        }
        if self.module_scope && local_functions::type_variable(self.graph, self.module, statement) {
            return Ok(true);
        }
        // Fixed parser schemas absorb their declarations. A recovered Python frame needs
        // the live parser binding and each registration in source order.
        if self.registered_local_start.is_none()
            && self.classes.parsers.declaration(self.module, statement)
        {
            return Ok(true);
        }
        let module = &self.graph.modules[self.module];
        if let ast::Stmt::Global(global) = statement
            && global.names.iter().all(|name| {
                matches!(module.bindings.get(name.as_str()), Some(Binding::Value(first))
                    if first.module == self.module && module.rebound_values.contains(&first.statement))
            })
        {
            return Ok(true);
        }
        if let Some(first) = module.rebound_target(self.module, statement)
            && (self.module_scope
                || crate::modules::value_target(statement).is_some_and(|target| {
                    self.graph.session.explicitly_global(self.module, target)
                }))
        {
            body.push(self.replace_global(first, statement)?);
            return Ok(true);
        }
        // A refused representation still contributes argument and receiver facts to inference.
        // finish_region discards its body after the remaining operands have been checked.
        if self.registered.is_some()
            && self.unrepresented.borrow().is_some()
            && let Some(targets) = registered::attribute_effects(statement)
            && targets.iter().all(|effect| {
                self.graph
                    .session
                    .is_dynamic_receiver(self.module, &effect.field.value)
            })
        {
            for effect in targets {
                let receiver = usize::from(effect.action == ir::AttributeAction::Store);
                let mut python_receiver = false;
                for (index, operand) in effect.operands().enumerate() {
                    let value = self.expr(operand)?;
                    if index == receiver {
                        python_receiver = value.ty == Type::PythonObject;
                    }
                }
                if !python_receiver {
                    return Err(error(
                        self.source,
                        effect.field,
                        "attribute target requires a Python object",
                    ));
                }
            }
            return Ok(true);
        }
        if checking && let ast::Stmt::If(branch) = statement {
            return self.if_chain(branch, None);
        }
        statements::StatementLowering::source_statement(self, statement, self.module_scope, body)
    }

    fn specialized_statement(
        &mut self,
        statement: &ast::Stmt,
    ) -> Result<statements::Block, String> {
        let result = match statement {
            ast::Stmt::ClassDef(class) if !self.module_scope => {
                self.define_local_class(class)?;
                None
            }
            ast::Stmt::AnnAssign(assign) => {
                let ast::Expr::Name(name) = &*assign.target else {
                    return Err(error(self.source, statement, "only names may be annotated"));
                };
                let document = documents::annotation(self.graph, self.module, &assign.annotation);
                let ty = if document == Some(documents::ANY) {
                    Type::ObjectValue
                } else {
                    annotation_in_class(
                        &assign.annotation,
                        self.graph,
                        self.module,
                        self.classes.declarations,
                        self.types,
                        self.class,
                    )?
                };
                if let Some(slot) = self.names.get(name.id.as_str()) {
                    self.inferred_containers.remove(slot);
                }
                let document = if ty == Type::JsonObject {
                    Some(documents::MAP)
                } else {
                    document
                };
                let list_site = (!self.names.contains_key(name.id.as_str())
                    && matches!(assign.value.as_deref(), Some(ast::Expr::List(list)) if list.elts.is_empty())
                    && self.types.element(ty) == Some(self.types.list(Type::ObjectValue))
                    && documents::collects_lists(self.graph, self.module, &assign.annotation))
                    .then(|| assign.start().to_u32());
                let ty = if list_site.is_some_and(|site| {
                    self.types
                        .collects_document_lists(self.module, site, self.class)
                }) {
                    self.types.list(documents::LIST)
                } else {
                    ty
                };
                let checked = match (assign.value.as_deref(), document) {
                    (Some(rhs), Some(document)) => {
                        Some(self.document_annotation_value(rhs, ty, document)?)
                    }
                    _ => None,
                };
                let ty = checked.as_ref().map_or(ty, |value| value.ty);
                let declaration = TypeDeclaration {
                    module: self.module,
                    annotation: assign.annotation.range(),
                };
                // A dominating new annotation evaluates its RHS against the previous binding.
                let outer_local =
                    !self.module_scope && self.block_depth == 1 && self.local_functions.is_empty();
                let replacement =
                    self.names.get(name.id.as_str()).copied().filter(|&old| {
                        outer_local && self.locals[old] != ty && assign.value.is_some()
                    });
                let slot = if let Some(old) = replacement {
                    old
                } else {
                    let slot = self.declare(name.id.as_str(), ty, statement)?;
                    self.bind_declared_type(slot, declaration);
                    if let Some(site) = list_site {
                        self.document_list_sites.insert(slot, site);
                    } else {
                        self.document_list_sites.remove(&slot);
                    }
                    slot
                };
                if let Some(rhs) = &assign.value {
                    let value = checked.map_or_else(
                        || {
                            self.expr_in_context(
                                rhs,
                                ty,
                                |_, _| false,
                                Self::coerce_supplied_result,
                                Some(declaration),
                            )
                        },
                        Ok,
                    )?;
                    if self.ends_before_assignment(&value, &[])? {
                        if let Some(old) = replacement {
                            let slot = self.replacement_slot(name.id.as_str(), old, ty);
                            self.bind_declared_type(slot, declaration);
                        }
                        return Ok((vec![self.statement_at(rhs, Statement::Expr(value))], false));
                    }
                    if replacement.is_some() {
                        let statements = self.replace_local(name.id.as_str(), slot, ty, value);
                        self.bind_declared_type(self.names[name.id.as_str()], declaration);
                        return Ok((statements, true));
                    }
                    self.assigned_value(slot, &value);
                    let statement = Statement::Assign(slot, value);
                    Some(if outer_local {
                        self.statement_at(rhs, statement)
                    } else {
                        statement
                    })
                } else {
                    None
                }
            }
            ast::Stmt::AugAssign(assign) => {
                if let ast::Expr::Subscript(target) = &*assign.target {
                    let list = self.expr(&target.value)?;
                    if self.registered.is_some() && list.ty == Type::PythonObject {
                        let key = registered::values::index_key(&target.slice, &mut |expr| {
                            self.expr(expr)
                        })?;
                        let rhs = self.expr(&assign.value)?;
                        return Ok((
                            vec![Statement::AugmentItem(
                                list,
                                key,
                                ir::ItemUpdate::Protocol(assign.op.as_str()),
                                rhs,
                            )],
                            true,
                        ));
                    }
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
                    Some(Statement::AugmentItem(
                        list,
                        index,
                        ir::ItemUpdate::Fixed(op),
                        value,
                    ))
                } else if let ast::Expr::Attribute(target) = &*assign.target {
                    let receiver = Self::weak_target(self.expr(&target.value)?);
                    if let Some(property @ (class, _)) = self.writable_property(receiver.ty, target)
                    {
                        let slot = self.hidden(receiver.ty);
                        let record = Self::exception_record(self.local(slot));
                        let old = self.class_attribute(target, class, Some(record), None)?;
                        let rhs = self.expr(&assign.value)?;
                        let combined = self.binary_expr(assign.op, old, rhs, statement)?;
                        if self.types.element(combined.ty).is_some() {
                            return Err(error(
                                self.source,
                                target,
                                "in-place list operations are unsupported",
                            ));
                        }
                        let put =
                            self.set_property(property, target, self.local(slot), combined)?;
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
                            kind: ExprKind::Builtin(Builtin::Float(None), vec![value]),
                        }
                    } else {
                        value
                    };
                    Some(Statement::AugmentField(receiver, field, op, value))
                } else if let ast::Expr::Name(name) = &*assign.target {
                    let left = self.expr(&assign.target)?;
                    if self.registered_local_start.is_some() {
                        let right = self.expr(&assign.value)?;
                        let value = registered::in_place(assign.op, left, right);
                        let slot = self.names[name.id.as_str()];
                        // An in-place or reflected method can replace the unannotated local.
                        self.locals[slot] = Type::PythonObject;
                        self.assigned_value(slot, &value);
                        return Ok((vec![Statement::Assign(slot, value)], true));
                    }
                    if assign.op == ast::Operator::Add
                        && let Some(element) = self.types.deque_element(left.ty)
                    {
                        let value = self.deque_extend(left, element, &assign.value, "__iadd__")?;
                        let mut suite = Vec::with_capacity(1);
                        self.assign(&assign.target, value, Some(&mut suite))?;
                        return Ok((suite, true));
                    }
                    let right = self.expr(&assign.value)?;
                    if assign.op == ast::Operator::Add
                        && (left.ty == Type::PythonObject
                            || right.ty == Type::PythonObject
                            || (left.ty == right.ty && self.types.element(left.ty).is_some()))
                    {
                        // The registered name store owns __iadd__ and its result. Even
                        // two lists with the same layout must preserve the original alias.
                        let failure = self.scoped_builtin_error("in-place addition", statement);
                        if !self.missing_operation(&failure) {
                            return Err(failure);
                        }
                        if let Some(&slot) = self.names.get(name.id.as_str()) {
                            self.assigned_value(slot, &Expr::unrepresented());
                        }
                        return Ok((Vec::new(), true));
                    }
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
                    let mut suite = Vec::with_capacity(1);
                    self.assign(&assign.target, value, Some(&mut suite))?;
                    return Ok((suite, true));
                } else {
                    return Err(error(
                        self.source,
                        statement,
                        "unsupported assignment target",
                    ));
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
                        if let Some(name) = self.boxed_float_attribute(attr, receiver.ty) {
                            return Err(self.scoped_builtin_error(name, attr));
                        }
                        if matches!(receiver.ty, Type::Coroutine(_))
                            && attr.attr.as_str() == "cr_origin"
                        {
                            return Err(self.object_builtin_error("coroutine.cr_origin", attr));
                        }
                        if receiver.ty == Type::CodecInfo
                            && matches!(
                                attr.attr.as_str(),
                                "_is_text_encoding" | "incrementaldecoder" | "incrementalencoder"
                            )
                        {
                            return Err(self.object_builtin_error(attr.attr.as_str(), attr));
                        }
                        if matches!(receiver.ty, Type::Popen(..)) && attr.attr.as_str() == "poll" {
                            statements.push(Statement::Expr(Expr {
                                ty: Type::None,
                                kind: ExprKind::Builtin(
                                    Builtin::process(ir::ProcessOperation::PopenDeletePoll),
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
                    if let Some(ty) = self.environ_item_type(&target.value) {
                        let key = self.expr_expected(&target.slice, ty)?;
                        statements.push(Statement::Expr(Expr {
                            ty: Type::None,
                            kind: ExprKind::Builtin(
                                Builtin::process(ir::ProcessOperation::EnvironDelete {
                                    bytes: ty == Type::Bytes,
                                }),
                                vec![key],
                            ),
                        }));
                        continue;
                    }
                    let receiver = self.expr(&target.value)?;
                    if self.registered.is_some() && receiver.ty == Type::PythonObject {
                        let key = registered::values::index_key(&target.slice, &mut |expr| {
                            self.expr(expr)
                        })?;
                        statements.push(Statement::Expr(registered::delete_item(receiver, key)));
                        continue;
                    }
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
                    statements.push(Statement::Expr(registered::delete_item(receiver, key)));
                }
                return Ok((statements, true));
            }
            ast::Stmt::Match(branch) => return self.match_statement(branch),
            ast::Stmt::While(loop_) => {
                self.invalidate_loop_facts(&loop_.body, None)?;
                for name in narrowing::expression_assignments(&loop_.test) {
                    if let Some(slot) = self.names.get(&name) {
                        self.narrowed.remove_mut(slot);
                        self.assigned
                            .set_type_source(*slot, self.declared_slots.get(slot).copied());
                    }
                }
                let test = self.condition(&loop_.test)?;
                let entered = match &*loop_.test {
                    ast::Expr::BooleanLiteral(literal) => Some(literal.value),
                    ast::Expr::NumberLiteral(ast::ExprNumberLiteral {
                        value: ast::Number::Int(n),
                        ..
                    }) => Some(n.as_i64() != Some(0)),
                    _ => None,
                };
                let namespace = self.module_namespace.clone();
                let facts = self.narrowed.clone();
                let before = self.assigned.clone();
                self.apply_facts(&loop_.test, true);
                self.loop_breaks.push(None);
                let (body, _) = self.loop_suite(&loop_.body)?;
                let breaks = self.loop_breaks.pop().unwrap();
                self.assigned.after_iteration(before);
                self.narrowed = facts;
                self.apply_facts(&loop_.test, false);
                if entered == Some(false) {
                    self.module_namespace = namespace;
                } else {
                    self.module_namespace.join(&namespace);
                }
                let can_exhaust = entered != Some(true);
                let (orelse, falls) = self.loop_else(&loop_.orelse, breaks, can_exhaust)?;
                let statement =
                    (!self.discarded_function_body()).then(|| Statement::While(test, body, orelse));
                return Ok((statement.into_iter().collect(), falls));
            }
            ast::Stmt::Assert(assertion) if self.graph.python_optimization() > 0 => {
                assertion_syntax::validate(self.source, assertion)?;
                None
            }
            ast::Stmt::Assert(assert) => {
                let test = self.condition(&assert.test)?;
                let successful = self.local_state();
                let result = if self.registered.is_some() {
                    let message = assert
                        .msg
                        .as_deref()
                        .map(|message| self.expr(message))
                        .transpose()?;
                    Vec::from(registered::assertion_statements(assert, test, message))
                } else {
                    let message = assert
                        .msg
                        .as_deref()
                        .map(|message| self.expr_expected(message, Type::Str).map(Box::new))
                        .transpose()?;
                    vec![Statement::If(
                        test,
                        Vec::new(),
                        vec![Statement::raise(Expr {
                            ty: Type::Exception,
                            kind: ExprKind::Exception { kind: 9, message },
                        })],
                    )]
                };
                self.restore_state(successful);
                self.apply_facts(&assert.test, true);
                return Ok((result, true));
            }
            ast::Stmt::With(with_) if !with_.is_async => {
                if self.registered.is_some() {
                    return self.checked_python_with(with_);
                }
                let keep_body = !self.discarded_function_body();
                if !with_.body.iter().any(ast::Stmt::is_function_def_stmt) {
                    return self.with_items(&with_.items, &with_.body, keep_body);
                }
                let enclosing = self.local_functions.clone();
                self.collect_suite_definitions(&with_.body, false)?;
                let result = self.with_items(&with_.items, &with_.body, keep_body);
                self.local_functions = enclosing;
                return result;
            }
            ast::Stmt::With(with_) if self.asynchronous => {
                return self.async_with(&with_.items, &with_.body);
            }
            ast::Stmt::Import(_) | ast::Stmt::ImportFrom(_)
                if self.graph.command_startup.is_some() =>
            {
                return Err(self.object_runtime_export("a conditional or local import", statement));
            }
            ast::Stmt::Import(_) | ast::Stmt::ImportFrom(_) if !self.suite_imports => None,
            ast::Stmt::TypeAlias(_) => {
                return Err(self.scoped_builtin_error("type alias declaration", statement));
            }
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
        keep_body: bool,
    ) -> Result<(Vec<Statement>, bool), String> {
        contexts::with_items(
            self,
            items.iter().enumerate(),
            |scope, (index, item), out| scope.acquire_with(out, item, &items[index + 1..], body),
            |scope, protected| scope.lower_block(body, keep_body.then_some(protected)),
        )
    }

    fn acquire_with<'s>(
        &mut self,
        out: &'s mut Vec<Statement>,
        item: &ast::WithItem,
        remaining: &[ast::WithItem],
        body: &[ast::Stmt],
    ) -> Result<contexts::Protected<'s>, String> {
        use std::ops::ControlFlow::{Break, Continue};
        if let ast::Expr::Call(call) = &item.context_expr
            && self.resolve(&call.func) == Some(Binding::Intrinsic(Intrinsic::CatchWarnings))
        {
            if !call.arguments.keywords.is_empty() {
                return Err(self.scoped_builtin_error("warnings.catch_warnings", call));
            }
            self.arguments(call, &[], None)?;
            let mut protected = Vec::new();
            if let Some(target) = &item.optional_vars {
                self.store_value(target, Expr::none(), None, Some(&mut protected))?;
            }
            let action = |op| {
                Statement::Expr(Expr {
                    ty: Type::None,
                    kind: ExprKind::Builtin(op, vec![]),
                })
            };
            return Ok(Continue(contexts::enter_native(
                out,
                vec![action(Builtin::WarningScopeEnter)],
                protected,
                Vec::new(),
                action(Builtin::WarningScopeExit),
                Vec::new(),
            )));
        }
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
            self.assert_raises_scope(out, item, remaining, body, class, manager)?;
            return Ok(Break(true));
        }
        if let Type::Class(class) = manager.ty {
            let record = &self.classes.declarations.definitions[class];
            if record.name == "_SubTestContext"
                && self.graph.modules[record.definition.module].is_bundled("unittest")
            {
                self.subtest_scope(out, item, remaining, body, class, manager)?;
                return Ok(Break(true));
            }
        }
        if self.types.payload(manager.ty).unwrap_or(manager.ty) == Type::HandlerLock {
            return Ok(Continue(self.handler_lock_context(out, item, manager)?));
        }
        if let Type::Class(class) = manager.ty
            && self.bundled_mock_class(class, "_DictionaryPatch")
        {
            return Ok(Continue(
                self.mock_dictionary_context(out, item, class, manager)?,
            ));
        }
        if manager.ty == Type::GeneratorContext {
            return Ok(Continue(self.enter_generator_context(out, item, manager)?));
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
            let failure = error(
                self.source,
                &item.context_expr,
                "with requires a supported file, socket, ZIP archive, or directory context",
            );
            if manager.ty == Type::PythonObject {
                let failure = failure.classify(Kind::ScopedObject);
                if self.missing_operation(&failure) {
                    // __enter__ supplies the optional target. Its storage is unknown,
                    // but the same checker can bind it and check the protected suite.
                    if let Some(target) = &item.optional_vars {
                        self.store_value(target, Expr::unrepresented(), None, None)?;
                    }
                    return Ok(Continue(out));
                }
                return Err(failure);
            }
            return Err(failure);
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
            self.assign(target, value, Some(&mut protected))?;
        }
        // The server cleanup is a checked virtual call. Preserve the protected
        // suite's first failure before checking its possible close methods.
        let server_body = if server.is_some() {
            Some(self.with_items(remaining, body, true)?)
        } else {
            None
        };
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
            Builtin::process(ir::ProcessOperation::PopenExit)
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
        let protected = contexts::enter_native(
            out,
            statements,
            protected,
            Vec::new(),
            Statement::Expr(cleanup),
            vec![captured],
        );
        Ok(match server_body {
            Some(tail) => Break(contexts::finish(protected, tail)),
            None => Continue(protected),
        })
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
        let exception = Expr {
            ty: Type::Exception,
            kind: ExprKind::Exception {
                kind,
                message: arg.map(Box::new),
            },
        };
        self.record_named_call(call, &exception);
        Ok(exception)
    }

    fn fault_enabled(&self) -> bool {
        self.graph
            .command_startup
            .as_ref()
            .is_some_and(|(flags, _)| flags.faulthandler)
    }

    fn source_filename(&self) -> String {
        if self.graph.virtual_command && self.module == self.graph.entry {
            "<string>".to_owned()
        } else {
            self.graph
                .module_filename(self.module)
                .to_string_lossy()
                .into_owned()
        }
    }

    fn source_line(&self, start: TextSize) -> u32 {
        self.graph.session.source_line(self.module, start)
    }

    fn fault_line(&self, site: &impl Ranged) -> Statement {
        Statement::Expr(Expr {
            ty: Type::None,
            kind: ExprKind::Builtin(Builtin::FaultLine(self.source_line(site.start())), vec![]),
        })
    }

    fn fault_frame(&self, body: &mut Vec<Statement>, name: &str, start: TextSize) {
        if self.fault_enabled() {
            body.insert(
                0,
                fault_frame_statement(self.source_filename(), name, self.source_line(start)),
            );
        }
    }

    fn fixed_frame(&mut self) -> Arc<ir::FixedFrame> {
        match &self.frame {
            LexicalFrame::Positioned(frame) => Arc::clone(frame),
            LexicalFrame::Named(name) => {
                let frame = Arc::new(ir::FixedFrame {
                    filename: self.source_filename(),
                    function: name.clone(),
                });
                self.frame = LexicalFrame::Positioned(Arc::clone(&frame));
                frame
            }
        }
    }

    fn checked_builtin_call(
        &mut self,
        operation: fn(Option<Box<ir::FixedOrigin>>) -> Builtin,
        call: &ast::ExprCall,
        args: &[Expr],
    ) -> Builtin {
        let builtin = operation(None);
        if builtin.contract(args).may_raise {
            operation(Some(Box::new(self.call_origin(call))))
        } else {
            builtin
        }
    }

    fn call_origin(&mut self, call: &ast::ExprCall) -> ir::FixedOrigin {
        self.fixed_origin(call.range(), Some((call.func.end(), b'(')))
    }

    fn fixed_origin(
        &mut self,
        range: TextRange,
        delimiter: Option<(TextSize, u8)>,
    ) -> ir::FixedOrigin {
        let (line, line_start) =
            self.graph
                .session
                .source_line_start(self.module, self.source, range.start());
        let span = self.source.get(range.to_std_range()).and_then(|text| {
            if self.unhighlighted_call == Some(range) || text.contains('\n') {
                return None;
            }
            let start = range.start() - line_start;
            let split = match delimiter {
                Some((prefix, opener)) => {
                    imre_ast::traceback_delimiter(
                        self.source.as_bytes(),
                        prefix.to_usize(),
                        range.end().to_usize(),
                        opener,
                    )? - line_start.to_usize()
                }
                None => start.to_usize(),
            };
            Some([
                start.to_u32(),
                u32::try_from(split).ok()?,
                (range.end() - line_start).to_u32(),
            ])
        });
        ir::FixedOrigin {
            frame: self.fixed_frame(),
            line,
            span,
        }
    }

    fn caught_exception(&self, slot: usize) -> Option<&CaughtException> {
        self.caught_exceptions
            .iter()
            .rev()
            .find(|value| value.slot == slot)
    }

    fn exception_origin(&mut self, exception: Expr, site: &impl Ranged) -> Expr {
        Expr {
            ty: Type::Exception,
            kind: ExprKind::Builtin(
                Builtin::ExceptionOrigin(self.fixed_origin(site.range(), None)),
                vec![exception],
            ),
        }
    }

    /// Binds an explicit native cause, including a member raised from its exception group.
    fn exception_with_cause(
        &mut self,
        raise: &ast::StmtRaise,
        raised: Expr,
        cause: &ast::Expr,
    ) -> Result<(Expr, Option<Expr>), String> {
        if let ExprKind::Builtin(Builtin::Process(ir::ProcessOperation::ExceptionAt, _), args) =
            &raised.kind
            && matches!(
                &args[1].kind,
                ExprKind::Int(_) | ExprKind::Unary(UnaryOp::USub, _)
            )
            && let ExprKind::Local(slot) = args[0].kind
            && let Some(name) = cause.as_name_expr()
            && self.names.get(name.id.as_str()) == Some(&slot)
        {
            let group = self.expr_expected(cause, Type::Exception)?;
            let member = Expr {
                ty: Type::Exception,
                kind: ExprKind::Builtin(
                    Builtin::process(ir::ProcessOperation::MemberCause),
                    vec![raised, group],
                ),
            };
            return Ok((member, None));
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
            ExprKind::Builtin(Builtin::Process(ir::ProcessOperation::ExceptionView, _),args)
                if matches!(args[0].kind, ExprKind::Builtin(Builtin::NamedException { .. } | Builtin::ImportErrorNew { .. }, _)))
        {
            return Err(error(
                self.source,
                raise,
                "explicit causes require a freshly constructed builtin exception",
            ));
        }
        let expected = self.types.optional(Type::Exception);
        let cause = if let Some(cause) = self.raised_class(cause)? {
            self.coerce(cause, expected, raise)?
        } else {
            self.expr_expected(cause, expected)?
        };
        Ok((raised, Some(cause)))
    }

    fn raised_class(&mut self, value: &ast::Expr) -> Result<Option<Expr>, String> {
        let Some(kind) = self.exception_category(value) else {
            return Ok(None);
        };
        if self.class_reference(value).is_some() {
            return Err(error(
                self.source,
                value,
                "raise a constructed exception payload with its required arguments",
            ));
        }
        let exception = if self.is_import_error(kind) {
            self.import_error_value(kind, Vec::new(), vec![], [None; 3])
        } else if kind == 32 {
            Expr {
                ty: Type::Exception,
                kind: ExprKind::Builtin(Builtin::SystemExitNew { omit_none: false }, Vec::new()),
            }
        } else if matches!(kind, 0 | 18 | 25..=31 | 34) {
            return Err(error(
                self.source,
                value,
                "this exception category is catch-only",
            ));
        } else {
            Expr {
                ty: Type::Exception,
                kind: ExprKind::Exception {
                    kind,
                    message: None,
                },
            }
        };
        Ok(Some(exception))
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
            Some(Binding::Intrinsic(Intrinsic::ScopedObjectExport(
                "socket.timeout" | "_socket.timeout",
            ))) => exception_kind("TimeoutError"),
            Some(Binding::Intrinsic(Intrinsic::GaiError)) => Some(68),
            Some(Binding::Intrinsic(Intrinsic::HError)) => Some(72),
            Some(Binding::Intrinsic(Intrinsic::BinasciiError)) => Some(69),
            Some(Binding::Intrinsic(Intrinsic::BinasciiIncomplete)) => Some(70),
            Some(Binding::Intrinsic(Intrinsic::PackageNotFoundError)) => Some(71),
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

    /// A literal builtin constructor cannot raise another exception of its own class.
    /// A handler naming that exact class therefore knows the class of the caught value.
    fn directly_raised_builtin(body: &[Statement], handlers: &[u32]) -> Option<&'static str> {
        let [Statement::Raise(Some(raised))] = body else {
            return None;
        };
        if raised.origin.is_none() || raised.cause.is_some() {
            return None;
        }
        let ExprKind::Exception { kind, message } = &raised.exception.kind else {
            return None;
        };
        if handlers != [*kind]
            || message
                .as_deref()
                .is_some_and(|arg| !matches!(&arg.kind, ExprKind::Str(_)))
        {
            return None;
        }
        let class = imre_ast::exceptions::Kind::try_from_raw(*kind)?;
        (exception_kind(class.name()) == Some(*kind)).then_some(class.name())
    }

    /// A handler may run after any failing operation, before its assignment has
    /// committed. Only incoming definite assignments survive; writes invalidate
    /// incoming optional facts. Finally uses the same conservative entry for all
    /// normal, exceptional, return, and loop exits.
    fn try_statement(&mut self, try_: &ast::StmtTry) -> Result<(Statement, bool), String> {
        let before = self.assigned.clone();
        let before_facts = self.narrowed.clone();
        let earlier_returns = self.explicit_return;
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
        let (mut deleted, raises) = narrowing::protected_deletions(protected_prefix);
        for name in &deleted {
            if let Some(slot) = self.names.get(name) {
                handler_assigned.remove(slot);
            }
        }
        let mut handler_facts = before_facts.clone();
        let mut assigned = narrowing::assigned_names(&try_.body);
        for name in &assigned {
            if let Some(slot) = self.names.get(name) {
                handler_facts.remove_mut(slot);
                handler_assigned.set_type_source(*slot, self.declared_slots.get(slot).copied());
            }
        }
        let mut handlers = Vec::new();
        for (index, handler) in try_.handlers.iter().enumerate() {
            let ast::ExceptHandler::ExceptHandler(handler) = handler;
            self.assigned = handler_assigned.clone();
            self.narrowed = handler_facts.clone();
            let categories = if self.registered.is_some() {
                None
            } else if let Some(target) = &handler.type_ {
                let expressions = if let ast::Expr::Tuple(tuple) = &**target {
                    tuple.elts.as_slice()
                } else {
                    std::slice::from_ref(&**target)
                };
                let mut kinds = Vec::with_capacity(expressions.len());
                for expression in expressions {
                    if let Some(kind) = self
                        .exception_category(expression)
                        .or_else(|| self.fixed_testcase_failure_kind(expression))
                    {
                        kinds.push(kind);
                        continue;
                    }
                    match self.resolve(expression) {
                        Some(Binding::RegisteredClass(site))
                            if self.graph.session.is_exception_declaration(site) => {}
                        Some(Binding::Intrinsic(Intrinsic::ScopedObjectExport(
                            name @ ("_pickle.PickleError"
                            | "_pickle.PicklingError"
                            | "_pickle.UnpicklingError"
                            | "lzma.LZMAError"),
                        ))) => return Err(self.scoped_builtin_error(name, expression)),
                        _ => {
                            return Err(error(
                                self.source,
                                expression,
                                "exception handlers require fixed builtin categories",
                            ));
                        }
                    }
                }
                (kinds.len() == expressions.len()).then_some(kinds)
            } else {
                if index + 1 != try_.handlers.len() {
                    return Err(error(self.source, handler, "a bare handler must be last"));
                }
                None
            };
            let matcher = if let Some(kinds) = categories {
                ir::ExceptionMatcher::Categories(kinds)
            } else {
                registered::exception_matcher(handler, try_.is_star, |class, _| {
                    if self.registered.is_some() {
                        return self.expr(class);
                    }
                    // Continue checking the handler body. Its class operand needs a Python frame.
                    self.require_registered_operation(class)?;
                    Ok(Expr::unrepresented())
                })?
            };
            let name = handler
                .name
                .as_ref()
                .map(|name| {
                    let (ty, kinds) = match &matcher {
                        ir::ExceptionMatcher::Categories(kinds) => {
                            (Type::Exception, kinds.as_slice())
                        }
                        _ => (Type::PythonObject, &[][..]),
                    };
                    let slot = self.declare(name.as_str(), ty, handler)?;
                    self.assigned.insert(slot);
                    self.narrowed.remove_mut(&slot);
                    if let [kind] = kinds
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
                    Ok::<_, Failure>(slot)
                })
                .transpose()?;
            let before_handler = name.and_then(|_| self.take_breaks());
            let capture_depth = self.caught_exceptions.len();
            let caught = name.zip(handler.name.as_ref()).and_then(|(slot, target)| {
                let ir::ExceptionMatcher::Categories(kinds) = &matcher else {
                    return None;
                };
                Some((slot, target, kinds))
            });
            if caught.is_some() || !try_.finalbody.is_empty() {
                let writes = narrowing::assigned_names(&handler.body);
                if let Some((slot, target, kinds)) = caught
                    && !writes.contains(target.as_str())
                {
                    self.caught_exceptions.push(CaughtException {
                        slot,
                        builtin_class: Self::directly_raised_builtin(&body, kinds),
                        fields: processes::process_error_view(raises, &body, self.types, kinds),
                    });
                }
                if !try_.finalbody.is_empty() {
                    assigned.extend(writes);
                }
            }
            let (handler_body, falls) = self.block(&handler.body)?;
            self.caught_exceptions.truncate(capture_depth);
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
                matcher,
                name,
                body: handler_body,
            });
        }

        let has_normal_exit = normal_exit.is_some();
        let finish =
            |finalbody| statements::finish_try(body, handlers, orelse, has_normal_exit, finalbody);
        let (mut normal_assigned, normal_facts) = normal_exit
            .map(|exit| (exit.assigned, exit.narrowed))
            .unwrap_or_default();

        if try_.finalbody.is_empty() {
            self.assigned = normal_assigned;
            self.narrowed = normal_facts;
            self.explicit_return |= earlier_returns;
            return Ok(finish((Vec::new(), true)));
        }
        self.assigned = before;
        self.narrowed = before_facts;
        deleted.extend(narrowing::deleted_names(
            &try_.body[protected_prefix.len()..],
        ));
        assigned.extend(narrowing::assigned_names(&try_.orelse));
        deleted.extend(narrowing::deleted_names(&try_.orelse));
        for handler in &try_.handlers {
            let ast::ExceptHandler::ExceptHandler(handler) = handler;
            deleted.extend(narrowing::deleted_names(&handler.body));
            if let Some(name) = &handler.name
                && let Some(slot) = self.names.get(name.as_str())
            {
                self.assigned.remove(slot);
                self.narrowed.remove_mut(slot);
            }
        }
        for name in deleted {
            if let Some(slot) = self.names.get(&name) {
                self.assigned.remove(slot);
            }
        }
        for name in assigned {
            if let Some(slot) = self.names.get(&name) {
                self.narrowed.remove_mut(slot);
                self.assigned
                    .set_type_source(*slot, self.declared_slots.get(slot).copied());
            }
        }
        // Pending breaks must pass through finally. Its own transfers replace
        // them; successful completion adds its assignments and invalidates any
        // optional facts it may have changed, rather than recording an early
        // "broken" flag before the final completion is known.
        let pending_breaks = self.take_breaks();
        let protected_returns = std::mem::replace(&mut self.explicit_return, earlier_returns);
        let outer_finally = self.finally_scope;
        self.finally_scope = FinallyScope::Cleanup;
        let final_result = self.block(&try_.finalbody);
        self.finally_scope = outer_finally;
        let (finalbody, final_falls) = final_result?;
        self.explicit_return |= final_falls && protected_returns;
        let deleted = narrowing::deleted_names(&try_.finalbody);
        if final_falls && let Some(mut pending) = pending_breaks {
            let final_state = self.local_state();
            let written = narrowing::assigned_names(&try_.finalbody)
                .into_iter()
                .filter_map(|name| self.names.get(&name).copied())
                .collect::<HashSet<_>>();
            let deleted = deleted
                .iter()
                .filter_map(|name| self.names.get(name).copied())
                .collect::<HashSet<_>>();
            for slot in &deleted {
                pending.assigned.remove(slot);
            }
            for slot in written.iter().chain(&deleted) {
                pending.narrowed.remove_mut(slot);
                pending
                    .assigned
                    .set_type_source(*slot, self.declared_slots.get(slot).copied());
            }
            pending.assigned.extend(&final_state.assigned);
            for (&slot, &ty) in &final_state.narrowed {
                pending.narrowed.insert_mut(slot, ty);
            }
            self.include_breaks(Some(pending));
        }
        self.include_breaks(earlier_breaks);
        for name in deleted {
            if let Some(slot) = self.names.get(&name) {
                normal_assigned.remove(slot);
            }
        }
        // Keep the facts established by the shared finally across interrupted paths.
        self.assigned.extend(&normal_assigned);
        Ok(finish((finalbody, final_falls)))
    }

    fn lower_if_suite(
        &mut self,
        suite: &[ast::Stmt],
        output: Option<&mut Vec<Statement>>,
        imports: bool,
    ) -> Result<bool, String> {
        let outer = std::mem::replace(&mut self.suite_imports, imports);
        let result = self.lower_block(suite, output);
        self.suite_imports = outer;
        result
    }

    fn if_chain(
        &mut self,
        branch: &ast::StmtIf,
        mut arm: Option<&mut Vec<Statement>>,
    ) -> Result<bool, String> {
        let clauses = &branch.elif_else_clauses;
        let conditions =
            statements::if_clauses(branch).map_while(|(test, body)| Some((test?, body)));
        let mut remaining_imports = self.suite_imports;
        let mut else_field = None;
        let mut joined = None;
        let mut falls_at_cutoff = None;
        let mut exit_fixed = false;
        let mut last_test = branch.test.as_ref();
        for (index, (test, body)) in conditions.enumerate() {
            // A literal true takes the remaining suites' import paths.
            let imports = match test.as_boolean_literal_expr() {
                Some(literal) => literal.value && std::mem::take(&mut remaining_imports),
                None => remaining_imports,
            };
            last_test = test;
            self.branch_layouts(body, &clauses[index..])?;
            let condition = self.condition(test)?;
            if let Some(sites) = &mut self.fixed_sites {
                sites.branch_test(test, &condition);
            }
            let known = self.instance_condition(&condition);
            if self.graph.reachable_only()
                && let Some(taken) = known
                && let [other] = clauses.as_slice()
                && other.test.is_none()
                && let Some(names) = literal_binding_names(body)
                && literal_binding_names(&other.body) == Some(names)
            {
                // Both suites declare exactly these locals. The dead constants
                // cannot constrain the slots initialized by the reachable suite.
                self.apply_facts(test, taken);
                let selected = if taken { body } else { &other.body };
                // A type test can still evaluate a call or raise on its operand.
                if let Some(body) = arm.as_deref_mut() {
                    body.push(Statement::Expr(condition));
                }
                return self.lower_block(selected, arm);
            }
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
            let field_depth = self.present_fields.len();
            self.enter_namespace_guards(test, true, body);
            self.guard_return_field(body, records::present_field(&condition, true));
            // In a reachable-only profile, an impossible compatibility guard's
            // expression-only body contains no local declarations to preserve.
            if clauses
                .get(index)
                .is_some_and(|clause| clause.test.is_none())
            {
                else_field = records::present_field(&condition, false);
            }
            let yes = arm.take().map(|output| {
                let (yes, no) = statements::push_if(output, condition);
                arm = Some(no);
                yes
            });
            let yes = if self.graph.reachable_only()
                && known == Some(false)
                && body.iter().all(|stmt| matches!(stmt, ast::Stmt::Expr(s) if narrowing::expression_assignments(&s.value).is_empty()))
            {
                Ok(false)
            } else {
                self.lower_if_suite(body, yes, imports)
            };
            self.present_fields.truncate(field_depth);
            self.regex_guards.truncate(guard_depth);
            self.present_subscripts.truncate(subscript_depth);
            let yes_falls = yes?;
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
        }
        let no_falls = if let Some(tail) = clauses.last().filter(|c| c.test.is_none()) {
            let guards = self.present_fields.len();
            self.enter_namespace_guards(last_test, false, &tail.body);
            self.guard_return_field(&tail.body, else_field);
            let result = self.lower_if_suite(&tail.body, arm, remaining_imports);
            self.present_fields.truncate(guards);
            result?
        } else {
            true
        };
        if no_falls && !exit_fixed {
            self.merge_exit(&mut joined);
        }
        let falls = falls_at_cutoff.unwrap_or(joined.is_some());
        if let Some(joined) = joined {
            self.restore_state(joined);
        }
        Ok(falls)
    }

    /// A call or expression can supply its own concrete collection layout.
    fn rebindable_container(&self, ty: Type) -> bool {
        ty == Type::Bytes
            || self.types.element(ty).is_some()
            || self.types.set_element(ty).is_some()
    }

    /// Record fresh, unannotated function locals, including tuple-unpacking targets.
    fn remember_inferred_container(&mut self, slot: usize) {
        if !self.module_scope
            && self.block_depth == 1
            && self.local_functions.is_empty()
            && self.rebindable_container(self.locals[slot])
        {
            self.inferred_containers.insert(slot);
        }
    }

    /// Bind following source reads to the replacement slot.
    fn replacement_slot(&mut self, name: &str, old: usize, ty: Type) -> usize {
        let slot = self.hidden(ty);
        if self.inferred_containers.remove(&old) {
            self.inferred_containers.insert(slot);
        }
        self.names.insert(name.to_owned(), slot);
        slot
    }

    /// A straight-line rebind publishes the new slot before releasing the old owner.
    fn replace_local(&mut self, name: &str, old: usize, ty: Type, value: Expr) -> Vec<Statement> {
        let slot = self.replacement_slot(name, old, ty);
        self.assigned_value(slot, &value);
        self.assigned.remove(&old);
        self.narrowed.remove_mut(&old);
        vec![Statement::Assign(slot, value), Statement::Clear(old)]
    }

    fn assign(
        &mut self,
        target: &ast::Expr,
        value: Expr,
        output: Option<&mut Vec<Statement>>,
    ) -> Result<(), String> {
        let ast::Expr::Name(name) = target else {
            return Err(error(
                self.source,
                target,
                "local assignment requires a name",
            ));
        };
        let existing = self.names.get(name.id.as_str()).copied();
        let value = if let Some(slot) = existing {
            // A pulled object replaces the value in the same unannotated Python
            // binding. An empty loop leaves its previous value there.
            if value.ty == Type::PythonObject
                && self.locals[slot] != Type::PythonObject
                && !self.declared_slots.contains_key(&slot)
                && self.registered_local_start.map_or_else(
                    || self.graph.modules[self.module].is_program_source(),
                    // On a backedge earlier fixed-layout reads can execute again.
                    |first| slot >= first && self.block_depth == 1,
                )
            {
                if self.registered_local_start.is_none() {
                    self.require_registered_operation(target)?;
                }
                self.locals[slot] = Type::PythonObject;
            }
            self.coerce(value, self.locals[slot], target)?
        } else if let Some(partial) = self.types.partial_function(value.ty) {
            // Different partials can share a call signature without sharing a capture frame.
            self.coerce(value, partial.callable, target)?
        } else if let Some(&nullable) = self.nullable_assignments.get(name.id.as_str())
            && (value.ty == Type::None || self.types.payload(nullable) == Some(value.ty))
        {
            self.coerce(value, nullable, target)?
        } else if let Some(signature) = self.types.method_signature(value.ty) {
            self.coerce(value, signature, target)?
        } else {
            Self::inferred_stream(value)
        };
        let slot = self.declare_binding(name.id.as_str(), value.ty, target, existing.is_some())?;
        if existing.is_none() {
            self.remember_inferred_container(slot);
        }
        self.assigned_value(slot, &value);
        if let Some(output) = output {
            output.push(Statement::Assign(slot, value));
        }
        Ok(())
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
        let (left, right) = if matches!(
            binary.op,
            ast::Operator::BitOr | ast::Operator::BitAnd | ast::Operator::Sub
        ) {
            let (left, left_key) = self.union_operand(&binary.left)?;
            let (right, right_key) = self.union_operand(&binary.right)?;
            if let (Some(left_key), Some(right_key)) = (left_key, right_key) {
                if self.registered_local_start.is_some()
                    && self.types.sets_with_object_keys(left.ty, right.ty)
                {
                    return Ok(registered::values::binary(
                        binary,
                        Type::PythonObject,
                        left,
                        right,
                        None,
                    ));
                }
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
        // Scalar arithmetic keeps both operands in the checked operation. The
        // registered emitter guards their actual handles, including subclasses.
        // Power has a distinct fixed operation selected from the exponent.
        let scalar = binary.op != ast::Operator::Pow
            && matches!(left.ty, Type::Int | Type::Float)
            && matches!(right.ty, Type::Int | Type::Float);
        if self.registered.is_none() || scalar {
            return self.binary_operands(binary, left, right);
        }
        // Other fixed layouts can convert operands or select a builtin. Check the
        // contract and keep the original handles for the Python operation.
        let ty = if left.ty == Type::PythonObject || right.ty == Type::PythonObject {
            Type::PythonObject
        } else {
            self.binary_operands(binary, left.clone(), right.clone())?
                .ty
        };
        Ok(registered::values::binary(
            binary,
            ty,
            left,
            right,
            Some(self.graph.session.operand_types(self.module)),
        ))
    }

    fn binary_operands(
        &mut self,
        binary: &ast::ExprBinOp,
        mut left: Expr,
        mut right: Expr,
    ) -> Result<Expr, String> {
        if left.ty == Type::PythonObject || right.ty == Type::PythonObject {
            return Err(self.classified_error(
                Kind::ScopedObject,
                binary,
                "binary operation requires Python object dispatch",
            ));
        }
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
        if binary.op == ast::Operator::Mod
            && left.ty == Type::Calendar(ir::CalendarType::Delta)
            && right.ty == left.ty
        {
            return Err(self.object_runtime_export("timedelta remainder", binary));
        }
        if let (Type::Calendar(first), Type::Calendar(second)) = (left.ty, right.ty) {
            if binary.op == ast::Operator::Sub
                && first == ir::CalendarType::Date
                && second == ir::CalendarType::Date
            {
                return Err(self.object_runtime_export("date difference", binary));
            }
            return Ok(calendar::binary(
                first,
                second,
                left,
                right,
                binary,
                self.source,
            )?);
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
            let value = self.path_join(left, right, binary)?;
            return Ok(self.expression_at(binary.range(), value));
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
        }
        if self.types.is_counter(left.ty) && self.types.is_counter(right.ty) {
            return self.counter_math(binary.op, left, right, false, binary);
        }
        self.binary_expr(binary.op, left, right, binary)
    }

    fn binary_expr(
        &mut self,
        op: ast::Operator,
        left: Expr,
        right: Expr,
        node: &impl Ranged,
    ) -> Result<Expr, String> {
        if matches!(op, ast::Operator::Add | ast::Operator::Mult)
            && [left.ty, right.ty]
                .into_iter()
                .any(|ty| matches!(ty, Type::Class(id) if self.decimal_class(id)))
        {
            return Err(self.scoped_builtin_error("Decimal arithmetic", node));
        }
        if op == ast::Operator::Mod
            && let Some(formatted) = single_dispatch::format_argument(self.types, &left, &right)
        {
            return Ok(formatted);
        }
        if op == ast::Operator::Mod && left.ty == Type::Bytes {
            // The object runtime formats the bytes after the rest of the program type-checks.
            let error = self.scoped_builtin_error("bytes percent formatting", node);
            self.defer_object_error(&error);
            return Ok(Expr {
                ty: Type::Bytes,
                kind: ExprKind::Builtin(Builtin::PercentFormat, vec![left, right]),
            });
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
        let value = Expr {
            ty,
            kind: ExprKind::Binary(op, Box::new(left), Box::new(right)),
        };
        Ok(self.expression_at(node.range(), value))
    }

    fn binary_op(
        &self,
        op: ast::Operator,
        left: Type,
        right: Type,
        node: &impl Ranged,
    ) -> Result<(Binary, Type), String> {
        if op == ast::Operator::BitXor
            && [left, right]
                .into_iter()
                .any(|ty| matches!(ty, Type::Set(_) | Type::FrozenStrings))
        {
            return Err(self.object_builtin_error("set binary operator", node));
        }
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
            if matches!(left, Type::Deque(_)) || matches!(right, Type::Deque(_)) {
                return Err(self.object_builtin_error("collections.deque repetition", node));
            }
            if let Some(sequence) = Type::repeated_sequence(left, right) {
                return Ok((Binary::Multiply, sequence));
            }
        }
        if op == ast::Operator::Add {
            if matches!(left, Type::Deque(_)) {
                return Err(self.object_builtin_error("collections.deque concatenation", node));
            }
            if left == right
                && (matches!(left, Type::Str | Type::Bytes) || self.types.element(left).is_some())
            {
                return Ok((Binary::Add, left));
            }
            let optional_str = |ty| self.types.payload(ty) == Some(Type::Str);
            if (optional_str(left) || optional_str(right))
                && [left, right]
                    .into_iter()
                    .all(|ty| matches!(ty, Type::Str | Type::None) || optional_str(ty))
            {
                return Ok((Binary::Add, Type::Str));
            }
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

    fn tracks_called_bindings(&self) -> bool {
        self.module_scope
            && (self.globals.registered_initializers || self.module == self.graph.entry)
    }

    fn expr(&mut self, expression: &ast::Expr) -> Result<Expr, String> {
        // Resolve a closed callee before evaluating arguments that can replace it.
        let python_call = self.tracks_called_bindings()
            && expression
                .as_call_expr()
                .is_some_and(|call| !self.closed_passive_call(call));
        let checked = self.typed_expr(expression);
        if python_call {
            self.invalidate_called_bindings(expression, &checked);
        }
        let value = checked?;
        Ok(if matches!(value.ty, Type::StrLiteral(_)) {
            literals::view(value, Type::Str)
        } else {
            value
        })
    }

    fn object_runtime_export(&self, name: &str, node: &impl Ranged) -> Failure {
        self.graph.needs_object_runtime.set(true);
        self.object_export_error(name, node)
    }

    fn object_builtin_error(&self, name: &str, node: &impl Ranged) -> Failure {
        self.graph.needs_object_runtime.set(true);
        self.scoped_builtin_error(name, node)
    }

    fn boxed_builtin_callable(&self, expression: &ast::Expr) -> Option<&'static str> {
        ["bin", "exec", "hex", "oct"]
            .into_iter()
            .find(|name| self.unshadowed_builtin(expression, name))
    }

    fn object_builtin_error_with_checked_calls(&self, name: &str, node: &impl Ranged) -> Failure {
        self.graph
            .check_fallback_calls
            .set(crate::analysis::FallbackCalls::FunctionBodies);
        self.object_builtin_error(name, node)
    }

    /// Checks the logarithm domain once before lowering the unboxed operation.
    fn checked_float_log2(&mut self, value: Expr) -> Expr {
        let slot = self.hidden(Type::Float);
        let input = self.local(slot);
        let invalid = pipelines::compare(
            Compare::LessEqual,
            input.clone(),
            Expr {
                ty: Type::Float,
                kind: ExprKind::Float(0.0),
            },
        );
        let prefix = Expr {
            ty: Type::Str,
            kind: ExprKind::Str("expected a positive input, got ".to_string()),
        };
        let spelling = Expr {
            ty: Type::Str,
            kind: ExprKind::Builtin(Builtin::Str, vec![input.clone()]),
        };
        let message = Expr {
            ty: Type::Str,
            kind: ExprKind::Binary(Binary::Add, Box::new(prefix), Box::new(spelling)),
        };
        let error = Expr {
            ty: Type::Exception,
            kind: ExprKind::Exception {
                kind: exception_kind("ValueError").unwrap(),
                message: Some(Box::new(message)),
            },
        };
        Expr {
            ty: Type::Float,
            kind: ExprKind::Scope {
                body: vec![
                    Statement::Assign(slot, value),
                    Statement::If(invalid, vec![Statement::raise(error)], vec![]),
                ],
                value: Box::new(Expr {
                    ty: Type::Float,
                    kind: ExprKind::Builtin(Builtin::FloatLog2, vec![input]),
                }),
                clear: vec![slot],
            },
        }
    }

    fn unsupported_comparison(&self, expression: &ast::Expr, left: Type, right: Type) -> Failure {
        if left == Type::PythonObject || right == Type::PythonObject {
            // The checked Python operation owns comparisons without a fixed operand layout.
            self.scoped_builtin_error("comparing Python objects", expression)
        } else {
            error(
                self.source,
                expression,
                "unsupported comparison for these types",
            )
        }
    }

    /// Select the object provider only when this precise diagnostic reaches the driver.
    fn scoped_builtin_error(&self, name: &str, node: &impl Ranged) -> Failure {
        self.object_export_error(name, node)
            .classify(Kind::ScopedObject)
    }

    fn object_export_error(&self, name: &str, node: &impl Ranged) -> Failure {
        self.classified_error(
            Kind::Object,
            node,
            &format!("{name} requires native Python object operations"),
        )
    }

    fn boxed_descriptor(&self, expression: &ast::Expr) -> Option<&'static str> {
        use ty_python_semantic::types::KnownClass;

        if let ast::Expr::Attribute(attribute) = expression
            && attribute.attr.as_str() == "__hash__"
            && self
                .graph
                .session
                .is_known_class(self.module, &attribute.value, KnownClass::Str)
        {
            return Some("str.__hash__");
        }
        if let ast::Expr::Attribute(attribute) = expression
            && let Some((method, class, ty)) = match attribute.attr.as_str() {
                "isprintable" => Some(("str.isprintable", KnownClass::Str, Type::Str)),
                "partition" => Some(("bytes.partition", KnownClass::Bytes, Type::Bytes)),
                "rjust" => Some(("bytes.rjust", KnownClass::Bytes, Type::Bytes)),
                _ => None,
            }
            && (self
                .graph
                .session
                .is_known_class(self.module, &attribute.value, class)
                || self
                    .graph
                    .session
                    .scalar_type(self.module, &attribute.value)
                    == Some(ty))
        {
            return Some(method);
        }
        self.graph.boxed_pattern_method(self.module, expression)
    }

    fn typed_expr(&mut self, expression: &ast::Expr) -> Result<Expr, String> {
        self.with_expression_site(expression, |checker| {
            match checker.lower_expression(expression) {
                Err(failure) if checker.missing_operation(&failure) => Ok(Expr::unrepresented()),
                result => result,
            }
        })
    }

    fn with_expression_site(
        &mut self,
        expression: &ast::Expr,
        lower: impl FnOnce(&mut Self) -> Result<Expr, String>,
    ) -> Result<Expr, String> {
        self.with_fixed_site(expression.range(), |checker| {
            let value = lower(checker)?;
            if let Some(sites) = &mut checker.fixed_sites
                && matches!(
                    (expression, &value.kind),
                    (ast::Expr::StringLiteral(_), ExprKind::Str(_))
                        | (ast::Expr::BytesLiteral(_), ExprKind::Bytes(_))
                        | (ast::Expr::NumberLiteral(_), ExprKind::Float(_))
                        | (ast::Expr::BooleanLiteral(_), ExprKind::Bool(_))
                        | (ast::Expr::NoneLiteral(_), ExprKind::None)
                        | (ast::Expr::List(_), ExprKind::List(_))
                        | (ast::Expr::Tuple(_), ExprKind::Tuple(_))
                        | (ast::Expr::Named(_), ExprKind::Bind(_, _))
                        | (
                            ast::Expr::FString(_),
                            ExprKind::Builtin(Builtin::FormatJoin, _)
                        )
                )
            {
                sites.checked_expression(expression.range(), &value);
            }
            Ok(value)
        })
    }

    /// Keep the written range on the checked expression that produced it.
    fn expression_at(&mut self, range: TextRange, value: Expr) -> Expr {
        if let Some(sites) = &mut self.fixed_sites {
            sites.checked_expression(range, &value);
        }
        value
    }

    /// Record the operation that an ordinary source operand produced for this statement.
    fn statement_at(&mut self, operand: &ast::Expr, statement: Statement) -> Statement {
        if let Some(sites) = &mut self.fixed_sites {
            sites.statement(operand.range(), &statement);
        }
        statement
    }

    fn with_fixed_site<T>(
        &mut self,
        range: TextRange,
        lower: impl FnOnce(&mut Self) -> Result<T, String>,
    ) -> Result<T, String> {
        let parent = self
            .fixed_sites
            .as_mut()
            .and_then(|sites| sites.enter(range));
        let value = lower(self)?;
        self.leave_fixed_site(parent);
        Ok(value)
    }

    fn leave_fixed_site(&mut self, parent: Option<fixed_body_source::PendingSite>) {
        if let Some(sites) = &mut self.fixed_sites
            && !sites.leave(parent)
            && let Some(partial) = self.fixed_sites.take()
            && let Some(saved) = &mut self.settled_attributes
        {
            **saved = partial.attributes;
        }
    }

    fn lower_expression(&mut self, expression: &ast::Expr) -> Result<Expr, String> {
        if let Some(value) = self.live_module_binding(expression) {
            return Ok(value);
        }
        if matches!(
            expression,
            ast::Expr::ListComp(_) | ast::Expr::SetComp(_) | ast::Expr::DictComp(_)
        ) {
            let value = self.comprehension(expression, None)?;
            return Ok(if let Some(module) = self.registered {
                module
                    .operands(self.source, self.graph.python_optimization())
                    .located(expression, value)
            } else {
                value
            });
        }
        if let Some(literals) = self.registered {
            if let Some(value) = self.registered_operand(expression, literals)? {
                return Ok(value);
            }
            // The public registered operand selector owns the receiver and call proof.
            // Continue checking the source even when this activation cannot emit the operation.
            self.require_registered_operation(expression)?;
        }
        self.fixed_expr(expression)
    }

    fn can_mix_list_items(&self, left: Type, right: Type) -> bool {
        !self.module_scope
            && matches!(left, Type::Int | Type::Str)
            && matches!(right, Type::Int | Type::Str)
    }

    /// Check the current operation with its ordinary layout rules. Children still use their
    /// selected calling convention, including registered functions and Python values.
    fn fixed_expr(&mut self, expression: &ast::Expr) -> Result<Expr, String> {
        if let Some(method) = self.boxed_descriptor(expression) {
            return Err(self.scoped_builtin_error(method, expression));
        }
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
            ast::Expr::Call(call) => {
                let mut result = self.call(call)?;
                if let ExprKind::Call(_, args) | ExprKind::Invoke { args, .. } = &mut result.kind {
                    // A conversion can return an unchanged, already positioned operand.
                    let origin = match args.site {
                        ir::CallSite::Written(range, end) => {
                            self.fixed_origin(range, Some((end, b'(')))
                        }
                        ir::CallSite::None => self.call_origin(call),
                        _ => return Ok(result),
                    };
                    args.site = ir::CallSite::Fixed(origin);
                }
                Ok(result)
            }
            ast::Expr::BinOp(binary) => self.binary_operation(binary),
            _ => self.non_call_expr(expression),
        }
    }

    /// Keep the large general frame off recursive left- and right-hand arithmetic.
    fn non_call_expr(&mut self, expression: &ast::Expr) -> Result<Expr, String> {
        if let Some(name) = self.boxed_builtin_callable(expression) {
            return Err(self.scoped_builtin_error(name, expression));
        }
        if self.unshadowed_len(expression) {
            return Ok(self.builtin_len());
        }
        let binding = self.resolve(expression);
        if binding == Some(Binding::Intrinsic(Intrinsic::AttrGetter)) {
            return Err(self.runtime_attrgetter(expression));
        }
        if binding == Some(Binding::Intrinsic(Intrinsic::PopenActive)) {
            return Ok(Expr {
                ty: self.types.list(Type::ActivePopen),
                kind: ExprKind::Builtin(
                    Builtin::process(ir::ProcessOperation::PopenActive),
                    Vec::new(),
                ),
            });
        }
        if typing::is_dummy(self.graph, binding) {
            return Ok(typing::dummy());
        }

        if let Some(name) = Self::collection_class_name(binding)
            && !self.local_collection_root(expression)
        {
            return Ok(annotations::class_value(name));
        }
        let base_binding = expression
            .as_attribute_expr()
            .and_then(|attr| self.resolve(&attr.value));
        if let ast::Expr::Attribute(attr) = expression
            && let Some(Binding::Intrinsic(Intrinsic::AnnotationFormat(number))) = base_binding
        {
            match attr.attr.as_str() {
                "value" => return Ok(Expr::integer(number)),
                "name" => {
                    return Ok(unpacking::string(
                        ["VALUE", "VALUE_WITH_FAKE_GLOBALS", "FORWARDREF", "STRING"]
                            [(number - 1) as usize],
                    ));
                }
                _ => {}
            }
        }
        let selector = match binding {
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
                    Builtin::process(ir::ProcessOperation::SelectorClass(mode)),
                    Vec::new(),
                ),
            });
        }
        if let Some(Binding::Intrinsic(
            Intrinsic::Declaration(symbol) | Intrinsic::RuntimeImport(symbol),
        )) = binding
        {
            return Err(error(
                self.source,
                expression,
                &format!("no native implementation for {symbol}"),
            ));
        }
        if let ast::Expr::Attribute(attr) = expression
            && let Some(value) = self.calendar_class_attribute(attr, base_binding)?
        {
            return Ok(value);
        }
        if let Some(target) = binding {
            // Calls to the sealed compile intrinsic have their own checked operands.
            // Taking the callable, flag object, or module itself requires its live namespace.
            if matches!(
                target,
                Binding::IntrinsicModule(crate::modules::IntrinsicModule::Re)
                    | Binding::Intrinsic(Intrinsic::RegexCompile | Intrinsic::RegexMultiline)
            ) {
                return Err(self.scoped_builtin_error("the re module binding", expression));
            }
            if target == Binding::Intrinsic(Intrinsic::CalendarUtc) {
                return Ok(calendar::utc());
            }
            if matches!(target, Binding::Module(_) | Binding::IntrinsicModule(_))
                && let Some(module) = self.module_docs(target, expression)?
            {
                return Ok(module);
            }
            if let Binding::Module(id) = target {
                if self.graph.modules[id].is_namespace() {
                    return Err(self.object_runtime_export("a namespace package value", expression));
                }
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
        if let Some(Binding::Intrinsic(Intrinsic::SocketConstant(name))) = binding {
            return Ok(socket::constant(name));
        }
        if let Some(Binding::Intrinsic(
            Intrinsic::LogLevel(level)
            | Intrinsic::IntConstant(level)
            | Intrinsic::AnnotationFormat(level),
        )) = binding
        {
            return Ok(Expr::integer(level));
        }
        if let Some(field) = expression.as_attribute_expr()
            && let Some(prefix) = cache_paths::config(field, binding, || base_binding)
        {
            return Ok(cache_paths::op(
                ir::CachePathOperation::Get(prefix),
                self.types.optional(Type::Str),
                vec![],
            ));
        }
        self.reject_class_receiver_value(expression)?;
        if let Some(name) = expression.as_name_expr()
            && let Some(binding) = self.local_functions.get(name.id.as_str())
        {
            if let Some(slot) = binding.value_slot {
                return Ok(self.local(slot));
            }
            if binding.is_bound() {
                // A value read needs the callable created by this execution of def,
                // including its particular closure and its Python identity.
                let failure = self.scoped_builtin_error("a local function value", expression);
                if self.missing_operation(&failure) {
                    return Ok(registered::values::name_binding(name.id.clone()));
                }
                return Err(failure);
            }
            return Err(error(
                self.source,
                expression,
                "local functions require a direct call or a noncapturing map/filter callback",
            ));
        }
        if let Some(Binding::Definition(definition)) = binding
            && let Some(signature) = self.functions.get(&definition)
        {
            // A registered definition lives in its Python namespace. Its value can be
            // stored in a container without inventing a fixed callable signature.
            if signature.registered_frame() {
                if let ast::Expr::Name(name) = expression {
                    self.globals
                        .missing_bindings
                        .borrow_mut()
                        .remove(&signature.id);
                    return Ok(registered::values::name_binding(name.id.clone()));
                }
                return Err(self.classified_error(
                    Kind::ScopedObject,
                    expression,
                    "function lookup requires its Python namespace",
                ));
            }
            return self.callable_value(signature, None, None, expression);
        }
        if let Some(Binding::Intrinsic(
            op @ (Intrinsic::Copy(_) | Intrinsic::RmTree | Intrinsic::Move | Intrinsic::DestInSrc),
        )) = binding
        {
            return self.shutil_function_value(op, None, expression);
        }
        match binding {
            Some(Binding::Value(definition)) => {
                let value = self.read_module_value(definition, None, expression)?;
                if let Some(sites) = &mut self.fixed_sites
                    && definition.module == self.module
                    && let ast::Expr::Name(name) = expression
                    && (self.class.is_none()
                        || !ruff_python_stdlib::identifiers::is_mangled_private(&name.id))
                {
                    sites.module_read(name, &value);
                }
                return Ok(value);
            }
            Some(Binding::ImportedValue(definition, member)) => {
                return self.read_module_value(definition, Some(member), expression);
            }
            _ => {}
        }
        if let ast::Expr::Attribute(attr) = expression
            && attr.attr.as_str() == "avoids_symlink_attacks"
            && base_binding == Some(Binding::Intrinsic(Intrinsic::RmTree))
        {
            return Ok(Expr::boolean(true));
        }
        if let Some(Binding::Intrinsic(Intrinsic::AccessMode(mode))) = binding {
            return Ok(Expr::integer(mode));
        }
        if let Some(Binding::Intrinsic(symbol @ (Intrinsic::Argv | Intrinsic::OrigArgv))) = binding
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
        if binding == Some(Binding::Intrinsic(Intrinsic::DefaultDict))
            && self.has_static_namespace_root(expression)
        {
            let source = self
                .registered
                .unwrap_or_else(|| self.graph.modules[self.module].registered_values())
                .operands(self.source, self.graph.python_optimization());
            if let Some(value) = registered::values::registered_value(expression, source) {
                self.require_registered_operation(expression)?;
                return Ok(value);
            }
        }
        if binding.is_some_and(Binding::is_registered_object) {
            self.require_registered_operation(expression)?;
            return Ok(Expr::unrepresented());
        }
        if binding == Some(Binding::RuntimeClass) {
            return Err(self.object_export_error("a class defined in a module guard", expression));
        }
        if binding == Some(Binding::ConditionalValue) {
            let operation = "a value selected by a module guard";
            if expression.as_name_expr().is_some_and(|name| {
                self.graph.modules[self.module]
                    .optional_imports
                    .values()
                    .flatten()
                    .any(|alias| alias == name.id.as_str())
            }) {
                return Err(self.object_export_error(operation, expression));
            }
            return Err(self.object_runtime_export(operation, expression));
        }
        if let Some(Binding::Intrinsic(symbol)) = binding {
            if let Some((flags, executable)) = &self.graph.command_startup {
                match symbol {
                    Intrinsic::SysExecutable => return Ok(unpacking::string(executable.clone())),
                    Intrinsic::SysDontWriteBytecode => {
                        return Ok(Expr::boolean(flags.dont_write_bytecode));
                    }
                    _ => {}
                }
            }
            match symbol {
                Intrinsic::GetEnv if cfg!(unix) && self.graph.command_startup.is_some() => {
                    return Err(self.scoped_builtin_error("os.getenv", expression));
                }
                Intrinsic::UrlSplit => {
                    return Err(self.scoped_builtin_error("urllib.parse.urlsplit", expression));
                }
                Intrinsic::CodecGetWriter => {
                    return Err(self.object_builtin_error("codecs.getwriter", expression));
                }
                Intrinsic::Popen | Intrinsic::CompletedProcess if cfg!(unix) => {
                    return Err(self.scoped_builtin_error("subprocess process class", expression));
                }
                Intrinsic::SplitResult { .. } => {
                    return Err(self.scoped_builtin_error("urllib.parse result class", expression));
                }
                Intrinsic::Chdir => {
                    return Err(self.scoped_builtin_error("posix.chdir", expression));
                }
                Intrinsic::TracebackFormatter(name) | Intrinsic::ObjectOnlyExport(name) => {
                    return Err(self.object_export_error(name, expression));
                }
                // Calls and specialized class attributes have already been handled.
                // A first-class datetime type needs its actual Python object.
                Intrinsic::Calendar(_) | Intrinsic::ClockTime
                    if ast::name::UnqualifiedName::from_expr(expression)
                        .is_some_and(|name| !self.names.contains_key(name.segments()[0])) =>
                {
                    return Err(self.scoped_builtin_error("datetime class value", expression));
                }
                Intrinsic::Callable => {
                    return Err(self.scoped_builtin_error("the Callable object", expression));
                }
                Intrinsic::NamedHash(
                    "blake2b" | "blake2s" | "sha3_224" | "sha3_256" | "sha3_384" | "sha3_512"
                    | "shake_128" | "shake_256",
                ) => {
                    self.graph.needs_object_runtime.set(true);
                    return Err(self.classified_error(
                        Kind::Object,
                        expression,
                        "a hash constructor used as a type requires native object dispatch",
                    ));
                }
                Intrinsic::CachedProperty => {
                    return Err(self.object_runtime_export("functools.cached_property", expression));
                }
                Intrinsic::Base64Decode => {
                    return Err(self.scoped_builtin_error("base64.b64decode", expression));
                }
                Intrinsic::MathPerm => {
                    return Err(error(
                        self.source,
                        expression,
                        "checked math.perm requires a direct call",
                    ));
                }
                Intrinsic::ScopedObjectExport("math.sumprod") => {
                    return Err(error(
                        self.source,
                        expression,
                        "checked math.sumprod requires a direct call",
                    ));
                }
                Intrinsic::ScopedObjectExport("operator.is_" | "operator.is_not") => {
                    return Err(error(
                        self.source,
                        expression,
                        "checked operator identity functions require a direct call",
                    ));
                }
                Intrinsic::ScopedObjectExport(name) => {
                    return Err(self.scoped_builtin_error(name, expression));
                }
                Intrinsic::Fd(ir::FdOperation::TcGetPgrp) => {
                    return Err(self.object_runtime_export("posix.tcgetpgrp", expression));
                }
                Intrinsic::Fd(ir::FdOperation::TcSetPgrp) => {
                    return Err(self.object_runtime_export("posix.tcsetpgrp", expression));
                }
                Intrinsic::ObjectRuntimeExport(name) => {
                    return Err(self.object_runtime_export(name, expression));
                }
                Intrinsic::RegisteredImport(name) => {
                    return Err(self.object_runtime_export(name, expression));
                }
                Intrinsic::Fnmatch(ir::FnmatchOperation::Match { normcase }) => {
                    return Ok(fnmatch::function(self.types, normcase));
                }
                Intrinsic::OperatorCall => {
                    return Err(error(
                        self.source,
                        expression,
                        "checked operator.call requires a direct call",
                    ));
                }
                Intrinsic::OperatorNot => {
                    return Err(error(
                        self.source,
                        expression,
                        "checked operator.not_ requires a direct call",
                    ));
                }
                Intrinsic::BuiltinsHex => {
                    return Err(self.scoped_builtin_error("builtins.hex", expression));
                }
                Intrinsic::BuiltinsLen => return Ok(self.builtin_len()),
                Intrinsic::CodecBom(bytes) => {
                    return Ok(Expr {
                        ty: Type::Bytes,
                        kind: ExprKind::Bytes(bytes.to_vec()),
                    });
                }
                Intrinsic::CodecNativeBom(little, big) => {
                    let bytes = |mark: &[u8]| {
                        Box::new(Expr {
                            ty: Type::Bytes,
                            kind: ExprKind::Bytes(mark.to_vec()),
                        })
                    };
                    let test = Expr {
                        ty: Type::Bool,
                        kind: ExprKind::Builtin(Builtin::TargetLittleEndian, vec![]),
                    };
                    return Ok(Expr {
                        ty: Type::Bytes,
                        kind: ExprKind::Conditional(Box::new(test), bytes(little), bytes(big)),
                    });
                }
                // Specialized lookups and mutations are handled at their call sites.
                // When the mapping itself escapes, preserve its identity and live views.
                Intrinsic::Environ | Intrinsic::EnvironBytes => {
                    return Err(self.scoped_builtin_error("environment mapping", expression));
                }
                Intrinsic::Stdin => return Err(self.object_runtime_export("sys.stdin", expression)),
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
                            Builtin::process(ir::ProcessOperation::DescriptorConstant(name)),
                            vec![],
                        ),
                    });
                }
                Intrinsic::HashAlgorithms { guaranteed } => {
                    return Ok(Expr {
                        ty: self.types.set(Type::Str),
                        kind: ExprKind::Builtin(Builtin::HashAlgorithms { guaranteed }, vec![]),
                    });
                }
                Intrinsic::Linecache(ir::LinecacheOperation::Cache) => {
                    return Ok(self.linecache_mapping());
                }
                Intrinsic::InspectSentinel => return Ok(inspect::sentinel()),
                Intrinsic::Placeholder(kind) => return Ok(partials::singleton(kind)),
                Intrinsic::WrapperAttributes(fields) => {
                    return Ok(functools::wrapper_attributes(fields, self.types));
                }
                Intrinsic::OsPathsep { posix: true } => return Ok(unpacking::string(":")),
                Intrinsic::OsSeparator { alternate, posix } => {
                    if posix {
                        return Ok(if alternate {
                            Expr::none()
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
                config @ (Intrinsic::StartupFilesystemRoots | Intrinsic::ModuleSuffixes) => {
                    let values: Vec<Expr> = if config == Intrinsic::ModuleSuffixes {
                        imre_ast::module_suffixes()
                            .into_iter()
                            .map(unpacking::string)
                            .collect()
                    } else {
                        self.graph
                            .filesystem_roots
                            .iter()
                            .map(|root| {
                                let path = root.to_str().ok_or_else(|| {
                                    error(self.source, expression, "filesystem roots must be valid UTF-8 for checked discovery")
                                })?;
                                Ok(unpacking::string(path))
                            })
                            .collect::<Result<_, String>>()?
                    };
                    let tuple = Expr {
                        ty: self.types.tuple(vec![Type::Str; values.len()]),
                        kind: ExprKind::Tuple(values),
                    };
                    return self.coerce(tuple, self.types.var_tuple(Type::Str), expression);
                }
                Intrinsic::StringConstant(value) => return Ok(unpacking::string(value)),
                Intrinsic::BoolConstant(value) => return Ok(Expr::boolean(value)),
                Intrinsic::LogRecordFlag(mask) => {
                    return Ok(logging_records::op(
                        ir::LogRecordOperation::Flag,
                        Type::Bool,
                        vec![Expr::integer(mask)],
                    ));
                }
                Intrinsic::LogRoot => return Ok(self.root_logger()),
                Intrinsic::LevelMap { names } => return Ok(self.logging_level_map(names)),
                Intrinsic::SysBuiltinModuleNames if self.has_static_namespace_root(expression) => {
                    return Ok(Expr {
                        ty: self.types.var_tuple(Type::Str),
                        kind: ExprKind::Builtin(Builtin::SysBuiltinModuleNames, vec![]),
                    });
                }
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
                Intrinsic::SysMaxSize => {
                    return Ok(Expr {
                        ty: Type::Int,
                        kind: ExprKind::Builtin(Builtin::SysMaxSize, Vec::new()),
                    });
                }
                Intrinsic::SysVersionInfo => return Ok(self.sys_version_info(false)),
                Intrinsic::SysThreadInfo => return Ok(self.sys_thread_info()),
                Intrinsic::SysHexversion => {
                    return Ok(Expr::integer(crate::version::hex(
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
                Intrinsic::SysExecutable
                | Intrinsic::SysPlatform
                | Intrinsic::OsName
                | Intrinsic::OsLinesep
                | Intrinsic::OsPathsep { posix: false } => {
                    let operation = match symbol {
                        Intrinsic::SysExecutable => {
                            Builtin::process(ir::ProcessOperation::Executable)
                        }
                        Intrinsic::OsName => Builtin::OsName,
                        Intrinsic::OsLinesep => Builtin::OsLinesep,
                        Intrinsic::OsPathsep { .. } => Builtin::OsPathsep,
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
                ast::Number::Int(value) => {
                    if let Some(sites) = &mut self.fixed_sites
                        && let Some(integer) = value.as_i64()
                    {
                        sites.record(expression.range(), FixedNode::Integer(integer));
                    }
                    (Type::Int, ExprKind::Int(value.to_string()))
                }
                ast::Number::Float(value) => (Type::Float, ExprKind::Float(*value)),
                _ if self.graph.command_startup.is_some() => {
                    return Err(self.scoped_builtin_error("complex literal", expression));
                }
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
            ast::Expr::TString(_) => {
                return Err(self.object_builtin_error("template literal", expression));
            }
            ast::Expr::Lambda(lambda) => return self.lambda_value(lambda, None, None),
            ast::Expr::Named(value) => return self.named_expression(value, None),
            ast::Expr::If(value) => return self.conditional(value, None),
            ast::Expr::NoneLiteral(_) => (Type::None, ExprKind::None),
            ast::Expr::Name(name)
                if let Some(value) = debug_operand(name, self.graph.python_optimization()) =>
            {
                (Type::Bool, value)
            }
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
                let Some(&slot) = self.names.get(name.id.as_str()) else {
                    let name = name.id.as_str();
                    let binding = self.resolve(expression);
                    if binding.is_none() {
                        if self
                            .graph
                            .session
                            .operand_types(self.module)
                            .reads_deferred_global(expression)
                        {
                            self.require_registered_operation(expression)?;
                            return Ok(registered::values::name_binding(name));
                        }
                        if let Some(value) = annotations::builtin_type(name) {
                            return Ok(value);
                        }
                    }
                    if self.comprehension_targets.contains(name) {
                        return Err(error(
                            self.source,
                            expression,
                            "comprehension local may be uninitialized on this path",
                        ));
                    }
                    if (imre_ast::providers::BUILTIN_FUNCTIONS.contains(&name)
                        || imre_ast::exceptions::boxed_builtin_kind(name).is_some())
                        && binding.is_none()
                        && self.local_function_binding(expression).is_none()
                    {
                        self.require_builtin_scope(name, expression);
                        return Ok(registered::values::name_binding(name));
                    }
                    if matches!(
                        binding,
                        Some(Binding::IntrinsicModule(
                            crate::modules::IntrinsicModule::Builtins
                        ))
                    ) {
                        return Err(
                            self.scoped_builtin_error("the builtin module as a value", expression)
                        );
                    }
                    if self.inference.is_some()
                        && self.graph.modules[self.module].is_program_source()
                        && matches!(binding, Some(Binding::IntrinsicModule(_)))
                    {
                        self.require_registered_operation(expression)?;
                        return Ok(registered::values::name_binding(name));
                    }
                    if let Some(Binding::Definition(_) | Binding::TypeAlias(..)) = binding {
                        return Err(self.classified_error(
                            Kind::ScopedObject,
                            expression,
                            "using this definition as a value requires object dispatch",
                        ));
                    }
                    return Err(error(
                        self.source,
                        expression,
                        "name is not a local value; globals and first-class functions are unsupported",
                    ));
                };
                if self.reachable && !self.assigned.contains(&slot) {
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
                let value = self.read_local(slot);
                if let Some(sites) = &mut self.fixed_sites
                    && matches!(value.kind, ExprKind::Local(read) if read == slot)
                    && value.ty == self.locals[slot]
                {
                    sites.checked_expression(expression.range(), &value);
                }
                return Ok(value);
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
                    if self.can_mix_list_items(element, item.ty) {
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
                        .collect::<StdResult<_, _>>()?;
                }
                (self.types.list(element), ExprKind::List(values))
            }
            ast::Expr::Dict(dict) => {
                if dict
                    .items
                    .iter()
                    .any(|item| item.key.is_none() && self.probe_document(&item.value))
                {
                    return Ok(self.document_literal(expression, documents::MAP)?.unwrap());
                }
                return self.dictionary(dict, None, None);
            }
            ast::Expr::Set(set) => return self.set_literal(set, None, None),
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
                    .collect::<StdResult<Vec<_>, _>>()?;
                let values = if values
                    .iter()
                    .all(|value| matches!(value.ty, Type::Document(_)))
                    && values.windows(2).any(|v| v[0].ty != v[1].ty)
                {
                    values
                        .into_iter()
                        .map(|v| self.document_coerce(v, documents::ANY, tuple))
                        .collect::<StdResult<Vec<_>, _>>()?
                } else {
                    values
                };
                let ty = self
                    .types
                    .tuple(values.iter().map(|value| value.ty).collect());
                (ty, ExprKind::Tuple(values))
            }
            ast::Expr::UnaryOp(unary) => {
                if let Some(literal) = literals::signed_number(expression, |ty| ty == Type::Float) {
                    return Ok(self.expression_at(expression.range(), literal));
                }
                let value = if unary.op == ast::UnaryOp::Not {
                    self.condition_at(&unary.operand, None)?
                } else {
                    self.expr(&unary.operand)?
                };
                let ty = match unary.op {
                    ast::UnaryOp::Not => Type::Bool,
                    ast::UnaryOp::Invert => {
                        self.expect(Type::Int, value.ty, expression)?;
                        Type::Int
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
                        value.ty
                    }
                };
                return Ok(Expr::unary(unary.op, value, ty));
            }
            ast::Expr::BoolOp(boolean) => {
                return self.boolean_value(boolean, None);
            }
            ast::Expr::Compare(compare) => {
                if let Some((left, op, right)) = self.identity_test(expression) {
                    return self.identity_comparison(left, right, op, expression);
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
                            Expr::not(found)
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
                            Expr::not(found)
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
                    if self.types.var_tuple_element(receiver.ty) == Some(Type::Str)
                        && matches!(key.ty, Type::Int | Type::None | Type::Bool | Type::Float)
                    {
                        let result = Expr::boolean(compare.ops[0] == ast::CmpOp::NotIn);
                        let result = self.evaluated_receiver(Some(receiver), result);
                        return Ok(self.evaluated_receiver(Some(key), result));
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
                    } else if self.types.var_tuple_element(receiver.ty) == Some(Type::Str) {
                        (Type::Str, Builtin::StringTupleContains)
                    } else if self.types.element(receiver.ty) == Some(Type::Str) {
                        (Type::Str, Builtin::StringListContains)
                    } else if receiver.ty == Type::PythonObject {
                        return Err(self.scoped_builtin_error("membership", expression));
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
                        result = Expr::not(result);
                    }
                    return Ok(result);
                }
                let mut values = Vec::new();
                let mut exit = None;
                for (index, value) in compare.operands.iter().enumerate() {
                    values.push(self.expr(value)?);
                    if compare.ops.len() > 1 && index != 0 {
                        self.merge_exit(&mut exit);
                    }
                }
                if let Some(exit) = exit {
                    self.restore_state(exit);
                }
                if let Some(first) = self.registered_local_start
                    && let [op] = compare.ops.as_ref()
                    && ir::Compare::from_operator(*op).is_some()
                    && let [left, right] = values.as_slice()
                {
                    let operand_types = self.graph.session.operand_types(self.module);
                    let python_comparison = match op {
                        ast::CmpOp::Eq | ast::CmpOp::NotEq => {
                            left.ty == Type::PythonObject
                                || right.ty == Type::PythonObject
                                || self.types.sets_with_object_keys(left.ty, right.ty)
                        }
                        // A Python handle can still have a checked scalar type at this
                        // source operand. Keep the original objects for rich comparison.
                        _ => match (left.ty, right.ty) {
                            (Type::PythonObject, Type::PythonObject) => true,
                            (Type::PythonObject, ty) => {
                                operand_types.scalar_type(&compare.operands[0]) == Some(ty)
                            }
                            (ty, Type::PythonObject) => {
                                operand_types.scalar_type(&compare.operands[1]) == Some(ty)
                            }
                            _ => false,
                        },
                    };
                    if python_comparison {
                        return Ok(Expr {
                            ty: Type::PythonObject,
                            kind: ExprKind::Compare(values, vec![*op]),
                        });
                    }
                    if !values.iter().any(|value| value.ty.builtin_scalar_name().is_some()
                        && matches!(value.without_location().kind, ExprKind::Local(slot) if slot < first))
                    {
                        self.require_registered_operation(expression)?;
                    }
                }
                // A registered frame compares its caller's original scalar argument. Keep
                // the ordinary operand checks before choosing the Python result convention.
                let live_scalar_operands = self.registered_local_start.is_some()
                    && values
                        .iter()
                        .all(|value| value.ty.builtin_scalar_name().is_some());
                if matches!(compare.ops.as_ref(), [ast::CmpOp::Eq | ast::CmpOp::NotEq])
                    && values
                        .iter()
                        .all(|value| self.types.element(value.ty).is_some())
                    && values.iter().any(
                        |value| matches!(&value.kind, ExprKind::List(items) if items.is_empty()),
                    )
                {
                    // With one empty list, equality only compares emptiness. No elements are read.
                    values = values
                        .into_iter()
                        .map(|list| self.truth_value(list))
                        .collect();
                }
                if values.iter().any(|v| matches!(v.ty, Type::WeakProxy(_))) {
                    return self.weak_proxy_comparison(values, &compare.ops, expression);
                }
                if values
                    .iter()
                    .any(|v| matches!(v.ty, Type::Class(id) if self.fraction_class(id)))
                    && compare
                        .ops
                        .iter()
                        .any(|op| !matches!(op, ast::CmpOp::Is | ast::CmpOp::IsNot))
                {
                    return Err(self.object_builtin_error_with_checked_calls(
                        "Fraction numeric comparison",
                        expression,
                    ));
                }
                let decimal =
                    |value: &Expr| matches!(value.ty, Type::Class(id) if self.decimal_class(id));
                if values.iter().any(decimal) {
                    for (pair, op) in values.windows(2).zip(compare.ops.iter()) {
                        let unsupported_numeric_op = match op {
                            ast::CmpOp::Is | ast::CmpOp::IsNot => false,
                            ast::CmpOp::Eq | ast::CmpOp::NotEq => {
                                pair.iter().any(|v| v.ty == Type::Float)
                            }
                            _ => true,
                        };
                        if unsupported_numeric_op
                            && pair.iter().any(decimal)
                            && pair.iter().all(|v| {
                                decimal(v)
                                    || matches!(
                                        v.ty,
                                        Type::Int | Type::Bool | Type::Float | Type::IntEnum(_)
                                    )
                            })
                        {
                            return Err(error(
                                self.source,
                                expression,
                                "native Decimal numeric comparisons are unsupported",
                            ));
                        }
                    }
                    return Err(self.object_builtin_error_with_checked_calls(
                        "Decimal comparison",
                        expression,
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
                    return Ok(Expr {
                        ty: Type::Bool,
                        kind: ExprKind::Compare(values, vec![*op]),
                    });
                }
                if let [op @ (ast::CmpOp::Eq | ast::CmpOp::NotEq)] = compare.ops.as_ref()
                    && values.iter().all(|v| v.ty == Type::SourceModule)
                {
                    return Ok(Self::identity_result(values, *op == ast::CmpOp::NotEq));
                }
                if values.iter().all(|v| matches!(v.ty, Type::ForwardRef(_))) {
                    if !compare
                        .ops
                        .iter()
                        .all(|op| matches!(op, ast::CmpOp::Eq | ast::CmpOp::NotEq))
                    {
                        return Err(error(self.source, expression, "ForwardRef has no ordering"));
                    }
                    let ops = compare.ops.to_vec();
                    return Ok(Expr {
                        ty: Type::Bool,
                        kind: ExprKind::Compare(values, ops),
                    });
                }
                let mut values = self.numeric_union_comparison(values);
                if let Some(result) = text::slice_equality(self.types, &mut values, &compare.ops) {
                    return Ok(result);
                }
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
                let uuid_and_scalar = |uuid: Type, other: Type| {
                    matches!(uuid, Type::Class(id) if self.classes.declarations.definitions[id].is_uuid(self.graph))
                        && self.types.immutable_key_field(uuid)
                        && matches!(
                            other,
                            Type::Int
                                | Type::Bool
                                | Type::Float
                                | Type::Str
                                | Type::StrLiteral(_)
                                | Type::Bytes
                                | Type::None
                        )
                };
                if let [op @ (ast::CmpOp::Eq | ast::CmpOp::NotEq)] = compare.ops.as_ref()
                    && let [left, right] = values.as_slice()
                    && (matches!(left.ty, Type::Enum(_) | Type::BuiltinClass)
                        || matches!(right.ty, Type::Enum(_) | Type::BuiltinClass)
                        || uuid_and_scalar(left.ty, right.ty)
                        || uuid_and_scalar(right.ty, left.ty))
                    && left.ty != right.ty
                    && [left.ty, right.ty].into_iter().all(|ty| {
                        self.types.payload(ty).is_none() && self.types.members(ty).is_none()
                    })
                {
                    return Ok(Expr {
                        ty: Type::Bool,
                        kind: ExprKind::Scope {
                            body: values.into_iter().map(Statement::Expr).collect(),
                            value: Box::new(Expr::boolean(*op == ast::CmpOp::NotEq)),
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
                    return Ok(Expr {
                        ty: Type::Bool,
                        kind: ExprKind::Compare(values, vec![*op]),
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
                            Expr::not(eq)
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
                    if matches!(
                        op,
                        ast::CmpOp::Lt | ast::CmpOp::LtE | ast::CmpOp::Gt | ast::CmpOp::GtE
                    ) && (matches!(ty, Type::Deque(_)) || matches!(right, Type::Deque(_)))
                    {
                        return Err(
                            self.scoped_builtin_error("collections.deque ordering", expression)
                        );
                    }
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
                    // Counters compare counts even when their key representations differ.
                    if self.types.is_counter(ty)
                        && self.types.is_counter(right)
                        && Compare::from_operator(op).is_some()
                    {
                        ops.push(op);
                        continue;
                    }
                    if (self.types.is_ordered_dict(ty) || self.types.is_ordered_dict(right))
                        && self.types.mapping(ty) == self.types.mapping(right)
                        && matches!(op, ast::CmpOp::Eq | ast::CmpOp::NotEq)
                    {
                        ops.push(op);
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
                    let strings = matches!(ty, Type::Str | Type::StrLiteral(_))
                        && matches!(right, Type::Str | Type::StrLiteral(_));
                    let ordered = numeric
                        || strings
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
                    if Compare::from_operator(op).is_none()
                        || !ordered && !matches!(op, ast::CmpOp::Eq | ast::CmpOp::NotEq)
                    {
                        return Err(self.unsupported_comparison(expression, ty, right));
                    }
                    if matches!(ty, Type::Patch(_)) {
                        return Err(error(
                            self.source,
                            expression,
                            "comparison of unified-diff objects is unsupported",
                        ));
                    }
                    if matches!(
                        ty,
                        Type::RegexPattern
                            | Type::RegexMatch
                            | Type::RegexBytesPattern
                            | Type::RegexBytesMatch
                    ) {
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
                        | Type::JsonEncoder
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
                            return Err(self.unsupported_comparison(expression, ty, right));
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
                let result = if live_scalar_operands {
                    Type::PythonObject
                } else {
                    Type::Bool
                };
                (result, ExprKind::Compare(values, ops))
            }
            ast::Expr::Subscript(subscript) => {
                if let Some(ty) = self.environ_item_type(&subscript.value) {
                    let key = self.expr_expected(&subscript.slice, ty)?;
                    return Ok(Expr {
                        ty,
                        kind: ExprKind::Builtin(
                            Builtin::process(ir::ProcessOperation::EnvironIndex {
                                bytes: ty == Type::Bytes,
                            }),
                            vec![key],
                        ),
                    });
                }
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
                                Builtin::process(ir::ProcessOperation::ExceptionAt),
                                vec![value, index],
                            ),
                        });
                    }
                }
                if self.resolve(&subscript.value) == Some(Binding::Intrinsic(Intrinsic::Callable)) {
                    return Err(self.object_builtin_error_with_checked_calls(
                        "a parameterized Callable",
                        subscript,
                    ));
                }
                let list = Self::enum_text(self.expr(&subscript.value)?);
                if list.ty == Type::PythonObject {
                    let key =
                        registered::values::index_key(&subscript.slice, &mut |key| self.expr(key))?;
                    return Ok(registered::values::located(
                        expression.range(),
                        expression.start(),
                        registered::values::subscript(subscript, list, key, self.source),
                    ));
                }
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
                    return Err(self.object_builtin_error_with_checked_calls(
                        "a runtime generic alias",
                        subscript,
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
                    return self.slice(list, slice, subscript.range());
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
                    return self.dict_subscript(list, &subscript.slice, key, value);
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
                    ExprKind::Builtin(
                        Builtin::ListGet(Some(Box::new(self.fixed_origin(
                            subscript.range(),
                            Some((subscript.value.end(), b'[')),
                        )))),
                        vec![list, index],
                    ),
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
                if attribute.attr.as_str() == "__qualname__"
                    && self.registered.is_none()
                    && !self.globals.registered_initializers
                    && let Some(call) = attribute.value.as_call_expr()
                    && self.unshadowed_builtin(&call.func, "type")
                    && let [argument] = call.arguments.args.as_ref()
                    && call.arguments.keywords.is_empty()
                    && let Some(slot) = argument
                        .as_name_expr()
                        .and_then(|name| self.names.get(name.id.as_str()))
                    && let Some(class) = self
                        .caught_exception(*slot)
                        .and_then(|caught| caught.builtin_class)
                {
                    let exception = self.expr(argument)?;
                    return Ok(partials::after(exception, unpacking::string(class)));
                }
                if self.boxed_deque_descriptor(attribute) {
                    return Err(self.scoped_builtin_error(
                        &format!("collections.deque.{}", attribute.attr.as_str()),
                        attribute,
                    ));
                }
                if attribute.attr.as_str() == "MISSING"
                    && base_binding == Some(Binding::Intrinsic(Intrinsic::ContextToken))
                {
                    return Ok(contextvars::op(
                        ir::ContextOperation::Missing,
                        Type::ContextMissing,
                        vec![],
                    ));
                }
                if matches!(attribute.attr.as_str(), "machinery" | "metadata" | "util")
                    && base_binding
                        == Some(Binding::IntrinsicModule(
                            crate::modules::IntrinsicModule::Importlib,
                        ))
                    && binding.is_none()
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
                if base_binding == Some(Binding::Intrinsic(Intrinsic::SysFlags))
                    && let Some(flag) = self.command_flag(attribute)?
                {
                    return Ok(flag);
                }
                if let Some(text) = self.testcase_class_text(attribute)? {
                    return Ok(text);
                }
                let receiver_binding = self.resolve(&attribute.value);
                if receiver_binding == Some(Binding::Intrinsic(Intrinsic::StructTime)) {
                    return self.struct_time_class_field(attribute);
                }
                if matches!(
                    (receiver_binding, attribute.attr.as_str()),
                    (
                        Some(Binding::Intrinsic(Intrinsic::SplitResult { bytes: false })),
                        "_encoded_counterpart"
                    ) | (
                        Some(Binding::Intrinsic(Intrinsic::SplitResult { bytes: true })),
                        "_decoded_counterpart"
                    )
                ) {
                    return Err(
                        self.scoped_builtin_error("urllib.parse result counterpart", attribute)
                    );
                }
                if let Some((origin, declaration)) = self.namedtuple_factory(receiver_binding) {
                    if matches!(origin, crate::types::TupleOrigin::Declaration(_))
                        && declaration
                            .fields
                            .iter()
                            .any(|field| field == attribute.attr.as_str())
                    {
                        return Err(
                            self.scoped_builtin_error("namedtuple field descriptor", attribute)
                        );
                    }
                    return self.namedtuple_class_attribute(attribute, &declaration);
                }
                if let Some(method) = ir::StrDescriptor::named(attribute.attr.as_str())
                    && matches!(&*attribute.value, ast::Expr::Name(n) if n.id == "str")
                    && !self.names.contains_key("str")
                    && receiver_binding.is_none()
                {
                    return Ok(inspect::descriptor(method));
                }
                if attribute.attr.as_str() == "join"
                    && matches!(&*attribute.value, ast::Expr::Name(n) if n.id == "str")
                    && !self.names.contains_key("str")
                    && receiver_binding.is_none()
                {
                    return Ok(self.string_join_value(None, attribute));
                }
                if let Some(Binding::Intrinsic(
                    stream @ (Intrinsic::Stdin | Intrinsic::OriginalStdout | Intrinsic::Stderr),
                )) = receiver_binding
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
                if matches!(receiver.ty, Type::RegexPattern | Type::RegexBytesPattern) {
                    let member = match attribute.attr.as_str() {
                        "pattern" => Some((
                            if receiver.ty == Type::RegexBytesPattern {
                                Type::Bytes
                            } else {
                                Type::Str
                            },
                            Builtin::RegexSpelling,
                        )),
                        "flags" => Some((Type::Int, Builtin::RegexFlags)),
                        _ => None,
                    };
                    if let Some((ty, member)) = member {
                        return Ok(Expr {
                            ty,
                            kind: ExprKind::Builtin(member, vec![receiver]),
                        });
                    }
                }
                if self.types.payload(receiver.ty).unwrap_or(receiver.ty) == Type::PythonObject {
                    return Ok(registered::values::loaded_attribute(attribute, receiver));
                }
                if receiver.ty == Type::Float
                    && matches!(attribute.attr.as_str(), "real" | "imag")
                {
                    if attribute.attr.as_str() == "real" {
                        return Ok(receiver);
                    }
                    let zero = Expr {
                        ty: Type::Float,
                        kind: ExprKind::Float(0.0),
                    };
                    return Ok(self.evaluated_receiver(Some(receiver), zero));
                }
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
                    && let Some(value) = self.dispatch_attribute(&receiver, layout, attribute)
                {
                    return Ok(value);
                }
                if let Some(wrapped) = self.callable_attribute(&receiver, attribute)? {
                    return Ok(wrapped);
                }
                let receiver = self.class_object_view(receiver);
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
                if matches!(
                    receiver.ty,
                    Type::Str | Type::StrLiteral(_) | Type::StrEnum(_)
                ) && ir::StrDescriptor::named(attribute.attr.as_str()).is_some_and(|method| {
                    method.is_predicate()
                        || matches!(
                            method,
                            ir::StrDescriptor::ZFill
                                | ir::StrDescriptor::Title
                                | ir::StrDescriptor::SwapCase
                        )
                }) {
                    return Err(self.object_builtin_error(
                        &format!("bound str.{}", attribute.attr.as_str()),
                        attribute,
                    ));
                }
                if receiver.ty == Type::Str && attribute.attr.as_str() == "join" {
                    return Ok(self.string_join_value(Some(receiver), attribute));
                }
                if receiver.ty == Type::JsonDecoder && attribute.attr.as_str() == "decode" {
                    return self.json_load_callable(Some(receiver), None, attribute);
                }
                if receiver.ty == Type::FrozenStrings && attribute.attr.as_str() == "__contains__" {
                    return Ok(self.builtin_callable(
                        attribute,
                        Some(receiver),
                        Builtin::SetContains,
                        vec![Type::Str],
                        Type::Bool,
                    ));
                }
                if attribute.attr.as_str() == "reverse" && self.types.element(receiver.ty).is_some()
                {
                    return Ok(self.builtin_callable(
                        attribute,
                        Some(receiver),
                        Builtin::ListReverse,
                        Vec::new(),
                        Type::None,
                    ));
                }
                if matches!(attribute.attr.as_str(), "keys" | "values" | "items")
                    && self.types.mapping(receiver.ty).is_some()
                {
                    return Err(
                        self.scoped_builtin_error("bound dictionary view method", attribute)
                    );
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
                if calendar::boxed_member(receiver.ty, attribute.attr.as_str())
                    || calendar::date_isoformat(receiver.ty, attribute.attr.as_str())
                {
                    return Err(self.object_runtime_export("datetime instance member", attribute));
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
                            .caught_exception(slot)
                            .and_then(|caught| caught.fields.tree_issue)
                    {
                        return Ok(Expr {
                            ty: self.types.tuple(vec![self.types.list(issue)]),
                            kind: ExprKind::Builtin(
                                Builtin::process(ir::ProcessOperation::TreeErrorArgs),
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
                            Builtin::process(ir::ProcessOperation::OsErrorField(4)),
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
                                Builtin::process(ir::ProcessOperation::OsErrorField(index)),
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
                if matches!(
                    attribute.attr.as_str(),
                    "buffer" | "write_through" | "line_buffering"
                ) && self.types.payload(receiver.ty).unwrap_or(receiver.ty) == Type::TextIO
                {
                    return Err(self.scoped_builtin_error("text stream buffer", attribute));
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
                if matches!(attribute.attr.as_str(), "object" | "multiple")
                    && let Type::Class(class) = receiver.ty
                    && self.bundled_mock_class(class, "_PatchFactory")
                {
                    return Err(self.object_runtime_export(
                        &format!("unittest.mock.patch.{}", attribute.attr.as_str()),
                        attribute,
                    ));
                }
                if matches!(attribute.attr.as_str(), "args" | "kwargs")
                    && self.mock_call_record(receiver.ty)
                {
                    return Err(self.object_runtime_export("unittest.mock call tuple", attribute));
                }
                if let Some(field) = self.setup_field(receiver.ty, attribute.attr.as_str()) {
                    return self.read_setup_field(receiver, attribute, field);
                }
                if let Type::Class(class) = receiver.ty
                    && (self.classes.methods[class].contains_key(attribute.attr.as_str())
                        || (attribute.attr.as_str() == "__float__" && self.decimal_class(class))
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
                    && self.present_fields.contains(&(slot, field))
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
                let value = Expr {
                    ty,
                    kind: ExprKind::Field(Box::new(receiver), field),
                };
                if let Some(sites) = &mut self.fixed_sites
                    && sites.wrapper_fields
                {
                    sites.field(attribute, &value);
                }
                return Ok(value);
            }
            ast::Expr::Call(_) | ast::Expr::BinOp(_) => unreachable!("dispatched by typed_expr"),
            ast::Expr::Generator(_) => {
                return Err(error(
                    self.source,
                    expression,
                    "generator expressions require an immediate for, comprehension, list, sum, min, max, any, or all consumer",
                ));
            }
            ast::Expr::Yield(_) | ast::Expr::YieldFrom(_) => {
                return Err(error(
                    self.source,
                    expression,
                    "generator suspension requires a registered frame",
                )
                .classify(Kind::FrameSuspension));
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

    fn call(&mut self, mut call: &ast::ExprCall) -> Result<Expr, String> {
        let live = match &*call.func {
            ast::Expr::Attribute(field) => self
                .live_module_binding(&field.value)
                .map(|receiver| registered::values::loaded_attribute(field, receiver)),
            target => self.live_module_binding(target),
        };
        if let Some(callee) = live {
            let value = self.call_with_target(call, callee)?;
            self.graph.checked_live_call(self.module, call);
            return Ok(value);
        }
        if self.module_scope
            && self.globals.registered_initializers
            && call.arguments.is_empty()
            && self.unshadowed_builtin(&call.func, "globals")
        {
            // A registered initializer already owns the namespace returned by globals().
            self.namespace_calls.insert(call.range());
            return Ok(registered::values::name_call(call, "globals", vec![]));
        }
        if self.graph.session.fixed_exec_call(self.module, call) {
            return Err(self.scoped_builtin_error("exec", call));
        }
        if self.unshadowed_builtin(&call.func, "range") {
            return self.range_value(call);
        }
        if self.unshadowed_builtin(&call.func, "__import__")
            && call
                .arguments
                .find_argument_value("name", 0)
                .is_some_and(|name| !matches!(name, ast::Expr::StringLiteral(_)))
        {
            return Err(self.scoped_builtin_error("__import__", call));
        }
        if self.unshadowed_builtin(&call.func, "getattr")
            && call.arguments.keywords.is_empty()
            && let [owner, ast::Expr::StringLiteral(key)]
            | [owner, ast::Expr::StringLiteral(key), _] = call.arguments.args.as_ref()
            && self.resolve(owner)
                == Some(Binding::IntrinsicModule(
                    crate::modules::IntrinsicModule::Time,
                ))
            && let Some(Binding::Intrinsic(Intrinsic::ScopedObjectExport(export))) =
                crate::modules::IntrinsicModule::Time.symbol(key.value.to_str())
        {
            return Err(self.scoped_builtin_error(export, call));
        }
        if let Some(method) = self.boxed_descriptor(&call.func) {
            if matches!(method, "re.Pattern.findall" | "re.Pattern.split") {
                return self.compiled_regex_collection(call, method == "re.Pattern.split", method);
            }
            return Err(self.scoped_builtin_error(method, call));
        }
        if let Some(local) = self.local_class(&call.func)?
            && let Some(base) = local.base
            && !local.abstract_methods.is_empty()
        {
            let message = mapping_abc::abstract_message(&local.name, &local.abstract_methods);
            return self.abstract_class_call(base, call, message);
        }
        if let Some(value) = self.literal_attrgetter_call(call)? {
            return Ok(value);
        }
        if let Some(method) = self.request_method_lookup(call)? {
            return Ok(method);
        }

        if let Some(name) = self.boxed_builtin_callable(&call.func) {
            if name == "exec"
                && self
                    .graph
                    .session
                    .is_stdlib_function(self.module, &call.func, "builtins", name)
                && call.arguments.keywords.is_empty()
                && let [operand] = call.arguments.args.as_ref()
                && self.graph.session.scalar_type(self.module, operand) == Some(Type::Str)
            {
                match self.expr(operand) {
                    Ok(source) if source.ty == Type::Str => {
                        return Ok(Expr {
                            ty: Type::None,
                            kind: ExprKind::Builtin(
                                Builtin::ObjectCall(
                                    ir::ObjectCall::ExecText,
                                    self.module,
                                    operand.range(),
                                ),
                                vec![source],
                            ),
                        });
                    }
                    Err(error) if !error.is_object() => return Err(error),
                    _ => {}
                }
            }
            return Err(self.scoped_builtin_error(name, call));
        }
        let binding = self.resolve(&call.func);
        if binding == Some(Binding::Intrinsic(Intrinsic::BuiltinsAscii)) {
            return self.ascii_call(call);
        }
        if let Some(registration) = self.immediate_registration(call)? {
            return Ok(registration);
        }
        if call
            .func
            .as_name_expr()
            .is_some_and(|n| n.id == "object" && !self.names.contains_key("object"))
            && binding.is_none()
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
        // Every queued call has already been checked for a static binding.
        let mut chain = Vec::new();
        while binding.is_none()
            && let ast::Expr::Attribute(method) = &*call.func
            && let ast::Expr::Call(inner) = &*method.value
            && inner.func.is_attribute_expr()
            && self.resolve(&inner.func).is_none()
            && !call.arguments.args.iter().any(ast::Expr::is_starred_expr)
            && call.arguments.keywords.iter().all(|kw| kw.arg.is_some())
        {
            chain.push(call);
            call = inner;
        }
        let mut value = self.call_with_binding(call, None, binding)?;
        for call in chain.into_iter().rev() {
            if matches!(value.ty, Type::StrLiteral(_)) {
                value = literals::view(value, Type::Str);
            }
            value = self.call_with_binding(call, Some(value), None)?;
        }
        Ok(value)
    }

    fn call_on(&mut self, call: &ast::ExprCall, receiver: Option<Expr>) -> Result<Expr, String> {
        let binding = self.resolve(&call.func);
        self.call_with_binding(call, receiver, binding)
    }

    fn call_with_binding(
        &mut self,
        call: &ast::ExprCall,
        mut receiver: Option<Expr>,
        binding: Option<Binding>,
    ) -> Result<Expr, String> {
        if binding == Some(Binding::Intrinsic(Intrinsic::BuiltinsLen))
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
            && binding == Some(Binding::Intrinsic(Intrinsic::DefaultDict))
            && !self.local_collection_root(&call.func)
            && let Some(raised) = self.defaultdict_factory_error(call, Type::None)?
        {
            return Ok(raised);
        }
        if let ast::Expr::Attribute(method) = &*call.func
            && method.attr.as_str() == "fromhex"
            && (self.unshadowed_builtin(&method.value, "bytes")
                || self.unshadowed_builtin(&method.value, "bytearray"))
        {
            return Err(self.object_builtin_error("fromhex", call));
        }
        if let ast::Expr::Attribute(method) = &*call.func
            && matches!(method.attr.as_str(), "fromhex" | "hex")
            && self.unshadowed_builtin(&method.value, "float")
        {
            return Err(self.scoped_builtin_error(&format!("float.{}", method.attr), call));
        }
        if receiver.is_none()
            && let ast::Expr::Attribute(method) = call.func.as_ref()
            && let Some(name) = self.boxed_set_method(method, None)
        {
            return Err(self.object_builtin_error(name, call));
        }
        if receiver.is_none()
            && let Some(result) = self.fromkeys_call(call, None)?
        {
            return Ok(result);
        }
        if receiver.is_none()
            && matches!(&*call.func,ast::Expr::Name(n) if n.id.as_str()=="len")
            && !self.names.contains_key("len")
            && binding.is_none()
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
        if let Some(value) = self.cache_constructor(call, binding)? {
            return Ok(value);
        }
        if let Some(value) = self.cache_control(call, receiver.as_ref())? {
            return Ok(value);
        }
        if let Some(Binding::Intrinsic(Intrinsic::MockCallList { mark })) = binding {
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
        if let Some(suite) = self.doctest_suite(call, binding)? {
            return Ok(suite);
        }
        if let Some(value) = self.sys_call(call, binding)? {
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
                && binding.is_none()
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
        if processes::boxed_launch_keywords(call)
            && matches!(
                binding,
                Some(Binding::Intrinsic(
                    Intrinsic::Popen
                        | Intrinsic::SubprocessCall
                        | Intrinsic::SubprocessCheckCall
                        | Intrinsic::SubprocessRun
                        | Intrinsic::CheckOutput
                ))
            )
        {
            return Err(self.scoped_builtin_error("subprocess launch keywords", call));
        }
        if binding == Some(Binding::Intrinsic(Intrinsic::OrderedDict)) {
            return self.ordered_dict_constructor(call, None);
        }
        if let Some(Binding::Intrinsic(
            kind @ (Intrinsic::TomliLoads
            | Intrinsic::TomliLoad
            | Intrinsic::TomlDumps
            | Intrinsic::TomliDump),
        )) = binding
        {
            return self.document_call(kind, call);
        }
        if let Some(expression) = self.parse_args(call)? {
            return Ok(expression);
        }
        self.call_with_key_view(call, binding, None, receiver)
    }

    fn dict_subscript(
        &mut self,
        mapping: Expr,
        index: &ast::Expr,
        key: Type,
        value: Type,
    ) -> Result<Expr, String> {
        // Reading an ordinary enum-keyed dictionary may probe with an equal integer.
        // A default factory can insert the supplied key, which changes its stored type.
        let integer = (matches!(key, Type::IntEnum(_))
            && !self.types.is_default_list_dict(mapping.ty)
            && !self.types.is_counter(mapping.ty))
        .then_some(Type::Int);
        let key = self.expr_expected_or(index, key, integer)?;
        let required = self.required_group_key(&mapping, &key);
        let expression = self
            .projected_group(&mapping, &key)
            .unwrap_or_else(|| Expr {
                ty: value,
                kind: ExprKind::Builtin(Builtin::DictGet, vec![mapping, key]),
            });
        Ok(if required {
            Expr {
                ty: Type::Str,
                kind: ExprKind::Unwrap(Box::new(expression)),
            }
        } else {
            expression
        })
    }

    fn dict_popitem(
        &self,
        call: &ast::ExprCall,
        receiver: Expr,
        args: &[ast::Expr],
    ) -> Result<Expr, String> {
        if !args.is_empty() || !call.arguments.keywords.is_empty() {
            return Err(error(
                self.source,
                call,
                "dict.popitem() takes no arguments",
            ));
        }
        let Some((key, value)) = self
            .types
            .mapping(receiver.ty)
            .filter(|&(k, v)| receiver.ty == self.types.dict(k, v))
        else {
            return Err(error(
                self.source,
                call,
                "dict.popitem requires a plain dictionary",
            ));
        };
        Ok(Expr {
            ty: self.types.tuple(vec![key, value]),
            kind: ExprKind::Builtin(Builtin::DictPopItem, vec![receiver]),
        })
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
            [] => self.coerce(Expr::none(), value_type, call)?,
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
                self.call_with_key_view(call, self.resolve(&call.func), Some(&mut view), None)?
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
        binding: Option<Binding>,
        key_view: Option<&mut bool>,
        receiver: Option<Expr>,
    ) -> Result<Expr, String> {
        if let Some((origin, declaration)) = self.namedtuple_factory(binding) {
            return self.namedtuple_call(origin, &declaration, call);
        }
        if binding == Some(Binding::Intrinsic(Intrinsic::NamedTuple))
            && !self.local_collection_root(&call.func)
        {
            return Err(self.scoped_builtin_error("collections.namedtuple", call));
        }
        if let Some((method, name)) = crate::classes::super_method(call)
            && self.unshadowed_builtin(name, "super")
        {
            if method.attr.as_str() == "__init__" {
                return self.call_super_initializer(call);
            }
            if let Some(owner) = self.super_method_owner
                && let Some(storage) = self.classes.declarations.definitions[owner].storage_base
            {
                let first = self.classes.declarations.definitions[owner].bases[0];
                let base = if self.classes.methods[first].contains_key(method.attr.as_str()) {
                    first
                } else {
                    storage
                };
                return self.call_super_method(call, method, base);
            }
        }
        if let Some(expr) = self.call_direct_base_initializer(call)? {
            return Ok(expr);
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
        if let ast::Expr::Attribute(attribute) = &*call.func
            && self.boxed_deque_descriptor(attribute)
        {
            return self.boxed_deque_call(attribute.attr.as_str(), call);
        }
        if let ast::Expr::Attribute(method) = &*call.func
            && method.attr.as_str() == "__len__"
            && self
                .graph
                .session
                .is_builtin_sequence_class(self.module, &method.value)
        {
            return Err(self.scoped_builtin_error("sequence length descriptor", call));
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
            && method.attr.as_str() == "popitem"
            && self.unshadowed_builtin(&method.value, "dict")
        {
            let Some((owner, rest)) = call.arguments.args.split_first() else {
                return Err(error(
                    self.source,
                    call,
                    "unbound dict.popitem requires a receiver",
                ));
            };
            let receiver = self.expr(owner)?;
            return self.dict_popitem(call, receiver, rest);
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
        match binding {
            Some(Binding::Intrinsic(Intrinsic::Partial)) => return self.partial(call),
            Some(Binding::Intrinsic(Intrinsic::LoggingShutdown)) => {
                return self.logging_shutdown(call);
            }
            Some(Binding::Intrinsic(Intrinsic::FileConfig)) if calls::has_unpacking(call) => {
                return self.logging_file_config(call);
            }
            _ => {}
        }
        if calls::starred(call) {
            match binding {
                Some(Binding::Intrinsic(Intrinsic::Credential(name))) => {
                    return self.credential_call(call, name);
                }
                Some(Binding::Intrinsic(Intrinsic::Fd(
                    op @ (ir::FdOperation::Pipe2
                    | ir::FdOperation::TcGetPgrp
                    | ir::FdOperation::TcSetPgrp
                    | ir::FdOperation::Read
                    | ir::FdOperation::Write
                    | ir::FdOperation::CopyFileRange),
                ))) => {
                    return self.fd_call(call, op);
                }
                _ => {}
            }
            if let Some(Binding::Definition(definition)) = binding
                && let Some(signature) = self.functions.get(&definition)
            {
                return self.source_call(call, signature, None, 0);
            }
            if let ast::Expr::Attribute(method) = &*call.func
                && binding.is_none()
            {
                if let Some(class) = self.class_reference(&method.value) {
                    return self.class_method_call(call, method, class, None);
                }
                let object = match receiver {
                    Some(value) => value,
                    None => self.expr(&method.value)?,
                };
                if object.ty == Type::PythonObject {
                    return self.call_object_method(call, method, object);
                }
                if let Type::Class(class)
                | Type::IntEnum(class)
                | Type::StrEnum(class)
                | Type::Enum(class) = object.ty
                    && self.classes.methods[class].contains_key(method.attr.as_str())
                {
                    return self.class_method_call(call, method, class, Some(object));
                }
            } else if binding.is_none() {
                let callable = self.expr(&call.func)?;
                if callable.ty == Type::PythonObject {
                    return self.invoke(call, callable);
                }
            }
            return Err(error(
                self.source,
                call,
                "argument unpacking requires a source-defined function or method",
            ));
        }
        match binding {
            Some(Binding::Intrinsic(Intrinsic::CachePath { reverse })) => {
                return self.cache_path_call(call, reverse);
            }
            Some(Binding::Intrinsic(Intrinsic::ModuleSpec)) => return self.module_spec(call),
            Some(Binding::Intrinsic(Intrinsic::SimpleNamespace)) => {
                return self.simple_namespace(call);
            }
            _ => {}
        }
        if calls::unpacked_keywords(call)
            && !matches!(binding, Some(Binding::Definition(id))
                if self.functions.get(&id).is_some_and(|s| calls::rest_keywords(call, &s.params).1.is_some() || calls::named_mapping(call, &s.params).is_some()))
        {
            return Err(error(self.source, call, calls::UNSUPPORTED_UNPACKING));
        }
        if matches!(&*call.func, ast::Expr::Name(name) if name.id == "dict" && !self.names.contains_key(name.id.as_str()))
            && binding.is_none()
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
            if let Some(key) = key.or_else(|| {
                let (key, _) = self.types.mapping(source.ty)?;
                self.graph.has_object_constructor_fallback.set(true);
                Some(key)
            }) {
                return Ok(Expr {
                    ty: self.types.set(key),
                    kind: ExprKind::Builtin(Builtin::KeySet, vec![source]),
                });
            }
            if matches!(
                source.ty,
                Type::ListInt | Type::List(_) | Type::VarTuple(_) | Type::Str | Type::Bytes
            ) || self
                .types
                .homogeneous_tuple_element(source.ty)
                .is_some_and(|ty| self.types.hashable_key(ty))
            {
                self.graph.has_object_constructor_fallback.set(true);
                return self
                    .prepare_container(source, false, input, false)
                    .and_then(|items| self.collect_set(items, input))
                    .map_err(|_| self.object_builtin_error("set", call));
            }
            return Err(self.object_builtin_error("set", call));
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
                self.classes.declarations,
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
                self.classes.declarations,
                self.types,
            )?;
            return self.counter(call, None, Some(ty));
        }
        match binding {
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
            Some(Binding::Intrinsic(
                Intrinsic::Declaration(symbol) | Intrinsic::RuntimeImport(symbol),
            )) => {
                return Err(error(
                    self.source,
                    call,
                    &format!("no native implementation for {symbol}"),
                ));
            }
            Some(Binding::TypeAlias(module, index))
                if matches!(
                    self.graph.modules[module].aliases[index],
                    crate::modules::TypeAlias::NewType(_)
                ) =>
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
            Some(Binding::Intrinsic(Intrinsic::Placeholder(kind))) => {
                return self.placeholder_call(call, partials::singleton(kind), kind);
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
                    && (matches!(
                        self.resolve(target),
                        Some(Binding::Intrinsic(Intrinsic::BytesIo | Intrinsic::StringIo))
                    ) || target.as_name_expr().is_some_and(|name| {
                        annotations::is_builtin_type(name.id.as_str())
                            && self.unshadowed_builtin(target, name.id.as_str())
                    }))
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
            Some(Binding::Intrinsic(Intrinsic::UnicodeProperty(property))) => {
                return self.unicode_character_property(call, property);
            }
            Some(Binding::Intrinsic(Intrinsic::UnicodeNormalize { check })) => {
                return self.unicode_normalize(call, check);
            }
            Some(Binding::Intrinsic(Intrinsic::CodecLookup)) => return self.codec_lookup(call),
            Some(Binding::Intrinsic(Intrinsic::SocketControlSize { padded })) => {
                let params = [ParameterKind::PositionalOnly.parameter("length", Type::Int, None)];
                let args = self.arguments(call, &params, None)?;
                return Ok(self.ordered_builtin(
                    Builtin::Socket(ir::SocketOp::ControlSize { padded }),
                    Type::Int,
                    args,
                ));
            }
            Some(Binding::Intrinsic(Intrinsic::SocketGetFqdn)) => {
                let params = [parameter(
                    "name",
                    Type::Str,
                    Some(Expr {
                        ty: Type::Str,
                        kind: ExprKind::Str(String::new()),
                    }),
                )];
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
                op @ (Intrinsic::Sha1
                | Intrinsic::Sha256
                | Intrinsic::NamedHash(_)
                | Intrinsic::HashNew),
            )) => {
                return self.hash_constructor(call, op);
            }
            Some(Binding::Intrinsic(Intrinsic::HashFileDigest)) => {
                return self.hash_file_digest(call);
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
                    &[parameter(
                        "initial_bytes",
                        Type::Bytes,
                        Some(Expr {
                            ty: Type::Bytes,
                            kind: ExprKind::Bytes(Vec::new()),
                        }),
                    )],
                    None,
                )?;
                return Ok(self.ordered_builtin(Builtin::BytesIo, Type::BytesIO, args));
            }
            Some(Binding::Intrinsic(Intrinsic::ZipFile)) => return self.zipfile_constructor(call),
            Some(Binding::Intrinsic(Intrinsic::ZipImportIndex)) => {
                let args = self.arguments(call, &[parameter("path", Type::Str, None)], None)?;
                return Ok(self.ordered_builtin(Builtin::ZipImportIndex, Type::ZipFile, args));
            }
            Some(Binding::Intrinsic(Intrinsic::Deque)) => return self.deque_new(call, None),
            Some(Binding::Intrinsic(Intrinsic::Counter)) => return self.counter(call, None, None),
            Some(Binding::Intrinsic(Intrinsic::ChainMap)) => return self.chainmap(call, None),
            Some(Binding::Intrinsic(Intrinsic::CountElements)) => return self.count_elements(call),
            Some(Binding::Intrinsic(
                Intrinsic::TracebackFormatter(name) | Intrinsic::ObjectOnlyExport(name),
            )) => {
                return Err(self.object_export_error(name, call));
            }
            Some(Binding::Intrinsic(
                Intrinsic::StandardEnum | Intrinsic::IntEnum | Intrinsic::StrEnum,
            )) if call.arguments.args.len() >= 2
                || call
                    .arguments
                    .keywords
                    .iter()
                    .any(|kw| kw.arg.as_deref() == Some("names")) =>
            {
                return Err(self.scoped_builtin_error("enum.Enum", call));
            }
            Some(Binding::Intrinsic(Intrinsic::BuiltinsHex)) => {
                return Err(self.scoped_builtin_error("builtins.hex", call));
            }
            Some(Binding::Intrinsic(Intrinsic::CachedProperty)) => {
                return Err(self.object_runtime_export("functools.cached_property", call));
            }
            Some(Binding::Intrinsic(Intrinsic::ScopedObjectExport(
                name @ ("imre._calendar.month_grid"
                | "calendar.monthcalendar"
                | "calendar.firstweekday"
                | "calendar.setfirstweekday"),
            ))) => return self.month_grid_call(name, call),
            Some(Binding::Intrinsic(Intrinsic::ScopedObjectExport(
                name @ ("base64.b32encode"
                | "base64.b32hexencode"
                | "base64.b32decode"
                | "base64.b32hexdecode"),
            ))) => return self.base32_codec(call, name),
            Some(Binding::Intrinsic(Intrinsic::ScopedObjectExport(
                name @ ("base64.a85encode" | "base64.b85encode" | "base64.z85encode"),
            ))) => return self.base85_encode(call, name),
            Some(Binding::Intrinsic(Intrinsic::ScopedObjectExport(
                name @ ("operator.is_" | "operator.is_not"),
            ))) => {
                if let Some((left, op, right)) = self.operator_identity_call(call, name) {
                    return self.identity_comparison(left, right, op, call);
                }
                return Err(error(
                    self.source,
                    call,
                    "checked operator identity functions require two positional arguments",
                ));
            }
            Some(Binding::Intrinsic(Intrinsic::ScopedObjectExport(
                name @ ("math.fabs" | "math.log1p"),
            ))) if call.arguments.keywords.is_empty()
                && let [arg] = call.arguments.args.as_ref() =>
            {
                let (member, builtin) = if name == "math.fabs" {
                    ("fabs", Builtin::Abs)
                } else {
                    ("log1p", Builtin::FloatLog1p)
                };
                if !self
                    .graph
                    .session
                    .is_stdlib_function(self.module, &call.func, "math", member)
                {
                    return Err(self.scoped_builtin_error(name, call));
                }
                let value = self.expr(arg)?;
                return stdlib::unary_float_builtin(builtin, value)
                    .ok_or_else(|| self.scoped_builtin_error(name, call));
            }
            Some(Binding::Intrinsic(Intrinsic::OperatorCall)) => {
                let args = &call.arguments;
                if let Some(target) = args.args.first()
                    && !args.args.iter().any(ast::Expr::is_starred_expr)
                    && args.keywords.iter().all(|kw| kw.arg.is_some())
                    && self.graph.session.is_stdlib_function(
                        self.module,
                        &call.func,
                        "_operator",
                        "call",
                    )
                {
                    let mut forwarded = call.clone();
                    *forwarded.func = forwarded.arguments.args.remove(0);
                    // Preserve the ordinary checked binder for an exact named function.
                    if target
                        .as_name_expr()
                        .is_some_and(|name| !self.names.contains_key(name.id.as_str()))
                        && matches!(self.resolve(target), Some(Binding::Definition(id)) if self.functions.contains_key(&id))
                    {
                        return self.call(&forwarded);
                    }
                    let value = self.expr(target)?;
                    return self.invoke(&forwarded, value);
                }
                return Err(error(
                    self.source,
                    call,
                    "checked operator.call requires a resolved target and explicit arguments",
                ));
            }
            Some(Binding::Intrinsic(Intrinsic::OperatorNot)) => {
                if let [arg] = call.arguments.args.as_ref()
                    && call.arguments.keywords.is_empty()
                    && self.graph.session.is_stdlib_function(
                        self.module,
                        &call.func,
                        "_operator",
                        "not_",
                    )
                {
                    // A function argument is evaluated as a value before its truth is tested.
                    let value = self.expr(arg)?;
                    let truth = self.test_truth(value, arg)?;
                    return Ok(Expr::not(truth));
                }
                return Err(error(
                    self.source,
                    call,
                    "checked operator.not_ requires a resolved call with one positional argument",
                ));
            }
            Some(Binding::Intrinsic(Intrinsic::MathPerm)) => return self.math_perm(call),
            Some(Binding::Intrinsic(Intrinsic::ScopedObjectExport(
                name @ ("math.comb" | "math.factorial"),
            ))) if call.arguments.keywords.is_empty() => {
                let (member, op) = match (name, call.arguments.args.len()) {
                    ("math.comb", 2) => ("comb", Builtin::MathComb),
                    ("math.factorial", 1) => ("factorial", Builtin::MathFactorial),
                    _ => return Err(self.scoped_builtin_error(name, call)),
                };
                if self
                    .graph
                    .session
                    .is_stdlib_function(self.module, &call.func, "math", member)
                {
                    let args = call
                        .arguments
                        .args
                        .iter()
                        .map(|arg| self.expr(arg).map(powers::integer))
                        .collect::<StdResult<Vec<_>, _>>()?;
                    if args.iter().all(|arg| arg.ty == Type::Int) {
                        return Ok(Expr {
                            ty: Type::Int,
                            kind: ExprKind::Builtin(op, args),
                        });
                    }
                }
                return Err(self.scoped_builtin_error(name, call));
            }
            Some(Binding::Intrinsic(Intrinsic::ScopedObjectExport(name @ "math.log2")))
                if call.arguments.keywords.is_empty()
                    && let [arg] = call.arguments.args.as_ref()
                    && self.graph.session.is_stdlib_function(
                        self.module,
                        &call.func,
                        "math",
                        "log2",
                    ) =>
            {
                let value = self
                    .expr(arg)
                    .map_err(|_| self.scoped_builtin_error(name, call))?;
                if value.ty == Type::Float {
                    return Ok(self.checked_float_log2(value));
                }
                return Err(self.scoped_builtin_error(name, call));
            }
            Some(Binding::Intrinsic(Intrinsic::ScopedObjectExport("math.sumprod"))) => {
                return self.sumprod_call(call);
            }
            Some(Binding::Intrinsic(Intrinsic::ScopedObjectExport(name @ "math.prod"))) => {
                return self
                    .integer_product(call)
                    .map_err(|_| self.scoped_builtin_error(name, call));
            }
            Some(Binding::Intrinsic(Intrinsic::ScopedObjectExport("_signal.pause"))) => {
                if call.arguments.is_empty() {
                    return Ok(Expr {
                        ty: Type::None,
                        kind: ExprKind::Builtin(Builtin::SignalPause, vec![]),
                    });
                }
                calls::reject_unpacking(call, self.source)?;
                let message = if call.arguments.keywords.is_empty() {
                    format!(
                        "_signal.pause() takes no arguments ({} given)",
                        call.arguments.args.len()
                    )
                } else {
                    "_signal.pause() takes no keyword arguments".to_owned()
                };
                return self.positional_argument_error(
                    call,
                    &[],
                    Type::None,
                    unpacking::string(message),
                );
            }
            Some(Binding::Intrinsic(Intrinsic::ScopedObjectExport("struct._clearcache"))) => {
                if let ast::Expr::Attribute(attr) = call.func.as_ref()
                    && self
                        .graph
                        .session
                        .is_stdlib_module(self.module, &attr.value, "struct")
                {
                    self.graph
                        .checked_builtin_attribute(self.module, attr.range());
                }
                if call.arguments.is_empty() {
                    // Checked struct operations have no shared format cache.
                    return Ok(Expr::none());
                }
                calls::reject_unpacking(call, self.source)?;
                let message = if call.arguments.keywords.is_empty() {
                    format!(
                        "_struct._clearcache() takes no arguments ({} given)",
                        call.arguments.args.len()
                    )
                } else {
                    "_struct._clearcache() takes no keyword arguments".to_owned()
                };
                let values = call
                    .arguments
                    .args
                    .iter()
                    .chain(call.arguments.keywords.iter().map(|keyword| &keyword.value))
                    .map(|value| self.expr(value))
                    .collect::<StdResult<_, _>>()?;
                return Ok(calls::type_error(Type::None, values, message));
            }
            Some(Binding::Intrinsic(Intrinsic::ScopedObjectExport(name))) => {
                return Err(self.scoped_builtin_error(name, call));
            }
            Some(Binding::Intrinsic(Intrinsic::ObjectRuntimeExport(name @ "math.copysign")))
                if call.arguments.keywords.is_empty()
                    && let [x, y] = call.arguments.args.as_ref()
                    && self.graph.session.is_stdlib_function(
                        self.module,
                        &call.func,
                        "math",
                        "copysign",
                    ) =>
            {
                let x = self
                    .expr(x)
                    .map_err(|_| self.object_runtime_export(name, call))?;
                let y = self
                    .expr(y)
                    .map_err(|_| self.object_runtime_export(name, call))?;
                if x.ty == Type::Float && y.ty == Type::Float {
                    return Ok(Expr {
                        ty: Type::Float,
                        kind: ExprKind::Builtin(Builtin::FloatCopySign, vec![x, y]),
                    });
                }
                return Err(self.object_runtime_export(name, call));
            }
            Some(Binding::Intrinsic(Intrinsic::ObjectRuntimeExport("posix.getpid")))
                if call.arguments.is_empty()
                    && self.graph.session.is_stdlib_function(
                        self.module,
                        &call.func,
                        "os",
                        "getpid",
                    ) =>
            {
                return Ok(Expr {
                    ty: Type::Int,
                    kind: ExprKind::Builtin(Builtin::process(ir::ProcessOperation::GetPid), vec![]),
                });
            }
            Some(Binding::Intrinsic(Intrinsic::ObjectRuntimeExport("itertools.accumulate")))
                if self.is_accumulate(call, binding) =>
            {
                let args = self.accumulate_args(call)?;
                if args.function.ty != Type::None
                    || !matches!(
                        args.source.ty,
                        Type::ListInt | Type::List(_) | Type::Tuple(_) | Type::VarTuple(_)
                    )
                {
                    return Err(self.object_runtime_export("itertools.accumulate", call));
                }
                let mut operands = vec![args.source];
                operands.extend(args.initial);
                return Ok(args.scope.finish(Expr {
                    ty: self.types.cursor(args.element),
                    kind: ExprKind::Builtin(Builtin::AccumulateCursor, operands),
                }));
            }
            Some(Binding::Intrinsic(Intrinsic::ObjectRuntimeExport(name))) => {
                return Err(self.object_runtime_export(name, call));
            }
            Some(Binding::Intrinsic(Intrinsic::RegisteredImport(name))) => {
                return Err(self.object_runtime_export(name, call));
            }
            Some(Binding::Intrinsic(Intrinsic::AttrGetter)) => {
                return Err(self.runtime_attrgetter(call));
            }
            Some(Binding::Intrinsic(Intrinsic::LengthHint)) => {
                return self.cursor_length_hint(call);
            }
            Some(Binding::Intrinsic(Intrinsic::ShallowCopy)) => return self.shallow_copy(call),
            Some(Binding::Intrinsic(Intrinsic::DeepCopy)) => return self.deep_copy(call),
            Some(Binding::Intrinsic(Intrinsic::Combinatorial(op))) => {
                return self.combinatorial_cursor(call, op);
            }
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
            Some(Binding::Intrinsic(Intrinsic::GetEnv)) => {
                // An ordinary POSIX environment can contain surrogate-escaped text
                // and getenv returns its default by identity, regardless of its type.
                if cfg!(unix) && self.graph.command_startup.is_some() {
                    return Err(self.scoped_builtin_error("os.getenv", call));
                }
                return self.environ_get(call);
            }
            Some(Binding::Intrinsic(Intrinsic::Strerror)) => {
                self.reject_builtin_keywords(call)?;
                let [code] = call.arguments.args.as_ref() else {
                    return Err(error(self.source, call, "os.strerror requires one integer"));
                };
                return Ok(Expr {
                    ty: Type::Str,
                    kind: ExprKind::Builtin(
                        Builtin::process(ir::ProcessOperation::Strerror),
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
            Some(Binding::Intrinsic(Intrinsic::Chdir)) => {
                // The boxed provider performs path and fd conversion. Check the operands
                // here and keep checking subsequent expressions with its known None result.
                for arg in &call.arguments.args {
                    self.evaluated_argument(if let ast::Expr::Starred(star) = arg {
                        &star.value
                    } else {
                        arg
                    })?;
                }
                for keyword in &call.arguments.keywords {
                    self.evaluated_argument(&keyword.value)?;
                }
                let error = self.scoped_builtin_error("posix.chdir", call);
                self.defer_object_error(&error);
                return Ok(Expr::none());
            }
            Some(Binding::Intrinsic(Intrinsic::GetCwd { bytes })) => {
                self.arguments(call, &[], None)?;
                let op = ir::ProcessOperation::GetCwd { bytes };
                return Ok(Expr {
                    ty: if bytes { Type::Bytes } else { Type::Str },
                    kind: ExprKind::Builtin(Builtin::process(op), vec![]),
                });
            }
            Some(Binding::Intrinsic(Intrinsic::Popen)) => return self.popen(call, None),
            Some(Binding::Intrinsic(Intrinsic::PopenCleanup)) => {
                self.arguments(call, &[], None)?;
                return Ok(Expr {
                    ty: Type::None,
                    kind: ExprKind::Builtin(
                        Builtin::process(ir::ProcessOperation::PopenCleanup),
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
            Some(Binding::Intrinsic(Intrinsic::TracebackPrintExc)) => {
                if !call.arguments.args.is_empty() || !call.arguments.keywords.is_empty() {
                    return Err(self.object_export_error("traceback.print_exc", call));
                }
                let args = self.arguments(call, &[], None)?;
                return Ok(self.ordered_builtin(
                    Builtin::Logging(ir::LogOperation::PrintActiveException),
                    Type::None,
                    args,
                ));
            }
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
                | Intrinsic::HandlerNames
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
            Some(Binding::Intrinsic(Intrinsic::InspectCoroutine { mark })) => {
                return self.inspect_coroutine(call, mark);
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
            Some(Binding::Intrinsic(Intrinsic::MathFmod)) => return self.math_fmod(call),
            Some(Binding::Intrinsic(op)) if let Some((name, builtin)) = op.unary_float_math() => {
                self.reject_builtin_keywords(call)?;
                let [arg] = call.arguments.args.as_ref() else {
                    return Err(error(
                        self.source,
                        call,
                        &format!("{name} takes one number"),
                    ));
                };
                let value = self.expr(arg)?;
                return stdlib::unary_float_builtin(builtin, value).ok_or_else(|| {
                    error(
                        self.source,
                        arg,
                        &format!("{name} requires an integer or float"),
                    )
                });
            }
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
                        kind: ExprKind::Builtin(Builtin::Float(None), vec![value]),
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
            Some(Binding::Intrinsic(Intrinsic::RegexSearch)) => {
                return self.search_regex(call, false);
            }
            Some(Binding::Intrinsic(Intrinsic::RegexMatchStart)) => {
                return self.search_regex(call, true);
            }
            Some(Binding::Intrinsic(Intrinsic::Fnmatch(operation))) => {
                return self.fnmatch_call(operation, call);
            }
            Some(Binding::Intrinsic(Intrinsic::RegexSub)) => return self.sub_regex(call),
            Some(Binding::Intrinsic(Intrinsic::RegexRuns { split })) => {
                return self.regex_runs(call, split);
            }
            Some(Binding::Intrinsic(
                intrinsic @ (Intrinsic::GetSysconfigPath | Intrinsic::OsPathExists),
            )) => {
                return self.system_path_call(intrinsic, call);
            }
            Some(Binding::Intrinsic(Intrinsic::Dedent)) => {
                return self.dedent_call(call);
            }
            Some(Binding::Intrinsic(Intrinsic::BinasciiUnhexlify(name))) => {
                return self.binascii_unhexlify(call, name);
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
                let args = self.arguments(call, &[parameter("s", Type::Bytes, None)], None)?;
                return Ok(self.ordered_builtin(Builtin::UrlsafeB64encode, Type::Bytes, args));
            }
            Some(Binding::Intrinsic(Intrinsic::UrlJoin)) => return self.url_join(call),
            Some(Binding::Intrinsic(Intrinsic::ParseQsl)) => return self.parse_qsl(call),
            Some(Binding::Intrinsic(Intrinsic::UrlSplit)) => return self.url_split(call),
            Some(Binding::Intrinsic(Intrinsic::UrlUnsplit)) => return self.url_unsplit(call),
            Some(Binding::Intrinsic(Intrinsic::UrlEncode)) => return self.url_encode(call),
            Some(Binding::Intrinsic(Intrinsic::QuoteFromBytes)) => {
                return self.url_quote_from_bytes(call);
            }
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
        )) = binding
        {
            return self.directory_manager(kind, call);
        }
        if matches!(binding, Some(Binding::Intrinsic(Intrinsic::SubprocessRun))) {
            return self.process_run(call);
        }
        if let Some(Binding::Intrinsic(Intrinsic::ShellOutput { status })) = binding {
            return self.shell_output(call, status);
        }
        if binding == Some(Binding::Intrinsic(Intrinsic::CheckOutput)) {
            return self.process_check_output(call);
        }
        if let ast::Expr::Attribute(attr) = call.func.as_ref() {
            let base = self.resolve(&attr.value);
            if let Some(Binding::Intrinsic(
                mapping @ (Intrinsic::Environ | Intrinsic::EnvironBytes),
            )) = base
            {
                return self.environ_method(attr, call, mapping == Intrinsic::EnvironBytes);
            }
            if base
                == Some(Binding::Intrinsic(Intrinsic::Calendar(
                    ir::CalendarType::DateTime,
                )))
            {
                match attr.attr.as_str() {
                    "fromisoformat" => return self.datetime_fromisoformat(call),
                    "now" => return self.calendar_now(call),
                    "combine" => return self.calendar_combine(call),
                    _ => {}
                }
            }
            self.calendar_class_attribute(attr, base)?;
        }
        if let Some(class) = self.class_reference(&call.func) {
            let result = self.construct(call, class);
            if result.is_err() && self.native_mock(class) && self.boxed_mock_arguments(call)? {
                return Err(self.object_runtime_export("configured unittest.mock.Mock", call));
            }
            return result;
        }
        if let Some(Binding::Intrinsic(Intrinsic::PosixPath(operation))) = binding {
            return self.posix_path(call, operation);
        }
        if matches!(binding, Some(Binding::Intrinsic(Intrinsic::Path))) {
            return self.path_constructor(call, Type::Path);
        }
        if let Some(Binding::Intrinsic(Intrinsic::PurePath(flavor))) = binding {
            return self.path_constructor(call, Type::PurePath(flavor));
        }
        if matches!(binding, Some(Binding::Intrinsic(Intrinsic::JsonDecode))) {
            return self.json_decode(call);
        }
        if binding == Some(Binding::Intrinsic(Intrinsic::JsonEncoderClass)) {
            let flag = |name, default| {
                ParameterKind::KeywordOnly.parameter(name, Type::Bool, Some(Expr::boolean(default)))
            };
            let args = self.arguments(
                call,
                &[
                    flag("ensure_ascii", true),
                    flag("allow_nan", true),
                    flag("sort_keys", false),
                ],
                None,
            )?;
            return Ok(self.ordered_builtin(Builtin::JsonEncoderNew, Type::JsonEncoder, args));
        }
        if binding == Some(Binding::Intrinsic(Intrinsic::JsonDecoderClass)) {
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
            binding
        {
            return self.json_loads(call, operation == Intrinsic::JsonLoad);
        }
        if matches!(binding, Some(Binding::Intrinsic(Intrinsic::TomlDecode))) {
            return self.toml_decode(call);
        }
        if let Some(Binding::Intrinsic(
            op @ (Intrinsic::JsonEncode | Intrinsic::JsonDumps | Intrinsic::JsonDump),
        )) = binding
        {
            return self.json_encode(call, op);
        }
        if binding.is_some_and(Binding::is_registered_object) {
            self.require_registered_operation(call.func.as_ref())?;
            return self.call_with_target(call, Expr::unrepresented());
        }
        let gzip = binding == Some(Binding::Intrinsic(Intrinsic::GzipOpen));
        let imported_open = binding == Some(Binding::Intrinsic(Intrinsic::BuiltinsOpen))
            && match &*call.func {
                ast::Expr::Name(name) => !self.names.contains_key(name.id.as_str()),
                ast::Expr::Attribute(attr) => attr
                    .value
                    .as_name_expr()
                    .is_some_and(|name| !self.names.contains_key(name.id.as_str())),
                _ => false,
            };
        if gzip || imported_open || self.unshadowed_builtin(&call.func, "open") {
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
                let result = self
                    .classes
                    .declarations
                    .subclass(self.graph, child, parent);
                return Ok(Expr::boolean(result));
            }
            if let [child, parent] = call.arguments.args.as_ref()
                && call.arguments.keywords.is_empty()
                && let Some(Binding::Definition(parent)) = self.resolve(parent)
                && self.graph.is_abstract_context_manager(parent)
                && let Some(local) = self.local_class(child)?
            {
                let nominal = local.base.is_some_and(|base| {
                    let classes = &self.classes.declarations;
                    classes.subclass(self.graph, base, classes.ids[&parent])
                });
                let protocol = ["__enter__", "__exit__"]
                    .iter()
                    .all(|name| local.attributes.get(*name) == Some(&Some(true)));
                return Ok(Expr::boolean(nominal || protocol));
            }
            if let Some(answer) = self.mapping_issubclass(call)? {
                return Ok(answer);
            }
        }
        if self.unshadowed_builtin(&call.func, "isinstance") {
            return self.isinstance(call);
        }
        if self.unshadowed_builtin(&call.func, "issubclass") {
            return self.issubclass(call);
        }
        if let Some(kind) = self.exception_category(&call.func) {
            return self.simple_exception(call, kind);
        }
        if let Some(binding) = binding {
            if binding == Binding::Intrinsic(Intrinsic::BuiltinsPrint) {
                return self.named_builtin_call(call, "print");
            }
            if matches!(binding, Binding::Value(_) | Binding::ImportedValue(..))
                || binding == Binding::Intrinsic(Intrinsic::DefaultDict)
            {
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
                .ok_or_else(|| error(self.source, call, "definition is not a callable function"))?;
            if self.classes.producer_items.contains_key(&signature.id) {
                return Err(error(
                    self.source,
                    call,
                    "generator functions require direct iteration",
                ));
            }
            let value = self.source_call(call, signature, None, 0)?;
            self.record_named_call(call, &value);
            return Ok(value);
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
            if let Some(class) = self.class_reference(&attribute.value) {
                if let Some(e) = self.exception_alias_call(call, attribute, class, None)? {
                    return Ok(e);
                }
                return self.class_method_call(call, attribute, class, None);
            }
            if binding.is_none()
                && self.resolve(&attribute.value)
                    == Some(Binding::IntrinsicModule(
                        crate::modules::IntrinsicModule::Threading,
                    ))
            {
                return Err(error(
                    self.source,
                    attribute,
                    &format!(
                        "unsupported sealed standard-library export threading.{}",
                        attribute.attr
                    ),
                ));
            }
            if attribute.attr.as_str() == "from_bytes"
                && self.unshadowed_builtin(&attribute.value, "int")
                && call.arguments.keywords.is_empty()
                && let [input] = call.arguments.args.as_slice()
                && self.graph.session.scalar_type(self.module, input) == Some(Type::Bytes)
            {
                let value = self.expr(input)?;
                if value.ty == Type::Bytes {
                    return Ok(Expr {
                        ty: Type::Int,
                        kind: ExprKind::Builtin(Builtin::IntFromBytes, vec![value]),
                    });
                }
            }
            if attribute.attr.as_str() == "__hash__"
                && self.unshadowed_builtin(&attribute.value, "int")
            {
                let [arg] = call.arguments.args.as_slice() else {
                    return Err(error(
                        self.source,
                        call,
                        "int.__hash__ requires one receiver",
                    ));
                };
                if !call.arguments.keywords.is_empty() {
                    return Err(error(
                        self.source,
                        call,
                        "int.__hash__ takes no keyword arguments",
                    ));
                }
                let value = self.expr(arg)?;
                let receiver = self.int_method_receiver(value, arg)?;
                return Ok(Expr {
                    ty: Type::Int,
                    kind: ExprKind::Builtin(Builtin::IntHash, vec![receiver]),
                });
            }
            self.check_namedtuple_asdict(attribute, None)?;
            let receiver = match receiver {
                Some(value) => value,
                None => self.expr(&attribute.value)?,
            };
            if attribute.attr.as_str() == "count"
                && let ExprKind::Builtin(Builtin::Path(ir::PathOperation::Parts), mut operands) =
                    receiver.kind
            {
                self.reject_builtin_keywords(call)?;
                let [component] = call.arguments.args.as_ref() else {
                    return Err(error(
                        self.source,
                        call,
                        "parts.count expects one component",
                    ));
                };
                operands.push(self.expr_expected(component, Type::Str)?);
                return Ok(paths::operation(
                    ir::PathOperation::CountPart,
                    Type::Int,
                    operands,
                ));
            }
            if receiver.ty == Type::PythonObject {
                return self.call_object_method(call, attribute, receiver);
            }
            self.check_namedtuple_asdict(attribute, Some(receiver.ty))?;
            if let Some(result) =
                self.url_result_method(&receiver, attribute.attr.as_str(), call)?
            {
                return Ok(result);
            }
            if let Some(layout) = self.types.dispatcher(receiver.ty) {
                return self.single_dispatch_method(call, attribute, receiver, layout);
            }
            if let Some(wrapped) = self.callable_attribute(&receiver, attribute)? {
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
            if attribute.attr.as_str() == "zfill"
                && matches!(
                    receiver.ty,
                    Type::Str | Type::StrLiteral(_) | Type::StrEnum(_)
                )
            {
                let copy = matches!(receiver.ty, Type::StrEnum(_));
                return self.zfill(Self::string_value(receiver), call, copy);
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
            if receiver.ty == Type::ByteArray {
                let mut args = vec![receiver];
                let method = match (
                    attribute.attr.as_str(),
                    call.arguments.args.as_ref(),
                    call.arguments.keywords.as_ref(),
                ) {
                    ("append", [arg], []) => {
                        let value = self.expr(arg)?;
                        if !matches!(value.ty, Type::Int | Type::Bool | Type::IntEnum(_)) {
                            return Err(self.object_builtin_error("bytearray.append", call));
                        }
                        args.push(self.index_value(value, arg)?);
                        Builtin::ByteArrayAppend
                    }
                    ("extend", [arg], []) => {
                        let value = self.expr(arg)?;
                        if !matches!(value.ty, Type::Bytes | Type::ByteArray) {
                            return Err(self.object_builtin_error("bytearray.extend", call));
                        }
                        args.push(value);
                        Builtin::ByteArrayExtend
                    }
                    ("reverse", [], []) => Builtin::ByteArrayReverse,
                    _ => return Err(self.object_builtin_error("bytearray method", call)),
                };
                return Ok(Expr {
                    ty: Type::None,
                    kind: ExprKind::Builtin(method, args),
                });
            }
            if let Some(element) = self.types.deque_element(receiver.ty) {
                return self.deque_method(call, attribute, receiver, element);
            }
            if matches!(
                receiver.ty,
                Type::RegexPattern
                    | Type::RegexMatch
                    | Type::RegexBytesPattern
                    | Type::RegexBytesMatch
            ) || matches!(
                self.types.payload(receiver.ty),
                Some(Type::RegexMatch | Type::RegexBytesMatch)
            ) {
                return self.regex_method(receiver, attribute, call);
            }
            if matches!(receiver.ty, Type::Str | Type::StrLiteral(_))
                && attribute.attr.as_str() == "encode"
            {
                return self.str_encode(Self::string_value(receiver), call);
            }
            if receiver.ty == Type::Bytes && attribute.attr.as_str() == "decode" {
                if call.arguments.keywords.is_empty() {
                    let builtin = match call.arguments.args.as_ref() {
                        [] => Some(Builtin::Decode),
                        [ast::Expr::StringLiteral(s)] => codecs::bytes_decoder(s.value.to_str())
                            .filter(|op| matches!(op, Builtin::Decode | Builtin::DecodeAscii)),
                        _ => None,
                    };
                    if let Some(builtin) = builtin {
                        return Ok(Expr {
                            ty: Type::Str,
                            kind: ExprKind::Builtin(builtin, vec![receiver]),
                        });
                    }
                }
                if !call.arguments.args.is_empty()
                    || call
                        .arguments
                        .keywords
                        .iter()
                        .any(|kw| kw.arg.as_deref() == Some("encoding"))
                {
                    let params = [
                        parameter("encoding", Type::Str, None),
                        parameter("errors", Type::Str, Some(unpacking::string("strict"))),
                    ];
                    self.arguments(call, &params, Some(receiver))?;
                    return Err(self.scoped_builtin_error("bytes.decode", call));
                }
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
            let method = attribute.attr.as_str();
            if receiver.ty == Type::Float && method == "hex" {
                return Err(self.scoped_builtin_error("float.hex", call));
            }
            if receiver.ty == Type::Float && method == "is_integer" && call.arguments.is_empty() {
                return Ok(Expr {
                    ty: Type::Bool,
                    kind: ExprKind::Builtin(Builtin::FloatIsInteger, vec![receiver]),
                });
            }
            if (method == "to_bytes"
                || (matches!(method, "bit_count" | "bit_length" | "__hash__")
                    && call.arguments.args.is_empty()
                    && call.arguments.keywords.is_empty()))
                && (matches!(receiver.ty, Type::Int | Type::Bool)
                    || self.types.is_bool_or_int(receiver.ty))
            {
                let receiver = self.int_method_receiver(receiver, attribute)?;
                if method != "to_bytes" {
                    let builtin = match method {
                        "__hash__" => Builtin::IntHash,
                        "bit_count" => Builtin::IntBitCount,
                        _ => Builtin::IntBitLength,
                    };
                    return Ok(Expr {
                        ty: Type::Int,
                        kind: ExprKind::Builtin(builtin, vec![receiver]),
                    });
                }
                let params = [
                    parameter("length", Type::Int, Some(Expr::integer(1))),
                    parameter("byteorder", Type::Str, Some(unpacking::string("big"))),
                    ParameterKind::KeywordOnly.parameter(
                        "signed",
                        Type::Bool,
                        Some(Expr::boolean(false)),
                    ),
                ];
                let args = self
                    .arguments(call, &params, Some(receiver))
                    .map_err(|_| self.object_runtime_export("int.to_bytes", call))?;
                return Ok(self.ordered_builtin(Builtin::IntToBytes, Type::Bytes, args));
            }
            if receiver.ty == Type::Hash {
                return self.hash_method(receiver, attribute, call);
            }
            if receiver.ty == Type::JsonEncoder && attribute.attr.as_str() == "encode" {
                return self.json_encoder_method(receiver, call);
            }
            if receiver.ty == Type::JsonDecoder && attribute.attr.as_str() == "decode" {
                let args =
                    self.arguments(call, &[parameter("s", Type::Str, None)], Some(receiver))?;
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
            if calendar::boxed_member(receiver.ty, attribute.attr.as_str()) {
                return Err(self.object_runtime_export("datetime instance member", attribute));
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
                "splitlines"
                    | "title"
                    | "swapcase"
                    | "isalnum"
                    | "isascii"
                    | "isalpha"
                    | "isdigit"
                    | "isnumeric"
                    | "isspace"
                    | "islower"
                    | "istitle"
                    | "isupper"
            ) && matches!(receiver.ty, Type::Str | Type::StrLiteral(_))
            {
                let value = self.splitlines_or_unary_text(
                    Self::string_value(receiver),
                    attribute.attr.as_str(),
                    call,
                )?;
                return Ok(self.bound_text_result(call, attribute, value));
            }
            if attribute.attr.as_str() == "expandtabs"
                && matches!(receiver.ty, Type::Str | Type::StrLiteral(_))
            {
                return self.expandtabs(Self::string_value(receiver), call);
            }
            if matches!(attribute.attr.as_str(), "lower" | "upper" | "capitalize")
                && matches!(receiver.ty, Type::Str | Type::StrLiteral(_))
            {
                let value =
                    self.text_method(Self::string_value(receiver), attribute.attr.as_str(), call)?;
                return Ok(self.bound_text_result(call, attribute, value));
            }
            if matches!(receiver.ty, Type::Str | Type::StrLiteral(_))
                && let Some(method) = stdlib::registered_string_method(call)
            {
                return self.registered_method_call(
                    call,
                    method,
                    Self::string_value(receiver),
                    Type::Str,
                );
            }
            if attribute.attr.as_str() == "format"
                && matches!(receiver.ty, Type::Str | Type::StrLiteral(_))
            {
                return self.string_format_call(Self::string_value(receiver), call);
            }
            if attribute.attr.as_str() == "translate"
                && matches!(receiver.ty, Type::Str | Type::StrLiteral(_))
            {
                return self.translate_method(Self::string_value(receiver), call);
            }
            if receiver.ty == Type::Str {
                match attribute.attr.as_str() {
                    "join" => return self.join_call(receiver, call),
                    "replace" => {
                        let value = self.replace_call(receiver, call)?;
                        return Ok(self.bound_text_result(call, attribute, value));
                    }
                    "split" | "rsplit" | "strip" | "lstrip" | "rstrip" | "startswith"
                    | "endswith" | "find" | "partition" => {
                        let value = self.text_method(receiver, attribute.attr.as_str(), call)?;
                        return Ok(self.bound_text_result(call, attribute, value));
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
                let [arg] = call.arguments.args.as_ref() else {
                    return Err(self.object_builtin_error("set.union", call));
                };
                if !call.arguments.keywords.is_empty() || arg.is_starred_expr() {
                    return Err(self.object_builtin_error("set.union", call));
                }
                let (other, other_key) = self.union_operand(arg)?;
                if other_key != Some(key) {
                    return Err(self.object_builtin_error("set.union", call));
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
            if attribute.attr.as_str() == "fromkeys" && self.types.mapping(receiver.ty).is_some() {
                return self.dict_fromkeys(call, None, Some(receiver));
            }
            if receiver.ty == Type::BinaryIO && attribute.attr.as_str() == "writelines" {
                return Err(self.object_builtin_error("binary writelines", call));
            }
            if attribute.attr.as_str() == "__getitem__"
                && let Some((key, value)) = self.types.mapping(receiver.ty)
                && receiver.ty == self.types.dict(key, value)
            {
                if let [index] = call.arguments.args.as_ref()
                    && call.arguments.keywords.is_empty()
                {
                    return self.dict_subscript(receiver, index, key, value);
                }
                let mut scope = calls::ArgumentScope::default();
                scope.hold(self, receiver);
                let error = self.one_positional_error(call, "dict.__getitem__", "key", value)?;
                return Ok(scope.finish(error));
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
                        _ => Expr::integer(0),
                    };
                    return Ok(Expr {
                        ty: Type::Int,
                        kind: ExprKind::Builtin(
                            Builtin::BinarySeek,
                            vec![receiver, offset, whence],
                        ),
                    });
                }
                // BytesIO has no raw device to refill, so read1 uses its ordinary memory read.
                let method = match (receiver.ty, attribute.attr.as_str()) {
                    (Type::BytesIO, "read1") => "read",
                    (_, method) => method,
                };
                let (builtin, ty) = match (method, call.arguments.args.len()) {
                    ("getbuffer", _) if receiver.ty == Type::BytesIO => {
                        return Err(self.object_builtin_error("BytesIO.getbuffer", call));
                    }
                    ("readinto", _) if binary => {
                        return Err(self.object_builtin_error("binary readinto", call));
                    }
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
                        return self.unsupported_method_call(
                            call,
                            attribute,
                            receiver,
                            "supported file methods are read(), binary read(size), readline(), write(data), flush(), and close()",
                        );
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
                && attribute.attr.as_str() == "hex"
                && call.arguments.is_empty()
            {
                return Ok(Expr {
                    ty: Type::Str,
                    kind: ExprKind::Builtin(Builtin::BytesHex, vec![receiver]),
                });
            }
            if let Some((key_type, value_type)) = self.types.mapping(receiver.ty) {
                if attribute.attr.as_str() == "popitem" {
                    return self.dict_popitem(call, receiver, &call.arguments.args);
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
                if matches!(attribute.attr.as_str(), "keys" | "values" | "items")
                    && call.arguments.is_empty()
                {
                    return Err(self.scoped_builtin_error("dictionary view", call));
                }
                let pop = attribute.attr.as_str() == "pop";
                if (!pop && attribute.attr.as_str() != "get")
                    || !(1..=2).contains(&call.arguments.args.len())
                {
                    return Err(error(
                        self.source,
                        call,
                        "supported dictionary methods are clear(), copy(), update(), get(key[, default]), pop(key[, default]), popitem() on plain dictionaries, loop-head items(), and keys() in a key union",
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
            if let Some(method) = self.boxed_set_method(attribute, Some(receiver.ty)) {
                return Err(self.object_builtin_error(method, call));
            }
            if let Some(element) = self.types.set_element(receiver.ty) {
                let builtin = match attribute.attr.as_str() {
                    "add" if call.arguments.args.len() == 1 => Builtin::SetAdd,
                    "discard" if call.arguments.args.len() == 1 => Builtin::SetDiscard,
                    _ => {
                        return self.unsupported_method_call(
                            call,
                            attribute,
                            receiver,
                            "supported set methods are add(value) and discard(value)",
                        );
                    }
                };
                let stopped = self.terminating_operand(&receiver);
                let value = self.expr_in_context(
                    &call.arguments.args[0],
                    element,
                    |_, _| stopped,
                    Self::coerce,
                    self.value_declaration(&receiver),
                )?;
                if stopped {
                    // The operand was checked, but the method never consumes it.
                    return Ok(calls::ArgumentScope::stopped(
                        self,
                        Vec::new(),
                        receiver,
                        Type::None,
                    ));
                }
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
            let Some(element) = self.types.element(receiver.ty) else {
                if self
                    .graph
                    .session
                    .missing_literal_attribute(self.module, attribute)
                {
                    return self.registered_method_call(
                        call,
                        attribute,
                        receiver,
                        Type::ObjectValue,
                    );
                }
                if attribute.attr.as_str() == "__repr__"
                    && self.unshadowed_builtin(&attribute.value, "object")
                    && self.graph.session.is_known_class(
                        self.module,
                        &attribute.value,
                        ty_python_semantic::types::KnownClass::Object,
                    )
                    && registered::values::fixed_positional_arguments(call)
                    && let [arg] = call.arguments.args.as_slice()
                {
                    let value = self.expr(arg)?;
                    if matches!(value.ty, Type::Str | Type::StrLiteral(_)) {
                        return Ok(Expr {
                            ty: Type::Str,
                            kind: ExprKind::Builtin(
                                Builtin::BaseObjectRepr,
                                vec![Self::string_value(value)],
                            ),
                        });
                    }
                }
                return Err(error(
                    self.source,
                    call,
                    "supported value methods are typed-list append/pop, dict.get, set.add/discard, and str.split()",
                ));
            };
            let append = if self.joined_lines.writes_buffer(call.range()) {
                Builtin::JoinBufferAppend
            } else {
                Builtin::Append
            };
            if attribute.attr.as_str() == "extend" && call.arguments.args.len() == 1 {
                return self.extend_list(receiver, &call.arguments.args[0], call, append);
            }
            let declaration = self.value_declaration(&receiver);
            let mut args = Vec::new();
            let (builtin, ty) = match attribute.attr.as_str() {
                "append" if call.arguments.args.len() == 1 => {
                    let arg = &call.arguments.args[0];
                    let value = if let Some(inference) = self.inference
                        && let Some(ValueSource::InferredList(owner)) = self.value_source(&receiver)
                    {
                        self.expr_in_context(
                            arg,
                            element,
                            |_, _| false,
                            |checker, value, expected, site| {
                                if checker
                                    .types
                                    .observe_module_list_use(owner, value.ty == Type::Str)
                                {
                                    inference.layout_changed();
                                }
                                checker.coerce(value, expected, site)
                            },
                            declaration,
                        )?
                    } else if let ExprKind::Local(slot) = receiver.kind
                        && let Some(&site) = self.document_list_sites.get(&slot)
                        && let Some(inference) = self.inference
                        && element != documents::LIST
                    {
                        let value =
                            self.expr_expected_if(arg, element, |_, ty| ty == documents::LIST)?;
                        if value.ty == documents::LIST
                            && self
                                .types
                                .record_document_lists(self.module, site, self.class)
                        {
                            inference.layout_changed();
                        }
                        self.coerce(value, element, arg)?
                    } else {
                        self.expr_expected_from(arg, element, declaration)?
                    };
                    args.push(value);
                    (append, Type::None)
                }
                "pop" if call.arguments.args.is_empty() => (Builtin::Pop, element),
                "clear" if call.arguments.args.is_empty() => (Builtin::ListClear, Type::None),
                _ => {
                    return self.unsupported_method_call(
                        call,
                        attribute,
                        receiver,
                        "supported list methods are append(value), extend(iterable), pop(), and clear()",
                    );
                }
            };
            if self.checked_call_contracts && !matches!(builtin, Builtin::JoinBufferAppend) {
                self.frame_arguments = FrameArguments::Python;
                return Ok(registered::values::bound_call(
                    call,
                    registered::values::method_target(attribute),
                    receiver,
                    args,
                ));
            }
            args.insert(0, receiver);
            let value = Expr {
                ty,
                kind: ExprKind::Builtin(builtin, args),
            };
            // Only the joined-lines rewrite substitutes a private buffer for the list.
            if let Some(sites) = &mut self.fixed_sites
                && !matches!(value.kind, ExprKind::Builtin(Builtin::JoinBufferAppend, _))
                && registered::values::fixed_positional_arguments(call)
            {
                sites.method(call, attribute, &value);
            }
            return Ok(value);
        }
        let ast::Expr::Name(name) = &*call.func else {
            return Err(error(
                self.source,
                call,
                "only statically bound direct calls are supported",
            ));
        };
        self.named_builtin_call(call, name.id.as_str())
    }

    fn named_builtin_call(&mut self, call: &ast::ExprCall, name: &str) -> Result<Expr, String> {
        const UNSUPPORTED_ARGUMENTS: &str = "unknown function or unsupported builtin arguments";

        // Passing stderr as a positional value displays the open text stream.
        if matches!(name, "print" | "repr" | "str")
            && call.arguments.keywords.is_empty()
            && call
                .arguments
                .args
                .iter()
                .any(|arg| self.resolve(arg) == Some(Binding::Intrinsic(Intrinsic::Stderr)))
            && (name == "print" || call.arguments.args.len() == 1)
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
            return Ok(if name == "print" {
                Expr {
                    ty: Type::None,
                    kind: ExprKind::Builtin(Builtin::Print, vec![text]),
                }
            } else {
                text
            });
        }
        if name == "print"
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
        if self.unshadowed_builtin(&call.func, "tuple")
            && call.arguments.keywords.is_empty()
            && let [source @ ast::Expr::Generator(_)] = call.arguments.args.as_ref()
        {
            return self.collect_tuple([(source, Some(source))]);
        }
        match name {
            "list" => return self.list_call(call, None),
            "sorted" => return self.sorted_call(call, None),
            "abs" => return self.abs_call(call),
            "pow" => return self.pow_call(call),
            "next" => return self.next_cursor(call),
            "iter" if call.arguments.args.len() == 1 => return self.iter_cursor(call),
            "sum" => return self.sum_call(call),
            "min" | "max" => return self.extremum_call(call, name == "min"),
            "any" | "all" => return self.quantifier_call(call, name == "all"),
            "map" if self.unshadowed_builtin(&call.func, name) => {
                return self.bind_registered_builtin_call(call, name, Vec::new());
            }
            "map" | "filter" | "zip" | "reversed" | "enumerate" | "range" => {
                return Err(error(
                    self.source,
                    call,
                    "iterator values require an immediate for, comprehension, list, sum, any, or all consumer",
                ));
            }
            _ => {}
        }
        if name == "ord" && !self.names.contains_key("ord") && self.resolve(&call.func).is_none() {
            return self.ordinal_call(call);
        }
        if name == "ascii"
            && !self.names.contains_key("ascii")
            && self.resolve(&call.func).is_none()
        {
            return self.ascii_call(call);
        }
        let keywords = self.reject_builtin_keywords(call);
        if name == "bool" {
            keywords?;
            return match call.arguments.args.as_ref() {
                [] => Ok(Expr::boolean(false)),
                [value] => self.condition_at(value, Some(call.into())),
                _ => Err(error(
                    self.source,
                    call,
                    "bool accepts zero or one positional argument",
                )),
            };
        }
        if name == "bytes" {
            keywords?;
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
                        "bytes accepts zero arguments or one int, bytes, bytearray, integer list, or integer tuple argument",
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
            if !matches!(
                value.ty,
                Type::Int | Type::ListInt | Type::Bytes | Type::ByteArray
            ) {
                return Err(error(
                    self.source,
                    call,
                    "bytes expects an int, bytes, bytearray, integer list, or integer tuple",
                ));
            }
            return Ok(Expr {
                ty: Type::Bytes,
                kind: ExprKind::Builtin(Builtin::Bytes, vec![value]),
            });
        }
        let tuple_dir = self.unshadowed_builtin(&call.func, "dir")
            && call.arguments.keywords.is_empty()
            && call.arguments.args.len() == 1;
        if tuple_dir {
            // Static tuple declarations may be used as constructors but not as first-class values.
            let target = &call.arguments.args[0];
            if self.unshadowed_builtin(target, "tuple")
                || matches!(
                    self.namedtuple_factory(self.resolve(target)),
                    Some((crate::types::TupleOrigin::Declaration(_), _))
                )
            {
                return Err(self.object_builtin_error("dir", call));
            }
        }
        let mut args = Vec::with_capacity(call.arguments.args.len());
        for arg in &call.arguments.args {
            let value = match self.expr(arg) {
                Ok(value) => value,
                // Preserve this binder's original refusal on the live path.
                Err(failure) => return Err(keywords.err().unwrap_or(failure)),
            };
            if self.terminating_operand(&value)
                && (imre_ast::providers::BUILTIN_FUNCTIONS.contains(&name)
                    || imre_ast::providers::BUILTIN_TYPES.contains(&name))
                && let Some(returns) = self
                    .graph
                    .session
                    .operand_types(self.module)
                    .stopped_call_result(call)
            {
                return Ok(calls::ArgumentScope::stopped(self, args, value, returns));
            }
            args.push(value);
        }
        keywords?;
        if name == "str" && args.len() == 1 {
            return self.checked_string(args.into_iter().next().unwrap(), call, true);
        }
        if self.unshadowed_builtin(&call.func, "str")
            && call.arguments.keywords.is_empty()
            && let [bytes, encoding] = args.as_slice()
            && bytes.ty == Type::Bytes
            && let ExprKind::Str(name) = &encoding.kind
            && let Some(builtin @ (Builtin::DecodeAscii | Builtin::DecodeLatin1)) =
                codecs::bytes_decoder(name)
        {
            // str(bytes, encoding) decodes the buffer even when bytes.decode is overridden.
            args.truncate(1);
            return Ok(Expr {
                ty: Type::Str,
                kind: ExprKind::Builtin(builtin, args),
            });
        }
        if tuple_dir && self.types.is_plain_or_declared_tuple(args[0].ty) {
            return Err(self.object_builtin_error("dir", call));
        }
        if let [operand] = args.as_slice()
            && matches!(
                operand.ty,
                Type::Str | Type::StrLiteral(_) | Type::Bytes | Type::ByteArray
            )
            && self.unshadowed_builtin(&call.func, "type")
        {
            let class = match operand.ty {
                Type::Bytes => "bytes",
                Type::ByteArray => "bytearray",
                _ => "str",
            };
            return Ok(partials::after(
                operand.clone(),
                annotations::class_value(class),
            ));
        }
        if let [proxy] = args.as_slice()
            && matches!(proxy.ty, Type::WeakProxy(_))
            && let Some(value) = self.weak_proxy_builtin(name, proxy.clone())
        {
            return Ok(value);
        }
        if self.unshadowed_builtin(&call.func, "tuple")
            && let [value] = args.as_slice()
            && let Some(tuple) = self.iterable_tuple(value)
        {
            return Ok(tuple);
        }
        if name == "tuple"
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
            let result = match name {
                "type" if kind == ir::PlaceholderKind::Value => {
                    Some(partials::singleton(ir::PlaceholderKind::Class))
                }
                "type" => annotations::builtin_type("type"),
                "callable" => Some(Expr::boolean(kind == ir::PlaceholderKind::Class)),
                _ => None,
            };
            if let Some(result) = result {
                return Ok(partials::after(value.clone(), result));
            }
        }
        if args.len() == 1
            && matches!(args[0].ty, Type::Calendar(_))
            && matches!(name, "print" | "repr")
        {
            return Ok(calendar::format_builtin(name, args));
        }
        if let [value] = args.as_slice()
            && let Some(result) = self.patch_builtin(name, value)
        {
            return Ok(result);
        }
        if name == "hash" {
            let [value] = args.as_slice() else {
                return Err(error(self.source, call, "hash requires one argument"));
            };
            if matches!(value.ty, Type::CmpKey(..)) {
                return Ok(self.cmp_key_hash(value.clone()));
            }
            // UUID hashes its integer value; the bool discriminator of UUID.int
            // affects its Python type but never participates in equality or hashing.
            let args = match value.ty {
                Type::Class(id)
                    if self.classes.declarations.definitions[id].is_uuid(self.graph)
                        && self.types.immutable_key_field(value.ty) =>
                {
                    let fields = &self.classes.declarations.definitions[id].fields;
                    let field = fields
                        .iter()
                        .position(|field| field.name == "_value")
                        .unwrap();
                    vec![Expr {
                        ty: Type::Int,
                        kind: ExprKind::Field(Box::new(value.clone()), field),
                    }]
                }
                ty if ty == Type::Bool || self.types.is_bool_or_int(ty) => {
                    vec![self.int_method_receiver(value.clone(), call)?]
                }
                _ => args,
            };
            let builtin = match args[0].ty {
                Type::Int => Builtin::IntHash,
                Type::Deque(_) => {
                    return Ok(calls::type_error(
                        Type::Int,
                        args,
                        "unhashable type: 'collections.deque'".into(),
                    ));
                }
                Type::Str | Type::StrLiteral(_) => Builtin::HashStr,
                Type::Bytes => Builtin::HashBytes,
                Type::Class(id) if self.fraction_class(id) => {
                    let fields = &self.classes.declarations.definitions[id].fields;
                    let index = |name| fields.iter().position(|field| field.name == name).unwrap();
                    Builtin::FractionHash(index("_numerator"), index("_denominator"))
                }
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
                        "hash requires a string, bytes, or a known frozen hash projection",
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
                && name == "print")
                || (args[0].ty == Type::PathInfo && matches!(name, "repr" | "print")))
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
            return Ok(if name != "print" {
                text
            } else {
                Expr {
                    ty: Type::None,
                    kind: ExprKind::Builtin(Builtin::Print, vec![text]),
                }
            });
        }
        if name == "int"
            && args.len() == 1
            && matches!(args[0].ty, Type::Union(_))
            && let Some(value) = self.int_enum_union_value(args[0].clone())
        {
            return Ok(value);
        }
        if args.len() == 1 && matches!(args[0].ty, Type::IntEnum(_)) && name == "int" {
            return Ok(Self::enum_integer(args.into_iter().next().unwrap()));
        }
        if let [class] = args.as_slice()
            && class.ty == Type::BuiltinClass
            && name == "type"
        {
            return Ok(
                self.evaluated_receiver(Some(class.clone()), annotations::class_value("type"))
            );
        }
        if args.len() == 1
            && matches!(args[0].ty, Type::Enum(_) | Type::BuiltinClass)
            && (name == "print" || (args[0].ty == Type::BuiltinClass && name == "repr"))
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
            return Ok(if name != "print" {
                text
            } else {
                Expr {
                    ty: Type::None,
                    kind: ExprKind::Builtin(Builtin::Print, vec![text]),
                }
            });
        }
        if args.len() == 1
            && (matches!(args[0].ty, Type::StrEnum(_)) && matches!(name, "print" | "len")
                || matches!(args[0].ty, Type::IntEnum(_)) && name == "print")
        {
            let text = Expr {
                ty: Type::Str,
                kind: ExprKind::Builtin(Builtin::StrEnumValue, args),
            };
            return Ok(match name {
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
        if name == "frozenset"
            && let [value] = args.as_slice()
            && value.ty == Type::FrozenStrings
        {
            return Ok(value.clone());
        }
        if name == "frozenset"
            && args.len() == 1
            && self.types.element(args[0].ty) == Some(Type::Str)
        {
            return Ok(Expr {
                ty: Type::FrozenStrings,
                kind: ExprKind::Builtin(Builtin::FreezeStrings, args),
            });
        }
        if name == "divmod" && args.len() == 2 {
            if args.iter().any(|arg| arg.ty == Type::Float) {
                if args
                    .iter()
                    .all(|arg| matches!(arg.ty, Type::Float | Type::Int))
                {
                    return Ok(Expr {
                        ty: self.types.tuple(vec![Type::Float, Type::Float]),
                        kind: ExprKind::Builtin(Builtin::FloatDivmod, args),
                    });
                }
                return Err(self.scoped_builtin_error("float divmod", call));
            }
            if args
                .iter()
                .all(|arg| arg.ty == Type::Calendar(ir::CalendarType::Delta))
            {
                return Err(self.object_runtime_export("timedelta divmod", call));
            }
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
            let op = match name {
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
        if name == "print" && args.len() == 1 && args[0].ty == Type::ObjectValue {
            let text = Expr {
                ty: Type::Str,
                kind: ExprKind::Builtin(Builtin::Str, args),
            };
            return Ok(Expr {
                ty: Type::None,
                kind: ExprKind::Builtin(Builtin::Print, vec![text]),
            });
        }
        if name == "print"
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
        if cfg!(unix)
            && let [value] = args.as_slice()
            && matches!(value.ty, Type::Popen(..) | Type::ActivePopen)
            && self.unshadowed_builtin(&call.func, "repr")
        {
            return Err(self.scoped_builtin_error("subprocess.Popen repr", call));
        }
        if name == "repr"
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
        if args.len() == 1 && args[0].ty == Type::LogRecord && matches!(name, "repr" | "print") {
            let text = logging_records::op(ir::LogRecordOperation::Repr, Type::Str, args);
            return Ok(if name == "print" {
                Expr {
                    ty: Type::None,
                    kind: ExprKind::Builtin(Builtin::Print, vec![text]),
                }
            } else {
                text
            });
        }
        if let [value] = args.as_slice()
            && name == "repr"
            && let Some(rendered) = self.mock_call_repr(value, call)?
        {
            return Ok(rendered);
        }
        if args.len() == 1
            && matches!(
                args[0].ty,
                Type::PlainObject | Type::StrDescriptor(_) | Type::Placeholder(_)
            )
            && matches!(name, "repr" | "print")
        {
            let rendered = Expr {
                ty: Type::Str,
                kind: ExprKind::Builtin(Builtin::Repr, args),
            };
            return Ok(if name == "print" {
                Expr {
                    ty: Type::None,
                    kind: ExprKind::Builtin(Builtin::Print, vec![rendered]),
                }
            } else {
                rendered
            });
        }
        if name == "hasattr"
            && let [receiver, key] = args.as_slice()
            && receiver.ty == Type::SysImplementation
            && self.resolve(&call.func).is_none()
            && !self.names.contains_key("hasattr")
        {
            self.reject_builtin_keywords(call)?;
            let key = self.coerce(key.clone(), Type::Str, &call.arguments.args[1])?;
            return Ok(self.implementation_hasattr(receiver.clone(), key));
        }
        if matches!(
            (name, args.len()),
            ("getattr", 2 | 3) | ("hasattr" | "delattr", 2) | ("setattr", 3)
        ) && self.unshadowed_builtin(&call.func, name)
            && matches!(args[0].ty, Type::Coroutine(_))
            && matches!(&args[1].kind, ExprKind::Str(member) if member == "cr_origin")
        {
            self.reject_builtin_keywords(call)?;
            return Err(self.object_builtin_error("coroutine.cr_origin", call));
        }
        if self.unshadowed_builtin(&call.func, "getattr")
            && let [receiver, attribute] | [receiver, attribute, _] = args.as_slice()
            && let ExprKind::Str(member) = &attribute.kind
            && (calendar::boxed_date_arithmetic(receiver.ty, member)
                || calendar::date_isoformat(receiver.ty, member)
                || (args.len() == 2 && calendar::boxed_comparison(receiver.ty, member)))
        {
            return Err(self.object_runtime_export("datetime instance member", call));
        }
        if matches!(name, "getattr" | "hasattr" | "vars")
            && args.first().is_some_and(|arg| arg.ty == Type::NativeModule)
            && self.resolve(&call.func).is_none()
            && !self.names.contains_key(name)
        {
            return self.errno_builtin(name, call, &args);
        }
        if matches!(name, "vars" | "hasattr" | "getattr")
            && self.unshadowed_builtin(&call.func, name)
            && let Some(value) = self.record_namespace_builtin(name, &args, call)?
        {
            return Ok(value);
        }
        if name == "vars"
            && let [receiver] = args.as_slice()
            && let Type::SimpleNamespace(id) = receiver.ty
        {
            return Ok(namespaces::backing(receiver.clone(), id));
        }
        if name == "repr" && args.len() == 1 && matches!(args[0].ty, Type::Class(_)) {
            return self.record_repr(args.into_iter().next().unwrap(), call);
        }
        if matches!(name, "print" | "repr")
            && let [value] = args.as_slice()
            && let Some(text) = self.structured_display(name, value, call)?
        {
            return Ok(if name == "print" {
                Expr {
                    ty: Type::None,
                    kind: ExprKind::Builtin(Builtin::Print, vec![text]),
                }
            } else {
                text
            });
        }
        if name == "len"
            && let [value] = args.as_slice()
            && self.mock_call_record(value.ty)
        {
            return Err(self.object_runtime_export("unittest.mock call tuple", call));
        }
        if name == "len"
            && let [value] = args.as_slice()
            && self.mapping_class(value.ty).is_some()
        {
            return self.mapping_length(value.clone(), call);
        }
        if name == "len" && matches!(args.as_slice(), [m] if m.ty == Type::ReadOnlyObjectMapping) {
            return Ok(Expr {
                ty: Type::Int,
                kind: ExprKind::Builtin(Builtin::Len, args),
            });
        }
        if name == "len"
            && let ([source], [set]) = (call.arguments.args.as_ref(), args.as_slice())
            && let Some(length) = self.short_set_len(source, set)
        {
            return Ok(length);
        }
        let (builtin, ty) = match name {
            "bytearray" if self.unshadowed_builtin(&call.func, "bytearray") => {
                if !args.is_empty() || !call.arguments.keywords.is_empty() {
                    return Err(self.object_builtin_error("bytearray", call));
                }
                // The scalar-default caller cache cannot replay object-provider eligibility.
                self.default_reads = None;
                self.graph.has_object_constructor_fallback.set(true);
                (Builtin::ByteArrayNew, Type::ByteArray)
            }
            "input" if args.is_empty() => (Builtin::Input, Type::Str),
            "input" if args.len() == 1 && self.unshadowed_builtin(&call.func, "input") => {
                return self.input_prompt(args.into_iter().next().unwrap(), call);
            }
            "chr" if args.len() == 1 && matches!(args[0].ty, Type::Int | Type::Bool) => {
                let value = args.into_iter().next().unwrap();
                let integer = if value.ty == Type::Int {
                    value
                } else {
                    Expr {
                        ty: Type::Int,
                        kind: ExprKind::Builtin(Builtin::Int(None), vec![value]),
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
            "type"
                if let [operand] = args.as_slice()
                    && let Type::Class(class) = operand.ty
                    && self.unshadowed_builtin(&call.func, "type") =>
            {
                return self.dispatch_record(class, operand.clone(), call, |checker, layout, _| {
                    Ok(checker.record_class_object(layout))
                });
            }
            "type"
                if let [value] = args.as_slice()
                    && self.has_plain_factory_metaclass(value.ty)
                    && self.unshadowed_builtin(&call.func, "type")
                    && registered::values::fixed_positional_arguments(call) =>
            {
                return Ok(
                    self.evaluated_receiver(Some(value.clone()), annotations::class_value("type"))
                );
            }
            "type"
                if let [exception] = args.as_slice()
                    && matches!(exception.ty, Type::Exception | Type::NamedException(_))
                    && self.unshadowed_builtin(&call.func, name)
                    && registered::values::fixed_positional_arguments(call) =>
            {
                // A handler can catch a subclass of its declared exception.
                return Ok(self.registered_builtin_call(call, name, args));
            }
            "type"
                if args.len() == 1
                    && args[0].ty == Type::Hash
                    && self.unshadowed_builtin(&call.func, "type") =>
            {
                (Builtin::HashType, Type::BuiltinClass)
            }
            name @ ("type" | "id")
                if args.len() == 1
                    && self.types.is_url_split_result(args[0].ty)
                    && self.unshadowed_builtin(&call.func, name) =>
            {
                return Err(
                    self.scoped_builtin_error("urllib.parse.urlsplit result identity", call)
                );
            }
            "id" if args.len() == 1
                && matches!(args[0].ty, Type::Hash | Type::VarTuple(_))
                && self.unshadowed_builtin(&call.func, "id") =>
            {
                (Builtin::PointerIdentity, Type::Int)
            }
            "id" if args.len() == 1
                && !self.names.contains_key("id")
                && self.resolve(&call.func).is_none()
                && matches!(args[0].ty, Type::Popen(..) | Type::ActivePopen) =>
            {
                (
                    Builtin::process(ir::ProcessOperation::PopenIdentity),
                    Type::Int,
                )
            }
            "int"
                if args.len() == 1
                    && matches!(args[0].ty, Type::Union(_))
                    && args[0].ty == self.types.string_or_int() =>
            {
                return Ok(self.integer_from_text_or_int(args.into_iter().next().unwrap(), call));
            }
            "int"
                if let [text, base] = args.as_slice()
                    && text.ty == Type::Str
                    && matches!(&base.kind, ExprKind::Int(n) if n == "16") =>
            {
                (
                    self.checked_builtin_call(Builtin::Int, call, &args),
                    Type::Int,
                )
            }
            "int"
                if args.len() == 1
                    && matches!(args[0].ty, Type::Int | Type::Float | Type::Bool | Type::Str) =>
            {
                (
                    self.checked_builtin_call(Builtin::Int, call, &args),
                    Type::Int,
                )
            }
            "float"
                if args.len() == 1
                    && matches!(args[0].ty, Type::Class(class) if self.decimal_class(class)) =>
            {
                return Err(self.scoped_builtin_error("Decimal.__float__", call));
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
                (
                    self.checked_builtin_call(Builtin::Float, call, &args),
                    Type::Float,
                )
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
                            | Type::ByteArray
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
                return Ok(Expr::sequence_len(args));
            }
            "print"
                if !args.is_empty()
                    && (args.len() == 1 || call.arguments.keywords.is_empty())
                    && args.iter().all(|arg| {
                        let scalar = |ty| {
                            matches!(
                                ty,
                                Type::Int
                                    | Type::Float
                                    | Type::Bool
                                    | Type::Str
                                    | Type::Bytes
                                    | Type::None
                            )
                        };
                        scalar(arg.ty)
                            || (args.len() > 1 && self.types.element(arg.ty).is_some_and(scalar))
                            || self
                                .types
                                .members(arg.ty)
                                .is_some_and(|members| members.into_iter().all(scalar))
                    }) =>
            {
                (Builtin::Print, Type::None)
            }
            "complex"
                if self.graph.command_startup.is_some()
                    && self.unshadowed_builtin(&call.func, "complex") =>
            {
                return Err(self.scoped_builtin_error("complex constructor", call));
            }
            "set" | "frozenset" if self.unshadowed_builtin(&call.func, name) => {
                return Err(self.object_builtin_error(name, call));
            }
            "exec" | "eval"
                if self.graph.session.is_stdlib_function(
                    self.module,
                    &call.func,
                    "builtins",
                    name,
                ) =>
            {
                return Err(self.classified_error(
                    Kind::ExecutionOperand(self.module),
                    call,
                    UNSUPPORTED_ARGUMENTS,
                ));
            }
            name if (imre_ast::providers::BUILTIN_FUNCTIONS.contains(&name)
                || imre_ast::providers::BUILTIN_TYPES.contains(&name))
                && self.unshadowed_builtin(&call.func, name)
                && registered::values::fixed_positional_arguments(call)
                && args
                    .iter()
                    .any(|value| self.types.stores_python_object(value.ty)) =>
            {
                return Ok(self.registered_builtin_call(call, name, args));
            }
            _ => return Err(error(self.source, call, UNSUPPORTED_ARGUMENTS)),
        };
        let value = Expr {
            ty,
            kind: ExprKind::Builtin(builtin, args),
        };
        self.record_named_call(call, &value);
        Ok(value)
    }

    /// Retain a written global lookup for the Python entry of a checked positional call.
    fn record_named_call(&mut self, call: &ast::ExprCall, value: &Expr) {
        if let Some(sites) = &mut self.fixed_sites
            && let Some(name) = call.func.as_name_expr()
            && self.namespace.is_none()
            && !self.names.contains_key(name.id.as_str())
            && !ruff_python_stdlib::identifiers::is_mangled_private(name.id.as_str())
            && call.start() == name.start()
            && self.source.get(name.range().to_std_range()) == Some(name.id.as_str())
        {
            sites.positional_call(call, value, || {
                Some(registered::values::located(
                    name.range(),
                    name.start(),
                    registered::values::name_binding(name.id.clone()),
                ))
            });
        }
    }

    /// Complete the checked arguments in the original positional and keyword order.
    fn bind_registered_builtin_call(
        &mut self,
        call: &ast::ExprCall,
        name: &str,
        mut args: Vec<Expr>,
    ) -> Result<Expr, String> {
        for argument in imre_ast::argument_values(&call.arguments).skip(args.len()) {
            args.push(self.expr(argument)?);
        }
        Ok(self.registered_builtin_call(call, name, args))
    }

    /// A fixed caller still checks the following operations before choosing its frame.
    fn require_builtin_scope(&self, name: &str, site: &impl Ranged) {
        if self.registered.is_none() && !self.globals.registered_initializers {
            self.unrepresented
                .borrow_mut()
                .get_or_insert_with(|| self.scoped_builtin_error(name, site));
        }
    }

    fn registered_builtin_call(&self, call: &ast::ExprCall, name: &str, args: Vec<Expr>) -> Expr {
        self.require_builtin_scope(name, call);
        self.discarded_call(Type::PythonObject)
            .unwrap_or_else(|| registered::values::name_call(call, name, args))
    }

    /// Text uses UTF-8 and defaults to reading; explicit modes stay literal.
    fn open(&mut self, call: &ast::ExprCall, gzip: bool) -> Result<Expr, String> {
        let args = &call.arguments;
        if gzip
            && let Some(ast::Expr::StringLiteral(mode)) = args.find_argument_value("mode", 1)
            && matches!(
                mode.value.to_str(),
                "w" | "wb" | "wt" | "a" | "ab" | "at" | "x" | "xb" | "xt"
            )
        {
            return Err(self.scoped_builtin_error("gzip.open", call));
        }
        // Sealed file handles close on drop without running Python's warning machinery.
        if !gzip
            && self
                .graph
                .command_startup
                .as_ref()
                .is_some_and(|(flags, _)| flags.has_warning_options)
        {
            return Err(self.object_builtin_error("open with startup warning filters", call));
        }
        if !gzip && args.find_argument_value("opener", 7).is_some() {
            return Err(self.object_builtin_error("open with opener", call));
        }
        // The typed BinaryIO ABI carries neither raw write results nor a buffer size.
        if !gzip
            && args.len() == 3
            && let Some(path) = args.find_argument_value("file", 0)
            && let Some(ast::Expr::StringLiteral(mode)) = args.find_argument_value("mode", 1)
            && let Some(size) = args.find_argument_value("buffering", 2)
            && paths::is_raw_binary_mode(mode.value.to_str())
            && (matches!(mode.value.to_str(), "wb" | "bw" | "ab" | "ba" | "xb" | "bx")
                || matches!(size, ast::Expr::NumberLiteral(size) if matches!(&size.value, ast::Number::Int(n) if n.as_i64() == Some(0))))
            && self.expr(path)?.ty == Type::Str
        {
            return Err(self.object_builtin_error("binary open with buffering", call));
        }
        let mut checked_path = None;
        if !gzip
            && let Some(ast::Expr::StringLiteral(mode)) = args.find_argument_value("mode", 1)
            && paths::is_update_mode(mode.value.to_str())
            && paths::has_open_arguments(args)
            && let Some(path) = args.find_argument_value("file", 0)
        {
            let value = self.expr(path)?;
            if matches!(
                value.ty,
                Type::Str | Type::Bytes | Type::Path | Type::PurePath(_)
            ) || (mode.value.to_str().contains('b') && value.ty == Type::Int)
            {
                return Err(self.scoped_builtin_error("buffered update open", call));
            }
            checked_path = Some(value);
        }
        if !gzip
            && let Some(mode) = args.find_argument_value("mode", 1)
            && !mode.is_string_literal_expr()
            && args.find_argument_value("file", 0).is_some()
            && paths::has_open_arguments(args)
        {
            let message = if mode.is_literal_expr() {
                "open mode must be a string"
            } else {
                "fixed-layout open requires a literal string mode to select its stream type. Use 'from io import open' for string modes selected at runtime"
            };
            return Err(error(self.source, mode, message));
        }
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
        let path = match checked_path {
            Some(value) => self.path_text(value, path)?,
            None => self.path_argument(path)?,
        };
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
                        let value = &keyword.value;
                        if !self.literal_encoding_matches(value, is_utf8_codec)? {
                            let message = if value.is_literal_expr()
                                && !value.is_string_literal_expr()
                                && !matches!(value, ast::Expr::NoneLiteral(_))
                            {
                                "open encoding must be a string or None"
                            } else {
                                "fixed-layout open requires encoding='utf-8'. Use 'from io import open' to pass another string encoding or None"
                            };
                            return Err(error(self.source, value, message));
                        }
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
        // The sealed gzip ABI supplies its own encoding spelling.
        self.utf8_encoding_keyword(encoding, |name| name == "utf-8")
    }

    fn utf8_encoding_keyword(
        &self,
        encoding: &ast::Keyword,
        accepts: fn(&str) -> bool,
    ) -> Result<(), String> {
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
        if !self.literal_encoding_matches(&encoding.value, accepts)? {
            return Err(error(
                self.source,
                encoding,
                "encoding must be the literal 'utf-8'",
            ));
        }
        Ok(())
    }

    fn literal_encoding_matches(
        &self,
        value: &ast::Expr,
        accepts: fn(&str) -> bool,
    ) -> Result<bool, String> {
        let ast::Expr::StringLiteral(literal) = value else {
            return Ok(false);
        };
        check_string(literal, self.source)?;
        Ok(accepts(literal.value.to_str()))
    }
}

/// Returns the names initialized by a suite of literal assignments. Each RHS
/// is independent of local bindings in either arm, including assignment order.
fn literal_binding_names(suite: &[ast::Stmt]) -> Option<std::collections::HashSet<&str>> {
    suite
        .iter()
        .map(|stmt| {
            let ast::Stmt::Assign(assign) = stmt else {
                return None;
            };
            let [ast::Expr::Name(name)] = assign.targets.as_slice() else {
                return None;
            };
            assign.value.is_literal_expr().then_some(name.id.as_str())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn checked_conversions_keep_their_operand_fault_sites() {
        use crate::ir::{Builtin, ExprKind, walk_expressions};
        use crate::modules::SourceGraph;

        let source = concat!(
            "def pick(text: str, index: int) -> int:\n",
            "    values = [text, 17]\n",
            "    return int(values[index])\n",
            "def field(text: str) -> int:\n",
            "    return int(text.split()[1])\n",
            "print(pick('1', 0), field('1 2'))\n",
        );
        let graph = SourceGraph::single(source).unwrap();
        let mut module = super::lower(&graph).unwrap();
        let mut sites = Vec::new();
        for f in &module.functions {
            walk_expressions(&f.body, &mut |value| {
                let (operation, site) = match &value.kind {
                    ExprKind::Builtin(Builtin::Int(Some(site)), _) => ("int", site),
                    ExprKind::Builtin(Builtin::ListGet(Some(site)), _) => ("index", site),
                    _ => return,
                };
                sites.push((site.line, operation, site.span));
            });
        }
        sites.sort();
        assert_eq!(
            sites,
            [
                (3, "index", Some([15, 21, 28])),
                (3, "int", None),
                (5, "index", Some([15, 27, 30])),
                (5, "int", None),
            ]
        );

        crate::split_fields::specialize(&mut module);
        let mut fused = Vec::new();
        for f in &module.functions {
            walk_expressions(&f.body, &mut |value| {
                if let ExprKind::Builtin(Builtin::SplitIntField(1, sites), _) = &value.kind {
                    let index = sites.index.as_ref().unwrap();
                    let conversion = sites.conversion.as_ref().unwrap();
                    fused.push((index.line, index.span, conversion.line, conversion.span));
                }
            });
        }
        assert_eq!(fused, [(5, Some([15, 27, 30]), 5, None)]);
    }

    #[test]
    fn fixed_open_diagnostics_preserve_literal_errors() {
        use crate::modules::SourceGraph;

        for (source, expected) in [
            ("mode = 'r'\nopen('data', mode)", "literal string mode"),
            ("open('data', None)", "open mode must be a string"),
            ("open('data', encoding=None)", "from io import open"),
            (
                "open('data', encoding=1)",
                "open encoding must be a string or None",
            ),
            (
                r"open('data', encoding='\ud800')",
                "surrogate string escapes",
            ),
        ] {
            let graph = SourceGraph::single_with_mode(source, true).unwrap();
            let error = super::lower(&graph).err().expect("invalid fixed open");
            assert!(error.contains(expected), "{source}: {error}");
        }
    }

    #[test]
    fn an_opaque_operand_cannot_hide_an_independent_bad_write() {
        for earlier in [
            "manager.value",
            "manager[0]",
            "lambda item: item + manager",
            "lambda item=None, *args, **kwargs: (item, args, kwargs)",
        ] {
            let source = format!(
                "def check(manager):\n    earlier = {earlier}\n    items: list[int] = [3]\n    items.append(True)\ndef produce():\n    return set()\ncheck(produce())\n"
            );
            let mut graph = crate::modules::SourceGraph::single(&source).unwrap();
            let Err(error) = super::lower_with_caller_cache(&mut graph, None, None) else {
                panic!("expected to reject the bad write after {earlier}");
            };
            assert_eq!(error.kind, super::Kind::ExactType);
            assert!(error.to_string().contains(":4:18:"));
        }
    }

    #[test]
    fn a_python_context_still_checks_its_body_and_successors() {
        for (body, line) in [
            (
                "        items: list[int] = [3]\n        items.append(True)\n",
                4,
            ),
            (
                "        pass\n    items: list[int] = [3]\n    items.append(True)\n",
                5,
            ),
        ] {
            let source = format!(
                "def check(manager):\n    with manager as stream:\n{body}def get_context():\n    return set()\ncheck(get_context())\n"
            );
            let mut graph = crate::modules::SourceGraph::single(&source).unwrap();
            let Err(error) = super::lower_with_caller_cache(&mut graph, None, None) else {
                panic!("expected the independent list operand to fail");
            };
            assert_eq!(error.kind, super::Kind::ExactType);
            assert!(error.to_string().contains(&format!(":{line}:")));
            assert!(error.to_string().contains("expected int, got bool"));
        }
    }

    #[test]
    fn fractions_use_python_numeric_equality() {
        use crate::modules::SourceGraph;
        let source = "from fractions import Fraction\nprint(Fraction(3, 6) == Fraction(1, 2))\n";
        let graph = SourceGraph::single_with_mode(source, true).unwrap();
        let error = super::lower(&graph)
            .err()
            .expect("numeric comparison requires the Python method");
        assert!(error.contains("Fraction numeric comparison"), "{error}");
        assert!(graph.needs_object_runtime.get());
    }

    #[test]
    fn reachable_literal_bindings_follow_the_known_instance_branch() {
        use crate::modules::SourceGraph;
        let source = r#"
def choose(value: str) -> str:
    if isinstance(value, bytes):
        sep = b"/"
        prefix = b""
    else:
        prefix = ""
        sep = "/"
    return prefix + sep
print(choose("x"))
"#;
        let graph = SourceGraph::single_with_mode(source, true).unwrap();
        assert!(super::lower(&graph).is_ok());
        let graph = SourceGraph::single_with_mode(source, false).unwrap();
        let error = super::lower(&graph)
            .err()
            .expect("whole-module checking inspects both suites");
        assert!(error.contains("expected bytes, got str"), "{error}");
    }

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
    fn typing_mapping_alias_checks_without_boxed_lowering() {
        crate::check(include_str!(
            "../../../tests/fixtures/classes/mapping_alias.py"
        ))
        .unwrap();
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
    fn literal_membership_filters_only_the_guarded_split() {
        let llvm = crate::compile(
            include_str!("../../../tests/fixtures/iterator_pipelines/split_matches.py"),
            &crate::CompileOptions::default(),
        )
        .unwrap();
        assert_eq!(
            llvm.matches("call ptr @imre_str_matching_split_iter(")
                .count(),
            1
        );
    }

    #[test]
    fn byte_split_fuses_only_the_immediately_consumed_list() {
        let llvm = crate::compile(
            include_str!("../../../tests/fixtures/iterator_pipelines/split_fields.py"),
            &crate::CompileOptions::default(),
        )
        .unwrap();
        assert_eq!(
            llvm.matches("call ptr @imre_str_split_byte_iter(").count(),
            2
        );
        assert_eq!(llvm.matches("call ptr @imre_str_split(").count(), 4);
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
