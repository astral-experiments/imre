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

    fn suite(&mut self, body: &[ast::Stmt]) -> Result<Id> {
        let values = body
            .iter()
            .map(|s| self.statement(s))
            .collect::<Result<_>>()?;
        Ok(self.list(values))
    }

    fn exprs(&mut self, exprs: &[ast::Expr]) -> Result<Id> {
        let values = exprs.iter().map(|e| self.expr(e)).collect::<Result<_>>()?;
        Ok(self.list(values))
    }

    fn optional(&mut self, expr: Option<&ast::Expr>) -> Result<Id> {
        expr.map(|e| self.expr(e))
            .transpose()
            .map(|e| e.unwrap_or(NONE))
    }

    fn arg(&mut self, parameter: &ast::Parameter) -> Result<Id> {
        let name = self.ctx.text(parameter.name.as_str());
        let annotation = self.optional(parameter.annotation.as_deref())?;
        Ok(self.node("arg", &[name, annotation, NONE], parameter.range()))
    }

    fn parameters(&mut self, p: &ast::Parameters) -> Result<Id> {
        let mut args = |xs: &[ast::ParameterWithDefault]| -> Result<_> {
            let vals = xs
                .iter()
                .map(|x| self.arg(&x.parameter))
                .collect::<Result<_>>()?;
            Ok(self.list(vals))
        };
        let positional_only = args(&p.posonlyargs)?;
        let positional = args(&p.args)?;
        let keyword_only = args(&p.kwonlyargs)?;
        let vararg = p
            .vararg
            .as_ref()
            .map(|a| self.arg(a))
            .transpose()?
            .unwrap_or(NONE);
        let kwarg = p
            .kwarg
            .as_ref()
            .map(|a| self.arg(a))
            .transpose()?
            .unwrap_or(NONE);
        let kd = p
            .kwonlyargs
            .iter()
            .map(|a| self.optional(a.default.as_deref()))
            .collect::<Result<_>>()?;
        let keyword_defaults = self.list(kd);
        let defaults = p
            .posonlyargs
            .iter()
            .chain(&p.args)
            .filter_map(|a| a.default.as_deref())
            .map(|e| self.expr(e))
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

    fn statement(&mut self, s: &ast::Stmt) -> Result<Id> {
        use ast::Stmt;
        let (kind, fields) = match s {
            Stmt::Expr(v) => ("Expr", vec![self.expr(&v.value)?]),
            Stmt::Pass(_) => ("Pass", vec![]),
            Stmt::Assign(v) => (
                "Assign",
                vec![self.exprs(&v.targets)?, self.expr(&v.value)?, NONE],
            ),
            Stmt::Return(v) => ("Return", vec![self.optional(v.value.as_deref())?]),
            Stmt::If(v) => {
                let mut otherwise = self.list(vec![]);
                for clause in v.elif_else_clauses.iter().rev() {
                    let body = self.suite(&clause.body)?;
                    otherwise = if let Some(test) = &clause.test {
                        let test = self.expr(test)?;
                        let nested = self.node(
                            "If",
                            &[test, body, otherwise],
                            TextRange::new(clause.start(), v.end()),
                        );
                        self.list(vec![nested])
                    } else {
                        body
                    };
                }
                (
                    "If",
                    vec![self.expr(&v.test)?, self.suite(&v.body)?, otherwise],
                )
            }
            Stmt::FunctionDef(v) if !v.is_async && v.type_params.is_none() => {
                let decorators = v
                    .decorator_list
                    .iter()
                    .map(|d| self.expr(&d.expression))
                    .collect::<Result<_>>()?;
                (
                    "FunctionDef",
                    vec![
                        self.ctx.text(v.name.as_str()),
                        self.parameters(&v.parameters)?,
                        self.suite(&v.body)?,
                        self.list(decorators),
                        self.optional(v.returns.as_deref())?,
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
                        ast::AnyNodeRef::from(other).kind()
                    ),
                ));
            }
        };
        Ok(self.node(kind, &fields, s.range()))
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

    fn expr(&mut self, e: &ast::Expr) -> Result<Id> {
        use ast::Expr;
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
            Expr::Attribute(v) => (
                "Attribute",
                vec![
                    self.expr(&v.value)?,
                    self.ctx.text(v.attr.as_str()),
                    self.context(v.ctx)?,
                ],
            ),
            Expr::Subscript(v) => (
                "Subscript",
                vec![
                    self.expr(&v.value)?,
                    self.expr(&v.slice)?,
                    self.context(v.ctx)?,
                ],
            ),
            Expr::Tuple(v) => ("Tuple", vec![self.exprs(&v.elts)?, self.context(v.ctx)?]),
            Expr::List(v) => ("List", vec![self.exprs(&v.elts)?, self.context(v.ctx)?]),
            Expr::If(v) => (
                "IfExp",
                vec![
                    self.expr(&v.test)?,
                    self.expr(&v.body)?,
                    self.expr(&v.orelse)?,
                ],
            ),
            Expr::Call(v) => {
                let mut keys = Vec::new();
                for k in &v.arguments.keywords {
                    let name = k
                        .arg
                        .as_ref()
                        .map(|a| self.ctx.text(a.as_str()))
                        .unwrap_or(NONE);
                    let value = self.expr(&k.value)?;
                    keys.push(self.node("keyword", &[name, value], k.range()));
                }
                (
                    "Call",
                    vec![
                        self.expr(&v.func)?,
                        self.exprs(&v.arguments.args)?,
                        self.list(keys),
                    ],
                )
            }
            other => {
                return Err(Error::new(
                    "NotImplementedError",
                    format!(
                        "ast.parse does not support {:?}",
                        ast::AnyNodeRef::from(other).kind()
                    ),
                ));
            }
        };
        Ok(self.node(kind, &fields, e.range()))
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
    lines.extend(source.match_indices('\n').map(|(i, _)| i + 1));
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
            let body = parser.suite(&body.syntax().body)?;
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
            let body = parser.expr(&expr.syntax().body)?;
            Ok(parsed_instance(parser.ctx, "Expression", &[body], None))
        }
        _ => Err(Error::new(
            "ValueError",
            "ast.parse supports exec or eval mode",
        )),
    }
}
