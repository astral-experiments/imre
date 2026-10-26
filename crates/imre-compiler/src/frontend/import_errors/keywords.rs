//! Bind closed keyword groups in the order used by CALL_FUNCTION_EX.
use super::{ast, error};
use std::collections::HashSet;

#[derive(Default)]
pub(super) struct Keywords<'a> {
    pub values: Vec<&'a ast::Expr>,
    pub fields: [Option<usize>; 3],
    pub init_error: Option<String>,
    pub binding_error: Option<String>,
}

/// Each dictionary is built before its keys are merged into the call. Duplicates
/// inside a display replace the value; duplicates across groups stop evaluation.
pub(super) fn bind<'a>(
    source: &str,
    call: &'a ast::ExprCall,
    callable: &str,
) -> Result<Keywords<'a>, String> {
    let mut result = Keywords::default();
    let mut seen = HashSet::new();
    let mut kwargs = call.arguments.keywords.iter().peekable();
    while let Some(kw) = kwargs.next() {
        let mut group = Vec::new();
        if let Some(name) = &kw.arg {
            result.values.push(&kw.value);
            group.push((true, name.to_string(), result.values.len()));
            while let Some(next) = kwargs.next_if(|next| next.arg.is_some()) {
                result.values.push(&next.value);
                group.push((
                    true,
                    next.arg.as_ref().unwrap().to_string(),
                    result.values.len(),
                ));
            }
        } else if let ast::Expr::Dict(display) = &kw.value {
            for item in &display.items {
                let (string, key) = match item.key.as_ref() {
                    Some(ast::Expr::StringLiteral(key)) => (true, key.value.to_string()),
                    Some(ast::Expr::NumberLiteral(number))
                        if let ast::Number::Int(n) = &number.value =>
                    {
                        (false, n.to_string())
                    }
                    _ => {
                        return Err(error(
                            source,
                            kw,
                            "import exception keyword displays require fixed string or integer keys",
                        ));
                    }
                };
                result.values.push(&item.value);
                let index = result.values.len();
                if let Some((_, _, previous)) =
                    group.iter_mut().find(|(s, k, _)| *s == string && *k == key)
                {
                    *previous = index;
                } else {
                    group.push((string, key, index));
                }
            }
        } else {
            let ty = match &kw.value {
                ast::Expr::NumberLiteral(number) if matches!(number.value, ast::Number::Int(_)) => {
                    "int"
                }
                ast::Expr::StringLiteral(_) => "str",
                ast::Expr::BooleanLiteral(_) => "bool",
                ast::Expr::NoneLiteral(_) => "NoneType",
                _ => {
                    return Err(error(
                        source,
                        kw,
                        "expanded import exception keywords require a fixed dictionary display",
                    ));
                }
            };
            result.values.push(&kw.value);
            result.binding_error = Some(format!(
                "{callable}() argument after ** must be a mapping, not {ty}"
            ));
            return Ok(result);
        }
        for (string, key, index) in group {
            if !seen.insert((string, key.clone())) {
                result.binding_error = Some(format!(
                    "{callable}() got multiple values for keyword argument '{key}'"
                ));
                return Ok(result);
            }
            if !string {
                result
                    .init_error
                    .get_or_insert_with(|| "keywords must be strings".to_owned());
                continue;
            }
            match key.as_str() {
                "name" => result.fields[0] = Some(index),
                "path" => result.fields[1] = Some(index),
                "name_from" => result.fields[2] = Some(index),
                _ => {
                    result.init_error.get_or_insert_with(|| {
                        format!("ImportError() got an unexpected keyword argument '{key}'")
                    });
                }
            }
        }
    }
    Ok(result)
}
