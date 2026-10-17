//! Enforces the closed typed subset independently of gradual Python typing.

use std::collections::{HashMap, HashSet};

use ruff_python_ast as ast;
use ruff_text_size::Ranged;

use crate::classes::Classes;
use crate::ir::{self, Binary, Builtin, Compare, Expr, ExprKind, Statement, Type};
use crate::modules::{Binding, DefinitionId, ModuleId, SourceGraph};

#[derive(Clone)]
struct Signature {
    id: usize,
    params: Vec<Type>,
    returns: Type,
}

struct ClassInfo {
    declarations: Classes,
    fields: Vec<Vec<Type>>,
    methods: Vec<HashMap<String, Signature>>,
}

impl ClassInfo {
    /// Resolves all nominal field types before checking constructors and methods.
    fn new(graph: &SourceGraph) -> Result<Self, String> {
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
        fn visit(id: usize, fields: &[Vec<Type>], states: &mut [u8]) -> bool {
            if states[id] != 0 {
                return states[id] == 2;
            }
            states[id] = 1;
            for field in &fields[id] {
                if let Type::Class(next) = field
                    && !visit(*next, fields, states)
                {
                    return false;
                }
            }
            states[id] = 2;
            true
        }
        let mut states = vec![0; fields.len()];
        for (id, class) in declarations.definitions.iter().enumerate() {
            if !visit(id, &fields, &mut states) {
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
    "int", "bool", "str", "list", "len", "input", "print", "range", "None", "True", "False",
    "__name__",
];

/// Resolves the project-wide call graph without changing source-level namespaces.
pub(crate) fn lower(graph: &SourceGraph) -> Result<ir::Module, String> {
    let mut classes = ClassInfo::new(graph)?;
    let mut signatures = HashMap::new();
    let mut definitions = Vec::new();
    for (module_id, module) in graph.modules.iter().enumerate() {
        for name in module.bindings.keys() {
            if RESERVED.contains(&name.as_str()) {
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
    let mut functions = Vec::new();
    let mut call_graph = Vec::new();
    for (module_id, definition, signature) in &definitions {
        let module = &graph.modules[*module_id];
        let result = (|| {
            let mut checker =
                Checker::new(graph, *module_id, &signatures, &classes, signature.returns);
            let mut params = Vec::new();
            for (parameter, &ty) in definition
                .parameters
                .iter_non_variadic_params()
                .zip(&signature.params)
            {
                let slot = checker.declare(parameter.name().as_str(), ty, parameter)?;
                checker.assigned.insert(slot);
                params.push(slot);
            }
            let (body, falls_through) = checker.block(&definition.body)?;
            if falls_through && signature.returns != Type::None {
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
    let mut states = vec![0; functions.len()];
    for (id, (module, node, _)) in definitions.iter().enumerate() {
        if !visit(id, &call_graph, &mut states) {
            return Err(graph.modules[*module]
                .error(*node, "recursive calls are unsupported in this prototype"));
        }
    }
    let mut entry = None;
    for (module_id, module) in graph.modules.iter().enumerate() {
        let mut checker = Checker::new(graph, module_id, &signatures, &classes, Type::None);
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
    Ok(ir::Module {
        functions,
        entry: entry.expect("source graph has an entry module"),
    })
}

fn signature(
    function: &ast::StmtFunctionDef,
    graph: &SourceGraph,
    module: ModuleId,
    classes: &Classes,
    id: usize,
    receiver: Option<Type>,
) -> Result<Signature, String> {
    let source = &graph.modules[module].source;
    if function.is_async
        || !function.decorator_list.is_empty()
        || function.type_params.is_some()
        || function.parameters.vararg.is_some()
        || function.parameters.kwarg.is_some()
        || !function.parameters.kwonlyargs.is_empty()
    {
        return Err(error(
            source,
            function,
            "async, decorators, type parameters, and variadic or keyword-only parameters are unsupported",
        ));
    }
    let mut params = Vec::new();
    for (index, parameter) in function.parameters.iter_non_variadic_params().enumerate() {
        if parameter.default.is_some() {
            return Err(error(
                source,
                parameter,
                "default parameter values are unsupported",
            ));
        }
        let ty = if let Some(receiver) = receiver
            && index == 0
        {
            if let Some(declared) = parameter.annotation()
                && annotation(declared, graph, module, classes)? != receiver
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
            )?
        };
        params.push(ty);
    }
    let returns = annotation(
        function
            .returns
            .as_deref()
            .ok_or_else(|| error(source, function, "function requires a return annotation"))?,
        graph,
        module,
        classes,
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
) -> Result<Type, String> {
    let source = &graph.modules[module].source;
    let binding = if let ast::Expr::StringLiteral(value) = expression {
        check_string(value, source)?;
        let mut names = value.value.to_str().split('.');
        let mut binding = names
            .next()
            .and_then(|name| graph.modules[module].bindings.get(name))
            .copied();
        for name in names {
            binding = match binding {
                Some(Binding::Module(module)) => graph.modules[module].bindings.get(name).copied(),
                _ => None,
            };
        }
        binding
    } else {
        graph.resolve(module, expression)
    };
    if let Some(Binding::Definition(definition)) = binding
        && let Some(&id) = classes.ids.get(&definition)
    {
        return Ok(Type::Class(id));
    }
    match expression {
        ast::Expr::Name(name) => match name.id.as_str() {
            "int" => Ok(Type::Int),
            "bool" => Ok(Type::Bool),
            "str" => Ok(Type::Str),
            _ => Err(error(
                source,
                expression,
                "supported annotations are int, bool, str, None, list[int], and fixed classes",
            )),
        },
        ast::Expr::NoneLiteral(_) => Ok(Type::None),
        ast::Expr::Subscript(value)
            if matches!(&*value.value, ast::Expr::Name(name) if name.id == "list")
                && matches!(&*value.slice, ast::Expr::Name(name) if name.id == "int") =>
        {
            Ok(Type::ListInt)
        }
        _ => Err(error(source, expression, "unsupported type annotation")),
    }
}

/// Ruff replaces surrogate escapes with U+FFFD in its analysis AST. Reject those
/// source literals rather than silently compiling a different Python value.
fn check_string(value: &ast::ExprStringLiteral, source: &str) -> Result<(), String> {
    for part in value.value.iter() {
        if matches!(
            part.flags.prefix(),
            ast::str_prefix::StringLiteralPrefix::Raw { .. }
        ) {
            continue;
        }
        let range = part.content_range();
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
                    value,
                    "surrogate string escapes are unsupported; strings must be valid UTF-8",
                ));
            }
            i += 2 + width;
        }
    }
    Ok(())
}

struct Checker<'a> {
    source: &'a str,
    graph: &'a SourceGraph,
    module: ModuleId,
    functions: &'a HashMap<DefinitionId, Signature>,
    classes: &'a ClassInfo,
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
        returns: Type,
    ) -> Self {
        Self {
            source: &graph.modules[module].source,
            graph,
            module,
            functions,
            classes,
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
        if RESERVED.contains(&name) || self.graph.modules[self.module].bindings.contains_key(name) {
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
        if let Type::Class(id) = ty {
            let class = &self.classes.declarations.definitions[id];
            format!(
                "{}.{}",
                self.graph.modules[class.definition.module].name, class.name
            )
        } else {
            ty.to_string()
        }
    }

    /// Resolves a fixed field without evaluating its receiver more than once.
    fn field(&mut self, attribute: &ast::ExprAttribute) -> Result<(Expr, usize, Type), String> {
        let receiver = self.expr(&attribute.value)?;
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

    /// Checks direct-call arguments in source order against the fixed signature.
    fn arguments(&mut self, call: &ast::ExprCall, params: &[Type]) -> Result<Vec<Expr>, String> {
        if call.arguments.args.len() != params.len() {
            return Err(error(
                self.source,
                call,
                "incorrect number of function arguments",
            ));
        }
        call.arguments
            .args
            .iter()
            .zip(params)
            .map(|(argument, &expected)| {
                let argument = self.expr(argument)?;
                self.expect(expected, argument.ty, call)?;
                Ok(argument)
            })
            .collect()
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
                let value = self.expr(&assign.value)?;
                Some(self.assign(&assign.targets[0], value)?)
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
                )?;
                let slot = self.declare(name.id.as_str(), ty, statement)?;
                if let Some(value) = &assign.value {
                    let value = self.expr(value)?;
                    self.expect(ty, value.ty, statement)?;
                    self.assigned.insert(slot);
                    Some(Statement::Assign(slot, value))
                } else {
                    None
                }
            }
            ast::Stmt::AugAssign(assign) => {
                if let ast::Expr::Subscript(target) = &*assign.target {
                    let list = self.expr(&target.value)?;
                    let index = self.expr(&target.slice)?;
                    self.expect(Type::ListInt, list.ty, target)?;
                    self.expect(Type::Int, index.ty, target)?;
                    let value = self.expr(&assign.value)?;
                    self.expect(Type::Int, value.ty, statement)?;
                    let op = self
                        .binary_op(assign.op, Type::Int, Type::Int, statement)?
                        .0;
                    Some(Statement::AugmentItem(list, index, op, value))
                } else if let ast::Expr::Attribute(target) = &*assign.target {
                    let (receiver, field, ty) = self.field(target)?;
                    self.expect(Type::Int, ty, statement)?;
                    let value = self.expr(&assign.value)?;
                    self.expect(Type::Int, value.ty, statement)?;
                    let op = self
                        .binary_op(assign.op, Type::Int, Type::Int, statement)?
                        .0;
                    Some(Statement::AugmentField(receiver, field, op, value))
                } else if let ast::Expr::Name(_) = &*assign.target {
                    let left = self.expr(&assign.target)?;
                    let right = self.expr(&assign.value)?;
                    let (op, ty) = self.binary_op(assign.op, left.ty, right.ty, statement)?;
                    if ty == Type::ListInt {
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
                let test = self.condition(&loop_.test)?;
                let before = self.assigned.clone();
                self.loop_depth += 1;
                let (body, _) = self.block(&loop_.body)?;
                self.loop_depth -= 1;
                self.assigned = before;
                Some(Statement::While(test, body))
            }
            ast::Stmt::For(loop_) if !loop_.is_async && loop_.orelse.is_empty() => {
                let ast::Expr::Call(call) = &*loop_.iter else {
                    return Err(error(
                        self.source,
                        statement,
                        "for loops require range(...)",
                    ));
                };
                if !matches!(&*call.func, ast::Expr::Name(name) if name.id == "range")
                    || !call.arguments.keywords.is_empty()
                    || !(1..=3).contains(&call.arguments.args.len())
                {
                    return Err(error(
                        self.source,
                        statement,
                        "for loops require range with one to three positional integer arguments",
                    ));
                }
                let mut args = Vec::new();
                for arg in &call.arguments.args {
                    let value = self.expr(arg)?;
                    self.expect(Type::Int, value.ty, arg)?;
                    args.push(value);
                }
                let ast::Expr::Name(name) = &*loop_.target else {
                    return Err(error(self.source, statement, "range target must be a name"));
                };
                let target = self.declare(name.id.as_str(), Type::Int, &*loop_.target)?;
                let before = self.assigned.clone();
                self.assigned.insert(target);
                self.loop_depth += 1;
                let (body, _) = self.block(&loop_.body)?;
                self.loop_depth -= 1;
                self.assigned = before;
                Some(Statement::For {
                    target,
                    cursor: self.hidden(Type::Int),
                    stop: self.hidden(Type::Int),
                    step: self.hidden(Type::Int),
                    args,
                    body,
                })
            }
            ast::Stmt::Return(return_) if !self.module_scope => {
                let value = if let Some(value) = &return_.value {
                    self.expr(value)?
                } else {
                    Expr {
                        ty: Type::None,
                        kind: ExprKind::None,
                    }
                };
                self.expect(self.returns, value.ty, statement)?;
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
            ast::Stmt::Assert(assert) if assert.msg.is_none() => {
                Some(Statement::Assert(self.condition(&assert.test)?))
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

    fn if_chain(
        &mut self,
        test: &ast::Expr,
        body: &[ast::Stmt],
        clauses: &[ast::ElifElseClause],
    ) -> Result<(Statement, bool), String> {
        let test = self.condition(test)?;
        let before = self.assigned.clone();
        let (yes, yes_falls) = self.block(body)?;
        let yes_assigned = self.assigned.clone();
        self.assigned = before;
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
        } else if yes_falls {
            self.assigned = yes_assigned;
        }
        Ok((Statement::If(test, yes, no), yes_falls || no_falls))
    }

    fn assign(&mut self, target: &ast::Expr, value: Expr) -> Result<Statement, String> {
        match target {
            ast::Expr::Name(name) => {
                let slot = self.declare(name.id.as_str(), value.ty, target)?;
                self.assigned.insert(slot);
                Ok(Statement::Assign(slot, value))
            }
            ast::Expr::Subscript(subscript) => {
                self.expect(Type::Int, value.ty, target)?;
                let list = self.expr(&subscript.value)?;
                let index = self.expr(&subscript.slice)?;
                self.expect(Type::ListInt, list.ty, target)?;
                self.expect(Type::Int, index.ty, target)?;
                Ok(Statement::SetItem(list, index, value))
            }
            ast::Expr::Attribute(attribute) => {
                let (receiver, field, ty) = self.field(attribute)?;
                self.expect(ty, value.ty, target)?;
                Ok(Statement::SetField(receiver, field, value))
            }
            _ => Err(error(self.source, target, "unsupported assignment target")),
        }
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
        if op == ast::Operator::Mult
            && ((left == Type::ListInt && right == Type::Int)
                || (left == Type::Int && right == Type::ListInt))
        {
            return Ok((Binary::Repeat, Type::ListInt));
        }
        self.expect(Type::Int, left, node)?;
        self.expect(Type::Int, right, node)?;
        let op = match op {
            ast::Operator::Add => Binary::Add,
            ast::Operator::Sub => Binary::Subtract,
            ast::Operator::Mult => Binary::Multiply,
            ast::Operator::FloorDiv => Binary::FloorDivide,
            ast::Operator::Mod => Binary::Modulo,
            _ => return Err(error(self.source, node, "unsupported integer operator")),
        };
        Ok((op, Type::Int))
    }

    fn expr(&mut self, expression: &ast::Expr) -> Result<Expr, String> {
        let (ty, kind) = match expression {
            ast::Expr::NumberLiteral(value) => match &value.value {
                ast::Number::Int(value) => (Type::Int, ExprKind::Int(value.to_string())),
                _ => {
                    return Err(error(
                        self.source,
                        expression,
                        "only integer numeric literals are supported",
                    ));
                }
            },
            ast::Expr::BooleanLiteral(value) => (Type::Bool, ExprKind::Bool(value.value)),
            ast::Expr::StringLiteral(value) => {
                check_string(value, self.source)?;
                (Type::Str, ExprKind::Str(value.value.to_string()))
            }
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
                let slot = self.names.get(name.id.as_str()).copied().ok_or_else(|| error(self.source, expression, "name is not a local value; globals and first-class functions are unsupported"))?;
                if !self.assigned.contains(&slot) {
                    return Err(error(
                        self.source,
                        expression,
                        "local may be uninitialized on this path",
                    ));
                }
                (self.locals[slot], ExprKind::Local(slot))
            }
            ast::Expr::List(list) => {
                let mut values = Vec::new();
                for value in &list.elts {
                    let item = self.expr(value)?;
                    self.expect(Type::Int, item.ty, value)?;
                    values.push(item);
                }
                (Type::ListInt, ExprKind::List(values))
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
                        self.expect(Type::Int, value.ty, expression)?;
                        if unary.op == ast::UnaryOp::UAdd {
                            return Ok(value);
                        }
                        (Type::Int, ExprKind::Negate(Box::new(value)))
                    }
                    _ => return Err(error(self.source, expression, "unsupported unary operator")),
                }
            }
            ast::Expr::BoolOp(boolean) => {
                let values = boolean
                    .values
                    .iter()
                    .map(|value| self.condition(value))
                    .collect::<Result<Vec<_>, _>>()?;
                (
                    Type::Bool,
                    ExprKind::Boolean(boolean.op == ast::BoolOp::And, values),
                )
            }
            ast::Expr::Compare(compare) => {
                let values = compare
                    .operands
                    .iter()
                    .map(|value| self.expr(value))
                    .collect::<Result<Vec<_>, _>>()?;
                let ty = values[0].ty;
                let mut ops = Vec::new();
                for (&op, value) in compare.ops.iter().zip(&values[1..]) {
                    self.expect(ty, value.ty, expression)?;
                    let op = match op {
                        ast::CmpOp::Eq => Compare::Equal,
                        ast::CmpOp::NotEq => Compare::NotEqual,
                        ast::CmpOp::Lt if ty == Type::Int => Compare::Less,
                        ast::CmpOp::LtE if ty == Type::Int => Compare::LessEqual,
                        ast::CmpOp::Gt if ty == Type::Int => Compare::Greater,
                        ast::CmpOp::GtE if ty == Type::Int => Compare::GreaterEqual,
                        _ => {
                            return Err(error(
                                self.source,
                                expression,
                                "unsupported comparison for these types",
                            ));
                        }
                    };
                    if ty == Type::ListInt {
                        return Err(error(
                            self.source,
                            expression,
                            "list comparison is unsupported",
                        ));
                    }
                    ops.push(op);
                }
                (Type::Bool, ExprKind::Compare(values, ops))
            }
            ast::Expr::Subscript(subscript) => {
                let list = self.expr(&subscript.value)?;
                let index = self.expr(&subscript.slice)?;
                self.expect(Type::ListInt, list.ty, expression)?;
                self.expect(Type::Int, index.ty, expression)?;
                (
                    Type::Int,
                    ExprKind::Builtin(Builtin::ListGet, vec![list, index]),
                )
            }
            ast::Expr::Attribute(attribute) => {
                let (receiver, field, ty) = self.field(attribute)?;
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

    fn call(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        if !call.arguments.keywords.is_empty() {
            return Err(error(
                self.source,
                call,
                "keyword arguments are unsupported",
            ));
        }
        if let Some(binding) = self.graph.resolve(self.module, &call.func) {
            let Binding::Definition(definition) = binding else {
                return Err(error(self.source, call, "a module is not callable"));
            };
            if let Some(&class_id) = self.classes.declarations.ids.get(&definition) {
                let class = &self.classes.declarations.definitions[class_id];
                let initializer_name = class.methods[class.initializer].name.as_str();
                let signature = self.classes.methods[class_id][initializer_name].clone();
                let fields = class.fields.len();
                let args = self.arguments(call, &signature.params[1..])?;
                self.calls.insert(signature.id);
                return Ok(Expr {
                    ty: Type::Class(class_id),
                    kind: ExprKind::Construct {
                        initializer: signature.id,
                        fields,
                        args,
                    },
                });
            }
            let signature = self
                .functions
                .get(&definition)
                .ok_or_else(|| error(self.source, call, "definition is not a callable function"))?
                .clone();
            let args = self.arguments(call, &signature.params)?;
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
                let mut args = vec![receiver];
                args.extend(self.arguments(call, &signature.params[1..])?);
                self.calls.insert(signature.id);
                return Ok(Expr {
                    ty: signature.returns,
                    kind: ExprKind::Call(signature.id, args),
                });
            }
            self.expect(Type::ListInt, receiver.ty, call)?;
            let mut args = vec![receiver];
            for arg in &call.arguments.args {
                args.push(self.expr(arg)?);
            }
            let (builtin, ty) = match attribute.attr.as_str() {
                "append" if args.len() == 2 => {
                    self.expect(Type::Int, args[1].ty, call)?;
                    (Builtin::Append, Type::None)
                }
                "pop" if args.len() == 1 => (Builtin::Pop, Type::Int),
                _ => {
                    return Err(error(
                        self.source,
                        call,
                        "supported methods are list[int].append(int) and .pop()",
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
        let args = call
            .arguments
            .args
            .iter()
            .map(|arg| self.expr(arg))
            .collect::<Result<Vec<_>, _>>()?;
        let (builtin, ty) = match name.id.as_str() {
            "input" if args.is_empty() => (Builtin::Input, Type::Str),
            "int"
                if args.len() == 1 && matches!(args[0].ty, Type::Int | Type::Bool | Type::Str) =>
            {
                (Builtin::Int, Type::Int)
            }
            "len" if args.len() == 1 && matches!(args[0].ty, Type::Str | Type::ListInt) => {
                (Builtin::Len, Type::Int)
            }
            "print"
                if args.len() == 1
                    && matches!(args[0].ty, Type::Int | Type::Bool | Type::Str | Type::None) =>
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
}
