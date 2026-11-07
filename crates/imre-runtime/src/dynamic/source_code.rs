//! In-process execution of source received by the boxed Python runtime.

mod annotations;
mod comprehensions;

use annotations::CONDITIONAL;

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::rc::Rc;

use imre_ast::code_flags;
use imre_ast::comprehension::{Output, Plan};
use ruff_python_ast as ast;
use ruff_text_size::Ranged;

use super::value::{Body, ExecGlobals, Location};
use super::*;

#[derive(Clone)]
struct SourceFilename {
    // Code objects own the Python spelling. Rust diagnostic locations require scalar text.
    object: Id,
    display: Rc<str>,
}

pub(super) struct Program {
    filename: SourceFilename,
    // All nested code shares a source map. Prelinked suspended suites start in a synthetic header.
    source_lines: Rc<[usize]>,
    linked_body_line: Option<u32>,
    // Check earlier scope declarations before reporting a misplaced future import.
    syntax_limit: Option<ruff_text_size::TextRange>,
    pub name: String,
    qualname: String,
    nested_global: bool,
    class_attributes: Option<BTreeSet<String>>,
    class_cell: bool,
    method_class_cell: Option<bool>,
    class_nonlocal: bool,
    needs_class_cell: bool,
    needs_dict_cell: bool,
    postponed_annotations: bool,
    has_named_bindings: bool,
    doc: Option<ruff_text_size::TextRange>,
    strip_docstrings: bool,
    literals: HashMap<ruff_text_size::TextRange, Id>,
    // For each capturing OR alternative, its operand slots in first-alternative binding order.
    alternative_captures: HashMap<ruff_text_size::TextRange, Vec<usize>>,
    pub(super) string_roots: Id,
    first_line: u32,
    kind: FunctionKind,
    scope_flags: u32,
    unlowered_yield: bool,
    interactive: bool,
    locals: indexmap::IndexSet<String>,
    directives: HashMap<String, (ScopeDirective, ruff_text_size::TextSize)>,
    reads: BTreeSet<String>,
    method_imports: Rc<std::collections::BTreeSet<String>>,
    private_names: Option<Rc<imre_ast::Names>>,
    template_expressions: Rc<HashMap<ruff_text_size::TextSize, String>>,
    instructions: Vec<(u32, Instruction)>,
    // Declarations in source lowering order, including nested suites.
    deferred_annotations: Vec<(String, ast::Expr)>,
    // Indices of the class declarations guarded by the private annotation cell.
    conditional_fields: Vec<usize>,
    class_binds_conditional_name: bool,
    expression_functions: HashMap<ruff_text_size::TextRange, (usize, Definition)>,
    eager_comprehension_targets: HashMap<ruff_text_size::TextRange, HashSet<String>>,
}

/// The original source expression and line, with no references into the defining heap.
pub(super) struct CapturedAcquire {
    line: u32,
    expression: ast::Expr,
}

impl CapturedAcquire {
    pub(super) fn load(
        self,
        ctx: &mut Context,
        scope: Id,
        name: Id,
        parameters: Id,
        qualname: Id,
        (filename, first_line): (Id, u32),
    ) -> Result<Id> {
        let mut program = Program::new(ctx.source_filename(filename)?, ctx.str_value(name)?, false);
        program.qualname = ctx.str_value(qualname)?.to_owned();
        program.first_line = first_line;
        program
            .instructions
            .push((self.line, Instruction::Eval(self.expression)));
        program.prepare_strings(
            &mut ctx.heap,
            &mut SharedConstants::default(),
            &StringReplacements::new(),
            &|_| self.line,
        )?;
        let freevars = ctx.enclosing_freevars(scope, program.reads.iter().map(String::as_str));
        Ok(ctx.box_source_closure(Rc::new(program), Some(parameters), freevars))
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FunctionKind {
    Plain,
    Generator,
    GeneratorLambda,
    Coroutine,
    AsyncGenerator,
}

impl FunctionKind {
    fn prelinked(code: &value::CodeObject) -> Self {
        if code.flags & imre_ast::code_flags::ASYNC_GENERATOR != 0 {
            Self::AsyncGenerator
        } else if code.is_coroutine() {
            Self::Coroutine
        } else {
            Self::Generator
        }
    }

    fn is_async(self) -> bool {
        matches!(self, Self::Coroutine | Self::AsyncGenerator)
    }
}

#[derive(Clone, Copy, PartialEq)]
enum ScopeDirective {
    Global,
    Nonlocal,
}

impl ScopeDirective {
    fn keyword(self) -> &'static str {
        match self {
            Self::Global => "global",
            Self::Nonlocal => "nonlocal",
        }
    }
}

#[derive(Clone, Copy)]
enum EnclosingBinding {
    TypeParameter,
    Local,
    ClassCell,
}

// A plain if or while needs a single forward destination without a heap allocation.
type BranchTargets = smallvec::SmallVec<[usize; 1]>;

// CPython's BUILD_SET batches leading operands only within its stack-use guideline.
fn set_display_prefix(elements: &[ast::Expr]) -> usize {
    if elements.len() <= 30 {
        elements
            .iter()
            .take_while(|item| !item.is_starred_expr())
            .count()
    } else {
        0
    }
}

#[derive(Clone, Copy, PartialEq)]
enum BranchUse {
    Condition,
    Operand,
}

/// Source locations of supported awaits within one expression. The plan is discarded
/// after lowering; untouched subtrees stay in the ordinary expression evaluator.
struct OperandPlan {
    sites: smallvec::SmallVec<[ruff_text_size::TextSize; 1]>,
    line: u32,
    function: bool,
}

impl OperandPlan {
    fn new(root: &ast::Expr, kind: FunctionKind, line: u32, function: bool) -> Self {
        let mut sites = smallvec::SmallVec::new();
        if kind.is_async() {
            let mut work = smallvec::SmallVec::<[&ast::Expr; 8]>::new();
            work.push(root);
            while let Some(expr) = work.pop() {
                match expr {
                    ast::Expr::Await(a) => {
                        sites.push(a.start());
                        work.push(&a.value);
                    }
                    ast::Expr::Generator(g) => work.push(&g.generators[0].iter),
                    expr if let Some(comp) = Plan::from_expr(expr) => {
                        if comp.suspends() {
                            sites.push(expr.start());
                        }
                    }
                    ast::Expr::Tuple(ast::ExprTuple { elts, .. })
                    | ast::Expr::List(ast::ExprList { elts, .. })
                        if !elts.iter().any(ast::Expr::is_starred_expr) =>
                    {
                        work.extend(elts.iter());
                    }
                    ast::Expr::BinOp(b) => work.extend([&*b.left, &*b.right]),
                    ast::Expr::Compare(c) => work.extend(c.operands.iter()),
                    ast::Expr::BoolOp(b) => work.extend(b.values.iter()),
                    ast::Expr::UnaryOp(u) => work.push(&u.operand),
                    ast::Expr::Attribute(a) => work.push(&a.value),
                    ast::Expr::Subscript(s) => work.extend([&*s.value, &*s.slice]),
                    ast::Expr::Slice(s) => {
                        work.extend(
                            [s.lower.as_deref(), s.upper.as_deref(), s.step.as_deref()]
                                .into_iter()
                                .flatten(),
                        );
                    }
                    ast::Expr::Named(n) => work.push(&n.value),
                    ast::Expr::Dict(d) => {
                        for item in &d.items {
                            work.extend(item.key.as_ref());
                            work.push(&item.value);
                        }
                    }
                    ast::Expr::Set(s) => {
                        work.extend(s.elts.iter().map(|item| match item {
                            ast::Expr::Starred(star) => star.value.as_ref(),
                            item => item,
                        }));
                    }
                    ast::Expr::If(b) => work.extend([&*b.body, &*b.test, &*b.orelse]),
                    ast::Expr::Call(c)
                        if !c.arguments.args.iter().any(ast::Expr::is_starred_expr)
                            && c.arguments.keywords.iter().all(|kw| kw.arg.is_some()) =>
                    {
                        work.push(&c.func);
                        work.extend(c.arguments.args.iter());
                        work.extend(c.arguments.keywords.iter().map(|kw| &kw.value));
                    }
                    _ => {}
                }
            }
            sites.sort_unstable();
        }
        Self {
            sites,
            line,
            function,
        }
    }

    fn suspends(&self, expr: &ast::Expr) -> bool {
        let first = self.sites.partition_point(|&site| site < expr.start());
        self.sites.get(first).is_some_and(|&site| site < expr.end())
    }
}

enum Instruction {
    Eval(ast::Expr),
    Discard(ast::Expr),
    AugAssign(ast::StmtAugAssign),
    DeleteName(String),
    DeleteItem(ast::ExprSubscript),
    DeleteAttr(ast::ExprAttribute),
    Assign(Vec<ast::Expr>, ast::Expr),
    Import {
        module: String,
        alias: Option<String>,
    },
    From(String, u32, Vec<(String, String)>),
    Define(Definition),
    ReturnClass(Definition),
    CheckAnnotationFormat,
    Annotations {
        entries: Vec<(String, ast::Expr)>,
        scope: annotations::AnnotationScope,
    },
    ModuleAnnotations(Option<Rc<Program>>),
    ModuleDoc(ruff_text_size::TextRange, String),
    MarkAnnotation(usize),
    StringAnnotation(String, ast::Expr),
    ClassAnnotator(Rc<Program>),
    ClassStart,
    ClassEnd,
    Yield(Option<ast::Expr>),
    YieldResult,
    MakeGenerator(ast::Expr),
    Return(Option<ast::Expr>),
    ReturnSent,
    Raise(ast::StmtRaise),
    Assert(ast::StmtAssert),
    Unless(ast::Expr, usize),
    Match {
        subject: ast::Expr,
        cases: Vec<SourceCase>,
        end: usize,
    },
    Iter(Option<ast::Expr>, bool),
    EnterComprehension {
        kind: &'static str,
        targets: HashSet<String>,
        end: usize,
    },
    CollectItem {
        value: Option<ast::Expr>,
        keyed: bool,
    },
    LeaveComprehension,
    AsyncNext,
    BindAsyncItem(Option<ast::Expr>),
    UnpackTarget {
        count: usize,
        star: Option<usize>,
    },
    BindOperand(ast::Expr),
    StoreAttribute(ast::Identifier),
    StoreSubscript,
    EndAsyncFor,
    Await(Option<ast::Expr>),
    SaveOperand(Option<ast::Expr>),
    BuildSequence {
        tuple: bool,
        count: usize,
    },
    BuildSlice([bool; 3]),
    StoreNamed(ast::Identifier),
    StartDict,
    StartSet(usize),
    InsertDict {
        unpack: bool,
    },
    InsertSet {
        unpack: bool,
    },
    FinishDisplay,
    Binary(&'static str, ruff_text_size::TextRange, bool),
    NextComparison {
        end: usize,
        condition: bool,
    },
    Branch {
        value: Option<ast::Expr>,
        when: bool,
        usage: BranchUse,
        end: usize,
    },
    LoadResult(ast::Expr),
    Unary(ast::UnaryOp),
    Attribute {
        name: ast::Identifier,
        method: bool,
    },
    JumpResult(usize),
    SaveCallMethod(ast::ExprAttribute),
    CallSaved {
        positional: usize,
        keywords: Vec<ast::Identifier>,
        position: value::ExpressionPosition,
    },
    Next(Option<ast::Expr>, usize),
    Jump(usize),
    Break(usize),
    EnterFinally {
        handler: usize,
        end: usize,
        annotation_count: usize,
    },
    EnterWith {
        item: ast::WithItem,
        handler: usize,
    },
    AsyncWithMethods(Option<ast::Expr>),
    CallAsyncEnter,
    AwaitWithResult(&'static str),
    EnterAsyncWith {
        target: Option<Box<ast::Expr>>,
        handler: usize,
        end: usize,
    },
    ExitWith,
    CallAsyncExit,
    TestAsyncExit,
    FinishAsyncExit,
    EnterExcept {
        handler: usize,
        end: usize,
    },
    ExceptMatch(ast::Expr, usize),
    BindException(String),
    ClearException(String),
    StartFinally,
    EndFinally,
    Delegate,
    BindSent(Vec<ast::Expr>),
}

struct SourceCase {
    pattern: ast::Pattern,
    captures: Vec<String>,
    line_offset: u32,
    guard: Option<(u32, Box<ast::Expr>)>,
    body: usize,
}

struct Definition {
    binding: String,
    decorators: Vec<ast::Decorator>,
    body: Rc<Program>,
    kind: DefinitionKind,
    annotate: Option<Rc<Program>>,
}

enum DefinitionKind {
    TypeAlias(Option<Box<ast::TypeParams>>),
    Function {
        parameters: Box<ast::Parameters>,
        type_params: Option<Box<ast::TypeParams>>,
        captured_locks: Option<Box<[String]>>,
    },
    Class {
        arguments: Option<Box<ast::Arguments>>,
        parameterized: bool,
        needs_enclosing_frame: bool,
    },
    ClassParameters(Box<ast::TypeParams>),
}

impl DefinitionKind {
    fn type_params(&self) -> Option<&ast::TypeParams> {
        match self {
            Self::Function { type_params, .. } | Self::TypeAlias(type_params) => {
                type_params.as_deref()
            }
            Self::ClassParameters(params) => Some(params),
            Self::Class { .. } => None,
        }
    }
}

#[derive(Default)]
struct ComprehensionBindings {
    iteration_variables: HashSet<String>,
    named_assignments: HashSet<String>,
}

/// Assignment expressions bind in the containing code unit, including unexecuted branches.
#[derive(Default)]
struct NamedBindings {
    targets: Vec<(String, ruff_text_size::TextSize, bool)>,
    comprehensions: Vec<ComprehensionBindings>,
    invalid_target: Option<Error>,
    in_iterable: bool,
    invalid_iterable: bool,
}

impl<'a> ast::visitor::Visitor<'a> for NamedBindings {
    // Compound statements lower their nested suites in their own source order.
    fn visit_body(&mut self, _body: &'a [ast::Stmt]) {}

    fn visit_annotation(&mut self, _expr: &'a ast::Expr) {}

    fn visit_expr(&mut self, expr: &'a ast::Expr) {
        if let ast::Expr::Named(named) = expr
            && let ast::Expr::Name(name) = named.target.as_ref()
        {
            if self.in_iterable {
                self.invalid_iterable = true;
            } else {
                if self
                    .comprehensions
                    .iter()
                    .any(|frame| frame.iteration_variables.contains(name.id.as_str()))
                {
                    self.invalid_target.get_or_insert_with(|| {
                        Error::new(
                            "SyntaxError",
                            format!(
                                "assignment expression cannot rebind comprehension iteration variable '{}'",
                                name.id
                            ),
                        )
                    });
                }
                if let Some(frame) = self.comprehensions.last_mut() {
                    frame.named_assignments.insert(name.id.to_string());
                }
                self.targets.push((
                    name.id.to_string(),
                    name.start(),
                    !self.comprehensions.is_empty(),
                ));
            }
        }
        if !self.in_iterable
            && let ast::Expr::Lambda(lambda) = expr
        {
            if let Some(params) = &lambda.parameters {
                self.visit_parameters(params);
            }
            return;
        }
        if let Some((output, clauses)) = Output::from_expr(expr) {
            self.comprehensions.push(ComprehensionBindings::default());
            stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || {
                for clause in clauses {
                    let frame = self
                        .comprehensions
                        .last_mut()
                        .expect("current comprehension");
                    if let Err(error) = Program::collect_target_names(
                        &clause.target,
                        &mut frame.iteration_variables,
                    ) {
                        self.invalid_target.get_or_insert(error);
                    }
                    if let Some(name) = frame
                        .named_assignments
                        .intersection(&frame.iteration_variables)
                        .min()
                    {
                        self.invalid_target.get_or_insert_with(|| {
                            Error::new(
                                "SyntaxError",
                                format!(
                                    "comprehension inner loop cannot rebind assignment expression target '{name}'"
                                ),
                            )
                        });
                    }
                    self.visit_expr(&clause.target);
                    self.visit_comprehension(clause);
                }
                for element in output.elements() {
                    self.visit_expr(element);
                }
            });
            self.comprehensions.pop();
            return;
        }
        stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || {
            ast::visitor::walk_expr(self, expr)
        });
    }

    fn visit_comprehension(&mut self, clause: &'a ast::Comprehension) {
        let previous = std::mem::replace(&mut self.in_iterable, true);
        self.visit_expr(&clause.iter);
        self.in_iterable = previous;
        for test in &clause.ifs {
            self.visit_expr(test);
        }
    }
}

impl NamedBindings {
    fn declare(self, program: &mut Program) -> Result<()> {
        if self.invalid_iterable {
            return Err(Error::new(
                "SyntaxError",
                "assignment expression cannot be used in a comprehension iterable expression",
            ));
        }
        if let Some(error) = self.invalid_target {
            return Err(error);
        }
        for (name, offset, comprehension) in self.targets {
            if comprehension && program.class_attributes.is_some() {
                return Err(Error::new(
                    "SyntaxError",
                    "assignment expression within a comprehension cannot be used in a class body",
                ));
            }
            if let Some((kind, directive)) = program.directives.get(&name)
                && offset < *directive
            {
                return Err(Error::new(
                    "SyntaxError",
                    format!(
                        "name '{name}' is assigned to before {} declaration",
                        kind.keyword()
                    ),
                ));
            }
            if name == "__debug__" {
                return Err(Error::new("SyntaxError", "cannot assign to __debug__"));
            }
            if comprehension && program.name == "<module>" {
                program
                    .directives
                    .entry(name.clone())
                    .or_insert((ScopeDirective::Global, 0.into()));
            }
            program.locals.insert(name);
        }
        Ok(())
    }
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
    protected: &'a mut Vec<FinallyBlock>,
    advanced: Option<Result<DelegateOutcome>>,
    nested_resume: bool,
    comprehension: Option<(Id, usize)>,
    sent: Id,
    thrown: Option<(Error, Vec<Id>)>,
}

impl<'a> SourceCursor<'a> {
    fn active_comprehension(blocks: &[FinallyBlock]) -> Option<(Id, usize)> {
        blocks.iter().rev().find_map(|b| match b.kind {
            HandlerKind::Comprehension(scope) => Some((scope, b.depth)),
            _ => None,
        })
    }

    /// Resume the source frame with its live iterators, protected blocks and pending input.
    fn new(
        ip: usize,
        iterators: &'a mut Vec<Id>,
        protected: &'a mut Vec<FinallyBlock>,
        advanced: Option<Result<DelegateOutcome>>,
        input: GeneratorInput,
    ) -> Self {
        let (sent, thrown) = match input {
            GeneratorInput::Send(sent) => (sent, None),
            GeneratorInput::Throw { error, args } => (NONE, Some((error, args))),
        };
        let comprehension = Self::active_comprehension(protected);
        Self {
            ip,
            comprehension,
            iterators,
            protected,
            advanced,
            nested_resume: false,
            sent,
            thrown,
        }
    }
}

pub(super) enum GeneratorInput {
    Send(Id),
    Throw { error: Error, args: Vec<Id> },
}

pub(super) enum DelegateOutcome {
    Yield(Id),
    Done(Id),
    LookupFailed(Error),
}

#[derive(Clone, Copy)]
enum Transfer {
    Jump { destination: usize, pop_iter: bool },
    Return(Id),
    Raise(Id),
}

#[derive(Clone, Copy)]
enum HandlerKind {
    Finally(usize),
    Except,
    With(Id),
    Comprehension(Id),
}

/// A protected source region survives yields, including yields in its cleanup.
#[derive(Clone)]
pub(super) struct FinallyBlock {
    start: usize,
    handler: usize,
    end: usize,
    depth: usize,
    pending: Option<Transfer>,
    kind: HandlerKind,
}

impl FinallyBlock {
    pub(super) fn roots(&self) -> impl Iterator<Item = Id> {
        let exit = match self.kind {
            HandlerKind::With(root) | HandlerKind::Comprehension(root) => Some(root),
            _ => None,
        };
        exit.into_iter().chain(match self.pending {
            Some(Transfer::Raise(id) | Transfer::Return(id)) => Some(id),
            _ => None,
        })
    }
}

enum Step {
    Yield(Id, usize),
    Return(Id),
    Advance(Id, usize, Id),
    Rejected(Error, usize),
}

fn unsupported(what: impl std::fmt::Display) -> Error {
    Error::new("NotImplementedError", format!("runtime source: {what}"))
}

/// Constants shared by code objects produced by one source compilation.
#[derive(Default)]
struct SharedConstants<'a> {
    text: HashMap<Cow<'a, str>, Id>,
    surrogate_text: HashMap<Vec<u32>, Id>,
    literal_tuples: HashMap<Vec<Id>, Id>,
}

/// One literal segment can span multiple adjacent template tokens, including raw ones.
#[derive(Default)]
struct TemplateSegment<'a> {
    text: Cow<'a, str>,
    replacements: Vec<u32>,
}

impl<'a> TemplateSegment<'a> {
    fn push(
        &mut self,
        lit: &'a ast::InterpolatedStringLiteralElement,
        spellings: &StringReplacements,
    ) {
        self.text += &*lit.value;
        // Escaped braces can join several middle tokens into one AST element.
        if lit.value.contains(char::REPLACEMENT_CHARACTER) {
            for (_, points) in spellings.range(lit.start()..lit.end()) {
                self.replacements.extend_from_slice(points);
            }
        }
    }
}

/// Names bound by enclosing inlined set comprehensions accompany each pending expression.
type ScopedExpression<'a> = (&'a ast::Expr, Option<Rc<HashSet<String>>>);

/// Collect names and string constants without recursing through an expression's left spine.
struct CompiledStrings<'a, 'ctx> {
    heap: &'ctx mut value::Heap,
    shared: &'ctx mut SharedConstants<'a>,
    spellings: &'ctx StringReplacements,
    constants: &'ctx mut HashMap<ruff_text_size::TextRange, Id>,
    roots: Vec<Id>,
    pending: Vec<ScopedExpression<'a>>,
    bound: Option<Rc<HashSet<String>>>,
    comprehension_reads: BTreeSet<String>,
    eager_comprehension_targets: &'ctx mut HashMap<ruff_text_size::TextRange, HashSet<String>>,
    reads: &'ctx mut BTreeSet<String>,
    directives: &'ctx HashMap<String, (ScopeDirective, ruff_text_size::TextSize)>,
    exhausted: bool,
}

