use super::{Context, Error, Id, Result, Value};
use crate::dynamic::builtins::boolean;
use crate::objects::fnmatch::{byte_spelling, compile, mixed_message, translate, translated_parts};

fn pattern(ctx: &Context, pat: Id) -> Result<(impl Fn(&str) -> bool + use<>, bool)> {
    match ctx.heap.get(ctx.scalar_value(pat)) {
        Value::Str(_) | Value::SurrogateStr(_) => Ok((compile(ctx.str_value(pat)?), false)),
        Value::Bytes(value) => Ok((compile(&byte_spelling(value)?), true)),
        Value::ByteArray(_)
        | Value::List(_)
        | Value::Dict(_)
        | Value::Set { frozen: false, .. } => Err(Error::new(
            "TypeError",
            format!("unhashable type: '{}'", ctx.type_name(pat)),
        )),
        _ => Err(Error::new(
            "TypeError",
            format!("object of type '{}' has no len()", ctx.type_name(pat)),
        )),
    }
}

fn matches(ctx: &Context, name: Id, bytes: bool, matcher: &impl Fn(&str) -> bool) -> Result<bool> {
    match ctx.heap.get(ctx.scalar_value(name)) {
        Value::Str(_) | Value::SurrogateStr(_) if !bytes => Ok(matcher(ctx.str_value(name)?)),
        Value::Bytes(value) | Value::ByteArray(value) if bytes => {
            Ok(matcher(&byte_spelling(value)?))
        }
        Value::Str(_) | Value::SurrogateStr(_) | Value::Bytes(_) | Value::ByteArray(_) => {
            Err(Error::new("TypeError", mixed_message(bytes)))
        }
        _ => Err(Error::new(
            "TypeError",
            format!(
                "expected string or bytes-like object, got '{}'",
                ctx.type_name(name)
            ),
        )),
    }
}

pub(super) fn call(
    ctx: &mut Context,
    op: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    match op {
        "fnmatch" | "fnmatchcase" => {
            let [mut name, mut pat] = super::required_arguments(op, ["name", "pat"], args, kwargs)?;
            if op == "fnmatch" {
                // POSIX normcase resolves PathLike values in name-then-pattern order.
                name = super::posixpath::fspath(ctx, name)?;
                pat = super::posixpath::fspath(ctx, pat)?;
            }
            let (matcher, bytes) = pattern(ctx, pat)?;
            Ok(boolean(matches(ctx, name, bytes, &matcher)?))
        }
        "filter" | "filterfalse" => {
            let [names, pat] = super::required_arguments(op, ["names", "pat"], args, kwargs)?;
            let pat = super::posixpath::fspath(ctx, pat)?;
            let (matcher, bytes) = pattern(ctx, pat)?;
            let iter = ctx.iterator(names)?;
            let result = ctx.alloc(Value::List(Vec::new()));
            while let Some(name) = ctx.next_item(iter)? {
                if matches(ctx, name, bytes, &matcher)? == (op == "filter") {
                    let Value::List(items) = ctx.heap.get_mut(result) else {
                        unreachable!()
                    };
                    items.push(name);
                }
            }
            Ok(result)
        }
        "translate" => {
            let [pat] = super::required_arguments(op, ["pat"], args, kwargs)?;
            let regex = match ctx.heap.get(ctx.scalar_value(pat)) {
                Value::Str(_) | Value::SurrogateStr(_) => translate(ctx.str_value(pat)?),
                Value::Bytes(bytes) if bytes.is_empty() => translate(""),
                Value::Bytes(_) => {
                    return Err(Error::new(
                        "TypeError",
                        "decoding to str: need a bytes-like object, int found",
                    ));
                }
                _ => {
                    return Err(Error::new(
                        "TypeError",
                        format!("object of type '{}' has no len()", ctx.type_name(pat)),
                    ));
                }
            };
            Ok(ctx.text(regex))
        }
        "_translate" => {
            let [pat, star, question] =
                super::required_arguments(op, ["pat", "star", "question_mark"], args, kwargs)?;
            let (parts, stars) = translated_parts(ctx.str_value(pat)?, "*", "?");
            let parts = parts
                .into_iter()
                .map(|part| match part.as_str() {
                    "*" => star,
                    "?" => question,
                    _ => ctx.text(part),
                })
                .collect();
            let parts = ctx.alloc(Value::List(parts));
            let indices = stars.into_iter().map(|i| ctx.int(i as i64)).collect();
            let indices = ctx.alloc(Value::List(indices));
            Ok(ctx.alloc(Value::Tuple(vec![parts, indices])))
        }
        _ => unreachable!("registered fnmatch function"),
    }
}
