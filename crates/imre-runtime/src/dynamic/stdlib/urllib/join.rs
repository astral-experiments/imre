//! Boxed URL joining preserves Python's empty-input and scheme-registry behavior.
use super::*;
use urlsplit::join::{JoinOption, join_reference};

pub(super) fn join(ctx: &mut Context, base: Id, url: Id, fragments: Option<Id>) -> Result<Id> {
    if !ctx.truth(base)? {
        return Ok(url);
    }
    if !ctx.truth(url)? {
        return Ok(base);
    }
    let text = ctx.string_storage(base).is_some();
    if ctx.truth(url)? && ctx.string_storage(url).is_some() != text {
        return Err(Error::new(
            "TypeError",
            "Cannot mix str and non-str arguments",
        ));
    }
    let (base, url) = if text {
        (base, url)
    } else {
        let ascii = ctx.text("ascii");
        let strict = ctx.text("strict");
        let decode = |ctx: &mut Context, value| {
            if !ctx.truth(value)? {
                return Ok(ctx.text(""));
            }
            let method = ctx.lookup_attr(value, "decode")?;
            ctx.call(method, vec![ascii, strict], Vec::new())
        };
        (decode(ctx, base)?, decode(ctx, url)?)
    };
    // Membership and truth can call Python and grow the heap while joining.
    let base_text = ctx.str_value(base)?.to_owned();
    let url_text = ctx.str_value(url)?.to_owned();
    let joined = join_reference(&base_text, &url_text, |option| {
        let (registry, scheme) = match option {
            JoinOption::AllowFragments => return ctx.optional_truth(fragments, true),
            JoinOption::UsesRelative(scheme) => ("uses_relative", scheme),
            JoinOption::UsesNetloc(scheme) => ("uses_netloc", scheme),
        };
        let module = ctx.module("urllib.parse")?;
        let schemes = ctx.lookup_attr(module, registry)?;
        let scheme = ctx.text(scheme.to_owned());
        ctx.contains(scheme, schemes)
    })?;
    let result = joined.map_or_else(|| url, |value| ctx.text(value));
    if text {
        Ok(result)
    } else {
        let encode = ctx.lookup_attr(result, "encode")?;
        let ascii = ctx.text("ascii");
        let strict = ctx.text("strict");
        ctx.call(encode, vec![ascii, strict], Vec::new())
    }
}