impl<'a> ast::visitor::Visitor<'a> for CompiledStrings<'a, '_> {
    fn visit_expr(&mut self, expr: &'a ast::Expr) {
        if self.pending.try_reserve(1).is_err() {
            self.exhausted = true;
        } else {
            self.pending.push((expr, self.bound.clone()));
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

    fn text(&mut self, text: Cow<'a, str>) -> Result<Id> {
        if let Some(&id) = self.shared.text.get(text.as_ref()) {
            return Ok(id);
        }
        self.shared
            .text
            .try_reserve(1)
            .map_err(|_| crate::Error::allocation())?;
        let id = self
            .heap
            .source_string_constant(&text)
            .ok_or_else(crate::Error::allocation)?;
        self.shared.text.insert(text, id);
        Ok(id)
    }

    /// Template strings and immutable tuple displays share the same code constant.
    fn tuple_constant(&mut self, items: Vec<Id>) -> Result<Id> {
        let id = if let Some(&id) = self.shared.literal_tuples.get(&items) {
            id
        } else {
            self.shared
                .literal_tuples
                .try_reserve(1)
                .map_err(|_| crate::Error::allocation())?;
            let mut tuple = Vec::new();
            tuple
                .try_reserve_exact(items.len())
                .map_err(|_| crate::Error::allocation())?;
            tuple.extend_from_slice(&items);
            let id = self.heap.packed_tuple(tuple, true);
            self.shared.literal_tuples.insert(items, id);
            id
        };
        Ok(id)
    }

    /// Build constants at compilation, including nested tuples whose first evaluation may be later.
    fn literal(&mut self, expr: &'a ast::Expr) -> Result<Id> {
        match expr {
            ast::Expr::NoneLiteral(_) => Ok(NONE),
            ast::Expr::EllipsisLiteral(_) => Ok(value::ELLIPSIS),
            ast::Expr::BooleanLiteral(v) => Ok(if v.value { TRUE } else { FALSE }),
            ast::Expr::StringLiteral(v) => self.literal_text(v),
            ast::Expr::BytesLiteral(v) => Ok(self
                .heap
                .bytes_from_data(v.value.bytes().collect::<Vec<_>>())),
            ast::Expr::NumberLiteral(v) => {
                let number = match &v.value {
                    ast::Number::Float(n) => Value::Float(*n),
                    ast::Number::Complex { real, imag } => {
                        Value::Complex(super::complex::Complex {
                            real: *real,
                            imag: *imag,
                        })
                    }
                    ast::Number::Int(n) => {
                        let n = crate::parse_integer_literal(&n.to_string())
                            .ok_or_else(|| Error::new("ValueError", "invalid integer"))?;
                        Value::Int(crate::Int::new(n))
                    }
                };
                Ok(self.heap.alloc(number))
            }
            ast::Expr::Tuple(tuple) => {
                let mut items = Vec::new();
                items
                    .try_reserve_exact(tuple.elts.len())
                    .map_err(|_| crate::Error::allocation())?;
                for item in &tuple.elts {
                    items.push(self.literal(item)?);
                }
                self.tuple_constant(items)
            }
            _ => unreachable!("only immutable literal elements form code constants"),
        }
    }

    /// Pool template text by its original Python code points.
    fn template_text(&mut self, segment: TemplateSegment<'a>) -> Result<Id> {
        self.decoded_text(segment.text, segment.replacements.iter().copied())
    }

    fn literal_text(&mut self, literal: &'a ast::ExprStringLiteral) -> Result<Id> {
        self.decoded_text(
            literal.value.to_str().into(),
            self.spellings
                .range(literal.start()..literal.end())
                .flat_map(|(_, points)| points.iter().copied()),
        )
    }

    fn decoded_text(
        &mut self,
        text: Cow<'a, str>,
        replacements: impl Iterator<Item = u32> + Clone,
    ) -> Result<Id> {
        let Some(points) = original_points(&text, replacements)? else {
            return self.text(text);
        };
        if let Some(&id) = self.shared.surrogate_text.get(&points) {
            return Ok(id);
        }
        self.shared
            .surrogate_text
            .try_reserve(1)
            .map_err(|_| crate::Error::allocation())?;
        let id = self.heap.alloc(Value::SurrogateStr(points.clone()));
        self.shared.surrogate_text.insert(points, id);
        Ok(id)
    }

    /// Keep exact code points for f-string text that has no scalar spelling.
    fn format_literal(&mut self, range: ruff_text_size::TextRange, text: &'a str) -> Result<()> {
        let replacements = self
            .spellings
            .range(range.start()..range.end())
            .flat_map(|(_, points)| points.iter().copied());
        if replacements.clone().any(|p| p != 0xfffd) {
            let id = self.decoded_text(text.into(), replacements)?;
            self.constant(range, id)?;
        }
        Ok(())
    }

    fn format_elements(&mut self, elements: &'a [ast::InterpolatedStringElement]) -> Result<()> {
        for element in elements {
            match element {
                ast::InterpolatedStringElement::Literal(lit) => {
                    self.format_literal(lit.range(), &lit.value)?;
                }
                ast::InterpolatedStringElement::Interpolation(field) => {
                    if let Some(spec) = &field.format_spec {
                        self.format_elements(&spec.elements)?;
                    }
                }
            }
        }
        Ok(())
    }

    fn template_strings(&mut self, template: &'a ast::ExprTString) -> Result<()> {
        let mut items = Vec::new();
        let mut segment = TemplateSegment::default();
        for element in template.value.elements() {
            match element {
                ast::InterpolatedStringElement::Literal(lit) => {
                    segment.push(lit, self.spellings);
                }
                ast::InterpolatedStringElement::Interpolation(field) => {
                    items
                        .try_reserve(1)
                        .map_err(|_| crate::Error::allocation())?;
                    items.push(self.template_text(std::mem::take(&mut segment))?);
                    if let Ok(Some(lit)) = imre_ast::strings::template_format_spec(field) {
                        let mut spec = TemplateSegment::default();
                        spec.push(lit, self.spellings);
                        let id = self.template_text(spec)?;
                        self.constant(lit.range(), id)?;
                    }
                }
            }
        }
        items
            .try_reserve(1)
            .map_err(|_| crate::Error::allocation())?;
        items.push(self.template_text(segment)?);
        let id = self.tuple_constant(items)?;
        self.constant(template.range(), id)
    }

    /// Root a literal for later evaluation at its source position.
    fn constant(&mut self, range: ruff_text_size::TextRange, id: Id) -> Result<()> {
        self.constants
            .try_reserve(1)
            .map_err(|_| Error::new("MemoryError", ""))?;
        self.constants.insert(range, id);
        self.keep(id)
    }

    /// Only defaults and the outer iterable of a comprehension are evaluated in this scope.
    fn check_directive_reads(&mut self) -> Result<()> {
        use ast::visitor::Visitor;
        let roots = std::mem::take(&mut self.pending);
        for (root, _) in &roots {
            self.visit_expr(root);
        }
        while let Some((expr, _)) = self.pending.pop() {
            match expr {
                ast::Expr::Name(name) => {
                    if let Some((kind, position)) = self.directives.get(name.id.as_str())
                        && name.start() < *position
                        && name.ctx == ast::ExprContext::Load
                    {
                        return Err(Error::new(
                            "SyntaxError",
                            format!(
                                "name '{}' is used prior to {} declaration",
                                name.id,
                                kind.keyword()
                            ),
                        ));
                    }
                }
                ast::Expr::Lambda(lambda) => {
                    if let Some(parameters) = &lambda.parameters {
                        self.visit_parameters(parameters);
                    }
                }
                expr if let Some((_, clauses)) = Output::from_expr(expr) => {
                    self.visit_expr(&clauses[0].iter);
                }
                _ => ast::visitor::walk_expr(self, expr),
            }
        }
        self.pending = roots;
        Ok(())
    }

    fn scan(&mut self, unlowered_yield: &mut bool) -> Result<(bool, Vec<ScopedExpression<'a>>)> {
        use ast::visitor::Visitor;
        let mut functions = Vec::new();
        if !self.directives.is_empty() {
            self.check_directive_reads()?;
        }
        let mut class_cell = false;
        while let Some((expr, bound)) = self.pending.pop() {
            self.bound = bound;
            match expr {
                // A yield still present in an expression has no suspension instruction.
                ast::Expr::Yield(_) | ast::Expr::YieldFrom(_) => *unlowered_yield = true,
                ast::Expr::Name(name) => {
                    if !self
                        .bound
                        .as_ref()
                        .is_some_and(|names| names.contains(name.id.as_str()))
                    {
                        class_cell |= matches!(name.id.as_str(), "super" | "__class__");
                        if name.ctx == ast::ExprContext::Load {
                            if self.bound.is_some() {
                                // Like nested functions, the comprehension body skips a class namespace.
                                self.comprehension_reads.insert(name.id.to_string());
                            } else {
                                self.reads.insert(name.id.to_string());
                            }
                        }
                    }
                    self.heap.code_name(name.id.as_str());
                }
                ast::Expr::Attribute(attr) => {
                    self.heap.code_name(attr.attr.as_str());
                }
                ast::Expr::Lambda(lambda) => {
                    functions
                        .try_reserve(1)
                        .map_err(|_| Error::new("MemoryError", ""))?;
                    functions.push((expr, self.bound.clone()));
                    if let Some(parameters) = &lambda.parameters {
                        self.visit_parameters(parameters);
                    }
                    continue;
                }
                ast::Expr::Generator(genexpr) => {
                    functions
                        .try_reserve(1)
                        .map_err(|_| Error::new("MemoryError", ""))?;
                    functions.push((expr, self.bound.clone()));
                    self.visit_expr(&genexpr.generators[0].iter);
                    continue;
                }
                expr if let Some(plan) = Plan::from_expr(expr) => {
                    let generators = plan.clauses;
                    self.visit_expr(&generators[0].iter);
                    let mut names = HashSet::new();
                    for clause in generators {
                        Program::collect_target_names(&clause.target, &mut names)?;
                    }
                    if !plan.suspends() {
                        self.eager_comprehension_targets
                            .insert(expr.range(), names.clone());
                    }
                    if let Some(enclosing) = &self.bound {
                        names.extend(enclosing.iter().cloned());
                    }
                    self.bound = Some(Rc::new(names));
                    plan.visit_body(|operand| self.visit_expr(operand));
                    continue;
                }
                ast::Expr::FString(formatted) => {
                    for part in formatted.value.iter() {
                        match part {
                            ast::FStringPartRef::Literal(lit) => {
                                self.format_literal(lit.range(), &lit.value)?;
                            }
                            ast::FStringPartRef::FString(part) => {
                                self.format_elements(&part.elements)?;
                            }
                        }
                    }
                }
                ast::Expr::TString(template) => self.template_strings(template)?,
                ast::Expr::NoneLiteral(_)
                | ast::Expr::BooleanLiteral(_)
                | ast::Expr::EllipsisLiteral(_) => {}
                _ if imre_ast::literal_tuple_element(expr) => {
                    let id = self.literal(expr)?;
                    self.constant(expr.range(), id)?;
                    continue;
                }
                _ => {}
            }
            ast::visitor::walk_expr(self, expr);
        }
        if self.exhausted {
            return Err(Error::new("MemoryError", ""));
        }
        Ok((class_cell, functions))
    }

    fn finish(self) -> Id {
        if self.roots.is_empty() {
            NONE
        } else {
            self.heap.alloc(Value::Tuple(self.roots))
        }
    }
}

type StringReplacements = BTreeMap<ruff_text_size::TextSize, Vec<u32>>;

/// Collect the original Python code points that Ruff represents as U+FFFD.
pub(super) fn string_replacements(source: &str, tokens: &ast::token::Tokens) -> StringReplacements {
    let mut entries = BTreeMap::new();
    for token in tokens {
        if !matches!(
            token.kind(),
            ast::token::TokenKind::String
                | ast::token::TokenKind::FStringMiddle
                | ast::token::TokenKind::TStringMiddle
        ) {
            continue;
        }
        let points: Vec<_> = match imre_ast::strings::cooked_content(token) {
            Some(range) => imre_ast::strings::replacements(&source[range]).collect(),
            None => source[token.range()]
                .matches(char::REPLACEMENT_CHARACTER)
                .map(|_| 0xfffd)
                .collect(),
        };
        if !points.is_empty() {
            entries.insert(token.start(), points);
        }
    }
    entries
}

/// Leave scalar strings on the UTF-8 path. Restore the spelling of every
/// replacement character when a literal includes at least one lone surrogate.
pub(super) fn original_points(
    text: &str,
    mut replacements: impl Iterator<Item = u32> + Clone,
) -> Result<Option<Vec<u32>>> {
    if !replacements.clone().any(|point| point != 0xfffd) {
        return Ok(None);
    }
    let mut points = Vec::new();
    points
        .try_reserve_exact(text.chars().count())
        .map_err(|_| crate::Error::allocation())?;
    for ch in text.chars() {
        points.push(if ch == char::REPLACEMENT_CHARACTER {
            replacements
                .next()
                .ok_or_else(|| unsupported("string escape spelling"))?
        } else {
            u32::from(ch)
        });
    }
    if replacements.next().is_some() {
        return Err(unsupported("string escape spelling"));
    }
    Ok(Some(points))
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

pub(super) const MAX_INDENT_LEVELS: usize = 99;

pub(super) struct IndentationDiagnostic {
    kind: &'static str,
    message: &'static str,
    column: i64,
    end_column: i64,
    row: i64,
    text: String,
}

impl IndentationDiagnostic {
    pub(super) fn from_issue(
        source: &str,
        issue: imre_ast::indentation::Error,
        file_input: bool,
    ) -> Self {
        let start = source[..issue.offset].rfind('\n').map_or(0, |i| i + 1);
        let mut text = source[start..]
            .split_inclusive('\n')
            .next()
            .unwrap_or_default()
            .to_owned();
        if file_input && !text.ends_with('\n') {
            text.push('\n');
        }
        let (column, end_column) = if issue.kind == "TabError" {
            (1, 0)
        } else {
            (text.chars().count() as i64, -1)
        };
        Self {
            kind: issue.kind,
            message: issue.message,
            column,
            end_column,
            row: issue.line as i64,
            text,
        }
    }

    /// Locate the first excess logical indent, unless parsing already failed earlier.
    pub(super) fn find<T>(
        parsed: &ruff_python_parser::Parsed<T>,
        source: &str,
        file_input: bool,
        first_statement_end: Option<ruff_text_size::TextSize>,
    ) -> Option<Self> {
        use ast::token::TokenKind::{Dedent, Indent};
        let mut depth = 0usize;
        for token in parsed.tokens() {
            match token.kind() {
                Indent if depth == MAX_INDENT_LEVELS => {
                    if first_statement_end.is_some_and(|end| token.start() >= end)
                        || parsed
                            .errors()
                            .first()
                            .is_some_and(|e| e.location.start() < token.start())
                    {
                        return None;
                    }
                    // An indent can span an explicit continuation. Report the physical
                    // line containing the first non-whitespace character.
                    let prefix = &source[..token.end().to_usize()];
                    let start = prefix.rfind('\n').map_or(0, |i| i + 1);
                    let row = 1 + prefix.bytes().filter(|&b| b == b'\n').count() as i64;
                    let mut text = source[start..].split_inclusive('\n').next()?.to_owned();
                    if file_input && !text.ends_with('\n') {
                        text.push('\n');
                    }
                    return Some(Self {
                        kind: "IndentationError",
                        message: "too many levels of indentation",
                        column: 1,
                        end_column: 0,
                        row,
                        text,
                    });
                }
                Indent => depth += 1,
                Dedent => depth = depth.saturating_sub(1),
                _ => {}
            }
        }
        None
    }

    pub(super) fn raise(self, ctx: &mut Context, filename: Id) -> Result<Id> {
        let message = self.message;
        if self.kind == "NotImplementedError" {
            return Err(Error::new(self.kind, message));
        }
        let msg = ctx.text(message);
        let row = ctx.int(self.row);
        let column = ctx.int(self.column);
        let end_column = ctx.int(self.end_column);
        let text = ctx.text(self.text);
        let details = ctx.alloc(Value::Tuple(vec![
            filename, row, column, text, row, end_column,
        ]));
        let args = [msg, details];
        let mut error = ctx.exception(self.kind, &args);
        error.message = format!("{message}{}", ctx.syntax_suffix(filename, row)?);
        ctx.retain_syntax_fields(&mut error, ctx.syntax_fields(&args))?;
        Err(error)
    }
}

/// Interactive input accepts one compound statement or one logical line of simple statements.
fn check_single_input(body: &[ast::Stmt], tokens: &ast::token::Tokens) -> Result<()> {
    use ast::token::TokenKind::{Dedent, Newline};

    let Some(first) = body.first() else {
        return Err(Error::new("SyntaxError", "invalid syntax"));
    };
    let mut trailing = tokens
        .iter()
        .skip_while(|token| token.start() < first.end());
    if ast::helpers::is_compound_statement(first) {
        // An indented suite ends in DEDENT. An inline suite needs a real closing newline.
        if body.len() != 1
            || !trailing.any(|token| {
                token.kind() == Dedent || (token.kind() == Newline && !token.range().is_empty())
            })
        {
            return Err(Error::new("SyntaxError", "invalid syntax"));
        }
    } else if let Some(last) = body.last()
        && trailing
            .take_while(|token| token.start() < last.start())
            .any(|token| token.kind() == Newline)
    {
        return Err(Error::new(
            "SyntaxError",
            "multiple statements found while compiling a single statement",
        ));
    }
    Ok(())
}

impl Program {
    /// A worker may replay one direct acquire on its one captured receiver.
    pub(super) fn captured_acquire(&self, freevar: &str) -> Option<CapturedAcquire> {
        let [(line, Instruction::Eval(expression))] = self.instructions.as_slice() else {
            return None;
        };
        let call = expression.as_call_expr()?;
        // Imported receiver spellings use ordinary attribute evaluation instead.
        let field =
            imre_ast::direct_method(call, &self.method_imports, self.private_names.as_deref())?;
        if field.attr.as_str() != "acquire"
            || field.value.as_name_expr()?.id.as_str() != freevar
            || !call.arguments.is_empty()
            || self.kind != FunctionKind::Plain
            || self.interactive
            || self.nested_global
            || self.class_attributes.is_some()
            || self.class_cell
            || !self.locals.is_empty()
            || !self.directives.is_empty()
            || self.doc.is_some()
            || self.string_roots != NONE
        {
            return None;
        }
        Some(CapturedAcquire {
            line: *line,
            expression: expression.clone(),
        })
    }

    /// Keep each code object's constants alive, including constants owned by unexecuted children.
    fn prepare_strings<'a>(
        &'a mut self,
        heap: &mut value::Heap,
        shared: &mut SharedConstants<'a>,
        spellings: &StringReplacements,
        line_at: &impl Fn(usize) -> u32,
    ) -> Result<(Id, &'a BTreeSet<String>)> {
        self.prepare_scope_strings(heap, shared, spellings, line_at, None)
    }

    fn prepare_scope_strings<'a>(
        &'a mut self,
        heap: &mut value::Heap,
        shared: &mut SharedConstants<'a>,
        spellings: &StringReplacements,
        line_at: &impl Fn(usize) -> u32,
        annotation_class: Option<&indexmap::IndexSet<String>>,
    ) -> Result<(Id, &'a BTreeSet<String>)> {
        use ast::visitor::Visitor;
        let class_parameters = self.class_parameter_header();
        let nested_scope_flags = self.nested_scope_flags();
        let mut nested_reads = BTreeSet::new();
        let stringized = self.string_annotations();
        let class_locals = self.class_attributes.as_ref().map(|_| &self.locals);
        let mut strings = CompiledStrings {
            reads: &mut self.reads,
            directives: &self.directives,
            heap,
            shared,
            spellings,
            constants: &mut self.literals,
            roots: Vec::new(),
            pending: Vec::new(),
            bound: None,
            comprehension_reads: BTreeSet::new(),
            eager_comprehension_targets: &mut self.eager_comprehension_targets,
            exhausted: false,
        };
        let mut enclosing_bindings = Vec::new();
        for (_, op) in &mut self.instructions {
            match op {
                Instruction::EnterComprehension { targets, .. } => {
                    let mut bound = targets.clone();
                    if let Some(parent) = &strings.bound {
                        bound.extend(parent.iter().cloned());
                    }
                    enclosing_bindings.push(strings.bound.replace(Rc::new(bound)));
                }
                Instruction::LeaveComprehension => {
                    strings.bound = enclosing_bindings.pop().expect("comprehension bindings");
                }
                Instruction::Eval(ast::Expr::StringLiteral(doc))
                    if self.scope_flags & code_flags::HAS_DOCSTRING != 0
                        && self.doc == Some(doc.range()) =>
                {
                    let text = imre_ast::strings::clean_doc(doc.value.to_str());
                    let id = strings.text(text.into())?;
                    strings.constant(doc.range(), id)?;
                }
                Instruction::Eval(e)
                | Instruction::Discard(e)
                | Instruction::Unless(e, _)
                | Instruction::Branch { value: Some(e), .. }
                | Instruction::MakeGenerator(e)
                | Instruction::LoadResult(e)
                | Instruction::Iter(Some(e), _)
                | Instruction::CollectItem { value: Some(e), .. }
                | Instruction::ExceptMatch(e, _)
                | Instruction::Await(Some(e))
                | Instruction::SaveOperand(Some(e)) => strings.visit_expr(e),
                Instruction::Attribute { name, .. } | Instruction::StoreNamed(name) => {
                    strings.heap.code_name(name.as_str());
                }
                Instruction::SaveCallMethod(field) => {
                    strings.heap.code_name(field.attr.as_str());
                    strings.visit_expr(&field.value);
                }
                Instruction::CallSaved { keywords, .. } => {
                    for name in keywords {
                        strings.heap.code_name(name.as_str());
                    }
                }
                Instruction::Match { subject, cases, .. } => {
                    strings.visit_expr(subject);
                    for case in cases {
                        strings.visit_pattern(&case.pattern);
                        for name in &case.captures {
                            strings.heap.code_name(name);
                        }
                        if let Some((_, guard)) = &case.guard {
                            strings.visit_expr(guard);
                        }
                    }
                }
                Instruction::BindAsyncItem(Some(target)) | Instruction::BindOperand(target) => {
                    strings.visit_expr(target);
                }
                Instruction::StoreAttribute(attr) => {
                    strings.heap.code_name(attr.as_str());
                }
                Instruction::AsyncWithMethods(Some(context)) => strings.visit_expr(context),
                Instruction::EnterAsyncWith {
                    target: Some(target),
                    ..
                } => strings.visit_expr(target),
                Instruction::EnterWith { item, .. } => {
                    strings.visit_expr(&item.context_expr);
                    if let Some(target) = &item.optional_vars {
                        strings.visit_expr(target);
                    }
                }
                Instruction::DeleteName(name)
                | Instruction::BindException(name)
                | Instruction::ClearException(name) => {
                    strings.heap.code_name(name);
                }
                Instruction::DeleteItem(item) => {
                    strings.visit_expr(&item.value);
                    strings.visit_expr(&item.slice);
                }
                Instruction::DeleteAttr(attr) => {
                    strings.visit_expr(&attr.value);
                    strings.heap.code_name(attr.attr.as_str());
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
                Instruction::ModuleDoc(range, text) => {
                    let id = strings.text(text.as_str().into())?;
                    strings.constant(*range, id)?;
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
                Instruction::From(module, _, bindings) => {
                    strings.heap.code_name(module);
                    for (name, alias) in bindings {
                        strings.heap.code_name(name);
                        strings.heap.code_name(alias);
                    }
                }
                Instruction::Annotations { entries, .. } => {
                    for (name, expr) in entries {
                        strings.annotation(name, expr, stringized)?;
                    }
                }
                Instruction::StringAnnotation(name, expr) => {
                    strings.annotation(name, expr, true)?
                }
                Instruction::ClassAnnotator(provider)
                | Instruction::ModuleAnnotations(Some(provider)) => {
                    nested_reads.extend(
                        strings
                            .annotation_code(provider, class_locals, line_at)?
                            .iter()
                            .cloned(),
                    );
                }
                Instruction::Define(definition) | Instruction::ReturnClass(definition) => {
                    let params = definition.kind.type_params();
                    let mut inherit_reads = |reads: &BTreeSet<String>| {
                        nested_reads.extend(
                            reads
                                .iter()
                                .filter(|name| {
                                    !params.is_some_and(|params| {
                                        params.iter().any(|p| p.name().as_str() == name.as_str())
                                    })
                                })
                                .cloned(),
                        );
                    };
                    if let Some(provider) = &mut definition.annotate {
                        inherit_reads(strings.annotation_code(provider, class_locals, line_at)?);
                    }
                    let child =
                        Rc::get_mut(&mut definition.body).expect("newly lowered child code");
                    strings.heap.code_name(&definition.binding);
                    if let DefinitionKind::ClassParameters(params) = &definition.kind {
                        child
                            .locals
                            .extend(params.iter().map(|p| p.name().to_string()));
                    }
                    let (roots, reads) =
                        child.prepare_strings(strings.heap, strings.shared, spellings, line_at)?;
                    inherit_reads(reads);
                    if roots != NONE {
                        strings.keep(roots)?;
                    }
                    for decorator in &definition.decorators {
                        strings.visit_expr(&decorator.expression);
                    }
                    match &definition.kind {
                        DefinitionKind::Function { parameters, .. } => {
                            strings.visit_parameters(parameters)
                        }
                        DefinitionKind::Class {
                            arguments: Some(args),
                            ..
                        } => strings.visit_arguments(args),
                        _ => {}
                    }
                }
                Instruction::ModuleAnnotations(None)
                | Instruction::MarkAnnotation(_)
                | Instruction::ClassStart
                | Instruction::ClassEnd
                | Instruction::CheckAnnotationFormat => {}
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
                Instruction::BindSent(targets) => {
                    for target in targets {
                        strings.visit_expr(target);
                    }
                }
                Instruction::ReturnSent
                | Instruction::YieldResult
                | Instruction::Await(None)
                | Instruction::SaveOperand(None)
                | Instruction::BuildSequence { .. }
                | Instruction::BuildSlice(_)
                | Instruction::StartDict
                | Instruction::StartSet(_)
                | Instruction::InsertDict { .. }
                | Instruction::InsertSet { .. }
                | Instruction::FinishDisplay
                | Instruction::Binary(_, _, _)
                | Instruction::NextComparison { .. }
                | Instruction::Branch { value: None, .. }
                | Instruction::Unary(_)
                | Instruction::JumpResult(_)
                | Instruction::Jump(_)
                | Instruction::Break(_)
                | Instruction::EnterFinally { .. }
                | Instruction::AsyncWithMethods(None)
                | Instruction::Iter(None, _)
                | Instruction::CollectItem { value: None, .. }
                | Instruction::AsyncNext
                | Instruction::EndAsyncFor
                | Instruction::BindAsyncItem(None)
                | Instruction::UnpackTarget { .. }
                | Instruction::StoreSubscript
                | Instruction::EnterAsyncWith { target: None, .. }
                | Instruction::ExitWith
                | Instruction::CallAsyncExit
                | Instruction::CallAsyncEnter
                | Instruction::AwaitWithResult(_)
                | Instruction::TestAsyncExit
                | Instruction::FinishAsyncExit
                | Instruction::EnterExcept { .. }
                | Instruction::StartFinally
                | Instruction::EndFinally
                | Instruction::Delegate => {}
            }
        }
        let (class_cell, functions) = strings.scan(&mut self.unlowered_yield)?;
        self.class_cell |= class_cell;
        self.expression_functions
            .try_reserve(functions.len())
            .map_err(|_| Error::new("MemoryError", ""))?;
        let mut comprehension_sites = HashMap::new();
        for (site, (expr, bound)) in functions.into_iter().enumerate() {
            let in_comprehension = bound.is_some();
            if let Some(names) = bound {
                comprehension_sites.insert(expr.range(), names);
            }
            let name = if matches!(expr, ast::Expr::Lambda(_)) {
                "<lambda>"
            } else {
                "<genexpr>"
            };
            let mut child = Self::new(self.filename.clone(), name, self.has_named_bindings);
            child.scope_flags = nested_scope_flags;
            child.strip_docstrings = self.strip_docstrings;
            child.source_lines = self.source_lines.clone();
            child.linked_body_line = self.linked_body_line;
            child.qualname =
                Self::qualified_name(&self.qualname, self.class_attributes.is_some(), name);
            child.method_imports = self.method_imports.clone();
            child.template_expressions = self.template_expressions.clone();
            child.private_names = self.private_names.clone();
            child.first_line = line_at(expr.start().to_usize());
            let params = match expr {
                ast::Expr::Lambda(lambda) => {
                    if !in_comprehension && self.class_attributes.is_some() {
                        child.method_class_cell = Some(imre_ast::class_cell::expression_needed(
                            &lambda.body,
                            lambda.parameters.as_deref(),
                            stringized,
                        ));
                    }
                    if self.has_named_bindings {
                        let mut named = NamedBindings::default();
                        named.visit_expr(&lambda.body);
                        named.declare(&mut child)?;
                    }
                    let params = lambda.parameters.clone().unwrap_or_default();
                    let line = child.first_line;
                    let end = match child.lower_suspension(*lambda.body.clone(), line, true)? {
                        Some(body) => Instruction::Return(Some(body)),
                        None => {
                            child.kind = FunctionKind::GeneratorLambda;
                            Instruction::ReturnSent
                        }
                    };
                    child.instructions.push((line, end));
                    params
                }
                ast::Expr::Generator(genexpr) => {
                    // Assignment expressions in the deferred frame still bind in the
                    // containing code unit. Nested lambdas have their own bindings.
                    if self.has_named_bindings {
                        let mut bindings = NamedBindings::default();
                        bindings.visit_expr(expr);
                        for (name, offset, _) in bindings.targets {
                            let kind = if self.name == "<module>"
                                || matches!(
                                    self.directives.get(&name),
                                    Some((ScopeDirective::Global, _))
                                ) {
                                ScopeDirective::Global
                            } else {
                                ScopeDirective::Nonlocal
                            };
                            child.directives.insert(name, (kind, offset));
                        }
                    }
                    child.lower_genexpr(genexpr, line_at)?
                }
                _ => unreachable!("anonymous function expression"),
            };
            child.scope_flags |= code_flags::function_flags(
                Some(&params),
                in_comprehension,
                !in_comprehension && self.class_attributes.is_some(),
            );
            child
                .locals
                .extend(params.iter().map(|p| p.name().to_string()));
            self.expression_functions.insert(
                expr.range(),
                (
                    site,
                    Definition {
                        binding: name.to_owned(),
                        decorators: Vec::new(),
                        body: Rc::new(child),
                        kind: DefinitionKind::Function {
                            parameters: params,
                            type_params: None,
                            captured_locks: None,
                        },
                        annotate: None,
                    },
                ),
            );
        }
        for (range, (_, definition)) in &mut self.expression_functions {
            let child =
                Rc::get_mut(&mut definition.body).expect("newly lowered expression function");
            let bound = comprehension_sites.get(range);
            let (roots, reads) =
                child.prepare_strings(strings.heap, strings.shared, spellings, line_at)?;
            let captured = reads
                .iter()
                .filter(|n| !bound.is_some_and(|b| b.contains(n.as_str())));
            if self.class_attributes.is_none() && !self.locals.contains("__class__") {
                self.class_cell |= captured.clone().any(|n| n == "__class__" || n == "super");
            }
            nested_reads.extend(captured.cloned());
            if roots != NONE {
                strings.keep(roots)?;
            }
        }
        nested_reads.append(&mut strings.comprehension_reads);
        self.string_roots = strings.finish();
        if class_parameters {
            // Captured parameters are cells. Only uncaptured (fast) parameters
            // bypass the enclosing class namespace when evaluating explicit bases.
            self.locals.retain(|name| !nested_reads.contains(name));
        }
        // An annotation's class-local name falls back to globals when the class
        // dictionary lacks it. Functions nested inside the annotation skip the class.
        if let Some(class) = annotation_class {
            self.class_binds_conditional_name = !self.conditional_fields.is_empty()
                && class.contains(CONDITIONAL)
                && !self.directives.contains_key(CONDITIONAL);
            self.reads
                .retain(|name| name == "__classdict__" || !class.contains(name));
        }
        if self.method_class_cell.is_none()
            && self.class_cell
            && self.reads.contains("super")
            && self.reads.contains(CONDITIONAL)
        {
            self.reads.insert("__class__".into());
        }
        // Even a nonlocal that is only assigned or deleted needs an enclosing cell.
        self.reads.extend(
            self.directives
                .iter()
                .filter(|(_, (kind, _))| *kind == ScopeDirective::Nonlocal)
                .map(|(name, _)| name.clone()),
        );
        if self.class_attributes.is_some() {
            self.reads.retain(|name| {
                !matches!(self.directives.get(name), Some((ScopeDirective::Global, _)))
            });
            // Methods skip the class namespace when resolving free variables.
            self.reads.retain(|name| !self.locals.contains(name));
            self.reads.extend(nested_reads);
        } else {
            self.reads.extend(nested_reads);
            self.reads.retain(|name| {
                !self.locals.contains(name)
                    && !matches!(self.directives.get(name), Some((ScopeDirective::Global, _)))
            });
        }
        // Implicit guard reads still use the cell when the expression names an explicit global.
        if !self.conditional_fields.is_empty() {
            self.reads.insert(CONDITIONAL.into());
        }
        Ok((self.string_roots, &self.reads))
    }

    fn declare(&mut self, kind: ScopeDirective, names: &[ast::Identifier]) -> Result<()> {
        for name in names {
            let spelling = name.as_str();
            let message = if self
                .directives
                .get(spelling)
                .is_some_and(|(old, _)| *old != kind)
            {
                Some(format!("name '{spelling}' is nonlocal and global"))
            } else if self.locals.contains(spelling) {
                Some(format!(
                    "name '{spelling}' is assigned to before {} declaration",
                    kind.keyword()
                ))
            } else {
                None
            };
            if let Some(message) = message {
                return Err(Error::new("SyntaxError", message));
            }
            self.directives
                .insert(name.to_string(), (kind, name.start()));
        }
        Ok(())
    }

    /// Validate nonlocals against lexical function scopes, including bindings later in the body.
    fn resolve_directives(
        &mut self,
        enclosing: &dyn Fn(&str) -> Option<EnclosingBinding>,
        module_globals: &mut HashSet<String>,
    ) -> Result<()> {
        self.locals
            .retain(|name| !self.directives.contains_key(name));
        for (name, (kind, _)) in &self.directives {
            if *kind == ScopeDirective::Global {
                module_globals.insert(name.clone());
            } else if let Some(binding) = enclosing(name) {
                if matches!(binding, EnclosingBinding::TypeParameter) {
                    return Err(Error::new(
                        "SyntaxError",
                        format!("nonlocal binding not allowed for type parameter '{name}'"),
                    ));
                }
                if matches!(binding, EnclosingBinding::ClassCell) {
                    self.class_nonlocal = true;
                    self.class_cell = true;
                }
            } else {
                let message = if self.name == "<module>" {
                    "nonlocal declaration not allowed at module level".to_owned()
                } else {
                    format!("no binding for nonlocal '{name}' found")
                };
                return Err(Error::new("SyntaxError", message));
            }
        }
        let function = self.name != "<module>" && self.class_attributes.is_none();
        let bound = |name: &str| {
            if self.class_attributes.is_some() && name == "__class__" {
                if !self.locals.contains(name)
                    && matches!(enclosing(name), Some(EnclosingBinding::TypeParameter))
                {
                    return Some(EnclosingBinding::TypeParameter);
                }
                Some(EnclosingBinding::ClassCell)
            } else if self.locals.contains(name)
                && (function || matches!(enclosing(name), Some(EnclosingBinding::TypeParameter)))
            {
                Some(EnclosingBinding::Local)
            } else if function
                && matches!(self.directives.get(name), Some((ScopeDirective::Global, _)))
            {
                None
            } else {
                // A method can bind the class's conditional annotation cell without an outer binding.
                enclosing(name).or_else(|| {
                    (self.class_attributes.is_some() && name == CONDITIONAL)
                        .then_some(EnclosingBinding::Local)
                })
            }
        };
        for (_, op) in &mut self.instructions {
            if let Instruction::Define(definition) | Instruction::ReturnClass(definition) = op {
                let child = Rc::get_mut(&mut definition.body).expect("newly lowered child code");
                if let Some(params) = definition.kind.type_params() {
                    child.resolve_directives(
                        &|name| {
                            if params.iter().any(|p| p.name().as_str() == name) {
                                Some(EnclosingBinding::TypeParameter)
                            } else {
                                bound(name)
                            }
                        },
                        module_globals,
                    )?;
                } else {
                    child.resolve_directives(&bound, module_globals)?;
                }
                if child.class_cell
                    && function
                    && matches!(enclosing("__class__"), Some(EnclosingBinding::ClassCell))
                {
                    self.class_cell = true;
                }
            }
        }
        Ok(())
    }

    fn class_parameter_header(&self) -> bool {
        matches!(
            self.instructions.first(),
            Some((_, Instruction::ReturnClass(_)))
        )
    }

    pub(super) fn expression_range(&self, range: ruff_text_size::TextRange) -> Option<[u32; 4]> {
        // Prelinked suspended suites are reparsed behind a synthetic function header.
        // Its columns need not match the original file.
        if self.linked_body_line.is_some() || self.source_lines.is_empty() {
            return None;
        }
        let point = |offset: ruff_text_size::TextSize| {
            let byte = offset.to_usize();
            let line = self.source_lines.partition_point(|&start| start <= byte);
            (line as u32, (byte - self.source_lines[line - 1]) as u32)
        };
        let (start_line, start_col) = point(range.start());
        let (end_line, end_col) = point(range.end());
        Some([start_line, end_line, start_col, end_col])
    }

    pub(super) fn expression_line(&self, offset: ruff_text_size::TextSize) -> Option<u32> {
        if self.source_lines.is_empty() {
            return None;
        }
        let line = self
            .source_lines
            .partition_point(|&start| start <= offset.to_usize()) as u32;
        Some(
            self.linked_body_line
                .map_or(line, |base| base + line.saturating_sub(2)),
        )
    }

    fn nested_program(&self, name: &str) -> Self {
        let mut child = Self::new(self.filename.clone(), name, self.has_named_bindings);
        child.source_lines = self.source_lines.clone();
        child.linked_body_line = self.linked_body_line;
        child.syntax_limit = self.syntax_limit;
        child.strip_docstrings = self.strip_docstrings;
        child.scope_flags = self.nested_scope_flags();
        child
    }

    fn nested_scope_flags(&self) -> u32 {
        let mut flags = self.scope_flags
            & (code_flags::FUTURE_ANNOTATIONS
                | code_flags::FUTURE_BARRY_AS_BDFL
                | code_flags::NESTED);
        // A class body keeps its enclosing function scope even when bound by a global declaration.
        if self.name != "<module>" && self.class_attributes.is_none() {
            flags |= code_flags::NESTED;
        }
        flags
    }

    fn new(filename: SourceFilename, name: &str, has_named_bindings: bool) -> Self {
        Self {
            filename,
            source_lines: Rc::default(),
            linked_body_line: None,
            syntax_limit: None,
            name: name.to_owned(),
            qualname: name.to_owned(),
            nested_global: false,
            class_attributes: None,
            class_cell: false,
            method_class_cell: None,
            class_nonlocal: false,
            needs_class_cell: false,
            needs_dict_cell: false,
            postponed_annotations: false,
            has_named_bindings,
            doc: None,
            strip_docstrings: false,
            literals: HashMap::new(),
            alternative_captures: HashMap::new(),
            string_roots: NONE,
            first_line: 1,
            kind: FunctionKind::Plain,
            scope_flags: 0,
            unlowered_yield: false,
            interactive: false,
            locals: indexmap::IndexSet::new(),
            directives: HashMap::new(),
            reads: BTreeSet::new(),
            method_imports: Rc::default(),
            private_names: None,
            template_expressions: Rc::default(),
            instructions: Vec::new(),
            deferred_annotations: Vec::new(),
            conditional_fields: Vec::new(),
            class_binds_conditional_name: false,
            expression_functions: HashMap::new(),
            eager_comprehension_targets: HashMap::new(),
        }
    }

    fn lower_awaited_call(&mut self, call: ast::ExprCall, plan: &OperandPlan) -> Result<()> {
        let line = plan.line;
        let method =
            imre_ast::direct_method(&call, &self.method_imports, self.private_names.as_deref());
        let position = value::ExpressionPosition::Source {
            offset: method.map_or_else(|| call.start(), |field| field.attr.start()),
            range: call.range(),
        };
        let callable = if method.is_some() {
            let ast::Expr::Attribute(field) = *call.func else {
                unreachable!()
            };
            if plan.suspends(&field.value) {
                self.lower_result(*field.value, plan)?;
                self.instructions.push((
                    line,
                    Instruction::Attribute {
                        name: field.attr,
                        method: true,
                    },
                ));
                Instruction::SaveOperand(None)
            } else {
                Instruction::SaveCallMethod(field)
            }
        } else {
            Instruction::SaveOperand(self.lower_operand(*call.func, plan)?)
        };
        self.instructions.push((line, callable));
        let positional = call.arguments.args.len();
        let keywords = call
            .arguments
            .keywords
            .iter()
            .map(|kw| kw.arg.clone().expect("fixed keyword"))
            .collect();
        let operands = call
            .arguments
            .args
            .into_iter()
            .chain(call.arguments.keywords.into_iter().map(|kw| kw.value));
        for expr in operands {
            self.save_operand(expr, plan)?;
        }
        self.instructions.push((
            line,
            Instruction::CallSaved {
                positional,
                keywords,
                position,
            },
        ));
        Ok(())
    }

    // A suspended expression supplies its result when execution resumes.
    fn lower_suspension(
        &mut self,
        expr: ast::Expr,
        line: u32,
        function: bool,
    ) -> Result<Option<ast::Expr>> {
        let plan = OperandPlan::new(&expr, self.kind, line, function);
        self.lower_operand(expr, &plan)
    }

    fn lower_operand(&mut self, expr: ast::Expr, plan: &OperandPlan) -> Result<Option<ast::Expr>> {
        stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || {
            self.emit_operand(expr, plan)
        })
    }

    fn emit_operand(&mut self, expr: ast::Expr, plan: &OperandPlan) -> Result<Option<ast::Expr>> {
        let line = plan.line;
        if !plan.suspends(&expr) {
            let op = match expr {
                ast::Expr::Yield(_) | ast::Expr::YieldFrom(_) if !plan.function => {
                    return Err(Error::new("SyntaxError", "'yield' outside function"));
                }
                ast::Expr::Yield(y) => Instruction::Yield(y.value.map(|value| *value)),
                ast::Expr::YieldFrom(y) => {
                    self.instructions
                        .push((line, Instruction::Iter(Some(*y.value), false)));
                    Instruction::Delegate
                }
                value => return Ok(Some(value)),
            };
            if self.kind != FunctionKind::AsyncGenerator {
                self.kind = FunctionKind::Generator;
            }
            self.instructions.push((line, op));
            return Ok(None);
        }
        if let ast::Expr::Generator(genexpr) = &expr {
            self.lower_result(genexpr.generators[0].iter.clone(), plan)?;
            self.instructions
                .push((line, Instruction::MakeGenerator(expr)));
            return Ok(None);
        }
        if let Some(comp) = Plan::from_expr(&expr) {
            self.lower_eager_comprehension(&comp, plan)?;
            return Ok(None);
        }
        match expr {
            ast::Expr::Await(a) => {
                let value = self.lower_operand(*a.value, plan)?;
                self.instructions.push((line, Instruction::Await(value)));
                self.instructions.push((line, Instruction::Delegate));
            }
            ast::Expr::Call(call) => self.lower_awaited_call(call, plan)?,
            ast::Expr::Tuple(tuple) => self.lower_sequence(tuple.elts, true, plan)?,
            ast::Expr::List(list) => self.lower_sequence(list.elts, false, plan)?,
            ast::Expr::Subscript(item) => {
                self.save_operand(*item.value, plan)?;
                self.save_operand(*item.slice, plan)?;
                self.instructions
                    .push((line, Instruction::Binary("get", item.range, false)));
            }
            ast::Expr::Slice(slice) => {
                let parts = [slice.lower, slice.upper, slice.step];
                let present = parts.each_ref().map(Option::is_some);
                for part in parts.into_iter().flatten() {
                    self.save_operand(*part, plan)?;
                }
                self.instructions
                    .push((line, Instruction::BuildSlice(present)));
            }
            ast::Expr::Named(named) => {
                let ast::Expr::Name(name) = *named.target else {
                    return Err(unsupported("assignment expression target"));
                };
                self.lower_result(*named.value, plan)?;
                self.instructions.push((
                    line,
                    Instruction::StoreNamed(ast::Identifier::new(name.id, name.range)),
                ));
            }
            ast::Expr::Dict(dict) => {
                self.instructions.push((line, Instruction::StartDict));
                for item in dict.items {
                    let unpack = item.key.is_none();
                    if let Some(key) = item.key {
                        self.save_operand(key, plan)?;
                    }
                    self.lower_result(item.value, plan)?;
                    self.instructions
                        .push((line, Instruction::InsertDict { unpack }));
                }
                self.instructions.push((line, Instruction::FinishDisplay));
            }
            ast::Expr::Set(set) => {
                let prefix = set_display_prefix(&set.elts);
                let mut items = set.elts.into_iter();
                for item in items.by_ref().take(prefix) {
                    self.save_operand(item, plan)?;
                }
                self.instructions
                    .push((line, Instruction::StartSet(prefix)));
                for item in items {
                    let unpack = item.is_starred_expr();
                    let value = match item {
                        ast::Expr::Starred(star) => *star.value,
                        value => value,
                    };
                    self.lower_result(value, plan)?;
                    self.instructions
                        .push((line, Instruction::InsertSet { unpack }));
                }
                self.instructions.push((line, Instruction::FinishDisplay));
            }
            ast::Expr::BinOp(binary) => {
                for operand in [*binary.left, *binary.right] {
                    self.save_operand(operand, plan)?;
                }
                self.instructions.push((
                    line,
                    Instruction::Binary(binary.op.as_str(), binary.range, false),
                ));
            }
            ast::Expr::Compare(c) => self.lower_comparison(c, plan, false)?,
            ast::Expr::BoolOp(b) => {
                let mut operands = b.values;
                let last = operands.pop().expect("boolean expression has operands");
                let mut done = BranchTargets::new();
                for expr in operands {
                    self.branch_on(
                        expr,
                        b.op == ast::BoolOp::Or,
                        BranchUse::Operand,
                        plan,
                        &mut done,
                    )?;
                }
                self.lower_result(last, plan)?;
                self.finish_branches(done);
            }
            ast::Expr::UnaryOp(u) => {
                self.lower_result(*u.operand, plan)?;
                self.instructions.push((line, Instruction::Unary(u.op)));
            }
            ast::Expr::Attribute(a) => {
                self.lower_result(*a.value, plan)?;
                self.instructions.push((
                    line,
                    Instruction::Attribute {
                        name: a.attr,
                        method: false,
                    },
                ));
            }
            ast::Expr::If(b) => {
                self.lower_choice(b, plan, Instruction::JumpResult(0), |program, arm, _| {
                    program.lower_result(arm, plan)
                })?;
            }
            _ => unreachable!("operand with a supported await"),
        }
        Ok(None)
    }

    /// Only the async pull and its await are protected by END_ASYNC_FOR.
    fn lower_async_next(&mut self, line: u32, target: Option<ast::Expr>) {
        self.instructions.push((line, Instruction::AsyncNext));
        self.instructions.push((line, Instruction::Delegate));
        self.instructions
            .push((line, Instruction::BindAsyncItem(target)));
    }

    fn finish_iterator_loop(&mut self, head: usize, line: u32) {
        let after = self.instructions.len();
        match &mut self.instructions[head].1 {
            Instruction::Next(_, end) => *end = after,
            Instruction::EnterExcept { handler, end } => {
                *handler = after;
                *end = after + 1;
                self.instructions.push((line, Instruction::EndAsyncFor));
            }
            _ => unreachable!("iterator loop guard"),
        }
    }

    fn lower_sequence(
        &mut self,
        items: Vec<ast::Expr>,
        tuple: bool,
        plan: &OperandPlan,
    ) -> Result<()> {
        let count = items.len();
        for item in items {
            self.save_operand(item, plan)?;
        }
        self.instructions
            .push((plan.line, Instruction::BuildSequence { tuple, count }));
        Ok(())
    }

    fn save_operand(&mut self, expr: ast::Expr, plan: &OperandPlan) -> Result<()> {
        let line = plan.line;
        let value = self.lower_operand(expr, plan)?;
        self.instructions
            .push((line, Instruction::SaveOperand(value)));
        Ok(())
    }

    fn lower_result(&mut self, expr: ast::Expr, plan: &OperandPlan) -> Result<()> {
        let line = plan.line;
        if let Some(expr) = self.lower_operand(expr, plan)? {
            self.instructions
                .push((line, Instruction::LoadResult(expr)));
        }
        Ok(())
    }

    /// Save the middle operand of a chain across comparison hooks and subsequent awaits.
    fn lower_comparison(
        &mut self,
        c: ast::ExprCompare,
        plan: &OperandPlan,
        condition: bool,
    ) -> Result<()> {
        let line = plan.line;
        let mut operands = c.operands.into_vec().into_iter();
        self.save_operand(operands.next().expect("left operand"), plan)?;
        let mut done = BranchTargets::new();
        let length = c.ops.len();
        for (i, (op, operand)) in c.ops.into_vec().into_iter().zip(operands).enumerate() {
            self.save_operand(operand, plan)?;
            let more = i + 1 < length;
            self.instructions
                .push((line, Instruction::Binary(op.as_str(), c.range, more)));
            if more {
                done.push(self.instructions.len());
                self.instructions
                    .push((line, Instruction::NextComparison { end: 0, condition }));
            }
        }
        self.finish_branches(done);
        Ok(())
    }

    /// A condition consumes a comparison chain's known truth. In an and/or value expression
    /// the chain yields its Python result and the enclosing operand tests it, even on an
    /// opposite branch that will discard the result. Nested boolean branches share exits.
    fn branch_on(
        &mut self,
        expr: ast::Expr,
        when: bool,
        usage: BranchUse,
        plan: &OperandPlan,
        exits: &mut BranchTargets,
    ) -> Result<()> {
        stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || {
            self.emit_branch(expr, when, usage, plan, exits)
        })
    }

    fn emit_branch(
        &mut self,
        expr: ast::Expr,
        when: bool,
        usage: BranchUse,
        plan: &OperandPlan,
        exits: &mut BranchTargets,
    ) -> Result<()> {
        let line = plan.line;
        let value = if plan.suspends(&expr) {
            match expr {
                ast::Expr::BoolOp(b) => {
                    let stop = b.op == ast::BoolOp::Or;
                    let mut operands = b.values;
                    let last = operands.pop().expect("boolean expression has operands");
                    let mut opposite = BranchTargets::new();
                    for expr in operands {
                        if stop == when {
                            self.branch_on(expr, when, usage, plan, exits)?;
                        } else {
                            self.branch_on(expr, stop, usage, plan, &mut opposite)?;
                        }
                    }
                    self.branch_on(last, when, usage, plan, exits)?;
                    self.finish_branches(opposite);
                    return Ok(());
                }
                ast::Expr::UnaryOp(u)
                    if usage == BranchUse::Condition && u.op == ast::UnaryOp::Not =>
                {
                    return self.branch_on(*u.operand, !when, usage, plan, exits);
                }
                ast::Expr::Compare(c) => {
                    self.lower_comparison(c, plan, usage == BranchUse::Condition)?;
                    None
                }
                ast::Expr::If(b) => {
                    self.lower_choice(
                        b,
                        plan,
                        Instruction::Jump(0),
                        |program, arm, fallthrough| {
                            if usage == BranchUse::Operand && !fallthrough {
                                // The body jumps over the else arm with a materialized value.
                                program.lower_result(arm, plan)?;
                                program.emit_truth_branch(None, when, usage, line, exits);
                                Ok(())
                            } else {
                                program.branch_on(arm, when, usage, plan, exits)
                            }
                        },
                    )?;
                    return Ok(());
                }
                value => self.lower_operand(value, plan)?,
            }
        } else {
            Some(expr)
        };
        self.emit_truth_branch(value, when, usage, line, exits);
        Ok(())
    }

    fn lower_choice(
        &mut self,
        choice: ast::ExprIf,
        plan: &OperandPlan,
        join: Instruction,
        mut arm: impl FnMut(&mut Self, ast::Expr, bool) -> Result<()>,
    ) -> Result<()> {
        let mut otherwise = BranchTargets::new();
        self.branch_on(
            *choice.test,
            false,
            BranchUse::Condition,
            plan,
            &mut otherwise,
        )?;
        arm(self, *choice.body, false)?;
        let done = self.instructions.len();
        self.instructions.push((plan.line, join));
        self.finish_branches(otherwise);
        arm(self, *choice.orelse, true)?;
        self.finish_branches([done]);
        Ok(())
    }

    fn emit_truth_branch(
        &mut self,
        value: Option<ast::Expr>,
        when: bool,
        usage: BranchUse,
        line: u32,
        exits: &mut BranchTargets,
    ) {
        exits.push(self.instructions.len());
        let branch = match value {
            Some(expr) if !when && usage == BranchUse::Condition => Instruction::Unless(expr, 0),
            value => Instruction::Branch {
                value,
                when,
                usage,
                end: 0,
            },
        };
        self.instructions.push((line, branch));
    }

    fn false_branches(
        &mut self,
        test: ast::Expr,
        line: u32,
        function: bool,
    ) -> Result<BranchTargets> {
        let plan = OperandPlan::new(&test, self.kind, line, function);
        let mut exits = BranchTargets::new();
        self.branch_on(test, false, BranchUse::Condition, &plan, &mut exits)?;
        Ok(exits)
    }

    fn finish_branches(&mut self, exits: impl IntoIterator<Item = usize>) {
        self.resolve_branches(exits, self.instructions.len());
    }

    fn resolve_branches(&mut self, exits: impl IntoIterator<Item = usize>, end: usize) {
        for index in exits {
            match &mut self.instructions[index].1 {
                Instruction::Branch { end: target, .. }
                | Instruction::NextComparison { end: target, .. }
                | Instruction::JumpResult(target)
                | Instruction::Jump(target)
                | Instruction::Unless(_, target) => *target = end,
                _ => unreachable!("branch destination"),
            }
        }
    }

    fn qualified_name(parent: &str, class: bool, name: &str) -> String {
        if parent == "<module>" {
            name.to_owned()
        } else if class {
            format!("{parent}.{name}")
        } else {
            format!("{parent}.<locals>.{name}")
        }
    }

    fn definition_spelling<'a>(&'a self, name: &'a ast::Identifier) -> &'a str {
        self.private_names
            .as_ref()
            .map_or(name.as_str(), |names| names.definition(name))
    }

    fn validate_type_params(&self, params: &ast::TypeParams, owner: &str) -> Result<()> {
        let mut names = HashSet::new();
        for param in params {
            let name = param.name();
            match name.as_str() {
                "__classdict__" => {
                    return Err(Error::new(
                        "SyntaxError",
                        "reserved name '__classdict__' cannot be used for type parameter",
                    ));
                }
                "__debug__" => {
                    return Err(Error::new("SyntaxError", "cannot assign to __debug__"));
                }
                _ => {}
            }
            if param.default().is_some()
                || matches!(param, ast::TypeParam::TypeVar(var) if var.bound.is_some())
            {
                return Err(unsupported(format!(
                    "{owner} type parameter bounds and defaults"
                )));
            }
            if !names.insert(name.as_str()) {
                return Err(Error::new(
                    "SyntaxError",
                    format!(
                        "duplicate type parameter '{}'",
                        self.definition_spelling(name)
                    ),
                ));
            }
        }
        Ok(())
    }

    fn definition_body(
        &self,
        name: &ast::Identifier,
        decorators: &[ast::Decorator],
        line: u32,
        lines: &[usize],
    ) -> Self {
        let original = self.definition_spelling(name);
        let mut child = self.nested_program(original);
        child.nested_global = self.name != "<module>"
            && matches!(
                self.directives.get(name.as_str()),
                Some((ScopeDirective::Global, _))
            );
        if !child.nested_global {
            child.qualname =
                Self::qualified_name(&self.qualname, self.class_attributes.is_some(), original);
        }
        child.method_imports = self.method_imports.clone();
        child.private_names = self.private_names.clone();
        child.template_expressions = self.template_expressions.clone();
        child.first_line = decorators.first().map_or(line, |d| {
            lines.partition_point(|n| *n <= d.start().to_usize()) as u32
        });
        child
    }

    fn string_annotations(&self) -> bool {
        self.private_names
            .as_ref()
            .is_some_and(|n| n.string_annotations)
    }

    fn lower_namespace(
        &mut self,
        mut body: ast::Suite,
        lines: &[usize],
        function: bool,
    ) -> Result<()> {
        let class = self.class_attributes.is_some();
        if !function && !class && imre_ast::annotations::has_variable_annotation(&body) {
            self.instructions
                .push((self.first_line, Instruction::ModuleAnnotations(None)));
        }
        if !self.interactive
            && let Some(ast::Stmt::Expr(first)) = body.first()
            && let ast::Expr::StringLiteral(doc) = first.value.as_ref()
        {
            if self.strip_docstrings {
                body.remove(0);
            } else if function || class {
                if function {
                    self.scope_flags |= code_flags::HAS_DOCSTRING;
                }
                self.doc = Some(doc.range());
            } else {
                let line = lines.partition_point(|n| *n <= first.start().to_usize()) as u32;
                let text = imre_ast::strings::clean_doc(doc.value.to_str());
                self.instructions
                    .push((line, Instruction::ModuleDoc(doc.range(), text)));
                body.remove(0);
            }
        }
        self.postponed_annotations = class
            && self.string_annotations()
            && imre_ast::annotations::has_variable_annotation(&body);
        self.needs_class_cell =
            class && imre_ast::class_cell::needed(&body, self.string_annotations());
        self.needs_dict_cell =
            class && imre_ast::class_cell::dict_needed(&body, self.string_annotations());
        self.lower_block(body, lines, function, None, class)?;
        if !self.deferred_annotations.is_empty() {
            let entries = std::mem::take(&mut self.deferred_annotations);
            let fields = std::mem::take(&mut self.conditional_fields);
            let provider = self.annotation_provider(
                entries,
                class.then_some(&self.qualname),
                self.first_line,
                fields,
            );
            if class {
                self.instructions
                    .push((self.first_line, Instruction::ClassAnnotator(provider)));
            } else {
                self.instructions[0].1 = Instruction::ModuleAnnotations(Some(provider));
            }
        }
        Ok(())
    }

    fn lower(
        &mut self,
        body: ast::Suite,
        lines: &[usize],
        function: bool,
        active: Option<&mut LoopJumps>,
    ) -> Result<()> {
        self.lower_block(body, lines, function, active, false)
    }

    fn lower_block(
        &mut self,
        mut body: ast::Suite,
        lines: &[usize],
        function: bool,
        active: Option<&mut LoopJumps>,
        class_top_level: bool,
    ) -> Result<()> {
        stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || {
            if let Some(limit) = self.syntax_limit {
                // Keep enclosing compound statements. A selected wildcard points at its alias.
                body.truncate(body.partition_point(|stmt| {
                    stmt.start() < limit.start()
                        && !(matches!(stmt, ast::Stmt::ImportFrom(_))
                            && stmt.range().contains_range(limit))
                }));
            }
            use ast::visitor::Visitor;
            let mut named = NamedBindings::default();
            if self.has_named_bindings {
                for stmt in &body {
                    named.visit_stmt(stmt);
                }
            }
            self.lower_suite(body, lines, function, active, class_top_level)?;
            named.declare(self)
        })
    }

