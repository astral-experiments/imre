//! Dispatch synchronous Expat events to the Python parser that owns its handlers.
use super::NativeParser;
use super::expat::{Handlers, ParseError};
use crate::dynamic::{Context, Error, Id, NONE, Result, Value};
use std::ffi::{CStr, c_char, c_int, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::slice;

#[derive(Default, PartialEq)]
struct Listeners {
    enabled: [bool; 8],
    mute_text: bool,
}

struct ActiveCallback<'a> {
    handle: &'a NativeParser,
    was_parsing: bool,
}

impl Drop for ActiveCallback<'_> {
    fn drop(&mut self) {
        // pyexpat clears its handler flag on every return, including a nested
        // buffer flush. Keep the separate Parse reentry guard until the outer
        // invocation also returns.
        self.handle.callback_active.set(false);
        self.handle.parsing.set(self.was_parsing);
    }
}

struct Feed<'a> {
    ctx: &'a mut Context,
    owner: Id,
    handle: &'a NativeParser,
    installed: Listeners,
    exception: Option<Error>,
}

const HANDLERS: [&str; 8] = [
    "StartElementHandler",
    "EndElementHandler",
    "CharacterDataHandler",
    "StartNamespaceDeclHandler",
    "EndNamespaceDeclHandler",
    "CommentHandler",
    "ProcessingInstructionHandler",
    "DefaultHandlerExpand",
];

impl Feed<'_> {
    fn handler_flags(&mut self) -> Result<Listeners> {
        let mut flags = [false; 8];
        for (flag, name) in flags.iter_mut().zip(HANDLERS) {
            *flag = self.ctx.lookup_attr(self.owner, name)? != NONE;
        }
        Ok(Listeners {
            enabled: flags,
            mute_text: self.flag("_ignore_unhandled_text")?,
        })
    }

    fn handlers(&self) -> Handlers {
        let [start, end, text, ns_start, ns_end, comment, pi, default] = self.installed.enabled;
        Handlers {
            start_element: start.then_some(start_element),
            end_element: end.then_some(end_element),
            text: if self.installed.mute_text {
                Some(ignore_text)
            } else {
                text.then_some(character_data)
            },
            start_ns: ns_start.then_some(start_namespace),
            end_ns: ns_end.then_some(end_namespace),
            comment: comment.then_some(xml_comment),
            pi: pi.then_some(instruction),
            default_expand: default.then_some(default_expand),
        }
    }

    fn call(&mut self, name: &str, args: Vec<Id>) -> Result<()> {
        if invoke(self.ctx, self.owner, self.handle, name, args)? {
            self.refresh_handlers()?;
        }
        Ok(())
    }

    fn refresh_handlers(&mut self) -> Result<()> {
        // A handler can replace itself or start listening for another kind of
        // event even while Expat is processing this chunk.
        let flags = self.handler_flags()?;
        if flags != self.installed {
            self.installed = flags;
            let handlers = self.handlers();
            // SAFETY: every installed callback expects this feed as its user data.
            unsafe { self.handle.parser.set_handlers(&handlers) };
        }
        Ok(())
    }

    fn flag(&mut self, name: &str) -> Result<bool> {
        let value = self.ctx.lookup_attr(self.owner, name)?;
        self.ctx.truth(value)
    }

    fn flush(&mut self) -> Result<()> {
        if deliver_buffer(self.ctx, self.owner, self.handle)? {
            self.refresh_handlers()?;
        }
        Ok(())
    }

    fn buffer_size(&mut self) -> Result<usize> {
        let size = self.ctx.lookup_attr(self.owner, "buffer_size")?;
        self.ctx
            .index_integer(size)?
            .to_i64()
            .and_then(|n| usize::try_from(n).ok())
            .ok_or_else(|| Error::new("ValueError", "invalid character buffer size"))
    }

    fn characters(&mut self, text: &str) -> Result<()> {
        if self.flag("buffer_text")? {
            let mut limit = self.buffer_size()?;
            if text.len() > limit.saturating_sub(self.handle.text.borrow().len()) {
                self.flush()?;
                if !self.installed.enabled[2] {
                    return Ok(());
                }
                limit = self.buffer_size()?;
            }
            if text.len() <= limit && self.flag("buffer_text")? {
                let mut buffer = self.handle.text.borrow_mut();
                buffer
                    .try_reserve(text.len())
                    .map_err(|_| Error::new("MemoryError", ""))?;
                buffer.push_str(text);
                return Ok(());
            }
        }
        let value = self.ctx.text(text);
        self.call("CharacterDataHandler", vec![value])
    }

    // All callers receive these nul-terminated UTF-8 strings from narrow-character Expat.
    fn string(&mut self, ptr: *const c_char) -> Result<Id> {
        if ptr.is_null() {
            return Ok(NONE);
        }
        // SAFETY: the Expat callback provides a live nul-terminated string.
        let text = unsafe { CStr::from_ptr(ptr) }
            .to_str()
            .map_err(|_| invalid_utf8())?;
        Ok(self.ctx.text(text))
    }

    fn name(&mut self, ptr: *const c_char) -> Result<Id> {
        let key = self.string(ptr)?;
        let intern = self.ctx.lookup_attr(self.owner, "intern")?;
        if intern == NONE {
            return Ok(key);
        }
        let storage = self
            .ctx
            .dict_storage(intern)
            .ok_or_else(|| Error::new("TypeError", "intern must be a dictionary"))?;
        let name = self
            .ctx
            .set_dictionary_key(storage, key, key, None, false)?;
        self.ctx.heap.keep(name);
        Ok(name)
    }

    fn attributes(&mut self, attrs: *const *const c_char) -> Result<Id> {
        let limit = if self.flag("specified_attributes")? {
            self.handle.parser.specified_attribute_count()
        } else {
            usize::MAX
        };
        let ordered = self.flag("ordered_attributes")?;
        let dict = if ordered {
            NONE
        } else {
            self.ctx.dict(Vec::new())
        };
        let mut names_and_values = Vec::new();
        let mut index = 0;
        while index < limit {
            // SAFETY: Expat provides pairs followed by a null name sentinel.
            let key = unsafe { *attrs.add(index) };
            if key.is_null() {
                break;
            }
            // SAFETY: a non-null name always has a corresponding value.
            let value = unsafe { *attrs.add(index + 1) };
            let key = self.name(key)?;
            let value = self.string(value)?;
            if ordered {
                names_and_values.extend([key, value]);
            } else {
                // An explicit intern dictionary can map different names to the
                // same key, or return an object that is not hashable.
                self.ctx.set_dictionary_key(dict, key, value, None, true)?;
            }
            index += 2;
        }
        Ok(if ordered {
            self.ctx.alloc(Value::List(names_and_values))
        } else {
            dict
        })
    }
}

