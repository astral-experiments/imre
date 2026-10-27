//! In-process execution of source received by the boxed Python runtime.

use std::collections::HashSet;
use std::rc::Rc;

use ruff_python_ast as ast;
use ruff_text_size::Ranged;

use super::value::{Body, ExecGlobals, Location};
use super::*;

pub(super) struct Program {
    pub filename: Rc<str>,
    pub name: String,
    first_line: u32,
    generator: bool,
    locals: HashSet<String>,
    instructions: Vec<(u32, Instruction)>,
}

enum Instruction {
    Eval(ast::Expr),
    Assign(String, ast::Expr),
    From(String, Vec<(String, String)>),
    Define(Rc<Program>, Box<ast::Parameters>),
    Yield(Option<ast::Expr>),
    Return(Option<ast::Expr>),
    Raise(ast::StmtRaise),
    Unless(ast::Expr, usize),
    Iter(ast::Expr),
    Next(Option<ast::Expr>, usize),
    Jump(usize),
}

enum Step {
    Yield(Id, usize),
    Return(Id),
}

fn unsupported(what: impl std::fmt::Display) -> Error {
    Error::new("NotImplementedError", format!("runtime source: {what}"))
}

impl Program {
    fn new(filename: Rc<str>, name: &str) -> Self {
        Self {
            filename,
            name: name.to_owned(),
            first_line: 1,
            generator: false,
            locals: HashSet::new(),
            instructions: Vec::new(),
        }
    }

    fn lower(&mut self, body: &[ast::Stmt], lines: &[usize], function: bool) -> Result<()> {
        use ast::Stmt;
        for stmt in body {
            let line = lines.partition_point(|n| *n <= stmt.start().to_usize()) as u32;
            let op = match stmt {
                Stmt::Pass(_) => continue,
                Stmt::Expr(s) => match &*s.value {
                    ast::Expr::Yield(y) => {
                        if !function {
                            return Err(Error::new("SyntaxError", "'yield' outside function"));
                        }
                        self.generator = true;
                        Instruction::Yield(y.value.as_deref().cloned())
                    }
                    ast::Expr::YieldFrom(y) if function => {
                        self.generator = true;
                        self.instructions
                            .push((line, Instruction::Iter((*y.value).clone())));
                        Instruction::Next(None, self.instructions.len() + 1)
                    }
                    e => Instruction::Eval(e.clone()),
                },
                Stmt::Assign(s) => {
                    let [ast::Expr::Name(n)] = s.targets.as_slice() else {
                        return Err(unsupported("assignment target"));
                    };
                    let n = n.id.to_string();
                    self.locals.insert(n.clone());
                    Instruction::Assign(n, (*s.value).clone())
                }
                Stmt::AnnAssign(s) if function => {
                    let ast::Expr::Name(name) = &*s.target else {
                        return Err(unsupported("annotated assignment target"));
                    };
                    let name = name.id.to_string();
                    self.locals.insert(name.clone());
                    // Local annotations establish the binding without evaluating the annotation.
                    let Some(value) = &s.value else { continue };
                    Instruction::Assign(name, (**value).clone())
                }
                Stmt::Return(s) => {
                    if !function {
                        return Err(Error::new("SyntaxError", "'return' outside function"));
                    }
                    Instruction::Return(s.value.as_deref().cloned())
                }
                Stmt::Raise(s) => Instruction::Raise(s.clone()),
                Stmt::ImportFrom(s) if s.level == 0 && !s.is_lazy => {
                    let module = s
                        .module
                        .as_ref()
                        .ok_or_else(|| unsupported("relative import"))?;
                    let mut names = vec![];
                    for alias in &s.names {
                        if alias.name.as_str() == "*" {
                            return Err(unsupported("star import"));
                        }
                        let local = alias.asname.as_ref().unwrap_or(&alias.name).to_string();
                        self.locals.insert(local.clone());
                        names.push((alias.name.to_string(), local));
                    }
                    Instruction::From(module.to_string(), names)
                }
                Stmt::FunctionDef(s)
                    if !s.is_async
                        && s.decorator_list.is_empty()
                        && s.type_params.is_none()
                        && !function =>
                {
                    let mut child = Program::new(self.filename.clone(), s.name.as_str());
                    child.first_line = line;
                    child.lower(&s.body, lines, true)?;
                    self.locals.insert(child.name.clone());
                    Instruction::Define(Rc::new(child), s.parameters.clone())
                }
                Stmt::For(s) if !s.is_async => {
                    self.loop_target(&s.target)?;
                    self.instructions
                        .push((line, Instruction::Iter((*s.iter).clone())));
                    let head = self.instructions.len();
                    self.instructions.push((line, Instruction::Jump(0)));
                    self.lower(&s.body, lines, function)?;
                    self.instructions.push((line, Instruction::Jump(head)));
                    self.instructions[head].1 =
                        Instruction::Next(Some((*s.target).clone()), self.instructions.len());
                    self.lower(&s.orelse, lines, function)?;
                    continue;
                }
                Stmt::If(s) => {
                    let mut ends = Vec::new();
                    let mut arm = Some((&*s.test, s.body.as_slice()));
                    for clause in &s.elif_else_clauses {
                        if let Some((test, body)) = arm.take() {
                            self.arm(test, body, line, lines, function, &mut ends)?;
                        }
                        if let Some(test) = &clause.test {
                            arm = Some((test, &clause.body));
                        } else {
                            self.lower(&clause.body, lines, function)?;
                        }
                    }
                    if let Some((test, body)) = arm {
                        self.arm(test, body, line, lines, function, &mut ends)?;
                    }
                    let end = self.instructions.len();
                    for i in ends {
                        self.instructions[i].1 = Instruction::Jump(end);
                    }
                    continue;
                }
                _ => {
                    return Err(unsupported(format!(
                        "{:?}",
                        ast::AnyNodeRef::from(stmt).kind()
                    )));
                }
            };
            self.instructions.push((line, op));
        }
        Ok(())
    }

