//! Declared native exception members backed by the boxed exception instance.

use super::{Context, Error, Id, NONE, Result, Value};
use imre_ast::exceptions::Kind;

pub(super) fn names(kind: Kind) -> &'static [&'static str] {
    match kind {
        Kind::BaseException => &["__suppress_context__"],
        Kind::AttributeError => &["name", "obj"],
        Kind::UnicodeEncodeError | Kind::UnicodeDecodeError => {
            &["encoding", "object", "start", "end", "reason"]
        }
        Kind::OSError => &["errno", "strerror", "filename", "filename2"],
        Kind::ImportError => &["msg", "name", "path", "name_from"],
        Kind::NameError => &["name"],
        Kind::StopIteration => &["value"],
        Kind::SystemExit => &["code"],
        Kind::SyntaxError => super::source_code::SYNTAX_MEMBERS,
        _ => &[],
    }
}

pub(super) fn read(ctx: &mut Context, kind: Kind, field: &str, object: Id) -> Result<Id> {
    let Value::Raised(error) = ctx.heap.get(object) else {
        unreachable!()
    };
    match kind {
        Kind::BaseException => Ok(super::builtins::boolean(error.suppresses_context())),
        Kind::AttributeError => Ok(NONE),
        Kind::UnicodeEncodeError | Kind::UnicodeDecodeError => {
            if let Some(state) = &error.decode {
                return Ok(match field {
                    "encoding" => state.encoding,
                    "object" => state.object,
                    "start" => ctx.int(state.start),
                    "end" => ctx.int(state.end),
                    "reason" => state.reason,
                    _ => unreachable!(),
                });
            }
            let Some([encoding, input, start, end, reason]) = ctx.exception_arguments(error) else {
                return Err(ctx.missing_attribute(object, field));
            };
            Ok(match field {
                "encoding" => *encoding,
                "object" => *input,
                "start" => *start,
                "end" => *end,
                "reason" => *reason,
                _ => unreachable!(),
            })
        }
        Kind::NameError => Ok(error
            .trace
            .as_ref()
            .and_then(|t| t.name_error_name)
            .unwrap_or(NONE)),
        Kind::StopIteration => Ok(error.stop_value()),
        Kind::SystemExit => Ok(error.exit_code),
        Kind::ImportError if field == "msg" => {
            Ok(ctx.import_error_message(object, None).unwrap_or(NONE))
        }
        Kind::ImportError => Ok(import_names(ctx, object)[import_index(field)].unwrap_or(NONE)),
        Kind::OSError => Ok(ctx
            .os_attribute(object, field)?
            .expect("declared OSError field")),
        Kind::SyntaxError => Ok(ctx.syntax_members(error).map_or(NONE, |fields| {
            fields[super::source_code::syntax_member(field).unwrap()]
        })),
        _ => unreachable!("undeclared exception member"),
    }
}

pub(super) fn write(
    ctx: &mut Context,
    kind: Kind,
    field: &str,
    object: Id,
    value: Option<Id>,
) -> Result<()> {
    match kind {
        Kind::BaseException => {
            let value = value
                .ok_or_else(|| Error::new("TypeError", "can't delete numeric/char attribute"))?;
            let Value::Bool(flag) = *ctx.heap.get(value) else {
                return Err(Error::new("TypeError", "attribute value type must be bool"));
            };
            let Value::Raised(error) = ctx.heap.get_mut(object) else {
                unreachable!()
            };
            error.tracing().suppress_context = Some(flag);
        }
        Kind::UnicodeEncodeError | Kind::UnicodeDecodeError => {
            let offset = if matches!(field, "start" | "end") {
                let index = value.ok_or_else(|| {
                    Error::new("TypeError", "can't delete numeric/char attribute")
                })?;
                Some(
                    ctx.index_integer(index)?
                        .to_i64()
                        .and_then(|i| isize::try_from(i).ok())
                        .ok_or_else(|| {
                            Error::new(
                                "OverflowError",
                                "Python int too large to convert to C ssize_t",
                            )
                        })? as i64,
                )
            } else {
                None
            };
            let Value::Raised(error) = ctx.heap.get_mut(object) else {
                unreachable!()
            };
            let Some(state) = &mut error.decode else {
                return Err(ctx.missing_attribute(object, field));
            };
            let old = match field {
                "encoding" => Some(std::mem::replace(
                    &mut state.encoding,
                    value.unwrap_or(NONE),
                )),
                "object" => Some(std::mem::replace(&mut state.object, value.unwrap_or(NONE))),
                "reason" => Some(std::mem::replace(&mut state.reason, value.unwrap_or(NONE))),
                "start" => {
                    state.start = offset.unwrap();
                    None
                }
                "end" => {
                    state.end = offset.unwrap();
                    None
                }
                _ => unreachable!(),
            };
            if let Some(id) = old
                && Some(id) != value
            {
                ctx.heap.weak_edge_removed(id);
            }
        }
        Kind::AttributeError => return Err(ctx.missing_attribute(object, field)),
        Kind::NameError => {
            let Value::Raised(error) = ctx.heap.get_mut(object) else {
                unreachable!()
            };
            if value.is_some() || error.trace.is_some() {
                let old = std::mem::replace(&mut error.tracing().name_error_name, value);
                if let Some(old) = old
                    && Some(old) != value
                {
                    ctx.heap.weak_edge_removed(old);
                }
            }
        }
        Kind::StopIteration => {
            let Value::Raised(error) = ctx.heap.get_mut(object) else {
                unreachable!()
            };
            error.set_stop_value(value.unwrap_or(NONE));
        }
        Kind::SystemExit => {
            ctx.replace_exit_code(object, value.unwrap_or(NONE));
        }
        Kind::ImportError if field == "msg" => {
            ctx.import_error_message(object, Some(value.unwrap_or(NONE)));
        }
        Kind::ImportError => {
            let Value::Raised(error) = ctx.heap.get_mut(object) else {
                unreachable!()
            };
            let slot = &mut error.tracing().import_names[import_index(field)];
            let old = std::mem::replace(slot, value);
            if let Some(old) = old
                && Some(old) != value
            {
                ctx.heap.weak_edge_removed(old);
            }
        }
        Kind::OSError => {
            ctx.update_os_field(object, field, value)?;
        }
        Kind::SyntaxError => {
            if !ctx.update_syntax_member(object, field, value.unwrap_or(NONE)) {
                return Err(Error::new("AttributeError", field));
            }
        }
        _ => unreachable!("undeclared exception member"),
    }
    Ok(())
}

fn import_index(field: &str) -> usize {
    super::builtins::IMPORT_KEYWORDS
        .iter()
        .position(|&name| name == field)
        .unwrap()
}

/// An empty slot and an explicit Python None have distinct pickle states.
pub(super) fn import_names(ctx: &Context, object: Id) -> &[Option<Id>; 3] {
    let Value::Raised(error) = ctx.heap.get(object) else {
        unreachable!()
    };
    error
        .trace
        .as_ref()
        .map_or(&[None; 3], |trace| &trace.import_names)
}