fn invoke(
    ctx: &mut Context,
    owner: Id,
    handle: &NativeParser,
    name: &str,
    args: Vec<Id>,
) -> Result<bool> {
    let target = ctx.lookup_attr(owner, name)?;
    if target == NONE {
        return Ok(false);
    }
    ctx.heap.keep(target);
    let _guard = ActiveCallback {
        handle,
        was_parsing: handle.parsing.replace(true),
    };
    handle.callback_active.set(true);
    ctx.call(target, args, vec![])?;
    Ok(true)
}

fn clear_handlers(ctx: &mut Context, owner: Id, error: &Error) {
    for id in error.roots() {
        ctx.heap.keep(id);
    }
    if let Ok(clear) = ctx.lookup_attr(owner, "_clear_handlers") {
        let _ = ctx.call(clear, vec![], vec![]);
    }
}

/// Clear on return from each flush. A setter called inside the handler can
/// flush the same pending text; once that inner flush returns, it is consumed.
fn deliver_buffer(ctx: &mut Context, owner: Id, handle: &NativeParser) -> Result<bool> {
    if handle.text.borrow().is_empty() {
        return Ok(false);
    }
    struct ClearOnReturn<'a>(&'a std::cell::RefCell<String>);
    impl Drop for ClearOnReturn<'_> {
        fn drop(&mut self) {
            self.0.borrow_mut().clear();
        }
    }
    let _clear = ClearOnReturn(&handle.text);
    let text = {
        let buffer = handle.text.borrow();
        let mut text = String::new();
        text.try_reserve_exact(buffer.len())
            .map_err(|_| Error::new("MemoryError", ""))?;
        text.push_str(&buffer);
        text
    };
    let text = ctx.text(text);
    invoke(ctx, owner, handle, "CharacterDataHandler", vec![text])
}

/// A syntax error can leave character data pending until a handler or buffer
/// setting changes. Drain it with the old handler before applying the change.
pub(super) fn flush_before_set(ctx: &mut Context, owner: Id, handle: &NativeParser) -> Result<()> {
    if let Err(error) = deliver_buffer(ctx, owner, handle) {
        handle.parser.stop();
        clear_handlers(ctx, owner, &error);
        return Err(error);
    }
    Ok(())
}

fn invalid_utf8() -> Error {
    Error::new("SystemError", "Expat returned invalid UTF-8")
}

