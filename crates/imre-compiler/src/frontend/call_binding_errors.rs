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

fn quoted_names(names: &[&str]) -> String {
    let quoted: Vec<_> = names.iter().map(|s| format!("'{s}'")).collect();
    match quoted.as_slice() {
        [name] => name.clone(),
        [a, b] => format!("{a} and {b}"),
        _ => {
            let (last, rest) = quoted.split_last().unwrap();
            format!("{}, and {last}", rest.join(", "))
        }
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
    let params = &function.parameters;
    let positional: Vec<_> = params.posonlyargs.iter().chain(&params.args).collect();
    let mut given: Vec<_> = positional
        .iter()
        .enumerate()
        .map(|(i, _)| i < supplied)
        .collect();
    let mut named = vec![false; params.kwonlyargs.len()];
    let name = owner.map_or_else(
        || function.name.to_string(),
        |c| format!("{c}.{}", function.name),
    );
    for keyword in keywords {
        let key = keyword.arg.as_deref()?;
        if let Some(i) = positional
            .iter()
            .position(|p| p.parameter.name.as_str() == key)
            && i >= params.posonlyargs.len()
        {
            if given[i] {
                return Some(format!("{name}() got multiple values for argument '{key}'"));
            }
            given[i] = true;
        } else if let Some(i) = params
            .kwonlyargs
            .iter()
            .position(|p| p.parameter.name.as_str() == key)
        {
            named[i] = true;
        } else if params.kwarg.is_none() {
            return None;
        }
    }
    if supplied > positional.len() && params.vararg.is_none() {
        let max = positional.len();
        let min = positional.iter().filter(|p| p.default.is_none()).count();
        let allowed = if min == max {
            max.to_string()
        } else {
            format!("from {min} to {max}")
        };
        let unit = if max == 1 && min == max {
            "argument"
        } else {
            "arguments"
        };
        let kwonly = named.iter().filter(|v| **v).count();
        let actual = if kwonly == 0 {
            supplied.to_string()
        } else {
            format!(
                "{supplied} positional argument{} (and {kwonly} keyword-only argument{})",
                if supplied == 1 { "" } else { "s" },
                if kwonly == 1 { "" } else { "s" }
            )
        };
        let verb = if supplied == 1 && kwonly == 0 {
            "was"
        } else {
            "were"
        };
        return Some(format!(
            "{name}() takes {allowed} positional {unit} but {actual} {verb} given"
        ));
    }
    for (kind, missing) in [
        (
            "positional",
            positional
                .iter()
                .zip(&given)
                .filter(|(p, supplied)| p.default.is_none() && !**supplied)
                .map(|(p, _)| p.parameter.name.as_str())
                .collect::<Vec<_>>(),
        ),
        (
            "keyword-only",
            params
                .kwonlyargs
                .iter()
                .zip(&named)
                .filter(|(p, supplied)| p.default.is_none() && !**supplied)
                .map(|(p, _)| p.parameter.name.as_str())
                .collect::<Vec<_>>(),
        ),
    ] {
        if !missing.is_empty() {
            let n = missing.len();
            return Some(format!(
                "{name}() missing {n} required {kind} argument{}: {}",
                if n == 1 { "" } else { "s" },
                quoted_names(&missing)
            ));
        }
    }
    None
}
