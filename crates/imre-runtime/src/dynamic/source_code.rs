//! In-process execution of source received by the boxed Python runtime.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use ruff_python_ast as ast;
use ruff_text_size::Ranged;

use super::value::{Body, ExecGlobals, Location};
use super::*;

pub(super) struct Program {
    pub filename: Rc<str>,
    pub name: String,
    doc: Option<ruff_text_size::TextRange>,
    literals: HashMap<ruff_text_size::TextRange, Id>,
    pub(super) string_roots: Id,
    first_line: u32,
    generator: bool,
    locals: HashSet<String>,
    method_imports: Rc<std::collections::BTreeSet<String>>,
    private_names: Option<Rc<imre_ast::Names>>,
    instructions: Vec<(u32, Instruction)>,
}

enum Instruction {
    Eval(ast::Expr),
    AugAssign(ast::StmtAugAssign),
    DeleteItem(ast::ExprSubscript),
    Assign(Vec<ast::Expr>, ast::Expr),
    Import {
        module: String,
        alias: Option<String>,
    },
    From(String, Vec<(String, String)>),
    Define(Rc<Program>, Box<ast::Parameters>),
    Yield(Option<ast::Expr>),
    Return(Option<ast::Expr>),
    Raise(ast::StmtRaise),
    Assert(ast::StmtAssert),
    Unless(ast::Expr, usize),
    Iter(ast::Expr),
    Next(Option<ast::Expr>, usize),
    Jump(usize),
    PopIter,
}

/// Pending exits of the innermost loop body being lowered.
struct LoopJumps {
    head: usize,
    has_iterator: bool,
    breaks: Vec<usize>,
}

/// Execution state shared by ordinary and resumed runtime-source instructions.
struct SourceCursor<'a> {
    ip: usize,
    iterators: &'a mut Vec<Id>,
    advanced: Option<Result<Option<Id>>>,
}

enum Step {
    Yield(Id, usize),
    Return(Id),
    Advance(Id, usize),
}

fn unsupported(what: impl std::fmt::Display) -> Error {
    Error::new("NotImplementedError", format!("runtime source: {what}"))
}

/// Collect names and string constants without recursing through an expression's left spine.
struct CompiledStrings<'a, 'ctx> {
    heap: &'ctx mut value::Heap,
    shared: &'ctx mut HashMap<&'a str, Id>,
    constants: &'ctx mut HashMap<ruff_text_size::TextRange, Id>,
    roots: Vec<Id>,
    pending: Vec<&'a ast::Expr>,
    exhausted: bool,
}

impl<'a> ast::visitor::Visitor<'a> for CompiledStrings<'a, '_> {
    fn visit_expr(&mut self, expr: &'a ast::Expr) {
        if self.pending.try_reserve(1).is_err() {
            self.exhausted = true;
        } else {
            self.pending.push(expr);
        }
    }

    fn visit_keyword(&mut self, keyword: &'a ast::Keyword) {
        if let Some(name) = &keyword.arg {
            self.heap.code_name(name.as_str());
        }
        self.visit_expr(&keyword.value);
    }

    fn visit_parameter(&mut self, parameter: &'a ast::Parameter) {
        self.heap.code_name(parameter.name.as_str());
        if let Some(annotation) = &parameter.annotation {
            self.visit_expr(annotation);
        }
    }
}

impl<'a> CompiledStrings<'a, '_> {
    fn keep(&mut self, id: Id) -> Result<()> {
        self.roots
            .try_reserve(1)
            .map_err(|_| Error::new("MemoryError", ""))?;
        self.roots.push(id);
        Ok(())
    }

    /// Share equal text constants across nested code objects.
    fn string_constant(&mut self, literal: &'a ast::ExprStringLiteral) -> Result<()> {
        let text = literal.value.to_str();
        let id = if let Some(&id) = self.shared.get(text) {
            id
        } else {
            self.shared
                .try_reserve(1)
                .map_err(|_| Error::new("MemoryError", ""))?;
            let id = self
                .heap
                .source_string_constant(text)
                .ok_or_else(|| Error::new("MemoryError", ""))?;
            self.shared.insert(text, id);
            id
        };
        self.constant(literal.range(), id)
    }

    /// Root a literal for later evaluation at its source position.
    fn constant(&mut self, range: ruff_text_size::TextRange, id: Id) -> Result<()> {
        self.constants
            .try_reserve(1)
            .map_err(|_| Error::new("MemoryError", ""))?;
        self.constants.insert(range, id);
        self.keep(id)
    }

    fn finish(mut self) -> Result<Id> {
        while let Some(expr) = self.pending.pop() {
            match expr {
                ast::Expr::Name(name) => {
                    self.heap.code_name(name.id.as_str());
                }
                ast::Expr::Attribute(attr) => {
                    self.heap.code_name(attr.attr.as_str());
                }
                ast::Expr::StringLiteral(literal) => self.string_constant(literal)?,
                ast::Expr::BytesLiteral(literal) => {
                    let id = self
                        .heap
                        .alloc(Value::Bytes(literal.value.bytes().collect()));
                    self.constant(literal.range(), id)?;
                }
                _ => {}
            }
            ast::visitor::walk_expr(&mut self, expr);
        }
        if self.exhausted {
            return Err(Error::new("MemoryError", ""));
        }
        Ok(if self.roots.is_empty() {
            NONE
        } else {
            self.heap.alloc(Value::Tuple(self.roots))
        })
    }
}

/// Collect source line offsets before constructing code or AST nodes.
pub(super) fn line_starts(offsets: impl Iterator<Item = usize> + Clone) -> Result<Vec<usize>> {
    let mut starts = Vec::new();
    starts
        .try_reserve_exact(1 + offsets.clone().count())
        .map_err(|_| crate::Error::allocation())?;
    starts.push(0);
    starts.extend(offsets);
    Ok(starts)
}

