//! Convert the Rust parser's tree into mutable Python AST nodes.
use std::collections::{BTreeMap, HashMap};

use ruff_python_ast as ast;
use ruff_text_size::{Ranged, TextRange, TextSize};

use super::{Context, Error, Id, NONE, Result, Value, builtins, parsed_instance};
use crate::dynamic::complex::Complex;
use crate::dynamic::value::{ELLIPSIS, FALSE, TRUE};

struct Parser<'a> {
    ctx: &'a mut Context,
    source: &'a str,
    literal: bool,
    lines: Vec<usize>,
    definition_starts: Vec<TextSize>,
    template_expressions: HashMap<TextSize, String>,
    replacements: BTreeMap<TextSize, Vec<u32>>,
    load: Id,
    store: Id,
}

enum Trailer {
    Unary(ast::UnaryOp),
    Binary(ast::Operator, Box<ast::Expr>),
    Attribute(ast::Identifier, ast::ExprContext),
    Subscript(Box<ast::Expr>, ast::ExprContext),
    Call(ast::Arguments, Vec<Id>),
}

/// Adjacent constant pieces share one AST node spanning the first through the last piece.
#[derive(Default)]
struct JoinedParts {
    values: Vec<Id>,
    text: Vec<u32>,
    literal: Option<(TextRange, Id)>,
}

impl JoinedParts {
    fn include_literal(&mut self, range: TextRange, kind: Id) {
        let (span, _) = self.literal.get_or_insert((range, kind));
        *span = span.cover(range);
    }

    /// CPython's set_ftstring_expr preserves the source spelling, removing comment text
    /// but retaining its newline. The location continues to span the original source.
    fn debug_literal(&mut self, text: &str, start: TextSize) {
        self.include_literal(TextRange::at(start, TextSize::of(text)), NONE);
        let starts_comment = |ch, quote: &mut Option<char>| {
            match (*quote, ch) {
                (None, '#') => return true,
                (None, '\'' | '"') => *quote = Some(ch),
                (Some(q), _) if ch == q => *quote = None,
                _ => {}
            }
            false
        };
        let mut quote = None;
        let mut chars = text.chars();
        while let Some(ch) = chars.next() {
            if ch == '\\' {
                chars.next();
            } else if starts_comment(ch, &mut quote) {
                // The pinned lexer skips escapes while detecting a comment, then
                // copies the spelling with a fresh quote state and no escape step.
                quote = None;
                let mut in_comment = false;
                for ch in text.chars() {
                    if ch == '\n' {
                        in_comment = false;
                    } else if !in_comment {
                        in_comment = starts_comment(ch, &mut quote);
                    }
                    if !in_comment {
                        self.text.push(u32::from(ch));
                    }
                }
                return;
            }
        }
        self.text.extend(text.chars().map(u32::from));
    }

    fn flush(&mut self, parser: &mut Parser<'_>) {
        if let Some((range, kind)) = self.literal.take()
            && !self.text.is_empty()
        {
            let value = parser.ctx.text_codepoints(std::mem::take(&mut self.text));
            self.values
                .push(parser.node("Constant", &[value, kind], range));
        }
    }
}

