//! Runtime code metadata, compilation of source objects and Python namespace operations.

mod annotations;
mod comprehensions;

use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::rc::Rc;

use imre_ast::code_flags;
use imre_ast::comprehension::{
    InvalidTarget, Output, Plan, outer_clause, try_collect_target_names,
};
use ruff_python_ast as ast;
use ruff_text_size::{Ranged, TextRange};

use super::value::{Body, ExecGlobals};
use super::*;

pub(super) const CONDITIONAL: &str = "__conditional_annotations__";

/// Only the module code object escapes validation of a source string.
struct ParsedModule {
    constants: Id,
    flags: u32,
}

/// An explicit Python closure tuple keeps its identity. A compiled binding can
/// expose the same cells lazily through the ordinary function attribute.
pub(super) enum FunctionClosure<'a> {
    Tuple(Id),
    Cells(&'a [Id]),
}

struct Program {
    // Stop symbol collection at the first invalid binding or misplaced future import.
    syntax_limit: Option<ruff_text_size::TextRange>,
    // An invalid pattern still has a symbol table. Let all its scopes finish checking
    // declarations before reporting the code-generation error. Valid modules use no slot.
    deferred_pattern:
        Option<Rc<std::cell::RefCell<(TextRange, imre_ast::executable::PatternMessage)>>>,
    pub name: String,
    class_scope: bool,
    has_named_bindings: bool,
    strip_docstrings: bool,
    is_async: bool,
    interactive: bool,
    locals: indexmap::IndexSet<String>,
    directives: HashMap<String, (ScopeDirective, ruff_text_size::TextRange)>,
    private_names: Option<Rc<imre_ast::Names>>,
    items: Vec<SourceItem>,
    // Declarations in source lowering order, including nested suites.
    deferred_annotations: Vec<(String, ast::Expr)>,
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
}

/// Suspension sites among the supported operands of one expression. The plan is discarded
/// after collecting code metadata; untouched subtrees are visited in place.
struct OperandPlan {
    sites: smallvec::SmallVec<[ruff_text_size::TextSize; 1]>,
}

