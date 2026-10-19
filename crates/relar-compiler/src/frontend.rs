//! Enforces the closed typed subset independently of gradual Python typing.

use std::collections::{HashMap, HashSet};

use ruff_python_ast as ast;
use ruff_text_size::{Ranged, TextRange};

use crate::classes::Classes;
use crate::ir::{self, Binary, Builtin, Compare, Expr, ExprKind, Statement, Type};
use crate::modules::{Binding, DefinitionId, Intrinsic, ModuleId, SourceGraph};
use crate::narrowing;
use crate::types::Types;

mod calls;
mod comprehensions;
use calls::{Parameter, ParameterKind};
use comprehensions::Helpers;

#[derive(Clone)]
struct Signature {
    id: usize,
    params: Vec<Parameter>,
    returns: Type,
}

struct ClassInfo {
    declarations: Classes,
    fields: Vec<Vec<Type>>,
    methods: Vec<HashMap<String, Signature>>,
}

impl ClassInfo {
    /// Resolves all nominal field types before checking constructors and methods.
    fn new(graph: &SourceGraph, types: &Types) -> Result<Self, String> {
        let declarations = Classes::collect(graph)?;
        let fields = declarations
            .definitions
            .iter()
            .map(|class| {
                class
                    .fields
                    .iter()
                    .map(|field| {
                        annotation(
                            &field.annotation,
                            graph,
                            class.definition.module,
                            &declarations,
                            types,
                        )
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
        fn visit_type(ty: Type, fields: &[Vec<Type>], types: &Types, states: &mut [u8]) -> bool {
            if let Type::Class(id) = ty {
                visit(id, fields, types, states)
            } else {
                types
                    .children(ty)
                    .into_iter()
                    .all(|child| visit_type(child, fields, types, states))
            }
        }
        fn visit(id: usize, fields: &[Vec<Type>], types: &Types, states: &mut [u8]) -> bool {
            if states[id] != 0 {
                return states[id] == 2;
            }
            states[id] = 1;
            if !fields[id]
                .iter()
                .all(|&ty| visit_type(ty, fields, types, states))
            {
                return false;
            }
            states[id] = 2;
            true
        }
        let mut states = vec![0; fields.len()];
        for (id, class) in declarations.definitions.iter().enumerate() {
            if !visit(id, &fields, types, &mut states) {
                return Err(graph.modules[class.definition.module].error(&class.node,
                    "cyclic heap types are unsupported; class fields must form an acyclic ownership graph"));
            }
        }
        Ok(Self {
            methods: vec![HashMap::new(); fields.len()],
            declarations,
            fields,
        })
    }
}

const RESERVED: &[&str] = &[
    "int",
    "float",
    "bool",
    "str",
    "bytes",
    "list",
    "tuple",
    "dict",
    "set",
    "len",
    "input",
    "print",
    "range",
    "enumerate",
    "open",
    "None",
    "True",
    "False",
    "__name__",
];

/// These numbers are the sealed exception categories in the runtime ABI.
fn exception_kind(name: &str) -> Option<u32> {
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
pub(crate) fn lower(graph: &SourceGraph) -> Result<ir::Module, String> {
    let types = Types::default();
    let mut classes = ClassInfo::new(graph, &types)?;
    let mut signatures = HashMap::new();
    let mut definitions = Vec::new();
    for (module_id, module) in graph.modules.iter().enumerate() {
        for name in module.bindings.keys() {
            if reserved(name) {
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
                        definitions.len(),
                        None,
                    )
                    .map_err(|error| format!("{}:{error}", module.path.display()))?;
                    signatures.insert(
                        DefinitionId {
                            module: module_id,
                            statement: statement_id,
                        },
                        signature.clone(),
                    );
                    definitions.push((module_id, function, signature));
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
        for method in &class.methods {
            let signature = signature(
                method,
                graph,
                module_id,
                &classes.declarations,
                &types,
                definitions.len(),
                Some(Type::Class(class_id)),
            )
            .map_err(|error| format!("{}:{error}", module.path.display()))?;
            if method.name.as_str() == "__init__" && signature.returns != Type::None {
                return Err(module.error(method, "__init__ must return None"));
            }
            classes.methods[class_id].insert(method.name.to_string(), signature.clone());
            definitions.push((module_id, method, signature));
        }
    }
    let helpers = Helpers::new(definitions.len());
    let mut functions = Vec::new();
    let mut call_graph = Vec::new();
    let mut recursion_errors = Vec::new();
    for (module_id, definition, signature) in &definitions {
        let module = &graph.modules[*module_id];
        let result = (|| {
            let mut checker = Checker::new(
                graph,
                *module_id,
                &signatures,
                &classes,
                &types,
                &helpers,
                signature.returns,
            );
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
            let (mut body, falls_through) = checker.block(&definition.body)?;
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
            Ok((
                ir::Function {
                    params,
                    returns: signature.returns,
                    locals: checker.locals,
                    body,
                },
                checker.calls,
            ))
        })();
        let (function, calls) =
            result.map_err(|error| format!("{}:{error}", module.path.display()))?;
        functions.push(function);
        call_graph.push(calls);
        recursion_errors.push(module.error(
            *definition,
            "recursive calls are unsupported in this prototype",
        ));
    }
    fn visit(id: usize, graph: &[HashSet<usize>], states: &mut [u8]) -> bool {
        if states[id] != 0 {
            return states[id] == 2;
        }
        states[id] = 1;
        for &callee in &graph[id] {
            if !visit(callee, graph, states) {
                return false;
            }
        }
        states[id] = 2;
        true
    }
    let mut entry = None;
    for (module_id, module) in graph.modules.iter().enumerate() {
        let mut checker = Checker::new(
            graph,
            module_id,
            &signatures,
            &classes,
            &types,
            &helpers,
            Type::None,
        );
        checker.module_scope = true;
        let statements = module
            .executable
            .iter()
            .map(|&index| module.parsed.syntax().body[index].clone())
            .collect::<Vec<_>>();
        let (body, _) = checker
            .block(&statements)
            .map_err(|error| format!("{}:{error}", module.path.display()))?;
        if module_id == graph.entry {
            entry = Some(ir::Function {
                params: Vec::new(),
                returns: Type::None,
                locals: checker.locals,
                body,
            });
        }
    }
    // Entry code and inactive module guards can create helpers too. Validate the
    // complete graph only after every source expression has been lowered.
    for helper in helpers.into_functions() {
        functions.push(helper.function);
        call_graph.push(helper.calls);
        recursion_errors.push(helper.recursion_error);
    }
    let mut states = vec![0; functions.len()];
    for (id, diagnostic) in recursion_errors.into_iter().enumerate() {
        if !visit(id, &call_graph, &mut states) {
            return Err(diagnostic);
        }
    }
    Ok(ir::Module {
        types,
        class_fields: classes.fields,
        functions,
        entry: entry.expect("source graph has an entry module"),
    })
}

fn signature(
    function: &ast::StmtFunctionDef,
    graph: &SourceGraph,
    module: ModuleId,
    classes: &Classes,
    types: &Types,
    id: usize,
    receiver: Option<Type>,
) -> Result<Signature, String> {
    let source = &graph.modules[module].source;
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
    for (index, parameter) in function.parameters.iter_non_variadic_params().enumerate() {
        let ty = if let Some(receiver) = receiver
            && index == 0
        {
            if let Some(declared) = parameter.annotation()
                && annotation(declared, graph, module, classes, types)? != receiver
            {
                return Err(error(
                    source,
                    parameter,
                    "self annotation must name its exact class",
                ));
            }
            receiver
        } else {
            annotation(
                parameter.annotation().ok_or_else(|| {
                    error(source, parameter, "parameter requires a type annotation")
                })?,
                graph,
                module,
                classes,
                types,
            )?
        };
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
            .map(|value| calls::scalar_default(value, ty, types, source))
            .transpose()?;
        params.push(Parameter {
            name: parameter.name().to_string(),
            ty,
            kind,
            default,
        });
    }
    let returns = annotation(
        function
            .returns
            .as_deref()
            .ok_or_else(|| error(source, function, "function requires a return annotation"))?,
        graph,
        module,
        classes,
        types,
    )?;
    Ok(Signature {
        id,
        params,
        returns,
    })
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
    annotation_at(
        expression,
        graph,
        module,
        classes,
        types,
        &graph.modules[module].source,
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
) -> Result<Type, String> {
    if let ast::Expr::StringLiteral(value) = expression {
        check_string(value, source)?;
        let contents = value.value.to_str();
        let parsed = ruff_python_parser::parse_expression(contents)
            .map_err(|_| error(source, expression, "invalid quoted type annotation"))?;
        return annotation_at(parsed.expr(), graph, module, classes, types, contents).map_err(
            |message| {
                error(
                    source,
                    expression,
                    &format!("invalid quoted annotation: {message}"),
                )
            },
        );
    }
    match graph.resolve(module, expression) {
        Some(Binding::Definition(definition)) => {
            if let Some(&id) = classes.ids.get(&definition) {
                return Ok(Type::Class(id));
            }
        }
        Some(Binding::Intrinsic(Intrinsic::TextIO)) => return Ok(Type::TextIO),
        Some(Binding::Intrinsic(Intrinsic::BinaryIO)) => return Ok(Type::BinaryIO),
        _ => {}
    }
    let nested =
        |expression: &ast::Expr| annotation_at(expression, graph, module, classes, types, source);
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
        ast::Expr::Subscript(value) => match &*value.value {
            ast::Expr::Name(name) if name.id == "list" => Ok(types.list(nested(&value.slice)?)),
            ast::Expr::Name(name) if name.id == "set" => {
                let element = nested(&value.slice)?;
                if !matches!(element, Type::Int | Type::Str | Type::Bytes) {
                    return Err(error(
                        source,
                        &*value.slice,
                        "set elements must be exactly int, str, or bytes",
                    ));
                }
                Ok(types.set(element))
            }
            ast::Expr::Name(name) if name.id == "dict" => {
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
                if !matches!(key_type, Type::Int | Type::Str | Type::Bytes) {
                    return Err(error(
                        source,
                        key,
                        "dictionary keys must be exactly int, str, or bytes",
                    ));
                }
                Ok(types.dict(key_type, nested(value)?))
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
            match (left, right) {
                (Type::None, other) | (other, Type::None) if other != Type::None => {
                    Ok(types.optional(other))
                }
                _ => Err(error(
                    source,
                    expression,
                    "only optional unions T | None are supported",
                )),
            }
        }
        _ => Err(error(source, expression, "unsupported type annotation")),
    }
}

/// Ruff replaces surrogate escapes with U+FFFD in its analysis AST. Reject those
/// source literals rather than silently compiling a different Python value.
fn check_string(value: &ast::ExprStringLiteral, source: &str) -> Result<(), String> {
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
}

struct Checker<'a> {
    source: &'a str,
    graph: &'a SourceGraph,
    module: ModuleId,
    functions: &'a HashMap<DefinitionId, Signature>,
    classes: &'a ClassInfo,
    types: &'a Types,
    helpers: &'a Helpers,
    comprehension_targets: HashSet<String>,
    narrowed: HashMap<usize, Type>,
    returns: Type,
    names: HashMap<String, usize>,
    locals: Vec<Type>,
    assigned: HashSet<usize>,
    calls: HashSet<usize>,
    loop_depth: usize,
    module_scope: bool,
}

impl<'a> Checker<'a> {
    fn new(
        graph: &'a SourceGraph,
        module: ModuleId,
        functions: &'a HashMap<DefinitionId, Signature>,
        classes: &'a ClassInfo,
        types: &'a Types,
        helpers: &'a Helpers,
        returns: Type,
    ) -> Self {
        Self {
            source: &graph.modules[module].source,
            graph,
            module,
            functions,
            classes,
            types,
            helpers,
            comprehension_targets: HashSet::new(),
            narrowed: HashMap::new(),
            returns,
            names: HashMap::new(),
            locals: Vec::new(),
            assigned: HashSet::new(),
            calls: HashSet::new(),
            loop_depth: 0,
            module_scope: false,
        }
    }

    /// Reuses a slot only at its original type and prevents rebinding fixed call targets.
    fn declare(&mut self, name: &str, ty: Type, node: &impl Ranged) -> Result<usize, String> {
        if reserved(name) || self.graph.modules[self.module].bindings.contains_key(name) {
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

    /// Optional conversions are explicit in the IR; other storage is invariant.
    fn coerce(&self, value: Expr, expected: Type, node: &impl Ranged) -> Result<Expr, String> {
        if value.ty == expected {
            return Ok(value);
        }
        if let Some(payload) = self.types.payload(expected) {
            let kind = if value.ty == Type::None {
                ExprKind::Absent(Box::new(value))
            } else {
                self.expect(payload, value.ty, node)?;
                ExprKind::Present(Box::new(value))
            };
            return Ok(Expr { ty: expected, kind });
        }
        self.expect(expected, value.ty, node)?;
        unreachable!("mismatched exact types return an error")
    }

    /// Context determines empty literals and optional elements at storage boundaries.
    fn expr_expected(&mut self, expression: &ast::Expr, expected: Type) -> Result<Expr, String> {
        let concrete = self.types.payload(expected).unwrap_or(expected);
        let value = match expression {
            ast::Expr::If(value) => return self.conditional(value, Some(expected)),
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
                if self.types.mapping(concrete).is_some()
                    && matches!(&*call.func, ast::Expr::Name(name) if name.id == "dict")
                    && call.arguments.args.is_empty()
                    && call.arguments.keywords.is_empty() =>
            {
                let (key, value) = self.types.mapping(concrete).unwrap();
                Expr {
                    ty: concrete,
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
                let (op, ty) = self.binary_op(binary.op, left.ty, right.ty, expression)?;
                Expr {
                    ty,
                    kind: ExprKind::Binary(op, Box::new(left), Box::new(right)),
                }
            }
            _ => self.expr(expression)?,
        };
        self.coerce(value, expected, expression)
    }

    /// Checks both arms under their own narrowing facts, while contextual types
    /// reach literals before the branches join. Only optional promotion can
    /// widen a result; unrelated concrete storage types never share a value.
    fn conditional(&mut self, value: &ast::ExprIf, expected: Option<Type>) -> Result<Expr, String> {
        let test = self.condition(&value.test)?;
        let facts = self.narrowed.clone();
        self.apply_facts(&value.test, true);
        let yes = if let Some(expected) = expected {
            self.expr_expected(&value.body, expected)?
        } else {
            self.expr(&value.body)?
        };
        self.narrowed = facts.clone();
        self.apply_facts(&value.test, false);
        let no = if let Some(expected) = expected {
            self.expr_expected(&value.orelse, expected)?
        } else {
            self.expr(&value.orelse)?
        };
        self.narrowed = facts;
        let ty = if yes.ty == no.ty {
            yes.ty
        } else if yes.ty == Type::None {
            self.types.optional(no.ty)
        } else if no.ty == Type::None {
            self.types.optional(yes.ty)
        } else if self.types.payload(yes.ty) == Some(no.ty) {
            yes.ty
        } else if self.types.payload(no.ty) == Some(yes.ty) {
            no.ty
        } else {
            return Err(error(
                self.source,
                value,
                "conditional branches require the same concrete type or its optional form",
            ));
        };
        let yes = self.coerce(yes, ty, &*value.body)?;
        let no = self.coerce(no, ty, &*value.orelse)?;
        Ok(Expr {
            ty,
            kind: ExprKind::Conditional(Box::new(test), Box::new(yes), Box::new(no)),
        })
    }

    fn apply_facts(&mut self, test: &ast::Expr, truth: bool) {
        for (name, present) in narrowing::facts(test, truth) {
            if let Some(&slot) = self.names.get(&name)
                && let Some(payload) = self.types.payload(self.locals[slot])
            {
                self.narrowed
                    .insert(slot, if present { payload } else { Type::None });
            }
        }
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
                self.narrowed.remove(slot);
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
        self.narrowed.remove(&slot);
        if let Some(payload) = self.types.payload(self.locals[slot]) {
            match value.kind {
                ExprKind::Present(_) => {
                    self.narrowed.insert(slot, payload);
                }
                ExprKind::Absent(_) => {
                    self.narrowed.insert(slot, Type::None);
                }
                _ => {}
            }
        }
    }

    /// Dictionary storage has one exact key/value pair of types, even when empty.
    fn dictionary(
        &mut self,
        dict: &ast::ExprDict,
        expected: Option<(Type, Type)>,
    ) -> Result<Expr, String> {
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
                if !matches!(key_value.ty, Type::Int | Type::Str | Type::Bytes) {
                    return Err(error(
                        self.source,
                        key,
                        "dictionary keys must be exactly int, str, or bytes",
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
        let mut element = expected;
        let mut values = Vec::with_capacity(set.elts.len());
        for value in &set.elts {
            let checked = if let Some(element) = element {
                self.expr_expected(value, element)?
            } else {
                self.expr(value)?
            };
            if !matches!(checked.ty, Type::Int | Type::Str | Type::Bytes) {
                return Err(error(
                    self.source,
                    value,
                    "set elements must be exactly int, str, or bytes",
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

    /// Builds the target once; LLVM lowering still evaluates the RHS before it.
    fn assignment(&mut self, target: &ast::Expr, value: &ast::Expr) -> Result<Statement, String> {
        match target {
            ast::Expr::Name(name) => {
                let value = if let Some(&slot) = self.names.get(name.id.as_str()) {
                    self.expr_expected(value, self.locals[slot])?
                } else {
                    self.expr(value)?
                };
                self.assign(target, value)
            }
            ast::Expr::Subscript(subscript) => {
                let list = self.expr(&subscript.value)?;
                let (key, element) = self
                    .types
                    .mapping(list.ty)
                    .or_else(|| {
                        self.types
                            .element(list.ty)
                            .map(|element| (Type::Int, element))
                    })
                    .ok_or_else(|| {
                        error(
                            self.source,
                            target,
                            "indexed assignment requires a mutable typed list or dictionary",
                        )
                    })?;
                let index = self.expr_expected(&subscript.slice, key)?;
                let value = self.expr_expected(value, element)?;
                Ok(Statement::SetItem(list, index, value))
            }
            ast::Expr::Attribute(attribute) => {
                let (receiver, field, ty) = self.field(attribute)?;
                let value = self.expr_expected(value, ty)?;
                Ok(Statement::SetField(receiver, field, value))
            }
            _ => Err(error(self.source, target, "unsupported assignment target")),
        }
    }

    /// Supplies existing target types to literal fields before any names are bound.
    fn unpacking_value(&mut self, target: &ast::Expr, value: &ast::Expr) -> Result<Expr, String> {
        if let Some(targets) = pattern_elements(target)
            && let ast::Expr::Tuple(tuple) = value
            && targets.len() == tuple.elts.len()
        {
            let values = targets
                .iter()
                .zip(&tuple.elts)
                .map(|(target, value)| self.unpacking_value(target, value))
                .collect::<Result<Vec<_>, _>>()?;
            return Ok(Expr {
                ty: self
                    .types
                    .tuple(values.iter().map(|value| value.ty).collect()),
                kind: ExprKind::Tuple(values),
            });
        }
        if let ast::Expr::Name(name) = target
            && let Some(&slot) = self.names.get(name.id.as_str())
        {
            let value = self.expr_expected(value, self.locals[slot])?;
            // Wrap at the eventual store, preserving known presence for local
            // narrowing instead of hiding it inside the temporary tuple.
            return Ok(match value.kind {
                ExprKind::Present(value) | ExprKind::Absent(value) => *value,
                _ => value,
            });
        }
        self.expr(value)
    }

    /// Captures a fixed tuple before rebinding any of its recursively named targets.
    fn bind_pattern(&mut self, target: &ast::Expr, value: Expr) -> Result<Vec<Statement>, String> {
        let Some(targets) = pattern_elements(target) else {
            return Ok(vec![self.assign(target, value)?]);
        };
        let fields = self.types.fields(value.ty).ok_or_else(|| {
            error(
                self.source,
                target,
                "unpacking requires a fixed tuple value",
            )
        })?;
        if targets.len() != fields.len() {
            return Err(error(
                self.source,
                target,
                "unpacking target has the wrong number of fields",
            ));
        }
        let captured = self.hidden(value.ty);
        let mut output = vec![Statement::Assign(captured, value)];
        for (index, (target, ty)) in targets.iter().zip(fields).enumerate() {
            let value = Expr {
                ty,
                kind: ExprKind::Field(Box::new(self.local(captured)), index),
            };
            output.extend(self.bind_pattern(target, value)?);
        }
        Ok(vec![Statement::Scope {
            body: output,
            clear: vec![captured],
        }])
    }

    /// Recognizes items views only at a loop boundary; no view value escapes.
    fn iteration_source(&mut self, expression: &ast::Expr) -> Result<(Expr, bool), String> {
        if let ast::Expr::Call(call) = expression
            && let ast::Expr::Attribute(attribute) = &*call.func
            && attribute.attr.as_str() == "items"
        {
            let receiver = self.expr(&attribute.value)?;
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
        let ((iterable, items), start) = if let ast::Expr::Call(call) = expression
            && matches!(&*call.func, ast::Expr::Name(name) if name.id == "enumerate")
        {
            if !call.arguments.keywords.is_empty() || !(1..=2).contains(&call.arguments.args.len())
            {
                return Err(error(
                    self.source,
                    call,
                    "enumerate requires a concrete container and an optional positional integer start",
                ));
            }
            let iterable = self.iteration_source(&call.arguments.args[0])?;
            let start = if let Some(start) = call.arguments.args.get(1) {
                self.expr_expected(start, Type::Int)?
            } else {
                Expr {
                    ty: Type::Int,
                    kind: ExprKind::Int("0".into()),
                }
            };
            (iterable, Some(start))
        } else {
            (self.iteration_source(expression)?, None)
        };
        Ok(Iteration::Container {
            iterable,
            items,
            start,
        })
    }

    /// Keeps the original container rooted and owns each item before user code runs.
    fn iteration(&mut self, loop_: &ast::StmtFor) -> Result<Vec<Statement>, String> {
        let input = self.iteration_input(&loop_.iter)?;
        self.invalidate_loop_facts(&loop_.body);
        self.lower_iteration(&loop_.target, input, |checker| {
            Ok(checker.block(&loop_.body)?.0)
        })
    }

    /// Shares iteration snapshots and cleanup between statements and eager displays.
    /// The callback runs with the target definitely assigned for one iteration.
    fn lower_iteration(
        &mut self,
        target: &ast::Expr,
        input: Iteration,
        lower_body: impl FnOnce(&mut Self) -> Result<Vec<Statement>, String>,
    ) -> Result<Vec<Statement>, String> {
        if let Iteration::Range(args) = input {
            let ast::Expr::Name(name) = target else {
                return Err(error(self.source, target, "range target must be a name"));
            };
            let target = self.declare(name.id.as_str(), Type::Int, target)?;
            self.narrowed.remove(&target);
            let facts = self.narrowed.clone();
            let before = self.assigned.clone();
            self.assigned.insert(target);
            self.loop_depth += 1;
            let body = lower_body(self)?;
            self.loop_depth -= 1;
            self.assigned = before;
            self.narrowed = facts;
            let cursor = self.hidden(Type::Int);
            let stop = self.hidden(Type::Int);
            let step = self.hidden(Type::Int);
            return Ok(vec![Statement::Scope {
                body: vec![Statement::For {
                    target,
                    cursor,
                    stop,
                    step,
                    args,
                    body,
                }],
                clear: vec![cursor, stop, step],
            }]);
        }
        let Iteration::Container {
            iterable,
            items,
            start,
        } = input
        else {
            unreachable!("range iteration returned above")
        };
        let mapping = self.types.mapping(iterable.ty);
        let set_element = self.types.set_element(iterable.ty);
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
        let counter = start.map(|start| {
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
                        Builtin::ListGet,
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
                vec![Statement::Break],
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
        let item = if let Some(counter) = counter {
            Expr {
                ty: self.types.tuple(vec![Type::Int, element]),
                kind: ExprKind::Tuple(vec![self.local(counter), item]),
            }
        } else {
            item
        };

        let mut facts = self.narrowed.clone();
        let before = self.assigned.clone();
        let bindings = self.bind_pattern(target, item)?;
        fn remove_target_facts(
            target: &ast::Expr,
            names: &HashMap<String, usize>,
            facts: &mut HashMap<usize, Type>,
        ) {
            if let ast::Expr::Name(name) = target {
                if let Some(slot) = names.get(name.id.as_str()) {
                    facts.remove(slot);
                }
            } else if let Some(elements) = pattern_elements(target) {
                for element in elements {
                    remove_target_facts(element, names, facts);
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
        self.loop_depth += 1;
        body.extend(lower_body(self)?);
        self.loop_depth -= 1;
        self.assigned = before;
        self.narrowed = facts;
        output.push(Statement::While(test, body));
        cleanup.extend(counter);
        Ok(vec![Statement::Scope {
            body: output,
            clear: cleanup,
        }])
    }

    /// Checks dead syntax too, but merges definite assignment only from reachable exits.
    fn block(&mut self, statements: &[ast::Stmt]) -> Result<(Vec<Statement>, bool), String> {
        let mut body = Vec::new();
        let mut falls_through = true;
        for statement in statements {
            let (lowered, continues) = self.statement(statement)?;
            body.extend(lowered);
            falls_through &= continues;
        }
        Ok((body, falls_through))
    }

    fn statement(&mut self, statement: &ast::Stmt) -> Result<(Vec<Statement>, bool), String> {
        let result = match statement {
            ast::Stmt::Assign(assign) if assign.targets.len() == 1 => {
                if pattern_elements(&assign.targets[0]).is_some() {
                    let value = self.unpacking_value(&assign.targets[0], &assign.value)?;
                    return Ok((self.bind_pattern(&assign.targets[0], value)?, true));
                }
                Some(self.assignment(&assign.targets[0], &assign.value)?)
            }
            ast::Stmt::AnnAssign(assign) => {
                let ast::Expr::Name(name) = &*assign.target else {
                    return Err(error(self.source, statement, "only names may be annotated"));
                };
                let ty = annotation(
                    &assign.annotation,
                    self.graph,
                    self.module,
                    &self.classes.declarations,
                    self.types,
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
                    let (op, ty) = self.binary_op(assign.op, left.ty, right.ty, statement)?;
                    if self.types.element(ty).is_some() {
                        return Err(error(
                            self.source,
                            statement,
                            "in-place list operations are unsupported",
                        ));
                    }
                    let value = Expr {
                        ty,
                        kind: ExprKind::Binary(op, Box::new(left), Box::new(right)),
                    };
                    Some(self.assign(&assign.target, value)?)
                } else {
                    return Err(error(
                        self.source,
                        statement,
                        "unsupported assignment target",
                    ));
                }
            }
            ast::Stmt::Expr(expression) => Some(Statement::Expr(self.expr(&expression.value)?)),
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
            ast::Stmt::If(branch) => {
                let (statement, flow) =
                    self.if_chain(&branch.test, &branch.body, &branch.elif_else_clauses)?;
                return Ok((vec![statement], flow));
            }
            ast::Stmt::While(loop_) => {
                if !loop_.orelse.is_empty() {
                    return Err(error(
                        self.source,
                        statement,
                        "loop else clauses are unsupported",
                    ));
                }
                self.invalidate_loop_facts(&loop_.body);
                let facts = self.narrowed.clone();
                let test = self.condition(&loop_.test)?;
                let before = self.assigned.clone();
                self.apply_facts(&loop_.test, true);
                self.loop_depth += 1;
                let (body, _) = self.block(&loop_.body)?;
                self.loop_depth -= 1;
                self.assigned = before;
                self.narrowed = facts;
                Some(Statement::While(test, body))
            }
            ast::Stmt::For(loop_) if !loop_.is_async && loop_.orelse.is_empty() => {
                return Ok((self.iteration(loop_)?, true));
            }
            ast::Stmt::Return(return_) if !self.module_scope => {
                let value = if let Some(value) = &return_.value {
                    self.expr_expected(value, self.returns)?
                } else {
                    Expr {
                        ty: Type::None,
                        kind: ExprKind::None,
                    }
                };
                let value = self.coerce(value, self.returns, statement)?;
                return Ok((vec![Statement::Return(value)], false));
            }
            ast::Stmt::Break(_) | ast::Stmt::Continue(_) if self.loop_depth > 0 => {
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
                self.apply_facts(&assert.test, true);
                Some(result)
            }
            ast::Stmt::Try(try_) if !try_.is_star => {
                return self.try_statement(try_);
            }
            ast::Stmt::With(with_) if !with_.is_async => {
                return self.with_items(&with_.items, &with_.body);
            }
            ast::Stmt::Raise(raise) if raise.cause.is_none() => {
                let value = raise
                    .exc
                    .as_deref()
                    .map(|value| {
                        if let Some(kind) = self.exception_category(value) {
                            if matches!(kind, 0 | 18) {
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
                    })
                    .transpose()?;
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

    /// Only sealed file managers are accepted. Enter precedes installation of
    /// its own finally, while already-entered outer managers protect later
    /// expressions. Cleanup uses an independent root if the user rebinds `as`.
    fn with_items(
        &mut self,
        items: &[ast::WithItem],
        body: &[ast::Stmt],
    ) -> Result<(Vec<Statement>, bool), String> {
        let Some((item, remaining)) = items.split_first() else {
            return self.block(body);
        };
        let manager = self.expr(&item.context_expr)?;
        if !matches!(manager.ty, Type::TextIO | Type::BinaryIO) {
            return Err(error(
                self.source,
                &item.context_expr,
                "with requires a sealed text or binary file",
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
        Ok((
            vec![Statement::Scope {
                body: vec![
                    Statement::Assign(captured, manager),
                    Statement::Expr(Expr {
                        ty: Type::None,
                        kind: ExprKind::Builtin(Builtin::TextEnter, vec![self.local(captured)]),
                    }),
                    Statement::Try {
                        body: protected,
                        handlers: Vec::new(),
                        orelse: Vec::new(),
                        finalbody: vec![Statement::Expr(Expr {
                            ty: Type::None,
                            kind: ExprKind::Builtin(Builtin::TextClose, vec![self.local(captured)]),
                        })],
                    },
                ],
                clear: vec![captured],
            }],
            falls,
        ))
    }

    /// Imported exception aliases resolve by identity; a spelling alone cannot
    /// turn an arbitrary first-party definition into a builtin category.
    fn exception_category(&self, expression: &ast::Expr) -> Option<u32> {
        match self.graph.resolve(self.module, expression) {
            Some(Binding::Intrinsic(Intrinsic::UnsupportedOperation)) => Some(19),
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
        let (body, body_falls) = self.block(&try_.body)?;
        let (orelse, else_falls) = self.block(&try_.orelse)?;
        let mut exits = Vec::new();
        if body_falls && else_falls {
            exits.push((self.assigned.clone(), self.narrowed.clone()));
        }

        let mut handler_assigned = before.clone();
        for name in narrowing::deleted_names(&try_.body) {
            if let Some(slot) = self.names.get(&name) {
                handler_assigned.remove(slot);
            }
        }
        let mut handler_facts = before_facts.clone();
        for name in narrowing::assigned_names(&try_.body) {
            if let Some(slot) = self.names.get(&name) {
                handler_facts.remove(slot);
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
                    self.narrowed.remove(&slot);
                    Ok::<_, String>(slot)
                })
                .transpose()?;
            let (handler_body, falls) = self.block(&handler.body)?;
            if let Some(name) = name {
                self.assigned.remove(&name);
                self.narrowed.remove(&name);
            }
            if falls {
                exits.push((self.assigned.clone(), self.narrowed.clone()));
            }
            handlers.push(ir::ExceptHandler {
                kinds,
                name,
                body: handler_body,
            });
        }

        let mut normal_assigned = exits.first().map(|exit| exit.0.clone()).unwrap_or_default();
        let mut normal_facts = exits.first().map(|exit| exit.1.clone()).unwrap_or_default();
        for (assigned, facts) in &exits[1.min(exits.len())..] {
            normal_assigned.retain(|slot| assigned.contains(slot));
            normal_facts.retain(|slot, ty| facts.get(slot) == Some(ty));
        }

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
                self.narrowed.remove(slot);
            }
        }
        for name in narrowing::deleted_names(&protected) {
            if let Some(slot) = self.names.get(&name) {
                self.assigned.remove(slot);
            }
        }
        for name in narrowing::assigned_names(&protected) {
            if let Some(slot) = self.names.get(&name) {
                self.narrowed.remove(slot);
            }
        }
        let (finalbody, final_falls) = self.block(&try_.finalbody)?;
        if try_.finalbody.is_empty() {
            self.assigned = normal_assigned;
            self.narrowed = normal_facts;
        } else {
            for name in narrowing::deleted_names(&try_.finalbody) {
                if let Some(slot) = self.names.get(&name) {
                    normal_assigned.remove(slot);
                }
            }
            self.assigned.extend(normal_assigned);
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
            !exits.is_empty() && final_falls,
        ))
    }

    fn if_chain(
        &mut self,
        test: &ast::Expr,
        body: &[ast::Stmt],
        clauses: &[ast::ElifElseClause],
    ) -> Result<(Statement, bool), String> {
        let condition = self.condition(test)?;
        let before = self.assigned.clone();
        let facts = self.narrowed.clone();
        self.apply_facts(test, true);
        let (yes, yes_falls) = self.block(body)?;
        let yes_assigned = self.assigned.clone();
        let yes_facts = self.narrowed.clone();
        self.assigned = before;
        self.narrowed = facts;
        self.apply_facts(test, false);
        let (no, no_falls) = if let Some((clause, rest)) = clauses.split_first() {
            if let Some(test) = &clause.test {
                let (nested, flow) = self.if_chain(test, &clause.body, rest)?;
                (vec![nested], flow)
            } else {
                self.block(&clause.body)?
            }
        } else {
            (Vec::new(), true)
        };
        if yes_falls && no_falls {
            self.assigned.retain(|slot| yes_assigned.contains(slot));
            self.narrowed
                .retain(|slot, ty| yes_facts.get(slot) == Some(ty));
        } else if yes_falls {
            self.assigned = yes_assigned;
            self.narrowed = yes_facts;
        }
        Ok((Statement::If(condition, yes, no), yes_falls || no_falls))
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

    fn condition(&mut self, expression: &ast::Expr) -> Result<Expr, String> {
        let expression_ir = self.expr(expression)?;
        self.expect(Type::Bool, expression_ir.ty, expression)?;
        Ok(expression_ir)
    }

    fn binary_op(
        &self,
        op: ast::Operator,
        left: Type,
        right: Type,
        node: &impl Ranged,
    ) -> Result<(Binary, Type), String> {
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
        if matches!(
            self.graph.resolve(self.module, expression),
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
            ast::Expr::If(value) => return self.conditional(value, None),
            ast::Expr::NoneLiteral(_) => (Type::None, ExprKind::None),
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
                if let Some(&ty) = self.narrowed.get(&slot) {
                    if ty == Type::None {
                        (Type::None, ExprKind::None)
                    } else {
                        (
                            ty,
                            ExprKind::Unwrap(Box::new(Expr {
                                ty: self.locals[slot],
                                kind: ExprKind::Local(slot),
                            })),
                        )
                    }
                } else {
                    (self.locals[slot], ExprKind::Local(slot))
                }
            }
            ast::Expr::List(list) => {
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
                let left = self.expr(&binary.left)?;
                let right = self.expr(&binary.right)?;
                let (op, ty) = self.binary_op(binary.op, left.ty, right.ty, expression)?;
                (ty, ExprKind::Binary(op, Box::new(left), Box::new(right)))
            }
            ast::Expr::UnaryOp(unary) => {
                let value = self.expr(&unary.operand)?;
                match unary.op {
                    ast::UnaryOp::Not => {
                        self.expect(Type::Bool, value.ty, expression)?;
                        (Type::Bool, ExprKind::Not(Box::new(value)))
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
                    _ => return Err(error(self.source, expression, "unsupported unary operator")),
                }
            }
            ast::Expr::BoolOp(boolean) => {
                let facts = self.narrowed.clone();
                let mut values = Vec::new();
                for value in &boolean.values {
                    values.push(self.condition(value)?);
                    self.apply_facts(value, boolean.op == ast::BoolOp::And);
                }
                self.narrowed = facts;
                (
                    Type::Bool,
                    ExprKind::Boolean(boolean.op == ast::BoolOp::And, values),
                )
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
                        } else if let Some(element) = self.types.set_element(receiver.ty) {
                            (element, Builtin::SetContains)
                        } else {
                            return Err(error(
                                self.source,
                                expression,
                                "membership requires a concrete dictionary or set",
                            ));
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
                        _ => {
                            return Err(error(
                                self.source,
                                expression,
                                "identity comparisons are supported only with None",
                            ));
                        }
                    };
                    let mut result = Expr {
                        ty: Type::Bool,
                        kind: ExprKind::Builtin(Builtin::IsNone, vec![value]),
                    };
                    if compare.ops[0] == ast::CmpOp::IsNot {
                        result = Expr {
                            ty: Type::Bool,
                            kind: ExprKind::Not(Box::new(result)),
                        };
                    }
                    return Ok(result);
                }
                let values = compare
                    .operands
                    .iter()
                    .map(|value| self.expr(value))
                    .collect::<Result<Vec<_>, _>>()?;
                let mut ops = Vec::new();
                for (&op, pair) in compare.ops.iter().zip(values.windows(2)) {
                    let (ty, right) = (pair[0].ty, pair[1].ty);
                    let numeric = matches!(ty, Type::Int | Type::Float)
                        && matches!(right, Type::Int | Type::Float);
                    if !numeric {
                        self.expect(ty, right, expression)?;
                    }
                    let op = match op {
                        ast::CmpOp::Eq => Compare::Equal,
                        ast::CmpOp::NotEq => Compare::NotEqual,
                        ast::CmpOp::Lt if numeric || ty == Type::Bytes => Compare::Less,
                        ast::CmpOp::LtE if numeric || ty == Type::Bytes => Compare::LessEqual,
                        ast::CmpOp::Gt if numeric || ty == Type::Bytes => Compare::Greater,
                        ast::CmpOp::GtE if numeric || ty == Type::Bytes => Compare::GreaterEqual,
                        _ => {
                            return Err(error(
                                self.source,
                                expression,
                                "unsupported comparison for these types",
                            ));
                        }
                    };
                    if matches!(
                        ty,
                        Type::ListInt
                            | Type::List(_)
                            | Type::Dict(_)
                            | Type::Set(_)
                            | Type::Tuple(_)
                            | Type::Optional(_)
                    ) {
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
                let list = self.expr(&subscript.value)?;
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
                let receiver = self.expr(&attribute.value)?;
                if matches!(receiver.ty, Type::TextIO | Type::BinaryIO)
                    && attribute.attr.as_str() == "closed"
                {
                    return Ok(Expr {
                        ty: Type::Bool,
                        kind: ExprKind::Builtin(Builtin::TextClosed, vec![receiver]),
                    });
                }
                let (receiver, field, ty) = self.field_value(attribute, receiver)?;
                (ty, ExprKind::Field(Box::new(receiver), field))
            }
            ast::Expr::Call(call) => return self.call(call),
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
                                if field.debug_text.is_some() {
                                    return Err(error(
                                        self.source,
                                        field,
                                        "debug f-string fields are unsupported",
                                    ));
                                }
                                if !matches!(
                                    field.conversion,
                                    ast::ConversionFlag::None | ast::ConversionFlag::Str
                                ) {
                                    return Err(error(
                                        self.source,
                                        field,
                                        "f-string conversions support only !s",
                                    ));
                                }
                                if field
                                    .format_spec
                                    .as_ref()
                                    .is_some_and(|spec| !spec.elements.is_empty())
                                {
                                    return Err(error(
                                        self.source,
                                        field,
                                        "f-string format specifications must be empty",
                                    ));
                                }
                                let expression = self.expr(&field.expression)?;
                                if !matches!(
                                    expression.ty,
                                    Type::Int
                                        | Type::Float
                                        | Type::Bool
                                        | Type::Str
                                        | Type::Bytes
                                        | Type::None
                                        | Type::Exception
                                ) {
                                    return Err(error(
                                        self.source,
                                        field,
                                        "f-string fields require exactly int, float, bool, str, bytes, None, or Exception",
                                    ));
                                }
                                if expression.ty == Type::Str {
                                    expression
                                } else {
                                    Expr {
                                        ty: Type::Str,
                                        kind: ExprKind::Builtin(Builtin::Str, vec![expression]),
                                    }
                                }
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
        if call
            .arguments
            .args
            .iter()
            .any(|argument| matches!(argument, ast::Expr::Starred(_)))
            || call
                .arguments
                .keywords
                .iter()
                .any(|keyword| keyword.arg.is_none())
        {
            return Err(error(
                self.source,
                call,
                "argument unpacking is unsupported",
            ));
        }
        if matches!(&*call.func, ast::Expr::Name(name) if name.id == "open") {
            return self.open(call);
        }
        if let Some(kind) = self.exception_category(&call.func) {
            self.reject_builtin_keywords(call)?;
            if matches!(kind, 0 | 18) {
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
        if let Some(binding) = self.graph.resolve(self.module, &call.func) {
            let Binding::Definition(definition) = binding else {
                return Err(error(self.source, call, "a module is not callable"));
            };
            if let Some(&class_id) = self.classes.declarations.ids.get(&definition) {
                let class = &self.classes.declarations.definitions[class_id];
                let initializer_name = class.methods[class.initializer].name.as_str();
                let signature = self.classes.methods[class_id][initializer_name].clone();
                let args = self.arguments(call, &signature.params[1..], None)?;
                self.calls.insert(signature.id);
                return Ok(Expr {
                    ty: Type::Class(class_id),
                    kind: ExprKind::Construct {
                        initializer: signature.id,
                        args,
                    },
                });
            }
            let signature = self
                .functions
                .get(&definition)
                .ok_or_else(|| error(self.source, call, "definition is not a callable function"))?
                .clone();
            let args = self.arguments(call, &signature.params, None)?;
            self.calls.insert(signature.id);
            return Ok(Expr {
                ty: signature.returns,
                kind: ExprKind::Call(signature.id, args),
            });
        }
        if let ast::Expr::Attribute(attribute) = &*call.func {
            let receiver = self.expr(&attribute.value)?;
            if let Type::Class(class) = receiver.ty {
                if attribute.attr.as_str().starts_with("__") {
                    return Err(error(
                        self.source,
                        call,
                        "special methods cannot be called directly",
                    ));
                }
                let signature = self.classes.methods[class]
                    .get(attribute.attr.as_str())
                    .ok_or_else(|| error(self.source, call, "unknown instance method"))?
                    .clone();
                let args = self.arguments(call, &signature.params[1..], Some(receiver))?;
                self.calls.insert(signature.id);
                return Ok(Expr {
                    ty: signature.returns,
                    kind: ExprKind::Call(signature.id, args),
                });
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
                            "supported file methods are read(), write(data), flush(), and close()",
                        ));
                    }
                };
                let mut args = vec![receiver];
                if let Some(value) = call.arguments.args.first() {
                    args.push(self.expr_expected(value, data_type)?);
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
                && matches!(attribute.attr.as_str(), "split" | "splitlines")
                && call.arguments.args.is_empty()
            {
                return Ok(Expr {
                    ty: self.types.list(Type::Str),
                    kind: ExprKind::Builtin(
                        if attribute.attr.as_str() == "split" {
                            Builtin::Split
                        } else {
                            Builtin::SplitLines
                        },
                        vec![receiver],
                    ),
                });
            }
            if let Some((key_type, value_type)) = self.types.mapping(receiver.ty) {
                if attribute.attr.as_str() != "get" || !(1..=2).contains(&call.arguments.args.len())
                {
                    return Err(error(
                        self.source,
                        call,
                        "supported dictionary methods are get(key[, default]) and loop-head items()",
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
                        "supported list methods are append(value) and pop()",
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
        self.reject_builtin_keywords(call)?;
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
        if name.id == "print" && args.len() == 1 && args[0].ty == Type::Exception {
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
    fn open(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        let [path, ast::Expr::StringLiteral(mode)] = call.arguments.args.as_ref() else {
            return Err(error(
                self.source,
                call,
                "open requires a path and a literal supported text or binary mode",
            ));
        };
        check_string(mode, self.source)?;
        if let Some(mode) = match mode.value.to_str() {
            "rb" => Some(0),
            "wb" => Some(1),
            "ab" => Some(2),
            "xb" => Some(3),
            _ => None,
        } {
            self.reject_builtin_keywords(call)?;
            return Ok(Expr {
                ty: Type::BinaryIO,
                kind: ExprKind::Builtin(
                    Builtin::BinaryOpen(mode),
                    vec![self.expr_expected(path, Type::Str)?],
                ),
            });
        }
        let mode = match mode.value.to_str() {
            "r" => 0,
            "w" => 1,
            _ => {
                return Err(error(
                    self.source,
                    call,
                    "open supports literal 'r', 'w', 'rb', 'wb', 'ab', or 'xb' mode",
                ));
            }
        };
        let [encoding] = call.arguments.keywords.as_ref() else {
            return Err(error(
                self.source,
                call,
                "open requires only encoding='utf-8' as a keyword",
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
                "open requires encoding='utf-8'",
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
        Ok(Expr {
            ty: Type::TextIO,
            kind: ExprKind::Builtin(
                Builtin::TextOpen(mode),
                vec![self.expr_expected(path, Type::Str)?],
            ),
        })
    }
}