impl Parser<'_> {
    fn location(&self, range: TextRange) -> [i64; 4] {
        let locate = |offset| {
            let line = self.lines.partition_point(|n| *n <= offset);
            (line as i64, (offset - self.lines[line - 1]) as i64)
        };
        let (line, col) = locate(range.start().to_usize());
        let (end_line, end_col) = locate(range.end().to_usize());
        [line, col, end_line, end_col]
    }

    fn node(&mut self, kind: &str, fields: &[Id], range: TextRange) -> Id {
        let position = self.location(range);
        parsed_instance(self.ctx, kind, fields, Some(position))
    }

    fn list(&mut self, values: Vec<Id>) -> Id {
        self.ctx.alloc(Value::List(values))
    }

    fn suite(&mut self, body: ast::Suite) -> Result<Id> {
        stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || self.suite_inner(body))
    }

    fn suite_inner(&mut self, body: ast::Suite) -> Result<Id> {
        let values = body
            .into_iter()
            .map(|s| self.statement(s))
            .collect::<Result<_>>()?;
        Ok(self.list(values))
    }

    fn exprs(&mut self, exprs: impl IntoIterator<Item = ast::Expr>) -> Result<Id> {
        let values = exprs
            .into_iter()
            .map(|e| self.expr(e))
            .collect::<Result<_>>()?;
        Ok(self.list(values))
    }

    fn comprehension_element(&mut self, element: ast::Expr) -> Result<Id> {
        // Ruff also recognizes iterable unpacking in comprehensions, added in Python 3.15.
        if element.is_starred_expr() {
            return Err(Error::new(
                "SyntaxError",
                "iterable unpacking cannot be used in comprehension",
            ));
        }
        self.expr(element)
    }

    fn comprehensions(
        &mut self,
        clauses: impl IntoIterator<Item = ast::Comprehension>,
    ) -> Result<Id> {
        let mut nodes = Vec::new();
        for clause in clauses {
            let target = self.expr(clause.target)?;
            let iter = self.expr(clause.iter)?;
            let filters = self.exprs(clause.ifs)?;
            let is_async = self.ctx.int(i64::from(clause.is_async));
            nodes.push(parsed_instance(
                self.ctx,
                "comprehension",
                &[target, iter, filters, is_async],
                None,
            ));
        }
        Ok(self.list(nodes))
    }

    fn optional(&mut self, expr: Option<Box<ast::Expr>>) -> Result<Id> {
        expr.map(|e| self.expr(*e))
            .transpose()
            .map(|e| e.unwrap_or(NONE))
    }

    fn aliases(&mut self, aliases: &[ast::Alias]) -> Id {
        let nodes = aliases
            .iter()
            .map(|alias| {
                let name = self.ctx.text(alias.name.as_str());
                let asname = alias
                    .asname
                    .as_ref()
                    .map_or(NONE, |n| self.ctx.text(n.as_str()));
                self.node("alias", &[name, asname], alias.range())
            })
            .collect();
        self.list(nodes)
    }

    fn arg(&mut self, parameter: &mut ast::Parameter) -> Result<Id> {
        let name = self.ctx.text(parameter.name.as_str());
        let annotation = self.optional(parameter.annotation.take())?;
        Ok(self.node("arg", &[name, annotation, NONE], parameter.range()))
    }

    fn parameters(&mut self, p: &mut ast::Parameters) -> Result<Id> {
        let mut args = |xs: &mut [ast::ParameterWithDefault]| -> Result<_> {
            let vals = xs
                .iter_mut()
                .map(|x| self.arg(&mut x.parameter))
                .collect::<Result<_>>()?;
            Ok(self.list(vals))
        };
        let positional_only = args(&mut p.posonlyargs)?;
        let positional = args(&mut p.args)?;
        let keyword_only = args(&mut p.kwonlyargs)?;
        let vararg = p
            .vararg
            .as_mut()
            .map(|a| self.arg(a))
            .transpose()?
            .unwrap_or(NONE);
        let kwarg = p
            .kwarg
            .as_mut()
            .map(|a| self.arg(a))
            .transpose()?
            .unwrap_or(NONE);
        let kd = p
            .kwonlyargs
            .iter_mut()
            .map(|a| self.optional(a.default.take()))
            .collect::<Result<_>>()?;
        let keyword_defaults = self.list(kd);
        let defaults = p
            .posonlyargs
            .iter_mut()
            .chain(&mut p.args)
            .filter_map(|a| a.default.take())
            .map(|e| self.expr(*e))
            .collect::<Result<_>>()?;
        let defaults = self.list(defaults);
        Ok(parsed_instance(
            self.ctx,
            "arguments",
            &[
                positional_only,
                positional,
                vararg,
                keyword_only,
                keyword_defaults,
                kwarg,
                defaults,
            ],
            None,
        ))
    }

    fn keyword(&mut self, keyword: ast::Keyword) -> Result<Id> {
        let range = keyword.range();
        let name = keyword.arg.map_or(NONE, |n| self.ctx.text(n.as_str()));
        let value = self.expr(keyword.value)?;
        Ok(self.node("keyword", &[name, value], range))
    }

    fn handler(&mut self, handler: ast::ExceptHandler) -> Result<Id> {
        let ast::ExceptHandler::ExceptHandler(handler) = handler;
        let range = handler.range();
        let exception = self.optional(handler.type_)?;
        let name = handler.name.map_or(NONE, |n| self.ctx.text(n.as_str()));
        let body = self.suite(handler.body)?;
        Ok(self.node("ExceptHandler", &[exception, name, body], range))
    }

    fn with_item(&mut self, item: ast::WithItem) -> Result<Id> {
        let context = self.expr(item.context_expr)?;
        let target = self.optional(item.optional_vars)?;
        Ok(parsed_instance(
            self.ctx,
            "withitem",
            &[context, target],
            None,
        ))
    }

    /// Convert a case clause, which has no independent source position in Python's AST.
    fn match_case(&mut self, case: ast::MatchCase) -> Result<Id> {
        let pattern = self.pattern(case.pattern)?;
        let guard = self.optional(case.guard)?;
        let body = self.suite(case.body)?;
        Ok(parsed_instance(
            self.ctx,
            "match_case",
            &[pattern, guard, body],
            None,
        ))
    }

    fn patterns(&mut self, patterns: impl IntoIterator<Item = ast::Pattern>) -> Result<Id> {
        let elements = patterns
            .into_iter()
            .map(|p| self.pattern(p))
            .collect::<Result<_>>()?;
        Ok(self.list(elements))
    }

    /// Preserve the shape and byte positions of nested structural patterns.
    fn pattern(&mut self, pattern: ast::Pattern) -> Result<Id> {
        stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || self.pattern_inner(pattern))
    }

    fn pattern_inner(&mut self, pattern: ast::Pattern) -> Result<Id> {
        use ast::Pattern;
        let range = pattern.range();
        let (kind, fields) = match pattern {
            Pattern::MatchValue(p) => ("MatchValue", vec![self.expr(*p.value)?]),
            Pattern::MatchSingleton(p) => {
                let value = match p.value {
                    ast::Singleton::None => NONE,
                    ast::Singleton::True => TRUE,
                    ast::Singleton::False => FALSE,
                };
                ("MatchSingleton", vec![value])
            }
            Pattern::MatchSequence(p) => ("MatchSequence", vec![self.patterns(p.patterns)?]),
            Pattern::MatchMapping(p) => (
                "MatchMapping",
                vec![
                    self.exprs(p.keys)?,
                    self.patterns(p.patterns)?,
                    p.rest.map_or(NONE, |name| self.ctx.text(name.as_str())),
                ],
            ),
            Pattern::MatchClass(p) => {
                let cls = self.expr(*p.cls)?;
                let positional = self.patterns(p.arguments.patterns)?;
                let mut attrs = Vec::with_capacity(p.arguments.keywords.len());
                let mut patterns = Vec::with_capacity(p.arguments.keywords.len());
                for kw in p.arguments.keywords {
                    attrs.push(self.ctx.text(kw.attr.as_str()));
                    patterns.push(self.pattern(kw.pattern)?);
                }
                (
                    "MatchClass",
                    vec![cls, positional, self.list(attrs), self.list(patterns)],
                )
            }
            Pattern::MatchStar(p) => (
                "MatchStar",
                vec![p.name.map_or(NONE, |name| self.ctx.text(name.as_str()))],
            ),
            Pattern::MatchAs(p) => {
                let inner = p.pattern.map_or(Ok(NONE), |p| self.pattern(*p))?;
                let name = p.name.map_or(NONE, |name| self.ctx.text(name.as_str()));
                ("MatchAs", vec![inner, name])
            }
            Pattern::MatchOr(p) => ("MatchOr", vec![self.patterns(p.patterns)?]),
        };
        Ok(self.node(kind, &fields, range))
    }

    /// Convert the identifier list carried by a global or nonlocal declaration.
    fn identifiers(&mut self, names: Vec<ast::Identifier>) -> Id {
        let names = names
            .into_iter()
            .map(|name| self.ctx.text(name.as_str()))
            .collect();
        self.list(names)
    }

    /// Preserve the bounds, defaults, and source spans of a declaration's type parameters.
    fn type_parameters(&mut self, params: Option<Box<ast::TypeParams>>) -> Result<Id> {
        let Some(params) = params else {
            return Ok(self.list(Vec::new()));
        };
        let mut nodes = Vec::with_capacity(params.len());
        for param in params.type_params {
            let range = param.range();
            let name = self.ctx.text(param.name().as_str());
            let (kind, fields) = match param {
                ast::TypeParam::TypeVar(v) => (
                    "TypeVar",
                    vec![name, self.optional(v.bound)?, self.optional(v.default)?],
                ),
                ast::TypeParam::TypeVarTuple(v) => {
                    ("TypeVarTuple", vec![name, self.optional(v.default)?])
                }
                ast::TypeParam::ParamSpec(v) => {
                    ("ParamSpec", vec![name, self.optional(v.default)?])
                }
            };
            nodes.push(self.node(kind, &fields, range));
        }
        Ok(self.list(nodes))
    }

    fn statement(&mut self, s: ast::Stmt) -> Result<Id> {
        use ast::Stmt;
        let range = self.statement_range(&s);
        let (kind, fields) = match s {
            Stmt::Expr(v) => ("Expr", vec![self.expr(*v.value)?]),
            Stmt::Pass(_) => ("Pass", vec![]),
            Stmt::Import(v) if !v.is_lazy => ("Import", vec![self.aliases(&v.names)]),
            // Other absolute future imports can require feature validation or change the grammar.
            Stmt::ImportFrom(v)
                if !v.is_lazy
                    && (v.level != 0
                        || v.module.as_deref() != Some("__future__")
                        || v.names
                            .iter()
                            .all(|name| name.name.as_str() == "annotations")) =>
            {
                let module = v
                    .module
                    .as_ref()
                    .map_or(NONE, |m| self.ctx.text(m.as_str()));
                let names = self.aliases(&v.names);
                let level = self.ctx.int(i64::from(v.level));
                ("ImportFrom", vec![module, names, level])
            }
            Stmt::AnnAssign(v) => (
                "AnnAssign",
                vec![
                    self.expr(*v.target)?,
                    self.expr(*v.annotation)?,
                    self.optional(v.value)?,
                    self.ctx.int(i64::from(v.simple)),
                ],
            ),
            Stmt::Global(v) => ("Global", vec![self.identifiers(v.names)]),
            Stmt::Nonlocal(v) => ("Nonlocal", vec![self.identifiers(v.names)]),
            Stmt::TypeAlias(v) => (
                "TypeAlias",
                vec![
                    self.expr(*v.name)?,
                    self.type_parameters(v.type_params)?,
                    self.expr(*v.value)?,
                ],
            ),
            Stmt::Assign(v) => (
                "Assign",
                vec![self.exprs(v.targets)?, self.expr(*v.value)?, NONE],
            ),
            Stmt::Return(v) => ("Return", vec![self.optional(v.value)?]),
            Stmt::Assert(v) => ("Assert", vec![self.expr(*v.test)?, self.optional(v.msg)?]),
            Stmt::Raise(v) => (
                "Raise",
                vec![self.optional(v.exc)?, self.optional(v.cause)?],
            ),
            Stmt::Break(_) => ("Break", vec![]),
            Stmt::Continue(_) => ("Continue", vec![]),
            Stmt::Delete(v) => ("Delete", vec![self.exprs(v.targets)?]),
            Stmt::AugAssign(v) => (
                "AugAssign",
                vec![
                    self.expr(*v.target)?,
                    self.ctx.builtins[&format!("ast._{:?}", v.op)],
                    self.expr(*v.value)?,
                ],
            ),
            Stmt::While(v) => (
                "While",
                vec![
                    self.expr(*v.test)?,
                    self.suite(v.body)?,
                    self.suite(v.orelse)?,
                ],
            ),
            Stmt::For(v) => (
                if v.is_async { "AsyncFor" } else { "For" },
                vec![
                    self.expr(*v.target)?,
                    self.expr(*v.iter)?,
                    self.suite(v.body)?,
                    self.suite(v.orelse)?,
                    NONE,
                ],
            ),
            Stmt::With(v) => {
                let items = v
                    .items
                    .into_iter()
                    .map(|i| self.with_item(i))
                    .collect::<Result<_>>()?;
                (
                    if v.is_async { "AsyncWith" } else { "With" },
                    vec![self.list(items), self.suite(v.body)?, NONE],
                )
            }
            Stmt::Match(v) => {
                let subject = self.expr(*v.subject)?;
                let cases = v
                    .cases
                    .into_iter()
                    .map(|case| self.match_case(case))
                    .collect::<Result<_>>()?;
                ("Match", vec![subject, self.list(cases)])
            }
            Stmt::Try(v) => {
                let body = self.suite(v.body)?;
                let handlers = v
                    .handlers
                    .into_iter()
                    .map(|h| self.handler(h))
                    .collect::<Result<_>>()?;
                (
                    if v.is_star { "TryStar" } else { "Try" },
                    vec![
                        body,
                        self.list(handlers),
                        self.suite(v.orelse)?,
                        self.suite(v.finalbody)?,
                    ],
                )
            }
            Stmt::ClassDef(v) => {
                let (bases, keywords) = if let Some(args) = v.arguments {
                    let bases = self.exprs(args.args)?;
                    let keywords = args
                        .keywords
                        .into_iter()
                        .map(|k| self.keyword(k))
                        .collect::<Result<_>>()?;
                    (bases, self.list(keywords))
                } else {
                    (self.list(vec![]), self.list(vec![]))
                };
                (
                    "ClassDef",
                    vec![
                        self.ctx.text(v.name.as_str()),
                        bases,
                        keywords,
                        self.suite(v.body)?,
                        self.exprs(v.decorator_list.into_iter().map(|d| d.expression))?,
                        self.type_parameters(v.type_params)?,
                    ],
                )
            }
            Stmt::If(v) => {
                let mut otherwise = self.list(vec![]);
                for clause in v.elif_else_clauses.into_iter().rev() {
                    let start = clause.start();
                    let body = self.suite(clause.body)?;
                    otherwise = if let Some(test) = clause.test {
                        let test = self.expr(test)?;
                        let nested = self.node(
                            "If",
                            &[test, body, otherwise],
                            TextRange::new(start, range.end()),
                        );
                        self.list(vec![nested])
                    } else {
                        body
                    };
                }
                (
                    "If",
                    vec![self.expr(*v.test)?, self.suite(v.body)?, otherwise],
                )
            }
            Stmt::FunctionDef(mut v) => {
                let decorators = v
                    .decorator_list
                    .into_iter()
                    .map(|d| self.expr(d.expression))
                    .collect::<Result<_>>()?;
                (
                    if v.is_async {
                        "AsyncFunctionDef"
                    } else {
                        "FunctionDef"
                    },
                    vec![
                        self.ctx.text(v.name.as_str()),
                        self.parameters(&mut v.parameters)?,
                        self.suite(v.body)?,
                        self.list(decorators),
                        self.optional(v.returns)?,
                        NONE,
                        self.type_parameters(v.type_params)?,
                    ],
                )
            }
            other => {
                return Err(Error::new(
                    "NotImplementedError",
                    format!(
                        "ast.parse does not support {:?}",
                        ast::AnyNodeRef::from(&other).kind()
                    ),
                ));
            }
        };
        Ok(self.node(kind, &fields, range))
    }

    fn statement_range(&self, statement: &ast::Stmt) -> TextRange {
        let decorators = match statement {
            ast::Stmt::FunctionDef(v) => &v.decorator_list,
            ast::Stmt::ClassDef(v) => &v.decorator_list,
            _ => return statement.range(),
        };
        if let Some(last) = decorators.last() {
            // Ruff includes decorators; CPython starts at the class, def or async keyword.
            let next = self
                .definition_starts
                .partition_point(|start| *start < last.end());
            return TextRange::new(self.definition_starts[next], statement.end());
        }
        statement.range()
    }

    fn context(&self, context: ast::ExprContext) -> Result<Id> {
        match context {
            ast::ExprContext::Load => Ok(self.load),
            ast::ExprContext::Store => Ok(self.store),
            ast::ExprContext::Del => Ok(self.ctx.builtins["ast._Del"]),
            _ => Err(Error::new(
                "NotImplementedError",
                "unsupported AST expression context",
            )),
        }
    }

    fn prepare_strings<'a>(
        &mut self,
        source: &str,
        tokens: &ast::token::Tokens,
        syntax: impl Into<ast::ModRef<'a>>,
    ) -> Result<()> {
        use ast::token::TokenKind::{
            Async, Class, Def, FStringEnd, FStringStart, TStringEnd, TStringStart,
        };

        self.replacements = crate::dynamic::source_code::string_replacements(source, tokens);
        if self.literal {
            return Ok(());
        }
        let mut string_depth = 0;
        let mut has_templates = false;
        for token in tokens.iter() {
            match token.kind() {
                FStringStart | TStringStart => {
                    string_depth += 1;
                    // One of CPython's 150 tokenizer modes is the outer Python source.
                    if string_depth == 150 {
                        return Err(Error::new(
                            "SyntaxError",
                            "too many nested f-strings or t-strings",
                        ));
                    }
                    has_templates |= token.kind() == TStringStart;
                }
                FStringEnd | TStringEnd => string_depth -= 1,
                Async | Class | Def => self.definition_starts.push(token.start()),
                _ => {}
            }
        }
        if has_templates {
            self.template_expressions =
                imre_ast::strings::template_expressions(source, tokens, syntax);
        }
        Ok(())
    }

    /// Ruff decodes both isolated surrogates and U+FFFD to its replacement character.
    fn append_literal_text(
        &self,
        points: &mut Vec<u32>,
        text: &str,
        range: TextRange,
    ) -> Result<()> {
        let mut original = self
            .replacements
            .range(range.start()..range.end())
            .flat_map(|(_, points)| points.iter().copied());
        let mismatch = || Error::new("NotImplementedError", "ast.parse: string escape spelling");
        points
            .try_reserve(text.chars().count())
            .map_err(|_| crate::Error::allocation())?;
        for ch in text.chars() {
            points.push(if ch == char::REPLACEMENT_CHARACTER {
                original.next().ok_or_else(mismatch)?
            } else {
                u32::from(ch)
            });
        }
        if original.next().is_some() {
            return Err(mismatch());
        }
        Ok(())
    }

    fn middle_literal(
        &self,
        parts: &mut JoinedParts,
        lit: &ast::InterpolatedStringLiteralElement,
        raw: bool,
        continued_brace_error: &'static str,
    ) -> Result<()> {
        if !lit.value.is_empty() {
            let source = &self.source[lit.range()];
            // CPython drops an empty middle token after a brace boundary;
            // Ruff can include it in the range of the preceding literal.
            let prefix = source.trim_end_matches("\\\n");
            if !raw && prefix.len() != source.len() && prefix.ends_with(['{', '}']) {
                return Err(Error::new("NotImplementedError", continued_brace_error));
            }
            self.append_literal_text(&mut parts.text, &lit.value, lit.range())?;
            parts.include_literal(lit.range(), NONE);
        }
        Ok(())
    }

    fn template_values(&mut self, template: &mut ast::ExprTString) -> Result<Id> {
        let mut parts = JoinedParts::default();
        for part in template.value.iter_mut() {
            let raw = part.flags.prefix().is_raw();
            for element in &mut part.elements {
                match element {
                    ast::InterpolatedStringElement::Literal(lit) => {
                        self.middle_literal(
                            &mut parts,
                            lit,
                            raw,
                            "ast.parse: continued t-string brace literal",
                        )?;
                    }
                    ast::InterpolatedStringElement::Interpolation(field) => {
                        parts.flush(self);
                        parts.values.push(self.template_field(field, raw)?);
                    }
                }
            }
        }
        parts.flush(self);
        Ok(self.list(parts.values))
    }

    fn template_field(&mut self, field: &mut ast::InterpolatedElement, raw: bool) -> Result<Id> {
        let lit = imre_ast::strings::template_format_spec(field)
            .map_err(|why| Error::new("NotImplementedError", why))?;
        let format_spec = if let Some(spec) = &field.format_spec {
            // CPython can fold multiple middle tokens into a Constant here.
            if !raw && self.source[spec.range()].contains(r"\N{") {
                return Err(Error::new(
                    "NotImplementedError",
                    "ast.parse: named Unicode escape in t-string format spec",
                ));
            }
            let mut parts = Vec::new();
            if let Some(lit) = lit.filter(|lit| !lit.value.is_empty()) {
                let mut points = Vec::new();
                self.append_literal_text(&mut points, &lit.value, lit.range())?;
                let value = self.ctx.text_codepoints(points);
                parts.push(self.node("Constant", &[value, NONE], lit.range()));
            }
            let parts = self.list(parts);
            // CPython includes the colon, but excludes the field's closing brace.
            let range = TextRange::new(
                spec.start() - TextSize::new(1),
                field.end() - TextSize::new(1),
            );
            self.node("JoinedStr", &[parts], range)
        } else {
            NONE
        };
        let spelling = self
            .template_expressions
            .remove(&field.start())
            .ok_or_else(|| {
                Error::new(
                    "NotImplementedError",
                    "ast.parse: commented t-string expression",
                )
            })?;
        let spelling = self.ctx.text(spelling);
        let conversion = self.ctx.int(i64::from(field.conversion as i8));
        let expression = self.field_expression(field)?;
        Ok(self.node(
            "Interpolation",
            &[expression, spelling, conversion, format_spec],
            field.range(),
        ))
    }

    fn field_expression(&mut self, field: &mut ast::InterpolatedElement) -> Result<Id> {
        // Take the expression so long operator chains follow the existing iterative path.
        let placeholder = ast::Expr::NoneLiteral(ast::ExprNoneLiteral {
            range: field.expression.range(),
            node_index: ast::AtomicNodeIndex::NONE,
        });
        self.expr(std::mem::replace(&mut *field.expression, placeholder))
    }

    fn formatted_values(&mut self, string: &mut ast::ExprFString) -> Result<Id> {
        let mut parts = JoinedParts::default();
        for part in string.value.iter_mut() {
            match part {
                ast::FStringPartMut::Literal(lit) => {
                    // Even an empty ordinary string contributes its range and leading 'u' kind.
                    let kind = if self.source[lit.range()].starts_with('u') {
                        self.ctx.text("u")
                    } else {
                        NONE
                    };
                    parts.include_literal(lit.range(), kind);
                    self.append_literal_text(&mut parts.text, &lit.value, lit.range())?;
                }
                ast::FStringPartMut::FString(part) => {
                    self.formatted_elements(
                        &mut parts,
                        &mut part.elements,
                        part.flags.prefix().is_raw(),
                        0,
                    )?;
                }
            }
        }
        parts.flush(self);
        Ok(self.list(parts.values))
    }

    fn has_named_escape(source: &str) -> bool {
        let mut chars = source.chars();
        while let Some(ch) = chars.next() {
            if ch == '\\' && chars.next() == Some('N') && chars.as_str().starts_with('{') {
                return true;
            }
        }
        false
    }

    fn formatted_elements(
        &mut self,
        parts: &mut JoinedParts,
        elements: &mut ast::InterpolatedStringElements,
        raw: bool,
        spec_depth: u8,
    ) -> Result<()> {
        for element in elements {
            match element {
                ast::InterpolatedStringElement::Literal(lit) => {
                    // CPython can fold several cooked format tokens into a plain Constant.
                    if spec_depth != 0 && !raw && Self::has_named_escape(&self.source[lit.range()])
                    {
                        return Err(Error::new(
                            "NotImplementedError",
                            "ast.parse: named Unicode escape in f-string format spec",
                        ));
                    }
                    self.middle_literal(
                        parts,
                        lit,
                        raw,
                        "ast.parse: continued f-string brace literal",
                    )?;
                }
                ast::InterpolatedStringElement::Interpolation(field) => {
                    // A new f-string starts its own replacement-field nesting count.
                    if spec_depth == 3 {
                        return Err(Error::new(
                            "SyntaxError",
                            "f-string: expressions nested too deeply",
                        ));
                    }
                    let mut conversion = field.conversion;
                    if let Some(debug) = &field.debug_text {
                        parts.debug_literal(debug.as_str(), field.start() + TextSize::new(1));
                        if conversion == ast::ConversionFlag::None && field.format_spec.is_none() {
                            conversion = ast::ConversionFlag::Repr;
                        }
                    }
                    parts.flush(self);
                    let expr = self.field_expression(field)?;
                    let end = field.end() - TextSize::new(1);
                    let spec = if let Some(spec) = &mut field.format_spec {
                        let mut nested = JoinedParts::default();
                        self.formatted_elements(
                            &mut nested,
                            &mut spec.elements,
                            raw,
                            spec_depth + 1,
                        )?;
                        nested.flush(self);
                        let values = self.list(nested.values);
                        self.node(
                            "JoinedStr",
                            &[values],
                            TextRange::new(spec.start() - TextSize::new(1), end),
                        )
                    } else {
                        NONE
                    };
                    let conversion = self.ctx.int(i64::from(conversion as i8));
                    parts.values.push(self.node(
                        "FormattedValue",
                        &[expr, conversion, spec],
                        field.range(),
                    ));
                }
            }
        }
        Ok(())
    }

    /// Decode a string with the original code points preserved for surrogate escapes.
    fn string_value(&mut self, v: &ast::ExprStringLiteral) -> Result<Id> {
        let text = v.value.to_str();
        let points = crate::dynamic::source_code::original_points(
            text,
            self.replacements
                .range(v.start()..v.end())
                .flat_map(|(_, points)| points.iter().copied()),
        )?;
        Ok(match points {
            Some(points) => self.ctx.text_codepoints(points),
            None => self.ctx.text(text),
        })
    }

    /// Decode a Ruff literal using the same values for ast.parse and literal_eval.
    fn constant_value(&mut self, value: ast::LiteralExpressionRef<'_>) -> Result<Id> {
        use ast::LiteralExpressionRef as Literal;
        Ok(match value {
            Literal::StringLiteral(v) => self.string_value(v)?,
            Literal::BytesLiteral(v) => self.ctx.alloc(Value::Bytes(v.value.bytes().collect())),
            Literal::NoneLiteral(_) => NONE,
            Literal::BooleanLiteral(v) => {
                if v.value {
                    TRUE
                } else {
                    FALSE
                }
            }
            Literal::EllipsisLiteral(_) => ELLIPSIS,
            Literal::NumberLiteral(v) => {
                let number = match &v.value {
                    ast::Number::Int(n) => {
                        let n = crate::parse_integer_literal(&n.to_string())
                            .ok_or_else(|| Error::new("ValueError", "invalid integer literal"))?;
                        Value::Int(crate::Int::new(n))
                    }
                    ast::Number::Float(n) => Value::Float(*n),
                    ast::Number::Complex { real, imag } => Value::Complex(Complex {
                        real: *real,
                        imag: *imag,
                    }),
                };
                self.ctx.alloc(number)
            }
        })
    }

    /// Elide temporary AST instances for a constant or a flat list or tuple of constants.
    fn direct_literal(&mut self, expr: &ast::Expr) -> Result<Option<Id>> {
        if let Some(value) = expr.as_literal_expr() {
            return if super::literal::plain_fields(self.ctx, None) {
                self.constant_value(value).map(Some)
            } else {
                Ok(None)
            };
        }
        let (class, elts) = match expr {
            ast::Expr::List(v) => ("ast.List", &v.elts),
            ast::Expr::Tuple(v) => ("ast.Tuple", &v.elts),
            _ => return Ok(None),
        };
        if !elts.iter().all(ast::Expr::is_literal_expr)
            || !super::literal::plain_fields(self.ctx, Some(class))
        {
            return Ok(None);
        }
        let values = elts
            .iter()
            .map(|elt| self.constant_value(elt.as_literal_expr().unwrap()))
            .collect::<Result<_>>()?;
        Ok(Some(if matches!(expr, ast::Expr::Tuple(_)) {
            self.ctx.heap.packed_tuple(values, false)
        } else {
            self.list(values)
        }))
    }

    fn expr(&mut self, e: ast::Expr) -> Result<Id> {
        stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || self.expr_inner(e))
    }

    fn expr_inner(&mut self, mut e: ast::Expr) -> Result<Id> {
        use ast::Expr;
        // Postfix expressions can grow without parentheses. Move their operands
        // out of Ruff's tree so conversion and destruction both use bounded stack.
        let mut trailers = Vec::new();
        loop {
            let range = e.range();
            let (next, trailer) = match e {
                Expr::UnaryOp(v) => (v.operand, Trailer::Unary(v.op)),
                Expr::BinOp(v) => (v.left, Trailer::Binary(v.op, v.right)),
                Expr::Attribute(v) => (v.value, Trailer::Attribute(v.attr, v.ctx)),
                Expr::Subscript(v) => (v.value, Trailer::Subscript(v.slice, v.ctx)),
                Expr::Call(mut v) => {
                    // A bare generator argument includes the call parentheses in CPython's range.
                    if let [Expr::Generator(generator)] = v.arguments.args.as_mut_slice()
                        && !generator.parenthesized
                    {
                        generator.range = v.arguments.range;
                    }
                    let keywords = std::mem::take(&mut v.arguments.keywords)
                        .into_iter()
                        .map(|kw| self.keyword(kw))
                        .collect::<Result<_>>()?;
                    (v.func, Trailer::Call(v.arguments, keywords))
                }
                _ => break,
            };
            trailers.push((range, trailer));
            e = *next;
        }
        let range = e.range();
        let (kind, fields) = match e {
            Expr::Named(v) if !self.literal => (
                "NamedExpr",
                vec![self.expr(*v.target)?, self.expr(*v.value)?],
            ),
            Expr::Name(v) => (
                "Name",
                vec![self.ctx.text(v.id.as_str()), self.context(v.ctx)?],
            ),
            Expr::FString(mut v) if !self.literal => {
                ("JoinedStr", vec![self.formatted_values(&mut v)?])
            }
            Expr::TString(mut v) if !self.literal => {
                ("TemplateStr", vec![self.template_values(&mut v)?])
            }
            ref expr if let Some(value) = expr.as_literal_expr() => {
                ("Constant", vec![self.constant_value(value)?, NONE])
            }
            Expr::BoolOp(v) if !self.literal => {
                let op = self.ctx.builtins[match v.op {
                    ast::BoolOp::And => "ast._And",
                    ast::BoolOp::Or => "ast._Or",
                }];
                ("BoolOp", vec![op, self.exprs(v.values)?])
            }
            Expr::Lambda(v) if !self.literal => {
                let mut parameters = v.parameters.unwrap_or_default();
                (
                    "Lambda",
                    vec![self.parameters(&mut parameters)?, self.expr(*v.body)?],
                )
            }
            Expr::ListComp(v) if !self.literal => (
                "ListComp",
                vec![
                    self.comprehension_element(*v.elt)?,
                    self.comprehensions(v.generators)?,
                ],
            ),
            Expr::SetComp(v) if !self.literal => (
                "SetComp",
                vec![
                    self.comprehension_element(*v.elt)?,
                    self.comprehensions(v.generators)?,
                ],
            ),
            Expr::DictComp(v) if !self.literal => {
                let key = v.key.ok_or_else(|| {
                    Error::new(
                        "SyntaxError",
                        "dict unpacking cannot be used in dict comprehension",
                    )
                })?;
                (
                    "DictComp",
                    vec![
                        self.expr(*key)?,
                        self.expr(*v.value)?,
                        self.comprehensions(v.generators)?,
                    ],
                )
            }
            Expr::Generator(v) if !self.literal => (
                "GeneratorExp",
                vec![
                    self.comprehension_element(*v.elt)?,
                    self.comprehensions(v.generators)?,
                ],
            ),
            Expr::Slice(v) if !self.literal => (
                "Slice",
                vec![
                    self.optional(v.lower)?,
                    self.optional(v.upper)?,
                    self.optional(v.step)?,
                ],
            ),
            Expr::Starred(v) if !self.literal => {
                ("Starred", vec![self.expr(*v.value)?, self.context(v.ctx)?])
            }
            Expr::Await(v) if !self.literal => ("Await", vec![self.expr(*v.value)?]),
            Expr::Yield(v) if !self.literal => ("Yield", vec![self.optional(v.value)?]),
            Expr::YieldFrom(v) if !self.literal => ("YieldFrom", vec![self.expr(*v.value)?]),
            Expr::Tuple(v) => ("Tuple", vec![self.exprs(v.elts)?, self.context(v.ctx)?]),
            Expr::List(v) => ("List", vec![self.exprs(v.elts)?, self.context(v.ctx)?]),
            Expr::Set(v) => ("Set", vec![self.exprs(v.elts)?]),
            Expr::Dict(v) => {
                let mut keys = Vec::with_capacity(v.items.len());
                let mut values = Vec::with_capacity(v.items.len());
                for item in v.items {
                    keys.push(
                        item.key
                            .map(|key| self.expr(key))
                            .transpose()?
                            .unwrap_or(NONE),
                    );
                    values.push(self.expr(item.value)?);
                }
                ("Dict", vec![self.list(keys), self.list(values)])
            }
            // literal_eval rejects the outer comparison regardless of its operands.
            Expr::Compare(v) if !self.literal => {
                let mut operands = v.operands.into_iter();
                let left = self.expr(operands.next().expect("comparison has a left operand"))?;
                let ops = v
                    .ops
                    .iter()
                    .map(|op| self.ctx.builtins[&format!("ast._{op:?}")])
                    .collect();
                let comparators = self.exprs(operands)?;
                ("Compare", vec![left, self.list(ops), comparators])
            }
            Expr::If(v) => (
                "IfExp",
                vec![
                    self.expr(*v.test)?,
                    self.expr(*v.body)?,
                    self.expr(*v.orelse)?,
                ],
            ),
            other => {
                if self.literal {
                    let kind = format!("{:?}", ast::AnyNodeRef::from(&other).kind());
                    let line = self.location(range)[0];
                    return Err(Error::new(
                        "ValueError",
                        format!(
                            "malformed node or string on line {line}: {}(...)",
                            kind.trim_start_matches("Expr")
                        ),
                    ));
                }
                return Err(Error::new(
                    "NotImplementedError",
                    format!(
                        "ast.parse does not support {:?}",
                        ast::AnyNodeRef::from(&other).kind()
                    ),
                ));
            }
        };
        let mut value = self.node(kind, &fields, range);
        for (range, trailer) in trailers.into_iter().rev() {
            value = match trailer {
                Trailer::Unary(op) => {
                    let op = self.ctx.builtins[&format!("ast._{op:?}")];
                    self.node("UnaryOp", &[op, value], range)
                }
                Trailer::Binary(op, right) => {
                    let op = self.ctx.builtins[&format!("ast._{op:?}")];
                    let right = self.expr(*right)?;
                    self.node("BinOp", &[value, op, right], range)
                }
                Trailer::Attribute(name, ctx) => {
                    let name = self.ctx.text(name.as_str());
                    self.node("Attribute", &[value, name, self.context(ctx)?], range)
                }
                Trailer::Subscript(slice, ctx) => {
                    let slice = self.expr(*slice)?;
                    self.node("Subscript", &[value, slice, self.context(ctx)?], range)
                }
                Trailer::Call(args, keywords) => {
                    let args = self.exprs(args.args)?;
                    let keywords = self.list(keywords);
                    self.node("Call", &[value, args, keywords], range)
                }
            };
        }
        Ok(value)
    }
}