    fn loop_target(&mut self, target: &ast::Expr) -> Result<()> {
        match target {
            ast::Expr::Name(n) => {
                self.locals.insert(n.id.to_string());
            }
            ast::Expr::Tuple(t) => {
                for e in &t.elts {
                    self.loop_target(e)?;
                }
            }
            _ => return Err(unsupported("for target")),
        }
        Ok(())
    }

    fn arm(
        &mut self,
        test: &ast::Expr,
        body: &[ast::Stmt],
        line: u32,
        lines: &[usize],
        function: bool,
        ends: &mut Vec<usize>,
    ) -> Result<()> {
        let index = self.instructions.len();
        self.instructions.push((line, Instruction::Jump(0)));
        self.lower(body, lines, function)?;
        ends.push(self.instructions.len());
        self.instructions.push((line, Instruction::Jump(0)));
        self.instructions[index].1 = Instruction::Unless(test.clone(), self.instructions.len());
        Ok(())
    }
}

impl Context {
    pub(super) fn compile_source(&mut self, source: Id, filename: Id, mode: Id) -> Result<Id> {
        let mode = self.str_value(mode)?;
        if mode != "exec" {
            return Err(if matches!(mode, "eval" | "single") {
                unsupported(format!("compile mode {mode}"))
            } else {
                Error::new(
                    "ValueError",
                    "compile() mode must be 'exec', 'eval' or 'single'",
                )
            });
        }
        let source = self.str_value(source)?;
        let filename = self.str_value(filename)?;
        let parsed = ruff_python_parser::parse_module(source)
            .map_err(|error| Error::new("SyntaxError", format!("{filename}: {error}")))?;
        let lines = std::iter::once(0)
            .chain(source.match_indices('\n').map(|(offset, _)| offset + 1))
            .collect::<Vec<_>>();
        let mut code = Program::new(Rc::from(filename), "<module>");
        code.lower(&parsed.syntax().body, &lines, false)?;
        Ok(self.box_source_code(Rc::new(code), None))
    }

    fn box_source_code(&mut self, program: Rc<Program>, parameters: Option<Id>) -> Id {
        let name = self.text(&program.name);
        let filename = self.text(program.filename.as_ref());
        self.alloc(Value::Code(value::CodeObject {
            name,
            filename,
            first_line: program.first_line,
            body: Body::Source(program),
            module_definition: true,
            coroutine: false,
            parameters: parameters.unwrap_or(NONE),
            nested: Vec::new(),
        }))
    }

