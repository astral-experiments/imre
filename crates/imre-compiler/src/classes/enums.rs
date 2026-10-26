//! Sealed enums assign one canonical member to each distinct value.

use super::*;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum HttpEnum {
    Status,
    Method,
}

impl HttpEnum {
    /// Only the compiler's immutable CPython source has these sealed __new__ layouts.
    pub(super) fn bundled(module: &SourceModule, node: &ast::StmtClassDef) -> Option<Self> {
        if !module.is_bundled("http") {
            return None;
        }
        match node.name.as_str() {
            "HTTPStatus" => Some(Self::Status),
            "HTTPMethod" => Some(Self::Method),
            _ => None,
        }
    }
}

#[derive(Default)]
pub(crate) struct Members {
    pub(crate) plain: bool,
    pub(crate) http: Option<HttpEnum>,
    pub(crate) http_members: Vec<crate::ir::HttpMember>,
    pub(crate) values: Vec<(String, String)>,
    pub(crate) names: HashMap<String, usize>,
    pub(crate) declared: Vec<String>,
    indices: HashMap<String, usize>,
}

impl Members {
    pub(super) fn declare(
        &mut self,
        graph: &SourceGraph,
        module: &SourceModule,
        module_id: usize,
        member: &ast::StmtAssign,
        bindings: &HashSet<String>,
    ) -> Result<String, String> {
        let [ast::Expr::Name(target)] = member.targets.as_slice() else {
            return Err(module.error(member, "a StrEnum member requires one name"));
        };
        let name = target.id.as_str();
        if name.starts_with('_')
            || matches!(name, "name" | "value" | "mro")
            || bindings.contains(name)
        {
            return Err(module.error(target, "duplicate or reserved StrEnum member name"));
        }
        let value = match &*member.value {
            ast::Expr::Tuple(tuple) if let Some(http) = self.http => {
                let invalid = || module.error(member, "invalid bundled HTTP enum member");
                let string = |expr: &ast::Expr| {
                    expr.as_string_literal_expr()
                        .map(|s| s.value.to_str().to_owned())
                        .ok_or_else(invalid)
                };
                let (code, value, phrase, description) = match (http, tuple.elts.as_slice()) {
                    (HttpEnum::Status, [number, phrase, rest @ ..]) if rest.len() <= 1 => {
                        let ast::Number::Int(number) =
                            &number.as_number_literal_expr().ok_or_else(invalid)?.value
                        else {
                            return Err(invalid());
                        };
                        let code = number
                            .as_i64()
                            .and_then(|n| u16::try_from(n).ok())
                            .ok_or_else(invalid)?;
                        (
                            Some(code),
                            code.to_string(),
                            string(phrase)?,
                            rest.first().map(string).transpose()?.unwrap_or_default(),
                        )
                    }
                    (HttpEnum::Method, [value, description]) => {
                        (None, string(value)?, String::new(), string(description)?)
                    }
                    _ => return Err(invalid()),
                };
                self.http_members.push(crate::ir::HttpMember {
                    code,
                    phrase,
                    description,
                });
                value
            }
            ast::Expr::StringLiteral(literal) => {
                crate::frontend::check_string(literal, &module.source)?;
                literal.value.to_str().to_owned()
            }
            ast::Expr::Name(previous)
                if let Some(&index) = self.names.get(previous.id.as_str()) =>
            {
                self.values[index].1.clone()
            }
            ast::Expr::Call(call)
                if call.arguments.args.is_empty()
                    && call.arguments.keywords.is_empty()
                    && !shadowed(&call.func, bindings)
                    && !self.plain
                    && graph.resolve(module_id, &call.func)
                        == Some(Binding::Intrinsic(Intrinsic::EnumAuto)) =>
            {
                if !name.is_ascii() {
                    return Err(
                        module.error(target, "StrEnum auto() requires an ASCII member name")
                    );
                }
                name.to_ascii_lowercase()
            }
            _ => {
                return Err(module.error(
                    &*member.value,
                    "StrEnum members require a literal string, auto(), or an earlier member",
                ));
            }
        };
        let index = *self.indices.entry(value.clone()).or_insert_with(|| {
            let index = self.values.len();
            self.values.push((name.to_owned(), value));
            index
        });
        self.names.insert(name.to_owned(), index);
        self.declared.push(name.to_owned());
        Ok(name.to_owned())
    }
}

fn shadowed(mut expression: &ast::Expr, bindings: &HashSet<String>) -> bool {
    while let ast::Expr::Attribute(attribute) = expression {
        expression = &attribute.value;
    }
    matches!(expression, ast::Expr::Name(name) if bindings.contains(name.id.as_str()))
}
