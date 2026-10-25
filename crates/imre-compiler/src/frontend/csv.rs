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
        let mut seen = HashSet::new();
        for option in &call.arguments.keywords {
            let name = option.arg.as_deref().unwrap_or("");
            if !seen.insert(name) {
                return Err(error(self.source, option, "repeated csv.reader option"));
            }
            match name {
                "dialect" if matches!(&option.value, ast::Expr::StringLiteral(s) if s.value.to_str() == "excel") =>
                    {}
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
                vec![input],
            ),
        })
    }
}
