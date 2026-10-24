//! Enforces the closed typed subset independently of gradual Python typing.

use std::collections::{HashMap, HashSet};

use ruff_python_ast as ast;
use ruff_text_size::{Ranged, TextRange};

use crate::classes::Classes;
use crate::ir::{self, Binary, Builtin, Compare, Expr, ExprKind, RegexCapture, Statement, Type};
use crate::modules::{Binding, DefinitionId, Intrinsic, ModuleId, SourceGraph};
use crate::narrowing;
use crate::types::Types;

mod argparse;
mod assignments;
mod callables;
mod caller_cache;
mod calls;
mod comprehensions;
mod dataclass_values;
mod enums;
mod expansions;
mod inference;
mod iterables;
mod json;
mod literals;
mod local_functions;
mod matching;
mod module_values;
mod paths;
mod pipelines;
mod processes;
mod projected_equality;
mod records;
mod regex;
mod sequences;
mod sign_parameters;
mod sorted_rows;
mod stdlib;
mod text;
mod truthiness;
mod unions;
mod unpacking;
mod zip_sums;
use assignments::LocalState;
pub(crate) use caller_cache::CallerCache;
use calls::{Parameter, ParameterKind};
use comprehensions::Helpers;
use module_values::{Initialization, ModuleValues};

#[derive(Clone)]
struct Signature {
    id: usize,
    params: Vec<Parameter>,
    returns: Type,
    method: Option<records::MethodContext>,
}

struct ClassInfo {
    parsers: argparse::Parsers,
    declarations: Classes,
    fields: Vec<Vec<Type>>,
    methods: Vec<HashMap<String, Signature>>,
}