    fn lower_suite(
        &mut self,
        body: ast::Suite,
        lines: &[usize],
        function: bool,
        mut active: Option<&mut LoopJumps>,
        class_top_level: bool,
    ) -> Result<()> {
        use ast::Stmt;
        for mut stmt in body {
            let line = lines.partition_point(|n| *n <= stmt.start().to_usize()) as u32;
            // A generic class's header selectively mangles its private type parameters.
            // Keep the shared spelling map for both the parameter frame and its body.
            let class_names = if self.private_names.is_none()
                && matches!(&stmt, Stmt::ClassDef(class) if class.type_params.is_some())
            {
                Some(Rc::new(imre_ast::lower_in(
                    std::slice::from_mut(&mut stmt),
                    "",
                    false,
                )))
            } else {
                None
            };
            let op = match stmt {
                Stmt::Pass(_) => continue,
                Stmt::Global(s) => {
                    self.declare(ScopeDirective::Global, &s.names)?;
                    continue;
                }
                Stmt::Nonlocal(s) => {
                    self.declare(ScopeDirective::Nonlocal, &s.names)?;
                    continue;
                }
                Stmt::Expr(s) => match self.lower_suspension(*s.value, line, function)? {
                    Some(expr) => Instruction::Eval(expr),
                    None => continue,
                },
                Stmt::Assign(s) => {
                    for target in &s.targets {
                        self.declare_target(target)?;
                    }
                    match self.lower_suspension(*s.value, line, function)? {
                        Some(value) => Instruction::Assign(s.targets, value),
                        None => Instruction::BindSent(s.targets),
                    }
                }
                Stmt::AnnAssign(s) => {
                    let name = s.target.as_name_expr().map(|name| name.id.to_string());
                    if ((s.simple || s.value.is_some()) && name.as_deref() == Some("__debug__"))
                        || s.target
                            .as_attribute_expr()
                            .is_some_and(|a| a.attr.as_str() == "__debug__")
                    {
                        return Err(Error::new("SyntaxError", "cannot assign to __debug__"));
                    }
                    if let Some(name) = &name {
                        if s.simple
                            && let Some((kind, _)) = self.directives.get(name)
                            && (function
                                || self.class_attributes.is_some()
                                || *kind != ScopeDirective::Global)
                        {
                            return Err(Error::new(
                                "SyntaxError",
                                format!("annotated name '{name}' can't be {}", kind.keyword()),
                            ));
                        }
                        if s.simple || s.value.is_some() {
                            self.locals.insert(name.clone());
                        }
                    }
                    annotations::validate(&s.annotation, name.is_some())?;
                    if let Some(value) = s.value {
                        let op = match self.lower_suspension(*value, line, function)? {
                            Some(value) => Instruction::Assign(vec![*s.target], value),
                            None => Instruction::BindSent(vec![*s.target]),
                        };
                        self.instructions.push((line, op));
                    } else {
                        match *s.target {
                            ast::Expr::Attribute(attr) => {
                                self.discard_annotation_operand(*attr.value, line, function)?;
                            }
                            ast::Expr::Subscript(item) => {
                                self.discard_annotation_operand(*item.value, line, function)?;
                                self.discard_annotation_slice(*item.slice, line, function)?;
                            }
                            ast::Expr::Name(_) => {}
                            _ => return Err(unsupported("annotated assignment target")),
                        }
                    }
                    // Function-local annotations never evaluate the annotation expression.
                    if function || !s.simple {
                        continue;
                    }
                    let name = name.expect("simple annotated name");
                    if self.string_annotations() {
                        Instruction::StringAnnotation(
                            name,
                            annotations::string_literal(&s.annotation),
                        )
                    } else {
                        let index = self.deferred_annotations.len();
                        self.deferred_annotations.push((name, *s.annotation));
                        if class_top_level {
                            continue;
                        }
                        if self.class_attributes.is_some() {
                            self.conditional_fields.push(index);
                            Instruction::MarkAnnotation(self.conditional_fields.len() - 1)
                        } else {
                            Instruction::MarkAnnotation(index)
                        }
                    }
                }
                Stmt::Delete(s) => {
                    for target in s.targets {
                        match target {
                            ast::Expr::Name(name) => {
                                let name = name.id.to_string();
                                self.locals.insert(name.clone());
                                self.instructions
                                    .push((line, Instruction::DeleteName(name)));
                            }
                            ast::Expr::Subscript(item) => {
                                self.instructions
                                    .push((line, Instruction::DeleteItem(item)));
                            }
                            ast::Expr::Attribute(attr) => {
                                self.instructions
                                    .push((line, Instruction::DeleteAttr(attr)));
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
                    let Some(value) = s.value else {
                        self.instructions.push((line, Instruction::Return(None)));
                        continue;
                    };
                    match self.lower_suspension(*value, line, function)? {
                        Some(value) => Instruction::Return(Some(value)),
                        None => Instruction::ReturnSent,
                    }
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
                Stmt::ImportFrom(s) if !s.is_lazy => {
                    let module = s.module.as_ref().map_or("", |module| module.as_str());
                    let mut names = vec![];
                    for alias in &s.names {
                        let local = alias.asname.as_ref().unwrap_or(&alias.name).to_string();
                        if alias.name.as_str() == "*" {
                            if function || self.class_attributes.is_some() {
                                return Err(Error::new(
                                    "SyntaxError",
                                    "import * only allowed at module level",
                                ));
                            }
                        } else {
                            self.locals.insert(local.clone());
                        }
                        names.push((alias.name.to_string(), local));
                    }
                    Instruction::From(module.to_owned(), s.level, names)
                }
                Stmt::TypeAlias(s)
                    if self.class_attributes.is_none()
                        && (!function || s.type_params.is_some()) =>
                {
                    if s.type_params.as_deref().is_some_and(|params| {
                        params.iter().any(|param| {
                            param.default().is_some()
                                || matches!(param, ast::TypeParam::TypeVar(var) if var.bound.is_some())
                        })
                    }) {
                        return Err(unsupported("type alias parameter bounds and defaults"));
                    }
                    let ast::Expr::Name(name) = *s.name else {
                        return Err(unsupported("type alias target"));
                    };
                    let name = ast::Identifier::new(name.id, name.range);
                    let mut child = self.definition_body(&name, &[], line, lines);
                    child.locals.insert(".format".to_owned());
                    child
                        .instructions
                        .push((line, Instruction::CheckAnnotationFormat));
                    let rhs_line =
                        lines.partition_point(|n| *n <= s.value.start().to_usize()) as u32;
                    child
                        .instructions
                        .push((rhs_line, Instruction::Return(Some(*s.value))));
                    self.locals.insert(name.to_string());
                    Instruction::Define(Definition {
                        binding: name.to_string(),
                        decorators: Vec::new(),
                        body: Rc::new(child),
                        kind: DefinitionKind::TypeAlias(s.type_params),
                        annotate: None,
                    })
                }
                Stmt::FunctionDef(mut s) => {
                    let mut child = self.definition_body(&s.name, &s.decorator_list, line, lines);
                    if self.class_attributes.is_some() && s.type_params.is_none() {
                        child.method_class_cell = Some(imre_ast::class_cell::function_needed(
                            &s.body,
                            Some(&s.parameters),
                            self.string_annotations(),
                        ));
                    }
                    if let Some(params) = &s.type_params {
                        child.validate_type_params(params, "function")?;
                    }
                    child.scope_flags |= code_flags::function_flags(
                        Some(&s.parameters),
                        s.type_params.is_some(),
                        self.class_attributes.is_some() && s.type_params.is_none(),
                    );
                    let captured_locks =
                        if s.is_async || s.returns.is_some() || s.type_params.is_some() {
                            None
                        } else {
                            imre_ast::captured_thread_locks(&s.body, &s.parameters)
                                .map(|names| names.into_iter().map(str::to_owned).collect())
                        };
                    let annotate = self.annotation_program(&mut s, &child, line)?;
                    if s.is_async {
                        child.kind = if imre_ast::code_flags::is_generator(&s.body) {
                            FunctionKind::AsyncGenerator
                        } else {
                            FunctionKind::Coroutine
                        };
                    }
                    child.lower_namespace(s.body, lines, true)?;
                    for param in s.parameters.iter() {
                        let name = param.name().as_str();
                        if let Some((kind, _)) = child.directives.get(name) {
                            return Err(Error::new(
                                "SyntaxError",
                                format!("name '{name}' is parameter and {}", kind.keyword()),
                            ));
                        }
                        child.locals.insert(name.to_owned());
                    }
                    self.locals.insert(s.name.to_string());
                    Instruction::Define(Definition {
                        binding: s.name.to_string(),
                        decorators: s.decorator_list.into_iter().collect(),
                        body: Rc::new(child),
                        kind: DefinitionKind::Function {
                            parameters: s.parameters,
                            type_params: s.type_params,
                            captured_locks,
                        },
                        annotate,
                    })
                }
                Stmt::ClassDef(mut s) => {
                    let mut child = self.definition_body(&s.name, &s.decorator_list, line, lines);
                    if let Some(names) = &class_names {
                        child.private_names = Some(Rc::clone(names));
                    }
                    if s.type_params.is_some()
                        && let Some((_, reason)) = s
                            .arguments
                            .as_deref()
                            .and_then(imre_ast::generic_class_arguments_error)
                    {
                        return Err(Error::new("SyntaxError", reason));
                    }
                    let spellings = child.private_names.get_or_insert_with(|| {
                        Rc::new(imre_ast::lower_in(&mut s.body, &child.name, false))
                    });
                    // The implicit type-parameter function encloses the class body.
                    if s.type_params.is_some() {
                        child.scope_flags |= code_flags::NESTED;
                    }
                    child.class_attributes = Some(imre_ast::class_attributes::static_attributes(
                        &s.body, spellings,
                    ));
                    // Method-only classes can use the lexical reads collected for their methods.
                    let needs_enclosing_frame = s.type_params.is_some()
                        || !s
                            .body
                            .iter()
                            .all(|stmt| matches!(stmt, Stmt::Pass(_) | Stmt::FunctionDef(_)));
                    child.instructions.push((line, Instruction::ClassStart));
                    child.lower_namespace(s.body, lines, false)?;
                    child.instructions.push((line, Instruction::ClassEnd));
                    self.locals.insert(s.name.to_string());
                    let mut definition = Definition {
                        binding: s.name.to_string(),
                        decorators: s.decorator_list.into_iter().collect(),
                        body: Rc::new(child),
                        kind: DefinitionKind::Class {
                            arguments: s.arguments,
                            parameterized: s.type_params.is_some(),
                            needs_enclosing_frame,
                        },
                        annotate: None,
                    };
                    if let Some(params) = s.type_params {
                        definition.body.validate_type_params(&params, "class")?;
                        // Bases and keywords run in the implicit type-parameter function.
                        // Decorators are evaluated and applied in the enclosing scope.
                        let mut header = self.definition_body(&s.name, &[], line, lines);
                        header.name = format!("<generic parameters of {}>", definition.body.name);
                        header.qualname = Self::qualified_name(
                            &self.qualname,
                            self.class_attributes.is_some(),
                            &header.name,
                        );
                        header.nested_global = false;
                        header.first_line = definition.body.first_line;
                        header.private_names = definition.body.private_names.clone();
                        let decorators = std::mem::take(&mut definition.decorators);
                        header
                            .instructions
                            .push((line, Instruction::ReturnClass(definition)));
                        definition = Definition {
                            binding: s.name.to_string(),
                            decorators,
                            body: Rc::new(header),
                            kind: DefinitionKind::ClassParameters(params),
                            annotate: None,
                        };
                    }
                    Instruction::Define(definition)
                }
                Stmt::Break(_) => {
                    let current = active
                        .as_deref_mut()
                        .ok_or_else(|| Error::new("SyntaxError", "'break' outside loop"))?;
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
                    let mut async_target = None;
                    let (body, orelse, guard) = match stmt {
                        Stmt::For(s) => {
                            self.declare_target(&s.target)?;
                            let guard = if s.is_async {
                                if !self.kind.is_async() {
                                    return Err(Error::new(
                                        "SyntaxError",
                                        "'async for' outside async function",
                                    ));
                                }
                                let iterable = self.lower_suspension(*s.iter, line, function)?;
                                self.instructions
                                    .push((line, Instruction::Iter(iterable, true)));
                                async_target = Some(*s.target);
                                Instruction::EnterExcept { handler: 0, end: 0 }
                            } else {
                                self.instructions
                                    .push((line, Instruction::Iter(Some(*s.iter), false)));
                                Instruction::Next(Some(*s.target), 0)
                            };
                            (s.body, s.orelse, guard)
                        }
                        Stmt::While(s) => (s.body, s.orelse, Instruction::Unless(*s.test, 0)),
                        _ => unreachable!("loop statement"),
                    };
                    let head = self.instructions.len();
                    let mut current = LoopJumps {
                        head,
                        has_iterator: async_target.is_some()
                            || matches!(guard, Instruction::Next(..)),
                        breaks: Vec::new(),
                    };
                    let skip = if let Instruction::Unless(test, _) = guard {
                        self.false_branches(test, line, function)?
                    } else {
                        self.instructions.push((line, guard));
                        BranchTargets::new()
                    };
                    if let Some(target) = async_target {
                        self.lower_async_next(line, Some(target));
                    }
                    self.lower(body, lines, function, Some(&mut current))?;
                    self.instructions.push((line, Instruction::Jump(head)));
                    if !skip.is_empty() {
                        self.finish_branches(skip);
                    } else {
                        self.finish_iterator_loop(head, line);
                    }
                    self.lower(orelse, lines, function, active.as_deref_mut())?;
                    let end = self.instructions.len();
                    for jump in current.breaks {
                        self.instructions[jump].1 = if current.has_iterator {
                            Instruction::Break(end)
                        } else {
                            Instruction::Jump(end)
                        };
                    }
                    continue;
                }
                Stmt::With(s) => {
                    if s.is_async && !self.kind.is_async() {
                        return Err(Error::new(
                            "SyntaxError",
                            "'async with' outside async function",
                        ));
                    }
                    let mut entered = Vec::new();
                    for item in s.items {
                        if let Some(target) = &item.optional_vars {
                            self.declare_target(target)?;
                        }
                        let entry = if s.is_async {
                            let context =
                                self.lower_suspension(item.context_expr, line, function)?;
                            self.instructions
                                .push((line, Instruction::AsyncWithMethods(context)));
                            self.instructions.push((line, Instruction::CallAsyncEnter));
                            self.instructions
                                .push((line, Instruction::AwaitWithResult("__aenter__")));
                            self.instructions.push((line, Instruction::Delegate));
                            Instruction::EnterAsyncWith {
                                target: item.optional_vars,
                                handler: 0,
                                end: 0,
                            }
                        } else {
                            Instruction::EnterWith { item, handler: 0 }
                        };
                        entered.push(self.instructions.len());
                        self.instructions.push((line, entry));
                    }
                    self.lower(s.body, lines, function, active.as_deref_mut())?;
                    for i in entered.into_iter().rev() {
                        self.instructions.push((line, Instruction::StartFinally));
                        let cleanup = self.instructions.len();
                        if s.is_async {
                            self.instructions.push((line, Instruction::CallAsyncExit));
                            self.instructions
                                .push((line, Instruction::AwaitWithResult("__aexit__")));
                            self.instructions.push((line, Instruction::Delegate));
                            self.instructions.push((line, Instruction::TestAsyncExit));
                            self.instructions.push((line, Instruction::FinishAsyncExit));
                        } else {
                            self.instructions.push((line, Instruction::ExitWith));
                        }
                        self.instructions.push((line, Instruction::EndFinally));
                        let after = self.instructions.len();
                        match &mut self.instructions[i].1 {
                            Instruction::EnterWith { handler, .. } => *handler = cleanup,
                            Instruction::EnterAsyncWith { handler, end, .. } => {
                                *handler = cleanup;
                                *end = after;
                            }
                            _ => unreachable!("context manager entry"),
                        }
                    }
                    continue;
                }
                Stmt::Try(s)
                    if s.is_star
                        && !function
                        && self.class_attributes.is_none()
                        && !self.string_annotations()
                        && imre_ast::annotations::has_variable_annotation(
                            std::slice::from_ref(&stmt),
                        ) =>
                {
                    return Err(unsupported("deferred module annotations in try suites"));
                }
                Stmt::Try(s) if !s.is_star && !s.handlers.is_empty() && s.finalbody.is_empty() => {
                    let entry = self.instructions.len();
                    self.instructions
                        .push((line, Instruction::EnterExcept { handler: 0, end: 0 }));
                    self.lower(s.body, lines, function, active.as_deref_mut())?;
                    // On fallthrough, leave the protected body before running the else suite.
                    self.instructions.push((line, Instruction::StartFinally));
                    let handler = self.instructions.len();
                    let before_handlers = self.deferred_annotations.len();
                    let mut handled = Vec::new();
                    let count = s.handlers.len();
                    for (i, ast::ExceptHandler::ExceptHandler(h)) in
                        s.handlers.into_iter().enumerate()
                    {
                        if h.type_.is_none() && i + 1 < count {
                            return Err(Error::new(
                                "SyntaxError",
                                "default 'except:' must be last",
                            ));
                        }
                        let handler_line =
                            lines.partition_point(|n| *n <= h.start().to_usize()) as u32;
                        let check = h.type_.map(|class| {
                            let i = self.instructions.len();
                            self.instructions
                                .push((handler_line, Instruction::ExceptMatch(*class, 0)));
                            i
                        });
                        let cleanup = h.name.map(|name| {
                            let name = name.to_string();
                            self.locals.insert(name.clone());
                            self.instructions
                                .push((handler_line, Instruction::BindException(name.clone())));
                            let entry = self.instructions.len();
                            self.instructions.push((handler_line, Instruction::Jump(0)));
                            (entry, name)
                        });
                        self.lower(h.body, lines, function, active.as_deref_mut())?;
                        if let Some((entry, name)) = cleanup {
                            // The implicit finally clears the target on fallthrough, unwind and generator close.
                            self.instructions
                                .push((handler_line, Instruction::StartFinally));
                            let handler = self.instructions.len();
                            self.instructions
                                .push((handler_line, Instruction::ClearException(name)));
                            self.instructions
                                .push((handler_line, Instruction::EndFinally));
                            let end = self.instructions.len();
                            self.instructions[entry].1 = Instruction::EnterFinally {
                                handler,
                                end,
                                annotation_count: 0,
                            };
                        }
                        handled.push(self.instructions.len());
                        self.instructions.push((handler_line, Instruction::Jump(0)));
                        if let Some(i) = check {
                            let next = self.instructions.len();
                            let Instruction::ExceptMatch(_, next_handler) =
                                &mut self.instructions[i].1
                            else {
                                unreachable!()
                            };
                            *next_handler = next;
                        }
                    }
                    self.instructions.push((line, Instruction::EndFinally));
                    let end = self.instructions.len();
                    self.instructions[entry].1 = Instruction::EnterExcept { handler, end };
                    let before_else = self.deferred_annotations.len();
                    self.lower(s.orelse, lines, function, active.as_deref_mut())?;
                    // The interpreter lays out handlers first, but the annotation provider
                    // visits the else suite first, even if both paths ran in different iterations.
                    let handler_count = before_else - before_handlers;
                    let else_count = self.deferred_annotations.len() - before_else;
                    if handler_count != 0 && else_count != 0 {
                        self.deferred_annotations[before_handlers..].rotate_left(handler_count);
                        for (i, (_, op)) in self.instructions[handler..].iter_mut().enumerate() {
                            if let Instruction::MarkAnnotation(index) = op {
                                if i < end - handler {
                                    *index += else_count;
                                } else {
                                    *index -= handler_count;
                                }
                            }
                        }
                    }
                    let done = self.instructions.len();
                    for i in handled {
                        self.instructions[i].1 = Instruction::Jump(done);
                    }
                    continue;
                }
                Stmt::Try(mut s) if !s.is_star && !s.finalbody.is_empty() => {
                    let finalbody = std::mem::take(&mut s.finalbody);
                    let enter = self.instructions.len();
                    self.instructions.push((line, Instruction::Jump(0)));
                    // The finally surrounds the handlers and else suite as well as the try body.
                    let protected = if s.handlers.is_empty() {
                        s.body
                    } else {
                        std::iter::once(Stmt::Try(s)).collect()
                    };
                    self.lower(protected, lines, function, active.as_deref_mut())?;
                    self.instructions.push((line, Instruction::StartFinally));
                    let handler = self.instructions.len();
                    let first = self.deferred_annotations.len();
                    self.lower(finalbody, lines, function, active.as_deref_mut())?;
                    let annotation_count = self.deferred_annotations.len() - first;
                    if annotation_count != 0 {
                        // A break or continue crossing this finally has its own inlined
                        // annotation sites at the jump, before the fallthrough copy.
                        if active.is_some()
                            && self.instructions[enter + 1..handler].iter().any(|(_, op)| {
                                matches!(op, Instruction::Jump(target) if *target <= enter)
                            })
                        {
                            return Err(unsupported("loop exit across an annotated finally"));
                        }
                        // CPython emits another copy of finally for exception unwinding.
                        // Share its executable suite, but keep the two deferred site ranges.
                        let exceptional = self.deferred_annotations.len();
                        self.deferred_annotations.extend_from_within(first..);
                        if self.class_attributes.is_some() {
                            self.conditional_fields
                                .extend(exceptional..exceptional + annotation_count);
                        }
                    }
                    self.instructions.push((line, Instruction::EndFinally));
                    let end = self.instructions.len();
                    self.instructions[enter].1 = Instruction::EnterFinally {
                        handler,
                        end,
                        annotation_count,
                    };
                    continue;
                }
                Stmt::Match(s) => {
                    let dispatch = self.instructions.len();
                    self.instructions.push((line, Instruction::Jump(0)));
                    let mut cases = Vec::new();
                    let mut ends = Vec::new();
                    let count = s.cases.len();
                    let line_offset = |offset: ruff_text_size::TextSize| {
                        lines.partition_point(|n| *n <= offset.to_usize()) as u32 - line
                    };
                    for case in s.cases {
                        let case_line = line_offset(case.pattern.start());
                        let mut captures = indexmap::IndexMap::new();
                        self.validate_pattern(
                            &case.pattern,
                            &mut captures,
                            case.guard.is_some() || cases.len() + 1 == count,
                        )?;
                        cases.push(SourceCase {
                            pattern: case.pattern,
                            captures: captures.into_values().collect(),
                            line_offset: case_line,
                            guard: case.guard.map(|expr| (line_offset(expr.start()), expr)),
                            body: self.instructions.len(),
                        });
                        self.lower(case.body, lines, function, active.as_deref_mut())?;
                        ends.push(self.instructions.len());
                        self.instructions.push((line, Instruction::Jump(0)));
                    }
                    let end = self.instructions.len();
                    self.instructions[dispatch].1 = Instruction::Match {
                        subject: *s.subject,
                        cases,
                        end,
                    };
                    for i in ends {
                        self.instructions[i].1 = Instruction::Jump(end);
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

    /// Check the executable subset and record captures in binding order.
    fn validate_pattern(
        &mut self,
        pattern: &ast::Pattern,
        captures: &mut indexmap::IndexMap<String, String>,
        allow_irrefutable: bool,
    ) -> Result<()> {
        let name = match pattern {
            ast::Pattern::MatchSingleton(_) | ast::Pattern::MatchValue(_) => None,
            ast::Pattern::MatchClass(p)
                if p.arguments.patterns.is_empty() && p.arguments.keywords.is_empty() =>
            {
                None
            }
            ast::Pattern::MatchSequence(p) => {
                if p.patterns
                    .iter()
                    .filter(|p| matches!(p, ast::Pattern::MatchStar(_)))
                    .count()
                    > 1
                {
                    return Err(Error::source_syntax(
                        "multiple starred names in sequence pattern",
                        p.range(),
                    ));
                }
                for child in &p.patterns {
                    self.validate_pattern(child, captures, true)?;
                }
                None
            }
            ast::Pattern::MatchOr(p) => {
                let mut first = indexmap::IndexMap::new();
                for (i, arm) in p.patterns.iter().enumerate() {
                    let mut names = indexmap::IndexMap::new();
                    self.validate_pattern(
                        arm,
                        &mut names,
                        allow_irrefutable && i + 1 == p.patterns.len(),
                    )?;
                    let order = if i == 0 {
                        first = names;
                        (0..first.len()).collect()
                    } else {
                        let order: Option<Vec<_>> =
                            first.keys().map(|name| names.get_index_of(name)).collect();
                        order
                            .filter(|order| order.len() == names.len())
                            .ok_or_else(|| {
                                Error::source_syntax(
                                    "alternative patterns bind different names",
                                    p.range(),
                                )
                            })?
                    };
                    if !order.is_empty() {
                        self.alternative_captures.insert(arm.range(), order);
                    }
                }
                for (name, binding) in first {
                    if captures.contains_key(&name) {
                        return Err(Error::source_syntax(
                            format!("multiple assignments to name '{name}' in pattern"),
                            p.range(),
                        ));
                    }
                    captures.insert(name, binding);
                }
                None
            }
            ast::Pattern::MatchStar(p) => p.name.as_ref(),
            ast::Pattern::MatchAs(p) => {
                if let Some(inner) = &p.pattern {
                    self.validate_pattern(inner, captures, allow_irrefutable)?;
                } else if !allow_irrefutable {
                    let reason = match &p.name {
                        Some(name) => format!("name capture '{}'", self.definition_spelling(name)),
                        None => "wildcard".to_owned(),
                    };
                    return Err(Error::source_syntax(
                        format!("{reason} makes remaining patterns unreachable"),
                        p.range(),
                    ));
                }
                p.name.as_ref()
            }
            _ => return Err(unsupported("match pattern")),
        };
        if let Some(name) = name {
            let spelling = self.definition_spelling(name);
            if captures
                .insert(spelling.to_owned(), name.to_string())
                .is_some()
            {
                return Err(Error::source_syntax(
                    format!("multiple assignments to name '{spelling}' in pattern"),
                    pattern.range(),
                ));
            }
            self.locals.insert(name.to_string());
        }
        Ok(())
    }

    /// Declare every local name in a loop or unpacking target.
    fn declare_target(&mut self, target: &ast::Expr) -> Result<()> {
        Self::collect_target_names(target, &mut self.locals)
    }

    fn collect_target_names(target: &ast::Expr, names: &mut impl Extend<String>) -> Result<()> {
        match target {
            ast::Expr::Name(n) => {
                names.extend([n.id.to_string()]);
            }
            ast::Expr::Attribute(_) | ast::Expr::Subscript(_) => {}
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
                    Self::collect_target_names(
                        match element {
                            ast::Expr::Starred(star) => &star.value,
                            _ => element,
                        },
                        names,
                    )?;
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
        let skip = self.false_branches(test, line, function)?;
        self.lower(body, lines, function, active)?;
        let end = self.instructions.len();
        self.instructions.push((line, Instruction::Jump(0)));
        self.finish_branches(skip);
        Ok(end)
    }
}

/// CPython symbol analysis and code generation expose source ranges differently.
#[derive(Clone, Copy, Debug)]
enum SyntaxSpan {
    /// Code generation includes the location tuple in SyntaxError.args.
    InArgs(ruff_text_size::TextRange),
    /// Symbol analysis only sets the location attributes.
    InFields(ruff_text_size::TextRange),
}

/// Reserved captures are symbol errors, before code generation checks sequence shapes or duplicates.
fn reserved_pattern_capture(body: &[ast::Stmt]) -> Option<ruff_text_size::TextRange> {
    use ast::visitor::{self, Visitor};

    struct Check(Option<ruff_text_size::TextRange>);
    impl<'a> Visitor<'a> for Check {
        fn visit_pattern(&mut self, pattern: &'a ast::Pattern) {
            if self.0.is_some() {
                return;
            }
            visitor::walk_pattern(self, pattern);
            let name = match pattern {
                ast::Pattern::MatchAs(p) => &p.name,
                ast::Pattern::MatchStar(p) => &p.name,
                ast::Pattern::MatchMapping(p) => &p.rest,
                _ => return,
            };
            if self.0.is_none() && name.as_ref().is_some_and(|name| name == "__debug__") {
                self.0 = Some(pattern.range());
            }
        }

        fn visit_expr(&mut self, _: &'a ast::Expr) {}
    }

    let mut check = Check(None);
    check.visit_body(body);
    check.0
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

    fn located_syntax_error(
        &mut self,
        message: &str,
        span: SyntaxSpan,
        filename: Id,
        lines: &[usize],
    ) -> Result<Error> {
        let (SyntaxSpan::InArgs(range) | SyntaxSpan::InFields(range)) = span;
        let position = |offset: ruff_text_size::TextSize| {
            let offset = offset.to_usize();
            let line = lines.partition_point(|start| *start <= offset);
            (line as i64, (offset - lines[line - 1] + 1) as i64)
        };
        let (line, column) = position(range.start());
        let (end_line, end_column) = position(range.end());
        let msg = self.text(message);
        let line = self.int(line);
        let location = [
            filename,
            line,
            self.int(column),
            NONE,
            self.int(end_line),
            self.int(end_column),
        ];
        let mut error = match span {
            SyntaxSpan::InArgs(_) => {
                let details = self.alloc(Value::Tuple(location.to_vec()));
                self.exception("SyntaxError", &[msg, details])
            }
            SyntaxSpan::InFields(_) => {
                let mut error = self.exception("SyntaxError", &[msg]);
                error.message.push_str(&self.syntax_suffix(filename, line)?);
                error
            }
        };
        let mut fields = self.syntax_fields(&[msg]);
        fields[1..7].copy_from_slice(&location);
        self.retain_syntax_fields(&mut error, fields)?;
        Ok(error)
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

    fn syntax_lineno(&self, line: Id) -> Option<i64> {
        if let Value::Int(number) = self.heap.get(line) {
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
        }
    }

    /// Construct a Python suffix from the filename's storage before rendering the message.
    pub(super) fn surrogate_syntax_suffix(&mut self, filename: Id, line: Id) -> Result<Option<Id>> {
        let Value::SurrogateStr(points) = self.heap.get(self.scalar_value(filename)) else {
            return Ok(None);
        };
        let name = points
            .rsplit(|&p| p == std::path::MAIN_SEPARATOR as u32)
            .next()
            .unwrap();
        let mut suffix = Vec::new();
        suffix
            .try_reserve_exact(name.len().saturating_add(40))
            .map_err(|_| Error::new("MemoryError", ""))?;
        suffix.extend([u32::from(' '), u32::from('(')]);
        suffix.extend_from_slice(name);
        if let Some(n) = self.syntax_lineno(line) {
            suffix.extend(format!(", line {n}").bytes().map(u32::from));
        }
        suffix.push(u32::from(')'));
        Ok(Some(self.text_codepoints(suffix)))
    }

    pub(super) fn syntax_suffix(&self, filename: Id, line: Id) -> Result<String> {
        use std::fmt::Write;
        let display = self
            .string_storage(filename)
            .map(|id| self.filename_display(id))
            .transpose()?;
        let filename = display
            .as_deref()
            .and_then(|path| path.rsplit(std::path::MAIN_SEPARATOR).next());
        let line = self.syntax_lineno(line);
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

    /// CPython expands tabs before checking the UTF-8 representation of a docstring.
    fn check_source_docstrings(
        &mut self,
        body: &[ast::Stmt],
        module_docstring: bool,
        spellings: &StringReplacements,
    ) -> Result<()> {
        let Some(doc) = imre_ast::strings::find_docstring(body, module_docstring, |doc| {
            spellings
                .range(doc.start()..doc.end())
                .any(|(_, points)| points.iter().any(|&p| p != 0xfffd))
                .then_some(doc)
        }) else {
            return Ok(());
        };
        let text = doc.value.to_str();
        let expanded = crate::objects::expand_tabs(text, 8)?;
        let points = original_points(
            expanded.as_ref().map_or(text, |s| s.as_str()),
            spellings
                .range(doc.start()..doc.end())
                .flat_map(|(_, points)| points.iter().copied()),
        )?
        .expect("docstring contains a surrogate");
        let id = self.text_codepoints(points);
        Err(self
            .encode_utf8_codepoints(id, false, "strict")
            .expect_err("surrogates cannot encode as UTF-8"))
    }

    pub(super) fn compile_source(
        &mut self,
        source: Id,
        filename: Id,
        mode: Id,
        strip_indent: bool,
    ) -> Result<Id> {
        let inherited = match self.heap.get(self.heap.current_code()) {
            Value::Code(caller) => {
                caller.flags & (code_flags::FUTURE_ANNOTATIONS | code_flags::FUTURE_BARRY_AS_BDFL)
            }
            _ => 0,
        };
        self.compile_source_with_flags(source, filename, mode, strip_indent, inherited)
    }

    fn source_filename(&self, object: Id) -> Result<SourceFilename> {
        Ok(SourceFilename {
            object,
            display: Rc::from(self.filename_display(object)?.as_ref()),
        })
    }

    pub(super) fn compile_source_with_flags(
        &mut self,
        source: Id,
        filename: Id,
        mode: Id,
        strip_indent: bool,
        inherited: u32,
    ) -> Result<Id> {
        use ruff_python_parser::Mode;
        let mode = self.str_value(mode)?;
        let interactive = mode == "single";
        let parser_mode = match mode {
            "exec" | "single" => Mode::Module,
            "eval" => Mode::Expression,
            _ => {
                return Err(Error::new(
                    "ValueError",
                    "compile() mode must be 'exec', 'eval' or 'single'",
                ));
            }
        };
        let filename_id = filename;
        let filename = self.source_filename(filename_id)?;
        let source = match self.heap.get(self.scalar_value(source)) {
            Value::Bytes(data) | Value::ByteArray(data) => {
                match crate::objects::decode_compile_source(data) {
                    Ok(text) => Cow::Owned(text),
                    Err(error) => return Err(self.source_decode_error(error, filename_id)),
                }
            }
            _ => Cow::Borrowed(self.str_value(source)?),
        };
        if source.contains('\0') {
            return Err(
                self.source_decode_error(crate::objects::SourceDecodeError::Null, filename_id)
            );
        }
        let source = if strip_indent {
            source.trim_start_matches([' ', '\t'])
        } else {
            &source
        };
        let normalized;
        let source = if source.contains('\r') {
            normalized = source.replace("\r\n", "\n").replace('\r', "\n");
            normalized.as_str()
        } else {
            source
        };
        let input = if parser_mode == Mode::Module {
            imre_ast::indentation::prepare(source)
        } else {
            imre_ast::indentation::Source::original(source)
        };
        let parsed = ruff_python_parser::parse_unchecked(&input.text, parser_mode.into());
        // In single mode, a second statement is an error before its suite is read.
        let first_statement_end = match (interactive, parsed.syntax()) {
            (true, ast::Mod::Module(module)) => module.body.first().map(Ranged::end),
            _ => None,
        };
        if let Some(limit) =
            IndentationDiagnostic::find(&parsed, source, mode == "exec", first_statement_end)
        {
            return limit.raise(self, filename_id);
        }
        if let Some(issue) = input.first_issue(parsed.errors().first()) {
            if interactive
                && let ast::Mod::Module(module) = parsed.syntax()
                && module.body.len() > 1
                && module.body[1].start().to_usize() < issue.offset
            {
                check_single_input(&module.body, parsed.tokens())?;
            }
            return IndentationDiagnostic::from_issue(source, issue, mode == "exec")
                .raise(self, filename_id);
        }
        let lines: Rc<[usize]> =
            line_starts(source.match_indices('\n').map(|(offset, _)| offset + 1))?.into();
        if let Some(error) = parsed.errors().first() {
            let position = |offset: ruff_text_size::TextSize| {
                let byte = offset.to_usize();
                let row = lines.partition_point(|&start| start <= byte);
                let column = source[lines[row - 1]..byte].chars().count() + 1;
                (row, column)
            };
            let (row, column) = position(error.location.start());
            let (end_row, end_column) = position(error.location.end());
            let end = lines.get(row).copied().unwrap_or(source.len());
            let mut text = source[lines[row - 1]..end].to_owned();
            if mode == "exec" && !text.ends_with('\n') {
                text.push('\n');
            }
            let detail = format!(": {error}");
            let message = format!("{}{detail}", filename.display);
            let detail = self.text(detail);
            let msg = self.concat_strings(filename_id, detail)?;
            let details = vec![
                filename_id,
                self.int(row as i64),
                self.int(column as i64),
                self.text(text),
                self.int(end_row as i64),
                self.int(end_column as i64),
            ];
            let location = self.alloc(Value::Tuple(details));
            let args = [msg, location];
            let mut error = self.exception("SyntaxError", &args);
            error.message = message;
            self.retain_syntax_fields(&mut error, self.syntax_fields(&args))?;
            return Err(error);
        }
        if interactive && let ast::Mod::Module(module) = parsed.syntax() {
            check_single_input(&module.body, parsed.tokens())?;
        }
        let future_error = match parsed.syntax() {
            ast::Mod::Module(module) => imre_ast::future_import_error(&module.body),
            _ => None,
        };
        let spellings = string_replacements(source, parsed.tokens());
        let scope_error = imre_ast::async_generators::check(parsed.syntax().into());
        if scope_error.is_some()
            && let ast::Mod::Module(module) = parsed.syntax()
            && imre_ast::misplaced_star_import(&module.body).is_some()
        {
            return Err(Error::new(
                "SyntaxError",
                "import * only allowed at module level",
            ));
        }
        let invalid = scope_error
            .map(|error| {
                let span = if error.location_in_args {
                    SyntaxSpan::InArgs(error.range)
                } else {
                    SyntaxSpan::InFields(error.range)
                };
                (span, error.message.to_owned())
            })
            .or_else(|| {
                let keyword = imre_ast::repeated_keyword(parsed.syntax().into())?;
                let name = keyword.arg.as_ref().unwrap();
                Some((
                    SyntaxSpan::InArgs(keyword.range()),
                    format!("keyword argument repeated: {name}"),
                ))
            });
        if let Some((span, message)) = invalid {
            return Err(self.located_syntax_error(&message, span, filename_id, &lines)?);
        }
        if let ast::Mod::Module(module) = parsed.syntax()
            && let Some((range, message)) = imre_ast::type_alias_error(&module.body)
        {
            return Err(self.located_syntax_error(
                &message,
                SyntaxSpan::InFields(range),
                filename_id,
                &lines,
            )?);
        }
        if let ast::Mod::Module(module) = parsed.syntax()
            && let Some(range) = reserved_pattern_capture(&module.body)
        {
            return Err(self.located_syntax_error(
                "cannot assign to __debug__",
                SyntaxSpan::InFields(range),
                filename_id,
                &lines,
            )?);
        }
        let mut code = Program::new(filename, "<module>", source.contains(":="));
        code.scope_flags = inherited;
        code.source_lines = lines.clone();
        code.strip_docstrings = self.config["optimize"].as_i64().unwrap_or(0) >= 2;
        code.interactive = interactive;
        code.template_expressions = Rc::new(imre_ast::strings::template_expressions(
            source,
            parsed.tokens(),
            parsed.syntax(),
        ));
        match parsed.into_syntax() {
            ast::Mod::Expression(expression) => {
                if code.has_named_bindings {
                    use ast::visitor::Visitor;
                    let mut named = NamedBindings::default();
                    named.visit_expr(&expression.body);
                    named.declare(&mut code)?;
                }
                code.instructions
                    .push((1, Instruction::Return(Some(*expression.body))));
            }
            ast::Mod::Module(module) => {
                let mut body = module.body;
                if !code.strip_docstrings && !spellings.is_empty() {
                    self.check_source_docstrings(&body, !interactive, &spellings)?;
                }
                code.method_imports = Rc::new(imre_ast::module_imports(&body));
                if imre_ast::future_import(&body, "barry_as_FLUFL") {
                    code.scope_flags |= code_flags::FUTURE_BARRY_AS_BDFL;
                }
                if code.scope_flags & code_flags::FUTURE_ANNOTATIONS != 0
                    || imre_ast::future_annotations(&body)
                {
                    code.scope_flags |= code_flags::FUTURE_ANNOTATIONS;
                    code.private_names = Some(Rc::new(imre_ast::lower_in(&mut body, "", true)));
                }
                code.syntax_limit = future_error.map(|(range, _)| range);
                if let Err(error) = code.lower_namespace(body, &lines, false) {
                    if error.kind == "NotImplementedError"
                        && let Some((range, message)) = future_error
                    {
                        return Err(self.located_syntax_error(
                            message,
                            SyntaxSpan::InFields(range),
                            filename_id,
                            &lines,
                        )?);
                    }
                    if let Some(span) = error.trace.as_ref().and_then(|trace| trace.pending_syntax)
                    {
                        return Err(self.located_syntax_error(
                            &error.message,
                            SyntaxSpan::InArgs(span),
                            filename_id,
                            &lines,
                        )?);
                    }
                    return Err(error);
                }
                // Closure binding resolution follows symbol collection and future placement.
                if future_error.is_none() {
                    let mut module_globals = HashSet::new();
                    code.resolve_directives(&|_| None, &mut module_globals)?;
                    // A global declared by a nested block makes module assignments use the globals dictionary.
                    for name in module_globals {
                        code.directives
                            .entry(name)
                            .or_insert((ScopeDirective::Global, 0.into()));
                    }
                }
            }
        }
        code.prepare_strings(
            &mut self.heap,
            &mut SharedConstants::default(),
            &spellings,
            &|offset| lines.partition_point(|n| *n <= offset) as u32,
        )?;
        if let Some((range, message)) = future_error {
            return Err(self.located_syntax_error(
                message,
                SyntaxSpan::InFields(range),
                filename_id,
                &lines,
            )?);
        }
        Ok(self.box_source_code(Rc::new(code), None))
    }

    /// Attach the filename after tokenization, just as compile does for string input.
    pub(super) fn source_decode_error(
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
                .filename_display(filename)
                .expect("source filename was validated");
            let basename = name
                .rsplit(std::path::MAIN_SEPARATOR)
                .next()
                .unwrap_or(&name);
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
        self.box_source_closure(program, parameters, None)
    }

    fn box_source_closure(
        &mut self,
        program: Rc<Program>,
        parameters: Option<Id>,
        freevars: Option<Vec<String>>,
    ) -> Id {
        let name = self.heap.code_name(&program.name);
        let qualname = self.heap.code_name(&program.qualname);
        let filename = program.filename.object;
        let doc = program
            .doc
            .and_then(|range| program.literals.get(&range).copied())
            .unwrap_or(NONE);
        self.alloc(Value::Code(value::CodeObject {
            parsed_parameters: Default::default(),
            name,
            qualname,
            doc,
            filename,
            first_line: program.first_line,
            module_definition: !program.nested_global && program.name == program.qualname,
            flags: program.scope_flags
                | match program.kind {
                    FunctionKind::Generator | FunctionKind::GeneratorLambda => {
                        imre_ast::code_flags::GENERATOR
                    }
                    FunctionKind::Coroutine => imre_ast::code_flags::COROUTINE,
                    FunctionKind::AsyncGenerator => imre_ast::code_flags::ASYNC_GENERATOR,
                    FunctionKind::Plain => 0,
                },
            flags_complete: program.scope_flags & code_flags::OPTIMIZED != 0
                && !program.unlowered_yield,
            body: Body::Source(program),
            parameters: parameters.unwrap_or(NONE),
            nested: Vec::new(),
            closure_supported: freevars.is_some(),
            freevars: freevars.unwrap_or_default(),
        }))
    }

    /// Define a function whose parameters and executable belong to the generated code object.
    fn source_function(&mut self, code: Id, scope: Id, defaults: Id) -> Id {
        let name = self.code_object(code).name;
        let doc = self.code_object(code).doc;
        let qualname = self.code_object(code).qualname;
        self.define_function(code, scope, name, defaults, qualname, doc, NONE, false)
    }

    pub(super) fn dict_named_item(&self, dict: Id, name: &str) -> Option<Id> {
        self.heap.dict_named_item(dict, name)
    }

    /// Attach the name at the failed lookup, leaving exceptions raised by Python code alone.
    fn undefined_source_name(name: &str, object: Id) -> Error {
        let mut error = Error::new("NameError", format!("name '{name}' is not defined"));
        error.tracing().name_error_name = Some(object);
        error
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
        Err(Self::undefined_source_name(name, self.text(name)))
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
                self.module_globals(scope)?
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
        let class_cell = freevars
            .iter()
            .position(|name| name == "__class__")
            .map(|index| cells[index]);
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
            class_cell,
            defaults,
            kwdefaults,
        }))
    }

    /// Populate the public module with the same native objects used by compiled calls.
    pub(super) fn install_builtins(&mut self, scope: Id) -> Result<()> {
        for (name, value) in [
            ("None", NONE),
            ("False", value::FALSE),
            ("True", value::TRUE),
            ("__debug__", value::TRUE),
            ("Ellipsis", value::ELLIPSIS),
            ("NotImplemented", value::NOT_IMPLEMENTED),
        ] {
            self.bind(scope, name, value);
        }
        for name in builtins::names() {
            let value = self.builtin(name);
            self.bind(scope, name, value);
        }
        let os_error = self.builtin("OSError");
        self.bind(scope, "EnvironmentError", os_error);
        self.bind(scope, "IOError", os_error);
        let dict = self.module_globals(scope)?;
        // Reimporting an evicted module leaves existing frames' builtins intact.
        self.source_builtins.get_or_insert(dict);
        Ok(())
    }

    pub(super) fn default_source_builtins(&mut self) -> Result<Id> {
        if let Some(dict) = self.source_builtins {
            return Ok(dict);
        }
        self.module("builtins")?;
        Ok(self
            .source_builtins
            .expect("initialized builtins dictionary"))
    }

    /// Evaluate one forward-reference expression in its defining Python namespaces.
    pub(super) fn eval_type_expression(&mut self, text: Id, globals: Id, locals: Id) -> Result<Id> {
        let source = self.str_value(text)?;
        let parsed = ruff_python_parser::parse_expression(source).map_err(|_| {
            Error::new(
                "SyntaxError",
                format!("Forward reference must be an expression -- got {source:?}"),
            )
        })?;
        if let ast::Expr::Name(ident) = parsed.syntax().body.as_ref()
            && usize::from(ident.range().len()) == source.len()
        {
            // ForwardRef.evaluate looks up the original string for a bare identifier.
            // The parser applies NFKC to ident.id, as required for compound expressions.
            for mapping in [locals, globals, self.default_source_builtins()?] {
                if self.contains(text, mapping)? {
                    return self.get_item(mapping, text);
                }
            }
            return Err(Self::undefined_source_name(self.str_value(text)?, text));
        }
        let filename = self.text("<string>");
        let mode = self.text("eval");
        let code = self.compile_source(text, filename, mode, false)?;
        self.run_in_namespace(code, globals, Some(locals), None, "eval", None)
    }

    /// Run eval or exec with explicit namespaces or the active Python frame.
    pub(super) fn call_eval_exec(
        &mut self,
        scope: Id,
        name: &str,
        args: &[Id],
        kwargs: &[(String, Id)],
    ) -> Result<Id> {
        let Some(&source) = args.first() else {
            return Err(Error::new(
                "TypeError",
                format!("{name}() takes at least 1 positional argument (0 given)"),
            ));
        };
        let (globals, locals, closure) = if name == "exec" {
            let [_, globals, locals, closure] = builtins::bind_method_arguments(
                name,
                args,
                kwargs,
                ["", "globals", "locals", "closure"],
                3,
            )?;
            (globals, locals, closure.filter(|&value| value != NONE))
        } else {
            let [_, globals, locals] =
                builtins::bind_method_arguments(name, args, kwargs, ["", "globals", "locals"], 3)?;
            (globals, locals, None)
        };
        let globals = globals.filter(|&g| g != NONE);
        let locals = match locals.filter(|&l| l != NONE) {
            Some(locals) => Some(locals),
            None if globals.is_none() => Some(self.frame_locals(scope)?),
            None => None,
        };
        let globals = match globals {
            Some(globals) => globals,
            None => self.module_globals(scope)?,
        };
        let value = self.run_in_namespace(source, globals, locals, None, name, closure)?;
        Ok(if name == "exec" { NONE } else { value })
    }

    /// Resolve the caller's namespace, copying bindings only for optimized frames.
    pub(super) fn frame_locals(&mut self, scope: Id) -> Result<Id> {
        self.locals_for_code(scope, self.heap.current_code())
    }

    /// Read bindings using the code that owns the scope, including its free variables.
    pub(super) fn locals_for_code(&mut self, scope: Id, code: Id) -> Result<Id> {
        let frame = self.scope_data(scope);
        if let Some(locals) = frame.exec_locals {
            return Ok(locals);
        }
        if frame.parent.is_none() && !frame.closure.cleared {
            return self.module_globals(scope);
        }
        if frame.class_body.is_some() {
            return Ok(self.active_class_namespace(scope));
        }
        let mut bindings = Vec::new();
        for name in self.frame_binding_names(scope, code)? {
            match self.lookup(scope, &name) {
                Ok(value) => bindings.push((self.heap.code_name(&name), value)),
                Err(error) if self.is_builtin_error(&error, "NameError") => {}
                Err(error) => return Err(error),
            }
        }
        let snapshot = self.dict(bindings);
        if let Some(extras) = self.scope_data(scope).closure.extra_locals {
            self.for_each_mapping_item(
                extras,
                snapshot,
                super::calls::MappingOperation::Update,
                |ctx, key, value, hash| {
                    ctx.set_dictionary_key(snapshot, key, value, hash, true)
                        .map(|_| ())
                },
            )?;
        }
        Ok(snapshot)
    }

    pub(super) fn frame_binding_names(
        &self,
        scope: Id,
        code: Id,
    ) -> Result<indexmap::IndexSet<String>> {
        let mut names = indexmap::IndexSet::new();
        let frame = self.scope_data(scope);
        if frame.comprehension.is_none()
            && let Value::Code(code) = self.heap.get(code)
            && let Value::Str(parameters) = self.heap.get(code.parameters)
        {
            let signature: serde_json::Value = serde_json::from_str(parameters)
                .map_err(|error| Error::new("SystemError", error.to_string()))?;
            // Argument slots come first, including arguments held in closure cells.
            let arguments = ["posonly", "positional", "kwonly"]
                .into_iter()
                .flat_map(|group| signature[group].as_array().into_iter().flatten())
                .filter_map(serde_json::Value::as_str)
                .chain(
                    ["varargs", "kwargs"]
                        .into_iter()
                        .filter_map(|key| signature[key].as_str()),
                );
            for name in arguments {
                if frame.names.contains_key(name) || frame.closure.cells.contains_key(name) {
                    names.insert(name.to_owned());
                }
            }
            // Slot order comes from the code layout even when a branch skipped the first store.
            for name in signature["scope_locals"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(serde_json::Value::as_str)
            {
                if frame.names.contains_key(name) {
                    names.insert(name.to_owned());
                }
            }
        }
        let mut local = scope;
        loop {
            let frame = self.scope_data(local);
            names.extend(frame.names.keys().map(|name| name.as_str().to_owned()));
            let mut cells: Vec<_> = frame.closure.cells.keys().cloned().collect();
            cells.sort_unstable();
            names.extend(cells);
            let Some(parent) = frame.parent else { break };
            if frame.comprehension != Some(ComprehensionKind::Ordinary)
                || self.scope_data(parent).parent.is_none()
            {
                break;
            }
            local = parent;
        }
        if let Value::Code(code) = self.heap.get(code) {
            names.extend(code.freevars.iter().cloned());
            // Some compiled functions retain their lexical environment because their entire
            // closure cannot yet be cloned. Their statically resolved reads are still locals.
            let frame = self.scope_data(local);
            if !code.closure_supported
                && frame.comprehension.is_none()
                && let Some(parent) = frame.parent
                && let Value::Str(signature) = self.heap.get(code.parameters)
            {
                #[derive(serde::Deserialize)]
                struct Reads<'a> {
                    #[serde(default, borrow)]
                    scope_reads: Vec<&'a str>,
                }
                let reads: Reads = serde_json::from_str(signature)
                    .map_err(|err| Error::new("SystemError", err.to_string()))?;
                names.extend(
                    reads
                        .scope_reads
                        .into_iter()
                        .filter(|name| self.is_enclosing_freevar(parent, name) == Some(true))
                        .map(str::to_owned),
                );
            }
        }
        Ok(names)
    }

    pub(super) fn exec_source(
        &mut self,
        source: Id,
        globals: Id,
        locals: Option<Id>,
        function: Option<Id>,
    ) -> Result<Id> {
        self.run_in_namespace(source, globals, locals, function, "exec", None)
            .map(|_| NONE)
    }

    fn run_in_namespace(
        &mut self,
        source: Id,
        globals: Id,
        locals: Option<Id>,
        function: Option<Id>,
        mode: &str,
        closure: Option<Id>,
    ) -> Result<Id> {
        if self.dict_storage(globals).is_none() {
            let ty = self.type_name(globals);
            return Err(Error::new(
                "TypeError",
                format!("{mode}() globals must be a dict, not {ty}"),
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
                self.module_globals(scope)?
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
        let code_id = match self.heap.get(self.scalar_value(source)) {
            Value::Code(code) if mode == "eval" && !code.freevars.is_empty() => {
                return Err(Error::new(
                    "TypeError",
                    "code object passed to eval() may not contain free variables",
                ));
            }
            Value::Code(code) if !code.freevars.is_empty() => {
                let required = code.freevars.len();
                if let Some(Value::Tuple(cells)) = closure.map(|id| self.heap.get(id))
                    && cells.len() == required
                    && cells
                        .iter()
                        .all(|&id| matches!(self.heap.get(id), Value::Cell(_)))
                {
                    return Err(unsupported("exec with an explicit closure"));
                }
                return Err(Error::new(
                    "TypeError",
                    format!("code object requires a closure of exactly length {required}"),
                ));
            }
            Value::Code(_) if closure.is_some() => {
                return Err(Error::new(
                    "TypeError",
                    "cannot use a closure with this code object",
                ));
            }
            Value::Code(_) => source,
            _ if closure.is_some() => {
                return Err(Error::new(
                    "TypeError",
                    "closure can only be used when source is a code object",
                ));
            }
            Value::Str(_) | Value::Bytes(_) | Value::ByteArray(_) => {
                let filename = self.text("<string>");
                let mode_id = self.text(mode);
                self.compile_source(source, filename, mode_id, mode == "eval")?
            }
            _ => {
                return Err(Error::new(
                    "TypeError",
                    format!("{mode}() arg 1 must be a string, bytes or code object"),
                ));
            }
        };
        let code = self.code_object(code_id);
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
            self.call(function, Vec::new(), Vec::new())
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
            self.execute(code_id, module, owner)
        }
    }

    pub(super) fn call_source(&mut self, code: Id, scope: Id, owner: FrameOwner) -> Result<Id> {
        let definition = self.code_object(code);
        let kind = match &definition.body {
            Body::Source(program) => program.kind,
            Body::PendingGenerator { .. } => FunctionKind::prelinked(definition),
            Body::Native(_) => unreachable!("source function"),
        };
        if kind != FunctionKind::Plain {
            let frame = self.new_generator(code, scope, owner);
            Ok(if kind == FunctionKind::Coroutine {
                let origin = self.coroutine_origin();
                let coroutine = self.alloc(Value::Coroutine { frame, origin });
                self.heap.watch_unstarted_coroutine(coroutine)?;
                coroutine
            } else if kind == FunctionKind::AsyncGenerator {
                self.async_generator(frame)
            } else {
                frame
            })
        } else {
            match self.run_source(
                code,
                scope,
                owner,
                SourceCursor::new(
                    0,
                    &mut Vec::new(),
                    &mut Vec::new(),
                    None,
                    GeneratorInput::Send(NONE),
                ),
            )? {
                Step::Return(v) => Ok(v),
                Step::Yield(..) | Step::Advance(..) | Step::Rejected(..) => {
                    unreachable!("classified generator")
                }
            }
        }
    }

    fn prepare_generator(&mut self, id: Id) -> Result<()> {
        let code = self.code_object(id);
        let Body::PendingGenerator { source, body_line } = &code.body else {
            return Ok(());
        };
        let body_line = *body_line;
        let input = imre_ast::indentation::parser_source(source)
            .map_err(|err| Error::new(err.kind, err.to_string()))?;
        let parsed = ruff_python_parser::parse_module(&input)
            .map_err(|err| Error::new("SyntaxError", err.to_string()))?;
        let spellings = string_replacements(source, parsed.tokens());
        let template_expressions =
            imre_ast::strings::template_expressions(source, parsed.tokens(), parsed.syntax());
        let mut syntax = parsed.into_syntax();
        let [ast::Stmt::FunctionDef(def)] = syntax.body.as_mut_slice() else {
            return Err(Error::new("SystemError", "invalid generator body"));
        };
        if reserved_pattern_capture(&def.body).is_some() {
            return Err(Error::new("SyntaxError", "cannot assign to __debug__"));
        }
        #[derive(serde::Deserialize)]
        struct Signature<'a> {
            #[serde(borrow)]
            private_scope: Option<(&'a str, bool)>,
            #[serde(default)]
            generator_lambda: bool,
            #[serde(default)]
            generator_expression: bool,
            #[serde(default)]
            method_imports: std::collections::BTreeSet<String>,
        }
        let signature: Signature<'_> = serde_json::from_str(self.str_value(code.parameters)?)
            .map_err(|_| Error::new("SystemError", "invalid generator signature"))?;
        let postponed = code.flags & imre_ast::code_flags::FUTURE_ANNOTATIONS != 0;
        let private_names = signature
            .private_scope
            .or(postponed.then_some(("", true)))
            .map(|(owner, postponed)| Rc::new(imre_ast::lower_in(&mut def.body, owner, postponed)));
        let lines: Rc<[usize]> =
            line_starts(source.match_indices('\n').map(|(offset, _)| offset + 1))?.into();
        let mut program = Program::new(
            self.source_filename(code.filename)?,
            self.str_value(code.name)?,
            source.contains(":="),
        );
        program.strip_docstrings = self.config["optimize"].as_i64().unwrap_or(0) >= 2;
        program.source_lines = lines.clone();
        program.linked_body_line = Some(body_line);
        program.method_imports = Rc::new(signature.method_imports);
        program.template_expressions = Rc::new(template_expressions);
        program.private_names = private_names;
        program.qualname = self.str_value(code.qualname)?.to_owned();
        program.first_line = code.first_line;
        program.scope_flags = code.flags;
        program.kind = FunctionKind::prelinked(code);
        if signature.generator_expression {
            let [ast::Stmt::Return(ret)] = def.body.as_slice() else {
                return Err(Error::new(
                    "SystemError",
                    "invalid generator expression body",
                ));
            };
            let Some(ast::Expr::Generator(expr)) = ret.value.as_deref() else {
                return Err(Error::new("SystemError", "invalid generator expression"));
            };
            program.lower_genexpr(expr, &|offset| {
                lines.partition_point(|n| *n <= offset) as u32
            })?;
        } else {
            program.lower_namespace(std::mem::take(&mut def.body), &lines, true)?;
        }
        if signature.generator_lambda {
            program.kind = FunctionKind::GeneratorLambda;
        }
        for (line, _) in &mut program.instructions {
            *line = body_line + line.saturating_sub(2);
        }
        // The prelinked generator was compiled in its enclosing lexical scope.
        program.resolve_directives(
            &|name| {
                (!code.closure_supported || code.freevars.iter().any(|var| var == name))
                    .then_some(EnclosingBinding::Local)
            },
            &mut HashSet::new(),
        )?;
        program.prepare_strings(
            &mut self.heap,
            &mut SharedConstants::default(),
            &spellings,
            &|offset| {
                body_line + (lines.partition_point(|n| *n <= offset) as u32).saturating_sub(2)
            },
        )?;
        let Value::Code(code) = self.heap.get_mut(id) else {
            unreachable!()
        };
        code.body = Body::Source(Rc::new(program));
        Ok(())
    }

    pub(super) fn generator_send(&mut self, generator: Id, sent: Id) -> Result<Id> {
        self.send_generator_frame(generator, generator, sent)
    }

    /// Send a value into a frame while keeping its public generator or coroutine alive.
    pub(super) fn send_generator_frame(
        &mut self,
        generator: Id,
        finalizer: Id,
        sent: Id,
    ) -> Result<Id> {
        let Value::Generator {
            code,
            running,
            offset,
            ..
        } = *self.heap.get(generator)
        else {
            return Err(Error::new(
                "TypeError",
                "generator.send requires a generator",
            ));
        };
        let coroutine = self.code_object(code).is_coroutine();
        let kind = if coroutine { "coroutine" } else { "generator" };
        if running {
            return Err(Error::new(
                "ValueError",
                format!("{kind} already executing"),
            ));
        }
        if coroutine && offset.is_none() {
            return Err(Error::new(
                "RuntimeError",
                "cannot reuse already awaited coroutine",
            ));
        }
        if sent != NONE && offset == Some(0) {
            return Err(Error::new(
                "TypeError",
                format!("can't send non-None value to a just-started {kind}"),
            ));
        }
        let next = if matches!(self.code_object(code).body, Body::Native(_)) {
            self.next_result(generator)?
        } else {
            stacker::maybe_grow(1024 * 1024, 4 * 1024 * 1024, || {
                self.resume_source_input(generator, finalizer, GeneratorInput::Send(sent))
            })?
        };
        next.ok_or_else(|| self.exception("StopIteration", &[]))
    }

    pub(super) fn resume_source(&mut self, id: Id, sent: Id) -> Result<Option<Id>> {
        self.resume_source_input(id, id, GeneratorInput::Send(sent))
    }

    pub(super) fn resume_source_input(
        &mut self,
        mut id: Id,
        finalizer: Id,
        mut input: GeneratorInput,
    ) -> Result<Option<Id>> {
        let initial = id;
        let mut parents = Vec::new();
        let mut advanced = None;
        loop {
            let step = self.resume_source_frame(
                id,
                if id == initial { finalizer } else { id },
                advanced.take(),
                std::mem::replace(&mut input, GeneratorInput::Send(NONE)),
                !parents.is_empty(),
            );
            let result = match step {
                Ok(Step::Advance(child, _, sent)) => {
                    if parents.try_reserve(1).is_err() {
                        advanced = Some(Err(crate::Error::allocation().into()));
                        continue;
                    }
                    parents.push(id);
                    id = child;
                    input = GeneratorInput::Send(sent);
                    continue;
                }
                Ok(Step::Yield(value, _)) => Ok(DelegateOutcome::Yield(value)),
                Ok(Step::Return(value)) => Ok(DelegateOutcome::Done(value)),
                Ok(Step::Rejected(error, _)) => Ok(DelegateOutcome::LookupFailed(error)),
                Err(error) => Err(error),
            };
            let Some(parent) = parents.pop() else {
                return match result? {
                    DelegateOutcome::Yield(value) => Ok(Some(value)),
                    DelegateOutcome::Done(NONE) => Ok(None),
                    DelegateOutcome::Done(value) => Err(self.exception("StopIteration", &[value])),
                    DelegateOutcome::LookupFailed(error) => Err(error),
                };
            };
            id = parent;
            advanced = Some(result);
        }
    }

    fn source_frame_error(&mut self, error: Error, code: Id) -> Error {
        match &self.code_object(code).body {
            // Unlike generator defs and genexprs, generator lambdas have no StopIteration handler.
            Body::Source(program) if program.kind == FunctionKind::GeneratorLambda => error,
            Body::Source(program) if program.kind == FunctionKind::AsyncGenerator => {
                let message = if self.is_builtin_error(&error, "StopAsyncIteration") {
                    "async generator raised StopAsyncIteration"
                } else if self.is_builtin_error(&error, "StopIteration") {
                    "async generator raised StopIteration"
                } else {
                    return error;
                };
                let cause = self.raised_instance(error);
                let mut converted = Error::new("RuntimeError", message);
                converted.tracing().cause = Some(cause);
                converted.context = cause;
                converted
            }
            _ => self.frame_error(error, code),
        }
    }

    /// Resume one frame, retaining its running state while a delegated child executes.
    fn resume_source_frame(
        &mut self,
        id: Id,
        finalizer: Id,
        advanced: Option<Result<DelegateOutcome>>,
        input: GeneratorInput,
        nested_resume: bool,
    ) -> Result<Step> {
        if advanced.is_none() {
            let input_roots = match &input {
                GeneratorInput::Send(_) => 1,
                GeneratorInput::Throw { error, args } => args.len() + error.roots().count(),
            };
            self.heap.reserve_temporaries(1 + input_roots)?;
            self.heap.keep(finalizer);
            match &input {
                GeneratorInput::Send(value) => self.heap.keep(*value),
                GeneratorInput::Throw { error, args } => {
                    for root in error.roots().chain(args.iter().copied()) {
                        self.heap.keep(root);
                    }
                }
            }
        }
        let Value::Generator {
            code,
            scope,
            iterators,
            protected,
            running,
            offset,
            ..
        } = self.heap.get_mut(id)
        else {
            unreachable!()
        };
        if *running && advanced.is_none() {
            return Err(Error::new("ValueError", "generator already executing"));
        }
        let Some(ip) = offset.take() else {
            return match input {
                GeneratorInput::Throw { error, .. } => Err(error),
                GeneratorInput::Send(_) => Ok(Step::Return(NONE)),
            };
        };
        *running = true;
        let (code, frame_scope) = (*code, *scope);
        let mut active = std::mem::take(iterators);
        let mut blocks = std::mem::take(protected);
        self.heap.disarm_finalizer(finalizer);
        let result = self
            .prepare_generator(code)
            .and_then(|()| {
                let mut cursor = SourceCursor::new(ip, &mut active, &mut blocks, advanced, input);
                cursor.nested_resume = nested_resume
                    && ip != 0
                    && cursor.advanced.is_none()
                    && cursor.sent == NONE
                    && cursor.thrown.is_none();
                self.run_source(code, frame_scope, FrameOwner::Generator(id), cursor)
            })
            .map_err(|error| self.source_frame_error(error, code));
        // The public async-generator object owns its first-iteration/finalization hooks.
        let async_generator =
            self.code_object(code).flags & imre_ast::code_flags::ASYNC_GENERATOR != 0;
        // These built-in iterators have no close hook. A suspended outer
        // try/finally still needs its own finalizer below.
        let delegate_needs_close = matches!(&result, Ok(Step::Yield(_, ip))
        if matches!(&self.code_object(code).body, Body::Source(program)
            if matches!(program.instructions.get(*ip), Some((_, Instruction::Delegate))))
        && active.last().is_none_or(|&iter| {
            !matches!(
                self.heap.get(iter),
                Value::Iterator { .. } | Value::RangeIterator(_) | Value::Repeat(_)
            )
        }));
        let Value::Generator {
            running,
            offset,
            scope,
            owner,
            frame,
            iterators,
            protected,
            ..
        } = self.heap.get_mut(id)
        else {
            unreachable!()
        };
        *running = matches!(result, Ok(Step::Advance(..)));
        let completed = match result {
            Ok(step @ (Step::Yield(_, ip) | Step::Advance(_, ip, _) | Step::Rejected(_, ip))) => {
                // A failed lookup of the delegate's throw method leaves it suspended.
                let needs_finalizer = matches!(step, Step::Rejected(..))
                    || (matches!(step, Step::Yield(..))
                        && (!blocks.is_empty() || delegate_needs_close));
                *offset = Some(ip);
                *iterators = active;
                *protected = blocks;
                if needs_finalizer && !async_generator {
                    self.heap.register_finalizer(finalizer);
                }
                return Ok(step);
            }
            Ok(Step::Return(value)) => Ok(value),
            Err(error) => Err(error),
        };
        *scope = NONE;
        *frame = None;
        *owner = FrameOwner::Module(NONE);
        self.heap.disarm_finalizer(finalizer);
        if self.heap.tracks_finalizers()
            && (!active.is_empty()
                || !self.scope_data(frame_scope).closure.cells.is_empty()
                || self
                    .scope_data(frame_scope)
                    .names
                    .values()
                    .any(|&value| self.heap.could_own_finalizer(value, true)))
            && self.heap.has_unbound_finalizer(NONE)
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
            if !self.can_skip_finalizer_sweep(self.heap.temporary_mark()) {
                self.collect_unreachable();
            }
        }
        completed.map(Step::Return)
    }

    /// Run pending cleanups from the inside out. A transfer inside a cleanup replaces
    /// its prior destination, while a jump staying in the same region leaves it intact.
    fn source_transfer(
        &mut self,
        cursor: &mut SourceCursor<'_>,
        transfer: Transfer,
    ) -> Result<Option<Step>> {
        while let Some(block) = cursor.protected.last_mut() {
            if let Transfer::Jump { destination, .. } = transfer {
                let region = if block.pending.is_some() {
                    block.handler..block.end
                } else {
                    block.start..block.handler
                };
                if region.contains(&destination) {
                    break;
                }
            }
            while cursor.iterators.len() > block.depth {
                let iter = cursor.iterators.pop().expect("inner iterator");
                self.release_loop_iterator(iter);
            }
            if matches!(block.kind, HandlerKind::Comprehension(_)) {
                cursor.protected.pop();
                cursor.comprehension = SourceCursor::active_comprehension(cursor.protected);
                continue;
            }
            if block.pending.is_none()
                && (!matches!(block.kind, HandlerKind::Except)
                    || matches!(transfer, Transfer::Raise(_)))
            {
                block.pending = Some(transfer);
                cursor.ip = block.handler;
                return Ok(None);
            }
            cursor.protected.pop();
        }
        match transfer {
            Transfer::Jump {
                destination,
                pop_iter,
            } => {
                if pop_iter {
                    let iter = cursor.iterators.pop().expect("active source loop");
                    self.release_loop_iterator(iter);
                }
                cursor.ip = destination;
                Ok(None)
            }
            Transfer::Return(value) => Ok(Some(Step::Return(value))),
            Transfer::Raise(exception) => {
                let Value::Raised(error) = self.heap.get(exception) else {
                    unreachable!("pending generator exception")
                };
                let mut error = error.clone();
                error.tracing().instance = Some(exception);
                Err(error)
            }
        }
    }

    fn run_source(
        &mut self,
        owner: Id,
        scope: Id,
        frame_owner: FrameOwner,
        mut cursor: SourceCursor<'_>,
    ) -> Result<Step> {
        let Body::Source(code) = self.code_object(owner).body.clone() else {
            unreachable!("source frame")
        };
        if cursor.ip == 0 && !code.directives.is_empty() {
            // A class body's own cell belongs to its methods. The body closes over its defining cell.
            let class_cell = match frame_owner {
                FrameOwner::Function(function) => match self.heap.get(function) {
                    Value::Function { class_cell, .. } => *class_cell,
                    _ => None,
                },
                _ => self.scope_data(scope).class_cell,
            };
            let Value::Scope(frame) = self.heap.get_mut(scope) else {
                unreachable!()
            };
            for (name, (kind, _)) in &code.directives {
                if code.class_nonlocal && name == "__class__" {
                    let cell = class_cell
                        .ok_or_else(|| Error::new("NameError", "empty __class__ cell"))?;
                    frame.closure.cells.insert(name.clone(), cell);
                    continue;
                }
                let names = match kind {
                    ScopeDirective::Global => &mut frame.globals,
                    ScopeDirective::Nonlocal => &mut frame.closure.nonlocals,
                };
                names.insert(name.clone());
            }
        }
        if cursor.advanced.is_none() {
            self.heap.reserve_temporaries(2)?;
            self.heap.keep(owner);
            self.heap.enter(scope, owner, frame_owner);
        }
        let mut live = Vec::new();
        let result = (|| {
            loop {
                // Inject at the suspended yield, even when there is no following statement.
                let index = cursor.ip;
                let op = code.instructions.get(index);
                if !matches!(
                    op,
                    Some((
                        _,
                        Instruction::BindSent(_)
                            | Instruction::ReturnSent
                            | Instruction::YieldResult
                            | Instruction::MakeGenerator(_)
                            | Instruction::StoreNamed(_)
                            | Instruction::InsertDict { .. }
                            | Instruction::InsertSet { .. }
                            | Instruction::Delegate
                            | Instruction::Await(None)
                            | Instruction::SaveOperand(None)
                            | Instruction::Branch { value: None, .. }
                            | Instruction::NextComparison { .. }
                            | Instruction::JumpResult(_)
                            | Instruction::Unary(_)
                            | Instruction::Attribute { .. }
                            | Instruction::AsyncWithMethods(None)
                            | Instruction::Iter(None, _)
                            | Instruction::CollectItem { value: None, .. }
                            | Instruction::BindAsyncItem(_)
                            | Instruction::CallAsyncEnter
                            | Instruction::AwaitWithResult(_)
                            | Instruction::EnterAsyncWith { .. }
                            | Instruction::TestAsyncExit
                            | Instruction::FinishAsyncExit
                    ))
                ) {
                    cursor.sent = NONE;
                }
                let inject =
                    cursor.thrown.is_some() && !matches!(op, Some((_, Instruction::Delegate)));
                let site = if inject {
                    index.saturating_sub(1)
                } else {
                    index
                };
                let line = code
                    .instructions
                    .get(site)
                    .map_or(code.first_line, |(line, _)| *line);
                if cursor.advanced.is_none() {
                    let caught = cursor
                        .protected
                        .iter()
                        .rev()
                        .find_map(|block| match block.pending {
                            Some(Transfer::Raise(error)) => Some(error),
                            _ => None,
                        })
                        .unwrap_or(NONE);
                    let extra = !cursor.protected.is_empty()
                        || cursor.sent != NONE
                        || cursor.thrown.is_some();
                    let roots = if extra {
                        live.clear();
                        live.extend_from_slice(cursor.iterators);
                        live.extend(cursor.protected.iter().flat_map(FinallyBlock::roots));
                        live.push(cursor.sent);
                        if let Some((error, args)) = &cursor.thrown {
                            live.extend(error.roots());
                            live.extend_from_slice(args);
                        }
                        &live[..]
                    } else {
                        &cursor.iterators[..]
                    };
                    let location = Location {
                        file: code.filename.display.clone(),
                        line,
                        span: None,
                    };
                    if std::mem::take(&mut cursor.nested_resume)
                        && matches!(op, Some((_, Instruction::Delegate)))
                        && !self.heap.collection_requested()
                    {
                        // The parent already checked for unreachable objects. Resuming its
                        // suspended delegate only adds live roots before the next Python step.
                        self.heap.publish_statement(roots, location, caught);
                    } else {
                        self.checkpoint(roots, location, caught);
                    }
                }
                #[cfg(unix)]
                let interrupt = if inject {
                    None
                } else {
                    self.dispatch_signals().err()
                };
                #[cfg(not(unix))]
                let interrupt: Option<Error> = None;
                let raised = if inject {
                    cursor.thrown.take().map(|(error, _)| error)
                } else {
                    interrupt
                };
                let transfer = if let Some(mut error) = raised {
                    self.record_exception_frame(&mut error, None);
                    if cursor.protected.is_empty() {
                        return Err(error);
                    }
                    Transfer::Raise(self.raised_instance(error))
                } else if let Some((_, op)) = op {
                    cursor.ip += 1;
                    match op {
                        Instruction::EnterFinally { handler, end, .. }
                        | Instruction::EnterExcept { handler, end } => {
                            cursor.protected.push(FinallyBlock {
                                start: cursor.ip,
                                handler: *handler,
                                end: *end,
                                depth: cursor.iterators.len(),
                                pending: None,
                                kind: if let Instruction::EnterFinally {
                                    annotation_count, ..
                                } = op
                                {
                                    HandlerKind::Finally(*annotation_count)
                                } else {
                                    HandlerKind::Except
                                },
                            });
                            continue;
                        }
                        Instruction::StartFinally => {
                            let end = cursor.protected.last().expect("active try").end;
                            Transfer::Jump {
                                destination: end,
                                pop_iter: false,
                            }
                        }
                        Instruction::EndAsyncFor => {
                            let block = cursor.protected.pop().expect("async for guard");
                            let Some(Transfer::Raise(exception)) = block.pending else {
                                unreachable!("async for exception handler");
                            };
                            let iter = cursor.iterators.pop().expect("async iterator");
                            self.release_loop_iterator(iter);
                            self.heap.weak_edge_removed(iter);
                            let Value::Raised(error) = self.heap.get(exception) else {
                                unreachable!("raised iteration error");
                            };
                            if self.is_builtin_error(error, "StopAsyncIteration") {
                                continue;
                            }
                            Transfer::Raise(exception)
                        }
                        Instruction::EndFinally => cursor
                            .protected
                            .pop()
                            .expect("active finally")
                            .pending
                            .expect("pending transfer"),
                        Instruction::Break(destination) | Instruction::Jump(destination) => {
                            Transfer::Jump {
                                destination: *destination,
                                pop_iter: matches!(op, Instruction::Break(_)),
                            }
                        }
                        _ => match self.source_step(
                            &code,
                            owner,
                            cursor.comprehension.map_or(scope, |(scope, _)| scope),
                            op,
                            &mut cursor,
                        ) {
                            Ok(Some(Step::Return(value))) => Transfer::Return(value),
                            Ok(Some(step)) => return Ok(step),
                            Ok(None) => continue,
                            Err(mut error) => {
                                let reraising = matches!(op, Instruction::Raise(s) if s.exc.is_none())
                                    && self.heap.active_exception() != NONE;
                                if !reraising {
                                    self.record_exception_frame(&mut error, None);
                                }
                                if cursor.protected.is_empty() {
                                    return Err(error);
                                }
                                Transfer::Raise(self.raised_instance(error))
                            }
                        },
                    }
                } else {
                    Transfer::Return(NONE)
                };
                if let Some(step) = self.source_transfer(&mut cursor, transfer)? {
                    return Ok(step);
                }
            }
        })();
        let (value, returning) = match &result {
            Ok(Step::Return(v)) => (*v, true),
            Ok(Step::Yield(v, _)) => (*v, false),
            // The parent remains visible while an iterated source child executes.
            Ok(Step::Advance(..)) => return result,
            Ok(Step::Rejected(..)) | Err(_) => (NONE, false),
        };
        self.leave_frame(value, returning);
        result
    }

    /// Release the old binding before the next generator target or instruction can observe it.
    pub(super) fn bind_source_name(&mut self, scope: Id, name: &str, value: Id) -> Result<()> {
        if self.store_source_name(scope, name, value)? && self.heap.has_unbound_finalizer(NONE) {
            self.finish_binding_release();
        }
        Ok(())
    }

    /// Publish a binding and report whether its previous value could own a buffered writer.
    pub(super) fn store_source_name(&mut self, scope: Id, name: &str, value: Id) -> Result<bool> {
        let value = self.enum_assignment(scope, name, value)?;
        let replaced = self.replacing_finalizer(scope, name, value);
        let scope = self.binding_scope(scope, name);
        let frame = self.scope_data(scope);
        let target = if let Some(locals) = frame.exec_locals.or(frame.class_body_mapping()) {
            if matches!(self.heap.get(locals), Value::List(_)) {
                return Err(Error::new(
                    "TypeError",
                    "list indices must be integers or slices, not str",
                ));
            }
            locals
        } else if frame.parent.is_none()
            && let Some(namespace) = frame.binding_namespace(name)
        {
            self.dict_storage(namespace)
                .expect("module globals dictionary")
        } else {
            self.bind(scope, name, value);
            return Ok(replaced);
        };
        let key = self.heap.code_name(name);
        self.put_item(target, key, value)?;
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

    /// Return the imported object and the hook that processed its fromlist.
    fn import_source_module(
        &mut self,
        owner: Id,
        scope: Id,
        module: &str,
        names: Option<&[(String, String)]>,
        level: u32,
    ) -> Result<Id> {
        let mut root = scope;
        while let Some(parent) = self.scope_data(root).parent {
            root = parent;
        }
        let globals = self.module_globals(root)?;
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
        let level = self.int(i64::from(level));
        let locals = if self.code_object(owner).parameters == NONE {
            self.scope_data(scope).exec_locals.unwrap_or(globals)
        } else {
            NONE
        };
        self.call(
            importer,
            vec![modname, globals, locals, fromlist, level],
            Vec::new(),
        )
    }

    /// Keep the iterator's backing sequence available when the final pull drops it.
    fn source_loop_owner(&mut self, iterator: Id) -> Id {
        if !self.heap.tracks_finalizers() {
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
        if self.heap.tracks_finalizers() && self.heap.could_own_finalizer(owner, true) {
            self.heap.release_unbound_finalizers();
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
            sent,
            thrown,
            ..
        } = cursor;
        let eval = |ctx: &mut Self, e: &ast::Expr| ctx.source_expr(code, scope, e);
        match op {
            Instruction::Annotations {
                entries,
                scope: annotation_scope,
            } => {
                let values = self.source_annotations(code, scope, entries, *annotation_scope)?;
                return Ok(Some(Step::Return(values)));
            }
            Instruction::ReturnSent => return Ok(Some(Step::Return(*sent))),
            Instruction::BindSent(targets) => {
                for target in targets {
                    self.bind_target(code, scope, target, *sent)?;
                }
                *sent = NONE;
            }
            Instruction::Eval(e) | Instruction::Discard(e) => {
                let value = eval(self, e)?;
                if code.interactive && matches!(op, Instruction::Eval(_)) {
                    self.display_expression(value)?;
                }
                self.heap.note_discarded_coroutine(value);
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
            Instruction::DeleteName(name) => {
                self.check_source_local(code, scope, name)?;
                let key = self.heap.code_name(name);
                self.op("delname", scope, key, NONE)?;
            }
            Instruction::DeleteItem(item) => {
                let object = eval(self, &item.value)?;
                let key = eval(self, &item.slice)?;
                self.delete_item(object, key)?;
            }
            Instruction::DeleteAttr(attr) => {
                let object = eval(self, &attr.value)?;
                self.delete_attr(object, attr.attr.as_str())?;
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
            Instruction::Match {
                subject,
                cases,
                end,
            } => {
                let match_line = code.instructions[*ip - 1].0;
                let mark = self.heap.temporary_mark();
                let subject = eval(self, subject)?;
                self.discard_temporaries(mark, Some(subject));
                let subject_mark = self.heap.temporary_mark();
                let mut target = *end;
                // CPython compiles a single trailing wildcard as the default suite.
                let default = cases
                    .last()
                    .is_some_and(|case| imre_ast::wildcard_pattern(&case.pattern));
                let subject_cases = cases.len() - usize::from(default);
                for (index, case) in cases.iter().enumerate() {
                    self.heap.set_statement_line(match_line + case.line_offset);
                    // Keep a separate subject only if a later case still needs it.
                    let live = if index + 1 < subject_cases {
                        subject_mark
                    } else {
                        mark
                    };
                    let operand = if index < subject_cases { subject } else { NONE };
                    self.heap.reserve_temporaries(case.captures.len() + 1)?;
                    let captures = std::iter::repeat_n(NONE, case.captures.len());
                    self.discard_temporaries(live, captures.chain([operand]));
                    if self.match_source_attempt(
                        code,
                        scope,
                        &case.pattern,
                        operand,
                        live..live + case.captures.len(),
                    )? {
                        // A failed guard retains bindings from a successful pattern.
                        for (slot, name) in case.captures.iter().enumerate() {
                            let value = *self.heap.temporary_root(live + slot);
                            self.bind_source_name(scope, name, value)?;
                        }
                        self.discard_truth(live);
                        if let Some((offset, guard)) = &case.guard {
                            self.heap.set_statement_line(match_line + *offset);
                            if !self.source_condition(code, scope, guard)? {
                                continue;
                            }
                        }
                        target = case.body;
                        break;
                    }
                }
                self.discard_truth(mark);
                *ip = target;
            }
            Instruction::Unless(e, destination) => {
                if !self.source_condition(code, scope, e)? {
                    *ip = *destination;
                }
            }
            Instruction::AsyncWithMethods(context) => {
                cursor
                    .protected
                    .try_reserve(1)
                    .map_err(|_| crate::Error::allocation())?;
                iterators
                    .try_reserve(2)
                    .map_err(|_| crate::Error::allocation())?;
                let manager = match context {
                    Some(expr) => eval(self, expr)?,
                    None => *sent,
                };
                let exit = self.context_method(manager, "__aexit__")?;
                self.heap.keep(exit);
                *sent = self.context_method(manager, "__aenter__")?;
                // Keep the bound exit below the delegate while __aenter__ is suspended.
                iterators.push(exit);
                self.heap.weak_edge_removed(manager);
                self.heap.release_unbound_finalizers();
            }
            Instruction::CallAsyncEnter => {
                let enter = *sent;
                *sent = self.call(enter, Vec::new(), Vec::new())?;
                self.heap.weak_edge_removed(enter);
                self.heap.release_unbound_finalizers();
            }
            Instruction::AwaitWithResult(method) => {
                let value = std::mem::replace(sent, NONE);
                iterators.push(self.await_iterator(value, Some(method))?);
                self.heap.weak_edge_removed(value);
                self.heap.release_unbound_finalizers();
            }
            Instruction::EnterAsyncWith {
                target,
                handler,
                end,
            } => {
                let exit = iterators.pop().expect("saved asynchronous context exit");
                cursor.protected.push(FinallyBlock {
                    start: *ip,
                    handler: *handler,
                    end: *end,
                    depth: iterators.len(),
                    pending: None,
                    kind: HandlerKind::With(exit),
                });
                if let Some(target) = target {
                    self.bind_target(code, scope, target, *sent)?;
                }
            }
            Instruction::EnterWith { item, handler } => {
                cursor
                    .protected
                    .try_reserve(1)
                    .map_err(|_| crate::Error::allocation())?;
                let manager = eval(self, &item.context_expr)?;
                let exit = self.context_method(manager, "__exit__")?;
                self.heap.keep(exit);
                let enter = self.context_method(manager, "__enter__")?;
                let value = self.call(enter, Vec::new(), Vec::new())?;
                // Binding belongs to the protected suite: a failed unpack must invoke __exit__.
                cursor.protected.push(FinallyBlock {
                    start: *ip,
                    handler: *handler,
                    end: handler + 2,
                    depth: iterators.len(),
                    pending: None,
                    kind: HandlerKind::With(exit),
                });
                if let Some(target) = &item.optional_vars {
                    self.bind_target(code, scope, target, value)?;
                }
            }
            Instruction::ExitWith
            | Instruction::CallAsyncExit
            | Instruction::TestAsyncExit
            | Instruction::FinishAsyncExit => {
                let block = cursor.protected.last_mut().expect("active context manager");
                let HandlerKind::With(exit) = block.kind else {
                    unreachable!("context manager exit");
                };
                let exception = match block.pending {
                    Some(Transfer::Raise(error)) => error,
                    _ => NONE,
                };
                let suppress = match op {
                    Instruction::CallAsyncExit => {
                        let (class, tb) = if exception == NONE {
                            (NONE, NONE)
                        } else {
                            (
                                self.exact_type(exception)?,
                                self.exception_traceback(exception)?,
                            )
                        };
                        *sent = self.call(exit, vec![class, exception, tb], Vec::new())?;
                        if exception == NONE {
                            // Only exceptional cleanup retains its callable during the await and truth test.
                            block.kind = HandlerKind::With(NONE);
                            self.heap.weak_edge_removed(exit);
                            self.heap.release_unbound_finalizers();
                        }
                        return Ok(None);
                    }
                    Instruction::TestAsyncExit => {
                        let result = *sent;
                        *sent = if exception != NONE && self.truth(result)? {
                            TRUE
                        } else {
                            FALSE
                        };
                        // Release the result while a raised body exception is still active.
                        self.heap.weak_edge_removed(result);
                        self.heap.release_unbound_finalizers();
                        return Ok(None);
                    }
                    Instruction::FinishAsyncExit => *sent == TRUE,
                    _ => {
                        let active = self.heap.active_exception();
                        let result = self.op("exit_context", exit, exception, active)?;
                        exception != NONE && result == TRUE
                    }
                };
                if suppress {
                    block.pending = Some(Transfer::Jump {
                        destination: block.end,
                        pop_iter: false,
                    });
                }
            }
            Instruction::BindException(name) => {
                self.bind_source_name(scope, name, self.heap.active_exception())?;
            }
            Instruction::ClearException(name) => {
                self.bind_source_name(scope, name, NONE)?;
                let key = self.heap.code_name(name);
                self.op("delname", scope, key, NONE)?;
            }
            Instruction::ExceptMatch(class, next_handler) => {
                let class = eval(self, class)?;
                let base = self.builtin("BaseException");
                let items = self.tuple_items(class)?;
                let classes = items.as_deref().unwrap_or(std::slice::from_ref(&class));
                if !classes
                    .iter()
                    .all(|&target| self.direct_subclass(target, base))
                {
                    return Err(Error::new(
                        "TypeError",
                        "catching classes that do not inherit from BaseException is not allowed",
                    ));
                }
                let Value::Raised(error) = self.heap.get(self.heap.active_exception()) else {
                    unreachable!("matching an active exception")
                };
                // Python except clauses use the actual inheritance chain, ignoring ABC registration.
                if !classes.iter().any(|&target| match error.class() {
                    Some(actual) => self.direct_subclass(actual, target),
                    None => self.raised_error_matches(error, target),
                }) {
                    *ip = *next_handler;
                }
            }
            Instruction::EnterFinally { .. }
            | Instruction::EnterExcept { .. }
            | Instruction::StartFinally
            | Instruction::EndFinally
            | Instruction::EndAsyncFor
            | Instruction::Jump(_)
            | Instruction::Break(_) => unreachable!("source control instruction"),
            Instruction::Iter(expr, is_async) => {
                let iterable = match expr {
                    Some(expr) => eval(self, expr)?,
                    None => std::mem::replace(sent, NONE),
                };
                iterators.push(if *is_async {
                    self.async_for_iter(iterable)?
                } else {
                    self.iterator(iterable)?
                });
            }
            Instruction::EnterComprehension { kind, targets, end } => {
                let first = iterators.pop().expect("outer iterator");
                let collection = self.op(kind, NONE, NONE, NONE)?;
                let child = self.comprehension_scope_with_targets(scope, targets.clone());
                let depth = iterators.len();
                iterators.extend([collection, first]);
                cursor.protected.push(FinallyBlock {
                    start: *ip,
                    handler: *end,
                    end: *end,
                    depth,
                    pending: None,
                    kind: HandlerKind::Comprehension(child),
                });
                cursor.comprehension = Some((child, depth));
            }
            Instruction::CollectItem { value, keyed } => {
                let (_, depth) = cursor.comprehension.expect("active comprehension");
                let collection = iterators[depth];
                let item = match value {
                    Some(expr) => eval(self, expr)?,
                    None => *sent,
                };
                if *keyed {
                    let key = iterators.pop().expect("dictionary key");
                    self.put_item(collection, key, item)?;
                } else {
                    self.op("append", collection, item, NONE)?;
                }
            }
            Instruction::LeaveComprehension => {
                *sent = iterators.pop().expect("comprehension result");
                cursor.protected.pop().expect("comprehension scope");
                cursor.comprehension = SourceCursor::active_comprehension(cursor.protected);
            }
            Instruction::AsyncNext => {
                let iter = *iterators.last().expect("async iterator");
                iterators.push(self.async_for_next(iter)?);
            }
            Instruction::BindAsyncItem(target) => {
                cursor.protected.pop().expect("completed async for pull");
                if let Some(target) = target {
                    self.bind_target(code, scope, target, *sent)?;
                }
            }
            Instruction::BindOperand(target) => {
                let value = iterators.pop().expect("target value");
                self.bind_target(code, scope, target, value)?;
            }
            Instruction::UnpackTarget { count, star } => {
                let value = iterators.pop().expect("target value");
                let items = self.unpack_items(value, *count, *star)?;
                iterators.extend(items.into_iter().rev());
            }
            Instruction::StoreAttribute(attr) => {
                let object = iterators.pop().expect("attribute receiver");
                let value = iterators.pop().expect("target value");
                self.set_attr(object, attr.as_str(), value)?;
            }
            Instruction::StoreSubscript => {
                let key = iterators.pop().expect("subscript key");
                let object = iterators.pop().expect("subscript receiver");
                let value = iterators.pop().expect("target value");
                self.put_item(object, key, value)?;
            }
            Instruction::Await(expr) => {
                let value = match expr {
                    Some(expr) => eval(self, expr)?,
                    None => std::mem::replace(sent, NONE),
                };
                iterators.push(self.await_iterator(value, None)?);
            }
            Instruction::SaveOperand(expr) => {
                let value = match expr {
                    Some(expr) => eval(self, expr)?,
                    None => *sent,
                };
                iterators.push(value);
            }
            Instruction::BuildSequence { tuple, count } => {
                let items = iterators.split_off(iterators.len() - count);
                *sent = self.alloc(if *tuple {
                    Value::Tuple(items)
                } else {
                    Value::List(items)
                });
            }
            Instruction::LoadResult(expr) => *sent = eval(self, expr)?,
            Instruction::StoreNamed(name) => {
                let mark = self.heap.temporary_mark();
                self.store_named_result(scope, name.as_str(), *sent, mark)?;
            }
            Instruction::BuildSlice(present) => {
                let mut parts = [NONE; 3];
                for (part, exists) in parts.iter_mut().zip(present).rev() {
                    if *exists {
                        *part = iterators.pop().expect("saved slice bound");
                    }
                }
                *sent = self.alloc(Value::Slice(parts));
            }
            Instruction::StartDict => iterators.push(self.dict(Vec::new())),
            Instruction::StartSet(count) => {
                let keys = iterators.split_off(iterators.len() - count);
                let mark = self.heap.temporary_mark();
                let set = self.build_source_set(mark, keys)?;
                iterators.push(set);
            }
            Instruction::InsertDict { unpack } => {
                if *unpack {
                    let dict = *iterators.last().expect("active dict display");
                    self.merge_mapping(dict, *sent, calls::MappingOperation::Unpack)?;
                } else {
                    let key = iterators.pop().expect("saved dictionary key");
                    let dict = *iterators.last().expect("active dict display");
                    self.put_item(dict, key, *sent)?;
                }
            }
            Instruction::InsertSet { unpack } => {
                let set = *iterators.last().expect("active set display");
                if *unpack {
                    builtins::update_set(self, set, *sent)?;
                } else {
                    builtins::set_add(self, set, *sent)?;
                }
            }
            Instruction::FinishDisplay => *sent = iterators.pop().expect("active display"),
            Instruction::Unary(op) => *sent = self.source_unary(*op, *sent)?,
            Instruction::Attribute { name, method } => {
                *sent = if *method {
                    self.source_method(*sent, name.as_str())?
                } else {
                    self.lookup_attr(*sent, name.as_str())?
                };
            }
            Instruction::JumpResult(end) => *ip = *end,
            Instruction::Branch {
                value,
                when,
                usage,
                end,
            } => {
                let truth = match value {
                    Some(expr) if *usage == BranchUse::Operand => {
                        let (value, known) = self.source_bool_operand(code, scope, expr)?;
                        *sent = value;
                        known.map_or_else(|| self.truth(value), Ok)?
                    }
                    Some(expr) => self.source_condition(code, scope, expr)?,
                    None => self.truth(*sent)?,
                };
                if truth == *when {
                    *ip = *end;
                }
            }
            Instruction::NextComparison { end, condition } => {
                if !self.truth(*sent)? {
                    let right = iterators.pop().expect("middle operand");
                    self.heap.weak_edge_removed(right);
                    self.heap.release_unbound_finalizers();
                    if *condition {
                        *sent = FALSE;
                    }
                    *ip = *end;
                }
            }
            Instruction::Binary(op, range, keep_right) => {
                let right = iterators.pop().expect("saved right operand");
                let left = iterators.pop().expect("saved left operand");
                let previous = self
                    .heap
                    .replace_expression(value::ExpressionPosition::Source {
                        offset: range.start(),
                        range: *range,
                    });
                let result = self.op(op, left, right, NONE);
                self.heap.replace_expression(previous);
                if *keep_right && result.is_ok() {
                    iterators.push(right);
                } else {
                    self.heap.weak_edge_removed(right);
                }
                self.heap.weak_edge_removed(left);
                self.heap.release_unbound_finalizers();
                *sent = result?;
            }
            Instruction::SaveCallMethod(field) => {
                let method = self.source_call_method(code, scope, field)?;
                iterators.push(method);
            }
            Instruction::CallSaved {
                positional,
                keywords,
                position,
            } => {
                let start = iterators.len() - (1 + positional + keywords.len());
                let (pos, named) = iterators[start + 1..].split_at(*positional);
                let kwargs = keywords
                    .iter()
                    .map(ToString::to_string)
                    .zip(named.iter().copied())
                    .collect();
                let previous = self.heap.replace_expression(*position);
                let result = self.call_in_scope(scope, iterators[start], pos.to_vec(), kwargs);
                self.heap.replace_expression(previous);
                for operand in iterators.drain(start..).rev() {
                    self.heap.weak_edge_removed(operand);
                }
                self.heap.release_unbound_finalizers();
                *sent = self.check_call_return(result?)?;
            }
            Instruction::Next(_, _) | Instruction::Delegate => {
                let delegated = matches!(op, Instruction::Delegate);
                let iter = *iterators.last().expect("active source iterator");
                let owner = self.source_loop_owner(iter);
                let item = if let Some(outcome) = advanced.take() {
                    outcome?
                } else if thrown.is_none()
                    && code.kind != FunctionKind::Plain
                    && let Value::Generator { code, .. } = self.heap.get(iter)
                    && !matches!(self.code_object(*code).body, Body::Native(_))
                {
                    return Ok(Some(Step::Advance(
                        iter,
                        *ip - 1,
                        if delegated { *sent } else { NONE },
                    )));
                } else if delegated {
                    let input = match thrown.take() {
                        Some((error, args)) => GeneratorInput::Throw { error, args },
                        None => GeneratorInput::Send(*sent),
                    };
                    self.advance_delegate(iter, input)?
                } else {
                    match self.next_item(iter)? {
                        Some(value) => DelegateOutcome::Yield(value),
                        None => DelegateOutcome::Done(NONE),
                    }
                };
                match item {
                    DelegateOutcome::Yield(value) => match op {
                        Instruction::Next(Some(target), _) => {
                            self.bind_target(code, scope, target, value)?
                        }
                        Instruction::Next(None, _) => *sent = value,
                        _ => return Ok(Some(Step::Yield(value, *ip - 1))),
                    },
                    DelegateOutcome::Done(value) => {
                        iterators.pop();
                        self.release_source_loop_owner(owner);
                        if let Instruction::Next(_, end) = op {
                            *ip = *end;
                        } else {
                            *sent = value;
                        }
                    }
                    DelegateOutcome::LookupFailed(error) => {
                        return Ok(Some(Step::Rejected(error, *ip - 1)));
                    }
                }
            }
            Instruction::MakeGenerator(expr) => {
                *sent = self.source_genexpr(code, scope, expr, Some(*sent))?;
            }
            Instruction::YieldResult => {
                let value = if code.kind == FunctionKind::AsyncGenerator {
                    self.wrap_async_generator_yield(*sent)
                } else {
                    *sent
                };
                return Ok(Some(Step::Yield(value, *ip)));
            }
            Instruction::Yield(e) | Instruction::Return(e) => {
                let v = e
                    .as_ref()
                    .map(|e| eval(self, e))
                    .transpose()?
                    .unwrap_or(NONE);
                return Ok(Some(if matches!(op, Instruction::Yield(_)) {
                    // An await suspends through Delegate. Only a value explicitly yielded by
                    // the async generator completes its pending __anext__ or asend operation.
                    let item = if code.kind == FunctionKind::AsyncGenerator {
                        self.wrap_async_generator_yield(v)
                    } else {
                        v
                    };
                    Step::Yield(item, *ip)
                } else {
                    Step::Return(v)
                }));
            }
            Instruction::Import { module, alias } => {
                let mut imported = self.import_source_module(owner, scope, module, None, 0)?;
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
            Instruction::From(module, level, names) => {
                let m = self.import_source_module(owner, scope, module, Some(names), *level)?;
                for (name, local) in names {
                    if name == "*" {
                        self.import_star(scope, m)?;
                        continue;
                    }
                    let name = self.text(name);
                    let v = self.op("from", m, name, NONE)?;
                    self.bind_source_name(scope, local, v)?;
                }
            }
            Instruction::StringAnnotation(name, expression) => {
                let key = self.heap.code_name(name);
                let spelling = self.source_expr(code, scope, expression)?;
                // LOAD_NAME for this implicit store skips enclosing function locals.
                let annotations = if code.class_attributes.is_some() {
                    self.lookup_class_local(scope, "__annotations__")?
                } else {
                    self.module_annotation_name(scope, "__annotations__")?
                };
                self.put_item(annotations, key, spelling)?;
            }
            Instruction::ModuleDoc(range, _) => {
                self.bind_source_name(scope, "__doc__", code.literals[range])?;
            }
            Instruction::ModuleAnnotations(provider) => {
                self.start_module_annotations(code, owner, scope, provider.as_ref())?;
            }
            Instruction::MarkAnnotation(index) => {
                let active = if code.class_attributes.is_some() {
                    let cell = self.source_conditional_cell(scope)?;
                    match self.heap.get(cell) {
                        Value::Cell(Some(active)) => *active,
                        _ => {
                            return Err(self.missing_binding(scope, scope, CONDITIONAL));
                        }
                    }
                } else {
                    self.module_annotation_name(scope, CONDITIONAL)?
                };
                let mut index = *index;
                for block in cursor.protected.iter() {
                    if let (HandlerKind::Finally(annotation_count), Some(Transfer::Raise(_))) =
                        (block.kind, block.pending)
                    {
                        index += annotation_count;
                    }
                }
                let index = self.int(index as i64);
                self.mark_annotation(active, index)?;
            }
            Instruction::ClassAnnotator(provider) => {
                let annotate = self.source_annotator(provider, owner, scope, scope)?;
                self.bind_source_name(scope, "__annotate_func__", annotate)?;
            }
            Instruction::CheckAnnotationFormat => {
                let format = self.lookup(scope, ".format")?;
                self.op("annotation_format", format, NONE, NONE)?;
            }
            Instruction::ClassStart => {
                self.enable_class_annotation_provider(scope);
                // Only postponed annotations need a visible mapping from the start of the suite.
                if !code.postponed_annotations
                    && self.scope_data(scope).class_body_mapping().is_none()
                {
                    let key = self.heap.code_name("__annotations__");
                    self.remove_binding(scope, "__annotations__", key)?;
                }
                let first = self.int(i64::from(code.first_line));
                self.store_class_metadata(scope, "__firstlineno__", first)?;
                if let Some(doc) = code.doc {
                    self.store_class_metadata(scope, "__doc__", code.literals[&doc])?;
                }
            }
            Instruction::ClassEnd => {
                let names = code
                    .class_attributes
                    .as_ref()
                    .expect("class body")
                    .iter()
                    .map(|name| self.heap.code_name(name))
                    .collect();
                let attrs = self.alloc(Value::Tuple(names));
                self.bind_source_name(scope, "__static_attributes__", attrs)?;
            }
            Instruction::Define(definition) | Instruction::ReturnClass(definition) => {
                let value = self.source_definition(code, owner, scope, *ip - 1, definition)?;
                if matches!(op, Instruction::ReturnClass(_)) {
                    return Ok(Some(Step::Return(value)));
                }
                self.bind_source_name(scope, &definition.binding, value)?;
            }
        }
        Ok(None)
    }

    /// Bind fresh PEP 695 parameters under their lexical (possibly mangled) names.
    fn source_parameter_scope(
        &mut self,
        child: &Program,
        outer: Id,
        params: &ast::TypeParams,
        class: bool,
    ) -> Result<Id> {
        let entries = params
            .iter()
            .map(|param| {
                let binding = self.heap.code_name(param.name().as_str());
                let name = self.heap.code_name(child.definition_spelling(param.name()));
                let declaration = match param {
                    ast::TypeParam::TypeVar(_) => name,
                    ast::TypeParam::TypeVarTuple(_) => super::typing::declared_pack(self, name),
                    ast::TypeParam::ParamSpec(_) => super::paramspec::declared(self, name),
                };
                (binding, declaration)
            })
            .collect();
        let names = self.dict(entries);
        super::typing::parameter_scope(self, outer, names, class)
    }

    /// Evaluate a definition's defaults and reuse its code object at this lexical site.
    fn source_definition(
        &mut self,
        code: &Program,
        owner: Id,
        scope: Id,
        site: usize,
        definition: &Definition,
    ) -> Result<Id> {
        let eval = |ctx: &mut Self, e: &ast::Expr| ctx.source_expr(code, scope, e);
        let mut decorators = Vec::new();
        for decorator in &definition.decorators {
            let value = eval(self, &decorator.expression)?;
            self.heap.keep(value);
            decorators.push(value);
        }
        let child = &definition.body;
        let defaults = self.dict(vec![]);
        let frame = self.scope_data(scope);
        let mut defining_scope = if frame.exec_locals.is_some() {
            frame.parent.expect("exec locals have a global frame")
        } else {
            scope
        };
        let mut type_params = NONE;
        let mut bases = None;
        let sig = match &definition.kind {
            DefinitionKind::TypeAlias(params) => {
                if let Some(params) = params {
                    defining_scope =
                        self.source_parameter_scope(child, defining_scope, params, false)?;
                    type_params = self.lookup(defining_scope, ".type_params")?;
                }
                let format = self.text(".format");
                let value_format = self.int(1);
                self.put_item(defaults, format, value_format)?;
                serde_json::json!({
                    "posonly": [".format"],
                    "scope_locals": [".format"],
                    "scope_locals_complete": true,
                })
            }
            DefinitionKind::ClassParameters(_) => serde_json::json!({}),
            DefinitionKind::Class {
                arguments: args,
                parameterized,
                ..
            } => {
                let mut values = Vec::new();
                let mut keywords = Vec::new();
                if let Some(args) = args {
                    values = self.source_sequence_items(code, scope, &args.args)?;
                    if !args.keywords.is_empty() {
                        let mapping =
                            self.source_keyword_mapping(code, scope, None, &args.keywords)?;
                        keywords = self.keyword_args(mapping)?;
                    }
                }
                if *parameterized {
                    values.push(self.lookup(defining_scope, ".generic_base")?);
                }
                bases = Some((self.alloc(Value::List(values)), keywords, *parameterized));
                let declared = |kind| {
                    child
                        .directives
                        .iter()
                        .filter_map(|(name, (directive, _))| (*directive == kind).then_some(name))
                        .collect::<BTreeSet<_>>()
                };
                serde_json::json!({
                    "class_globals": declared(ScopeDirective::Global),
                    "class_frame_reads": child.reads,
                    "scope_nonlocals": declared(ScopeDirective::Nonlocal),
                    "needs_class_cell": child.needs_class_cell,
                    "needs_dict_cell": child.needs_dict_cell,
                    "postponed_annotations": child.postponed_annotations,
                })
            }
            DefinitionKind::Function {
                parameters: p,
                type_params: params,
                captured_locks,
            } => {
                let mut sig = serde_json::json!({"posonly":[],"positional":[],"kwonly":[],"varargs":p.vararg.as_ref().map(|x|x.name.as_str()),"kwargs":p.kwarg.as_ref().map(|x|x.name.as_str())});
                if let Some(locks) = captured_locks {
                    sig["thread_captured_locks"] = serde_json::json!(locks);
                }
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
                // Defaults are evaluated above, before entering the type-parameter scope.
                if let Some(params) = params {
                    let class_cell = self.scope_data(defining_scope).class_cell;
                    defining_scope =
                        self.source_parameter_scope(child, defining_scope, params, false)?;
                    // The nearest __class__ binding also supplies zero-argument super().
                    let class_cell = if params
                        .iter()
                        .any(|param| param.name().as_str() == "__class__")
                    {
                        Some(self.capture_free_cell(defining_scope, "__class__"))
                    } else {
                        class_cell
                    };
                    let Value::Scope(frame) = self.heap.get_mut(defining_scope) else {
                        unreachable!()
                    };
                    frame.class_cell = class_cell;
                    type_params = self.lookup(defining_scope, ".type_params")?;
                }
                // A method that names the compiler's private cell shares it with the
                // class annotator, even if the prepared namespace has a homonymous entry.
                if self.scope_data(scope).class_body.is_some()
                    && self.source_captures_conditional(scope, defining_scope, child)
                {
                    let class_cell = self.scope_data(defining_scope).class_cell;
                    let parent = if defining_scope == scope {
                        self.scope_data(scope)
                            .parent
                            .expect("class definition scope")
                    } else {
                        defining_scope
                    };
                    let cell = self.source_conditional_cell(scope)?;
                    let mut names = vec![CONDITIONAL.into()];
                    let mut cells = vec![cell];
                    if child.method_class_cell.is_none()
                        && child.class_cell
                        && let Some(class_cell) = class_cell
                    {
                        names.push("__class__".into());
                        cells.push(class_cell);
                    }
                    defining_scope = self.closure_environment(parent, names, &cells);
                    let Value::Scope(frame) = self.heap.get_mut(defining_scope) else {
                        unreachable!()
                    };
                    frame.class_cell = class_cell;
                }
                if let Some(captures_class) = child.method_class_cell {
                    sig["method_class_cell"] = captures_class.into();
                }
                sig["class_cell"] = child.method_class_cell.unwrap_or(child.class_cell).into();
                sig["scope_locals"] = serde_json::json!(child.locals.iter().collect::<Vec<_>>());
                sig["scope_locals_complete"] = true.into();
                sig
            }
        };
        let code = if let Some((_, existing)) = self
            .code_object(owner)
            .nested
            .iter()
            .find(|(i, _)| *i == site)
        {
            *existing
        } else {
            let parameters = self.text(sig.to_string());
            let freevars = if let Some(captures_class) = child.method_class_cell {
                let enclosing = if defining_scope == scope {
                    self.scope_data(scope).parent
                } else {
                    Some(defining_scope)
                };
                self.method_freevars(
                    enclosing,
                    child.reads.iter().map(String::as_str),
                    captures_class,
                )
            } else if type_params != NONE
                || (defining_scope != scope && self.scope_data(scope).class_body.is_some())
            {
                self.enclosing_freevars(defining_scope, child.reads.iter().map(String::as_str))
            } else if (code.name != "<module>" || self.scope_data(scope).comprehension.is_some())
                && !matches!(
                    definition.kind,
                    DefinitionKind::Class {
                        needs_enclosing_frame: true,
                        ..
                    }
                )
                && !child.class_cell
            {
                // A function defined in a class captures an enclosing function's cells.
                let enclosing = if code.class_attributes.is_some()
                    && self.scope_data(scope).comprehension.is_none()
                {
                    self.scope_data(scope).parent.unwrap_or(scope)
                } else {
                    scope
                };
                self.enclosing_freevars(enclosing, child.reads.iter().map(String::as_str))
            } else {
                None
            };
            let id = self.box_source_closure(child.clone(), Some(parameters), freevars);
            let Value::Code(parent) = self.heap.get_mut(owner) else {
                unreachable!()
            };
            parent.nested.push((site, id));
            id
        };
        let mut value = self.source_function(code, defining_scope, defaults);
        if matches!(definition.kind, DefinitionKind::TypeAlias(_)) {
            let name = self.code_object(code).name;
            return self.op("type_alias", name, value, type_params);
        }
        if type_params != NONE {
            self.set_function_attr(value, "__type_params__", type_params)?;
        }
        if let Some(provider) = &definition.annotate {
            let annotate = self.source_annotator(provider, code, scope, defining_scope)?;
            self.set_function_attr(value, "__annotate__", annotate)?;
        }
        if let DefinitionKind::ClassParameters(params) = &definition.kind {
            let parameters = self.source_parameter_scope(child, defining_scope, params, true)?;
            if self.scope_data(scope).class_body.is_some() {
                let namespace = self.active_class_namespace(scope);
                self.bind(parameters, "__classdict__", namespace);
            }
            value = self.execute(code, parameters, FrameOwner::Function(value))?;
        }
        if let Some((bases, keywords, parameterized)) = bases {
            value = self.class(bases, value, keywords, parameterized)?;
        }
        for decorator in decorators.into_iter().rev() {
            value = self.call(decorator, vec![value], Vec::new())?;
        }
        Ok(value)
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

    fn source_call_method(
        &mut self,
        code: &Program,
        scope: Id,
        field: &ast::ExprAttribute,
    ) -> Result<Id> {
        let receiver = self.source_expr(code, scope, &field.value)?;
        self.source_method(receiver, field.attr.as_str())
    }

    fn source_unary(&mut self, op: ast::UnaryOp, value: Id) -> Result<Id> {
        let name = match op {
            ast::UnaryOp::UAdd => "pos",
            ast::UnaryOp::USub => "neg",
            op => op.as_str(),
        };
        self.op(name, value, NONE, NONE)
    }

    fn source_method(&mut self, receiver: Id, name: &str) -> Result<Id> {
        let method = self.lookup_method(receiver, name)?;
        // Evaluating an argument can delete the receiver's reference to the callable.
        self.heap.reserve_temporaries(1)?;
        self.heap.keep(method);
        Ok(method)
    }

    fn source_expr(&mut self, code: &Program, scope: Id, expr: &ast::Expr) -> Result<Id> {
        let position = imre_ast::expression_position(
            expr,
            &code.method_imports,
            code.private_names.as_deref(),
        );
        let previous = self
            .heap
            .replace_expression(value::ExpressionPosition::Source {
                offset: position,
                range: expr.range(),
            });
        let result = stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || {
            self.source_expr_inner(code, scope, expr)
        });
        self.heap.replace_expression(previous);
        let value = result?;
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
        if let ast::Expr::If(branch) = expr {
            return if self.source_condition(code, scope, &branch.test)? {
                Ok((self.source_expr(code, scope, &branch.body)?, None))
            } else {
                self.source_bool_operand(code, scope, &branch.orelse)
            };
        }
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

    /// Try a case or OR alternative with its own tentative capture slots.
    fn match_source_attempt(
        &mut self,
        code: &Program,
        scope: Id,
        pattern: &ast::Pattern,
        subject: Id,
        mut tentative: std::ops::Range<usize>,
    ) -> Result<bool> {
        let mark = tentative.start;
        let matched = self.match_source_pattern(code, scope, pattern, subject, &mut tentative);
        if matches!(matched, Ok(true)) {
            return matched;
        }
        // Unread operands unwind from the top, before tentative bindings in
        // traversal order. Each release can run Python or a weakref callback.
        let error = matched.as_ref().err();
        let pending = self.heap.temporary_mark();
        if self.heap.tracks_finalizers() || self.heap.has_weak_values {
            for root in error.into_iter().flat_map(|e| e.roots()) {
                self.heap.keep(root);
            }
            let top = self.heap.temporary_mark();
            for slot in (tentative.end..pending).rev().chain(mark..tentative.start) {
                let value = std::mem::replace(self.heap.temporary_root(slot), NONE);
                self.heap.keep(value);
                self.discard_truth(top);
            }
        }
        self.discard_temporaries(mark, error.into_iter().flat_map(|e| e.roots()));
        matched
    }

    fn match_source_pattern(
        &mut self,
        code: &Program,
        scope: Id,
        pattern: &ast::Pattern,
        subject: Id,
        slots: &mut std::ops::Range<usize>,
    ) -> Result<bool> {
        match pattern {
            ast::Pattern::MatchSingleton(p) => {
                let singleton = match p.value {
                    ast::Singleton::None => NONE,
                    ast::Singleton::False => FALSE,
                    ast::Singleton::True => TRUE,
                };
                Ok(subject == singleton)
            }
            ast::Pattern::MatchValue(p) => {
                let operand = self.heap.temporary_mark() - 1;
                let value = self.source_expr(code, scope, &p.value)?;
                let comparison = self.binary("==", subject, value);
                let comparison = self.release_with_result(operand, comparison)?;
                self.truth(comparison)
            }
            ast::Pattern::MatchOr(p) => {
                // Each attempt gets its own operand and tentative captures. Dropping a failed
                // attempt can run finalizers before the next alternative evaluates any Python.
                let mark = self.heap.temporary_mark();
                for arm in &p.patterns {
                    let order = code
                        .alternative_captures
                        .get(&arm.range())
                        .map(Vec::as_slice)
                        .unwrap_or_default();
                    self.heap.reserve_temporaries(order.len() + 1)?;
                    for _ in order {
                        self.heap.keep(NONE);
                    }
                    self.heap.keep(subject);
                    if self.match_source_attempt(
                        code,
                        scope,
                        arm,
                        subject,
                        mark..mark + order.len(),
                    )? {
                        for index in order {
                            let value = *self.heap.temporary_root(mark + index);
                            let slot = slots.next().expect("validated OR capture");
                            *self.heap.temporary_root(slot) = value;
                        }
                        self.discard_truth(mark);
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            ast::Pattern::MatchClass(p) => {
                let class = self.source_expr(code, scope, &p.cls)?;
                self.class_pattern_matches(subject, class)
            }
            ast::Pattern::MatchSequence(p) => {
                let patterns = &p.patterns;
                let operand = self.heap.temporary_mark() - 1;
                if !self
                    .pattern_flags(subject)
                    .has(collection_pattern::PatternFlags::SEQUENCE)
                {
                    return Ok(false);
                }
                let plan = imre_ast::sequence_pattern_plan(patterns);
                if let Some((op, required)) = plan.length {
                    let length = self.sequence_len(subject)?;
                    if length < required || (op == ast::CmpOp::Eq && length != required) {
                        return Ok(false);
                    }
                }
                let star = match plan.items {
                    imre_ast::SequencePatternItems::Ignore => return Ok(true),
                    imre_ast::SequencePatternItems::IndexAround(star) => {
                        // A starred wildcard indexes only the inspected elements. Each suffix
                        // sees the length after any earlier comparison or item lookup.
                        let mark = operand + 1;
                        for (i, child) in patterns
                            .iter()
                            .enumerate()
                            .filter(|(_, p)| !imre_ast::wildcard_pattern(p))
                        {
                            let index = if i < star {
                                i as i64
                            } else {
                                self.sequence_len(subject)? as i64 - (patterns.len() - i) as i64
                            };
                            let index = self.int(index);
                            let item = self.get_item(subject, index);
                            let item = self.release_with_result(mark, item)?;
                            let matched =
                                self.match_source_pattern(code, scope, child, item, slots)?;
                            self.discard_truth(mark);
                            if !matched {
                                return Ok(false);
                            }
                        }
                        return Ok(true);
                    }
                    imre_ast::SequencePatternItems::Unpack(star) => star,
                };
                let items = self.unpack_items(subject, patterns.len(), star)?;
                // Unpacking consumes this subject. Keep the next subpattern's operand on top.
                self.discard_temporaries(operand, items.iter().rev().copied());
                for (pattern, item) in patterns.iter().zip(items) {
                    let operand = self.heap.temporary_mark() - 1;
                    let matched = self.match_source_pattern(code, scope, pattern, item, slots)?;
                    self.discard_truth(operand);
                    if !matched {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
            ast::Pattern::MatchAs(ast::PatternMatchAs { name, .. })
            | ast::Pattern::MatchStar(ast::PatternMatchStar { name, .. }) => {
                if let ast::Pattern::MatchAs(p) = pattern
                    && let Some(inner) = &p.pattern
                {
                    // The alias retains the subject while its inner pattern consumes a copy.
                    self.heap.keep(subject);
                    if !self.match_source_pattern(code, scope, inner, subject, slots)? {
                        return Ok(false);
                    }
                }
                if name.is_some() {
                    // Validation records captures in the same left-to-right, inner-before-alias order.
                    let slot = slots.next().expect("validated capture");
                    *self.heap.temporary_root(slot) = subject;
                }
                Ok(true)
            }
            _ => unreachable!("validated runtime source pattern"),
        }
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

    /// A function-local read or deletion must not fall back to a same-named global.
    fn check_source_local(&self, code: &Program, scope: Id, name: &str) -> Result<()> {
        if code.class_attributes.is_none()
            && code.name != "<module>"
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
        Ok(())
    }

    /// Retain only an expression's result and the roots that preceded its evaluation.
    fn keep_source_result(&mut self, mark: usize, value: Id) -> Result<Id> {
        let collect = self.heap.discard_truth_operands(mark, self.modules);
        self.heap.reserve_temporaries(1)?;
        self.heap.keep(value);
        if collect {
            self.collect_unreachable();
        }
        Ok(value)
    }

    fn join_source_format(&mut self, parts: Vec<Id>) -> Result<Id> {
        // A lone field preserves the exact object returned by __format__.
        if let [part] = parts.as_slice() {
            return Ok(*part);
        }
        let empty = self.text("");
        let items = self.alloc(Value::List(parts));
        builtins::call(self, "str.join", &[empty, items], &[])
    }

    fn source_format_text(
        &mut self,
        code: &Program,
        range: ruff_text_size::TextRange,
        text: &str,
    ) -> Id {
        code.literals
            .get(&range)
            .copied()
            .unwrap_or_else(|| self.text(text))
    }

    fn source_format_elements(
        &mut self,
        code: &Program,
        scope: Id,
        elements: &[ast::InterpolatedStringElement],
        parts: &mut Vec<Id>,
    ) -> Result<()> {
        for element in elements {
            let part = match element {
                ast::InterpolatedStringElement::Literal(lit) if lit.value.is_empty() => continue,
                ast::InterpolatedStringElement::Literal(lit) => {
                    self.source_format_text(code, lit.range(), &lit.value)
                }
                ast::InterpolatedStringElement::Interpolation(field) => {
                    if let Some(debug) = &field.debug_text {
                        parts.push(self.text(debug.as_str()));
                    }
                    let mark = self.heap.temporary_mark();
                    let mut value = self.source_expr(code, scope, &field.expression)?;
                    let conversion = field.conversion.to_char().or_else(|| {
                        (field.debug_text.is_some() && field.format_spec.is_none()).then_some('r')
                    });
                    if let Some(conversion) = conversion {
                        value = builtins::format_conversion(self, value, conversion)?;
                        self.keep_source_result(mark, value)?;
                    }
                    let spec = if let Some(spec) = &field.format_spec {
                        let spec_mark = self.heap.temporary_mark();
                        let mut nested = Vec::new();
                        self.source_format_elements(code, scope, &spec.elements, &mut nested)?;
                        let spec = self.join_source_format(nested)?;
                        Some(self.keep_source_result(spec_mark, spec)?)
                    } else {
                        None
                    };
                    let rendered = builtins::format_value(self, value, spec)?;
                    self.keep_source_result(mark, rendered)?
                }
            };
            parts.push(part);
        }
        Ok(())
    }

    fn source_keyword_mapping(
        &mut self,
        code: &Program,
        scope: Id,
        callable: Option<Id>,
        keywords: &[ast::Keyword],
    ) -> Result<Id> {
        let merged = self.dict(Vec::new());
        self.heap.keep(merged);
        // Merge each ** mapping immediately, but evaluate adjacent named arguments as a group.
        for group in keywords.chunk_by(|left, right| left.arg.is_some() && right.arg.is_some()) {
            let mapping = if group[0].arg.is_none() {
                self.source_expr(code, scope, &group[0].value)?
            } else {
                let named = self.dict(Vec::new());
                self.heap.keep(named);
                for item in group {
                    let key = self
                        .heap
                        .code_name(item.arg.as_ref().expect("named keyword").as_str());
                    let value = self.source_expr(code, scope, &item.value)?;
                    self.put_item(named, key, value)?;
                }
                named
            };
            self.merge_mapping(
                merged,
                mapping,
                super::calls::MappingOperation::Keywords(callable),
            )?;
        }
        Ok(merged)
    }

    fn source_sequence_items(
        &mut self,
        code: &Program,
        scope: Id,
        elements: &[ast::Expr],
    ) -> Result<Vec<Id>> {
        let mut items = Vec::with_capacity(elements.len());
        for item in elements {
            if let ast::Expr::Starred(star) = item {
                let mark = self.heap.temporary_mark();
                let first = items.len();
                let expanded = self
                    .source_expr(code, scope, &star.value)
                    .and_then(|iterable| self.extend_source_items(&mut items, iterable, None));
                // Only yielded values outlive this operand. Release the iterator before
                // evaluating the next element or dispatching a weakref callback.
                let collect = self.heap.discard_truth_operands(mark, self.modules);
                match &expanded {
                    Ok(()) => {
                        for &value in &items[first..] {
                            self.heap.keep(value);
                        }
                    }
                    Err(error) => {
                        for root in error.roots() {
                            self.heap.keep(root);
                        }
                    }
                }
                if collect {
                    self.collect_unreachable();
                }
                expanded?;
            } else {
                items.push(self.source_expr(code, scope, item)?);
            }
        }
        Ok(items)
    }

    fn source_set_display(
        &mut self,
        code: &Program,
        scope: Id,
        elements: &[ast::Expr],
    ) -> Result<Id> {
        let prefix_len = set_display_prefix(elements);
        let (prefix, rest) = elements.split_at(prefix_len);
        let mark = self.heap.temporary_mark();
        let mut keys = Vec::with_capacity(prefix_len);
        for expr in prefix {
            let operand = self.heap.temporary_mark();
            let key = self.source_expr(code, scope, expr)?;
            keys.push(self.keep_source_result(operand, key)?);
        }
        let result = self.build_source_set(mark, keys)?;
        for expr in rest {
            let operand = self.heap.temporary_mark();
            if let ast::Expr::Starred(star) = expr {
                let iterable = self.source_expr(code, scope, &star.value)?;
                builtins::update_set(self, result, iterable)?;
            } else {
                let key = self.source_expr(code, scope, expr)?;
                builtins::set_add(self, result, key)?;
            }
            self.discard_truth(operand);
        }
        Ok(result)
    }

    fn build_source_set(&mut self, mark: usize, keys: Vec<Id>) -> Result<Id> {
        let result = self.alloc(Value::Set {
            entries: Default::default(),
            frozen: false,
        });
        // Pop keys in insertion order. A rejected duplicate may finalize before the next hash.
        self.heap.release_temporaries_since(mark);
        self.heap.keep(result);
        for &key in keys.iter().rev() {
            self.heap.keep(key);
        }
        let count = keys.len();
        for (index, key) in keys.into_iter().enumerate() {
            builtins::set_add(self, result, key)?;
            self.discard_truth(mark + count - index);
        }
        Ok(result)
    }

    fn extend_source_items(
        &mut self,
        args: &mut Vec<Id>,
        iterable: Id,
        sole_star_callable: Option<Id>,
    ) -> Result<()> {
        let iter = match self.iterator(iterable) {
            Err(error)
                if error.kind == "TypeError"
                    && self.slot(iterable, "__iter__").is_none()
                    && self.slot(iterable, "__getitem__").is_none() =>
            {
                let prefix = if let Some(callable) = sole_star_callable {
                    format!(
                        "{} argument after *",
                        self.keyword_callable(Some(callable))?
                    )
                } else {
                    "Value after *".into()
                };
                return Err(Error::new(
                    "TypeError",
                    format!(
                        "{prefix} must be an iterable, not {}",
                        self.type_name(iterable)
                    ),
                ));
            }
            other => other?,
        };
        self.heap.keep(iter);
        if sole_star_callable.is_none() {
            let default = self.int(8);
            let hint = self.length_hint(iterable, Some(default))?;
            let hint = self.as_integer(hint)? as usize;
            if hint <= isize::MAX as usize - args.len() {
                args.try_reserve(hint)
                    .map_err(|_| Error::new("MemoryError", ""))?;
            }
        }
        while let Some(item) = self.next_item(iter)? {
            args.try_reserve(1)
                .map_err(|_| Error::new("MemoryError", ""))?;
            // Further arguments may clear the iterable and invoke Python callbacks.
            self.heap.reserve_temporaries(1)?;
            self.heap.keep(item);
            args.push(item);
        }
        Ok(())
    }

    fn store_named_result(&mut self, scope: Id, name: &str, value: Id, mark: usize) -> Result<()> {
        if self.store_source_name(self.named_expression_scope(scope), name, value)?
            && self.heap.named_assignment(mark, value)
        {
            self.finish_binding_release();
        }
        Ok(())
    }

    fn source_expr_inner(&mut self, code: &Program, scope: Id, expr: &ast::Expr) -> Result<Id> {
        use ast::Expr;
        let eval = |ctx: &mut Self, e: &Expr| ctx.source_expr(code, scope, e);
        match expr {
            Expr::Lambda(_) if code.expression_functions.contains_key(&expr.range()) => {
                let (site, definition) = &code.expression_functions[&expr.range()];
                self.source_definition(
                    code,
                    self.heap.current_code(),
                    scope,
                    code.instructions.len() + site,
                    definition,
                )
            }
            Expr::Generator(_) if code.expression_functions.contains_key(&expr.range()) => {
                self.source_genexpr(code, scope, expr, None)
            }
            Expr::Name(n) => {
                let name = n.id.as_str();
                let mut enclosing = scope;
                while self.scope_data(enclosing).comprehension.is_some() {
                    let frame = self.scope_data(enclosing);
                    if frame.closure.declared.contains(name) {
                        return self.lookup(enclosing, name);
                    }
                    enclosing = frame.parent.expect("enclosing comprehension scope");
                }
                if scope == enclosing
                    && code.class_parameter_header()
                    && !code.locals.contains(name)
                    && let Some(namespace) = self.scope_name(scope, "__classdict__")
                {
                    match self.load_exec_name(namespace, name) {
                        Err(error) if self.is_builtin_error(&error, "KeyError") => {}
                        value => return value,
                    }
                }
                if scope == enclosing
                    && code.class_attributes.is_some()
                    && code.locals.contains(name)
                {
                    return self.lookup_class_local(scope, name);
                }
                // An explicitly class-local annotation name falls back to globals. The
                // separate private cell is still used to guard which expressions run.
                if name == CONDITIONAL
                    && code.class_binds_conditional_name
                    && self.scope_data(scope).exec_locals.is_some()
                {
                    return self.module_annotation_name(scope, name);
                }
                self.check_source_local(code, enclosing, name)?;
                self.lookup(scope, name)
            }
            Expr::Named(named) if code.has_named_bindings => {
                let Expr::Name(name) = named.target.as_ref() else {
                    return Err(unsupported("assignment expression target"));
                };
                let mark = self.heap.temporary_mark();
                let value = eval(self, &named.value)?;
                self.store_named_result(scope, name.id.as_str(), value, mark)?;
                Ok(value)
            }
            Expr::StringLiteral(_) | Expr::BytesLiteral(_) | Expr::NumberLiteral(_) => {
                Ok(code.literals[&expr.range()])
            }
            Expr::FString(fstring) => {
                let mark = self.heap.temporary_mark();
                let mut parts = Vec::new();
                for part in fstring.value.iter() {
                    match part {
                        ast::FStringPartRef::Literal(lit) if !lit.value.is_empty() => {
                            parts.push(self.source_format_text(code, lit.range(), &lit.value));
                        }
                        ast::FStringPartRef::Literal(_) => {}
                        ast::FStringPartRef::FString(part) => {
                            self.source_format_elements(code, scope, &part.elements, &mut parts)?;
                        }
                    }
                }
                let result = self.join_source_format(parts)?;
                self.keep_source_result(mark, result)
            }
            Expr::TString(template) => {
                let strings = code.literals[&expr.range()];
                let mut interpolations = Vec::new();
                for field in template
                    .value
                    .iter()
                    .flat_map(|s| s.elements.interpolations())
                {
                    let spec =
                        imre_ast::strings::template_format_spec(field).map_err(unsupported)?;
                    let spelling = code
                        .template_expressions
                        .get(&field.start())
                        .ok_or_else(|| unsupported("t-string expression spelling"))?;
                    let value = eval(self, &field.expression)?;
                    let expression = self.text(spelling);
                    let conversion = field
                        .conversion
                        .to_char()
                        .map_or(NONE, |c| self.text(c.to_string()));
                    let spec = match spec {
                        Some(lit) => code.literals[&lit.range()],
                        None => self.text(""),
                    };
                    interpolations
                        .push(self.template_interpolation(value, expression, conversion, spec)?);
                }
                Ok(self.template_from_literal(strings, interpolations))
            }
            Expr::NoneLiteral(_) => Ok(NONE),
            Expr::EllipsisLiteral(_) => Ok(value::ELLIPSIS),
            Expr::BooleanLiteral(v) => Ok(if v.value { TRUE } else { FALSE }),
            Expr::List(v) => {
                let items = self.source_sequence_items(code, scope, &v.elts)?;
                Ok(self.alloc(Value::List(items)))
            }
            Expr::Tuple(v) => {
                if let Some(&id) = code.literals.get(&v.range()) {
                    return Ok(id);
                }
                let items = self.source_sequence_items(code, scope, &v.elts)?;
                Ok(self
                    .heap
                    .packed_tuple(items, v.elts.iter().all(imre_ast::literal_tuple_element)))
            }
            expr if let Some(targets) = code.eager_comprehension_targets.get(&expr.range())
                && let Some(comp) = Plan::from_expr(expr) =>
            {
                self.source_eager_comprehension(code, scope, &comp, targets)
            }
            Expr::Set(display) => self.source_set_display(code, scope, &display.elts),
            Expr::Dict(v) => {
                let result = self.dict(vec![]);
                for item in &v.items {
                    if let Some(key) = &item.key {
                        let key = eval(self, key)?;
                        let value = eval(self, &item.value)?;
                        self.put_item(result, key, value)?;
                    } else {
                        let mapping = eval(self, &item.value)?;
                        self.merge_mapping(result, mapping, calls::MappingOperation::Unpack)?;
                    }
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
                self.source_unary(v.op, value)
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
                    self.source_call_method(code, scope, field)?
                } else {
                    eval(self, &v.func)?
                };
                let mut args = Vec::new();
                let mut sole_star = None;
                for arg in &v.arguments.args {
                    if let Expr::Starred(star) = arg {
                        let value = eval(self, &star.value)?;
                        if v.arguments.args.len() == 1 {
                            // CALL_FUNCTION_EX consumes a sole iterable after evaluating keywords.
                            sole_star = Some(value);
                        } else {
                            self.extend_source_items(&mut args, value, None)?;
                        }
                    } else {
                        args.push(eval(self, arg)?);
                    }
                }
                let keywords = &v.arguments.keywords;
                let mut kwargs = Vec::new();
                let mapping = if keywords.iter().any(|item| item.arg.is_none()) {
                    Some(self.source_keyword_mapping(code, scope, Some(f), keywords)?)
                } else {
                    for item in keywords {
                        let key = item.arg.as_ref().expect("named keyword");
                        kwargs.push((key.to_string(), eval(self, &item.value)?));
                    }
                    None
                };
                if let Some(value) = sole_star {
                    self.extend_source_items(&mut args, value, Some(f))?;
                }
                let result = if let Some(mapping) = mapping {
                    self.call_with_keyword_dict(scope, f, args, mapping)?
                } else {
                    self.call_in_scope(scope, f, args, kwargs)?
                };
                self.finish_call_operands(mark, result);
                self.check_call_return(result)
            }
            _ => Err(unsupported(format!(
                "{:?}",
                ast::AnyNodeRef::from(expr).kind()
            ))),
        }
    }
}
