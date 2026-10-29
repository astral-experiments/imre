//! Convert the Rust parser's tree into mutable Python AST nodes.
use ruff_python_ast as ast;
use ruff_text_size::{Ranged, TextRange, TextSize};

use super::{Context, Error, Id, NONE, Result, Value, builtins, parsed_instance};
use crate::dynamic::complex::Complex;
use crate::dynamic::value::{ELLIPSIS, FALSE, TRUE};

struct Parser<'a> {
    ctx: &'a mut Context,
    literal: bool,
    lines: Vec<usize>,
    function_starts: Vec<TextSize>,
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

    fn optional(&mut self, expr: Option<Box<ast::Expr>>) -> Result<Id> {
        expr.map(|e| self.expr(*e))
            .transpose()
            .map(|e| e.unwrap_or(NONE))
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

    fn statement(&mut self, s: ast::Stmt) -> Result<Id> {
        use ast::Stmt;
        let range = self.statement_range(&s);
        let (kind, fields) = match s {
            Stmt::Expr(v) => ("Expr", vec![self.expr(*v.value)?]),
            Stmt::Pass(_) => ("Pass", vec![]),
            Stmt::Assign(v) => (
                "Assign",
                vec![self.exprs(v.targets)?, self.expr(*v.value)?, NONE],
            ),
            Stmt::Return(v) => ("Return", vec![self.optional(v.value)?]),
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
            Stmt::FunctionDef(mut v) if !v.is_async && v.type_params.is_none() => {
                let decorators = v
                    .decorator_list
                    .into_iter()
                    .map(|d| self.expr(d.expression))
                    .collect::<Result<_>>()?;
                (
                    "FunctionDef",
                    vec![
                        self.ctx.text(v.name.as_str()),
                        self.parameters(&mut v.parameters)?,
                        self.suite(v.body)?,
                        self.list(decorators),
                        self.optional(v.returns)?,
                        NONE,
                        self.list(vec![]),
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
        if let ast::Stmt::FunctionDef(function) = statement
            && let Some(last) = function.decorator_list.last()
        {
            // Ruff includes decorators in the range. Python starts at the def keyword.
            let next = self
                .function_starts
                .partition_point(|start| *start < last.end());
            return TextRange::new(self.function_starts[next], function.end());
        }
        statement.range()
    }

    fn context(&self, context: ast::ExprContext) -> Result<Id> {
        match context {
            ast::ExprContext::Load => Ok(self.load),
            ast::ExprContext::Store => Ok(self.store),
            _ => Err(Error::new(
                "NotImplementedError",
                "unsupported AST expression context",
            )),
        }
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
                    let mut keywords = Vec::new();
                    for keyword in std::mem::take(&mut v.arguments.keywords) {
                        let range = keyword.range();
                        let name = keyword
                            .arg
                            .as_ref()
                            .map(|n| self.ctx.text(n.as_str()))
                            .unwrap_or(NONE);
                        let value = self.expr(keyword.value)?;
                        keywords.push(self.node("keyword", &[name, value], range));
                    }
                    (v.func, Trailer::Call(v.arguments, keywords))
                }
                _ => break,
            };
            trailers.push((range, trailer));
            e = *next;
        }
        let range = e.range();
        let (kind, fields) = match e {
            Expr::Name(v) => (
                "Name",
                vec![self.ctx.text(v.id.as_str()), self.context(v.ctx)?],
            ),
            Expr::StringLiteral(v) => ("Constant", vec![self.ctx.text(v.value.to_string()), NONE]),
            Expr::BytesLiteral(v) => (
                "Constant",
                vec![
                    self.ctx.alloc(Value::Bytes(v.value.bytes().collect())),
                    NONE,
                ],
            ),
            Expr::EllipsisLiteral(_) => ("Constant", vec![ELLIPSIS, NONE]),
            Expr::NoneLiteral(_) => ("Constant", vec![NONE, NONE]),
            Expr::BooleanLiteral(v) => ("Constant", vec![if v.value { TRUE } else { FALSE }, NONE]),
            Expr::NumberLiteral(v) => {
                let number = match v.value {
                    ast::Number::Int(n) => {
                        let n = crate::parse_integer_literal(&n.to_string())
                            .ok_or_else(|| Error::new("ValueError", "invalid integer literal"))?;
                        Value::Int(crate::Int::new(n))
                    }
                    ast::Number::Float(n) => Value::Float(n),
                    ast::Number::Complex { real, imag } => Value::Complex(Complex { real, imag }),
                };
                ("Constant", vec![self.ctx.alloc(number), NONE])
            }
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
    let source = ctx
        .str_value(src.ok_or_else(|| {
            Error::new(
                "TypeError",
                "parse() missing 1 required positional argument: 'source'",
            )
        })?)?
        .to_owned();
    let filename = filename
        .map(|f| ctx.str_value(f))
        .transpose()?
        .unwrap_or("<unknown>")
        .to_owned();
    let mode = mode
        .map(|m| ctx.str_value(m))
        .transpose()?
        .unwrap_or("exec")
        .to_owned();
    parse_source(ctx, &source, &filename, &mode, false)
}

pub(super) fn parse_source(
    ctx: &mut Context,
    source: &str,
    filename: &str,
    mode: &str,
    literal: bool,
) -> Result<Id> {
    let bytes = source.as_bytes();
    let lines = crate::dynamic::source_code::line_starts(bytes.iter().enumerate().filter_map(
        |(i, &byte)| {
            (byte == b'\n' || (byte == b'\r' && bytes.get(i + 1) != Some(&b'\n'))).then_some(i + 1)
        },
    ))?;
    let load = ctx.builtins["ast._Load"];
    let store = ctx.builtins["ast._Store"];
    let mut parser = Parser {
        ctx,
        literal,
        lines,
        function_starts: Vec::new(),
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
            let body = ruff_python_parser::parse_module(source).map_err(syntax)?;
            parser.function_starts = body
                .tokens()
                .iter()
                .filter(|token| token.kind() == ast::token::TokenKind::Def)
                .map(Ranged::start)
                .collect();
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
            let body = parser.expr(*expr.into_syntax().body)?;
            Ok(parsed_instance(parser.ctx, "Expression", &[body], None))
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