impl ClassInfo {
    /// Resolves all nominal field types before checking constructors and methods.
    fn new(graph: &SourceGraph, types: &Types) -> Result<Self, String> {
        let mut declarations = Classes::collect(graph)?;
        let mut fields = declarations
            .definitions
            .iter()
            .enumerate()
            .map(|(class_id, class)| {
                class
                    .fields
                    .iter()
                    .map(|field| {
                        annotation_in_class(
                            &field.annotation,
                            graph,
                            class.definition.module,
                            &declarations,
                            types,
                            Some(class_id),
                        )
                        .and_then(|ty| {
                            records::field_default(
                                field,
                                ty,
                                types,
                                &graph.modules[class.definition.module].source,
                            )?;
                            if callables::contains_callable(ty, types) {
                                Err(error(
                                    &graph.modules[class.definition.module].source,
                                    &field.annotation,
                                    "class fields cannot contain callable values",
                                ))
                            } else {
                                Ok(ty)
                            }
                        })
                        .map_err(|error| {
                            format!(
                                "{}:{error}",
                                graph.modules[class.definition.module].path.display()
                            )
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()
            })
            .collect::<Result<Vec<_>, _>>()?;
        // Reference counting is sufficient only while no accepted field graph can
        // close a cycle. This concerns storage types, not method signatures.
        if let Some(id) = Self::cyclic_field_root(&fields, types) {
            let class = &declarations.definitions[id];
            return Err(graph.modules[class.definition.module].error(&class.node,
                "cyclic heap types are unsupported; class fields must form an acyclic ownership graph"));
        }
        let parsers = argparse::Parsers::collect(graph, types, &mut declarations, &mut fields)?;
        Ok(Self {
            parsers,
            methods: vec![HashMap::new(); fields.len()],
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
    "sorted",
    "divmod",
    "isinstance",
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
    "__name__",
    "__doc__",
];

/// These numbers are the sealed exception categories in the runtime ABI.
fn exception_kind(name: &str) -> Option<u32> {
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
    let mut classes = ClassInfo::new(graph, &types)?;
    dataclass_values::validate(&classes, graph, &types)?;
    let mut signatures = HashMap::new();
    let inference = inference::Inference::default();
    let mut definitions = Vec::new();
    let mut callable_definitions = Vec::new();
    for (module_id, module) in graph.modules.iter().enumerate() {
        for (name, binding) in &module.bindings {
            if reserved(name)
                && !(name == "open" && *binding == Binding::Intrinsic(Intrinsic::GzipOpen))
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
                    let signature = signature(
                        function,
                        graph,
                        module_id,
                        &classes.declarations,
                        &types,
                        None,
                        &inference,
                    )
                    .map_err(|error| format!("{}:{error}", module.path.display()))?;
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
        for (index, method) in class.methods.iter().enumerate() {
            let context = records::MethodContext {
                class: class_id,
                index,
            };
            let mut method =
                records::executable_method(method, context, &classes.declarations, graph, &types)?;
            let mut signature = signature(
                &method,
                graph,
                module_id,
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
            } else if method.name.as_str() == "__str__" && signature.returns != Type::Str {
                return Err(module.error(&method, "__str__ must return str"));
            }
            classes.methods[class_id].insert(method.name.to_string(), signature.clone());
            definitions.push((module_id, method, signature));
            callable_definitions.push(class.definition);
        }
    }
    inference.solve(
        graph,
        &mut definitions,
        &mut signatures,
        &mut classes,
        &types,
    )?;
    let initializer_start = definitions.len();
    let helpers = Helpers::new(definitions.len() + graph.initializer_modules().count());
    let globals = ModuleValues::default();
    let mut initializations = Vec::new();
    let mut available = HashSet::new();
    for module_id in graph.initializer_modules() {
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
            Initialization::lower(checker, &mut available)
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
        if sums.len() >= 2 {
            zip_sums.push((functions.len(), sums));
        }
        owners.push(*module_id);
        functions.push(body.function);
        call_graph.push(body.calls);
        class_reads.push(body.class_reads);
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
        owners.push(initialization.module);
        functions.push(ir::Function {
            params: Vec::new(),
            returns: Type::None,
            locals: initialization.function.locals.clone(),
            body: initialization.function.body.clone(),
        });
        call_graph.push(initialization.calls.clone());
        class_reads.push(HashSet::new());
        let module = &graph.modules[initialization.module];
        recursion_errors
            .push(module.error(module.parsed.syntax(), "recursive module initialization"));
    }
    // Entry code and inactive module guards can create helpers too. Validate the
    // complete graph only after every source expression has been lowered.
    for helper in helpers.into_functions() {
        owners.push(helper.module);
        functions.push(helper.function);
        call_graph.push(helper.calls);
        class_reads.push(helper.class_reads);
        recursion_errors.push(helper.recursion_error);
    }
    let entry = entry.expect("source graph has an entry module");
    let callable_targets = callables::targets(&functions, &entry);
    for (function, calls) in functions.iter().zip(&mut call_graph) {
        ir::walk_expressions(&function.body, &mut |value| {
            if let ExprKind::Invoke { callable, .. } = &value.kind
                && let Some(targets) = callable_targets.get(&callable.ty)
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
    let json_records = json::schemas(graph, &classes, &types, &functions, &entry)?;
    for (id, sums) in zip_sums {
        zip_sums::fuse(&mut functions, id, &sums);
    }
    if let Some(cache) = cache {
        cache.finish(&types);
    }
    Ok((
        ir::Module {
            types,
            class_metadata: dataclass_values::metadata(&classes, graph),
            class_fields: classes.fields,
            json_records,
            globals: globals.into_types(),
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
    default_reads: Vec<caller_cache::DefaultId>,
}

type BodyResult = Result<(LoweredBody, HashMap<usize, zip_sums::Sum>), String>;

fn lower_body(
    mut checker: Checker<'_>,
    definition: &ast::StmtFunctionDef,
    signature: &Signature,
) -> BodyResult {
    #[cfg(test)]
    caller_cache::record_lowering(definition.name.as_str());
    checker.returns = signature.returns;
    checker.method_context(signature.method);
    let mut params = Vec::new();
    for (parameter, signature) in definition
        .parameters
        .iter_non_variadic_params()
        .zip(&signature.params)
    {
        let slot = checker.declare(parameter.name().as_str(), signature.ty, parameter)?;
        checker.assigned.insert(slot);
        params.push(slot);
    }
    let (mut body, falls_through) = checker.function_body(&definition.body)?;
    if let Some(context) = signature.method
        && let class = &checker.classes.declarations.definitions[context.class]
        && class.generated_initializer
        && class.initializer == Some(context.index)
        && let Some(post) = checker.classes.methods[context.class].get("__post_init__")
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
    checker.fuse_sorted_rows(&definition.body, &mut body);
    if falls_through && checker.types.payload(signature.returns).is_some() {
        body.push(Statement::Return(Expr {
            ty: signature.returns,
            kind: ExprKind::Absent(Box::new(Expr {
                ty: Type::None,
                kind: ExprKind::None,
            })),
        }));
    } else if falls_through && signature.returns != Type::None {
        return Err(error(
            checker.source,
            definition,
            "not every path returns the declared type",
        ));
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
            default_reads: checker.default_reads.unwrap_or_default(),
        },
        checker.zip_sums,
    ))
}

fn signature(
    function: &ast::StmtFunctionDef,
    graph: &SourceGraph,
    module: ModuleId,
    classes: &Classes,
    types: &Types,
    method: Option<records::MethodContext>,
    inference: &inference::Inference,
) -> Result<Signature, String> {
    let source = &graph.modules[module].source;
    let owner = method.map(|context| context.class);
    let receiver = method
        .filter(|context| {
            matches!(
                context.kind(classes),
                crate::classes::MethodKind::Instance | crate::classes::MethodKind::Property
            )
        })
        .map(|context| classes.instance(context.class));
    if function.is_async
        || !function.decorator_list.is_empty()
        || function.type_params.is_some()
        || function.parameters.vararg.is_some()
        || function.parameters.kwarg.is_some()
    {
        return Err(error(
            source,
            function,
            "async, decorators, type parameters, and variadic parameters are unsupported",
        ));
    }
    let mut params = Vec::new();
    let mut parameter_types = Vec::new();
    for (index, parameter) in function.parameters.iter_non_variadic_params().enumerate() {
        let sequence = parameter.annotation().is_some_and(|value|
            matches!(value, ast::Expr::Subscript(subscript) if graph.resolve(module, &subscript.value) == Some(Binding::Intrinsic(Intrinsic::Sequence))));
        let empty_sequence = sequence
            && matches!(parameter.default.as_deref(), Some(ast::Expr::Tuple(tuple)) if tuple.elts.is_empty());
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
                .map(|value| {
                    if let ast::Expr::Subscript(sequence) = value
                        && graph.resolve(module, &sequence.value)
                            == Some(Binding::Intrinsic(Intrinsic::Sequence))
                    {
                        sequences::check_reads(
                            function,
                            parameter.name().as_str(),
                            empty_sequence,
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
                    .filter(|_| !empty_sequence)
                    .map(|value| calls::scalar_literal(value, source).map(|value| value.ty))
                    .transpose()?)
        };
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
                if empty_sequence {
                    // The read-only Sequence contract makes its empty backing list unobservable.
                    Ok(Expr {
                        ty,
                        kind: ExprKind::List(Vec::new()),
                    })
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
    let returns = function
        .returns
        .as_deref()
        .map(|value| {
            if let ast::Expr::Subscript(subscript) = value
                && graph.resolve(module, &subscript.value)
                    == Some(Binding::Intrinsic(Intrinsic::Sequence))
            {
                annotation_in_class(&subscript.slice, graph, module, classes, types, owner)
                    .map(|element| types.list(element))
            } else {
                annotation_in_class(value, graph, module, classes, types, owner)
            }
        })
        .transpose()?
        .or_else(|| match (receiver, function.name.as_str()) {
            (Some(_), "__init__" | "__post_init__") => Some(Type::None),
            (Some(_), "__str__") => Some(Type::Str),
            _ => None,
        });
    let id = inference.register(parameter_types, returns);
    Ok(Signature {
        id,
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

fn error(source: &str, node: &impl Ranged, message: &str) -> String {
    let offset = usize::from(node.start());
    let prefix = &source[..offset];
    let line = prefix.bytes().filter(|&byte| byte == b'\n').count() + 1;
    let column = prefix.rsplit('\n').next().unwrap_or("").chars().count() + 1;
    format!("{line}:{column}: Relar: {message}")
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
    if let ast::Expr::StringLiteral(value) = expression {
        check_string(value, source)?;
        let contents = value.value.to_str();
        let parsed = ruff_python_parser::parse_expression(contents)
            .map_err(|_| error(source, expression, "invalid quoted type annotation"))?;
        return annotation_at(
            parsed.expr(),
            graph,
            module,
            classes,
            types,
            contents,
            owner,
        )
        .map_err(|message| {
            error(
                source,
                expression,
                &format!("invalid quoted annotation: {message}"),
            )
        });
    }
    match graph.resolve(module, expression) {
        Some(Binding::Definition(definition)) => {
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
        Some(Binding::Intrinsic(Intrinsic::TextIO)) => return Ok(Type::TextIO),
        Some(Binding::Intrinsic(Intrinsic::BinaryIO)) => return Ok(Type::BinaryIO),
        Some(Binding::Intrinsic(Intrinsic::Path)) => return Ok(Type::Path),
        Some(Binding::Intrinsic(Intrinsic::CompletedProcess)) => {
            return Ok(Type::CompletedProcess);
        }
        _ => {}
    }
    let nested = |expression: &ast::Expr| {
        annotation_at(expression, graph, module, classes, types, source, owner)
    };
    match expression {
        ast::Expr::Name(name) => match name.id.as_str() {
            "int" => Ok(Type::Int),
            "float" => Ok(Type::Float),
            "bool" => Ok(Type::Bool),
            "str" => Ok(Type::Str),
            "bytes" => Ok(Type::Bytes),
            "Exception" => Ok(Type::Exception),
            _ => Err(error(
                source,
                expression,
                "unsupported exact type annotation",
            )),
        },
        ast::Expr::NoneLiteral(_) => Ok(Type::None),
        ast::Expr::Subscript(subscript)
            if graph.resolve(module, &subscript.value)
                == Some(Binding::Intrinsic(Intrinsic::Literal)) =>
        {
            literals::annotation(&subscript.slice, types, source)
        }
        ast::Expr::Subscript(value)
            if graph.resolve(module, &value.value)
                == Some(Binding::Intrinsic(Intrinsic::AbstractSet))
                && nested(&value.slice)? == Type::Str =>
        {
            Ok(Type::StringKeys)
        }
        ast::Expr::Subscript(value)
            if let Some(Binding::Intrinsic(
                kind @ (Intrinsic::RegexPattern | Intrinsic::RegexMatch),
            )) = graph.resolve(module, &value.value) =>
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
            if matches!(
                graph.resolve(module, &value.value),
                Some(Binding::Intrinsic(Intrinsic::CompletedProcess))
            ) =>
        {
            if nested(&value.slice)? != Type::Str {
                return Err(error(
                    source,
                    expression,
                    "CompletedProcess requires str output",
                ));
            }
            Ok(Type::CompletedProcess)
        }
        ast::Expr::Subscript(value)
            if matches!(
                graph.resolve(module, &value.value),
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
                graph.resolve(module, &value.value),
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
            ast::Expr::Name(name) if name.id == "list" => Ok(types.list(nested(&value.slice)?)),
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
                    || graph.resolve(module, receiver)
                        == Some(Binding::Intrinsic(Intrinsic::DefaultDict)) =>
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
                    && graph.resolve(module, value) == Some(Binding::Intrinsic(Intrinsic::Any))
                {
                    return Ok(Type::JsonObject);
                }
                if !types.hashable_key(key_type) {
                    return Err(error(
                        source,
                        key,
                        "dictionary keys must be int, str, bytes, Path, or immutable scalar tuples",
                    ));
                }
                let value_type = nested(value)?;
                if graph.resolve(module, receiver)
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
                } else {
                    Ok(types.dict(key_type, value_type))
                }
            }
            ast::Expr::Name(name) if name.id == "tuple" => {
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
            let left = nested(&union.left)?;
            let right = nested(&union.right)?;
            types.union(&[left, right]).ok_or_else(|| {
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
    regex_guards: Vec<(usize, regex::CaptureNames)>,
    namespace_guards: Vec<(usize, usize)>,
    returns: Type,
    names: HashMap<String, usize>,
    local_functions: HashMap<String, local_functions::Binding>,
    locals: Vec<Type>,
    assigned: assignments::Assigned,
    calls: HashSet<usize>,
    class_reads: std::cell::RefCell<HashSet<DefinitionId>>,
    default_reads: Option<Vec<caller_cache::DefaultId>>,
    loop_breaks: Vec<Option<LocalState>>,
    in_finally: bool,
    reachable: bool,
    module_scope: bool,
    class: Option<usize>,
    class_receiver: Option<String>,
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
            zip_sum_rhs: None,
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
            namespace_guards: Vec::new(),
            returns: Type::None,
            names: HashMap::new(),
            local_functions: HashMap::new(),
            locals: Vec::new(),
            assigned: assignments::Assigned::default(),
            calls: HashSet::new(),
            class_reads: Default::default(),
            default_reads: None,
            loop_breaks: Vec::new(),
            in_finally: false,
            reachable: true,
            module_scope: false,
            class: None,
            class_receiver: None,
            initializing: false,
        }
    }

    /// Reuses a slot only at its original type and prevents rebinding fixed call targets.
    fn declare(&mut self, name: &str, ty: Type, node: &impl Ranged) -> Result<usize, String> {
        if reserved(name)
            || self.graph.modules[self.module].bindings.contains_key(name)
            || self.local_functions.contains_key(name)
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

    /// Resolves a fixed field without evaluating its receiver more than once.
    fn field(&mut self, attribute: &ast::ExprAttribute) -> Result<(Expr, usize, Type), String> {
        let receiver = self.expr(&attribute.value)?;
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

    /// Optional and finite-union promotions are explicit; containers stay invariant.
    fn coerce(&self, value: Expr, expected: Type, node: &impl Ranged) -> Result<Expr, String> {
        if value.ty == expected {
            return Ok(value);
        }
        if value.ty == Type::JsonValue
            && matches!(
                self.types.payload(expected).unwrap_or(expected),
                Type::JsonObject | Type::Int | Type::Str
            )
        {
            return Ok(json::select(value, expected, None, None));
        }
        if self.types.is_default_list_dict(value.ty)
            && !self.types.is_default_list_dict(expected)
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
        if value.ty == Type::TimeoutExpired && expected == Type::Exception {
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

    /// Context determines empty literals and optional elements at storage boundaries.
    fn expr_expected(&mut self, expression: &ast::Expr, expected: Type) -> Result<Expr, String> {
        self.reject_class_receiver_value(expression)?;
        let concrete = self.types.payload(expected).unwrap_or(expected);
        if matches!(concrete, Type::Callable(_)) {
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
                let receiver = self.expr(&attribute.value)?;
                if let Type::Class(class) | Type::StrEnum(class) = receiver.ty {
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
            ast::Expr::Call(call)
                if self.resolve(&call.func) == Some(Binding::Intrinsic(Intrinsic::DefaultDict)) =>
            {
                let (key, value) = self.types.mapping(concrete).ok_or_else(|| {
                    error(
                        self.source,
                        call,
                        "defaultdict(list) requires a typed dictionary context",
                    )
                })?;
                if self.types.element(value).is_none()
                    || !matches!(call.arguments.args.as_slice(), [ast::Expr::Name(name)] if name.id == "list")
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
                    && matches!(&*call.func, ast::Expr::Name(name) if name.id == "dict")
                    && call.arguments.args.is_empty()
                    && call.arguments.keywords.is_empty() =>
            {
                let (key, value) = self.types.mapping(concrete).unwrap();
                Expr {
                    ty: self.types.dict(key, value),
                    kind: ExprKind::Dict {
                        key,
                        value,
                        entries: Vec::new(),
                    },
                }
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
        self.coerce(value, expected, expression)
    }

    /// Checks both arms under their own narrowing facts, while contextual types
    /// reach literals before the branches join. Only optional promotion can
    /// widen a result; unrelated concrete storage types never share a value.
    fn conditional(&mut self, value: &ast::ExprIf, expected: Option<Type>) -> Result<Expr, String> {
        self.conditional_with(value, |checker, arm| {
            if let Some(expected) = expected {
                checker.expr_expected(arm, expected)
            } else {
                checker.expr(arm)
            }
        })
    }

    /// Checks branch-local facts once for value, contextual, and truth consumers.
    fn conditional_with(
        &mut self,
        value: &ast::ExprIf,
        mut check: impl FnMut(&mut Self, &ast::Expr) -> Result<Expr, String>,
    ) -> Result<Expr, String> {
        let test = self.condition(&value.test)?;
        let mut facts = assignments::ConditionalFacts::capture(self, value);
        self.apply_facts(&value.test, true);
        let yes = check(self, &value.body)?;
        facts.switch_arm(self);
        self.apply_facts(&value.test, false);
        let no = check(self, &value.orelse)?;
        facts.join(self);
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
    fn invalidate_loop_facts(&mut self, body: &[ast::Stmt]) {
        for name in narrowing::assigned_names(body) {
            if let Some(slot) = self.names.get(&name) {
                self.narrowed.remove_mut(slot);
            }
        }
        for name in narrowing::deleted_names(body) {
            if let Some(slot) = self.names.get(&name) {
                self.assigned.remove(slot);
            }
        }
    }

    fn assigned_value(&mut self, slot: usize, value: &Expr) {
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
        if let ExprKind::Builtin(Builtin::UnionInject, args) = &value.kind
            && matches!(args[0].ty, Type::Str | Type::Bytes)
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
        let mut types = expected;
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
                let value = self.expr(&item.value)?;
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
    fn iteration_source(&mut self, expression: &ast::Expr) -> Result<(Expr, bool), String> {
        if let ast::Expr::Call(call) = expression
            && let ast::Expr::Attribute(attribute) = &*call.func
            && attribute.attr.as_str() == "items"
        {
            if self.class_reference(&attribute.value).is_some() {
                return Ok((self.call(call)?, false));
            }
            let receiver = self.expr(&attribute.value)?;
            if let Type::Class(class) = receiver.ty {
                return Ok((
                    self.class_method_call(call, attribute, class, Some(receiver))?,
                    false,
                ));
            }
            if self.types.mapping(receiver.ty).is_some() {
                if !call.arguments.args.is_empty() || !call.arguments.keywords.is_empty() {
                    return Err(error(self.source, call, "dict.items() takes no arguments"));
                }
                return Ok((receiver, true));
            }
        }
        Ok((self.expr(expression)?, false))
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
        let (iterable, items) = self.iteration_source(expression)?;
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
            input =
                Iteration::Prepared(self.prepare_iteration(input, loop_.iter.as_ref(), false)?);
        }
        self.invalidate_loop_facts(&loop_.body);
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
        if matches!(iterable.ty, Type::TextIO | Type::BinaryIO) {
            let input = self.prepare_file_lines(iterable);
            return self.lower_prepared_iteration(
                target,
                input,
                orelse,
                before_iteration,
                lower_body,
            );
        }
        let mapping = self.types.mapping(iterable.ty);
        let set_element = self
            .types
            .set_element(iterable.ty)
            .or((iterable.ty == Type::StringKeys).then_some(Type::Str));
        let string = iterable.ty == Type::Str;
        let bytes = iterable.ty == Type::Bytes;
        let element = self
            .types
            .element(iterable.ty)
            .or(set_element)
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
        let cursor = if mapping.is_none() && set_element.is_none() && !string && !bytes {
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
                        if matches!(self.locals[captured], Type::List(_)) {
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
        let mut body = Vec::new();
        let mut falls_through = true;
        let reachable = self.reachable;
        let enclosing_sum = self.zip_sum_rhs;
        let mut previous_sum = false;
        for (index, statement) in statements.iter().enumerate() {
            if let ast::Stmt::Assign(assignment) = statement
                && let [ast::Expr::Name(name)] = assignment.targets.as_slice()
                && !self.names.contains_key(name.id.as_str())
                && matches!(&*assignment.value, ast::Expr::List(list) if list.elts.is_empty())
                && let Some(value) =
                    inference::first_append(name.id.as_str(), &statements[index + 1..])
                && let Some(element) = self.graph.session.scalar_type(self.module, value)
            {
                self.declare(name.id.as_str(), self.types.list(element), name)?;
            }
            self.reachable = reachable && falls_through;
            let sum = zip_sums::rhs(statement);
            self.zip_sum_rhs = sum.filter(|_| {
                self.inference.is_none()
                    && (previous_sum || statements.get(index + 1).and_then(zip_sums::rhs).is_some())
            });
            previous_sum = sum.is_some();
            let (lowered, continues) = self.statement(statement)?;
            body.extend(lowered);
            falls_through &= continues;
        }
        self.reachable = reachable;
        self.zip_sum_rhs = enclosing_sum;
        Ok((body, falls_through))
    }

    fn statement(&mut self, statement: &ast::Stmt) -> Result<(Vec<Statement>, bool), String> {
        if self.classes.parsers.declaration(self.module, statement) {
            return Ok((Vec::new(), true));
        }
        let result = match statement {
            ast::Stmt::FunctionDef(function) => {
                self.define_local_function(function)?;
                None
            }
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
                    let (key, element) = if let Some(mapping) = self.types.mapping(list.ty) {
                        mapping
                    } else {
                        let element = self.types.element(list.ty).ok_or_else(|| {
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
                    let index = self.expr_expected(&target.slice, key)?;
                    let value = self.expr(&assign.value)?;
                    let value = Self::integer_addend(assign.op, value, element);
                    let (op, result) = self.binary_op(assign.op, element, value.ty, statement)?;
                    self.expect(element, result, statement)?;
                    Some(Statement::AugmentItem(list, index, op, value))
                } else if let ast::Expr::Attribute(target) = &*assign.target {
                    let (receiver, field, ty) = self.field(target)?;
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
                    let value = self.binary_expr(assign.op, left, right, statement)?;
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
                if let ast::Expr::Call(call) = &*expression.value
                    && self.resolve(&call.func) == Some(Binding::Intrinsic(Intrinsic::AssertNever))
                {
                    return Ok((vec![self.assert_never(call, false)?], false));
                }
                Some(Statement::Expr(self.expr(&expression.value)?))
            }
            ast::Stmt::Delete(delete) => {
                let mut statements = Vec::new();
                for target in &delete.targets {
                    let ast::Expr::Subscript(target) = target else {
                        return Err(error(
                            self.source,
                            target,
                            "deletion requires a dictionary subscript",
                        ));
                    };
                    let receiver = self.expr(&target.value)?;
                    let (key, _) = self.types.mapping(receiver.ty).ok_or_else(|| {
                        error(
                            self.source,
                            target,
                            "deletion requires a dictionary subscript",
                        )
                    })?;
                    let key = self.expr_expected(&target.slice, key)?;
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
                self.invalidate_loop_facts(&loop_.body);
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
                    if self.inferred_returns.is_some() {
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
                return self.with_items(&with_.items, &with_.body);
            }
            ast::Stmt::Raise(raise) => {
                let mut value = raise
                    .exc
                    .as_deref()
                    .map(|value| self.raised_exception(value))
                    .transpose()?;
                if let Some(cause) = &raise.cause {
                    let raised = value.take().expect("raise from has an exception");
                    if !matches!(raised.kind, ExprKind::Exception { .. }) {
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
                return Ok((vec![Statement::Raise(value)], false));
            }
            ast::Stmt::Pass(_) => None,
            _ => {
                return Err(error(
                    self.source,
                    statement,
                    "unsupported statement in the Relar subset",
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
        if temporary.is_none() && !matches!(manager.ty, Type::TextIO | Type::BinaryIO) {
            return Err(error(
                self.source,
                &item.context_expr,
                "with requires a sealed file or a direct TemporaryDirectory call",
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
            protected.push(self.assign(target, self.local(captured))?);
        }
        let (nested, falls) = self.with_items(remaining, body)?;
        protected.extend(nested);
        let mut statements = vec![Statement::Assign(captured, manager)];
        if temporary.is_none() {
            statements.push(Statement::Expr(Expr {
                ty: Type::None,
                kind: ExprKind::Builtin(Builtin::TextEnter, vec![self.local(captured)]),
            }));
        }
        let cleanup = if temporary.is_some() {
            Builtin::Path(ir::PathOperation::CleanupTemporaryDirectory)
        } else {
            Builtin::TextClose
        };
        statements.push(Statement::Try {
            body: protected,
            handlers: Vec::new(),
            orelse: Vec::new(),
            finalbody: vec![Statement::Expr(Expr {
                ty: Type::None,
                kind: ExprKind::Builtin(cleanup, vec![self.local(captured)]),
            })],
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
    fn raised_exception(&mut self, value: &ast::Expr) -> Result<Expr, String> {
        if let Some(kind) = self.exception_category(value) {
            if matches!(kind, 0 | 18 | 25..=32) {
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
            Some(Binding::Intrinsic(Intrinsic::UnsupportedOperation)) => Some(19),
            Some(Binding::Intrinsic(Intrinsic::SubprocessError)) => Some(25),
            Some(Binding::Intrinsic(Intrinsic::CalledProcessError)) => Some(26),
            Some(Binding::Intrinsic(Intrinsic::TimeoutExpired)) => Some(27),
            Some(Binding::Intrinsic(Intrinsic::BadGzipFile)) => Some(28),
            Some(Binding::Intrinsic(Intrinsic::ZlibError)) => Some(29),
            Some(Binding::Intrinsic(Intrinsic::TomlDecodeError)) => Some(30),
            Some(Binding::Intrinsic(Intrinsic::JsonDecodeError)) => Some(31),
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
        for name in narrowing::deleted_names(&try_.body) {
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
                        if let Some(kind) = self.exception_category(target) {
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
                    if !kinds.is_empty() && kinds.iter().all(|kind| *kind == 27) {
                        self.narrowed.insert_mut(slot, Type::TimeoutExpired);
                    }
                    Ok::<_, String>(slot)
                })
                .transpose()?;
            let before_handler = name.and_then(|_| self.take_breaks());
            let (handler_body, falls) = self.block(&handler.body)?;
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
        for (test, body) in conditions {
            let condition = self.condition(test)?;
            let known = self.instance_condition(&condition);
            let before = self.local_state();
            self.apply_facts(test, true);
            let guard_depth = self.regex_guards.len();
            if let Some(guard) = self.regex_guard(&condition, body) {
                self.regex_guards.push(guard);
            }
            let namespace_depth = self.namespace_guards.len();
            if let Some(guard) = self.namespace_guard(test, body) {
                self.namespace_guards.push(guard);
            }
            let (yes, yes_falls) = self.block(body)?;
            self.namespace_guards.truncate(namespace_depth);
            self.regex_guards.truncate(guard_depth);
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
            self.block(&tail.body)?
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
        } else {
            value
        };
        let slot = self.declare(name.id.as_str(), value.ty, target)?;
        self.assigned_value(slot, &value);
        Ok(Statement::Assign(slot, value))
    }

    fn integer_addend(op: ast::Operator, value: Expr, other: Type) -> Expr {
        if op == ast::Operator::Add
            && value.ty == Type::Bool
            && matches!(other, Type::Int | Type::Bool)
        {
            Expr {
                ty: Type::Int,
                kind: ExprKind::Builtin(Builtin::Int, vec![value]),
            }
        } else {
            value
        }
    }

    fn binary_expr(
        &self,
        op: ast::Operator,
        left: Expr,
        right: Expr,
        node: &impl Ranged,
    ) -> Result<Expr, String> {
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
            if (self.types.element(left).is_some() || left == Type::Bytes) && right == Type::Int {
                return Ok((Binary::Repeat, left));
            }
            if left == Type::Int && (self.types.element(right).is_some() || right == Type::Bytes) {
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

    fn expr(&mut self, expression: &ast::Expr) -> Result<Expr, String> {
        let value = self.typed_expr(expression)?;
        Ok(if matches!(value.ty, Type::StrLiteral(_)) {
            literals::view(value, Type::Str)
        } else {
            value
        })
    }

    fn typed_expr(&mut self, expression: &ast::Expr) -> Result<Expr, String> {
        self.reject_class_receiver_value(expression)?;
        if self.local_function_binding(expression).is_some() {
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
        if matches!(
            self.resolve(expression),
            Some(Binding::Intrinsic(Intrinsic::Argv))
        ) {
            return Ok(Expr {
                ty: self.types.list(Type::Str),
                kind: ExprKind::Builtin(Builtin::SysArgv, Vec::new()),
            });
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
            ast::Expr::Name(name) if name.id == "__doc__" => {
                let module = &self.graph.modules[self.module];
                if let Some(ast::Stmt::Expr(doc)) = module.parsed.syntax().body.first()
                    && let ast::Expr::StringLiteral(text) = &*doc.value
                {
                    check_string(text, self.source)?;
                    (Type::Str, ExprKind::Str(text.value.to_string()))
                } else {
                    (Type::None, ExprKind::None)
                }
            }
            ast::Expr::Name(name) if name.id == "__name__" => {
                let name = if self.module == self.graph.entry {
                    "__main__"
                } else {
                    &self.graph.modules[self.module].name
                };
                (Type::Str, ExprKind::Str(name.to_owned()))
            }
            ast::Expr::Name(name) => {
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
                for value in &list.elts {
                    let item = self.expr(value)?;
                    if values.is_empty() {
                        element = item.ty;
                    }
                    self.expect(element, item.ty, value)?;
                    values.push(item);
                }
                (self.types.list(element), ExprKind::List(values))
            }
            ast::Expr::ListComp(_) | ast::Expr::DictComp(_) | ast::Expr::SetComp(_) => {
                return self.comprehension(expression, None);
            }
            ast::Expr::Dict(dict) => return self.dictionary(dict, None),
            ast::Expr::Set(set) => return self.set_literal(set, None),
            ast::Expr::Tuple(tuple) => {
                let values = tuple
                    .elts
                    .iter()
                    .map(|value| self.expr(value))
                    .collect::<Result<Vec<_>, _>>()?;
                let ty = self
                    .types
                    .tuple(values.iter().map(|value| value.ty).collect());
                (ty, ExprKind::Tuple(values))
            }
            ast::Expr::BinOp(binary) => {
                let (mut left, mut right) = if binary.op == ast::Operator::BitOr {
                    let (left, left_key) = self.union_operand(&binary.left)?;
                    let (right, right_key) = self.union_operand(&binary.right)?;
                    if let (Some(left_key), Some(right_key)) = (left_key, right_key) {
                        self.expect(left_key, right_key, binary)?;
                        return Ok(Expr {
                            ty: self.types.set(left_key),
                            kind: ExprKind::Builtin(Builtin::KeyUnion, vec![left, right]),
                        });
                    }
                    (left, right)
                } else {
                    (self.expr(&binary.left)?, self.expr(&binary.right)?)
                };
                if binary.op == ast::Operator::Add
                    && matches!(left.ty, Type::Str | Type::StrEnum(_) | Type::StrLiteral(_))
                    && matches!(right.ty, Type::Str | Type::StrEnum(_) | Type::StrLiteral(_))
                {
                    left = Self::string_value(left);
                    right = Self::string_value(right);
                }
                if binary.op == ast::Operator::Div
                    && (left.ty == Type::Path || right.ty == Type::Path)
                {
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
                return self.binary_expr(binary.op, left, right, expression);
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
                if compare.ops.len() == 1
                    && matches!(compare.ops[0], ast::CmpOp::In | ast::CmpOp::NotIn)
                {
                    let key = self.expr(&compare.operands[0])?;
                    let receiver = self.expr(&compare.operands[1])?;
                    let (key_type, builtin) =
                        if let Some((key, _)) = self.types.mapping(receiver.ty) {
                            (key, Builtin::DictContains)
                        } else if let Some(element) = self
                            .types
                            .set_element(receiver.ty)
                            .or((receiver.ty == Type::StringKeys).then_some(Type::Str))
                        {
                            (element, Builtin::SetContains)
                        } else if self.types.element(receiver.ty) == Some(Type::Str) {
                            (Type::Str, Builtin::StringListContains)
                        } else {
                            return Err(error(
                                self.source,
                                expression,
                                "membership requires a concrete dictionary or set",
                            ));
                        };
                    let key = if key_type == Type::Str {
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
                            return self.enum_identity(left, right, compare.ops[0], expression);
                        }
                        _ => unreachable!(),
                    };
                    let mut result = Expr {
                        ty: Type::Bool,
                        kind: match value.kind {
                            ExprKind::Builtin(
                                Builtin::RegexCapture {
                                    operation: RegexCapture::Group,
                                    known,
                                },
                                args,
                            ) => ExprKind::Builtin(
                                Builtin::RegexCapture {
                                    operation: RegexCapture::Absent,
                                    known,
                                },
                                args,
                            ),
                            _ => ExprKind::Builtin(Builtin::IsNone, vec![value]),
                        },
                    };
                    if compare.ops[0] == ast::CmpOp::IsNot {
                        result = Expr {
                            ty: Type::Bool,
                            kind: ExprKind::Not(Box::new(result)),
                        };
                    }
                    return Ok(result);
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
                if values
                    .iter()
                    .any(|value| matches!(value.ty, Type::JsonValue | Type::JsonObject))
                {
                    return json::compare(values, &compare.ops, self.source, expression);
                }
                if let [left, right] = values.as_slice()
                    && matches!(compare.ops.as_ref(), [ast::CmpOp::Eq | ast::CmpOp::NotEq])
                    && (self.types.payload(left.ty).is_some()
                        || self.types.payload(right.ty).is_some())
                    && let ty @ Type::StrEnum(_) = self.types.payload(left.ty).unwrap_or(left.ty)
                    && self.types.payload(right.ty).unwrap_or(right.ty) == ty
                {
                    return Ok(Self::identity_result(
                        values,
                        compare.ops[0] == ast::CmpOp::NotEq,
                    ));
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
                let mut ops = Vec::new();
                for (&op, pair) in compare.ops.iter().zip(values.windows(2)) {
                    let (ty, right) = (pair[0].ty, pair[1].ty);
                    let numeric = matches!(ty, Type::Int | Type::Float)
                        && matches!(right, Type::Int | Type::Float);
                    let records = matches!(op, ast::CmpOp::Eq | ast::CmpOp::NotEq)
                        && matches!((ty, right), (Type::Class(_), Type::Class(_)));
                    if !numeric && !records {
                        self.expect(ty, right, expression)?;
                    }
                    let ordered = numeric
                        || ty == Type::Bytes
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
                    if matches!(ty, Type::RegexPattern | Type::RegexMatch) {
                        return Err(error(
                            self.source,
                            expression,
                            "regex objects do not support value comparisons in Relar",
                        ));
                    }
                    let tuple_list = self.types.element(ty).is_some_and(|item| {
                        matches!(item, Type::Tuple(_)) && self.types.hashable_key(item)
                    });
                    if matches!(ty, Type::ListInt | Type::List(_)) && !tuple_list
                        || matches!(
                            ty,
                            Type::Dict(_)
                                | Type::Set(_)
                                | Type::StringKeys
                                | Type::Optional(_)
                                | Type::Union(_)
                        )
                        || matches!(ty, Type::Tuple(_)) && !self.types.hashable_key(ty)
                    {
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
                let list = Self::enum_text(self.expr(&subscript.value)?);
                if matches!(list.ty, Type::JsonValue | Type::JsonObject) {
                    let ast::Expr::StringLiteral(key) = subscript.slice.as_ref() else {
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
                if let Some((key, value)) = self.types.mapping(list.ty) {
                    let key = self.expr_expected(&subscript.slice, key)?;
                    return Ok(Expr {
                        ty: value,
                        kind: ExprKind::Builtin(Builtin::DictGet, vec![list, key]),
                    });
                }
                if let Some(fields) = self.types.fields(list.ty) {
                    let (literal, negative) = if let ast::Expr::UnaryOp(unary) = &*subscript.slice {
                        if unary.op != ast::UnaryOp::USub {
                            return Err(error(
                                self.source,
                                expression,
                                "tuple indexes must be integer literals",
                            ));
                        }
                        (&*unary.operand, true)
                    } else {
                        (&*subscript.slice, false)
                    };
                    let index = if let ast::Expr::NumberLiteral(value) = literal
                        && let ast::Number::Int(value) = &value.value
                    {
                        value.to_string().parse::<i64>().ok()
                    } else {
                        None
                    }
                    .ok_or_else(|| {
                        error(
                            self.source,
                            expression,
                            "tuple indexes must be bounded integer literals",
                        )
                    })?;
                    let index = if negative { -index } else { index };
                    let index = if index < 0 {
                        index + fields.len() as i64
                    } else {
                        index
                    };
                    let field = usize::try_from(index)
                        .ok()
                        .filter(|&index| index < fields.len())
                        .ok_or_else(|| {
                            error(self.source, expression, "tuple index is out of bounds")
                        })?;
                    return Ok(Expr {
                        ty: fields[field],
                        kind: ExprKind::Field(Box::new(list), field),
                    });
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
                (
                    element,
                    ExprKind::Builtin(Builtin::ListGet, vec![list, index]),
                )
            }
            ast::Expr::Attribute(attribute) => {
                if let Some(class) = self.class_reference(&attribute.value) {
                    if let Some(member) = self.enum_member(class, attribute.attr.as_str()) {
                        return Ok(member);
                    }
                    return self.class_attribute(attribute, class, None, None);
                }
                let receiver = self.expr(&attribute.value)?;
                if let Type::StrEnum(class) = receiver.ty {
                    match attribute.attr.as_str() {
                        "name" => {
                            return Ok(Expr {
                                ty: Type::Str,
                                kind: ExprKind::Builtin(Builtin::StrEnumName, vec![receiver]),
                            });
                        }
                        "value" => return Ok(Self::enum_text(receiver)),
                        _ => return self.class_attribute(attribute, class, Some(receiver), None),
                    }
                }
                if receiver.ty == Type::Path {
                    return self.path_attribute(receiver, attribute);
                }
                if receiver.ty == Type::CompletedProcess {
                    return self.process_attribute(receiver, attribute);
                }
                if matches!(receiver.ty, Type::Exception | Type::TimeoutExpired) {
                    let field = if receiver.ty == Type::TimeoutExpired {
                        self.timeout_error_attribute(&receiver, attribute)?
                    } else {
                        self.process_error_attribute(&receiver, attribute)
                    };
                    if let Some(value) = field {
                        return Ok(value);
                    }
                    let (ty, builtin) = match attribute.attr.as_str() {
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
                if matches!(receiver.ty, Type::TextIO | Type::BinaryIO)
                    && attribute.attr.as_str() == "closed"
                {
                    return Ok(Expr {
                        ty: Type::Bool,
                        kind: ExprKind::Builtin(Builtin::TextClosed, vec![receiver]),
                    });
                }
                if let Type::Class(class) = receiver.ty
                    && self.classes.methods[class].contains_key(attribute.attr.as_str())
                {
                    return self.class_attribute(attribute, class, Some(receiver), None);
                }
                let (receiver, field, ty) = self.field_value(attribute, receiver)?;
                if let ExprKind::Local(slot) = receiver.kind
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
            ast::Expr::Call(call) => return self.call(call),
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
                    "unsupported expression in the Relar subset",
                ));
            }
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
        Ok(parts
            .into_iter()
            .reduce(|left, right| Expr {
                ty: Type::Str,
                kind: ExprKind::Binary(Binary::Concat, Box::new(left), Box::new(right)),
            })
            .unwrap_or_else(|| Expr {
                ty: Type::Str,
                kind: ExprKind::Str(String::new()),
            }))
    }

    fn call(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        if let Some(expression) = self.parse_args(call)? {
            return Ok(expression);
        }
        self.call_with_key_view(call, None)
    }

    /// A direct `dict.keys()` operand roots the mapping until the operation.
    /// Other methods called `keys` still use ordinary class/module dispatch.
    fn union_operand(&mut self, source: &ast::Expr) -> Result<(Expr, Option<Type>), String> {
        let mut view = false;
        let value = match source {
            ast::Expr::Call(call) if matches!(&*call.func, ast::Expr::Attribute(attr) if attr.attr.as_str() == "keys") => {
                self.call_with_key_view(call, Some(&mut view))?
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
    ) -> Result<Expr, String> {
        calls::reject_unpacking(call, self.source)?;
        if let Some(binding) = self.local_function_binding(&call.func) {
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
        match self.resolve(&call.func) {
            Some(Binding::Intrinsic(Intrinsic::GroupBy)) => {
                return Err(error(
                    self.source,
                    call,
                    "groupby requires direct for iteration with an immediate list(group)",
                ));
            }
            Some(Binding::Intrinsic(Intrinsic::RegexCompile)) => return self.compile_regex(call),
            Some(Binding::Intrinsic(Intrinsic::RegexSearch)) => return self.search_regex(call),
            Some(Binding::Intrinsic(Intrinsic::Dedent)) => {
                return self.text_function(call, "text", ir::TextOperation::Dedent);
            }
            Some(Binding::Intrinsic(Intrinsic::Unquote)) => {
                return self.text_function(call, "string", ir::TextOperation::Unquote);
            }
            _ => {}
        }
        if matches!(
            self.resolve(&call.func),
            Some(Binding::Intrinsic(Intrinsic::TemporaryDirectory))
        ) {
            return Err(error(
                self.source,
                call,
                "TemporaryDirectory must be constructed directly in a with statement",
            ));
        }
        if matches!(
            self.resolve(&call.func),
            Some(Binding::Intrinsic(Intrinsic::SubprocessRun))
        ) {
            return self.process_run(call);
        }
        if let ast::Expr::Attribute(attribute) = &*call.func
            && matches!(
                self.resolve(&attribute.value),
                Some(Binding::Intrinsic(Intrinsic::Environ))
            )
        {
            return self.environ_method(attribute, call);
        }
        if let Some(class) = self.class_reference(&call.func) {
            return self.construct(call, class);
        }
        if matches!(
            self.resolve(&call.func),
            Some(Binding::Intrinsic(Intrinsic::Path))
        ) {
            return self.path_constructor(call);
        }
        if matches!(
            self.resolve(&call.func),
            Some(Binding::Intrinsic(Intrinsic::JsonDecode))
        ) {
            return self.json_decode(call);
        }
        if self.resolve(&call.func) == Some(Binding::Intrinsic(Intrinsic::JsonLoads)) {
            return self.json_loads(call);
        }
        if matches!(
            self.resolve(&call.func),
            Some(Binding::Intrinsic(Intrinsic::TomlDecode))
        ) {
            return self.toml_decode(call);
        }
        if matches!(
            self.resolve(&call.func),
            Some(Binding::Intrinsic(Intrinsic::JsonEncode))
        ) {
            return self.json_encode(call);
        }
        let gzip = matches!(
            self.resolve(&call.func),
            Some(Binding::Intrinsic(Intrinsic::GzipOpen))
        );
        if gzip || matches!(&*call.func, ast::Expr::Name(name) if name.id == "open") {
            return self.open(call, gzip);
        }
        if matches!(&*call.func, ast::Expr::Name(name) if name.id == "isinstance") {
            return self.isinstance(call);
        }
        if let Some(kind) = self.exception_category(&call.func) {
            self.reject_builtin_keywords(call)?;
            if matches!(kind, 0 | 18 | 25..=32) {
                return Err(error(
                    self.source,
                    call,
                    "this exception category is catch-only",
                ));
            }
            if call.arguments.args.len() > 1 {
                return Err(error(
                    self.source,
                    call,
                    "builtin exceptions accept zero arguments or one string message",
                ));
            }
            let message = call
                .arguments
                .args
                .first()
                .map(|message| self.expr_expected(message, Type::Str).map(Box::new))
                .transpose()?;
            return Ok(Expr {
                ty: Type::Exception,
                kind: ExprKind::Exception { kind, message },
            });
        }
        if let Some(binding) = self.resolve(&call.func) {
            let Binding::Definition(definition) = binding else {
                return Err(error(self.source, call, "a module is not callable"));
            };
            let signature = self
                .functions
                .get(&definition)
                .ok_or_else(|| error(self.source, call, "definition is not a callable function"))?
                .clone();
            if self.inference.is_some() {
                let (ty, args) = self.infer_call(call, &signature, None, 0)?;
                return Ok(Expr {
                    ty,
                    kind: ExprKind::Call(signature.id, args),
                });
            }
            let args = self.arguments(call, &signature.params, None)?;
            if let Some(reads) = &mut self.default_reads {
                // The binder appends omitted defaults after all source operands.
                let supplied = call.arguments.args.len() + call.arguments.keywords.len();
                reads.extend(args.order.iter().enumerate().filter_map(|(param, &value)| {
                    (value >= supplied).then_some((definition, param))
                }));
            }
            self.calls.insert(signature.id);
            return Ok(Expr {
                ty: signature.returns,
                kind: ExprKind::Call(signature.id, args),
            });
        }
        if matches!(&*call.func, ast::Expr::Name(name) if self.names.contains_key(name.id.as_str()))
            || !matches!(&*call.func, ast::Expr::Name(_) | ast::Expr::Attribute(_))
        {
            let callable = self.expr(&call.func)?;
            return self.invoke(call, callable);
        }
        if let ast::Expr::Attribute(attribute) = &*call.func {
            if let Some(class) = self.class_reference(&attribute.value) {
                return self.class_method_call(call, attribute, class, None);
            }
            let receiver = self.expr(&attribute.value)?;
            if let Type::Class(class) = receiver.ty {
                return self.class_method_call(call, attribute, class, Some(receiver));
            }
            if let Type::StrEnum(class) = receiver.ty
                && self.classes.methods[class].contains_key(attribute.attr.as_str())
            {
                return self.class_method_call(call, attribute, class, Some(receiver));
            }
            let receiver = Self::enum_text(receiver);
            if receiver.ty == Type::Path {
                return self.path_method(receiver, attribute, call);
            }
            if receiver.ty == Type::CompletedProcess {
                return self.process_method(receiver, attribute, call);
            }
            if matches!(receiver.ty, Type::RegexPattern | Type::RegexMatch) {
                return self.regex_method(receiver, attribute, call);
            }
            if attribute.attr.as_str() == "format"
                && matches!(receiver.ty, Type::Str | Type::StrLiteral(_))
            {
                return self.named_format_call(Self::string_value(receiver), call);
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
            if attribute.attr.as_str() == "sort" && self.types.element(receiver.ty).is_some() {
                return self.list_sort(receiver, call);
            }
            self.reject_builtin_keywords(call)?;
            if matches!(receiver.ty, Type::TextIO | Type::BinaryIO) {
                let binary = receiver.ty == Type::BinaryIO;
                let data_type = if binary { Type::Bytes } else { Type::Str };
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
            if call.arguments.args.is_empty() {
                let conversion = match (receiver.ty, attribute.attr.as_str()) {
                    (Type::Str, "encode") => Some((Builtin::Encode, Type::Bytes)),
                    (Type::Bytes, "decode") => Some((Builtin::Decode, Type::Str)),
                    _ => None,
                };
                if let Some((builtin, ty)) = conversion {
                    return Ok(Expr {
                        ty,
                        kind: ExprKind::Builtin(builtin, vec![receiver]),
                    });
                }
            }
            if receiver.ty == Type::Str
                && attribute.attr.as_str() == "splitlines"
                && call.arguments.args.is_empty()
            {
                return Ok(Expr {
                    ty: self.types.list(Type::Str),
                    kind: ExprKind::Builtin(Builtin::SplitLines, vec![receiver]),
                });
            }
            if let Some((key_type, value_type)) = self.types.mapping(receiver.ty) {
                if attribute.attr.as_str() == "keys"
                    && call.arguments.args.is_empty()
                    && let Some(view) = key_view
                {
                    *view = true;
                    return Ok(receiver);
                }
                if attribute.attr.as_str() != "get" || !(1..=2).contains(&call.arguments.args.len())
                {
                    return Err(error(
                        self.source,
                        call,
                        "supported dictionary methods are get(key[, default]), loop-head items(), and keys() in a key union",
                    ));
                }
                let key = self.expr_expected(&call.arguments.args[0], key_type)?;
                let mut args = vec![receiver, key];
                let optional = self.types.optional(value_type);
                let (builtin, ty) = if let Some(default) = call.arguments.args.get(1) {
                    let default = self.expr_expected(default, optional)?;
                    if optional == value_type {
                        args.push(default);
                        (Builtin::DictGetDefault, value_type)
                    } else if let ExprKind::Present(default) = default.kind {
                        args.push(*default);
                        (Builtin::DictGetDefault, value_type)
                    } else {
                        args.push(default);
                        (Builtin::DictTryGet(value_type), optional)
                    }
                } else {
                    (Builtin::DictTryGet(value_type), optional)
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
            let element = self.types.element(receiver.ty).ok_or_else(|| {
                error(
                    self.source,
                    call,
                    "supported value methods are typed-list append/pop, dict.get, set.add/discard, and str.split()",
                )
            })?;
            if attribute.attr.as_str() == "extend" && call.arguments.args.len() == 1 {
                return self.extend_list(receiver, &call.arguments.args[0], call);
            }
            let mut args = vec![receiver];
            let (builtin, ty) = match attribute.attr.as_str() {
                "append" if call.arguments.args.len() == 1 => {
                    args.push(self.expr_expected(&call.arguments.args[0], element)?);
                    (Builtin::Append, Type::None)
                }
                "pop" if call.arguments.args.is_empty() => (Builtin::Pop, element),
                _ => {
                    return Err(error(
                        self.source,
                        call,
                        "supported list methods are append(value), extend(iterable), and pop()",
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
        if name.id == "print"
            && let [field] = call.arguments.keywords.as_slice()
            && field
                .arg
                .as_ref()
                .is_some_and(|name| name.as_str() == "file")
            && self.resolve(&field.value) == Some(Binding::Intrinsic(Intrinsic::Stderr))
            && let [text] = call.arguments.args.as_slice()
        {
            return Ok(Expr {
                ty: Type::None,
                kind: ExprKind::Builtin(
                    Builtin::StderrLine,
                    vec![self.expr_expected(text, Type::Str)?],
                ),
            });
        }
        match name.id.as_str() {
            "list" => return self.list_call(call, None),
            "sorted" => return self.sorted_call(call, None),
            "abs" => return self.abs_call(call),
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
                        "bytes accepts zero arguments or one int, list[int], or bytes argument",
                    ));
                }
            };
            if !matches!(value.ty, Type::Int | Type::ListInt | Type::Bytes) {
                return Err(error(
                    self.source,
                    call,
                    "bytes accepts exactly int, list[int], or bytes",
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
        if args.len() == 1
            && args[0].ty == Type::Path
            && matches!(name.id.as_str(), "str" | "print")
        {
            let text = paths::operation(ir::PathOperation::Text, Type::Str, args);
            return Ok(if name.id == "str" {
                text
            } else {
                Expr {
                    ty: Type::None,
                    kind: ExprKind::Builtin(Builtin::Print, vec![text]),
                }
            });
        }
        if args.len() == 1
            && matches!(args[0].ty, Type::StrEnum(_))
            && matches!(name.id.as_str(), "str" | "print" | "len")
        {
            let text = Self::enum_text(args.into_iter().next().unwrap());
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
        if name.id == "print"
            && args.len() == 1
            && matches!(args[0].ty, Type::Exception | Type::TimeoutExpired)
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
        if args.len() == 1 && dataclass_values::repr_supported(args[0].ty, self.types) {
            if name.id == "repr" {
                return Ok(Expr {
                    ty: Type::Str,
                    kind: ExprKind::Builtin(Builtin::Repr, args),
                });
            }
            if matches!(name.id.as_str(), "str" | "print")
                && matches!(args[0].ty, Type::Class(_) | Type::CompletedProcess)
            {
                let text = self.record_text(args.into_iter().next().unwrap());
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
        if name.id.as_str() == "len"
            && let ([source], [set]) = (call.arguments.args.as_ref(), args.as_slice())
            && let Some(length) = self.short_set_len(source, set)
        {
            return Ok(length);
        }
        let (builtin, ty) = match name.id.as_str() {
            "input" if args.is_empty() => (Builtin::Input, Type::Str),
            "str" if args.is_empty() => {
                return Ok(Expr {
                    ty: Type::Str,
                    kind: ExprKind::Str(String::new()),
                });
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
                            | Type::TimeoutExpired
                    ) =>
            {
                (Builtin::Str, Type::Str)
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
            "len"
                if args.len() == 1
                    && matches!(
                        args[0].ty,
                        Type::Str
                            | Type::Bytes
                            | Type::ListInt
                            | Type::List(_)
                            | Type::Dict(_)
                            | Type::Set(_)
                            | Type::StringKeys
                            | Type::Tuple(_)
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

    /// Explicit UTF-8 and literal mode keep this builtin's semantics sealed;
    /// only the path is dynamic, and it is evaluated once in source order.
    fn open(&mut self, call: &ast::ExprCall, gzip: bool) -> Result<Expr, String> {
        let [path, ast::Expr::StringLiteral(mode)] = call.arguments.args.as_ref() else {
            return Err(error(
                self.source,
                call,
                "open requires a path and a literal supported text or binary mode",
            ));
        };
        check_string(mode, self.source)?;
        let (ty, builtin) = match (gzip, mode.value.to_str()) {
            (true, "rt") => (Type::TextIO, Builtin::GzipOpen),
            (true, "rb") => (Type::BinaryIO, Builtin::GzipOpen),
            (false, "r") => (Type::TextIO, Builtin::TextOpen(0)),
            (false, "w") => (Type::TextIO, Builtin::TextOpen(1)),
            (false, "rb") => (Type::BinaryIO, Builtin::BinaryOpen(0)),
            (false, "wb") => (Type::BinaryIO, Builtin::BinaryOpen(1)),
            (false, "ab") => (Type::BinaryIO, Builtin::BinaryOpen(2)),
            (false, "xb") => (Type::BinaryIO, Builtin::BinaryOpen(3)),
            (true, _) => {
                return Err(error(
                    self.source,
                    call,
                    "gzip.open supports literal 'rb' or 'rt' mode",
                ));
            }
            (false, _) => {
                return Err(error(
                    self.source,
                    call,
                    "open supports literal 'r', 'w', 'rb', 'wb', 'ab', or 'xb' mode",
                ));
            }
        };
        if ty == Type::BinaryIO {
            self.reject_builtin_keywords(call)?;
        } else {
            self.utf8_encoding(call)?;
        }
        Ok(Expr {
            ty,
            kind: ExprKind::Builtin(builtin, vec![self.path_argument(path)?]),
        })
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
            llvm.matches("call ptr @relar_str_splitlines_iter(").count(),
            2
        );
        assert_eq!(llvm.matches("call ptr @relar_str_splitlines(").count(), 1);
        let (_, after_read) = llvm.split_once("call ptr @relar_path_read_text(").unwrap();
        let (_, after_cursor) = after_read
            .split_once("call ptr @relar_str_splitlines_iter(")
            .unwrap();
        assert!(after_cursor.contains("call i64 @relar_int_parse("));
    }

    #[test]
    fn small_literal_string_loops_elide_container_allocations() {
        for length in [1, 2, 8] {
            let values = vec!["\"value\""; length].join(", ");
            let source = format!(
                "def measure() -> int:\n    count = 0\n    for value in [{values}]:\n        count += len(value)\n    return count\nprint(measure())\n"
            );
            let llvm = crate::compile(&source, &crate::CompileOptions::default()).unwrap();
            assert!(!llvm.contains("call ptr @relar_seq_new("));
            assert!(!llvm.contains("call void @relar_seq_append("));
            assert!(!llvm.contains("call ptr @relar_record_new("));
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
            assert!(llvm.contains("call ptr @relar_seq_new("), "{source}");
        }
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