impl Program {
    /// Keep each code object's constants alive, including constants owned by unexecuted children.
    fn prepare_strings<'a>(
        &'a mut self,
        heap: &mut value::Heap,
        shared: &mut HashMap<&'a str, Id>,
    ) -> Result<Id> {
        use ast::visitor::Visitor;
        let mut strings = CompiledStrings {
            heap,
            shared,
            constants: &mut self.literals,
            roots: Vec::new(),
            pending: Vec::new(),
            exhausted: false,
        };
        for (_, op) in &mut self.instructions {
            match op {
                Instruction::Eval(e) | Instruction::Unless(e, _) | Instruction::Iter(e) => {
                    strings.visit_expr(e)
                }
                Instruction::DeleteItem(item) => {
                    strings.visit_expr(&item.value);
                    strings.visit_expr(&item.slice);
                }
                Instruction::Assert(stmt) => {
                    strings.visit_expr(&stmt.test);
                    if let Some(message) = &stmt.msg {
                        strings.visit_expr(message);
                    }
                }
                Instruction::AugAssign(stmt) => {
                    strings.visit_expr(&stmt.target);
                    strings.visit_expr(&stmt.value);
                }
                Instruction::Assign(targets, e) => {
                    for target in targets {
                        strings.visit_expr(target);
                    }
                    strings.visit_expr(e);
                }
                Instruction::Import { module, alias } => {
                    for part in module.split('.') {
                        strings.heap.code_name(part);
                    }
                    if let Some(alias) = alias {
                        strings.heap.code_name(alias);
                    }
                }
                Instruction::From(module, bindings) => {
                    strings.heap.code_name(module);
                    for (name, alias) in bindings {
                        strings.heap.code_name(name);
                        strings.heap.code_name(alias);
                    }
                }
                Instruction::Define(child, parameters) => {
                    let child = Rc::get_mut(child).expect("newly lowered child code");
                    strings.heap.code_name(&child.name);
                    let roots = child.prepare_strings(strings.heap, strings.shared)?;
                    if roots != NONE {
                        strings.keep(roots)?;
                    }
                    strings.visit_parameters(parameters);
                }
                Instruction::Yield(e) | Instruction::Return(e) | Instruction::Next(e, _) => {
                    if let Some(e) = e {
                        strings.visit_expr(e);
                    }
                }
                Instruction::Raise(statement) => {
                    if let Some(e) = &statement.exc {
                        strings.visit_expr(e);
                    }
                    if let Some(e) = &statement.cause {
                        strings.visit_expr(e);
                    }
                }
                Instruction::Jump(_) | Instruction::PopIter => {}
            }
        }
        self.string_roots = strings.finish()?;
        Ok(self.string_roots)
    }

    fn new(filename: Rc<str>, name: &str) -> Self {
        Self {
            filename,
            name: name.to_owned(),
            doc: None,
            literals: HashMap::new(),
            string_roots: NONE,
            first_line: 1,
            generator: false,
            locals: HashSet::new(),
            method_imports: Rc::default(),
            private_names: None,
            instructions: Vec::new(),
        }
    }

    fn lower(
        &mut self,
        body: ast::Suite,
        lines: &[usize],
        function: bool,
        active: Option<&mut LoopJumps>,
    ) -> Result<()> {
        stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || {
            self.lower_suite(body, lines, function, active)
        })
    }

    fn lower_suite(
        &mut self,
        body: ast::Suite,
        lines: &[usize],
        function: bool,
        mut active: Option<&mut LoopJumps>,
    ) -> Result<()> {
        use ast::Stmt;
        if function
            && let Some(Stmt::Expr(statement)) = body.first()
            && let ast::Expr::StringLiteral(value) = &*statement.value
        {
            self.doc = Some(value.range());
        }
        for stmt in body {
            let line = lines.partition_point(|n| *n <= stmt.start().to_usize()) as u32;
            let op = match stmt {
                Stmt::Pass(_) => continue,
                Stmt::Expr(s) => match *s.value {
                    ast::Expr::Yield(y) => {
                        if !function {
                            return Err(Error::new("SyntaxError", "'yield' outside function"));
                        }
                        self.generator = true;
                        Instruction::Yield(y.value.map(|value| *value))
                    }
                    ast::Expr::YieldFrom(y) if function => {
                        self.generator = true;
                        self.instructions.push((line, Instruction::Iter(*y.value)));
                        Instruction::Next(None, self.instructions.len() + 1)
                    }
                    e => Instruction::Eval(e),
                },
                Stmt::Assign(s) => {
                    for target in &s.targets {
                        self.declare_target(target)?;
                    }
                    Instruction::Assign(s.targets, *s.value)
                }
                Stmt::AnnAssign(s) if function => {
                    let ast::Expr::Name(name) = &*s.target else {
                        return Err(unsupported("annotated assignment target"));
                    };
                    self.locals.insert(name.id.to_string());
                    // Local annotations establish the binding without evaluating the annotation.
                    let Some(value) = s.value else { continue };
                    Instruction::Assign(vec![*s.target], *value)
                }
                Stmt::Delete(s) => {
                    for target in s.targets {
                        match target {
                            ast::Expr::Subscript(item)
                                if !matches!(*item.slice, ast::Expr::Slice(_)) =>
                            {
                                self.instructions
                                    .push((line, Instruction::DeleteItem(item)));
                            }
                            _ => return Err(unsupported("deletion target")),
                        }
                    }
                    continue;
                }
                Stmt::AugAssign(s) => {
                    self.declare_target(&s.target)?;
                    Instruction::AugAssign(s)
                }
                Stmt::Return(s) => {
                    if !function {
                        return Err(Error::new("SyntaxError", "'return' outside function"));
                    }
                    Instruction::Return(s.value.map(|value| *value))
                }
                Stmt::Raise(s) => Instruction::Raise(s),
                Stmt::Import(s) if !s.is_lazy => {
                    for name in &s.names {
                        let module = name.name.to_string();
                        let alias = name.asname.as_ref().map(ToString::to_string);
                        let local = alias
                            .as_deref()
                            .unwrap_or_else(|| module.split('.').next().unwrap());
                        self.locals.insert(local.to_owned());
                        self.instructions
                            .push((line, Instruction::Import { module, alias }));
                    }
                    continue;
                }
                Stmt::Assert(s) => Instruction::Assert(s),
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
                    child.method_imports = self.method_imports.clone();
                    child.private_names = self.private_names.clone();
                    child.first_line = line;
                    child.lower(s.body, lines, true, None)?;
                    self.locals.insert(child.name.clone());
                    Instruction::Define(Rc::new(child), s.parameters)
                }
                Stmt::Break(_) => {
                    let current = active
                        .as_deref_mut()
                        .ok_or_else(|| Error::new("SyntaxError", "'break' outside loop"))?;
                    if current.has_iterator {
                        self.instructions.push((line, Instruction::PopIter));
                    }
                    current.breaks.push(self.instructions.len());
                    Instruction::Jump(0)
                }
                Stmt::Continue(_) => {
                    let current = active.as_deref().ok_or_else(|| {
                        Error::new("SyntaxError", "'continue' not properly in loop")
                    })?;
                    Instruction::Jump(current.head)
                }
                Stmt::For(_) | Stmt::While(_) => {
                    let (body, orelse, guard) = match stmt {
                        Stmt::For(s) if !s.is_async => {
                            self.declare_target(&s.target)?;
                            self.instructions.push((line, Instruction::Iter(*s.iter)));
                            (s.body, s.orelse, Instruction::Next(Some(*s.target), 0))
                        }
                        Stmt::While(s) => (s.body, s.orelse, Instruction::Unless(*s.test, 0)),
                        _ => return Err(unsupported("async for")),
                    };
                    let head = self.instructions.len();
                    let mut current = LoopJumps {
                        head,
                        has_iterator: matches!(guard, Instruction::Next(..)),
                        breaks: Vec::new(),
                    };
                    self.instructions.push((line, guard));
                    self.lower(body, lines, function, Some(&mut current))?;
                    self.instructions.push((line, Instruction::Jump(head)));
                    let other = self.instructions.len();
                    match &mut self.instructions[head].1 {
                        Instruction::Next(_, end) | Instruction::Unless(_, end) => *end = other,
                        _ => unreachable!("loop guard"),
                    }
                    self.lower(orelse, lines, function, active.as_deref_mut())?;
                    let end = self.instructions.len();
                    for jump in current.breaks {
                        self.instructions[jump].1 = Instruction::Jump(end);
                    }
                    continue;
                }
                Stmt::If(s) => {
                    let mut ends = Vec::new();
                    let mut arm = Some((*s.test, s.body));
                    for clause in s.elif_else_clauses {
                        if let Some((test, body)) = arm.take() {
                            ends.push(self.arm(
                                test,
                                body,
                                line,
                                lines,
                                function,
                                active.as_deref_mut(),
                            )?);
                        }
                        if let Some(test) = clause.test {
                            arm = Some((test, clause.body));
                        } else {
                            self.lower(clause.body, lines, function, active.as_deref_mut())?;
                        }
                    }
                    if let Some((test, body)) = arm {
                        ends.push(self.arm(
                            test,
                            body,
                            line,
                            lines,
                            function,
                            active.as_deref_mut(),
                        )?);
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
                        ast::AnyNodeRef::from(&stmt).kind()
                    )));
                }
            };
            self.instructions.push((line, op));
        }
        Ok(())
    }

    /// Declare every local name in a loop or unpacking target.
    fn declare_target(&mut self, target: &ast::Expr) -> Result<()> {
        match target {
            ast::Expr::Name(n) => {
                self.locals.insert(n.id.to_string());
            }
            ast::Expr::Attribute(_) => {}
            ast::Expr::Subscript(item) if !matches!(*item.slice, ast::Expr::Slice(_)) => {}
            ast::Expr::Starred(_) => {
                return Err(Error::new(
                    "SyntaxError",
                    "starred assignment target must be in a list or tuple",
                ));
            }
            ast::Expr::Tuple(ast::ExprTuple { elts, .. })
            | ast::Expr::List(ast::ExprList { elts, .. }) => {
                if elts
                    .iter()
                    .filter(|item| matches!(item, ast::Expr::Starred(_)))
                    .count()
                    > 1
                {
                    return Err(Error::new(
                        "SyntaxError",
                        "multiple starred expressions in assignment",
                    ));
                }
                for element in elts {
                    self.declare_target(match element {
                        ast::Expr::Starred(star) => &star.value,
                        _ => element,
                    })?;
                }
            }
            _ => return Err(unsupported("unpacking target")),
        }
        Ok(())
    }

    fn arm(
        &mut self,
        test: ast::Expr,
        body: ast::Suite,
        line: u32,
        lines: &[usize],
        function: bool,
        active: Option<&mut LoopJumps>,
    ) -> Result<usize> {
        let index = self.instructions.len();
        self.instructions.push((line, Instruction::Jump(0)));
        self.lower(body, lines, function, active)?;
        let end = self.instructions.len();
        self.instructions.push((line, Instruction::Jump(0)));
        self.instructions[index].1 = Instruction::Unless(test, self.instructions.len());
        Ok(end)
    }
}