    /// Define a function whose parameters and executable belong to the generated code object.
    fn source_function(&mut self, code: Id, scope: Id, defaults: Id) -> Id {
        let name = self.code_object(code).name;
        self.define_function(code, scope, name, defaults, name, NONE, NONE, false)
    }

    pub(super) fn dict_named_item(&self, dict: Id, name: &str) -> Option<Id> {
        self.dict_entries(dict)?.iter().find_map(|&(key, value)| {
            matches!(self.heap.get(key), Value::Str(k) if k == name).then_some(value)
        })
    }

    pub(super) fn source_global(&mut self, globals: ExecGlobals, name: &str) -> Result<Id> {
        for mapping in [globals.dict, globals.builtins] {
            if matches!(self.heap.get(mapping), Value::Dict(_)) {
                if let Some(value) = self.dict_named_item(mapping, name) {
                    return Ok(value);
                }
            } else {
                match self.load_exec_name(mapping, name) {
                    Err(error) if self.is_builtin_error(&error, "KeyError") => {}
                    value => return value,
                }
            }
        }
        Err(Error::new(
            "NameError",
            format!("name '{name}' is not defined"),
        ))
    }

    fn default_source_builtins(&mut self) -> Result<Id> {
        if let Some(dict) = self.source_builtins {
            return Ok(dict);
        }
        let dict = self.dict(Vec::new());
        for name in builtins::names() {
            let k = self.text(name);
            let v = self.builtin(name);
            self.put_item(dict, k, v)?;
        }
        self.source_builtins = Some(dict);
        Ok(dict)
    }

    pub(super) fn exec_source(
        &mut self,
        source: Id,
        globals: Id,
        locals: Option<Id>,
    ) -> Result<Id> {
        if self.dict_storage(globals).is_none() {
            let ty = self.type_name(globals);
            return Err(Error::new(
                "TypeError",
                format!("exec() globals must be a dict, not {ty}"),
            ));
        }
        let effective_locals = locals.filter(|&id| id != NONE).unwrap_or(globals);
        let locals = (effective_locals != globals
            || !matches!(self.heap.get(globals), Value::Dict(_)))
        .then_some(effective_locals);
        if let Some(locals) = locals
            && !matches!(
                self.heap.get(locals),
                Value::Dict(_)
                    | Value::DefaultDict { .. }
                    | Value::Context(_)
                    | Value::List(_)
                    | Value::Tuple(_)
                    | Value::NamedTuple { .. }
                    | Value::Str(_)
                    | Value::Bytes(_)
                    | Value::Range { .. }
            )
            && self.slot(locals, "__getitem__").is_none()
        {
            return Err(Error::new(
                "TypeError",
                format!(
                    "locals must be a mapping or None, not {}",
                    self.type_name(locals)
                ),
            ));
        }
        let builtins = if let Some(builtins) = self.dict_named_item(globals, "__builtins__") {
            if let Value::Module { scope, .. } = *self.heap.get(builtins) {
                self.module_globals(scope)
            } else {
                builtins
            }
        } else {
            let builtins = self.default_source_builtins()?;
            let key = self.text("__builtins__");
            self.put_item(
                self.dict_storage(globals).expect("exec dictionary"),
                key,
                builtins,
            )?;
            builtins
        };
        let code_id = if matches!(self.heap.get(source), Value::Str(_)) {
            let filename = self.text("<string>");
            let mode = self.text("exec");
            self.compile_source(source, filename, mode)?
        } else {
            source
        };
        let Value::Code(code) = self.heap.get(code_id) else {
            return Err(Error::new(
                "TypeError",
                "exec() arg 1 must be a string or code object",
            ));
        };
        let function_code = code.parameters != NONE;
        let module_definition = code.module_definition;
        let scope = self.scope("<exec>", None);
        let Value::Scope(environment) = self.heap.get_mut(scope) else {
            unreachable!()
        };
        environment.namespace = Some(globals);
        environment.exec_globals = Some(ExecGlobals {
            dict: globals,
            builtins,
        });
        if function_code {
            if !module_definition {
                return Err(unsupported(
                    "exec of a prelinked function with an enclosing scope",
                ));
            }
            let defaults = self.dict(Vec::new());
            let function = self.source_function(code_id, scope, defaults);
            self.call(function, Vec::new(), Vec::new())?;
        } else {
            // Definitions retain the global frame. The module alone sees the separate mapping.
            let module = if let Some(locals) = locals {
                let module = self.scope("<exec>", Some(scope));
                let Value::Scope(frame) = self.heap.get_mut(module) else {
                    unreachable!()
                };
                frame.exec_locals = Some(locals);
                module
            } else {
                scope
            };
            self.execute(code_id, module)?;
        }
        Ok(NONE)
    }