pub(super) fn parse(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let [src, filename, mode] =
        builtins::bind("parse", args, kwargs, ["source", "filename", "mode"])?;
    let copy = |text: &str| crate::Str::try_new(text).map_err(|_| crate::Error::allocation());
    let source = src.ok_or_else(|| {
        Error::new(
            "TypeError",
            "parse() missing 1 required positional argument: 'source'",
        )
    })?;
    let filename_id = filename;
    let filename = copy(
        filename
            .map(|f| ctx.str_value(f))
            .transpose()?
            .unwrap_or("<unknown>"),
    )?;
    let mode = copy(
        mode.map(|m| ctx.str_value(m))
            .transpose()?
            .unwrap_or("exec"),
    )?;
    // compile validates mode before asking the tokenizer to decode source bytes.
    if !matches!(mode.as_str(), "exec" | "eval" | "single" | "func_type") {
        return Err(Error::new(
            "ValueError",
            "compile() mode must be 'exec', 'eval', 'single' or 'func_type'",
        ));
    }
    let text;
    let decoded;
    let source = if ctx.string_storage(source).is_some() {
        text = copy(ctx.str_value(source)?)?;
        if text.contains('\0') {
            let file = filename_id.unwrap_or_else(|| ctx.text("<unknown>"));
            return Err(ctx.source_decode_error(crate::objects::SourceDecodeError::Null, file));
        }
        text.as_str()
    } else {
        let bytes = ctx.native_buffer_bytes(source).map_err(|_| {
            Error::new(
                "TypeError",
                "compile() arg 1 must be a string, bytes or AST object",
            )
        })?;
        decoded = crate::objects::decode_compile_source(bytes).map_err(|error| {
            let filename_id = filename_id.unwrap_or_else(|| ctx.text("<unknown>"));
            ctx.source_decode_error(error, filename_id)
        })?;
        decoded.as_str()
    };
    parse_source(ctx, source, &filename, &mode, false)
}

