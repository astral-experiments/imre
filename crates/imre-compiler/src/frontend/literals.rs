use super::*;

pub(super) fn annotation(slice: &ast::Expr, types: &Types, source: &str) -> Result<Type, String> {
    let arguments = match slice {
        ast::Expr::Tuple(tuple) => tuple.elts.as_slice(),
        item => std::slice::from_ref(item),
    };
    if arguments.is_empty() {
        return Err(error(source, slice, "Literal requires at least one string"));
    }
    let mut values = Vec::with_capacity(arguments.len());
    for argument in arguments {
        let ast::Expr::StringLiteral(string) = argument else {
            return Err(error(
                source,
                argument,
                "Literal requires fixed string values",
            ));
        };
        check_string(string, source)?;
        values.push(string.value.to_string());
    }
    Ok(types.string_literal(values))
}

/// A literal may flow to a wider declared domain only when every possible
/// value is accepted. Unconstrained strings never acquire this proof.
pub(super) fn accepts(types: &Types, expected: Type, value: &Expr) -> bool {
    if let ExprKind::Str(text) = &value.kind
        && let Some(choices) = types.literal_strings(expected)
    {
        return choices.binary_search(text).is_ok();
    }
    domain_fits(types, value.ty, expected)
}

pub(super) fn domain_fits(types: &Types, actual: Type, expected: Type) -> bool {
    let Some(actual) = types.literal_strings(actual) else {
        return false;
    };
    expected == Type::Str
        || types.literal_strings(expected).is_some_and(|choices| {
            actual
                .iter()
                .all(|text| choices.binary_search(text).is_ok())
        })
}

pub(super) fn view(value: Expr, ty: Type) -> Expr {
    Expr {
        ty,
        kind: ExprKind::Builtin(Builtin::LiteralStringView, vec![value]),
    }
}

impl Checker<'_> {
    /// Equality with a constant proves the value of an exact string local.
    /// Field reads and comparisons with other expressions can observe mutation.
    pub(super) fn apply_literal_fact(&mut self, test: &ast::Expr, truth: bool) {
        let ast::Expr::Compare(compare) = test else {
            return;
        };
        if !matches!(
            (compare.ops.as_ref(), truth),
            ([ast::CmpOp::Eq], true) | ([ast::CmpOp::NotEq], false)
        ) {
            return;
        }
        let (name, text) = match compare.operands.as_ref() {
            [ast::Expr::Attribute(attr), ast::Expr::StringLiteral(text)]
            | [ast::Expr::StringLiteral(text), ast::Expr::Attribute(attr)] => {
                self.narrow_command(attr, text.value.to_str());
                return;
            }
            [ast::Expr::Name(name), ast::Expr::StringLiteral(text)]
            | [ast::Expr::StringLiteral(text), ast::Expr::Name(name)] => (name, text),
            _ => return,
        };
        if let Some(&slot) = self.names.get(name.id.as_str())
            && matches!(self.locals[slot], Type::Str | Type::StrLiteral(_))
        {
            let ty = self.types.string_literal(vec![text.value.to_string()]);
            self.narrowed.insert_mut(slot, ty);
        }
    }
}
