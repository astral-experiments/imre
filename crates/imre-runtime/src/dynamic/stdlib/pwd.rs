//! Account enumeration and lookups through the host's password database.
use std::{
    ffi::{CStr, CString},
    mem::MaybeUninit,
    ptr,
    sync::Mutex,
};

use super::{Context, Error, Id, NONE, Result, Value, fixed_structseq, os};

pub(in crate::dynamic) fn install(ctx: &mut Context, scope: Id) {
    for name in ["getpwall", "getpwnam", "getpwuid", "struct_passwd"] {
        let value = ctx.builtin(&format!("pwd.{name}"));
        ctx.bind(scope, name, value);
    }
}

struct Entry {
    strings: [Option<Vec<u8>>; 5],
    uid: libc::uid_t,
    gid: libc::gid_t,
}

impl Entry {
    /// Copy a passwd result before its storage can be reused.
    ///
    /// # Safety
    /// Every non-null string field must point to a live NUL-terminated NSS string.
    unsafe fn snapshot(p: &libc::passwd) -> Self {
        let strings = [p.pw_name, p.pw_passwd, p.pw_gecos, p.pw_dir, p.pw_shell].map(|s| {
            (!s.is_null()).then(|| {
                // SAFETY: the null check and snapshot contract guarantee a live C string.
                unsafe { CStr::from_ptr(s) }.to_bytes().to_vec()
            })
        });
        Self {
            strings,
            uid: p.pw_uid,
            gid: p.pw_gid,
        }
    }

    fn record(self, ctx: &mut Context) -> Result<Id> {
        let codec = crate::objects::Codec::lookup(crate::sys::filesystem_encoding()?.as_str())?;
        let mut values = vec![NONE; 7];
        for (index, bytes) in [0, 1, 4, 5, 6].into_iter().zip(self.strings) {
            if let Some(bytes) = bytes {
                values[index] = ctx.text_codepoints(codec.decode_filesystem(&bytes)?);
            }
        }
        values[2] = ctx.int(python_id(self.uid));
        values[3] = ctx.int(python_id(self.gid));
        Ok(fixed_structseq::record(
            ctx,
            fixed_structseq::PASSWD,
            values,
        ))
    }
}

fn python_id(id: u32) -> i64 {
    if id == u32::MAX { -1 } else { i64::from(id) }
}

fn entries() -> Vec<Entry> {
    // All contexts in the process share one libc enumeration cursor. Finish the snapshot
    // and close it before allocating Python objects or running user code.
    static CURSOR: Mutex<()> = Mutex::new(());
    let _lock = CURSOR.lock().unwrap_or_else(|poison| poison.into_inner());
    struct Cursor;
    impl Drop for Cursor {
        fn drop(&mut self) {
            // SAFETY: this guard pairs setpwent with endpwent while CURSOR is locked.
            unsafe { libc::endpwent() };
        }
    }
    // SAFETY: CURSOR serializes this process-wide enumeration from reset through close.
    unsafe { libc::setpwent() };
    let _cursor = Cursor;
    let mut rows = Vec::new();
    // SAFETY: a non-null getpwent result points to a record valid until the next cursor operation.
    while let Some(p) = unsafe { libc::getpwent().as_ref() } {
        // SAFETY: the locked cursor has not advanced since getpwent supplied its fields.
        rows.push(unsafe { Entry::snapshot(p) });
    }
    rows
}

enum Query {
    Name(CString),
    Uid(libc::uid_t),
}

/// Reentrant point lookups own their result storage, independently of the enumeration cursor.
fn lookup(query: &Query) -> Result<Option<Entry>> {
    // SAFETY: sysconf reads the named NSS buffer-size hint and takes no pointers.
    let suggested = unsafe { libc::sysconf(libc::_SC_GETPW_R_SIZE_MAX) };
    let mut size = usize::try_from(suggested).unwrap_or(1024).max(1);
    let mut storage = Vec::<u8>::new();
    loop {
        storage
            .try_reserve_exact(size - storage.len())
            .map_err(|_| Error::new("MemoryError", ""))?;
        storage.resize(size, 0);
        let mut pwd = MaybeUninit::uninit();
        let mut found = ptr::null_mut();
        let buf = storage.as_mut_ptr().cast();
        // SAFETY: pwd and found are writable outputs, buf spans size bytes of live storage,
        // and a name query keeps its NUL-terminated CString alive throughout the call.
        let status = unsafe {
            match query {
                Query::Name(name) => {
                    libc::getpwnam_r(name.as_ptr(), pwd.as_mut_ptr(), buf, size, &mut found)
                }
                Query::Uid(uid) => libc::getpwuid_r(*uid, pwd.as_mut_ptr(), buf, size, &mut found),
            }
        };
        if status != libc::ERANGE {
            return Ok(if status == 0 {
                // SAFETY: success returns null or the initialized pwd backed by the still-live buffer.
                unsafe { found.as_ref().map(|entry| Entry::snapshot(entry)) }
            } else {
                None
            });
        }
        size = size
            .checked_mul(2)
            .ok_or_else(|| Error::new("MemoryError", ""))?;
    }
}

pub(super) fn call(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            format!("pwd.{name}() takes no keyword arguments"),
        ));
    }
    if name == "getpwall" {
        if !args.is_empty() {
            return Err(Error::new(
                "TypeError",
                format!("pwd.getpwall() takes no arguments ({} given)", args.len()),
            ));
        }
        let records = entries()
            .into_iter()
            .map(|entry| entry.record(ctx))
            .collect::<Result<_>>()?;
        return Ok(ctx.alloc(Value::List(records)));
    }
    let &[value] = args else {
        return Err(Error::new(
            "TypeError",
            format!(
                "pwd.{name}() takes exactly one argument ({} given)",
                args.len()
            ),
        ));
    };
    let query = if name == "getpwuid" {
        let index = ctx.index_integer(value).map_err(|_| {
            Error::new(
                "TypeError",
                format!("uid should be integer, not {}", ctx.type_name(value)),
            )
        })?;
        let uid = crate::objects::os::credential_id(&index, false).map_err(|_| {
            let key = ctx.text("getpwuid(): uid not found");
            ctx.lookup_error("KeyError", key)
        })?;
        Query::Uid(uid as libc::uid_t)
    } else {
        if !matches!(
            ctx.heap.get(ctx.scalar_value(value)),
            Value::Str(_) | Value::SurrogateStr(_)
        ) {
            return Err(Error::new(
                "TypeError",
                format!(
                    "getpwnam() argument must be str, not {}",
                    ctx.type_name(value)
                ),
            ));
        }
        Query::Name(os::c_path(ctx, value, "embedded null byte")?)
    };
    if let Some(entry) = lookup(&query)? {
        return entry.record(ctx);
    }
    let key = match query {
        Query::Name(_) => {
            let mut message: Vec<u32> = "getpwnam(): name not found: "
                .chars()
                .map(u32::from)
                .collect();
            if let Some(repr) = ctx.repr_override(value)? {
                message.extend_from_slice(&ctx.string_codepoints(repr)?);
            } else {
                message.extend(ctx.repr(value).chars().map(u32::from));
            }
            ctx.text_codepoints(message)
        }
        Query::Uid(uid) => ctx.text(format!("getpwuid(): uid not found: {}", python_id(uid))),
    };
    Err(ctx.lookup_error("KeyError", key))
}
