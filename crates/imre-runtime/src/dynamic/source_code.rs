//! In-process execution of source received by the boxed Python runtime.

mod annotations;

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::rc::Rc;

use ruff_python_ast as ast;
use ruff_text_size::Ranged;

use super::value::{Body, ExecGlobals, Location};
use super::*;

pub(super) struct Program {
    pub filename: Rc<str>,
    pub name: String,
    qualname: String,
    nested_global: bool,
    class_attributes: Option<BTreeSet<String>>,
    class_cell: bool,
    class_nonlocal: bool,
    needs_class_cell: bool,
    needs_dict_cell: bool,
    postponed_annotations: bool,
    has_named_bindings: bool,
    doc: Option<ruff_text_size::TextRange>,
    strip_docstrings: bool,
    literals: HashMap<ruff_text_size::TextRange, Id>,
    pub(super) string_roots: Id,
    first_line: u32,
    kind: FunctionKind,
    interactive: bool,
    locals: HashSet<String>,
    directives: HashMap<String, (ScopeDirective, ruff_text_size::TextSize)>,
    reads: BTreeSet<String>,
    method_imports: Rc<std::collections::BTreeSet<String>>,
    private_names: Option<Rc<imre_ast::Names>>,
    template_expressions: Rc<HashMap<ruff_text_size::TextSize, String>>,
    instructions: Vec<(u32, Instruction)>,
    // Declarations collected across module suites while lowering.
    module_annotations: Vec<(String, ast::Expr)>,
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
        location: Location,
    ) -> Result<Id> {
        let mut program = Program::new(
            Rc::from(location.file.as_ref()),
            ctx.str_value(name)?,
            false,
        );
        program.qualname = ctx.str_value(qualname)?.to_owned();
        program.first_line = location.line;
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

enum Instruction {
    Eval(ast::Expr),
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
    MarkModuleAnnotation(usize),
    StringAnnotation(String, ast::Expr),
    ClassAnnotator(Rc<Program>),
    ClassStart,
    ClassEnd,
    Yield(Option<ast::Expr>),
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
    Iter(ast::Expr),
    Await(ast::Expr),
    Next(Option<ast::Expr>, usize),
    Jump(usize),
    Break(usize),
    EnterFinally {
        handler: usize,
        end: usize,
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
    pattern: SourcePattern,
    line_offset: u32,
    guard: Option<(u32, Box<ast::Expr>)>,
    body: usize,
}

enum SourcePattern {
    Value(Box<ast::Expr>),
    Class(Box<ast::Expr>),
    Wildcard,
}

struct Definition {
    binding: String,
    decorators: Vec<ast::Decorator>,
    body: Rc<Program>,
    kind: DefinitionKind,
    annotate: Option<Rc<Program>>,
}

enum DefinitionKind {
    TypeAlias(Vec<String>),
    Function(Box<ast::Parameters>),
    Class {
        arguments: Option<Box<ast::Arguments>>,
        parameterized: bool,
    },
    ClassParameters(Vec<ast::Identifier>),
}

/// Check the expressions executed inline or in a synchronous generator's deferred frame.
fn synchronous_comprehension(
    element: &ast::Expr,
    clauses: &[ast::Comprehension],
    deferred: bool,
) -> bool {
    use ast::visitor::{self, Visitor};
    struct Body<'a>(Vec<&'a ast::Expr>);
    impl<'a> Visitor<'a> for Body<'a> {
        fn visit_expr(&mut self, expr: &'a ast::Expr) {
            self.0.push(expr);
        }
    }
    let mut body = Body(vec![element]);
    for (index, clause) in clauses.iter().enumerate() {
        if clause.is_async {
            return false;
        }
        body.visit_expr(&clause.target);
        if !deferred || index != 0 {
            body.visit_expr(&clause.iter);
        }
        for condition in &clause.ifs {
            body.visit_expr(condition);
        }
    }
    while let Some(expr) = body.0.pop() {
        match expr {
            ast::Expr::Await(_) => return false,
            // A deferred assignment would need to update a binding outside its frame.
            ast::Expr::Named(_) if deferred => return false,
            ast::Expr::Lambda(lambda) => {
                if let Some(params) = &lambda.parameters {
                    body.visit_parameters(params);
                }
            }
            ast::Expr::Generator(nested) => body.visit_expr(&nested.generators[0].iter),
            ast::Expr::ListComp(c) if c.generators.iter().any(|g| g.is_async) => return false,
            ast::Expr::SetComp(c) if c.generators.iter().any(|g| g.is_async) => return false,
            ast::Expr::DictComp(c) if c.generators.iter().any(|g| g.is_async) => return false,
            _ => visitor::walk_expr(&mut body, expr),
        }
    }
    true
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
        if let ast::Expr::SetComp(ast::ExprSetComp {
            elt, generators, ..
        })
        | ast::Expr::ListComp(ast::ExprListComp {
            elt, generators, ..
        }) = expr
        {
            self.comprehensions.push(ComprehensionBindings::default());
            stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || {
                for clause in generators {
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
                self.visit_expr(elt);
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
    sent: Id,
    thrown: Option<(Error, Vec<Id>)>,
}

impl<'a> SourceCursor<'a> {
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
        Self {
            ip,
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
    Finally,
    Except,
    With(Id),
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
            HandlerKind::With(exit) => Some(exit),
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
    string_tuples: HashMap<Vec<Id>, Id>,
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

    /// Template strings and equivalent tuple displays load the same code constant.
    fn string_tuple(&mut self, range: ruff_text_size::TextRange, items: Vec<Id>) -> Result<()> {
        let id = if let Some(&id) = self.shared.string_tuples.get(&items) {
            id
        } else {
            self.shared
                .string_tuples
                .try_reserve(1)
                .map_err(|_| crate::Error::allocation())?;
            let mut tuple = Vec::new();
            tuple
                .try_reserve_exact(items.len())
                .map_err(|_| crate::Error::allocation())?;
            tuple.extend_from_slice(&items);
            let id = self.heap.packed_tuple(tuple, true);
            self.shared.string_tuples.insert(items, id);
            id
        };
        self.constant(range, id)
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
        self.string_tuple(template.range(), items)
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
                ast::Expr::ListComp(comp) => self.visit_expr(&comp.generators[0].iter),
                ast::Expr::SetComp(comp) => self.visit_expr(&comp.generators[0].iter),
                ast::Expr::DictComp(comp) => self.visit_expr(&comp.generators[0].iter),
                ast::Expr::Generator(comp) => self.visit_expr(&comp.generators[0].iter),
                _ => ast::visitor::walk_expr(self, expr),
            }
        }
        self.pending = roots;
        Ok(())
    }

    fn scan(&mut self) -> Result<(bool, Vec<ScopedExpression<'a>>)> {
        use ast::visitor::Visitor;
        let mut functions = Vec::new();
        if !self.directives.is_empty() {
            self.check_directive_reads()?;
        }
        let mut class_cell = false;
        while let Some((expr, bound)) = self.pending.pop() {
            self.bound = bound;
            match expr {
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
                ast::Expr::Generator(genexpr)
                    if synchronous_comprehension(&genexpr.elt, &genexpr.generators, true) =>
                {
                    functions
                        .try_reserve(1)
                        .map_err(|_| Error::new("MemoryError", ""))?;
                    functions.push((expr, self.bound.clone()));
                    self.visit_expr(&genexpr.generators[0].iter);
                    continue;
                }
                ast::Expr::SetComp(ast::ExprSetComp {
                    elt, generators, ..
                })
                | ast::Expr::ListComp(ast::ExprListComp {
                    elt, generators, ..
                }) => {
                    self.visit_expr(&generators[0].iter);
                    let mut names = HashSet::new();
                    for clause in generators {
                        Program::collect_target_names(&clause.target, &mut names)?;
                    }
                    if synchronous_comprehension(elt, generators, false) {
                        self.eager_comprehension_targets
                            .insert(expr.range(), names.clone());
                    }
                    if let Some(enclosing) = &self.bound {
                        names.extend(enclosing.iter().cloned());
                    }
                    self.bound = Some(Rc::new(names));
                    for (index, clause) in generators.iter().enumerate() {
                        self.visit_expr(&clause.target);
                        if index != 0 {
                            self.visit_expr(&clause.iter);
                        }
                        for predicate in &clause.ifs {
                            self.visit_expr(predicate);
                        }
                    }
                    self.visit_expr(elt);
                    continue;
                }
                ast::Expr::StringLiteral(literal) => {
                    let id = self.literal_text(literal)?;
                    self.constant(literal.range(), id)?;
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
                ast::Expr::Tuple(tuple)
                    if !tuple.elts.is_empty()
                        && tuple.elts.iter().all(ast::Expr::is_string_literal_expr) =>
                {
                    let mut items = Vec::new();
                    items
                        .try_reserve_exact(tuple.elts.len())
                        .map_err(|_| crate::Error::allocation())?;
                    for literal in tuple
                        .elts
                        .iter()
                        .filter_map(ast::Expr::as_string_literal_expr)
                    {
                        items.push(self.literal_text(literal)?);
                    }
                    self.string_tuple(tuple.range(), items)?;
                    continue;
                }
                ast::Expr::BytesLiteral(literal) => {
                    let id = self
                        .heap
                        .alloc(Value::Bytes(literal.value.bytes().collect()));
                    self.constant(literal.range(), id)?;
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
        annotation_class: Option<&HashSet<String>>,
    ) -> Result<(Id, &'a BTreeSet<String>)> {
        use ast::visitor::Visitor;
        let class_parameters = self.class_parameter_header();
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
        for (_, op) in &mut self.instructions {
            match op {
                Instruction::Eval(e)
                | Instruction::Unless(e, _)
                | Instruction::Iter(e)
                | Instruction::ExceptMatch(e, _)
                | Instruction::Await(e) => strings.visit_expr(e),
                Instruction::Match { subject, cases, .. } => {
                    strings.visit_expr(subject);
                    for case in cases {
                        match &case.pattern {
                            SourcePattern::Value(expr) | SourcePattern::Class(expr) => {
                                strings.visit_expr(expr)
                            }
                            SourcePattern::Wildcard => {}
                        }
                        if let Some((_, guard)) = &case.guard {
                            strings.visit_expr(guard);
                        }
                    }
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
                    if let Some(provider) = &mut definition.annotate {
                        nested_reads.extend(
                            strings
                                .annotation_code(provider, class_locals, line_at)?
                                .iter()
                                .cloned(),
                        );
                    }
                    let child =
                        Rc::get_mut(&mut definition.body).expect("newly lowered child code");
                    strings.heap.code_name(&definition.binding);
                    if let DefinitionKind::ClassParameters(params) = &definition.kind {
                        child.locals.extend(params.iter().map(ToString::to_string));
                    }
                    let (roots, reads) =
                        child.prepare_strings(strings.heap, strings.shared, spellings, line_at)?;
                    if let DefinitionKind::ClassParameters(params) = &definition.kind {
                        nested_reads.extend(
                            reads
                                .iter()
                                .filter(|name| {
                                    !params.iter().any(|param| param.as_str() == name.as_str())
                                })
                                .cloned(),
                        );
                    } else {
                        nested_reads.extend(reads.iter().cloned());
                    }
                    if roots != NONE {
                        strings.keep(roots)?;
                    }
                    for decorator in &definition.decorators {
                        strings.visit_expr(&decorator.expression);
                    }
                    match &definition.kind {
                        DefinitionKind::Function(parameters) => {
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
                | Instruction::MarkModuleAnnotation(_)
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
                | Instruction::Jump(_)
                | Instruction::Break(_)
                | Instruction::EnterFinally { .. }
                | Instruction::AsyncWithMethods(None)
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
        let (class_cell, functions) = strings.scan()?;
        self.class_cell |= class_cell;
        self.expression_functions
            .try_reserve(functions.len())
            .map_err(|_| Error::new("MemoryError", ""))?;
        let mut comprehension_sites = HashMap::new();
        for (site, (expr, bound)) in functions.into_iter().enumerate() {
            if let Some(names) = bound {
                comprehension_sites.insert(expr.range(), names);
            }
            let name = if matches!(expr, ast::Expr::Lambda(_)) {
                "<lambda>"
            } else {
                "<genexpr>"
            };
            let mut child = Self::new(self.filename.clone(), name, self.has_named_bindings);
            child.strip_docstrings = self.strip_docstrings;
            child.qualname =
                Self::qualified_name(&self.qualname, self.class_attributes.is_some(), name);
            child.method_imports = self.method_imports.clone();
            child.template_expressions = self.template_expressions.clone();
            child.private_names = self.private_names.clone();
            child.first_line = line_at(expr.start().to_usize());
            let params = match expr {
                ast::Expr::Lambda(lambda) => {
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
                ast::Expr::Generator(genexpr) => child.lower_genexpr(genexpr, line_at)?,
                _ => unreachable!("anonymous function expression"),
            };
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
                        kind: DefinitionKind::Function(params),
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
            self.reads
                .retain(|name| name == "__classdict__" || !class.contains(name));
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
        Ok((self.string_roots, &self.reads))
    }

    /// Lower a generator expression to resumable for loops. Its frame already
    /// owns the outer iterator; only inner clauses evaluate Iter instructions.
    fn lower_genexpr(
        &mut self,
        genexpr: &ast::ExprGenerator,
        line_at: &impl Fn(usize) -> u32,
    ) -> Result<Box<ast::Parameters>> {
        self.kind = FunctionKind::Generator;
        let mut heads = Vec::new();
        for (index, clause) in genexpr.generators.iter().enumerate() {
            self.declare_target(&clause.target)?;
            let line = line_at(clause.target.start().to_usize());
            if index != 0 {
                self.instructions
                    .push((line, Instruction::Iter(clause.iter.clone())));
            }
            let head = self.instructions.len();
            heads.push(head);
            self.instructions
                .push((line, Instruction::Next(Some(clause.target.clone()), 0)));
            for predicate in &clause.ifs {
                self.instructions.push((
                    line_at(predicate.start().to_usize()),
                    Instruction::Unless(predicate.clone(), head),
                ));
            }
        }
        self.instructions.push((
            line_at(genexpr.elt.start().to_usize()),
            Instruction::Yield(Some(*genexpr.elt.clone())),
        ));
        for head in heads.into_iter().rev() {
            self.instructions
                .push((self.instructions[head].0, Instruction::Jump(head)));
            let end = self.instructions.len();
            let Instruction::Next(_, target) = &mut self.instructions[head].1 else {
                unreachable!("loop head")
            };
            *target = end;
        }
        let range = genexpr.range();
        let mut params = ast::Parameters::default();
        params.args.push(ast::ParameterWithDefault {
            range,
            node_index: ast::AtomicNodeIndex::NONE,
            default: None,
            parameter: ast::Parameter {
                range,
                node_index: ast::AtomicNodeIndex::NONE,
                name: ast::Identifier::new(".0", range),
                annotation: None,
            },
        });
        Ok(Box::new(params))
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
                enclosing(name)
            }
        };
        for (_, op) in &mut self.instructions {
            if let Instruction::Define(definition) | Instruction::ReturnClass(definition) = op {
                let child = Rc::get_mut(&mut definition.body).expect("newly lowered child code");
                if let DefinitionKind::ClassParameters(params) = &definition.kind {
                    child.resolve_directives(
                        &|name| {
                            if params.iter().any(|p| p.as_str() == name) {
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

    fn nested_program(&self, name: &str) -> Self {
        let mut child = Self::new(self.filename.clone(), name, self.has_named_bindings);
        child.strip_docstrings = self.strip_docstrings;
        child
    }

    fn new(filename: Rc<str>, name: &str, has_named_bindings: bool) -> Self {
        Self {
            filename,
            name: name.to_owned(),
            qualname: name.to_owned(),
            nested_global: false,
            class_attributes: None,
            class_cell: false,
            class_nonlocal: false,
            needs_class_cell: false,
            needs_dict_cell: false,
            postponed_annotations: false,
            has_named_bindings,
            doc: None,
            strip_docstrings: false,
            literals: HashMap::new(),
            string_roots: NONE,
            first_line: 1,
            kind: FunctionKind::Plain,
            interactive: false,
            locals: HashSet::new(),
            directives: HashMap::new(),
            reads: BTreeSet::new(),
            method_imports: Rc::default(),
            private_names: None,
            template_expressions: Rc::default(),
            instructions: Vec::new(),
            module_annotations: Vec::new(),
            expression_functions: HashMap::new(),
            eager_comprehension_targets: HashMap::new(),
        }
    }

    // A suspended expression supplies its result when execution resumes.
    fn lower_suspension(
        &mut self,
        expr: ast::Expr,
        line: u32,
        function: bool,
    ) -> Result<Option<ast::Expr>> {
        let op = match expr {
            ast::Expr::Yield(_) | ast::Expr::YieldFrom(_) if !function => {
                return Err(Error::new("SyntaxError", "'yield' outside function"));
            }
            ast::Expr::Await(a) if self.kind == FunctionKind::Coroutine => {
                self.instructions.push((line, Instruction::Await(*a.value)));
                self.instructions.push((line, Instruction::Delegate));
                return Ok(None);
            }
            ast::Expr::Yield(y) => Instruction::Yield(y.value.map(|value| *value)),
            ast::Expr::YieldFrom(y) => {
                self.instructions.push((line, Instruction::Iter(*y.value)));
                Instruction::Delegate
            }
            value => return Ok(Some(value)),
        };
        self.kind = FunctionKind::Generator;
        self.instructions.push((line, op));
        Ok(None)
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
        if !self.interactive
            && let Some(ast::Stmt::Expr(first)) = body.first()
            && let ast::Expr::StringLiteral(doc) = first.value.as_ref()
        {
            if self.strip_docstrings {
                body.remove(0);
            } else if function || self.class_attributes.is_some() {
                self.doc = Some(doc.range());
            }
        }
        let mut fields = Vec::new();
        let class = self.class_attributes.is_some();
        self.postponed_annotations = class
            && self.string_annotations()
            && imre_ast::annotations::has_variable_annotation(&body);
        self.needs_class_cell =
            class && imre_ast::class_cell::needed(&body, self.string_annotations());
        self.needs_dict_cell =
            class && imre_ast::class_cell::dict_needed(&body, self.string_annotations());
        let module_annotations =
            !function && !class && imre_ast::annotations::has_variable_annotation(&body);
        if module_annotations {
            self.instructions
                .push((self.first_line, Instruction::ModuleAnnotations(None)));
        }
        self.lower_with_fields(body, lines, function, None, class.then_some(&mut fields))?;
        if !self.module_annotations.is_empty() {
            let entries = std::mem::take(&mut self.module_annotations);
            let provider = self.annotation_provider(entries, None, self.first_line);
            self.instructions[0].1 = Instruction::ModuleAnnotations(Some(provider));
        }
        if !fields.is_empty() {
            let provider = self.annotation_provider(fields, Some(&self.qualname), self.first_line);
            self.instructions
                .push((self.first_line, Instruction::ClassAnnotator(provider)));
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
        self.lower_with_fields(body, lines, function, active, None)
    }

    fn lower_with_fields(
        &mut self,
        body: ast::Suite,
        lines: &[usize],
        function: bool,
        active: Option<&mut LoopJumps>,
        fields: Option<&mut Vec<(String, ast::Expr)>>,
    ) -> Result<()> {
        stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || {
            use ast::visitor::Visitor;
            let mut named = NamedBindings::default();
            if self.has_named_bindings {
                for stmt in &body {
                    named.visit_stmt(stmt);
                }
            }
            self.lower_suite(body, lines, function, active, fields)?;
            named.declare(self)
        })
    }

    fn lower_suite(
        &mut self,
        body: ast::Suite,
        lines: &[usize],
        function: bool,
        mut active: Option<&mut LoopJumps>,
        mut fields: Option<&mut Vec<(String, ast::Expr)>>,
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
                Stmt::AnnAssign(s)
                    if function
                        || s.simple
                        || (self.class_attributes.is_none() && s.target.is_name_expr()) =>
                {
                    let name = s.target.as_name_expr().map(|name| name.id.to_string());
                    if let Some(name) = &name {
                        if name == "__debug__" {
                            return Err(Error::new("SyntaxError", "cannot assign to __debug__"));
                        }
                        if let Some((kind, _)) = self.directives.get(name)
                            && (function
                                || self.class_attributes.is_some()
                                || *kind != ScopeDirective::Global)
                        {
                            return Err(Error::new(
                                "SyntaxError",
                                format!("annotated name '{name}' can't be {}", kind.keyword()),
                            ));
                        }
                        if function || s.simple || s.value.is_some() {
                            self.locals.insert(name.clone());
                        }
                    } else if s.value.is_none() {
                        return Err(unsupported("annotated assignment target"));
                    }
                    annotations::validate(&s.annotation, name.is_some())?;
                    if let Some(value) = s.value {
                        let op = match self.lower_suspension(*value, line, function)? {
                            Some(value) => Instruction::Assign(vec![*s.target], value),
                            None => Instruction::BindSent(vec![*s.target]),
                        };
                        self.instructions.push((line, op));
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
                    } else if self.class_attributes.is_none() {
                        let index = self.module_annotations.len();
                        self.module_annotations.push((name, *s.annotation));
                        Instruction::MarkModuleAnnotation(index)
                    } else if let Some(entries) = &mut fields {
                        entries.push((name, *s.annotation));
                        continue;
                    } else {
                        return Err(unsupported("conditional class annotation"));
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
                Stmt::TypeAlias(s) if !function && self.class_attributes.is_none() => {
                    let mut parameters = Vec::new();
                    if let Some(params) = s.type_params {
                        for param in &*params {
                            let ast::TypeParam::TypeVar(var) = param else {
                                return Err(unsupported("variadic type alias parameters"));
                            };
                            if var.bound.is_some() || var.default.is_some() {
                                return Err(unsupported(
                                    "type alias parameter bounds and defaults",
                                ));
                            }
                            parameters.push(var.name.to_string());
                        }
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
                        kind: DefinitionKind::TypeAlias(parameters),
                        annotate: None,
                    })
                }
                Stmt::FunctionDef(mut s) if s.type_params.is_none() => {
                    let mut child = self.definition_body(&s.name, &s.decorator_list, line, lines);
                    let annotate = self.annotation_program(&mut s, &child, line)?;
                    if s.is_async {
                        if imre_ast::code_flags::is_generator(&s.body) {
                            return Err(unsupported("async generator function"));
                        }
                        child.kind = FunctionKind::Coroutine;
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
                        kind: DefinitionKind::Function(s.parameters),
                        annotate,
                    })
                }
                Stmt::ClassDef(mut s) => {
                    if let Some(args) = &s.arguments
                        && (args.keywords.iter().any(|kw| kw.arg.is_none())
                            || args.args.iter().any(|a| matches!(a, ast::Expr::Starred(_))))
                    {
                        return Err(unsupported("expanded class arguments"));
                    }
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
                    child.class_attributes = Some(imre_ast::class_attributes::static_attributes(
                        &s.body, spellings,
                    ));
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
                        },
                        annotate: None,
                    };
                    if let Some(params) = s.type_params {
                        let mut parameters = Vec::new();
                        for param in &*params {
                            let ast::TypeParam::TypeVar(var) = param else {
                                return Err(unsupported("variadic class type parameters"));
                            };
                            match var.name.as_str() {
                                "__classdict__" => {
                                    return Err(Error::new(
                                        "SyntaxError",
                                        "reserved name '__classdict__' cannot be used for type parameter",
                                    ));
                                }
                                "__debug__" => {
                                    return Err(Error::new(
                                        "SyntaxError",
                                        "cannot assign to __debug__",
                                    ));
                                }
                                _ => {}
                            }
                            if var.bound.is_some() || var.default.is_some() {
                                return Err(unsupported(
                                    "class type parameter bounds and defaults",
                                ));
                            }
                            if parameters
                                .iter()
                                .any(|p: &ast::Identifier| p.id == var.name.id)
                            {
                                return Err(Error::new(
                                    "SyntaxError",
                                    format!(
                                        "duplicate type parameter '{}'",
                                        definition.body.definition_spelling(&var.name)
                                    ),
                                ));
                            }
                            parameters.push(var.name.clone());
                        }
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
                            kind: DefinitionKind::ClassParameters(parameters),
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
                        self.instructions[jump].1 = if current.has_iterator {
                            Instruction::Break(end)
                        } else {
                            Instruction::Jump(end)
                        };
                    }
                    continue;
                }
                Stmt::With(s) => {
                    if s.is_async && self.kind != FunctionKind::Coroutine {
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
                Stmt::Try(_)
                    if !function
                        && self.class_attributes.is_none()
                        && !self.string_annotations()
                        && imre_ast::annotations::has_variable_annotation(
                            std::slice::from_ref(&stmt),
                        ) =>
                {
                    // CPython duplicates finally suites on unwind and numbers else before handlers.
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
                            self.instructions.push((
                                handler_line,
                                Instruction::EnterFinally { handler: 0, end: 0 },
                            ));
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
                            self.instructions[entry].1 = Instruction::EnterFinally { handler, end };
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
                    self.lower(s.orelse, lines, function, active.as_deref_mut())?;
                    let done = self.instructions.len();
                    for i in handled {
                        self.instructions[i].1 = Instruction::Jump(done);
                    }
                    continue;
                }
                Stmt::Try(mut s) if !s.is_star && !s.finalbody.is_empty() => {
                    let finalbody = std::mem::take(&mut s.finalbody);
                    let enter = self.instructions.len();
                    self.instructions
                        .push((line, Instruction::EnterFinally { handler: 0, end: 0 }));
                    // The finally surrounds the handlers and else suite as well as the try body.
                    let protected = if s.handlers.is_empty() {
                        s.body
                    } else {
                        std::iter::once(Stmt::Try(s)).collect()
                    };
                    self.lower(protected, lines, function, active.as_deref_mut())?;
                    self.instructions.push((line, Instruction::StartFinally));
                    let handler = self.instructions.len();
                    self.lower(finalbody, lines, function, active.as_deref_mut())?;
                    self.instructions.push((line, Instruction::EndFinally));
                    let end = self.instructions.len();
                    self.instructions[enter].1 = Instruction::EnterFinally { handler, end };
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
                        let pattern = match case.pattern {
                            ast::Pattern::MatchValue(p) => SourcePattern::Value(p.value),
                            ast::Pattern::MatchClass(p)
                                if p.arguments.patterns.is_empty()
                                    && p.arguments.keywords.is_empty() =>
                            {
                                SourcePattern::Class(p.cls)
                            }
                            ast::Pattern::MatchAs(p) if p.pattern.is_none() && p.name.is_none() => {
                                if case.guard.is_none() && cases.len() + 1 < count {
                                    return Err(Error::new(
                                        "SyntaxError",
                                        "wildcard makes remaining patterns unreachable",
                                    ));
                                }
                                SourcePattern::Wildcard
                            }
                            _ => return Err(unsupported("match pattern")),
                        };
                        cases.push(SourceCase {
                            pattern,
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

    /// Declare every local name in a loop or unpacking target.
    fn declare_target(&mut self, target: &ast::Expr) -> Result<()> {
        Self::collect_target_names(target, &mut self.locals)
    }

    fn collect_target_names(target: &ast::Expr, names: &mut HashSet<String>) -> Result<()> {
        match target {
            ast::Expr::Name(n) => {
                names.insert(n.id.to_string());
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
        let filename = self
            .string_storage(filename)
            .map(|id| self.str_value(id))
            .transpose()?
            .and_then(|path| path.rsplit(std::path::MAIN_SEPARATOR).next());
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
        let filename_id = filename;
        let filename = filename_text;
        let source = if strip_indent {
            source.trim_start_matches([' ', '\t'])
        } else {
            &source
        };
        let parsed = ruff_python_parser::parse(source, parser_mode.into())
            .map_err(|error| Error::new("SyntaxError", format!("{filename}: {error}")))?;
        if interactive && let ast::Mod::Module(module) = parsed.syntax() {
            check_single_input(&module.body, parsed.tokens())?;
        }
        let spellings = string_replacements(source, parsed.tokens());
        let lines = line_starts(source.match_indices('\n').map(|(offset, _)| offset + 1))?;
        if let Some(keyword) = imre_ast::repeated_keyword(parsed.syntax().into()) {
            let message = format!(
                "keyword argument repeated: {}",
                keyword.arg.as_ref().unwrap()
            );
            let position = |offset: usize| {
                let mut line = 1;
                let mut line_start = 0;
                for (at, newline) in source[..offset].match_indices(['\r', '\n']) {
                    if newline == "\n" || !source[at + 1..].starts_with('\n') {
                        line += 1;
                        line_start = at + 1;
                    }
                }
                (line, offset - line_start + 1)
            };
            let (line, column) = position(keyword.start().to_usize());
            let (end_line, end_column) = position(keyword.end().to_usize());
            let msg = self.text(&message);
            let details = vec![
                filename_id,
                self.int(line as i64),
                self.int(column as i64),
                NONE,
                self.int(end_line as i64),
                self.int(end_column as i64),
            ];
            let location = self.alloc(Value::Tuple(details));
            let args = [msg, location];
            let mut error = self.exception("SyntaxError", &args);
            self.retain_syntax_fields(&mut error, self.syntax_fields(&args))?;
            return Err(error);
        }
        if let ast::Mod::Module(module) = parsed.syntax()
            && let Some((expr, message)) = imre_ast::type_alias_error(&module.body)
        {
            let position = |offset: ruff_text_size::TextSize| {
                let offset = offset.to_usize();
                let line = lines.partition_point(|start| *start <= offset);
                (line as i64, (offset - lines[line - 1] + 1) as i64)
            };
            let (line, column) = position(expr.start());
            let (end_line, end_column) = position(expr.end());
            let msg = self.text(message);
            let line = self.int(line);
            let mut fields = self.syntax_fields(&[msg]);
            fields[1] = filename_id;
            fields[2] = line;
            fields[3] = self.int(column);
            fields[5] = self.int(end_line);
            fields[6] = self.int(end_column);
            let mut error = self.exception("SyntaxError", &[msg]);
            error
                .message
                .push_str(&self.syntax_suffix(filename_id, line)?);
            self.retain_syntax_fields(&mut error, fields)?;
            return Err(error);
        }
        let mut code = Program::new(Rc::from(filename), "<module>", source.contains(":="));
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
                let caller = self.heap.current_code();
                let inherited = matches!(self.heap.get(caller), Value::Code(code) if
            code.flags & imre_ast::code_flags::FUTURE_ANNOTATIONS != 0
            || matches!(&code.body, Body::Source(p) if p.private_names.as_ref().is_some_and(|n| n.string_annotations)));
                if inherited || imre_ast::future_annotations(&body) {
                    code.private_names = Some(Rc::new(imre_ast::lower_in(&mut body, "", true)));
                }
                code.lower_namespace(body, &lines, false)?;
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
        code.prepare_strings(
            &mut self.heap,
            &mut SharedConstants::default(),
            &spellings,
            &|offset| lines.partition_point(|n| *n <= offset) as u32,
        )?;
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
                .str_value(filename)
                .expect("source filename was validated");
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
        let filename = self.text(program.filename.as_ref());
        let doc = program
            .doc
            .and_then(|range| program.literals.get(&range).copied())
            .unwrap_or(NONE);
        self.alloc(Value::Code(value::CodeObject {
            name,
            qualname,
            doc,
            filename,
            first_line: program.first_line,
            module_definition: !program.nested_global && program.name == program.qualname,
            coroutine: program.kind == FunctionKind::Coroutine,
            flags: match program.kind {
                FunctionKind::Generator | FunctionKind::GeneratorLambda => {
                    imre_ast::code_flags::GENERATOR
                }
                FunctionKind::Coroutine => imre_ast::code_flags::COROUTINE,
                FunctionKind::Plain => 0,
            },
            flags_complete: false,
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
        self.dict_entries(dict)?.iter().find_map(|&(key, value)| {
            matches!(self.heap.get(key), Value::Str(k) if k == name).then_some(value)
        })
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
        if frame.parent.is_none() {
            return self.module_globals(scope);
        }
        if frame.class_body.is_some() {
            return Ok(self.active_class_namespace(scope));
        }
        let mut names = BTreeSet::new();
        let mut local = scope;
        loop {
            let frame = self.scope_data(local);
            names.extend(
                frame
                    .names
                    .keys()
                    .chain(frame.closure.cells.keys())
                    .cloned(),
            );
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
        let mut bindings = Vec::new();
        for name in names {
            match self.lookup(scope, &name) {
                Ok(value) => bindings.push((self.heap.code_name(&name), value)),
                Err(error) if self.is_builtin_error(&error, "NameError") => {}
                Err(error) => return Err(error),
            }
        }
        Ok(self.dict(bindings))
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
            Body::PendingGenerator { .. } if definition.coroutine => FunctionKind::Coroutine,
            Body::PendingGenerator { .. } => FunctionKind::Generator,
            Body::Native(_) => unreachable!("source function"),
        };
        if kind != FunctionKind::Plain {
            let frame = self.alloc(Value::Generator {
                owner,
                frame: None,
                code,
                scope,
                iterators: Vec::new(),
                protected: Vec::new(),
                offset: Some(0),
                running: false,
            });
            Ok(if kind == FunctionKind::Coroutine {
                self.alloc(Value::Coroutine(frame))
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
        let parsed = ruff_python_parser::parse_module(source)
            .map_err(|err| Error::new("SyntaxError", err.to_string()))?;
        let spellings = string_replacements(source, parsed.tokens());
        let template_expressions =
            imre_ast::strings::template_expressions(source, parsed.tokens(), parsed.syntax());
        let mut syntax = parsed.into_syntax();
        let [ast::Stmt::FunctionDef(def)] = syntax.body.as_mut_slice() else {
            return Err(Error::new("SystemError", "invalid generator body"));
        };
        #[derive(serde::Deserialize)]
        struct Signature<'a> {
            #[serde(borrow)]
            private_scope: Option<(&'a str, bool)>,
            #[serde(default)]
            generator_lambda: bool,
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
        let lines = line_starts(source.match_indices('\n').map(|(offset, _)| offset + 1))?;
        let mut program = Program::new(
            self.str_value(code.filename)?.into(),
            self.str_value(code.name)?,
            source.contains(":="),
        );
        program.strip_docstrings = self.config["optimize"].as_i64().unwrap_or(0) >= 2;
        program.method_imports = Rc::new(signature.method_imports);
        program.template_expressions = Rc::new(template_expressions);
        program.private_names = private_names;
        program.qualname = self.str_value(code.qualname)?.to_owned();
        program.first_line = code.first_line;
        program.kind = if def.is_async {
            FunctionKind::Coroutine
        } else {
            FunctionKind::Generator
        };
        program.lower_namespace(std::mem::take(&mut def.body), &lines, true)?;
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
        let coroutine = self.code_object(code).coroutine;
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
        let next = if sent == NONE || matches!(self.code_object(code).body, Body::Native(_)) {
            self.next_result(generator)?
        } else {
            stacker::maybe_grow(1024 * 1024, 4 * 1024 * 1024, || {
                self.resume_source(generator, sent)
            })?
        };
        next.ok_or_else(|| self.exception("StopIteration", &[]))
    }

    pub(super) fn resume_source(&mut self, id: Id, sent: Id) -> Result<Option<Id>> {
        self.resume_source_input(id, GeneratorInput::Send(sent))
    }

    pub(super) fn resume_source_input(
        &mut self,
        mut id: Id,
        mut input: GeneratorInput,
    ) -> Result<Option<Id>> {
        let mut parents = Vec::new();
        let mut advanced = None;
        loop {
            let step = self.resume_source_frame(
                id,
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

    /// Resume one frame, retaining its running state while a delegated child executes.
    fn resume_source_frame(
        &mut self,
        id: Id,
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
            self.heap.keep(id);
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
        self.heap.disarm_generator_finalizer(id);
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
            .map_err(|error| {
                // CPython emits the StopIteration handler for generator defs and genexprs,
                // but a lambda that yields lets the original exception escape.
                if matches!(&self.code_object(code).body, Body::Source(p) if p.kind == FunctionKind::GeneratorLambda) {
                    error
                } else {
                    self.frame_error(error, code)
                }
            });
        let suspended_at_delegate = matches!(&result, Ok(Step::Yield(_, ip))
            if matches!(&self.code_object(code).body, Body::Source(program)
                if matches!(program.instructions.get(*ip), Some((_, Instruction::Delegate)))));
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
                        && (!blocks.is_empty() || suspended_at_delegate));
                *offset = Some(ip);
                *iterators = active;
                *protected = blocks;
                if needs_finalizer {
                    self.heap.register_finalizer(id);
                }
                return Ok(step);
            }
            Ok(Step::Return(value)) => Ok(value),
            Err(error) => Err(error),
        };
        *scope = NONE;
        *frame = None;
        *owner = FrameOwner::Module(NONE);
        self.heap.disarm_generator_finalizer(id);
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
            self.collect_unreachable();
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
                            | Instruction::Delegate
                            | Instruction::AsyncWithMethods(None)
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
                        file: code.filename.to_string().into(),
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
                        Instruction::EnterFinally { handler, end }
                        | Instruction::EnterExcept { handler, end } => {
                            cursor.protected.push(FinallyBlock {
                                start: cursor.ip,
                                handler: *handler,
                                end: *end,
                                depth: cursor.iterators.len(),
                                pending: None,
                                kind: if matches!(op, Instruction::EnterExcept { .. }) {
                                    HandlerKind::Except
                                } else {
                                    HandlerKind::Finally
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
                        _ => match self.source_step(&code, owner, scope, op, &mut cursor) {
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
            self.collect_unreachable();
            self.heap.release_unbound_finalizers();
        }
        Ok(())
    }

    /// Publish a binding and report whether its previous value could own a buffered writer.
    pub(super) fn store_source_name(&mut self, scope: Id, name: &str, value: Id) -> Result<bool> {
        let value = self.enum_assignment(scope, name, value)?;
        let replaced = self.replacing_finalizer(scope, name, value);
        let scope = self.binding_scope(scope, name);
        let frame = self.scope_data(scope);
        if let Some(locals) = frame.exec_locals.or(frame.class_body_mapping()) {
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
            Instruction::Eval(e) => {
                let value = eval(self, e)?;
                if code.interactive {
                    self.display_expression(value)?;
                }
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
                self.heap.release_temporaries_since(mark);
                self.heap.keep(subject);
                let subject_mark = self.heap.temporary_mark();
                let mut target = *end;
                // CPython compiles a single trailing wildcard as the default suite.
                let default = cases
                    .last()
                    .is_some_and(|case| matches!(case.pattern, SourcePattern::Wildcard));
                let subject_cases = cases.len() - usize::from(default);
                for (index, case) in cases.iter().enumerate() {
                    self.heap.set_statement_line(match_line + case.line_offset);
                    let matched = match &case.pattern {
                        SourcePattern::Value(expr) => {
                            let pattern = eval(self, expr)?;
                            let comparison = self.binary("==", subject, pattern)?;
                            self.truth(comparison)?
                        }
                        SourcePattern::Class(expr) => {
                            let class = eval(self, expr)?;
                            if !matches!(self.heap.get(class), Value::Class { .. })
                                && !matches!(
                                    self.builtin_type_name(class),
                                    Some("type" | "typing._AnyMeta")
                                )
                            {
                                return Err(Error::new(
                                    "TypeError",
                                    "called match pattern must be a class",
                                ));
                            }
                            self.heap.get(subject).instance_class() == Some(class)
                                || self.python_instance_check(subject, class)?
                        }
                        SourcePattern::Wildcard => true,
                    };
                    // Only later cases need the subject while a guard executes.
                    let live = if index + 1 < subject_cases {
                        subject_mark
                    } else {
                        mark
                    };
                    self.heap.release_temporaries_since(live);
                    if matched {
                        if let Some((offset, guard)) = &case.guard {
                            self.heap.set_statement_line(match_line + *offset);
                            let accept = self.source_condition(code, scope, guard)?;
                            self.heap.release_temporaries_since(live);
                            if !accept {
                                continue;
                            }
                        }
                        target = case.body;
                        break;
                    }
                }
                self.heap.release_temporaries_since(mark);
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
            | Instruction::Jump(_)
            | Instruction::Break(_) => unreachable!("source control instruction"),
            Instruction::Iter(e) => {
                let value = eval(self, e)?;
                iterators.push(self.iterator(value)?);
            }
            Instruction::Await(e) => {
                let value = eval(self, e)?;
                iterators.push(self.await_iterator(value, None)?);
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
                    DelegateOutcome::Yield(value) => {
                        if let Instruction::Next(Some(target), _) = op {
                            self.bind_target(code, scope, target, value)?;
                        } else {
                            return Ok(Some(Step::Yield(value, *ip - 1)));
                        }
                    }
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
            Instruction::ModuleAnnotations(provider) => {
                self.start_module_annotations(code, owner, scope, provider.as_ref())?;
            }
            Instruction::MarkModuleAnnotation(index) => {
                let index = self.int(*index as i64);
                self.mark_module_annotation(scope, index)?;
            }
            Instruction::ClassAnnotator(provider) => {
                let annotate = self.source_annotator(provider, owner, scope)?;
                self.bind_source_name(scope, "__annotate_func__", annotate)?;
            }
            Instruction::CheckAnnotationFormat => {
                let format = self.lookup(scope, ".format")?;
                self.op("annotation_format", format, NONE, NONE)?;
            }
            Instruction::ClassStart => {
                self.enable_class_annotation_provider(scope);
                // Only postponed annotations need a visible mapping from the start of the suite.
                if !code.postponed_annotations {
                    let key = self.heap.code_name("__annotations__");
                    self.remove_binding(scope, "__annotations__", key)?;
                }
                let first = self.int(i64::from(code.first_line));
                self.bind(scope, "__firstlineno__", first);
                if let Some(doc) = code.doc {
                    self.bind(scope, "__doc__", code.literals[&doc]);
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
        let mut alias_parameters = NONE;
        let mut bases = None;
        let sig = match &definition.kind {
            DefinitionKind::TypeAlias(names) => {
                if !names.is_empty() {
                    let entries = names
                        .iter()
                        .map(|name| {
                            let id = self.heap.code_name(name);
                            (id, id)
                        })
                        .collect();
                    let names = self.dict(entries);
                    defining_scope =
                        super::typing::parameter_scope(self, defining_scope, names, false)?;
                    alias_parameters = self.lookup(defining_scope, ".type_params")?;
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
            } => {
                let mut values = Vec::new();
                let mut keywords = Vec::new();
                if let Some(args) = args {
                    for expression in &args.args {
                        let base = eval(self, expression)?;
                        self.heap.keep(base);
                        values.push(base);
                    }
                    for keyword in &args.keywords {
                        let name = keyword.arg.as_ref().expect("validated class keyword");
                        let value = eval(self, &keyword.value)?;
                        self.heap.keep(value);
                        keywords.push((name.to_string(), value));
                    }
                }
                if *parameterized {
                    values.push(self.lookup(defining_scope, ".generic_base")?);
                }
                bases = Some((self.alloc(Value::List(values)), keywords, *parameterized));
                serde_json::json!({
                    "needs_class_cell": child.needs_class_cell,
                    "needs_dict_cell": child.needs_dict_cell,
                    "postponed_annotations": child.postponed_annotations,
                })
            }
            DefinitionKind::Function(p) => {
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
                sig["class_cell"] = serde_json::json!(child.class_cell);
                sig["scope_locals"] =
                    serde_json::json!(child.locals.iter().collect::<BTreeSet<_>>());
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
            let freevars = if alias_parameters != NONE {
                self.enclosing_freevars(defining_scope, child.reads.iter().map(String::as_str))
            } else if (code.name != "<module>" || self.scope_data(scope).comprehension.is_some())
                && child.class_attributes.is_none()
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
            return self.op("type_alias", name, value, alias_parameters);
        }
        if let Some(provider) = &definition.annotate {
            let annotate = self.source_annotator(provider, code, defining_scope)?;
            self.set_function_attr(value, "__annotate__", annotate)?;
        }
        if let DefinitionKind::ClassParameters(params) = &definition.kind {
            let entries = params
                .iter()
                .map(|param| {
                    let name = child.definition_spelling(param);
                    (
                        self.heap.code_name(param.as_str()),
                        self.heap.code_name(name),
                    )
                })
                .collect();
            let names = self.dict(entries);
            let parameters = super::typing::parameter_scope(self, defining_scope, names, true)?;
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
        callable: Id,
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
            self.op("keywords", merged, mapping, callable)?;
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
                    format!("{} argument after *", self.keyword_callable(callable)?)
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

    /// Obtain an iterator in the selected scope and drop its consumed expression operands.
    fn source_comprehension_iterator(
        &mut self,
        code: &Program,
        scope: Id,
        expression: &ast::Expr,
    ) -> Result<Id> {
        let mark = self.heap.temporary_mark();
        let source = self.source_expr(code, scope, expression)?;
        let iterator = self.iterator(source)?;
        self.keep_source_result(mark, iterator)
    }

    /// Append each element to the result before advancing its innermost iterator.
    fn source_eager_comprehension(
        &mut self,
        code: &Program,
        parent: Id,
        element: &ast::Expr,
        clauses: &[ast::Comprehension],
        kind: &str,
        targets: &HashSet<String>,
    ) -> Result<Id> {
        let mark = self.heap.temporary_mark();
        let result = (|| -> Result<Id> {
            let first = self.source_comprehension_iterator(code, parent, &clauses[0].iter)?;
            let scope = self.comprehension_scope_with_targets(parent, targets.clone());
            let collection = self.op(kind, NONE, NONE, NONE)?;
            // Each mark preserves the iterator and persistent roots of all outer clauses.
            let mut levels = Vec::new();
            levels
                .try_reserve_exact(clauses.len())
                .map_err(|_| crate::Error::memory_error())?;
            levels.push((first, self.heap.temporary_mark()));
            'advance: while let Some(&(iterator, step)) = levels.last() {
                self.comprehension_step(step);
                let Some(item) = self.next_item(iterator)? else {
                    levels.pop();
                    continue;
                };
                self.heap.keep(item);
                let clause = &clauses[levels.len() - 1];
                self.bind_target(code, scope, &clause.target, item)?;
                for test in &clause.ifs {
                    if !self.source_condition(code, scope, test)? {
                        continue 'advance;
                    }
                }
                if let Some(next) = clauses.get(levels.len()) {
                    let inner = self.source_comprehension_iterator(code, scope, &next.iter)?;
                    levels.push((inner, self.heap.temporary_mark()));
                } else {
                    let element = self.source_expr(code, scope, element)?;
                    self.op("append", collection, element, NONE)?;
                }
            }
            Ok(collection)
        })();
        match result {
            Ok(collection) => self.keep_source_result(mark, collection),
            Err(error) => {
                let collect = self.heap.discard_truth_operands(mark, self.modules);
                for root in error.roots() {
                    self.heap.keep(root);
                }
                if collect {
                    self.collect_unreachable();
                }
                Err(error)
            }
        }
    }

    fn source_expr_inner(&mut self, code: &Program, scope: Id, expr: &ast::Expr) -> Result<Id> {
        use ast::Expr;
        let eval = |ctx: &mut Self, e: &Expr| ctx.source_expr(code, scope, e);
        match expr {
            Expr::Lambda(_) | Expr::Generator(_)
                if code.expression_functions.contains_key(&expr.range()) =>
            {
                let (site, definition) = &code.expression_functions[&expr.range()];
                let function = self.source_definition(
                    code,
                    self.heap.current_code(),
                    scope,
                    code.instructions.len() + site,
                    definition,
                )?;
                if let Expr::Generator(genexpr) = expr {
                    self.heap.keep(function);
                    let outer = eval(self, &genexpr.generators[0].iter)?;
                    let iter = self.iterator(outer)?;
                    let generator = self.call(function, vec![iter], Vec::new())?;
                    let Value::Generator { iterators, .. } = self.heap.get_mut(generator) else {
                        unreachable!("source generator expression")
                    };
                    iterators.push(iter);
                    Ok(generator)
                } else {
                    Ok(function)
                }
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
                self.check_source_local(code, enclosing, name)?;
                self.lookup(scope, name)
            }
            Expr::Named(named) if code.has_named_bindings => {
                let Expr::Name(name) = named.target.as_ref() else {
                    return Err(unsupported("assignment expression target"));
                };
                let mark = self.heap.temporary_mark();
                let value = eval(self, &named.value)?;
                if self.store_source_name(
                    self.named_expression_scope(scope),
                    name.id.as_str(),
                    value,
                )? && self.heap.named_assignment(mark, value)
                {
                    self.collect_unreachable();
                    self.heap.release_unbound_finalizers();
                }
                Ok(value)
            }
            Expr::StringLiteral(_) | Expr::BytesLiteral(_) => Ok(code.literals[&expr.range()]),
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
            Expr::NumberLiteral(v) => {
                let value = match &v.value {
                    ast::Number::Float(value) => Value::Float(*value),
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
                Ok(self.alloc(value))
            }
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
            ast::Expr::SetComp(ast::ExprSetComp {
                elt, generators, ..
            })
            | ast::Expr::ListComp(ast::ExprListComp {
                elt, generators, ..
            }) if let Some(targets) = code.eager_comprehension_targets.get(&expr.range()) => {
                let kind = if matches!(expr, Expr::ListComp(_)) {
                    "list"
                } else {
                    "set"
                };
                self.source_eager_comprehension(code, scope, elt, generators, kind, targets)
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
                    let method = self.lookup_method(receiver, field.attr.as_str())?;
                    // Evaluating an argument can delete the receiver's reference to the callable.
                    self.heap.reserve_temporaries(1)?;
                    self.heap.keep(method);
                    method
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
                    Some(self.source_keyword_mapping(code, scope, f, keywords)?)
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
                if let Some(mapping) = mapping {
                    kwargs = self.keyword_args(mapping)?;
                }
                let result = self.call_in_scope(scope, f, args, kwargs)?;
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