    pub(super) fn call_source(&mut self, code: Id, scope: Id) -> Result<Id> {
        let generator = match &self.code_object(code).body {
            Body::Source(program) => program.generator,
            Body::PendingGenerator { .. } => true,
            Body::Native(_) => unreachable!("source function"),
        };
        if generator {
            Ok(self.alloc(Value::Generator {
                code,
                scope,
                iterators: Vec::new(),
                offset: Some(0),
                running: false,
            }))
        } else {
            match self.run_source(code, scope, 0, &mut Vec::new())? {
                Step::Return(v) => Ok(v),
                Step::Yield(..) => unreachable!("classified generator"),
            }
        }
    }

    fn prepare_generator(&mut self, id: Id) -> Result<()> {
        let code = self.code_object(id);
        let Body::PendingGenerator { source, body_line } = &code.body else {
            return Ok(());
        };
        let parsed = ruff_python_parser::parse_module(source)
            .map_err(|err| Error::new("SyntaxError", err.to_string()))?;
        let [ast::Stmt::FunctionDef(def)] = parsed.syntax().body.as_slice() else {
            return Err(Error::new("SystemError", "invalid generator body"));
        };
        let lines = std::iter::once(0)
            .chain(source.match_indices('\n').map(|(offset, _)| offset + 1))
            .collect::<Vec<_>>();
        let mut program = Program::new(
            self.str_value(code.filename)?.into(),
            self.str_value(code.name)?,
        );
        program.first_line = code.first_line;
        program.generator = true;
        program.lower(&def.body, &lines, true)?;
        for (line, _) in &mut program.instructions {
            *line = body_line + line.saturating_sub(2);
        }
        let Value::Code(code) = self.heap.get_mut(id) else {
            unreachable!()
        };
        code.body = Body::Source(Rc::new(program));
        Ok(())
    }

    pub(super) fn resume_source(&mut self, id: Id) -> Result<Option<Id>> {
        self.heap.keep(id);
        let Value::Generator {
            code,
            scope,
            iterators,
            running,
            offset,
        } = self.heap.get_mut(id)
        else {
            unreachable!()
        };
        if *running {
            return Err(Error::new("ValueError", "generator already executing"));
        }
        let Some(ip) = offset.take() else {
            return Ok(None);
        };
        *running = true;
        let (code, scope) = (*code, *scope);
        let mut active = std::mem::take(iterators);
        let result = self
            .prepare_generator(code)
            .and_then(|()| self.run_source(code, scope, ip, &mut active))
            .map_err(|error| self.generator_error(error));
        let Value::Generator {
            running,
            offset,
            scope,
            iterators,
            ..
        } = self.heap.get_mut(id)
        else {
            unreachable!()
        };
        *running = false;
        if !matches!(result, Ok(Step::Yield(..))) {
            *scope = NONE;
        }
        match result? {
            Step::Yield(value, ip) => {
                *offset = Some(ip);
                *iterators = active;
                Ok(Some(value))
            }
            Step::Return(NONE) => Ok(None),
            Step::Return(value) => Err(self.exception("StopIteration", &[value])),
        }
    }

    fn run_source(
        &mut self,
        owner: Id,
        scope: Id,
        mut ip: usize,
        iterators: &mut Vec<Id>,
    ) -> Result<Step> {
        let Body::Source(code) = self.code_object(owner).body.clone() else {
            unreachable!("source frame")
        };
        self.heap.keep(owner);
        self.heap.enter(scope, owner);
        let result = (|| {
            while let Some((line, op)) = code.instructions.get(ip) {
                let loc = Location {
                    file: code.filename.to_string().into(),
                    line: *line,
                    span: None,
                };
                // The frame owns its code and scope. Active iterators must also survive nested calls.
                self.checkpoint(iterators, loc, NONE);
                ip += 1;
                match self.source_step(&code, owner, scope, op, &mut ip, iterators) {
                    Ok(Some(step)) => return Ok(step),
                    Ok(None) => (),
                    Err(mut e) => {
                        self.record_exception_frame(&mut e, None);
                        return Err(e);
                    }
                }
            }
            Ok(Step::Return(NONE))
        })();
        self.heap.leave(match &result {
            Ok(Step::Yield(v, _) | Step::Return(v)) => *v,
            Err(_) => NONE,
        });
        result
    }