/// Evaluate literal source after applying the same syntax checks as parsing an expression.
pub(super) fn literal_source(ctx: &mut Context, source: &str) -> Result<Id> {
    parse_source(ctx, source, "<unknown>", "eval", true)
}

fn parse_source(
    ctx: &mut Context,
    source: &str,
    filename: &str,
    mode: &str,
    literal: bool,
) -> Result<Id> {
    // Python normalizes physical newlines before decoding literals and computing AST positions.
    let normalized;
    let source = if source.contains('\r') {
        normalized = source.replace("\r\n", "\n").replace('\r', "\n");
        &normalized
    } else {
        source
    };
    let lines = crate::dynamic::source_code::line_starts(source)?;
    let load = ctx.builtins["ast._Load"];
    let store = ctx.builtins["ast._Store"];
    let mut parser = Parser {
        ctx,
        source,
        literal,
        lines,
        definition_starts: Vec::new(),
        template_expressions: HashMap::new(),
        replacements: BTreeMap::new(),
        load,
        store,
    };
    if literal {
        super::literal::check_source(source, |offset| {
            parser.lines.partition_point(|&n| n <= offset) as i64
        })?;
    }
    let syntax = |err| Error::new("SyntaxError", format!("{filename}: {err}"));
    match mode {
        "exec" => {
            let input = imre_ast::indentation::prepare(source);
            let body =
                ruff_python_parser::parse_unchecked_source(&input.text, ast::PySourceType::Python);
            if let Some(limit) =
                crate::dynamic::source_code::IndentationDiagnostic::find(&body, source, true, None)
            {
                let file = parser.ctx.text(filename);
                return limit.raise(parser.ctx, file);
            }
            if let Some(error) = input.first_issue(body.errors().first()) {
                let issue = crate::dynamic::source_code::IndentationDiagnostic::from_issue(
                    source, error, true,
                );
                let file = parser.ctx.text(filename);
                return issue.raise(parser.ctx, file);
            }
            if let Some(error) = body.errors().first() {
                return Err(syntax(error.clone()));
            }
            parser.prepare_strings(source, body.tokens(), body.syntax())?;
            let body = parser.suite(body.into_syntax().body)?;
            let ignores = parser.list(vec![]);
            Ok(parsed_instance(
                parser.ctx,
                "Module",
                &[body, ignores],
                None,
            ))
        }
        "eval" => {
            let expr = ruff_python_parser::parse_expression(source).map_err(|err| {
                if literal {
                    let line = parser.location(err.location)[0];
                    super::literal::syntax_error(&err, source, line)
                } else {
                    syntax(err)
                }
            })?;
            parser.prepare_strings(source, expr.tokens(), expr.syntax())?;
            let expr = *expr.into_syntax().body;
            if literal && let Some(value) = parser.direct_literal(&expr)? {
                return Ok(value);
            }
            let body = parser.expr(expr)?;
            let expression = parsed_instance(parser.ctx, "Expression", &[body], None);
            if literal {
                super::literal::from_node(parser.ctx, expression)
            } else {
                Ok(expression)
            }
        }
        _ => Err(Error::new(
            "ValueError",
            "ast.parse supports exec or eval mode",
        )),
    }
}

#[cfg(test)]
mod stack_tests {
    #[test]
    fn convert_nested_conditionals_on_a_small_stack() {
        std::thread::Builder::new()
            .stack_size(256 * 1024)
            .spawn(|| {
                let argv = [c"imre".as_ptr()];
                // SAFETY: argv and the configuration outlive initialization, which returns an owned context.
                let mut ctx = unsafe {
                    Box::from_raw(crate::dynamic::ffi::imre_dyn_new(
                        1,
                        argv.as_ptr(),
                        c"{\"paths\":[]}".as_ptr(),
                    ))
                };
                ctx.module("ast").unwrap();
                let source = format!("{}z", "x if y else ".repeat(512));
                super::parse_source(&mut ctx, &source, "nested.py", "eval", false).unwrap();
            })
            .unwrap()
            .join()
            .unwrap();
    }
}
