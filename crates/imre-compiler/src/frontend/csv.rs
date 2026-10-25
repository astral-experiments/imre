//! A fixed dialect and a live source produce a first-class, single-pass reader.

use super::*;

impl Checker<'_> {
    pub(super) fn csv_reader(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        let [lines] = call.arguments.args.as_ref() else {
            return Err(error(
                self.source,
                call,
                "csv.reader expects a text stream or list of strings",
            ));
        };
        let input = if matches!(lines, ast::Expr::List(_)) {
            self.expr_expected(lines, self.types.list(Type::Str))?
        } else {
            self.expr(lines)?
        };
        if input.ty != Type::TextIO
            && input.ty != Type::Str
            && self.types.element(input.ty) != Some(Type::Str)
        {
            return Err(error(
                self.source,
                lines,
                "csv.reader requires TextIO, str or list[str]",
            ));
        }
        let (mut delimiter, mut quote, mut escape, mut flags) = (',', Some('"'), None, 1);
        let mut args = vec![input];
        let mut seen = HashSet::new();
        for option in &call.arguments.keywords {
            let name = option.arg.as_deref().unwrap_or("");
            if !seen.insert(name) {
                return Err(error(self.source, option, "repeated csv.reader option"));
            }
            match name {
                "dialect" if matches!(&option.value, ast::Expr::StringLiteral(s) if s.value.to_str() == "excel") =>
                    {}
                "lineterminator" => args.extend(self.csv_terminator(&option.value)?),
                "delimiter" | "quotechar" | "escapechar" => {
                    let value = match &option.value {
                        ast::Expr::NoneLiteral(_) if name != "delimiter" => None,
                        ast::Expr::StringLiteral(s) if s.value.to_str().chars().count() == 1 => {
                            check_string(s, self.source)?;
                            s.value.to_str().chars().next()
                        }
                        _ => {
                            return Err(error(
                                self.source,
                                option,
                                "expected a literal CSV character",
                            ));
                        }
                    };
                    match name {
                        "delimiter" => delimiter = value.unwrap(),
                        "quotechar" => quote = value,
                        _ => escape = value,
                    }
                }
                "doublequote" | "skipinitialspace" | "strict" => {
                    let ast::Expr::BooleanLiteral(literal) = &option.value else {
                        return Err(error(self.source, option, "expected a literal CSV boolean"));
                    };
                    let bit = match name {
                        "doublequote" => 1,
                        "skipinitialspace" => 2,
                        _ => 4,
                    };
                    if literal.value {
                        flags |= bit;
                    } else {
                        flags &= !bit;
                    }
                }
                _ => {
                    return Err(error(
                        self.source,
                        option,
                        "unsupported csv.reader dialect or option",
                    ));
                }
            }
        }
        Ok(Expr {
            ty: Type::CsvReader,
            kind: ExprKind::Builtin(
                Builtin::CsvReader {
                    delimiter,
                    quote,
                    escape,
                    flags,
                },
                args,
            ),
        })
    }
    /// Forward the checked text or the exact invalid type name to the runtime.
    /// Validation follows source iteration, including a closed file's own error.
    fn csv_terminator(&mut self, option: &ast::Expr) -> Result<[Expr; 2], String> {
        let value = Self::enum_text(self.expr(option)?);
        let ty = value.ty;
        let payload = self.types.payload(ty);
        let string = |name: &str| Expr {
            ty: Type::Str,
            kind: ExprKind::Str(name.to_owned()),
        };
        if matches!(payload.unwrap_or(ty), Type::Str | Type::StrLiteral(_)) {
            return Ok([value, string("NoneType")]);
        }
        let concrete = payload.unwrap_or(ty);
        let record = self
            .types
            .namedtuple_definition(concrete)
            .and_then(|definition| self.types.namedtuple(definition));
        let invalid = match concrete {
            Type::None => "NoneType",
            Type::Bool => "bool",
            Type::Int => "int",
            Type::Float => "float",
            Type::Bytes => "bytes",
            Type::ListInt | Type::List(_) => "list",
            Type::Dict(_) => "dict",
            Type::Set(_) => "set",
            Type::Tuple(_) => record
                .as_ref()
                .map_or("tuple", |record| record.name.as_str()),
            Type::Class(id) => self.classes.declarations.definitions[id].name.as_str(),
            _ => {
                return Err(error(
                    self.source,
                    option,
                    "unsupported csv.reader lineterminator type",
                ));
            }
        };
        let invalid = string(invalid);
        let name = if payload.is_some() {
            let slot = self.hidden(ty);
            let test = Expr {
                ty: Type::Bool,
                kind: ExprKind::Builtin(Builtin::IsNone, vec![self.local(slot)]),
            };
            Expr {
                ty: Type::Str,
                kind: ExprKind::Scope {
                    body: vec![Statement::Assign(slot, value)],
                    clear: vec![slot],
                    value: Box::new(Expr {
                        ty: Type::Str,
                        kind: ExprKind::Conditional(
                            Box::new(test),
                            Box::new(string("NoneType")),
                            Box::new(invalid),
                        ),
                    }),
                },
            }
        } else {
            Expr {
                ty: Type::Str,
                kind: ExprKind::Scope {
                    body: vec![Statement::Expr(value)],
                    clear: vec![],
                    value: Box::new(invalid),
                },
            }
        };
        let absent = Expr {
            ty: self.types.optional(Type::Str),
            kind: ExprKind::Absent(Box::new(Expr {
                ty: Type::None,
                kind: ExprKind::None,
            })),
        };
        Ok([absent, name])
    }
}
