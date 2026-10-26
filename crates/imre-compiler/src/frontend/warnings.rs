//! Bind native warning filters in source order before validating their arguments.
mod display;
use super::*;

const CATEGORIES: &[&str] = &[
    "Warning",
    "ResourceWarning",
    "RuntimeWarning",
    "UserWarning",
    "DeprecationWarning",
    "PendingDeprecationWarning",
    "SyntaxWarning",
    "FutureWarning",
    "ImportWarning",
    "UnicodeWarning",
    "BytesWarning",
    "EncodingWarning",
];

/// Warning categories have the ordinary, immutable identity of builtin classes.
pub(super) fn is_warning_class(name: &str) -> bool {
    CATEGORIES.contains(&name)
}

impl Checker<'_> {
    /// Keep the source spelling for CPython's filter deduplication, separately
    /// from the expression that the native regular expression engine consumes.
    fn warning_pattern(
        &mut self,
        node: &ast::Expr,
        message: bool,
        compiled: &mut String,
    ) -> Result<Expr, String> {
        let value = self.expr(node)?;
        if value.ty != Type::Str {
            return Ok(value);
        }
        let ast::Expr::StringLiteral(literal) = node else {
            return Err(error(
                self.source,
                node,
                "warning filter regex requires a fixed string pattern",
            ));
        };
        let original = literal.value.to_str();
        if !original.is_empty() {
            // The resource emitter's message is ASCII. Python has extra
            // ignore-case pairs across ASCII/Unicode (dotted and dotless I).
            if message
                && (!original.is_ascii() || original.contains(r"\u") || original.contains(r"\U"))
            {
                return Err(error(
                    self.source,
                    node,
                    "case-insensitive warning message patterns require ASCII literals",
                ));
            }
            *compiled = crate::fixed_regex::translate(original, false)
                .map_err(|reason| {
                    error(
                        self.source,
                        node,
                        &format!("unsupported warning filter regex: {reason}"),
                    )
                })?
                .source;
        }
        Ok(value)
    }

    /// Share the ordinary positional/keyword binder; native validation runs
    /// after all supplied expressions, including invalid ones, are evaluated.
    pub(super) fn warning_filter(
        &mut self,
        call: &ast::ExprCall,
        simple: bool,
    ) -> Result<Expr, String> {
        let fields: &[&str] = if simple {
            &["action", "category", "lineno", "append"]
        } else {
            &[
                "action", "message", "category", "module", "lineno", "append",
            ]
        };
        let empty = || unpacking::string("");
        let no = Expr {
            ty: Type::Bool,
            kind: ExprKind::Bool(false),
        };
        let params = fields
            .iter()
            .map(|&name| {
                let default = match name {
                    "action" => None,
                    "message" | "module" => Some(empty()),
                    "append" => Some(no.clone()),
                    _ => Some(pipelines::integer(0)),
                };
                Parameter {
                    name: name.into(),
                    ty: default.as_ref().map_or(Type::Str, |v| v.ty),
                    kind: ParameterKind::PositionalOrKeyword,
                    default,
                    sign_only: false,
                }
            })
            .collect::<Vec<_>>();
        let mut category = 0;
        let mut invalid = 0;
        let mut message_re = String::new();
        let mut module_re = String::new();
        let args = self.arguments_with(call, &params, None, |checker, i, arg| {
            let value = match fields[i] {
                "action" => checker.expr_expected(arg, Type::Str)?,
                "message" => checker.warning_pattern(arg, true, &mut message_re)?,
                "module" => checker.warning_pattern(arg, false, &mut module_re)?,
                "category" => {
                    if let Some(id) = CATEGORIES
                        .iter()
                        .position(|name| checker.unshadowed_builtin(arg, name))
                    {
                        category = id as u32;
                        pipelines::integer(0)
                    } else if !simple && checker.unshadowed_builtin(arg, "int") {
                        invalid |= 2;
                        pipelines::integer(0)
                    } else if !simple {
                        let value = checker.expr(arg)?;
                        if !matches!(value.ty, Type::Int | Type::Bool | Type::Str | Type::None) {
                            return Err(error(
                                checker.source,
                                arg,
                                "warning category must be a statically known builtin class",
                            ));
                        }
                        invalid |= 2;
                        value
                    } else {
                        return Err(error(
                            checker.source,
                            arg,
                            "warning category must be a statically known builtin class",
                        ));
                    }
                }
                "lineno" if checker.unshadowed_builtin(arg, "int") => {
                    invalid |= 8;
                    pipelines::integer(0)
                }
                "lineno" | "append" => checker.expr(arg)?,
                _ => unreachable!(),
            };
            Ok(value)
        })?;

        let mut scope = calls::ArgumentScope::default();
        let held: Vec<_> = args
            .values
            .into_iter()
            .map(|v| scope.hold(self, v))
            .collect();
        let field = |name| {
            fields
                .iter()
                .position(|f| *f == name)
                .map(|i| held[args.order[i]].clone())
        };
        let mut pattern = |name, bit| match field(name) {
            Some(v) if v.ty == Type::Str => v,
            Some(_) => {
                invalid |= bit;
                empty()
            }
            None => empty(),
        };
        let message = pattern("message", 1);
        let module = pattern("module", 4);
        let line = field("lineno").unwrap();
        let line = if matches!(line.ty, Type::Int | Type::Bool) {
            powers::integer(line)
        } else {
            invalid |= 8;
            pipelines::integer(0)
        };
        let append = field("append").unwrap();
        if !matches!(append.ty, Type::Bool | Type::Int) {
            return Err(error(
                self.source,
                call,
                "warning filter append requires a native bool or int",
            ));
        }
        let append = Expr {
            ty: Type::Bool,
            kind: ExprKind::Builtin(Builtin::Bool, vec![append]),
        };
        Ok(scope.finish(Expr {
            ty: Type::None,
            kind: ExprKind::Builtin(
                Builtin::WarningFilter { category, invalid },
                vec![
                    field("action").unwrap(),
                    message,
                    unpacking::string(message_re),
                    module,
                    unpacking::string(module_re),
                    line,
                    append,
                ],
            ),
        }))
    }
}
