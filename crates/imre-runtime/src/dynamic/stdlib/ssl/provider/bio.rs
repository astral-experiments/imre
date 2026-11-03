//! OpenSSL's in-memory byte transport, owned by one Python MemoryBIO.
use std::cell::RefCell;
use std::ffi::c_void;
use std::ptr::{self, NonNull};
use std::rc::Rc;

use super::Api;
use crate::dynamic::{
    Context, builtins, stdlib,
    value::{Error, Id, NONE, Result, Value},
};

pub(in crate::dynamic) const CLASS: &str = "_ssl.MemoryBIO";

// OpenSSL bio.h: BIO_set_mem_eof_return, BIO_set_retry_read and BIO_clear_retry_flags.
const SET_MEM_EOF_RETURN: i32 = 130;
const RETRY_READ: i32 = 0x09;
const RETRY_FLAGS: i32 = 0x0f;

pub(in crate::dynamic) struct Bio {
    api: &'static Api,
    handle: NonNull<c_void>,
    eof_written: bool,
}

impl Drop for Bio {
    fn drop(&mut self) {
        // SAFETY: BIO_new transferred this BIO's only native owner to us.
        unsafe { (self.api.BIO_free)(self.handle.as_ptr()) };
    }
}

impl Bio {
    fn new() -> Result<Self> {
        let api = super::api()
            .ok_or_else(|| Error::new("ImportError", "system TLS provider unavailable"))?;
        // SAFETY: the memory method and its entry points are from the same live library.
        let handle = NonNull::new(unsafe { (api.BIO_new)((api.BIO_s_mem)()) })
            .ok_or_else(|| Error::new("MemoryError", "failed to allocate BIO"))?;
        // SAFETY: this is a fresh memory BIO. Like CPython, an empty BIO retries until marked EOF.
        unsafe {
            (api.BIO_set_flags)(handle.as_ptr(), RETRY_READ);
            (api.BIO_ctrl)(handle.as_ptr(), SET_MEM_EOF_RETURN, -1, ptr::null_mut());
        }
        Ok(Self {
            api,
            handle,
            eof_written: false,
        })
    }

    fn pending(&self) -> usize {
        // SAFETY: the BIO is live for the duration of the call.
        unsafe { (self.api.BIO_ctrl_pending)(self.handle.as_ptr()) }
    }

    fn mark_eof(&mut self) {
        // SAFETY: both operations only change the live memory BIO's retry behavior.
        unsafe {
            (self.api.BIO_clear_flags)(self.handle.as_ptr(), RETRY_FLAGS);
            (self.api.BIO_ctrl)(self.handle.as_ptr(), SET_MEM_EOF_RETURN, 0, ptr::null_mut());
        }
        self.eof_written = true;
    }
}

pub(in crate::dynamic) fn install(ctx: &mut Context, module: Id, scope: Id) -> Result<()> {
    let fields = ctx.scope("_ssl", Some(scope));
    let module_name = ctx.text("_ssl");
    let qualname = ctx.text("MemoryBIO");
    ctx.bind(fields, "__module__", module_name);
    ctx.bind(fields, "__qualname__", qualname);
    ctx.bind(fields, "__doc__", NONE);
    let object = ctx.builtin("object");
    let owner = ctx.allocate_class("MemoryBIO".into(), fields, vec![object])?;
    let Value::Class { heap_type, .. } = ctx.heap.get_mut(owner) else {
        unreachable!()
    };
    *heap_type = Some(CLASS);
    let Value::Scope(frame) = ctx.heap.get_mut(fields) else {
        unreachable!()
    };
    let namespace = frame.class_namespace.as_mut().unwrap();
    namespace.slot_names = Some([].into());
    namespace.immutable_type = true;
    namespace.defining_module = Some(module);
    for name in ["__new__", "read", "write", "write_eof", "pending", "eof"] {
        let method = ctx.alloc(Value::HeapTypeMethod { owner, name });
        let member = match name {
            "__new__" => ctx.alloc(Value::Method {
                callable: method,
                receiver: owner,
                explicit: false,
            }),
            "pending" | "eof" => ctx.alloc(Value::GetSetDescriptor {
                owner: CLASS,
                name,
                get: method,
                set: None,
            }),
            _ => method,
        };
        ctx.bind(fields, name, member);
    }
    ctx.bind(scope, "MemoryBIO", owner);
    Ok(())
}

