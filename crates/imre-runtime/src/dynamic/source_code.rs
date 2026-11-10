//! Runtime code metadata, compilation of source objects and Python namespace operations.

mod annotations;
mod comprehensions;

use annotations::CONDITIONAL;

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

use imre_ast::code_flags;
use imre_ast::comprehension::{
    InvalidTarget, Output, Plan, outer_clause, try_collect_target_names,
};
use ruff_python_ast as ast;
use ruff_text_size::{Ranged, TextRange};

use super::value::{Body, ExecGlobals};
use super::*;

#[derive(Clone)]
struct SourceFilename {
    // Code objects own the Python spelling. Rust diagnostic locations require scalar text.
    object: Id,
    display: Arc<str>,
}

struct Program {
    filename: SourceFilename,
    // Stop symbol collection at the first invalid binding or misplaced future import.
    syntax_limit: Option<ruff_text_size::TextRange>,
    // An invalid pattern still has a symbol table. Let all its scopes finish checking
    // declarations before reporting the code-generation error. Valid modules use no slot.
    deferred_pattern:
        Option<Rc<std::cell::RefCell<(TextRange, imre_ast::executable::PatternMessage)>>>,
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
    string_roots: Id,
    first_line: u32,
    kind: FunctionKind,
    scope_flags: u32,
    unlowered_yield: bool,
    interactive: bool,
    locals: indexmap::IndexSet<String>,
    directives: HashMap<String, (ScopeDirective, ruff_text_size::TextRange)>,
    reads: BTreeSet<String>,
    private_names: Option<Rc<imre_ast::Names>>,
    template_expressions: Rc<HashMap<ruff_text_size::TextSize, String>>,
    items: Vec<(u32, SourceItem)>,
    // Declarations in source lowering order, including nested suites.
    deferred_annotations: Vec<(String, ast::Expr)>,
    // Indices of the class declarations guarded by the private annotation cell.
    conditional_fields: Vec<usize>,
    class_binds_conditional_name: bool,
    expression_functions: HashMap<ruff_text_size::TextRange, (usize, Definition)>,
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

/// Suspension sites among the supported operands of one expression. The plan is discarded
/// after collecting code metadata; untouched subtrees are visited in place.
struct OperandPlan {
    sites: smallvec::SmallVec<[ruff_text_size::TextSize; 1]>,
    line: u32,
    function: bool,
}

impl OperandPlan {
    fn new(root: &ast::Expr, kind: FunctionKind, line: u32, function: bool) -> Self {
        let mut sites = smallvec::SmallVec::new();
        if kind.is_async() || (function && imre_ast::code_flags::yields_in_expression(root)) {
            let mut work = smallvec::SmallVec::<[&ast::Expr; 8]>::new();
            work.push(root);
            while let Some(expr) = work.pop() {
                match expr {
                    ast::Expr::Await(a) if kind.is_async() => {
                        sites.push(a.start());
                        work.push(&a.value);
                    }
                    ast::Expr::Yield(y) => {
                        sites.push(y.start());
                        work.extend(y.value.as_deref());
                    }
                    ast::Expr::YieldFrom(y) => {
                        sites.push(y.start());
                        work.push(&y.value);
                    }
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

/// Parsed items used to construct and validate code-object metadata.
enum SourceItem {
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
    From(String, Vec<(String, String)>),
    Define(Definition),
    ReturnClass(Definition),
    Annotations(Vec<(String, ast::Expr)>),
    ModuleAnnotations(Option<Rc<Program>>),
    ModuleDoc(TextRange, String),
    StringAnnotation(String, ast::Expr),
    ClassAnnotator(Rc<Program>),
    MakeGenerator(ast::Expr),
    Return(Option<ast::Expr>),
    Raise(ast::StmtRaise),
    Assert(ast::StmtAssert),
    Match {
        subject: ast::Expr,
        cases: Vec<SourceCase>,
    },
    EnterComprehension(HashSet<String>),
    LeaveComprehension,
    Name(ast::Identifier),
    Keywords(Vec<ast::Identifier>),
    EnterWith(ast::WithItem),
    BindSent(Vec<ast::Expr>),
}

struct SourceCase {
    pattern: ast::Pattern,
    captures: Vec<String>,
    guard: Option<Box<ast::Expr>>,
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
    },
    Class {
        arguments: Option<Box<ast::Arguments>>,
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
                    if let Err(error) =
                        try_collect_target_names(&clause.target, &mut frame.iteration_variables)
                            .map_err(invalid_target)
                    {
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
                && offset < directive.start()
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
                    .or_insert((ScopeDirective::Global, Default::default()));
            }
            program.locals.insert(name);
        }
        Ok(())
    }
}

fn unsupported(what: impl std::fmt::Display) -> Error {
    Error::new("NotImplementedError", format!("runtime source: {what}"))
}

fn invalid_target(error: InvalidTarget) -> Error {
    match error {
        InvalidTarget::BareStar => Error::new(
            "SyntaxError",
            "starred assignment target must be in a list or tuple",
        ),
        InvalidTarget::MultipleStars => {
            Error::new("SyntaxError", "multiple starred expressions in assignment")
        }
        InvalidTarget::Unsupported => unsupported("unpacking target"),
    }
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
    reads: &'ctx mut BTreeSet<String>,
    directives: &'ctx HashMap<String, (ScopeDirective, ruff_text_size::TextRange)>,
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
                        && name.start() < position.start()
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
                    self.visit_expr(&outer_clause(clauses).iter);
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
                    self.visit_expr(&outer_clause(&genexpr.generators).iter);
                    continue;
                }
                expr if let Some(plan) = Plan::from_expr(expr) => {
                    self.visit_expr(plan.outer);
                    let mut names = plan.try_target_names().map_err(invalid_target)?;
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
            exhausted: false,
        };
        let mut enclosing_bindings = Vec::new();
        let mut prepared_generators = Vec::new();
        for (_, op) in &mut self.items {
            match op {
                SourceItem::EnterComprehension(targets) => {
                    let mut bound = targets.clone();
                    if let Some(parent) = &strings.bound {
                        bound.extend(parent.iter().cloned());
                    }
                    enclosing_bindings.push(strings.bound.replace(Rc::new(bound)));
                }
                SourceItem::LeaveComprehension => {
                    strings.bound = enclosing_bindings.pop().expect("comprehension bindings");
                }
                SourceItem::Eval(ast::Expr::StringLiteral(doc))
                    if self.scope_flags & code_flags::HAS_DOCSTRING != 0
                        && self.doc == Some(doc.range()) =>
                {
                    let text = imre_ast::strings::clean_doc(doc.value.to_str());
                    let id = strings.text(text.into())?;
                    strings.constant(doc.range(), id)?;
                }
                SourceItem::Eval(e) | SourceItem::Discard(e) => strings.visit_expr(e),
                SourceItem::Name(name) => {
                    strings.heap.code_name(name.as_str());
                }
                SourceItem::Keywords(names) => {
                    for name in names {
                        strings.heap.code_name(name.as_str());
                    }
                }
                SourceItem::Match { subject, cases, .. } => {
                    strings.visit_expr(subject);
                    for case in cases {
                        strings.visit_pattern(&case.pattern);
                        for name in &case.captures {
                            strings.heap.code_name(name);
                        }
                        if let Some(guard) = &case.guard {
                            strings.visit_expr(guard);
                        }
                    }
                }
                SourceItem::MakeGenerator(expr) => {
                    prepared_generators.push((&*expr, strings.bound.clone()));
                }
                SourceItem::EnterWith(item) => {
                    strings.visit_expr(&item.context_expr);
                    if let Some(target) = &item.optional_vars {
                        strings.visit_expr(target);
                    }
                }
                SourceItem::DeleteName(name) => {
                    strings.heap.code_name(name);
                }
                SourceItem::DeleteItem(item) => {
                    strings.visit_expr(&item.value);
                    strings.visit_expr(&item.slice);
                }
                SourceItem::DeleteAttr(attr) => {
                    strings.visit_expr(&attr.value);
                    strings.heap.code_name(attr.attr.as_str());
                }
                SourceItem::Assert(stmt) => {
                    strings.visit_expr(&stmt.test);
                    if let Some(message) = &stmt.msg {
                        strings.visit_expr(message);
                    }
                }
                SourceItem::AugAssign(stmt) => {
                    strings.visit_expr(&stmt.target);
                    strings.visit_expr(&stmt.value);
                }
                SourceItem::ModuleDoc(range, text) => {
                    let id = strings.text(text.as_str().into())?;
                    strings.constant(*range, id)?;
                }
                SourceItem::Assign(targets, e) => {
                    for target in targets {
                        strings.visit_expr(target);
                    }
                    strings.visit_expr(e);
                }
                SourceItem::Import { module, alias } => {
                    for part in module.split('.') {
                        strings.heap.code_name(part);
                    }
                    if let Some(alias) = alias {
                        strings.heap.code_name(alias);
                    }
                }
                SourceItem::From(module, bindings) => {
                    strings.heap.code_name(module);
                    for (name, alias) in bindings {
                        strings.heap.code_name(name);
                        strings.heap.code_name(alias);
                    }
                }
                SourceItem::Annotations(entries) => {
                    for (name, expr) in entries {
                        strings.annotation(name, expr, stringized)?;
                    }
                }
                SourceItem::StringAnnotation(name, expr) => strings.annotation(name, expr, true)?,
                SourceItem::ClassAnnotator(provider)
                | SourceItem::ModuleAnnotations(Some(provider)) => {
                    nested_reads.extend(
                        strings
                            .annotation_code(provider, class_locals, line_at)?
                            .iter()
                            .cloned(),
                    );
                }
                SourceItem::Define(definition) | SourceItem::ReturnClass(definition) => {
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
                SourceItem::ModuleAnnotations(None) => {}
                SourceItem::Return(e) => {
                    if let Some(e) = e {
                        strings.visit_expr(e);
                    }
                }
                SourceItem::Raise(statement) => {
                    if let Some(e) = &statement.exc {
                        strings.visit_expr(e);
                    }
                    if let Some(e) = &statement.cause {
                        strings.visit_expr(e);
                    }
                }
                SourceItem::BindSent(targets) => {
                    for target in targets {
                        strings.visit_expr(target);
                    }
                }
            }
        }
        let (class_cell, mut functions) = strings.scan(&mut self.unlowered_yield)?;
        functions.extend(prepared_generators);
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

            child.qualname =
                Self::qualified_name(&self.qualname, self.class_attributes.is_some(), name);
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
                        Some(body) => SourceItem::Return(Some(body)),
                        None => {
                            child.kind = FunctionKind::GeneratorLambda;
                            SourceItem::Return(None)
                        }
                    };
                    child.items.push((line, end));
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
                            child
                                .directives
                                .insert(name, (kind, ruff_text_size::TextRange::empty(offset)));
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

    fn declare(
        &mut self,
        kind: ScopeDirective,
        names: &[ast::Identifier],
        range: ruff_text_size::TextRange,
    ) -> Result<()> {
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
                return Err(Error::located_source_syntax(
                    message,
                    SyntaxSpan::InFields(range),
                ));
            }
            self.directives.insert(name.to_string(), (kind, range));
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
        for (name, (kind, range)) in &self.directives {
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
                return Err(Error::located_source_syntax(
                    message,
                    SyntaxSpan::InFields(*range),
                ));
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
        for (_, op) in &mut self.items {
            if let SourceItem::Define(definition) | SourceItem::ReturnClass(definition) = op {
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
        matches!(self.items.first(), Some((_, SourceItem::ReturnClass(_))))
    }

    fn nested_program(&self, name: &str) -> Self {
        let mut child = Self::new(self.filename.clone(), name, self.has_named_bindings);

        child.syntax_limit = self.syntax_limit;
        child.deferred_pattern.clone_from(&self.deferred_pattern);
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

            syntax_limit: None,
            deferred_pattern: None,
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
            string_roots: NONE,
            first_line: 1,
            kind: FunctionKind::Plain,
            scope_flags: 0,
            unlowered_yield: false,
            interactive: false,
            locals: indexmap::IndexSet::new(),
            directives: HashMap::new(),
            reads: BTreeSet::new(),

            private_names: None,
            template_expressions: Rc::default(),
            items: Vec::new(),
            deferred_annotations: Vec::new(),
            conditional_fields: Vec::new(),
            class_binds_conditional_name: false,
            expression_functions: HashMap::new(),
        }
    }

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
        if matches!(expr, ast::Expr::Yield(_) | ast::Expr::YieldFrom(_)) {
            if !plan.function {
                self.invalid_transfer(expr.range(), "'yield' outside function")?;
            }
            if self.kind != FunctionKind::AsyncGenerator {
                self.kind = FunctionKind::Generator;
            }
        }
        if !plan.suspends(&expr) {
            return Ok(Some(expr));
        }
        if let ast::Expr::Generator(genexpr) = &expr {
            if let Some(outer) = self.lower_suspension(
                outer_clause(&genexpr.generators).iter.clone(),
                line,
                plan.function,
            )? {
                self.items.push((line, SourceItem::Discard(outer)));
            }
            self.items.push((line, SourceItem::MakeGenerator(expr)));
            return Ok(None);
        }
        if let Some(comp) = Plan::from_expr(&expr) {
            self.lower_eager_comprehension(&comp, plan)?;
            return Ok(None);
        }
        match expr {
            ast::Expr::Yield(y) => {
                if let Some(value) = y.value {
                    self.record_operand(*value, plan)?;
                }
            }
            ast::Expr::YieldFrom(y) => self.record_operand(*y.value, plan)?,
            ast::Expr::Await(a) => self.record_operand(*a.value, plan)?,
            ast::Expr::Call(call) => {
                self.record_operand(*call.func, plan)?;
                for arg in call.arguments.args {
                    self.record_operand(arg, plan)?;
                }
                let mut keywords = Vec::new();
                for kw in call.arguments.keywords {
                    keywords.push(kw.arg.expect("fixed keyword"));
                    self.record_operand(kw.value, plan)?;
                }
                self.items.push((line, SourceItem::Keywords(keywords)));
            }
            ast::Expr::Tuple(ast::ExprTuple { elts, .. })
            | ast::Expr::List(ast::ExprList { elts, .. })
            | ast::Expr::Set(ast::ExprSet { elts, .. }) => {
                for item in elts {
                    match item {
                        ast::Expr::Starred(star) => self.record_operand(*star.value, plan)?,
                        item => self.record_operand(item, plan)?,
                    }
                }
            }
            ast::Expr::Subscript(item) => {
                self.record_operand(*item.value, plan)?;
                self.record_operand(*item.slice, plan)?;
            }
            ast::Expr::Slice(slice) => {
                for part in [slice.lower, slice.upper, slice.step].into_iter().flatten() {
                    self.record_operand(*part, plan)?;
                }
            }
            ast::Expr::Named(named) => {
                let ast::Expr::Name(name) = *named.target else {
                    return Err(unsupported("assignment expression target"));
                };
                self.record_operand(*named.value, plan)?;
                self.items.push((
                    line,
                    SourceItem::Name(ast::Identifier::new(name.id, name.range)),
                ));
            }
            ast::Expr::Dict(dict) => {
                for item in dict.items {
                    if let Some(key) = item.key {
                        self.record_operand(key, plan)?;
                    }
                    self.record_operand(item.value, plan)?;
                }
            }
            ast::Expr::BinOp(b) => {
                self.record_operand(*b.left, plan)?;
                self.record_operand(*b.right, plan)?;
            }
            ast::Expr::Compare(c) => {
                for expr in c.operands.into_vec() {
                    self.record_operand(expr, plan)?;
                }
            }
            ast::Expr::BoolOp(b) => {
                for expr in b.values {
                    self.record_operand(expr, plan)?;
                }
            }
            ast::Expr::UnaryOp(u) => self.record_operand(*u.operand, plan)?,
            ast::Expr::Attribute(a) => {
                self.record_operand(*a.value, plan)?;
                self.items.push((line, SourceItem::Name(a.attr)));
            }
            ast::Expr::If(b) => {
                for expr in [b.test, b.body, b.orelse] {
                    self.record_operand(*expr, plan)?;
                }
            }
            _ => unreachable!("operand with a supported suspension"),
        }
        Ok(None)
    }

    fn record_operand(&mut self, expr: ast::Expr, plan: &OperandPlan) -> Result<()> {
        if let Some(expr) = self.lower_operand(expr, plan)? {
            self.items.push((plan.line, SourceItem::Discard(expr)));
        }
        Ok(())
    }

    fn record_expression(&mut self, expr: ast::Expr, line: u32, function: bool) -> Result<()> {
        let plan = OperandPlan::new(&expr, self.kind, line, function);
        self.record_operand(expr, &plan)
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
            self.items
                .push((self.first_line, SourceItem::ModuleAnnotations(None)));
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
                self.items
                    .push((line, SourceItem::ModuleDoc(doc.range(), text)));
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
                self.items
                    .push((self.first_line, SourceItem::ClassAnnotator(provider)));
            } else {
                self.items[0].1 = SourceItem::ModuleAnnotations(Some(provider));
            }
        }
        Ok(())
    }

