//! Owned libexpat handles used by the Python pyexpat module.
use std::{
    cell::{Cell, RefCell},
    ffi::CString,
    rc::Rc,
};

use super::{Context, Error, FALSE, Id, NONE, Result, TRUE, Value, os};

mod callbacks;
mod expat;

/// The Python instance owns this handle and its handler fields. Keeping boxed
/// references out of the handle lets GC trace cycles with a target or callback.
pub(in crate::dynamic) struct NativeParser {
    parser: expat::Parser,
    parsing: Cell<bool>,
    text: RefCell<String>,
    callback_active: Cell<bool>,
    defer_reparse: Cell<bool>,
}

fn handle(ctx: &Context, value: Id) -> Result<Rc<NativeParser>> {
    match ctx.heap.get(value) {
        Value::ExpatParser(parser) => Ok(Rc::clone(parser)),
        _ => Err(Error::new("TypeError", "expected an Expat parser handle")),
    }
}

fn library_missing() -> Error {
    Error::new("ImportError", "libexpat is unavailable")
}

fn create(ctx: &mut Context, encoding: Id, separator: Id) -> Result<Id> {
    let encoding = if encoding == NONE {
        None
    } else {
        Some(
            CString::new(ctx.str_value(encoding)?)
                .map_err(|_| Error::new("ValueError", "embedded null character"))?,
        )
    };
    let separator = if separator == NONE {
        None
    } else {
        match ctx.str_value(separator)?.as_bytes() {
            [] => Some(0),
            [0] => return Err(Error::new("ValueError", "embedded null character")),
            [byte] => Some(*byte as std::ffi::c_char),
            _ => {
                return Err(Error::new(
                    "ValueError",
                    "namespace_separator must be at most one character, omitted, or None",
                ));
            }
        }
    };
    let parser =
        expat::Parser::new(encoding.as_deref(), separator).map_err(|error| match error {
            expat::CreateError::Unavailable => library_missing(),
            expat::CreateError::NoMemory => Error::new("MemoryError", ""),
        })?;
    Ok(ctx.alloc(Value::ExpatParser(Rc::new(NativeParser {
        defer_reparse: Cell::new(parser.supports_reparse_deferral()),
        parser,
        parsing: Cell::new(false),
        text: RefCell::default(),
        callback_active: Cell::new(false),
    }))))
}

fn parse(
    ctx: &mut Context,
    handle: Rc<NativeParser>,
    owner: Id,
    data: Id,
    is_final: Id,
) -> Result<Id> {
    // XML_Parse invokes Python synchronously. A callback may collect objects
    // or enter a different parser, but cannot feed this one again.
    if handle.parsing.replace(true) {
        return Err(Error::new(
            "RuntimeError",
            "cannot re-enter an active Expat parser",
        ));
    }
    let result = ctx.with_buffer_export(data, |ctx| {
        let is_final = ctx.truth(is_final)?;
        let bytes = if let Some(text) = ctx.string_storage(data) {
            let bytes = match ctx.heap.get(text) {
                Value::Str(s) => s.as_bytes().to_vec(),
                _ => ctx.encode_utf8_codepoints(text, false, "strict")?,
            };
            handle.parser.set_utf8();
            bytes
        } else {
            // The heap or its backing allocation may move during a Python callback.
            ctx.native_buffer_bytes(data)?.to_vec()
        };
        match callbacks::parse(ctx, owner, &handle, &bytes, is_final)? {
            Ok(()) => Ok(NONE),
            Err(error) => {
                let code = ctx.int(i64::from(error.code));
                let message = ctx.text(expat::error_message(error.code).unwrap_or("unknown error"));
                let line = ctx.alloc(Value::Int(crate::Int::new(error.line.into())));
                let column = ctx.alloc(Value::Int(crate::Int::new(error.column.into())));
                Ok(ctx.alloc(Value::Tuple(vec![code, message, line, column])))
            }
        }
    });
    handle.parsing.set(false);
    result
}

pub(super) fn call(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    match (name, args, kwargs) {
        ("create", [encoding, separator], []) => create(ctx, *encoding, *separator),
        ("parse", [resource, owner, data, final_chunk], []) => {
            parse(ctx, handle(ctx, *resource)?, *owner, *data, *final_chunk)
        }
        ("flush_buffer", [resource, owner], []) => {
            let handle = handle(ctx, *resource)?;
            callbacks::flush_before_set(ctx, *owner, &handle)?;
            Ok(NONE)
        }
        ("position", [resource], []) => {
            let handle = handle(ctx, *resource)?;
            let line = ctx.alloc(Value::Int(crate::Int::new(handle.parser.line().into())));
            let column = ctx.alloc(Value::Int(crate::Int::new(handle.parser.column().into())));
            let index = ctx.int(handle.parser.byte_index());
            Ok(ctx.alloc(Value::Tuple(vec![line, column, index])))
        }
        ("error_code", [resource], []) => {
            let handle = handle(ctx, *resource)?;
            Ok(ctx.int(i64::from(handle.parser.error_code())))
        }
        ("version_info", [], []) => {
            let (major, minor, micro) = expat::version().ok_or_else(library_missing)?;
            let parts = [major, minor, micro].map(|v| ctx.int(i64::from(v)));
            Ok(ctx.alloc(Value::Tuple(parts.to_vec())))
        }
        ("error_string", [code], []) => {
            let code = os::c_int(ctx, *code)?;
            Ok(expat::error_message(code).map_or(NONE, |msg| ctx.text(msg)))
        }
        ("in_callback", [resource], []) => Ok(if handle(ctx, *resource)?.callback_active.get() {
            TRUE
        } else {
            FALSE
        }),
        ("get_reparse_deferral", [resource], []) => {
            Ok(if handle(ctx, *resource)?.defer_reparse.get() {
                TRUE
            } else {
                FALSE
            })
        }
        ("set_reparse_deferral", [resource, enabled], []) => {
            let handle = handle(ctx, *resource)?;
            let enabled = ctx.truth(*enabled)?;
            if handle.parser.set_reparse_deferral(enabled) {
                handle.defer_reparse.set(enabled);
            }
            Ok(NONE)
        }
        _ => Err(Error::new(
            "TypeError",
            format!("invalid arguments to _imre_pyexpat.{name}()"),
        )),
    }
}