fn ssl_error(ctx: &Context, owner: Id, message: &str) -> Error {
    let Value::Class { scope, .. } = ctx.heap.get(owner) else {
        unreachable!()
    };
    let module = ctx
        .scope_data(*scope)
        .class_namespace
        .as_ref()
        .and_then(|namespace| namespace.defining_module)
        .expect("MemoryBIO retains its defining extension module");
    super::objects::ssl_error(ctx, module, message)
}

pub(in crate::dynamic) fn construct(
    ctx: &mut Context,
    owner: Id,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    if !args.is_empty() {
        return Err(Error::new(
            "TypeError",
            "MemoryBIO() takes no positional arguments",
        ));
    }
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            "MemoryBIO() takes no keyword arguments",
        ));
    }
    let state = ctx.alloc(Value::MemoryBio(Rc::new(RefCell::new(Bio::new()?))));
    let instance = ctx.instance(owner, None);
    let Value::Instance { scalar, .. } = ctx.heap.get_mut(instance) else {
        unreachable!()
    };
    *scalar = Some(state);
    Ok(instance)
}

pub(in crate::dynamic) fn call(
    ctx: &mut Context,
    owner: Id,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let Some((&receiver, rest)) = args.split_first() else {
        return Err(Error::new(
            "TypeError",
            format!("unbound method MemoryBIO.{name}() needs an argument"),
        ));
    };
    if name == "__new__" {
        let (&ty, rest) = rest.split_first().ok_or_else(|| {
            Error::new(
                "TypeError",
                "_ssl.MemoryBIO.__new__(): not enough arguments",
            )
        })?;
        if ty != owner {
            return Err(ctx.invalid_new_subtype(CLASS, ty));
        }
        return construct(ctx, owner, rest, kwargs);
    }
    ctx.check_heap_method_receiver(owner, name, receiver)?;
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            format!("MemoryBIO.{name}() takes no keyword arguments"),
        ));
    }
    let Value::MemoryBio(state) = ctx.heap.get(ctx.scalar_value(receiver)) else {
        return Err(Error::new("TypeError", "expected an initialized MemoryBIO"));
    };
    let state = Rc::clone(state);
    match name {
        "pending" => Ok(ctx.int(state.borrow().pending() as i64)),
        "eof" => {
            let bio = state.borrow();
            Ok(builtins::boolean(bio.eof_written && bio.pending() == 0))
        }
        "write_eof" => {
            if !rest.is_empty() {
                return Err(Error::new(
                    "TypeError",
                    format!(
                        "MemoryBIO.write_eof() takes no arguments ({} given)",
                        rest.len()
                    ),
                ));
            }
            state.borrow_mut().mark_eof();
            Ok(NONE)
        }
        "read" => {
            let len = match rest {
                [] => -1,
                [len] => stdlib::index_c_int(ctx, *len)?,
                _ => {
                    return Err(Error::new(
                        "TypeError",
                        format!("read expected at most 1 argument, got {}", rest.len()),
                    ));
                }
            };
            // __index__ can call into this same BIO. Inspect its state only after conversion.
            let bio = state.borrow_mut();
            let limit = if len < 0 { i32::MAX } else { len };
            let size = bio.pending().min(limit as usize);
            let mut data = Vec::new();
            data.try_reserve_exact(size)
                .map_err(|_| Error::new("MemoryError", ""))?;
            data.resize(size, 0);
            if size != 0 {
                // SAFETY: data owns size writable bytes, and size fits a C int.
                let read = unsafe {
                    (bio.api.BIO_read)(bio.handle.as_ptr(), data.as_mut_ptr().cast(), size as i32)
                };
                if read < 0 {
                    return Err(ssl_error(ctx, owner, "BIO_read failed"));
                }
                data.truncate(read as usize);
            }
            Ok(ctx.alloc(Value::Bytes(data)))
        }
        "write" => {
            let input = builtins::one_positional_arg("MemoryBIO.write", rest, kwargs)?;
            ctx.check_buffer_exporter(input)?;
            let data = ctx.native_buffer_bytes(input)?;
            let len = i32::try_from(data.len())
                .map_err(|_| Error::new("OverflowError", "string longer than 2147483647 bytes"))?;
            let bio = state.borrow_mut();
            if bio.eof_written {
                return Err(ssl_error(ctx, owner, "cannot write() after write_eof()"));
            }
            // SAFETY: data is a contiguous buffer rooted by the call, and BIO_write copies it.
            let written =
                unsafe { (bio.api.BIO_write)(bio.handle.as_ptr(), data.as_ptr().cast(), len) };
            if written < 0 {
                return Err(ssl_error(ctx, owner, "BIO_write failed"));
            }
            Ok(ctx.int(i64::from(written)))
        }
        _ => unreachable!("registered MemoryBIO method"),
    }
}
