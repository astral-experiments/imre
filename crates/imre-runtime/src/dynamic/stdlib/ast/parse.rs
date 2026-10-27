//! Convert the Rust parser's tree into mutable Python AST nodes.
use ruff_python_ast as ast;
use ruff_text_size::{Ranged, TextRange};

use super::{Context, Error, Id, NONE, Result, Value, builtins, parsed_instance};
use crate::dynamic::value::{FALSE, TRUE};

struct Parser<'a> {
    ctx: &'a mut Context,
    lines: Vec<usize>,
    load: Id,
    store: Id,
}

enum Trailer {
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
        let range = s.range();
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

    fn expr(&mut self, mut e: ast::Expr) -> Result<Id> {
        use ast::Expr;
        // Postfix expressions can grow without parentheses. Move their operands
        // out of Ruff's tree so conversion and destruction both use bounded stack.
        let mut trailers = Vec::new();
        loop {
            let range = e.range();
            let (next, trailer) = match e {
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
            Expr::NoneLiteral(_) => ("Constant", vec![NONE, NONE]),
            Expr::BooleanLiteral(v) => ("Constant", vec![if v.value { TRUE } else { FALSE }, NONE]),
            Expr::NumberLiteral(v) => {
                let ast::Number::Int(n) = &v.value else {
                    return Err(Error::new("NotImplementedError", "noninteger AST literal"));
                };
                let integer = crate::parse_integer_literal(&n.to_string())
                    .ok_or_else(|| Error::new("ValueError", "invalid integer literal"))?;
                (
                    "Constant",
                    vec![self.ctx.alloc(Value::Int(crate::Int::new(integer))), NONE],
                )
            }
            Expr::Tuple(v) => ("Tuple", vec![self.exprs(v.elts)?, self.context(v.ctx)?]),
            Expr::List(v) => ("List", vec![self.exprs(v.elts)?, self.context(v.ctx)?]),
            Expr::If(v) => (
                "IfExp",
                vec![
                    self.expr(*v.test)?,
                    self.expr(*v.body)?,
                    self.expr(*v.orelse)?,
                ],
            ),
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
        let mut value = self.node(kind, &fields, range);
        for (range, trailer) in trailers.into_iter().rev() {
            value = match trailer {
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
    let mut lines = vec![0];
    let bytes = source.as_bytes();
    for (i, &byte) in bytes.iter().enumerate() {
        if byte == b'\n' || (byte == b'\r' && bytes.get(i + 1) != Some(&b'\n')) {
            lines.push(i + 1);
        }
    }
    let load = ctx.builtins["ast._Load"];
    let store = ctx.builtins["ast._Store"];
    let mut parser = Parser {
        ctx,
        lines,
        load,
        store,
    };
    let syntax = |err| Error::new("SyntaxError", format!("{filename}: {err}"));
    match mode.as_str() {
        "exec" => {
            let body = ruff_python_parser::parse_module(&source).map_err(syntax)?;
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
            let expr = ruff_python_parser::parse_expression(&source).map_err(syntax)?;
            let body = parser.expr(*expr.into_syntax().body)?;
            Ok(parsed_instance(parser.ctx, "Expression", &[body], None))
        }
        _ => Err(Error::new(
            "ValueError",
            "ast.parse supports exec or eval mode",
        )),
    }
}
