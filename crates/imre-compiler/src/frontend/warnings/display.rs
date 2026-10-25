//! Bind the ordinary formatting hooks in source order, then render using the
//! same cache and text stream objects visible to the caller.
use super::*;

impl Checker<'_> {
    pub(in crate::frontend) fn warning_display(
        &mut self,
        call: &ast::ExprCall,
        show: bool,
    ) -> Result<Expr, String> {
        let fields: &[&str] = if show {
            &["message", "category", "filename", "lineno", "file", "line"]
        } else {
            &["message", "category", "filename", "lineno", "line"]
        };
        let none = || Expr {
            ty: Type::None,
            kind: ExprKind::None,
        };
        let params = fields
            .iter()
            .map(|&name| Parameter {
                name: name.into(),
                ty: Type::None,
                kind: ParameterKind::PositionalOrKeyword,
                default: matches!(name, "file" | "line").then(none),
                sign_only: false,
            })
            .collect::<Vec<_>>();
        let mut stdout = false;
        let args = self.arguments_with(call, &params, None, |c, i, node| {
            let field = fields[i];
            if field == "file"
                && let Some(Binding::Intrinsic(stream @ (Intrinsic::Stdout | Intrinsic::Stderr))) =
                    c.resolve(node)
            {
                stdout = matches!(stream, Intrinsic::Stdout);
                return Ok(none());
            }
            let value = Self::string_value(c.expr(node)?);
            let underlying = c.types.payload(value.ty).unwrap_or(value.ty);
            let valid = match field {
                "category" => value.ty == Type::BuiltinClass,
                "filename" => value.ty == Type::Str,
                "line" => matches!(underlying, Type::None | Type::Str),
                "file" => matches!(underlying, Type::None | Type::TextIO | Type::StringIO),
                "lineno" => matches!(
                    value.ty,
                    Type::Int | Type::Bool | Type::Float | Type::Str | Type::None
                ),
                "message" => matches!(
                    value.ty,
                    Type::Str
                        | Type::Int
                        | Type::Float
                        | Type::Bool
                        | Type::Bytes
                        | Type::None
                        | Type::Exception
                        | Type::NamedException(_)
                ),
                _ => unreachable!(),
            };
            if !valid {
                return Err(error(
                    c.source,
                    node,
                    &format!(
                        "unsupported warning display {field}: {}",
                        c.type_name(value.ty)
                    ),
                ));
            }
            Ok(value)
        })?;
        let mut scope = calls::ArgumentScope::default();
        let held = args
            .values
            .into_iter()
            .map(|v| scope.hold(self, v))
            .collect::<Vec<_>>();
        let field = |index: usize| held[args.order[index]].clone();
        let number = field(3);
        // linecache compares int/bool numerically. Non-int values cannot index
        // its list: the public formatter catches that error after populating the cache.
        let index = match number.ty {
            Type::Int => number.clone(),
            Type::Bool => powers::integer(number.clone()),
            _ => pipelines::integer(0),
        };
        let string = |value: Expr| {
            if value.ty == Type::Str {
                value
            } else {
                Expr {
                    ty: Type::Str,
                    kind: ExprKind::Builtin(Builtin::Str, vec![value]),
                }
            }
        };
        // A line-number conversion failure precedes the message's __str__.
        let number = scope.hold(self, string(number));
        let (root, environment) = self.linecache_search();
        let text = Expr {
            ty: Type::Str,
            kind: ExprKind::Builtin(
                Builtin::WarningFormat {
                    entry: self.linecache_entry(),
                    environment,
                },
                vec![
                    string(field(0)),
                    annotations::type_name(field(1)),
                    field(2),
                    number,
                    index,
                    field(fields.len() - 1),
                    root,
                    unpacking::string("\n"),
                ],
            ),
        };
        Ok(scope.finish(if show {
            Expr {
                ty: Type::None,
                kind: ExprKind::Builtin(Builtin::WarningWrite { stdout }, vec![field(4), text]),
            }
        } else {
            text
        }))
    }
}