pub(super) fn syntax_member(name: &str) -> Option<usize> {
    [
        "msg",
        "filename",
        "lineno",
        "offset",
        "text",
        "end_lineno",
        "end_offset",
        "print_file_and_line",
        "_metadata",
    ]
    .iter()
    .position(|field| *field == name)
}

impl Context {
    pub(super) fn syntax_fields(&self, args: &[Id]) -> [Id; 9] {
        let mut fields = [NONE; 9];
        fields[0] = args.first().copied().unwrap_or(NONE);
        if let [_, location] = args
            && let Value::Tuple(values) = self.heap.get(*location)
        {
            for (field, value) in fields[1..7].iter_mut().zip(values) {
                *field = *value;
            }
        }
        fields
    }

    pub(super) fn retain_syntax_fields(&self, error: &mut Error, fields: [Id; 9]) -> Result<()> {
        let fields = crate::shared::Shared::try_new(fields.map(std::cell::Cell::new))
            .map_err(|_| Error::new("MemoryError", ""))?;
        if error.trace.is_none() {
            let trace = allocator_api2::boxed::Box::try_new(super::value::Trace::default())
                .map_err(|_| Error::new("MemoryError", ""))?;
            error.trace = Some(trace);
        }
        error.tracing().source_syntax = Some(fields);
        Ok(())
    }

    pub(super) fn update_syntax_member(&self, object: Id, name: &str, value: Id) -> bool {
        let Value::Raised(error) = self.heap.get(object) else {
            return false;
        };
        let Some(fields) = error
            .trace
            .as_ref()
            .and_then(|trace| trace.source_syntax.as_ref())
        else {
            return false;
        };
        let Some(slot) = syntax_member(name) else {
            return false;
        };
        fields[slot].set(value);
        true
    }

