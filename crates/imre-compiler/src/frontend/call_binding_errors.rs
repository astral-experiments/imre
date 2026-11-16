//! Python rejects the arguments of a known function before entering its body.
//! The diagnostic uses its declared signature, including a bound receiver.

use super::*;

impl Checker<'_> {
    /// A static binding error is valid for a bound base receiver only if a
    /// concrete subclass cannot select an override with a different signature.
    pub(super) fn assertion_callback_arity(
        &self,
        class: usize,
        method: &str,
        bound: bool,
        supplied: usize,
        keywords: &[ast::Keyword],
    ) -> Option<String> {
        use crate::classes::MethodKind;
        let classes = &self.classes.declarations;
        let record = &classes.definitions[class];
        let index = record
            .methods
            .iter()
            .position(|m| m.name.as_str() == method)?;
        let origin = record.method_origins[index];
        if bound
            && classes.definitions.iter().any(|child| {
                child.bases.contains(&class)
                    && child.methods.iter().enumerate().any(|(i, m)| {
                        m.name.as_str() == method && child.method_origins[i] != origin
                    })
            })
        {
            return None;
        }
        let extra = match record.method_kinds[index] {
            MethodKind::Instance => usize::from(bound),
            MethodKind::Class => 1,
            MethodKind::Static => 0,
            MethodKind::Property | MethodKind::PropertySetter => return None,
        };
        let owner = &classes.definitions[*classes.ids.get(&origin)?].name;
        arity_error(
            &record.methods[index],
            Some(owner),
            supplied + extra,
            keywords,
        )
    }
}

/// Return a proven arity failure. Unpacking and unrecognized keywords still
/// go through the normal call binder instead of standing in for a Python error.
pub(super) fn arity_error(
    function: &ast::StmtFunctionDef,
    owner: Option<&str>,
    supplied: usize,
    keywords: &[ast::Keyword],
) -> Option<String> {
    // A decorator can replace the callable and its signature. Plain method
    // descriptors retain the declared arguments; other decorators use normal lowering.
    if function.decorator_list.iter().any(|d| {
        owner.is_none()
            || !matches!(&d.expression, ast::Expr::Name(n)
            if matches!(n.id.as_str(), "staticmethod" | "classmethod"))
    }) {
        return None;
    }
    let name = owner.map_or_else(
        || function.name.to_string(),
        |c| format!("{c}.{}", function.name),
    );
    // A duplicate before an unpacked keyword is definite. Missing or excess
    // arguments still go through normal checking when a keyword is unknown.
    let explicit = keywords
        .iter()
        .position(|kw| kw.arg.is_none())
        .unwrap_or(keywords.len());
    let keys = keywords[..explicit]
        .iter()
        .map(|kw| kw.arg.as_deref().unwrap());
    match bind_source(function, &name, supplied, keys) {
        Err(imre_ast::source_arguments::Error::Duplicate(message)) => Some(message),
        Err(imre_ast::source_arguments::Error::Arity(message)) if explicit == keywords.len() => {
            Some(message)
        }
        _ => None,
    }
}

/// Bind named operands to the selected source declaration using the Python frame binder.
pub(crate) fn bind_source<'a>(
    function: &ast::StmtFunctionDef,
    name: &str,
    supplied: usize,
    keys: impl ExactSizeIterator<Item = &'a str> + Clone,
) -> std::result::Result<imre_ast::source_arguments::Binding, imre_ast::source_arguments::Error> {
    let params = &function.parameters;
    let names: Vec<_> = params
        .iter_non_variadic_params()
        .map(|p| p.name().as_str())
        .collect();
    let (positional, keyword_only) = names.split_at(names.len() - params.kwonlyargs.len());
    let shape = imre_ast::source_arguments::Shape {
        positional,
        positional_only: params.posonlyargs.len(),
        keyword_only,
        varargs: params.vararg.is_some(),
        kwargs: params.kwarg.is_some(),
    };
    let defaults = params
        .posonlyargs
        .iter()
        .chain(&params.args)
        .filter(|p| p.default.is_some())
        .count();
    shape.bind(
        name,
        supplied,
        keys,
        defaults,
        |slot, _| params.kwonlyargs[slot].default.is_some(),
        |_, _| None,
    )
}
