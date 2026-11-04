//! Label operations used by the pinned IDNA codec and its incremental factories.
use super::{Context, Error, Id, Result, Value};
use crate::objects::idna::{self, LabelError};

pub(super) fn label(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let [operation, label] = args else {
        return Err(Error::new(
            "TypeError",
            "_imre_idna_label requires two arguments",
        ));
    };
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            "_imre_idna_label takes no keyword arguments",
        ));
    }
    match ctx.str_value(*operation)? {
        "split" => {
            let points = ctx.string_codepoints(*label)?.into_owned();
            let labels = idna::labels(&points)
                .map(|part| ctx.text_codepoints(part.to_vec()))
                .collect();
            Ok(ctx.alloc(Value::List(labels)))
        }
        "ToUnicode" => {
            let bytes = ctx.bytes_like(*label)?;
            match idna::to_unicode(bytes) {
                Ok(text) => Ok(ctx.text(text)),
                Err(err @ LabelError::Invalid { text: Some(_), .. }) => {
                    Err(encode_issue(ctx, *label, err))
                }
                Err(err) => {
                    let codec = if bytes.len() <= 1024
                        && !bytes
                            .get(..4)
                            .is_some_and(|p| p.eq_ignore_ascii_case(b"xn--"))
                    {
                        "ascii"
                    } else {
                        "idna"
                    };
                    let error = err.into_decode(codec, bytes);
                    Err(ctx.decode_error(error))
                }
            }
        }
        op @ ("nameprep" | "ToASCII") => {
            let points = ctx.string_codepoints(*label)?;
            let result = if op == "nameprep" {
                idna::prepare(&points).map(|text| ctx.text(text))
            } else {
                idna::to_ascii(&points).map(|bytes| ctx.alloc(Value::Bytes(bytes)))
            };
            result.map_err(|error| encode_issue(ctx, *label, error))
        }
        _ => Err(Error::new("ValueError", "unknown IDNA label operation")),
    }
}

fn encode_issue(ctx: &mut Context, input: Id, error: LabelError) -> Error {
    let LabelError::Invalid {
        bounds,
        reason,
        text,
    } = error
    else {
        return Error::new(
            "OverflowError",
            "Python int too large to convert to C ssize_t",
        );
    };
    let object = text.map_or(input, |points| ctx.text_codepoints(points));
    let encoding = ctx.text("idna");
    let start = ctx.alloc(Value::Int(crate::Int::from_usize(bounds.start)));
    let end = ctx.alloc(Value::Int(crate::Int::from_usize(bounds.end)));
    let reason = ctx.text(reason);
    ctx.exception(
        "UnicodeEncodeError",
        &[encoding, object, start, end, reason],
    )
}