    pub(super) fn bind_source_name(&mut self, scope: Id, name: &str, value: Id) -> Result<()> {
        let scope = self.binding_scope(scope, name);
        if let Some(locals) = self.scope_data(scope).exec_locals {
            if matches!(self.heap.get(locals), Value::List(_)) {
                return Err(Error::new(
                    "TypeError",
                    "list indices must be integers or slices, not str",
                ));
            }
            let key = self.text(name);
            self.put_item(locals, key, value)
        } else if let Some(globals) = self.scope_data(scope).exec_globals {
            let key = self.text(name);
            let storage = self
                .dict_storage(globals.dict)
                .expect("exec globals dictionary");
            self.put_item(storage, key, value)
        } else {
            self.bind(scope, name, value);
            Ok(())
        }
    }

    pub(super) fn load_exec_name(&mut self, locals: Id, name: &str) -> Result<Id> {
        let key = self.text(name);
        if let Some(get) = self.bound_slot(locals, "__getitem__")? {
            return self.call(get, vec![key], Vec::new());
        }
        let indices = match self.heap.get(locals) {
            Value::List(_) => "list indices must be integers or slices, not str",
            Value::Tuple(_) | Value::NamedTuple { .. } => {
                "tuple indices must be integers or slices, not str"
            }
            Value::Str(_) => "string indices must be integers, not 'str'",
            Value::Bytes(_) => "byte indices must be integers or slices, not str",
            Value::Range { .. } => "range indices must be integers or slices, not str",
            _ => return self.get_item(locals, key),
        };
        Err(Error::new("TypeError", indices))
    }