    pub(super) fn syntax_suffix(&self, filename: Id, line: Id) -> Result<String> {
        use std::fmt::Write;
        let filename = self.string_storage(filename).map(|id| {
            let Value::Str(path) = self.heap.get(id) else {
                unreachable!()
            };
            path.rsplit(std::path::MAIN_SEPARATOR)
                .next()
                .unwrap_or(path)
        });
        let line = if let Value::Int(number) = self.heap.get(line) {
            Some(if std::mem::size_of::<libc::c_long>() == 4 {
                number
                    .to_i64()
                    .and_then(|n| i32::try_from(n).ok())
                    .map(i64::from)
                    .unwrap_or(-1)
            } else {
                number.to_i64().unwrap_or(-1)
            })
        } else {
            None
        };
        let mut suffix = String::new();
        if filename.is_some() || line.is_some() {
            suffix
                .try_reserve(filename.map_or(0, str::len).saturating_add(40))
                .map_err(|_| Error::new("MemoryError", ""))?;
            suffix.push_str(" (");
            if let Some(name) = filename {
                suffix.push_str(name);
            }
            if let Some(n) = line {
                if filename.is_some() {
                    suffix.push_str(", ");
                }
                write!(&mut suffix, "line {n}").expect("writing to a string succeeds");
            }
            suffix.push(')');
        }
        Ok(suffix)
    }

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
        let filename_text = self.str_value(filename)?;
        let source = match self.heap.get(self.scalar_value(source)) {
            Value::Bytes(data) | Value::ByteArray(data) => {
                match crate::objects::decode_compile_source(data) {
                    Ok(text) => Cow::Owned(text),
                    Err(error) => return Err(self.source_decode_error(error, filename)),
                }
            }
            _ => Cow::Borrowed(self.str_value(source)?),
        };
        let filename = filename_text;
        let parsed = ruff_python_parser::parse_module(&source)
            .map_err(|error| Error::new("SyntaxError", format!("{filename}: {error}")))?;
        let lines = line_starts(source.match_indices('\n').map(|(offset, _)| offset + 1))?;
        let mut code = Program::new(Rc::from(filename), "<module>");
        code.method_imports = Rc::new(imre_ast::module_imports(&parsed.syntax().body));
        code.lower(parsed.into_syntax().body, &lines, false, None)?;
        code.prepare_strings(&mut self.heap, &mut HashMap::new())?;
        Ok(self.box_source_code(Rc::new(code), None))
    }

    /// Attach the filename after tokenization, just as compile does for string input.
    fn source_decode_error(
        &mut self,
        error: crate::objects::SourceDecodeError,
        filename: Id,
    ) -> Error {
        use crate::objects::SourceDecodeError::*;
        let null = matches!(error, Null);
        let (message, range) = match error {
            Allocation => return Error::new("MemoryError", ""),
            Null => (
                "source code string cannot contain null bytes".to_owned(),
                None,
            ),
            Invalid(message, range) => (message, range),
            NonText(message) => (message, None),
            Unsupported(name) => {
                return Error::new(
                    "NotImplementedError",
                    format!("compile: native source decoder for '{name}' is unavailable"),
                );
            }
        };
        let msg = self.text(&message);
        let mut args = vec![msg];
        let mut rendered = message;
        if !null {
            let lineno = range.as_ref().map_or(0, |r| r.line);
            let name = self
                .str_value(filename)
                .expect("compile validated its filename");
            let basename = name
                .rsplit(std::path::MAIN_SEPARATOR)
                .next()
                .unwrap_or(name);
            rendered = format!("{rendered} ({basename}, line {lineno})");
            let line = self.int(lineno as i64);
            let details = if let Some(r) = range {
                vec![
                    if r.tokenizer { NONE } else { filename },
                    line,
                    self.int(r.start as i64),
                    self.text(r.text),
                    line,
                    self.int(r.end as i64),
                ]
            } else {
                vec![filename, line, self.int(-1), NONE]
            };
            args.push(self.alloc(Value::Tuple(details)));
        }
        let mut error = self.exception("SyntaxError", &args);
        error.message = rendered;
        let mut fields = self.syntax_fields(&args);
        // The tokenizer can omit the filename from args while compile exposes it on the exception.
        fields[1] = if null { NONE } else { filename };
        match self.retain_syntax_fields(&mut error, fields) {
            Ok(()) => error,
            Err(allocation) => allocation,
        }
    }

    fn box_source_code(&mut self, program: Rc<Program>, parameters: Option<Id>) -> Id {
        let name = self.heap.code_name(&program.name);
        let filename = self.text(program.filename.as_ref());
        let doc = program
            .doc
            .and_then(|range| program.literals.get(&range).copied())
            .unwrap_or(NONE);
        self.alloc(Value::Code(value::CodeObject {
            name,
            qualname: name,
            doc,
            filename,
            first_line: program.first_line,
            body: Body::Source(program),
            module_definition: true,
            coroutine: false,
            parameters: parameters.unwrap_or(NONE),
            nested: Vec::new(),
            freevars: Vec::new(),
            closure_supported: false,
        }))
    }

    /// Define a function whose parameters and executable belong to the generated code object.
    fn source_function(&mut self, code: Id, scope: Id, defaults: Id) -> Id {
        let name = self.code_object(code).name;
        let doc = self.code_object(code).doc;
        self.define_function(code, scope, name, defaults, name, doc, NONE, false)
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

    /// Construct a function whose globals are supplied by Python code.
    pub(super) fn construct_function(
        &mut self,
        args: &[Id],
        kwargs: &[(String, Id)],
    ) -> Result<Id> {
        let [code, globals, name, defaults, closure, kwdefaults] = builtins::bind_method_arguments(
            "function",
            args,
            kwargs,
            [
                "code",
                "globals",
                "name",
                "argdefs",
                "closure",
                "kwdefaults",
            ],
            6,
        )?;
        let code = code.ok_or_else(|| {
            Error::new(
                "TypeError",
                "function() missing required argument 'code' (pos 1)",
            )
        })?;
        let globals = globals.ok_or_else(|| {
            Error::new(
                "TypeError",
                "function() missing required argument 'globals' (pos 2)",
            )
        })?;
        let Value::Code(data) = self.heap.get(code) else {
            return Err(Error::new(
                "TypeError",
                format!(
                    "function() argument 'code' must be code, not {}",
                    self.type_name(code)
                ),
            ));
        };
        if self.dict_storage(globals).is_none() {
            return Err(Error::new(
                "TypeError",
                format!(
                    "function() argument 'globals' must be dict, not {}",
                    self.type_name(globals)
                ),
            ));
        }
        if !data.module_definition && !data.closure_supported {
            return Err(unsupported(
                "function construction without closure metadata",
            ));
        }
        let freevars = data.freevars.clone();
        let code_name = data.name;
        let code_qualname = data.qualname;
        let code_doc = data.doc;
        let name = name.filter(|&v| v != NONE).unwrap_or(code_name);
        if self.string_storage(name).is_none() {
            return Err(Error::new(
                "TypeError",
                "arg 3 (name) must be None or string",
            ));
        }
        let defaults = defaults.unwrap_or(NONE);
        if defaults != NONE && !matches!(self.heap.get(defaults), Value::Tuple(_)) {
            return Err(Error::new(
                "TypeError",
                "arg 4 (defaults) must be None or tuple",
            ));
        }
        let closure = closure.unwrap_or(NONE);
        let cells = match self.heap.get(closure) {
            Value::Tuple(cells) => cells.as_slice(),
            _ if closure == NONE && freevars.is_empty() => &[],
            _ if closure == NONE => {
                return Err(Error::new("TypeError", "arg 5 (closure) must be tuple"));
            }
            _ => {
                return Err(Error::new(
                    "TypeError",
                    "arg 5 (closure) must be None or tuple",
                ));
            }
        };
        let kwdefaults = kwdefaults.unwrap_or(NONE);
        if kwdefaults != NONE && self.dict_storage(kwdefaults).is_none() {
            return Err(Error::new(
                "TypeError",
                "arg 6 (kwdefaults) must be None or dict",
            ));
        }
        if cells.len() != freevars.len() {
            return Err(Error::new(
                "ValueError",
                format!(
                    "{} requires closure of length {}, not {}",
                    self.str_value(code_name)?,
                    freevars.len(),
                    cells.len()
                ),
            ));
        }
        for &cell in cells {
            if !matches!(self.heap.get(cell), Value::Cell(_)) {
                return Err(Error::new(
                    "TypeError",
                    format!(
                        "arg 5 (closure) expected cell, found {}",
                        self.type_name(cell)
                    ),
                ));
            }
        }
        let cells = cells.to_vec();
        let builtins = if let Some(v) = self.dict_named_item(globals, "__builtins__") {
            if let Value::Module { scope, .. } = *self.heap.get(v) {
                self.module_globals(scope)
            } else {
                v
            }
        } else {
            self.default_source_builtins()?
        };
        let scope = self.scope("", None);
        let Value::Scope(environment) = self.heap.get_mut(scope) else {
            unreachable!()
        };
        environment.namespace = Some(globals);
        environment.exec_globals = Some(ExecGlobals {
            dict: globals,
            builtins,
        });
        let scope = self.closure_environment(scope, freevars, &cells);
        let fields = super::function_fields::FunctionFields::new(
            self,
            scope,
            name,
            code_qualname,
            code_doc,
            NONE,
        );
        Ok(self.alloc(Value::Function {
            fields,
            closure,
            code,
            scope,
            class_cell: None,
            defaults,
            kwdefaults,
        }))
    }

    fn default_source_builtins(&mut self) -> Result<Id> {
        if let Some(dict) = self.source_builtins {
            return Ok(dict);
        }
        let name = self.text("__name__");
        let module = self.text("builtins");
        let ellipsis = self.text("Ellipsis");
        let not_implemented = self.text("NotImplemented");
        let dict = self.dict(vec![
            (name, module),
            (ellipsis, value::ELLIPSIS),
            (not_implemented, value::NOT_IMPLEMENTED),
        ]);
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
        function: Option<Id>,
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
                    | Value::Range(_)
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
            let owner = function.map_or_else(
                || FrameOwner::Module(self.function_module(scope)),
                FrameOwner::Function,
            );
            self.execute(code_id, module, owner)?;
        }
        Ok(NONE)
    }

    pub(super) fn call_source(&mut self, code: Id, scope: Id, owner: FrameOwner) -> Result<Id> {
        let generator = match &self.code_object(code).body {
            Body::Source(program) => program.generator,
            Body::PendingGenerator { .. } => true,
            Body::Native(_) => unreachable!("source function"),
        };
        if generator {
            Ok(self.alloc(Value::Generator {
                owner,
                code,
                scope,
                iterators: Vec::new(),
                offset: Some(0),
                running: false,
            }))
        } else {
            match self.run_source(code, scope, owner, 0, &mut Vec::new(), None)? {
                Step::Return(v) => Ok(v),
                Step::Yield(..) | Step::Advance(..) => unreachable!("classified generator"),
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
        let mut syntax = parsed.into_syntax();
        let [ast::Stmt::FunctionDef(def)] = syntax.body.as_mut_slice() else {
            return Err(Error::new("SystemError", "invalid generator body"));
        };
        #[derive(serde::Deserialize)]
        struct Signature<'a> {
            #[serde(borrow)]
            private_scope: Option<(&'a str, bool)>,
            #[serde(default)]
            method_imports: std::collections::BTreeSet<String>,
        }
        let signature: Signature<'_> = serde_json::from_str(self.str_value(code.parameters)?)
            .map_err(|_| Error::new("SystemError", "invalid generator signature"))?;
        let private_names = signature
            .private_scope
            .map(|(owner, postponed)| Rc::new(imre_ast::lower_in(&mut def.body, owner, postponed)));
        let lines = line_starts(source.match_indices('\n').map(|(offset, _)| offset + 1))?;
        let mut program = Program::new(
            self.str_value(code.filename)?.into(),
            self.str_value(code.name)?,
        );
        program.method_imports = Rc::new(signature.method_imports);
        program.private_names = private_names;
        program.first_line = code.first_line;
        program.generator = true;
        program.lower(std::mem::take(&mut def.body), &lines, true, None)?;
        for (line, _) in &mut program.instructions {
            *line = body_line + line.saturating_sub(2);
        }
        program.prepare_strings(&mut self.heap, &mut HashMap::new())?;
        let Value::Code(code) = self.heap.get_mut(id) else {
            unreachable!()
        };
        code.body = Body::Source(Rc::new(program));
        Ok(())
    }

    pub(super) fn resume_source(&mut self, mut id: Id) -> Result<Option<Id>> {
        let mut parents = Vec::new();
        let mut advanced = None;
        loop {
            let result = match self.resume_source_frame(id, advanced.take()) {
                Ok(Step::Advance(child, _)) => {
                    if parents.try_reserve(1).is_err() {
                        advanced = Some(Err(crate::Error::allocation().into()));
                        continue;
                    }
                    parents.push(id);
                    id = child;
                    continue;
                }
                Ok(Step::Yield(value, _)) => Ok(Some(value)),
                Ok(Step::Return(value)) if value == NONE || !parents.is_empty() => Ok(None),
                Ok(Step::Return(value)) => Err(self.exception("StopIteration", &[value])),
                Err(error) => Err(error),
            };
            let Some(parent) = parents.pop() else {
                return result;
            };
            id = parent;
            advanced = Some(result);
        }
    }

    /// Resume one frame, retaining its running state while a delegated child executes.
    fn resume_source_frame(
        &mut self,
        id: Id,
        advanced: Option<Result<Option<Id>>>,
    ) -> Result<Step> {
        if advanced.is_none() {
            self.heap.reserve_temporaries(1)?;
            self.heap.keep(id);
        }
        let Value::Generator {
            owner,
            code,
            scope,
            iterators,
            running,
            offset,
        } = self.heap.get_mut(id)
        else {
            unreachable!()
        };
        if *running && advanced.is_none() {
            return Err(Error::new("ValueError", "generator already executing"));
        }
        let Some(ip) = offset.take() else {
            return Ok(Step::Return(NONE));
        };
        *running = true;
        let (code, frame_scope, owner) = (*code, *scope, *owner);
        let mut active = std::mem::take(iterators);
        let result = self
            .prepare_generator(code)
            .and_then(|()| self.run_source(code, frame_scope, owner, ip, &mut active, advanced))
            .map_err(|error| self.generator_error(error));
        let Value::Generator {
            running,
            offset,
            scope,
            owner,
            iterators,
            ..
        } = self.heap.get_mut(id)
        else {
            unreachable!()
        };
        *running = matches!(result, Ok(Step::Advance(..)));
        let completed = match result {
            Ok(step @ (Step::Yield(_, ip) | Step::Advance(_, ip))) => {
                *offset = Some(ip);
                *iterators = active;
                return Ok(step);
            }
            Ok(Step::Return(value)) => Ok(value),
            Err(error) => Err(error),
        };
        *scope = NONE;
        *owner = FrameOwner::Module(NONE);
        if self.heap.has_file_writers()
            && (!active.is_empty()
                || !self.scope_data(frame_scope).closure.cells.is_empty()
                || self
                    .scope_data(frame_scope)
                    .names
                    .values()
                    .any(|&value| self.heap.could_own_writer(value, true)))
            && self.heap.has_unbound_writer(NONE)
        {
            self.heap.release_scope_temporary(frame_scope);
            match &completed {
                Ok(value) => self.heap.keep(*value),
                Err(error) => {
                    for root in error.roots() {
                        self.heap.keep(root);
                    }
                }
            }
            self.collect_unreachable();
        }
        completed.map(Step::Return)
    }

    fn run_source(
        &mut self,
        owner: Id,
        scope: Id,
        frame_owner: FrameOwner,
        ip: usize,
        iterators: &mut Vec<Id>,
        advanced: Option<Result<Option<Id>>>,
    ) -> Result<Step> {
        let Body::Source(code) = self.code_object(owner).body.clone() else {
            unreachable!("source frame")
        };
        if advanced.is_none() {
            self.heap.reserve_temporaries(2)?;
            self.heap.keep(owner);
            self.heap.enter(scope, owner, frame_owner);
        }
        let mut cursor = SourceCursor {
            ip,
            iterators,
            advanced,
        };
        let result = (|| {
            while let Some((line, op)) = code.instructions.get(cursor.ip) {
                let loc = Location {
                    file: code.filename.to_string().into(),
                    line: *line,
                    span: None,
                };
                // The frame owns its code and scope. Active iterators must also survive nested calls.
                if cursor.advanced.is_none() {
                    self.checkpoint(cursor.iterators, loc, NONE);
                }
                cursor.ip += 1;
                match self.source_step(&code, owner, scope, op, &mut cursor) {
                    Ok(Some(step)) => return Ok(step),
                    Ok(None) => (),
                    Err(mut e) => {
                        let reraising = matches!(op, Instruction::Raise(s) if s.exc.is_none())
                            && self.heap.active_exception() != NONE;
                        if !reraising {
                            self.record_exception_frame(&mut e, None);
                        }
                        return Err(e);
                    }
                }
            }
            Ok(Step::Return(NONE))
        })();
        let (value, returning) = match &result {
            Ok(Step::Return(v)) => (*v, true),
            Ok(Step::Yield(v, _)) => (*v, false),
            // The parent frame remains visible until the child returns its next item.
            Ok(Step::Advance(..)) => return result,
            Err(_) => (NONE, false),
        };
        self.leave_frame(value, returning);
        result
    }

    /// Release the old binding before the next generator target or instruction can observe it.
    fn bind_source_name(&mut self, scope: Id, name: &str, value: Id) -> Result<()> {
        if self.store_source_name(scope, name, value)? && self.heap.has_unbound_writer(NONE) {
            self.collect_unreachable();
            self.heap.release_unbound_writers();
        }
        Ok(())
    }

    /// Publish a binding and report whether its previous value could own a buffered writer.
    pub(super) fn store_source_name(&mut self, scope: Id, name: &str, value: Id) -> Result<bool> {
        let replaced = self.replacing_writer(scope, name, value);
        let scope = self.binding_scope(scope, name);
        if let Some(locals) = self.scope_data(scope).exec_locals {
            if matches!(self.heap.get(locals), Value::List(_)) {
                return Err(Error::new(
                    "TypeError",
                    "list indices must be integers or slices, not str",
                ));
            }
            let key = self.heap.code_name(name);
            self.put_item(locals, key, value)?;
        } else if let Some(globals) = self.scope_data(scope).exec_globals {
            let key = self.heap.code_name(name);
            let storage = self
                .dict_storage(globals.dict)
                .expect("exec globals dictionary");
            self.put_item(storage, key, value)?;
        } else {
            self.bind(scope, name, value);
        }
        Ok(replaced)
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
            Value::Range(_) => "range indices must be integers or slices, not str",
            _ => return self.get_item(locals, key),
        };
        Err(Error::new("TypeError", indices))
    }

    /// Call the current scope's import hook with the globals and locals of the source frame.
    fn import_source_module(
        &mut self,
        owner: Id,
        scope: Id,
        module: &str,
        names: Option<&[(String, String)]>,
    ) -> Result<Id> {
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
        let fromlist = if let Some(names) = names {
            let names = names.iter().map(|(name, _)| self.text(name)).collect();
            self.alloc(Value::Tuple(names))
        } else {
            NONE
        };
        let zero = self.int(0);
        let locals = if self.code_object(owner).parameters == NONE {
            self.scope_data(scope).exec_locals.unwrap_or(globals)
        } else {
            NONE
        };
        self.call(
            importer,
            vec![modname, globals, locals, fromlist, zero],
            Vec::new(),
        )
    }

    /// Keep the iterator's backing sequence available when the final pull drops it.
    fn source_loop_owner(&mut self, iterator: Id) -> Id {
        if !self.heap.has_file_writers() {
            return iterator;
        }
        match self.heap.get(iterator) {
            Value::RangeIterator(_) => NONE,
            Value::Iterator {
                context: None,
                sequence,
                ..
            } => {
                let owner = sequence.unwrap_or(NONE);
                self.heap.keep(owner);
                owner
            }
            _ => iterator,
        }
    }

    /// Schedule a flush only when the discarded owner could contain a writer.
    fn release_source_loop_owner(&mut self, owner: Id) {
        if self.heap.has_file_writers() && self.heap.could_own_writer(owner, true) {
            self.heap.release_unbound_writers();
        }
    }

    /// Recheck buffered writers when a break discards the loop's hidden iterator.
    pub(super) fn release_loop_iterator(&mut self, iterator: Id) {
        let owner = self.source_loop_owner(iterator);
        self.release_source_loop_owner(owner);
    }

    fn source_step(
        &mut self,
        code: &Program,
        owner: Id,
        scope: Id,
        op: &Instruction,
        cursor: &mut SourceCursor<'_>,
    ) -> Result<Option<Step>> {
        let SourceCursor {
            ip,
            iterators,
            advanced,
        } = cursor;
        let eval = |ctx: &mut Self, e: &ast::Expr| ctx.source_expr(code, scope, e);
        match op {
            Instruction::Eval(e) => {
                eval(self, e)?;
            }
            Instruction::Assert(stmt) => {
                if !self.source_condition(code, scope, &stmt.test)? {
                    let class = self.builtin("AssertionError");
                    let args = match &stmt.msg {
                        Some(message) => vec![eval(self, message)?],
                        None => Vec::new(),
                    };
                    let error = self.call(class, args, Vec::new())?;
                    self.op("raise", error, NONE, FALSE)?;
                }
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
            Instruction::Assign(targets, e) => {
                let value = eval(self, e)?;
                for target in targets {
                    self.bind_target(code, scope, target, value)?;
                }
            }
            Instruction::DeleteItem(item) => {
                let object = eval(self, &item.value)?;
                let key = eval(self, &item.slice)?;
                self.delete_item(object, key)?;
            }
            Instruction::AugAssign(stmt) => {
                let update = |ctx: &mut Self, left: Id| -> Result<Id> {
                    ctx.heap.keep(left);
                    let right = eval(ctx, &stmt.value)?;
                    let value = ctx.inplace(stmt.op.as_str(), left, right)?;
                    ctx.heap.keep(value);
                    Ok(value)
                };
                match &*stmt.target {
                    ast::Expr::Name(name) => {
                        let left = eval(self, &stmt.target)?;
                        let value = update(self, left)?;
                        self.bind_source_name(scope, name.id.as_str(), value)?;
                    }
                    ast::Expr::Attribute(field) => {
                        let object = eval(self, &field.value)?;
                        let left = self.lookup_attr(object, field.attr.as_str())?;
                        let value = update(self, left)?;
                        self.set_attr(object, field.attr.as_str(), value)?;
                    }
                    ast::Expr::Subscript(item) => {
                        let object = eval(self, &item.value)?;
                        let key = eval(self, &item.slice)?;
                        let left = self.get_item(object, key)?;
                        let value = update(self, left)?;
                        self.put_item(object, key, value)?;
                    }
                    _ => unreachable!("validated augmented assignment target"),
                }
            }
            Instruction::Unless(e, destination) => {
                if !self.source_condition(code, scope, e)? {
                    *ip = *destination;
                }
            }
            Instruction::Jump(destination) => *ip = *destination,
            Instruction::PopIter => {
                let iterator = iterators.pop().expect("active source loop");
                self.release_loop_iterator(iterator);
            }
            Instruction::Iter(e) => {
                let v = eval(self, e)?;
                iterators.push(self.iterator(v)?);
            }
            Instruction::Next(target, end) => {
                let iter = *iterators.last().expect("active source loop");
                let owner = self.source_loop_owner(iter);
                let next = if let Some(result) = advanced.take() {
                    result?
                } else if code.generator
                    && let Value::Generator { code, .. } = self.heap.get(iter)
                    && !matches!(self.code_object(*code).body, Body::Native(_))
                {
                    return Ok(Some(Step::Advance(iter, *ip - 1)));
                } else {
                    self.next_item(iter)?
                };
                match next {
                    Some(v) => {
                        if let Some(target) = target {
                            self.bind_target(code, scope, target, v)?;
                        } else {
                            return Ok(Some(Step::Yield(v, *ip - 1)));
                        }
                    }
                    None => {
                        iterators.pop();
                        self.release_source_loop_owner(owner);
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
            Instruction::Import { module, alias } => {
                let mut imported = self.import_source_module(owner, scope, module, None)?;
                let mut parts = module.split('.');
                let first = parts.next().expect("nonempty module name");
                if alias.is_some() {
                    for part in parts {
                        let part = self.text(part);
                        imported = self.op("from", imported, part, NONE)?;
                    }
                }
                self.bind_source_name(scope, alias.as_deref().unwrap_or(first), imported)?;
            }
            Instruction::From(module, names) => {
                let m = self.import_source_module(owner, scope, module, Some(names))?;
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

    /// Unpack each level before binding its component targets from left to right.
    fn bind_target(
        &mut self,
        code: &Program,
        scope: Id,
        target: &ast::Expr,
        value: Id,
    ) -> Result<()> {
        match target {
            ast::Expr::Name(n) => self.bind_source_name(scope, n.id.as_str(), value)?,
            ast::Expr::Attribute(field) => {
                let object = self.source_expr(code, scope, &field.value)?;
                self.set_attr(object, field.attr.as_str(), value)?;
            }
            ast::Expr::Subscript(item) => {
                let object = self.source_expr(code, scope, &item.value)?;
                let key = self.source_expr(code, scope, &item.slice)?;
                self.put_item(object, key, value)?;
            }
            ast::Expr::Starred(star) => self.bind_target(code, scope, &star.value, value)?,
            ast::Expr::Tuple(ast::ExprTuple { elts, .. })
            | ast::Expr::List(ast::ExprList { elts, .. }) => {
                let count = elts.len();
                let star = elts
                    .iter()
                    .position(|item| matches!(item, ast::Expr::Starred(_)));
                let unpacked = self.unpack_items(value, count, star)?;
                for (element, item) in elts.iter().zip(unpacked) {
                    self.bind_target(code, scope, element, item)?;
                }
            }
            _ => unreachable!("validated unpacking target"),
        }
        Ok(())
    }

    fn source_expr(&mut self, code: &Program, scope: Id, expr: &ast::Expr) -> Result<Id> {
        let value = stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || {
            self.source_expr_inner(code, scope, expr)
        })?;
        self.heap.keep(value);
        Ok(value)
    }

    /// Preserve the selected operand and any truth test that short-circuited a nested expression.
    fn source_bool_operand(
        &mut self,
        code: &Program,
        scope: Id,
        expr: &ast::Expr,
    ) -> Result<(Id, Option<bool>)> {
        let ast::Expr::BoolOp(op) = expr else {
            return Ok((self.source_expr(code, scope, expr)?, None));
        };
        let (last, operands) = op
            .values
            .split_last()
            .expect("boolean expression has operands");
        let stop = op.op == ast::BoolOp::Or;
        for operand in operands {
            let mark = self.heap.temporary_mark();
            let (value, known) = self.source_bool_operand(code, scope, operand)?;
            if known.map_or_else(|| self.truth(value), Ok)? == stop {
                return Ok((value, Some(stop)));
            }
            self.discard_truth(mark);
        }
        self.source_bool_operand(code, scope, last)
    }

    /// Return the comparison result and whether an earlier comparison already tested false.
    fn source_comparison(
        &mut self,
        code: &Program,
        scope: Id,
        comparison: &ast::ExprCompare,
    ) -> Result<(Id, bool)> {
        let mut left = self.source_expr(code, scope, &comparison.operands[0])?;
        for (index, (op, right)) in comparison
            .ops
            .iter()
            .zip(&comparison.operands[1..])
            .enumerate()
        {
            let right = self.source_expr(code, scope, right)?;
            let result = self.op(op.as_str(), left, right, NONE)?;
            if index + 1 == comparison.ops.len() {
                return Ok((result, false));
            }
            if !self.truth(result)? {
                return Ok((result, true));
            }
            left = right;
        }
        unreachable!("comparison has at least one operator")
    }

    /// Evaluate a branch condition without testing a short-circuited operand twice.
    fn source_condition(&mut self, code: &Program, scope: Id, expr: &ast::Expr) -> Result<bool> {
        if let ast::Expr::UnaryOp(op) = expr
            && op.op == ast::UnaryOp::Not
        {
            return Ok(!self.source_condition(code, scope, &op.operand)?);
        }
        if let ast::Expr::BoolOp(op) = expr {
            let stop = op.op == ast::BoolOp::Or;
            for operand in &op.values {
                if self.source_condition(code, scope, operand)? == stop {
                    return Ok(stop);
                }
            }
            return Ok(!stop);
        }
        if let ast::Expr::If(branch) = expr {
            let arm = if self.source_condition(code, scope, &branch.test)? {
                &branch.body
            } else {
                &branch.orelse
            };
            return self.source_condition(code, scope, arm);
        }
        let mark = self.heap.temporary_mark();
        let truth = if let ast::Expr::Compare(comparison) = expr {
            let (value, stopped) = self.source_comparison(code, scope, comparison)?;
            !stopped && self.truth(value)?
        } else {
            let value = self.source_expr(code, scope, expr)?;
            self.truth(value)?
        };
        self.discard_truth(mark);
        Ok(truth)
    }

    fn source_expr_inner(&mut self, code: &Program, scope: Id, expr: &ast::Expr) -> Result<Id> {
        use ast::Expr;
        let eval = |ctx: &mut Self, e: &Expr| ctx.source_expr(code, scope, e);
        match expr {
            Expr::Name(n) => {
                let name = n.id.as_str();
                if code.name != "<module>"
                    && code.locals.contains(name)
                    && self.scope_name(scope, name).is_none()
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
            Expr::StringLiteral(_) | Expr::BytesLiteral(_) => Ok(code.literals[&expr.range()]),
            Expr::NoneLiteral(_) => Ok(NONE),
            Expr::BooleanLiteral(v) => Ok(if v.value { TRUE } else { FALSE }),
            Expr::NumberLiteral(v) => {
                if let ast::Number::Float(value) = &v.value {
                    return Ok(self.alloc(Value::Float(*value)));
                }
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
            Expr::Set(display) => {
                let items = display
                    .elts
                    .iter()
                    .map(|item| eval(self, item))
                    .collect::<Result<Vec<_>>>()?;
                let result = self.alloc(Value::Set {
                    entries: Default::default(),
                    frozen: false,
                });
                for item in items {
                    builtins::set_add(self, result, item)?;
                }
                Ok(result)
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
            Expr::BoolOp(_) => Ok(self.source_bool_operand(code, scope, expr)?.0),
            Expr::BinOp(v) => {
                let a = eval(self, &v.left)?;
                let b = eval(self, &v.right)?;
                self.op(v.op.as_str(), a, b, NONE)
            }
            Expr::UnaryOp(v) => {
                let value = eval(self, &v.operand)?;
                let op = match v.op {
                    ast::UnaryOp::UAdd => "pos",
                    ast::UnaryOp::USub => "neg",
                    op => op.as_str(),
                };
                self.op(op, value, NONE, NONE)
            }
            Expr::Compare(v) => Ok(self.source_comparison(code, scope, v)?.0),
            Expr::If(v) => {
                let branch = if self.source_condition(code, scope, &v.test)? {
                    &v.body
                } else {
                    &v.orelse
                };
                eval(self, branch)
            }
            Expr::Call(v) => {
                let mark = self.heap.temporary_mark();
                let f = if let Some(field) =
                    imre_ast::direct_method(v, &code.method_imports, code.private_names.as_deref())
                {
                    let receiver = eval(self, &field.value)?;
                    self.lookup_method(receiver, field.attr.as_str())?
                } else {
                    eval(self, &v.func)?
                };
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
                let result = self.call(f, args, kwargs)?;
                self.finish_call_operands(mark, result);
                Ok(result)
            }
            _ => Err(unsupported(format!(
                "{:?}",
                ast::AnyNodeRef::from(expr).kind()
            ))),
        }
    }
}