    fn lower(
        &mut self,
        body: ast::Suite,
        lines: &[usize],
        function: bool,
        active: Option<&mut usize>,
    ) -> Result<()> {
        self.lower_block(body, lines, function, active, false)
    }

    fn lower_block(
        &mut self,
        mut body: ast::Suite,
        lines: &[usize],
        function: bool,
        active: Option<&mut usize>,
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
        mut active: Option<&mut usize>,
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
                    self.declare(ScopeDirective::Global, &s.names, s.range())?;
                    continue;
                }
                Stmt::Nonlocal(s) => {
                    self.declare(ScopeDirective::Nonlocal, &s.names, s.range())?;
                    continue;
                }
                Stmt::Expr(s) => match self.lower_suspension(*s.value, line, function)? {
                    Some(expr) => SourceItem::Eval(expr),
                    None => continue,
                },
                Stmt::Assign(s) => {
                    for target in &s.targets {
                        self.declare_target(target)?;
                    }
                    match self.lower_suspension(*s.value, line, function)? {
                        Some(value) => SourceItem::Assign(s.targets, value),
                        None => SourceItem::BindSent(s.targets),
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
                            Some(value) => SourceItem::Assign(vec![*s.target], value),
                            None => SourceItem::BindSent(vec![*s.target]),
                        };
                        self.items.push((line, op));
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
                        SourceItem::StringAnnotation(
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
                        }
                        continue;
                    }
                }
                Stmt::Delete(s) => {
                    for target in s.targets {
                        match target {
                            ast::Expr::Name(name) => {
                                let name = name.id.to_string();
                                self.locals.insert(name.clone());
                                self.items.push((line, SourceItem::DeleteName(name)));
                            }
                            ast::Expr::Subscript(item) => {
                                self.items.push((line, SourceItem::DeleteItem(item)));
                            }
                            ast::Expr::Attribute(attr) => {
                                self.items.push((line, SourceItem::DeleteAttr(attr)));
                            }
                            _ => return Err(unsupported("deletion target")),
                        }
                    }
                    continue;
                }
                Stmt::AugAssign(s) => {
                    self.declare_target(&s.target)?;
                    SourceItem::AugAssign(s)
                }
                Stmt::Return(s) => {
                    if !function {
                        self.invalid_transfer(s.range(), "'return' outside function")?;
                    }
                    let Some(value) = s.value else {
                        self.items.push((line, SourceItem::Return(None)));
                        continue;
                    };
                    match self.lower_suspension(*value, line, function)? {
                        Some(value) => SourceItem::Return(Some(value)),
                        None => continue,
                    }
                }
                Stmt::Raise(s) => SourceItem::Raise(s),
                Stmt::Import(s) if !s.is_lazy => {
                    for name in &s.names {
                        let module = name.name.to_string();
                        let alias = name.asname.as_ref().map(ToString::to_string);
                        let local = alias
                            .as_deref()
                            .unwrap_or_else(|| module.split('.').next().unwrap());
                        self.locals.insert(local.to_owned());
                        self.items
                            .push((line, SourceItem::Import { module, alias }));
                    }
                    continue;
                }
                Stmt::Assert(s) => SourceItem::Assert(s),
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
                    SourceItem::From(module.to_owned(), names)
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
                    let rhs_line =
                        lines.partition_point(|n| *n <= s.value.start().to_usize()) as u32;
                    child
                        .items
                        .push((rhs_line, SourceItem::Return(Some(*s.value))));
                    self.locals.insert(name.to_string());
                    SourceItem::Define(Definition {
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
                        if let Some((kind, range)) = child.directives.get(name) {
                            return Err(Error::located_source_syntax(
                                format!("name '{name}' is parameter and {}", kind.keyword()),
                                SyntaxSpan::InFields(*range),
                            ));
                        }
                        child.locals.insert(name.to_owned());
                    }
                    self.locals.insert(s.name.to_string());
                    SourceItem::Define(Definition {
                        binding: s.name.to_string(),
                        decorators: s.decorator_list.into_iter().collect(),
                        body: Rc::new(child),
                        kind: DefinitionKind::Function {
                            parameters: s.parameters,
                            type_params: s.type_params,
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
                    child.lower_namespace(s.body, lines, false)?;
                    self.locals.insert(s.name.to_string());
                    let mut definition = Definition {
                        binding: s.name.to_string(),
                        decorators: s.decorator_list.into_iter().collect(),
                        body: Rc::new(child),
                        kind: DefinitionKind::Class {
                            arguments: s.arguments,
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
                            .items
                            .push((line, SourceItem::ReturnClass(definition)));
                        definition = Definition {
                            binding: s.name.to_string(),
                            decorators,
                            body: Rc::new(header),
                            kind: DefinitionKind::ClassParameters(params),
                            annotate: None,
                        };
                    }
                    SourceItem::Define(definition)
                }
                Stmt::Break(_) | Stmt::Continue(_) => {
                    if let Some(exits) = active.as_deref_mut() {
                        *exits += 1;
                    } else {
                        let reason = if matches!(stmt, Stmt::Break(_)) {
                            "'break' outside loop"
                        } else {
                            "'continue' not properly in loop"
                        };
                        self.invalid_transfer(stmt.range(), reason)?;
                    }
                    continue;
                }
                Stmt::For(_) | Stmt::While(_) => {
                    let (body, orelse) = match stmt {
                        Stmt::For(s) => {
                            self.declare_target(&s.target)?;
                            if s.is_async && !self.kind.is_async() {
                                return Err(Error::new(
                                    "SyntaxError",
                                    "'async for' outside async function",
                                ));
                            }
                            self.record_expression(*s.iter, line, function)?;
                            self.items.push((line, SourceItem::Discard(*s.target)));
                            (s.body, s.orelse)
                        }
                        Stmt::While(s) => {
                            self.record_expression(*s.test, line, function)?;
                            (s.body, s.orelse)
                        }
                        _ => unreachable!(),
                    };
                    self.lower(body, lines, function, Some(&mut 0))?;
                    self.lower(orelse, lines, function, active.as_deref_mut())?;
                    continue;
                }
                Stmt::With(s) => {
                    if s.is_async && !self.kind.is_async() {
                        return Err(Error::new(
                            "SyntaxError",
                            "'async with' outside async function",
                        ));
                    }
                    for item in s.items {
                        if let Some(target) = &item.optional_vars {
                            self.declare_target(target)?;
                        }
                        if s.is_async {
                            self.record_expression(item.context_expr, line, function)?;
                            if let Some(target) = item.optional_vars {
                                self.items.push((line, SourceItem::Discard(*target)));
                            }
                        } else {
                            self.items.push((line, SourceItem::EnterWith(item)));
                        }
                    }
                    self.lower(s.body, lines, function, active.as_deref_mut())?;
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
                    self.lower(s.body, lines, function, active.as_deref_mut())?;
                    let before_handlers = self.deferred_annotations.len();
                    for ast::ExceptHandler::ExceptHandler(h) in s.handlers {
                        let handler_line =
                            lines.partition_point(|n| *n <= h.start().to_usize()) as u32;
                        if let Some(class) = h.type_ {
                            self.items.push((handler_line, SourceItem::Discard(*class)));
                        }
                        if let Some(name) = h.name {
                            self.locals.insert(name.to_string());
                            self.items.push((handler_line, SourceItem::Name(name)));
                        }
                        self.lower(h.body, lines, function, active.as_deref_mut())?;
                    }
                    let before_else = self.deferred_annotations.len();
                    self.lower(s.orelse, lines, function, active.as_deref_mut())?;
                    // CPython emits annotations from else before annotations from handlers.
                    let handler_count = before_else - before_handlers;
                    let else_count = self.deferred_annotations.len() - before_else;
                    if handler_count != 0 && else_count != 0 {
                        self.deferred_annotations[before_handlers..].rotate_left(handler_count);
                    }
                    continue;
                }
                Stmt::Try(mut s) if !s.is_star && !s.finalbody.is_empty() => {
                    let finalbody = std::mem::take(&mut s.finalbody);
                    let exits_before = active.as_deref().copied();
                    // The finally surrounds the handlers and else suite as well as the try body.
                    let protected = if s.handlers.is_empty() {
                        s.body
                    } else {
                        std::iter::once(Stmt::Try(s)).collect()
                    };
                    self.lower(protected, lines, function, active.as_deref_mut())?;
                    let exits_after = active.as_deref().copied();
                    let first = self.deferred_annotations.len();
                    self.lower(finalbody, lines, function, active.as_deref_mut())?;
                    let annotation_count = self.deferred_annotations.len() - first;
                    if annotation_count != 0 {
                        // A break or continue crossing this finally has its own inlined
                        // annotation sites at the jump, before the fallthrough copy.
                        if exits_after != exits_before {
                            return Err(unsupported("loop exit across an annotated finally"));
                        }
                        // CPython emits another copy of finally for exception unwinding.
                        // Keep both ranges of deferred annotation sites.
                        let exceptional = self.deferred_annotations.len();
                        self.deferred_annotations.extend_from_within(first..);
                        if self.class_attributes.is_some() {
                            self.conditional_fields
                                .extend(exceptional..exceptional + annotation_count);
                        }
                    }
                    continue;
                }
                Stmt::Match(s) => {
                    let dispatch = self.items.len();
                    let mut cases = Vec::new();
                    let count = s.cases.len();
                    for (i, case) in s.cases.into_iter().enumerate() {
                        let captures = self.case_captures(&case, i + 1 < count)?;
                        cases.push(SourceCase {
                            pattern: case.pattern,
                            captures,
                            guard: case.guard,
                        });
                        self.lower(case.body, lines, function, active.as_deref_mut())?;
                    }
                    self.items.insert(
                        dispatch,
                        (
                            line,
                            SourceItem::Match {
                                subject: *s.subject,
                                cases,
                            },
                        ),
                    );
                    continue;
                }
                Stmt::If(s) => {
                    self.record_expression(*s.test, line, function)?;
                    self.lower(s.body, lines, function, active.as_deref_mut())?;
                    for clause in s.elif_else_clauses {
                        if let Some(test) = clause.test {
                            self.record_expression(test, line, function)?;
                        }
                        self.lower(clause.body, lines, function, active.as_deref_mut())?;
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
            self.items.push((line, op));
        }
        Ok(())
    }

    /// Keep scanning declarations when a pattern already prevents execution.
    fn invalid_transfer(
        &self,
        range: ruff_text_size::TextRange,
        message: &'static str,
    ) -> Result<()> {
        let Some(pending) = &self.deferred_pattern else {
            return Err(Error::source_syntax(message, range));
        };
        let mut first = pending.borrow_mut();
        if range.start() < first.0.start() {
            *first = (range, message.to_owned().into());
        }
        Ok(())
    }

    fn case_captures(&mut self, case: &ast::MatchCase, has_next: bool) -> Result<Vec<String>> {
        use ast::visitor::{self, Visitor};

        struct Operands<'a> {
            locals: &'a mut indexmap::IndexSet<String>,
            supported: bool,
        }
        impl<'a> Visitor<'a> for Operands<'_> {
            fn visit_pattern(&mut self, pattern: &'a ast::Pattern) {
                visitor::walk_pattern(self, pattern);
                self.supported &= match pattern {
                    ast::Pattern::MatchMapping(_) => false,
                    ast::Pattern::MatchClass(p) => {
                        p.arguments.patterns.is_empty() && p.arguments.keywords.is_empty()
                    }
                    _ => true,
                };
                if let Some(name) = imre_ast::executable::capture_name(pattern) {
                    self.locals.insert(name.to_string());
                }
            }

            fn visit_expr(&mut self, _: &'a ast::Expr) {}
        }

        let mut operands = Operands {
            locals: &mut self.locals,
            supported: true,
        };
        operands.visit_pattern(&case.pattern);
        // Finish collecting symbols in a rejected module. Its captures will never execute.
        if self.deferred_pattern.is_some() {
            return Ok(Vec::new());
        }
        if !operands.supported {
            return Err(unsupported("match pattern"));
        }
        imre_ast::executable::source_case_bindings(
            case,
            has_next,
            self.private_names.as_deref(),
            &mut HashMap::new(),
        )
        .map_err(|(range, message)| {
            match message.render(crate::integer_text::check_decimal) {
                Ok(reason) => Error::source_syntax(reason.into_owned(), range),
                Err(error) => error.into(),
            }
        })
    }

    /// Declare every local name in a loop or unpacking target.
    fn declare_target(&mut self, target: &ast::Expr) -> Result<()> {
        try_collect_target_names(target, &mut self.locals).map_err(invalid_target)
    }
}

/// CPython symbol analysis and code generation expose source ranges differently.
#[derive(Clone, Copy, Debug)]
pub(super) enum SyntaxSpan {
    /// Code generation includes the location tuple in SyntaxError.args.
    InArgs(ruff_text_size::TextRange),
    Artificial,
    /// Symbol analysis only sets the location attributes.
    InFields(ruff_text_size::TextRange),
}

pub(super) const SYNTAX_MEMBERS: &[&str] = &[
    "msg",
    "filename",
    "lineno",
    "offset",
    "text",
    "end_lineno",
    "end_offset",
    "print_file_and_line",
    "_metadata",
];

pub(super) fn syntax_member(name: &str) -> Option<usize> {
    SYNTAX_MEMBERS.iter().position(|field| *field == name)
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

    fn syntax_program_line(&mut self, filename: Id, lineno: usize) -> Option<Id> {
        use std::io::{BufRead, BufReader};
        let file = std::fs::File::open(self.import_path(filename).ok()?).ok()?;
        if !file.metadata().ok()?.is_file() {
            return None;
        }
        let mut reader = BufReader::new(file);
        let mut bytes = Vec::new();
        // The tokenizer uses the first two lines to resolve the file's encoding.
        for _ in 0..lineno.max(2) {
            if reader.read_until(b'\n', &mut bytes).ok()? == 0 {
                break;
            }
        }
        let terminated = bytes.ends_with(b"\n") || bytes.ends_with(b"\r");
        let mut lines = crate::objects::decode_source_lines(&bytes).ok()?;
        if lineno == lines.len() && !terminated {
            lines.last_mut()?.pop();
        }
        (lineno <= lines.len()).then(|| self.text(lines.swap_remove(lineno - 1)))
    }

    pub(super) fn located_syntax_error(
        &mut self,
        message: &str,
        span: SyntaxSpan,
        filename: Id,
        lines: &[usize],
    ) -> Result<Error> {
        let (SyntaxSpan::InArgs(range) | SyntaxSpan::InFields(range)) = span else {
            return self.syntax_error_at(message, filename, [-1, -1, 0, 0], true);
        };
        let position = |offset: ruff_text_size::TextSize| {
            let offset = offset.to_usize();
            let line = lines.partition_point(|start| *start <= offset);
            (line as i64, (offset - lines[line - 1] + 1) as i64)
        };
        let (line, column) = position(range.start());
        let (end_line, end_column) = position(range.end());
        self.syntax_error_at(
            message,
            filename,
            [line, end_line, column, end_column],
            matches!(span, SyntaxSpan::InArgs(_)),
        )
    }

    pub(super) fn parser_syntax_error(
        &mut self,
        filename: Id,
        display: &str,
        detail: &str,
        prefix_filename: bool,
        [row, end_row, column, end_column]: [i64; 4],
        text: String,
    ) -> Result<Error> {
        let msg = self.text(detail);
        let (msg, message) = if prefix_filename {
            (
                self.concat_strings(filename, msg)?,
                format!("{display}{detail}"),
            )
        } else {
            (msg, detail.to_owned())
        };
        let details = vec![
            filename,
            self.int(row),
            self.int(column),
            self.text(text),
            self.int(end_row),
            self.int(end_column),
        ];
        let location = self.alloc(Value::Tuple(details));
        let args = [msg, location];
        let mut error = self.exception("SyntaxError", &args);
        error.message = message;
        self.retain_syntax_fields(&mut error, self.syntax_fields(&args))?;
        Ok(error)
    }

    pub(super) fn syntax_error_at(
        &mut self,
        message: &str,
        filename: Id,
        [line, end_line, column, end_column]: [i64; 4],
        in_args: bool,
    ) -> Result<Error> {
        let text = usize::try_from(line)
            .ok()
            .and_then(|line| self.syntax_program_line(filename, line))
            .unwrap_or(NONE);
        let msg = self.text(message);
        let line = self.int(line);
        let location = [
            filename,
            line,
            self.int(column),
            text,
            self.int(end_line),
            self.int(end_column),
        ];
        let mut error = if in_args {
            let details = self.alloc(Value::Tuple(location.to_vec()));
            self.exception("SyntaxError", &[msg, details])
        } else {
            let mut error = self.exception("SyntaxError", &[msg]);
            error.message.push_str(&self.syntax_suffix(filename, line)?);
            error
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
        self.compile_source_with_flags(
            source,
            filename,
            mode,
            strip_indent,
            self.inherited_compile_flags(),
        )
    }

    pub(super) fn inherited_compile_flags(&self) -> u32 {
        match self.heap.get(self.heap.current_code()) {
            Value::Code(caller) => {
                caller.flags & (code_flags::FUTURE_ANNOTATIONS | code_flags::FUTURE_BARRY_AS_BDFL)
            }
            _ => 0,
        }
    }

    fn source_filename(&self, object: Id) -> Result<SourceFilename> {
        Ok(SourceFilename {
            object,
            display: Arc::from(self.filename_display(object)?.as_ref()),
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
        let parse_error = parsed
            .errors()
            .first()
            .map(|error| (error.location, format!(": {error}"), true))
            .or_else(|| {
                imre_ast::comprehension::dict_unpacking_error(&parsed)
                    .map(|(range, reason)| (range, reason.to_owned(), false))
            });
        if let Some((range, detail, prefix_filename)) = parse_error {
            let position = |offset: ruff_text_size::TextSize| {
                let byte = offset.to_usize();
                let row = lines.partition_point(|&start| start <= byte);
                let column = source[lines[row - 1]..byte].chars().count() + 1;
                (row, column)
            };
            let (row, column) = position(range.start());
            let (end_row, end_column) = position(range.end());
            let end = lines.get(row).copied().unwrap_or(source.len());
            let mut text = source[lines[row - 1]..end].to_owned();
            if mode == "exec" && !text.ends_with('\n') {
                text.push('\n');
            }
            return Err(self.parser_syntax_error(
                filename_id,
                &filename.display,
                &detail,
                prefix_filename,
                [row as i64, end_row as i64, column as i64, end_column as i64],
                text,
            )?);
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
            && let Some((range, message)) = imre_ast::annotation_scope_error(&module.body)
        {
            return Err(self.located_syntax_error(
                &message,
                SyntaxSpan::InFields(range),
                filename_id,
                &lines,
            )?);
        }
        let bindings = match parsed.syntax() {
            ast::Mod::Module(module) => {
                imre_ast::executable::BindingErrors::check(&module.body, source)
            }
            _ => Default::default(),
        };
        let symbol_error = bindings.first_symbol(future_error);
        let mut code = Program::new(filename, "<module>", source.contains(":="));
        code.deferred_pattern = bindings
            .pattern
            .map(|issue| Rc::new(std::cell::RefCell::new(issue)));
        let pending_syntax = |ctx: &mut Self, code: &Program| -> Result<Option<Error>> {
            let locate = |ctx: &mut Self, message: &str, span| {
                ctx.located_syntax_error(message, span, filename_id, &lines)
                    .map(Some)
            };
            if let Some((range, message)) = symbol_error {
                return locate(ctx, message, SyntaxSpan::InFields(range));
            }
            if let Some(issue) = bindings.handler {
                let span = issue
                    .location
                    .map_or(SyntaxSpan::Artificial, SyntaxSpan::InArgs);
                return locate(ctx, issue.message, span);
            }
            if let Some(deferred) = &code.deferred_pattern {
                let (range, message) = &*deferred.borrow();
                return locate(
                    ctx,
                    &message.render(crate::integer_text::check_decimal)?,
                    SyntaxSpan::InArgs(*range),
                );
            }
            Ok(None)
        };
        code.scope_flags = inherited;
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
                code.items
                    .push((1, SourceItem::Return(Some(*expression.body))));
            }
            ast::Mod::Module(module) => {
                let mut body = module.body;
                if !code.strip_docstrings && !spellings.is_empty() {
                    self.check_source_docstrings(&body, !interactive, &spellings)?;
                }
                if imre_ast::future_import(&body, "barry_as_FLUFL") {
                    code.scope_flags |= code_flags::FUTURE_BARRY_AS_BDFL;
                }
                if code.scope_flags & code_flags::FUTURE_ANNOTATIONS != 0
                    || imre_ast::future_annotations(&body)
                {
                    code.scope_flags |= code_flags::FUTURE_ANNOTATIONS;
                    code.private_names = Some(Rc::new(imre_ast::lower_in(&mut body, "", true)));
                }
                code.syntax_limit = symbol_error.map(|(range, _)| range);
                let lowering = code.lower_namespace(body, &lines, false).and_then(|()| {
                    // Closure binding resolution follows symbol collection and future placement.
                    if symbol_error.is_none() {
                        let mut module_globals = HashSet::new();
                        code.resolve_directives(&|_| None, &mut module_globals)?;
                        // A global declared by a nested block makes module assignments use the globals dictionary.
                        for name in module_globals {
                            code.directives
                                .entry(name)
                                .or_insert((ScopeDirective::Global, Default::default()));
                        }
                    }
                    Ok(())
                });
                if let Err(error) = lowering {
                    if error.kind == "NotImplementedError"
                        && let Some(issue) = pending_syntax(self, &code)?
                    {
                        return Err(issue);
                    }
                    if let Some(span) = error.trace.as_ref().and_then(|trace| trace.pending_syntax)
                    {
                        return Err(self.located_syntax_error(
                            &error.message,
                            span,
                            filename_id,
                            &lines,
                        )?);
                    }
                    return Err(error);
                }
            }
        }
        code.prepare_strings(
            &mut self.heap,
            &mut SharedConstants::default(),
            &spellings,
            &|offset| lines.partition_point(|n| *n <= offset) as u32,
        )?;
        if let Some(issue) = pending_syntax(self, &code)? {
            return Err(issue);
        }
        Ok(self.box_source_code(code))
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

    fn box_source_code(&mut self, program: Program) -> Id {
        let name = self.heap.code_name(&program.name);
        let qualname = self.heap.code_name(&program.qualname);
        let filename = program.filename.object;
        let doc = program
            .doc
            .and_then(|range| program.literals.get(&range).copied())
            .unwrap_or(NONE);
        self.alloc(Value::Code(value::CodeObject {
            compilation_constants: Default::default(),
            parsed_parameters: Default::default(),
            varnames: NONE,
            cellvars: NONE,
            freevar_names: NONE,
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
            generator_lambda: program.kind == FunctionKind::GeneratorLambda,
            body: Body::Parsed {
                constants: program.string_roots,
            },
            parameters: NONE,
            nested: Vec::new(),
            closure_supported: false,
            freevars: Vec::new(),
        }))
    }

    /// Define a function whose parameters and executable belong to the generated code object.
    fn source_function(&mut self, code: Id, scope: Id, defaults: Id) -> Result<Id> {
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
        let required = data.freevars.len();
        let code_name = data.name;
        let qualname = data.qualname;
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
            _ if closure == NONE && required == 0 => &[],
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
        if cells.len() != required {
            return Err(Error::new(
                "ValueError",
                format!(
                    "{} requires closure of length {}, not {}",
                    self.str_value(code_name)?,
                    required,
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
        let builtins = match self.dict_named_item(globals, "__builtins__") {
            Some(value) => self.definition_builtins(value)?,
            None => self.caller_builtins()?,
        };
        let scope = self.function_environment("", globals, builtins);
        Ok(
            self.function_with_closure(
                code,
                scope,
                closure,
                (name, qualname),
                defaults,
                kwdefaults,
            ),
        )
    }

    /// Bind a validated closure to its code object's free variables in order.
    fn function_with_closure(
        &mut self,
        code: Id,
        scope: Id,
        closure: Id,
        (name, qualname): (Id, Id),
        defaults: Id,
        kwdefaults: Id,
    ) -> Id {
        let data = self.code_object(code);
        let freevars = data.freevars.clone();
        let code_doc = data.doc;
        let cells = match self.heap.get(closure) {
            Value::Tuple(cells) => cells.to_vec(),
            _ if closure == NONE => Vec::new(),
            _ => unreachable!("validated function closure"),
        };
        let class_cell = freevars
            .iter()
            .position(|name| name == "__class__")
            .map(|index| cells[index]);
        let scope = self.closure_environment(scope, freevars, &cells);
        let fields = super::function_fields::FunctionFields::new(
            self, scope, name, qualname, code_doc, NONE,
        );
        self.alloc(Value::Function {
            fields,
            closure,
            code,
            scope,
            class_cell,
            defaults,
            kwdefaults,
        })
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

    /// Native entry has no Python frame whose builtins the new namespace can inherit.
    fn caller_builtins(&mut self) -> Result<Id> {
        if self.heap.python_depth() == 0 {
            self.default_source_builtins()
        } else {
            self.function_builtins(self.heap.current_scope())
        }
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
        if let Some(&LinkedSource::Expression(entry)) = self.heap.linked_sources.get(&text) {
            let code = self.module_code(
                ModuleInitializer::Native(entry),
                std::path::Path::new("<string>"),
            )?;
            return self.execute_namespace_code(code, globals, Some(locals), None, "eval", None);
        }
        if crate::objects::python_isidentifier(source)
            && !ruff_python_stdlib::keyword::is_keyword(source)
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
        let code = match self.compile_source(text, filename, mode, false) {
            Ok(code) => code,
            Err(error) => {
                // Preserve the forward-reference diagnostic for invalid grammar, while
                // keeping errors from scope checking or allocation as originally raised.
                let source = self.str_value(text)?;
                if ruff_python_parser::parse_expression(source).is_err() {
                    return Err(Error::new(
                        "SyntaxError",
                        format!("Forward reference must be an expression -- got {source:?}"),
                    ));
                }
                return Err(error);
            }
        };
        self.execute_namespace_code(code, globals, Some(locals), None, "eval", None)
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
        let value = self.execute_namespace_code(source, globals, locals, None, name, closure)?;
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
            for name in super::code_locals::argument_slots(&signature) {
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
        self.execute_namespace_code(source, globals, locals, function, "exec", None)
            .map(|_| NONE)
    }

    pub(super) fn namespace_builtins(&mut self, globals: Id, function: Option<Id>) -> Result<Id> {
        let builtins = if let Some(function) = function
            && let Value::Function { scope, .. } = *self.heap.get(function)
        {
            self.function_builtins(scope)?
        } else if let Some(builtins) = self.dict_named_item(globals, "__builtins__") {
            self.definition_builtins(builtins)?
        } else {
            let builtins = self.caller_builtins()?;
            let key = self.text("__builtins__");
            self.put_item(
                self.dict_storage(globals).expect("exec dictionary"),
                key,
                builtins,
            )?;
            builtins
        };
        Ok(builtins)
    }

    fn execute_namespace_code(
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
        let builtins = self.namespace_builtins(globals, function)?;
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
                    if !matches!(code.body, Body::Native(..) | Body::Suspended(_)) {
                        return Err(unsupported("exec with an explicit closure"));
                    }
                    source
                } else {
                    return Err(Error::new(
                        "TypeError",
                        format!("code object requires a closure of exactly length {required}"),
                    ));
                }
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
        let scope = self.function_environment("<exec>", globals, builtins);
        let code = self.code_object(code_id);
        if code.parameters != NONE {
            // A complete layout can bind even a nested body without its defining frame.
            if code.closure_supported || closure.is_some() {
                let cells = closure.unwrap_or(NONE);
                let name = code.name;
                let function =
                    self.function_with_closure(code_id, scope, cells, (name, name), NONE, NONE);
                return self.call(function, Vec::new(), Vec::new());
            }
            if !code.module_definition {
                return Err(unsupported(
                    "exec of a prelinked function with an enclosing scope",
                ));
            }
            let defaults = self.dict(Vec::new());
            let function = self.source_function(code_id, scope, defaults)?;
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

    pub(super) fn missing_compiled_body<T>(&self, code: Id) -> Result<T> {
        let info = self.code_object(code);
        Err(Error::new(
            "NotImplementedError",
            format!(
                "no compiled Python body for {} ({}:{})",
                self.str_value(info.qualname)?,
                self.filename_display(info.filename)?,
                info.first_line
            ),
        ))
    }

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
        if matches!(
            self.heap.get(target),
            Value::Dict(_) | Value::DefaultDict { .. }
        ) {
            self.store_binding_item(target, key, value)?;
        } else {
            self.put_item(target, key, value)?;
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
    pub(super) fn import_in_scope(
        &mut self,
        owner: Id,
        scope: Id,
        modname: Id,
        fromlist: Id,
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
}