impl OperandPlan {
    fn new(root: &ast::Expr, is_async: bool) -> Self {
        let mut sites = smallvec::SmallVec::new();
        let mut work = smallvec::SmallVec::<[&ast::Expr; 8]>::new();
        work.push(root);
        while let Some(expr) = work.pop() {
            match expr {
                ast::Expr::Await(a) if is_async => {
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
        Self { sites }
    }

    fn for_function(expr: &ast::Expr, is_async: bool) -> Option<Self> {
        (is_async || code_flags::yields_in_expression(expr)).then(|| Self::new(expr, is_async))
    }

    fn suspends(&self, expr: &ast::Expr) -> bool {
        let first = self.sites.partition_point(|&site| site < expr.start());
        self.sites.get(first).is_some_and(|&site| site < expr.end())
    }
}

/// Parsed items used to construct and validate code-object metadata.
enum SourceItem {
    Discard(ast::Expr),
    Define(Definition),
    Annotations(Vec<(String, ast::Expr)>),
    Docstring(ast::StringLiteralValue),
    StringAnnotation(String, ast::Expr),
    MakeGenerator(ast::Expr),
    Pattern(ast::Pattern, Box<[String]>),
    Name(ast::Identifier),
    Names(Vec<ast::name::Name>),
}

struct Definition {
    binding: String,
    decorators: Vec<ast::Decorator>,
    body: Box<Program>,
    kind: DefinitionKind,
}

enum DefinitionKind {
    TypeAlias(Option<Box<ast::TypeParams>>),
    Function {
        parameters: Box<ast::Parameters>,
        type_params: Option<Box<ast::TypeParams>>,
    },
    Class {
        arguments: Option<Box<ast::Arguments>>,
        type_params: Option<Box<ast::TypeParams>>,
    },
}

impl DefinitionKind {
    fn type_params(&self) -> Option<&ast::TypeParams> {
        match self {
            Self::Function { type_params, .. }
            | Self::Class { type_params, .. }
            | Self::TypeAlias(type_params) => type_params.as_deref(),
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
    // Anonymous expression scopes validate their bindings after lexical resolution.
    fn declare(self, mut program: Option<&mut Program>) -> Result<()> {
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
            if comprehension && program.as_ref().is_some_and(|p| p.class_scope) {
                return Err(Error::new(
                    "SyntaxError",
                    "assignment expression within a comprehension cannot be used in a class body",
                ));
            }
            if let Some((kind, directive)) = program.as_ref().and_then(|p| p.directives.get(&name))
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
            let Some(program) = program.as_deref_mut() else {
                continue;
            };
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
struct SharedConstants {
    // Compare the spelling on the live constant, so pooling never borrows from a child AST.
    text: hashbrown::HashTable<(u64, Id)>,
    hasher: std::hash::RandomState,
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

/// Collect names and string constants without recursing through an expression's left spine.
struct CompiledStrings<'a, 'ctx> {
    heap: &'ctx mut value::Heap,
    shared: &'ctx mut SharedConstants,
    spellings: &'ctx StringReplacements,
    roots: Vec<Id>,
    pending: Vec<&'a ast::Expr>,
    directives: &'ctx HashMap<String, (ScopeDirective, ruff_text_size::TextRange)>,
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

impl<'a, 'ctx> CompiledStrings<'a, 'ctx> {
    fn new(
        heap: &'ctx mut value::Heap,
        shared: &'ctx mut SharedConstants,
        spellings: &'ctx StringReplacements,
        directives: &'ctx HashMap<String, (ScopeDirective, TextRange)>,
    ) -> Self {
        Self {
            heap,
            shared,
            spellings,
            directives,
            pending: Vec::new(),
            roots: Vec::new(),
            exhausted: false,
        }
    }

    fn keep(&mut self, id: Id) -> Result<()> {
        self.roots
            .try_reserve(1)
            .map_err(|_| Error::new("MemoryError", ""))?;
        self.roots.push(id);
        Ok(())
    }

    fn text(&mut self, text: Cow<'a, str>) -> Result<Id> {
        use std::hash::BuildHasher;
        let hash = self.shared.hasher.hash_one(text.as_ref());
        if let Some(&(_, id)) = self.shared.text.find(
            hash,
            |&(_, id)| matches!(self.heap.get(id), Value::Str(s) if s.as_str() == text.as_ref()),
        ) {
            return Ok(id);
        }
        self.shared
            .text
            .try_reserve(1, |entry| entry.0)
            .map_err(|_| crate::Error::allocation())?;
        let id = self
            .heap
            .source_string_constant(&text)
            .ok_or_else(crate::Error::allocation)?;
        self.shared
            .text
            .insert_unique(hash, (hash, id), |entry| entry.0);
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
            self.keep(id)?;
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
                        self.keep(id)?;
                    }
                }
            }
        }
        items
            .try_reserve(1)
            .map_err(|_| crate::Error::allocation())?;
        items.push(self.template_text(segment)?);
        let id = self.tuple_constant(items)?;
        self.keep(id)
    }

    /// Only defaults and the outer iterable of a comprehension are evaluated in this scope.
    fn check_directive_reads(&mut self) -> Result<()> {
        use ast::visitor::Visitor;
        let roots = std::mem::take(&mut self.pending);
        for root in &roots {
            self.visit_expr(root);
        }
        while let Some(expr) = self.pending.pop() {
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

    fn scan(&mut self) -> Result<Vec<&'a ast::Expr>> {
        use ast::visitor::Visitor;
        let mut functions = Vec::new();
        if !self.directives.is_empty() {
            self.check_directive_reads()?;
        }
        while let Some(expr) = self.pending.pop() {
            match expr {
                ast::Expr::Name(name) => {
                    self.heap.code_name(name.id.as_str());
                }
                ast::Expr::Attribute(attr) => {
                    self.heap.code_name(attr.attr.as_str());
                }
                ast::Expr::Lambda(lambda) => {
                    functions
                        .try_reserve(1)
                        .map_err(|_| Error::new("MemoryError", ""))?;
                    functions.push(expr);
                    if let Some(parameters) = &lambda.parameters {
                        self.visit_parameters(parameters);
                    }
                    continue;
                }
                ast::Expr::Generator(genexpr) => {
                    functions
                        .try_reserve(1)
                        .map_err(|_| Error::new("MemoryError", ""))?;
                    functions.push(expr);
                    self.visit_expr(&outer_clause(&genexpr.generators).iter);
                    continue;
                }
                expr if let Some(plan) = Plan::from_expr(expr) => {
                    self.visit_expr(plan.outer);
                    plan.try_target_names().map_err(invalid_target)?;
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
                    self.keep(id)?;
                    continue;
                }
                _ => {}
            }
            ast::visitor::walk_expr(self, expr);
        }
        if self.exhausted {
            return Err(Error::new("MemoryError", ""));
        }
        Ok(functions)
    }

    fn prepare_children(
        &mut self,
        functions: Vec<&'a ast::Expr>,
        has_named_bindings: bool,
        module: bool,
    ) -> Result<()> {
        use ast::visitor::Visitor;
        enum Child<'a> {
            Expression(&'a ast::Expr),
            Lowered(Box<Program>),
        }
        let mut children = HashMap::new();
        children
            .try_reserve(functions.len())
            .map_err(|_| crate::Error::allocation())?;
        for expr in functions {
            let child = match expr {
                ast::Expr::Lambda(lambda) => {
                    if has_named_bindings {
                        let mut named = NamedBindings::default();
                        named.visit_expr(&lambda.body);
                        named.declare(None)?;
                    }
                    if let Some(plan) = OperandPlan::for_function(&lambda.body, false) {
                        let mut child = Program::new("<lambda>", has_named_bindings);
                        child.record_operand(*lambda.body.clone(), &plan)?;
                        Child::Lowered(Box::new(child))
                    } else {
                        Child::Expression(&lambda.body)
                    }
                }
                ast::Expr::Generator(genexpr) => {
                    let mut child = Program::new("<genexpr>", has_named_bindings);
                    if has_named_bindings {
                        let mut bindings = NamedBindings::default();
                        bindings.visit_expr(expr);
                        for (name, offset, _) in bindings.targets {
                            let kind = if module
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
                                .insert(name, (kind, TextRange::empty(offset)));
                        }
                    }
                    child.lower_genexpr(genexpr)?;
                    Child::Lowered(Box::new(child))
                }
                _ => unreachable!("anonymous function expression"),
            };
            children.insert(expr.range(), child);
        }
        for child in children.into_values() {
            let roots = match child {
                Child::Expression(body) => {
                    let directives = HashMap::new();
                    let mut strings =
                        CompiledStrings::new(self.heap, self.shared, self.spellings, &directives);
                    strings.visit_expr(body);
                    strings.finish_nested_scope(has_named_bindings)?
                }
                Child::Lowered(mut program) => {
                    program.prepare_strings(self.heap, self.shared, self.spellings)?
                }
            };
            if roots != NONE {
                self.keep(roots)?;
            }
        }
        Ok(())
    }

    fn finish_nested_scope(mut self, has_named_bindings: bool) -> Result<Id> {
        let children = self.scan()?;
        self.prepare_children(children, has_named_bindings, false)?;
        Ok(self.finish())
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

/// Normalize physical line endings before parsing and measuring offsets.
fn normalized_source(source: &str) -> Cow<'_, str> {
    if source.contains('\r') {
        Cow::Owned(imre_ast::source_encoding::universal_newlines(
            source.to_owned(),
        ))
    } else {
        Cow::Borrowed(source)
    }
}

/// Convert a parser byte range in normalized source to Python grammar-error fields.
fn parser_location(
    source: &str,
    lines: &[usize],
    range: TextRange,
    ensure_newline: bool,
) -> ([i64; 4], String) {
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
    if ensure_newline && !text.ends_with('\n') {
        text.push('\n');
    }
    ([row, end_row, column, end_column].map(|n| n as i64), text)
}

/// Collect source line offsets before constructing code or AST nodes.
pub(super) fn line_starts(source: &str) -> Result<Vec<usize>> {
    let offsets = source.match_indices('\n').map(|(offset, _)| offset + 1);
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

    pub(super) fn raise<T>(self, ctx: &mut Context, filename: Id) -> Result<T> {
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

impl Program {
    /// Keep each code object's constants alive, including constants owned by unexecuted children.
    fn prepare_strings(
        &mut self,
        heap: &mut value::Heap,
        shared: &mut SharedConstants,
        spellings: &StringReplacements,
    ) -> Result<Id> {
        self.prepare_scope_strings(heap, shared, spellings, None)
    }

    fn prepare_scope_strings<'a>(
        &'a mut self,
        heap: &mut value::Heap,
        shared: &mut SharedConstants,
        spellings: &StringReplacements,
        expression_body: Option<&'a ast::Expr>,
    ) -> Result<Id> {
        use ast::visitor::Visitor;
        let stringized = self.string_annotations();
        let mut strings = CompiledStrings::new(heap, shared, spellings, &self.directives);
        // Module annotation constants are prepared before its statements; class annotations follow them.
        if !self.deferred_annotations.is_empty() {
            strings.annotation_scope(
                &std::mem::take(&mut self.deferred_annotations),
                false,
                stringized,
                self.has_named_bindings,
            )?;
        }
        let mut prepared_generators = Vec::new();
        let mut items = std::mem::take(&mut self.items);
        for op in &mut items {
            match op {
                SourceItem::Discard(e) => strings.visit_expr(e),
                SourceItem::Name(name) => {
                    strings.heap.code_name(name.as_str());
                }
                SourceItem::Names(names) => {
                    for name in names {
                        strings.heap.code_name(name.as_str());
                    }
                }
                SourceItem::Pattern(pattern, captures) => {
                    strings.visit_pattern(pattern);
                    for name in captures.iter() {
                        strings.heap.code_name(name);
                    }
                }
                SourceItem::MakeGenerator(expr) => {
                    prepared_generators.push(&*expr);
                }
                SourceItem::Docstring(doc) => {
                    let text = imre_ast::strings::clean_doc(doc.to_str());
                    let id = strings.text(text.into())?;
                    strings.keep(id)?;
                }
                SourceItem::Annotations(entries) => {
                    strings.annotation_scope(
                        entries,
                        self.class_scope,
                        stringized,
                        self.has_named_bindings,
                    )?;
                }
                SourceItem::StringAnnotation(name, expr) => strings.annotation(name, expr, true)?,
                SourceItem::Define(definition) => {
                    let child = &mut definition.body;
                    strings.heap.code_name(&definition.binding);
                    let mut roots =
                        child.prepare_strings(strings.heap, strings.shared, spellings)?;
                    if let DefinitionKind::Class {
                        arguments,
                        type_params: Some(_),
                    } = &definition.kind
                    {
                        // A generic class's bases and keywords belong to its type-parameter scope.
                        let directives = HashMap::new();
                        let mut header = CompiledStrings::new(
                            strings.heap,
                            strings.shared,
                            spellings,
                            &directives,
                        );
                        if roots != NONE {
                            header.keep(roots)?;
                        }
                        if let Some(args) = arguments {
                            header.visit_arguments(args);
                        }
                        roots = header.finish_nested_scope(self.has_named_bindings)?;
                    }
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
                            type_params: None,
                        } => strings.visit_arguments(args),
                        _ => {}
                    }
                }
            }
        }
        if let Some(body) = expression_body {
            strings.visit_expr(body);
        }
        let mut functions = strings.scan()?;
        functions.extend(prepared_generators);
        strings.prepare_children(functions, self.has_named_bindings, self.name == "<module>")?;
        Ok(strings.finish())
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
    ) -> Result<()> {
        self.locals
            .retain(|name| !self.directives.contains_key(name));
        for (name, (kind, range)) in &self.directives {
            if *kind == ScopeDirective::Global {
                continue;
            }
            if let Some(binding) = enclosing(name) {
                if matches!(binding, EnclosingBinding::TypeParameter) {
                    return Err(Error::new(
                        "SyntaxError",
                        format!("nonlocal binding not allowed for type parameter '{name}'"),
                    ));
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
        let function = self.name != "<module>" && !self.class_scope;
        let bound = |name: &str| {
            if self.class_scope && name == "__class__" {
                if !self.locals.contains(name)
                    && matches!(enclosing(name), Some(EnclosingBinding::TypeParameter))
                {
                    return Some(EnclosingBinding::TypeParameter);
                }
                Some(EnclosingBinding::Local)
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
                    (self.class_scope && name == CONDITIONAL).then_some(EnclosingBinding::Local)
                })
            }
        };
        for op in &mut self.items {
            if let SourceItem::Define(definition) = op {
                let child = &mut definition.body;
                if let Some(params) = definition.kind.type_params() {
                    child.resolve_directives(&|name| {
                        if params.iter().any(|p| p.name().as_str() == name) {
                            Some(EnclosingBinding::TypeParameter)
                        } else {
                            bound(name)
                        }
                    })?;
                } else {
                    child.resolve_directives(&bound)?;
                }
            }
        }
        Ok(())
    }

    fn nested_program(&self, name: &str) -> Self {
        let mut child = Self::new(name, self.has_named_bindings);

        child.syntax_limit = self.syntax_limit;
        child.deferred_pattern.clone_from(&self.deferred_pattern);
        child.strip_docstrings = self.strip_docstrings;
        child
    }

    fn new(name: &str, has_named_bindings: bool) -> Self {
        Self {
            syntax_limit: None,
            deferred_pattern: None,
            name: name.to_owned(),
            class_scope: false,
            has_named_bindings,
            strip_docstrings: false,
            is_async: false,
            interactive: false,
            locals: indexmap::IndexSet::new(),
            directives: HashMap::new(),

            private_names: None,
            items: Vec::new(),
            deferred_annotations: Vec::new(),
        }
    }

    fn lower_suspension(&mut self, expr: ast::Expr, function: bool) -> Result<Option<ast::Expr>> {
        if !function {
            if matches!(expr, ast::Expr::Yield(_) | ast::Expr::YieldFrom(_)) {
                self.invalid_transfer(expr.range(), "'yield' outside function")?;
            }
            return Ok(Some(expr));
        }
        if let Some(plan) = OperandPlan::for_function(&expr, self.is_async) {
            self.lower_operand(expr, &plan)
        } else {
            Ok(Some(expr))
        }
    }

    fn lower_operand(&mut self, expr: ast::Expr, plan: &OperandPlan) -> Result<Option<ast::Expr>> {
        stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || {
            self.emit_operand(expr, plan)
        })
    }

    fn emit_operand(&mut self, expr: ast::Expr, plan: &OperandPlan) -> Result<Option<ast::Expr>> {
        if !plan.suspends(&expr) {
            return Ok(Some(expr));
        }
        if let ast::Expr::Generator(genexpr) = &expr {
            if let Some(outer) =
                self.lower_suspension(outer_clause(&genexpr.generators).iter.clone(), true)?
            {
                self.items.push(SourceItem::Discard(outer));
            }
            self.items.push(SourceItem::MakeGenerator(expr));
            return Ok(None);
        }
        if let Some(comp) = Plan::from_expr(&expr) {
            self.lower_eager_comprehension(&comp)?;
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
                    keywords.push(kw.arg.expect("fixed keyword").id);
                    self.record_operand(kw.value, plan)?;
                }
                self.items.push(SourceItem::Names(keywords));
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
                self.items
                    .push(SourceItem::Name(ast::Identifier::new(name.id, name.range)));
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
                self.items.push(SourceItem::Name(a.attr));
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
            self.items.push(SourceItem::Discard(expr));
        }
        Ok(())
    }

    fn assignment_operands(
        &mut self,
        targets: impl IntoIterator<Item = ast::Expr>,
        value: ast::Expr,
        function: bool,
    ) -> Result<()> {
        let value = self.lower_suspension(value, function)?;
        self.items
            .extend(targets.into_iter().chain(value).map(SourceItem::Discard));
        Ok(())
    }

    fn record_expression(&mut self, expr: ast::Expr, function: bool) -> Result<()> {
        if let Some(expr) = self.lower_suspension(expr, function)? {
            self.items.push(SourceItem::Discard(expr));
        }
        Ok(())
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

    fn definition_body(&self, name: &ast::Identifier) -> Self {
        let mut child = self.nested_program(self.definition_spelling(name));
        child.private_names = self.private_names.clone();
        child
    }

    fn string_annotations(&self) -> bool {
        self.private_names
            .as_ref()
            .is_some_and(|n| n.string_annotations)
    }

    fn lower_namespace(&mut self, mut body: ast::Suite, function: bool) -> Result<()> {
        let class = self.class_scope;
        if !self.interactive
            && (self.strip_docstrings || !class)
            && body.first().is_some_and(ast::helpers::is_docstring_stmt)
        {
            let first = body.remove(0).expr_stmt().unwrap();
            if !self.strip_docstrings {
                // A pending symbol error can exclude the entire function suite.
                if !function
                    || self
                        .syntax_limit
                        .is_none_or(|limit| first.start() < limit.start())
                {
                    let doc = first.value.string_literal_expr().unwrap();
                    self.items.push(SourceItem::Docstring(doc.value));
                }
            }
        }
        self.lower(body, function, None)?;
        if class && !self.deferred_annotations.is_empty() {
            let entries = std::mem::take(&mut self.deferred_annotations);
            self.items.push(SourceItem::Annotations(entries));
        }
        Ok(())
    }

    fn lower(
        &mut self,
        mut body: ast::Suite,
        function: bool,
        active: Option<&mut usize>,
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
            self.lower_suite(body, function, active)?;
            named.declare(Some(self))
        })
    }

    fn lower_suite(
        &mut self,
        body: ast::Suite,
        function: bool,
        mut active: Option<&mut usize>,
    ) -> Result<()> {
        use ast::Stmt;
        for mut stmt in body {
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
                Stmt::Expr(s) => {
                    self.record_expression(*s.value, function)?;
                    continue;
                }
                Stmt::Assign(s) => {
                    for target in &s.targets {
                        self.declare_target(target)?;
                    }
                    self.assignment_operands(s.targets, *s.value, function)?;
                    continue;
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
                            && (function || self.class_scope || *kind != ScopeDirective::Global)
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
                        self.assignment_operands([*s.target], *value, function)?;
                    } else {
                        match *s.target {
                            ast::Expr::Attribute(attr) => {
                                self.record_expression(*attr.value, function)?;
                            }
                            ast::Expr::Subscript(item) => {
                                self.record_expression(*item.value, function)?;
                                self.discard_annotation_slice(*item.slice, function)?;
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
                        self.deferred_annotations.push((name, *s.annotation));
                        continue;
                    }
                }
                Stmt::Delete(s) => {
                    for target in s.targets {
                        match target {
                            ast::Expr::Name(name) => {
                                self.locals.insert(name.id.to_string());
                                self.items.push(SourceItem::Name(ast::Identifier::new(
                                    name.id, name.range,
                                )));
                            }
                            ast::Expr::Subscript(item) => {
                                self.items.push(SourceItem::Discard(*item.value));
                                self.items.push(SourceItem::Discard(*item.slice));
                            }
                            ast::Expr::Attribute(attr) => {
                                self.items.push(SourceItem::Discard(*attr.value));
                                self.items.push(SourceItem::Name(attr.attr));
                            }
                            _ => return Err(unsupported("deletion target")),
                        }
                    }
                    continue;
                }
                Stmt::AugAssign(s) => {
                    self.declare_target(&s.target)?;
                    self.items.push(SourceItem::Discard(*s.target));
                    SourceItem::Discard(*s.value)
                }
                Stmt::Return(s) => {
                    if !function {
                        self.invalid_transfer(s.range(), "'return' outside function")?;
                    }
                    if let Some(value) = s.value {
                        self.record_expression(*value, function)?;
                    }
                    continue;
                }
                Stmt::Raise(s) => {
                    for expr in s.exc.into_iter().chain(s.cause) {
                        self.items.push(SourceItem::Discard(*expr));
                    }
                    continue;
                }
                Stmt::Import(s) if !s.is_lazy => {
                    let mut names = Vec::with_capacity(s.names.len());
                    for alias in s.names {
                        let mut parts = alias.name.as_str().split('.');
                        let root = parts.next().unwrap();
                        let local = alias.asname.as_ref().map_or(root, |name| name.as_str());
                        self.locals.insert(local.to_owned());
                        names.push(ast::name::Name::new(root));
                        names.extend(parts.map(ast::name::Name::new));
                        names.extend(alias.asname.map(|name| name.id));
                    }
                    SourceItem::Names(names)
                }
                Stmt::Assert(s) => {
                    self.items.push(SourceItem::Discard(*s.test));
                    if let Some(message) = s.msg {
                        self.items.push(SourceItem::Discard(*message));
                    }
                    continue;
                }
                Stmt::ImportFrom(s) if !s.is_lazy => {
                    let mut names = Vec::with_capacity(1 + 2 * s.names.len());
                    names.push(s.module.map_or_else(ast::name::Name::empty, |name| name.id));
                    for alias in s.names {
                        let name = alias.name.id;
                        let asname = alias.asname.map(|alias| alias.id);
                        if name.as_str() == "*" {
                            if function || self.class_scope {
                                return Err(Error::new(
                                    "SyntaxError",
                                    "import * only allowed at module level",
                                ));
                            }
                        } else {
                            self.locals
                                .insert(asname.as_ref().unwrap_or(&name).to_string());
                        }
                        names.push(name);
                        names.extend(asname);
                    }
                    SourceItem::Names(names)
                }
                Stmt::TypeAlias(s)
                    if !self.class_scope && (!function || s.type_params.is_some()) =>
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
                    let mut child = self.definition_body(&name);
                    child.locals.insert(".format".to_owned());
                    child.items.push(SourceItem::Discard(*s.value));
                    self.locals.insert(name.to_string());
                    SourceItem::Define(Definition {
                        binding: name.to_string(),
                        decorators: Vec::new(),
                        body: Box::new(child),
                        kind: DefinitionKind::TypeAlias(s.type_params),
                    })
                }
                Stmt::FunctionDef(mut s) => {
                    let mut child = self.definition_body(&s.name);
                    if let Some(params) = &s.type_params {
                        child.validate_type_params(params, "function")?;
                    }
                    let annotations = self.function_annotations(&mut s)?;
                    child.is_async = s.is_async;
                    child.lower_namespace(s.body, true)?;
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
                    if !annotations.is_empty() {
                        self.items.push(SourceItem::Annotations(annotations));
                    }
                    self.locals.insert(s.name.to_string());
                    SourceItem::Define(Definition {
                        binding: s.name.to_string(),
                        decorators: s.decorator_list.into_iter().collect(),
                        body: Box::new(child),
                        kind: DefinitionKind::Function {
                            parameters: s.parameters,
                            type_params: s.type_params,
                        },
                    })
                }
                Stmt::ClassDef(mut s) => {
                    let mut child = self.definition_body(&s.name);
                    if let Some(names) = &class_names {
                        child.private_names = Some(Rc::clone(names));
                    }
                    child.private_names.get_or_insert_with(|| {
                        Rc::new(imre_ast::lower_in(&mut s.body, &child.name, false))
                    });
                    child.class_scope = true;
                    child.lower_namespace(s.body, false)?;
                    self.locals.insert(s.name.to_string());
                    if let Some(params) = &s.type_params {
                        child.validate_type_params(params, "class")?;
                    }
                    SourceItem::Define(Definition {
                        binding: s.name.to_string(),
                        decorators: s.decorator_list.into_iter().collect(),
                        body: Box::new(child),
                        kind: DefinitionKind::Class {
                            arguments: s.arguments,
                            type_params: s.type_params,
                        },
                    })
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
                            if s.is_async && !self.is_async {
                                return Err(Error::new(
                                    "SyntaxError",
                                    "'async for' outside async function",
                                ));
                            }
                            self.record_expression(*s.iter, function)?;
                            self.items.push(SourceItem::Discard(*s.target));
                            (s.body, s.orelse)
                        }
                        Stmt::While(s) => {
                            self.record_expression(*s.test, function)?;
                            (s.body, s.orelse)
                        }
                        _ => unreachable!(),
                    };
                    self.lower(body, function, Some(&mut 0))?;
                    self.lower(orelse, function, active.as_deref_mut())?;
                    continue;
                }
                Stmt::With(s) => {
                    if s.is_async && !self.is_async {
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
                            self.record_expression(item.context_expr, function)?;
                        } else {
                            self.items.push(SourceItem::Discard(item.context_expr));
                        }
                        if let Some(target) = item.optional_vars {
                            self.items.push(SourceItem::Discard(*target));
                        }
                    }
                    self.lower(s.body, function, active.as_deref_mut())?;
                    continue;
                }
                Stmt::Try(s)
                    if s.is_star
                        && !function
                        && !self.class_scope
                        && !self.string_annotations()
                        && imre_ast::annotations::has_variable_annotation(
                            std::slice::from_ref(&stmt),
                        ) =>
                {
                    return Err(unsupported("deferred module annotations in try suites"));
                }
                Stmt::Try(s) if !s.is_star && !s.handlers.is_empty() && s.finalbody.is_empty() => {
                    self.lower(s.body, function, active.as_deref_mut())?;
                    let before_handlers = self.deferred_annotations.len();
                    for ast::ExceptHandler::ExceptHandler(h) in s.handlers {
                        if let Some(class) = h.type_ {
                            self.items.push(SourceItem::Discard(*class));
                        }
                        if let Some(name) = h.name {
                            self.locals.insert(name.to_string());
                            self.items.push(SourceItem::Name(name));
                        }
                        self.lower(h.body, function, active.as_deref_mut())?;
                    }
                    let before_else = self.deferred_annotations.len();
                    self.lower(s.orelse, function, active.as_deref_mut())?;
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
                    self.lower(protected, function, active.as_deref_mut())?;
                    let exits_after = active.as_deref().copied();
                    let first = self.deferred_annotations.len();
                    self.lower(finalbody, function, active.as_deref_mut())?;
                    let annotation_count = self.deferred_annotations.len() - first;
                    if annotation_count != 0 {
                        // A break or continue crossing this finally has its own inlined
                        // annotation sites at the jump, before the fallthrough copy.
                        if exits_after != exits_before {
                            return Err(unsupported("loop exit across an annotated finally"));
                        }
                        // CPython emits another copy of finally for exception unwinding.
                        // Keep both ranges of deferred annotation sites.
                        self.deferred_annotations.extend_from_within(first..);
                    }
                    continue;
                }
                Stmt::Match(s) => {
                    // Case bodies validate in order while their metadata follows all the headers.
                    let mut headers = std::mem::take(&mut self.items);
                    headers.push(SourceItem::Discard(*s.subject));
                    let count = s.cases.len();
                    for (i, case) in s.cases.into_iter().enumerate() {
                        let captures = self.case_captures(&case, i + 1 < count)?;
                        headers.push(SourceItem::Pattern(
                            case.pattern,
                            captures.into_boxed_slice(),
                        ));
                        if let Some(guard) = case.guard {
                            headers.push(SourceItem::Discard(*guard));
                        }
                        self.lower(case.body, function, active.as_deref_mut())?;
                    }
                    headers.append(&mut self.items);
                    self.items = headers;
                    continue;
                }
                Stmt::If(s) => {
                    self.record_expression(*s.test, function)?;
                    self.lower(s.body, function, active.as_deref_mut())?;
                    for clause in s.elif_else_clauses {
                        if let Some(test) = clause.test {
                            self.record_expression(test, function)?;
                        }
                        self.lower(clause.body, function, active.as_deref_mut())?;
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
            self.items.push(op);
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
        imre_ast::executable::source_case_bindings(case, has_next, self.private_names.as_deref())
            .map_err(
                |(range, message)| match message.render(crate::integer_text::check_decimal) {
                    Ok(reason) => Error::source_syntax(reason.into_owned(), range),
                    Err(error) => error.into(),
                },
            )
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

impl SyntaxSpan {
    pub(super) fn scope_error(error: imre_ast::async_generators::Error) -> (Self, String) {
        let span = if error.location_in_args {
            SyntaxSpan::InArgs(error.range)
        } else {
            SyntaxSpan::InFields(error.range)
        };
        (span, error.message)
    }
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

    pub(super) fn retain_syntax_fields(
        &mut self,
        error: &mut Error,
        fields: [Id; 9],
    ) -> Result<()> {
        if error.trace.is_none() {
            let trace = allocator_api2::boxed::Box::try_new(super::value::Trace::default())
                .map_err(|_| Error::new("MemoryError", ""))?;
            error.trace = Some(trace);
        }
        error.tracing().source_syntax = Some(
            self.heap
                .try_alloc_slot(Value::SyntaxMembers(fields), false)?,
        );
        Ok(())
    }

    pub(super) fn syntax_members(&self, error: &Error) -> Option<&[Id; 9]> {
        let members = error.trace.as_ref()?.source_syntax?;
        let Value::SyntaxMembers(fields) = self.heap.get(members) else {
            unreachable!("syntax exception member storage")
        };
        Some(fields)
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

    /// Validate annotation text without resolving its names or compiling its body.
    pub(super) fn check_annotation(&mut self, spelling: Id, check_scope: bool) -> Result<()> {
        use imre_ast::forward_refs::{self, ParseError};
        let source = normalized_source(self.str_value(spelling)?);
        let parsed = match forward_refs::parse(&source) {
            Ok(parsed) => parsed,
            Err(error) => {
                let (range, detail, prefix_filename) = match error {
                    ParseError::Token(message) => return Err(Error::new("SyntaxError", message)),
                    ParseError::Grammar(error) => (error.location, format!(": {error}"), true),
                    ParseError::Syntax(range, message) => (range, message.to_owned(), false),
                };
                let lines = line_starts(&source)?;
                let (position, text) = parser_location(&source, &lines, range, false);
                let filename = self.text("<string>");
                return Err(self.parser_syntax_error(
                    filename,
                    "<string>",
                    &detail,
                    prefix_filename,
                    position,
                    text,
                )?);
            }
        };
        if check_scope && let Err(error) = forward_refs::validate(&parsed) {
            return Err(match error {
                forward_refs::Error::Syntax(message) => Error::new("SyntaxError", message),
                forward_refs::Error::InvalidScope(error) => {
                    let (span, message) = SyntaxSpan::scope_error(error);
                    let lines = line_starts(&source)?;
                    let filename = self.text("<string>");
                    self.located_syntax_error(&message, span, filename, &lines)?
                }
                forward_refs::Error::Scope => Error::new(
                    "NotImplementedError",
                    "forward reference requires scope validation",
                ),
            });
        }
        Ok(())
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

    pub(super) fn update_syntax_member(&mut self, object: Id, name: &str, value: Id) -> bool {
        let Value::Raised(error) = self.heap.get(object) else {
            return false;
        };
        let Some(members) = error.trace.as_ref().and_then(|trace| trace.source_syntax) else {
            return false;
        };
        let Some(slot) = syntax_member(name) else {
            return false;
        };
        let Value::SyntaxMembers(fields) = self.heap.get_mut(members) else {
            unreachable!("syntax exception member storage")
        };
        let previous = std::mem::replace(&mut fields[slot], value);
        if previous != value {
            self.heap.weak_edge_removed(previous);
        }
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

    pub(super) fn inherited_compile_flags(&self) -> u32 {
        match self.heap.get(self.heap.current_code()) {
            Value::Code(caller) => {
                caller.definition.flags
                    & (code_flags::FUTURE_ANNOTATIONS | code_flags::FUTURE_BARRY_AS_BDFL)
            }
            _ => 0,
        }
    }

    pub(super) fn compile_source_with_flags(
        &mut self,
        source: Id,
        filename: Id,
        mode: Id,
        inherited: u32,
    ) -> Result<Id> {
        let program = self.prepare_source_program(source, filename, mode, false, inherited)?;
        Ok(self.box_source_code(program, filename))
    }

    fn prepare_source_program(
        &mut self,
        source: Id,
        filename: Id,
        mode: Id,
        strip_indent: bool,
        inherited: u32,
    ) -> Result<ParsedModule> {
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
        let display = self.filename_display(filename_id)?;
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
        let normalized = normalized_source(source);
        let source = normalized.as_ref();
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
                imre_ast::check_single_input(&module.body, parsed.tokens())
                    .map_err(|message| Error::new("SyntaxError", message))?;
            }
            return IndentationDiagnostic::from_issue(source, issue, mode == "exec")
                .raise(self, filename_id);
        }
        let lines = line_starts(source)?;
        let parse_error = parsed
            .errors()
            .first()
            .map(|error| (error.location, format!(": {error}"), true))
            .or_else(|| {
                imre_ast::comprehension::dict_unpacking_error(&parsed)
                    .map(|(range, reason)| (range, reason.to_owned(), false))
            });
        if let Some((range, detail, prefix_filename)) = parse_error {
            let (position, text) = parser_location(source, &lines, range, mode == "exec");
            let display = display.into_owned();
            return Err(self.parser_syntax_error(
                filename_id,
                &display,
                &detail,
                prefix_filename,
                position,
                text,
            )?);
        }
        if interactive && let ast::Mod::Module(module) = parsed.syntax() {
            imre_ast::check_single_input(&module.body, parsed.tokens())
                .map_err(|message| Error::new("SyntaxError", message))?;
        }
        let future_error = match parsed.syntax() {
            ast::Mod::Module(module) => imre_ast::future_import_error(&module.body),
            _ => None,
        };
        let spellings = string_replacements(source, parsed.tokens());
        let has_named_bindings = source.contains(":=");
        let scope_error =
            imre_ast::async_generators::check(parsed.syntax().into(), has_named_bindings);
        if scope_error.is_some()
            && let ast::Mod::Module(module) = parsed.syntax()
            && imre_ast::misplaced_star_import(&module.body).is_some()
        {
            return Err(Error::new(
                "SyntaxError",
                "import * only allowed at module level",
            ));
        }
        let invalid = scope_error.map(SyntaxSpan::scope_error).or_else(|| {
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
        let mut code = Program::new("<module>", has_named_bindings);
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
        let mut flags = inherited;
        code.strip_docstrings = self.config["optimize"].as_i64().unwrap_or(0) >= 2;
        code.interactive = interactive;
        let expression_body = match parsed.into_syntax() {
            ast::Mod::Expression(expression) => {
                if code.has_named_bindings {
                    use ast::visitor::Visitor;
                    let mut named = NamedBindings::default();
                    named.visit_expr(&expression.body);
                    named.declare(Some(&mut code))?;
                }
                Some(expression.body)
            }
            ast::Mod::Module(module) => {
                let mut body = module.body;
                if !code.strip_docstrings && !spellings.is_empty() {
                    self.check_source_docstrings(&body, !interactive, &spellings)?;
                }
                flags |= imre_ast::future_flags(&body);
                if flags & code_flags::FUTURE_ANNOTATIONS != 0 {
                    code.private_names = Some(Rc::new(imre_ast::lower_in(&mut body, "", true)));
                }
                code.syntax_limit = symbol_error.map(|(range, _)| range);
                let lowering = code.lower_namespace(body, false).and_then(|()| {
                    // Closure binding resolution follows symbol collection and future placement.
                    if symbol_error.is_none() {
                        code.resolve_directives(&|_| None)?;
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
                None
            }
        };
        let constants = code.prepare_scope_strings(
            &mut self.heap,
            &mut SharedConstants::default(),
            &spellings,
            expression_body.as_deref(),
        )?;
        if let Some(issue) = pending_syntax(self, &code)? {
            return Err(issue);
        }
        Ok(ParsedModule { constants, flags })
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

    fn box_source_code(&mut self, module: ParsedModule, filename: Id) -> Id {
        let name = self.heap.code_name("<module>");
        self.realize_code(
            Body::Parsed {
                constants: module.constants,
            },
            value::CodeDefinition {
                module_definition: true,
                // The supported module compile modes admit only future flags.
                flags: module.flags,
                flags_complete: true,
                ..Default::default()
            },
            name,
            NONE,
            (name, NONE),
            (filename, 1),
        )
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
                if let Some(value) = self.lookup_dict_name(mapping, name)? {
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
        if !data.definition.module_definition && !data.definition.closure_supported {
            return Err(unsupported(
                "function construction without closure metadata",
            ));
        }
        let required = data.definition.freevars().len();
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
        let (module, builtins) =
            self.capture_function_namespace(|ctx, name| ctx.lookup_dict_name(globals, name), true)?;
        let builtins = match builtins {
            Some(builtins) => builtins,
            None => self.caller_builtins()?,
        };
        let scope = self.function_environment("", globals, builtins);
        Ok(self.function_with_closure(
            code,
            (scope, module),
            FunctionClosure::Tuple(closure),
            (name, qualname),
            defaults,
            kwdefaults,
        ))
    }

    /// Bind a validated closure to its code object's free variables in order.
    pub(super) fn function_with_closure(
        &mut self,
        code: Id,
        (globals, module): (Id, Id),
        closure: FunctionClosure<'_>,
        (name, qualname): (Id, Id),
        defaults: Id,
        kwdefaults: Id,
    ) -> Id {
        let data = self.code_object(code);
        let freevars = data.definition.freevars().to_vec();
        let code_doc = data.doc;
        let (closure, cells) = match closure {
            FunctionClosure::Cells(cells) => (NONE, Cow::Borrowed(cells)),
            FunctionClosure::Tuple(NONE) => (NONE, Cow::Borrowed(&[][..])),
            FunctionClosure::Tuple(tuple) => {
                let Value::Tuple(cells) = self.heap.get(tuple) else {
                    unreachable!("validated function closure")
                };
                (tuple, Cow::Owned(cells.to_vec()))
            }
        };
        let class_cell = freevars
            .iter()
            .position(|name| name == "__class__")
            .map(|index| cells[index]);
        let scope = self.closure_environment(globals, freevars, &cells);
        let fields = super::function_fields::FunctionFields::new(
            self, scope, module, name, qualname, code_doc, NONE,
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
            (
                "__debug__",
                builtins::boolean(self.config["optimize"].as_i64().unwrap_or(0) == 0),
            ),
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

    fn linked_expression(&self, text: Id) -> Option<Code> {
        match self.heap.linked_sources.get(&text) {
            Some(&LinkedSource::Expression(entry)) => Some(entry),
            _ => None,
        }
    }

    /// Evaluate one forward-reference expression in its defining Python namespaces.
    pub(super) fn eval_type_expression(&mut self, text: Id, globals: Id, locals: Id) -> Result<Id> {
        if let Some(entry) = self.linked_expression(text) {
            let filename = self.text("<string>");
            let code = self.native_module_code(entry, filename, None);
            return self.eval_namespace_code(code, globals, locals);
        }
        let source = self.str_value(text)?;
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
        let code =
            self.compile_source_with_flags(text, filename, mode, self.inherited_compile_flags())?;
        self.eval_namespace_code(code, globals, locals)
    }

    /// Prepare eval or exec with explicit namespaces or the active Python frame.
    pub(super) fn prepare_eval_exec(
        &mut self,
        scope: Id,
        name: &str,
        args: &[Id],
        kwargs: &[(String, Id)],
    ) -> Result<calls::RegisteredCall> {
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
        self.prepare_namespace_code(source, globals, locals, None, name, closure)
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
        if frame.in_class_body {
            return Ok(self.active_class_namespace(scope));
        }
        let mut bindings = Vec::new();
        for name in self.frame_binding_names(scope, code, false)? {
            match self.lookup(scope, &name) {
                Ok(value) => {
                    let key = self.heap.code_name(super::code_locals::visible_slot_name(&name));
                    bindings.push((key, value));
                }
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
        include_unbound: bool,
    ) -> Result<indexmap::IndexSet<String>> {
        let mut names = indexmap::IndexSet::new();
        let frame = self.scope_data(scope);
        if frame.comprehension.is_none()
            && let Value::Code(layout) = self.heap.get(code)
            && matches!(self.heap.get(layout.parameters), Value::Str(_))
        {
            let parameters = self.parsed_parameters(code)?;
            // Argument slots come first, including arguments held in closure cells.
            for name in parameters.argument_slots() {
                if frame.names.contains_key(name) || frame.closure.cells.contains_key(name) {
                    names.insert(name.to_owned());
                }
            }
            // Slot order comes from the code layout even when a branch skipped the first store.
            for name in parameters.locals.iter() {
                if frame.names.contains_key(name.as_str()) {
                    names.insert(name.clone());
                }
            }
        }
        let mut local = scope;
        loop {
            let frame = self.scope_data(local);
            names.extend(frame.names.keys().map(|name| name.as_str().to_owned()));
            let mut cells: Vec<_> = frame.closure.cells.keys().cloned().collect();
            // A class proxy's closure environment already has its owned cells before its free cells.
            if code != NONE || !frame.closure.environment {
                cells.sort_unstable();
            }
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
            names.extend(code.definition.freevars().iter().cloned());
            // Some compiled functions retain their lexical environment because their entire
            // closure cannot yet be cloned. Their statically resolved reads are still locals.
            let frame = self.scope_data(local);
            if !code.definition.closure_supported
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
        if include_unbound {
            names.extend(frame.closure.declared.iter().cloned());
        }
        // The implicit argument wins when a free variable has the same public spelling.
        if names.contains(imre_ast::annotations::FORMAT_PARAMETER) {
            names.shift_remove("format");
        }
        Ok(names)
    }

    fn eval_namespace_code(&mut self, code: Id, globals: Id, locals: Id) -> Result<Id> {
        let call = self.prepare_namespace_code(code, globals, Some(locals), None, "eval", None)?;
        // SAFETY: this synchronous caller owns the context that prepared the code.
        unsafe { call.run(self) }
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

    pub(super) fn prepare_namespace_code(
        &mut self,
        source: Id,
        globals: Id,
        locals: Option<Id>,
        function: Option<Id>,
        mode: &str,
        closure: Option<Id>,
    ) -> Result<calls::RegisteredCall> {
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
            Value::Code(code) if mode == "eval" && !code.definition.freevars().is_empty() => {
                return Err(Error::new(
                    "TypeError",
                    "code object passed to eval() may not contain free variables",
                ));
            }
            Value::Code(code) if !code.definition.freevars().is_empty() => {
                let required = code.definition.freevars().len();
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
            Value::Code(code) if matches!(code.body, Body::Parsed { .. }) => {
                return self.missing_compiled_body(source);
            }
            Value::Code(_) => source,
            _ if closure.is_some() => {
                return Err(Error::new(
                    "TypeError",
                    "closure can only be used when source is a code object",
                ));
            }
            Value::Str(_) | Value::Bytes(_) | Value::ByteArray(_) => {
                let inherited = self.inherited_compile_flags();
                let entry = if mode == "eval" {
                    self.linked_expression(source)
                } else {
                    None
                };
                // Annotation bodies are prepared without inherited flags. The raw text of a
                // starred annotation owns the native entry for its expanded expression.
                let exact = entry.is_some()
                    && inherited == 0
                    && !imre_ast::forward_refs::is_starred(self.str_value(source)?);
                let filename = self.text("<string>");
                if !exact {
                    let mode_id = self.text(mode);
                    self.prepare_source_program(
                        source,
                        filename,
                        mode_id,
                        mode == "eval",
                        inherited,
                    )?;
                }
                let entry =
                    entry.ok_or_else(|| Self::missing_body_error("<module>", "<string>", 1))?;
                self.native_module_code(entry, filename, exact.then_some(inherited))
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
            let function = if code.definition.closure_supported || closure.is_some() {
                let cells = closure.unwrap_or(NONE);
                let name = code.name;
                self.function_with_closure(
                    code_id,
                    (scope, self.root_module_name(scope)),
                    FunctionClosure::Tuple(cells),
                    (name, name),
                    NONE,
                    NONE,
                )
            } else {
                if !code.definition.module_definition {
                    return Err(unsupported(
                        "exec of a prelinked function with an enclosing scope",
                    ));
                }
                let defaults = self.dict(Vec::new());
                self.source_function(code_id, scope, defaults)?
            };
            let result = self.call(function, Vec::new(), Vec::new())?;
            Ok(calls::RegisteredCall::Returned(if mode == "exec" {
                NONE
            } else {
                result
            }))
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
                || FrameOwner::Module(self.root_module_name(scope)),
                FrameOwner::Function,
            );
            self.prepare_execution(code_id, module, owner, mode == "exec")
        }
    }

    pub(super) fn missing_compiled_body<T>(&self, code: Id) -> Result<T> {
        let info = self.code_object(code);
        Err(Self::missing_body_error(
            self.str_value(info.qualname)?,
            &self.filename_display(info.filename)?,
            info.first_line,
        ))
    }

    fn missing_body_error(qualname: &str, filename: &str, first_line: u32) -> Error {
        Error::new(
            "NotImplementedError",
            format!("no compiled Python body for {qualname} ({filename}:{first_line})"),
        )
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
            self.bind_local(scope, name, value);
            return Ok(replaced);
        };
        let key = self.heap.code_name(name);
        Ok(self.store_binding_item(target, key, value)? || replaced)
    }

    pub(super) fn load_exec_name(&mut self, locals: Id, name: &str) -> Result<Id> {
        let key = self.text(name);
        self.get_item(locals, key)
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