// Expat calls these trampolines only while Parser::parse owns the user pointer.
// A nested Parse on the same parser is rejected before any callback can reenter
// this Feed. A different parser creates its own Feed and reborrows the Context.
fn dispatch(user: *mut c_void, action: impl FnOnce(&mut Feed<'_>) -> Result<()>) {
    // SAFETY: parse installs this live feed exclusively for synchronous callbacks.
    let feed = unsafe { &mut *user.cast::<Feed<'_>>() };
    if feed.exception.is_some() {
        return; // Expat can finish reporting an empty element after XML_StopParser.
    }
    let mark = feed.ctx.heap.temporary_mark();
    let result = catch_unwind(AssertUnwindSafe(|| action(feed)))
        .unwrap_or_else(|_| Err(Error::new("SystemError", "panic in Expat callback")));
    feed.ctx.heap.release_temporaries_since(mark);
    if let Err(error) = result {
        for root in error.roots() {
            feed.ctx.heap.keep(root);
        }
        feed.exception = Some(error);
        feed.handle.parser.stop();
    }
}

fn event(user: *mut c_void, handler: &str, args: impl FnOnce(&mut Feed<'_>) -> Result<Vec<Id>>) {
    dispatch(user, |feed| {
        feed.flush()?;
        let args = args(feed)?;
        feed.call(handler, args)
    });
}

unsafe extern "C" fn start_element(
    user: *mut c_void,
    name: *const c_char,
    attrs: *const *const c_char,
) {
    event(user, "StartElementHandler", |feed| {
        Ok(vec![feed.name(name)?, feed.attributes(attrs)?])
    });
}
unsafe extern "C" fn end_element(user: *mut c_void, name: *const c_char) {
    event(user, "EndElementHandler", |feed| Ok(vec![feed.name(name)?]));
}
unsafe extern "C" fn start_namespace(user: *mut c_void, prefix: *const c_char, uri: *const c_char) {
    event(user, "StartNamespaceDeclHandler", |feed| {
        Ok(vec![feed.name(prefix)?, feed.name(uri)?])
    });
}
unsafe extern "C" fn end_namespace(user: *mut c_void, prefix: *const c_char) {
    event(user, "EndNamespaceDeclHandler", |feed| {
        Ok(vec![feed.name(prefix)?])
    });
}
unsafe extern "C" fn xml_comment(user: *mut c_void, text: *const c_char) {
    event(user, "CommentHandler", |feed| Ok(vec![feed.string(text)?]));
}
unsafe extern "C" fn instruction(user: *mut c_void, target: *const c_char, data: *const c_char) {
    event(user, "ProcessingInstructionHandler", |feed| {
        Ok(vec![feed.name(target)?, feed.string(data)?])
    });
}
unsafe extern "C" fn ignore_text(_: *mut c_void, _: *const c_char, _: c_int) {}

unsafe extern "C" fn character_data(user: *mut c_void, ptr: *const c_char, len: c_int) {
    dispatch(user, |feed| {
        // SAFETY: Expat supplies a live character buffer and its nonnegative byte length.
        let bytes = unsafe { slice::from_raw_parts(ptr.cast::<u8>(), len as usize) };
        feed.characters(std::str::from_utf8(bytes).map_err(|_| invalid_utf8())?)
    });
}
unsafe extern "C" fn default_expand(user: *mut c_void, ptr: *const c_char, len: c_int) {
    event(user, "DefaultHandlerExpand", |feed| {
        // SAFETY: Expat supplies a live character buffer and its nonnegative byte length.
        let bytes = unsafe { slice::from_raw_parts(ptr.cast::<u8>(), len as usize) };
        Ok(vec![feed.ctx.text(
            std::str::from_utf8(bytes).map_err(|_| invalid_utf8())?,
        )])
    });
}

/// Give Expat a callback frame for one synchronous chunk, including its final text flush.
pub(super) fn parse(
    ctx: &mut Context,
    owner: Id,
    handle: &NativeParser,
    bytes: &[u8],
    final_chunk: bool,
) -> Result<std::result::Result<(), ParseError>> {
    ctx.heap.keep(owner);
    let mut feed = Feed {
        ctx,
        owner,
        handle,
        installed: Listeners::default(),
        exception: None,
    };
    feed.installed = feed.handler_flags()?;
    let handlers = feed.handlers();
    let user = (&raw mut feed).cast::<c_void>();
    // SAFETY: feed stays live here until parse has cleared the pointer, and the
    // installed trampolines capture dispatch errors before returning to C.
    let parsed = unsafe { handle.parser.parse(bytes, final_chunk, user, &handlers) };
    if let Some(error) = feed.exception.take() {
        clear_handlers(feed.ctx, owner, &error);
        return Err(error);
    }
    if parsed.is_ok()
        && let Err(error) = feed.flush()
    {
        handle.parser.stop();
        clear_handlers(feed.ctx, owner, &error);
        return Err(error);
    }
    Ok(parsed)
}