    fn source_step(
        &mut self,
        code: &Program,
        owner: Id,
        scope: Id,
        op: &Instruction,
        ip: &mut usize,
        iterators: &mut Vec<Id>,
    ) -> Result<Option<Step>> {
        let eval = |ctx: &mut Self, e: &ast::Expr| ctx.source_expr(code, scope, e);
        match op {
            Instruction::Eval(e) => {
                eval(self, e)?;
            }
            Instruction::Raise(s) => {
                let (op, exception) = match &s.exc {
                    Some(exc) => ("raise", eval(self, exc)?),
                    None => ("reraise", NONE),
                };
                let (cause, explicit) = match &s.cause {
                    Some(cause) => (eval(self, cause)?, TRUE),
                    None => (NONE, FALSE),
                };
                self.op(op, exception, cause, explicit)?;
            }
            Instruction::Assign(n, e) => {
                let value = eval(self, e)?;
                self.bind_source_name(scope, n, value)?;
            }
            Instruction::Unless(e, destination) => {
                let value = eval(self, e)?;
                if !self.truth(value)? {
                    *ip = *destination;
                }
            }
            Instruction::Jump(destination) => *ip = *destination,
            Instruction::Iter(e) => {
                let v = eval(self, e)?;
                iterators.push(self.iterator(v)?);
            }
            Instruction::Next(target, end) => {
                let iter = *iterators.last().expect("active source loop");
                match self.next_item(iter)? {
                    Some(v) => {
                        if let Some(target) = target {
                            self.bind_loop_target(scope, target, v)?;
                        } else {
                            return Ok(Some(Step::Yield(v, *ip - 1)));
                        }
                    }
                    None => {
                        iterators.pop();
                        *ip = *end;
                    }
                }
            }
            Instruction::Yield(e) | Instruction::Return(e) => {
                let v = e
                    .as_ref()
                    .map(|e| eval(self, e))
                    .transpose()?
                    .unwrap_or(NONE);
                return Ok(Some(if matches!(op, Instruction::Yield(_)) {
                    Step::Yield(v, *ip)
                } else {
                    Step::Return(v)
                }));
            }
            Instruction::From(module, names) => {
                let mut root = scope;
                while let Some(parent) = self.scope_data(root).parent {
                    root = parent;
                }
                let globals = self.module_globals(root);
                let builtins = match self.scope_data(root).exec_globals {
                    Some(supplied) => supplied.builtins,
                    None => self.default_source_builtins()?,
                };
                let importer = self
                    .load_exec_name(builtins, "__import__")
                    .map_err(|error| {
                        if self.is_builtin_error(&error, "KeyError") {
                            Error::new("ImportError", "__import__ not found")
                        } else {
                            error
                        }
                    })?;
                let modname = self.text(module);
                let fromlist = names.iter().map(|(name, _)| self.text(name)).collect();
                let fromlist = self.alloc(Value::Tuple(fromlist));
                let zero = self.int(0);
                let locals = if self.code_object(owner).parameters == NONE {
                    self.scope_data(scope).exec_locals.unwrap_or(globals)
                } else {
                    NONE
                };
                let m = self.call(
                    importer,
                    vec![modname, globals, locals, fromlist, zero],
                    Vec::new(),
                )?;
                for (name, local) in names {
                    let name = self.text(name);
                    let v = self.op("from", m, name, NONE)?;
                    self.bind_source_name(scope, local, v)?;
                }
            }
            Instruction::Define(child, p) => {
                let defaults = self.dict(vec![]);
                let mut sig = serde_json::json!({"posonly":[],"positional":[],"kwonly":[],"varargs":p.vararg.as_ref().map(|x|x.name.as_str()),"kwargs":p.kwarg.as_ref().map(|x|x.name.as_str())});
                for (group, args) in [
                    ("posonly", &p.posonlyargs),
                    ("positional", &p.args),
                    ("kwonly", &p.kwonlyargs),
                ] {
                    for arg in args {
                        let n = arg.parameter.name.as_str();
                        sig[group].as_array_mut().unwrap().push(n.into());
                        if let Some(default) = &arg.default {
                            let v = eval(self, default)?;
                            let k = self.text(n);
                            self.put_item(defaults, k, v)?;
                        }
                    }
                }
                let site = *ip - 1;
                let code = if let Some((_, existing)) = self
                    .code_object(owner)
                    .nested
                    .iter()
                    .find(|(i, _)| *i == site)
                {
                    *existing
                } else {
                    let parameters = self.text(sig.to_string());
                    let id = self.box_source_code(child.clone(), Some(parameters));
                    let Value::Code(parent) = self.heap.get_mut(owner) else {
                        unreachable!()
                    };
                    parent.nested.push((site, id));
                    id
                };
                let frame = self.scope_data(scope);
                let defining_scope = if frame.exec_locals.is_some() {
                    frame.parent.expect("exec locals have a global frame")
                } else {
                    scope
                };
                let f = self.source_function(code, defining_scope, defaults);
                self.bind_source_name(scope, &child.name, f)?;
            }
        }
        Ok(None)
    }

    fn bind_loop_target(&mut self, scope: Id, target: &ast::Expr, value: Id) -> Result<()> {
        match target {
            ast::Expr::Name(n) => self.bind_source_name(scope, n.id.as_str(), value)?,
            ast::Expr::Tuple(t) => {
                let count = t.elts.len();
                let iter = self.iterator(value)?;
                let mut unpacked = Vec::with_capacity(count);
                for i in 0..count {
                    unpacked.push(self.next_item(iter)?.ok_or_else(|| {
                        Error::new(
                            "ValueError",
                            format!("not enough values to unpack (expected {count}, got {i})"),
                        )
                    })?);
                }
                if self.next_item(iter)?.is_some() {
                    let size = match self.heap.get(value) {
                        Value::List(items) | Value::Tuple(items) => Some(items.len()),
                        Value::Dict(items) => Some(items.len()),
                        _ => None,
                    };
                    let detail = size.map_or_else(String::new, |n| format!(", got {n}"));
                    return Err(Error::new(
                        "ValueError",
                        format!("too many values to unpack (expected {count}{detail})"),
                    ));
                }
                for (element, item) in t.elts.iter().zip(unpacked) {
                    self.bind_loop_target(scope, element, item)?;
                }
            }
            _ => unreachable!("validated loop target"),
        }
        Ok(())
    }

    fn source_expr(&mut self, code: &Program, scope: Id, expr: &ast::Expr) -> Result<Id> {
        let value = self.source_expr_inner(code, scope, expr)?;
        self.heap.keep(value);
        Ok(value)
    }

    fn source_expr_inner(&mut self, code: &Program, scope: Id, expr: &ast::Expr) -> Result<Id> {
        use ast::Expr;
        let eval = |ctx: &mut Self, e: &Expr| ctx.source_expr(code, scope, e);
        match expr {
            Expr::Name(n) => {
                let name = n.id.as_str();
                if code.name != "<module>"
                    && code.locals.contains(name)
                    && !self.scope_data(scope).names.contains_key(name)
                {
                    return Err(Error::new(
                        "UnboundLocalError",
                        format!(
                            "cannot access local variable '{name}' where it is not associated with a value"
                        ),
                    ));
                }
                self.lookup(scope, name)
            }
            Expr::StringLiteral(v) => Ok(self.text(v.value.to_string())),
            Expr::NoneLiteral(_) => Ok(NONE),
            Expr::BooleanLiteral(v) => Ok(if v.value { TRUE } else { FALSE }),
            Expr::NumberLiteral(v) => {
                let ast::Number::Int(n) = &v.value else {
                    return Err(unsupported("noninteger literal"));
                };
                let n = crate::parse_integer_literal(&n.to_string())
                    .ok_or_else(|| Error::new("ValueError", "invalid integer"))?;
                Ok(self.alloc(Value::Int(crate::Int::new(n))))
            }
            Expr::List(v) => {
                let items = v
                    .elts
                    .iter()
                    .map(|x| eval(self, x))
                    .collect::<Result<_>>()?;
                Ok(self.alloc(Value::List(items)))
            }
            Expr::Tuple(v) => {
                let items = v
                    .elts
                    .iter()
                    .map(|x| eval(self, x))
                    .collect::<Result<_>>()?;
                Ok(self.alloc(Value::Tuple(items)))
            }
            Expr::Dict(v) => {
                let result = self.dict(vec![]);
                for item in &v.items {
                    let Some(key) = &item.key else {
                        return Err(unsupported("dict unpack"));
                    };
                    let k = eval(self, key)?;
                    let val = eval(self, &item.value)?;
                    self.put_item(result, k, val)?;
                }
                Ok(result)
            }
            Expr::Attribute(v) => {
                let base = eval(self, &v.value)?;
                self.lookup_attr(base, v.attr.as_str())
            }
            Expr::Slice(v) => {
                let mut parts = [NONE; 3];
                for (part, expr) in parts.iter_mut().zip([&v.lower, &v.upper, &v.step]) {
                    if let Some(expr) = expr {
                        *part = eval(self, expr)?;
                    }
                }
                Ok(self.alloc(Value::Slice(parts)))
            }
            Expr::Subscript(v) => {
                let base = eval(self, &v.value)?;
                let k = eval(self, &v.slice)?;
                self.get_item(base, k)
            }
            Expr::BinOp(v) => {
                let a = eval(self, &v.left)?;
                let b = eval(self, &v.right)?;
                self.op(v.op.as_str(), a, b, NONE)
            }
            Expr::Compare(v) => {
                let mut left = eval(self, &v.operands[0])?;
                for (op, right) in v.ops.iter().zip(&v.operands[1..]) {
                    let right = eval(self, right)?;
                    let result = self.op(op.as_str(), left, right, NONE)?;
                    if !self.truth(result)? {
                        return Ok(result);
                    }
                    left = right;
                }
                Ok(TRUE)
            }
            Expr::If(v) => {
                let test = eval(self, &v.test)?;
                let branch = if self.truth(test)? {
                    &v.body
                } else {
                    &v.orelse
                };
                eval(self, branch)
            }
            Expr::Call(v) => {
                let f = eval(self, &v.func)?;
                let args = v
                    .arguments
                    .args
                    .iter()
                    .map(|a| eval(self, a))
                    .collect::<Result<Vec<_>>>()?;
                let mut kwargs = Vec::new();
                for item in &v.arguments.keywords {
                    let key = item
                        .arg
                        .as_ref()
                        .ok_or_else(|| unsupported("keyword unpack"))?;
                    kwargs.push((key.to_string(), eval(self, &item.value)?));
                }
                self.call(f, args, kwargs)
            }
            _ => Err(unsupported(format!(
                "{:?}",
                ast::AnyNodeRef::from(expr).kind()
            ))),
        }
    }
}
