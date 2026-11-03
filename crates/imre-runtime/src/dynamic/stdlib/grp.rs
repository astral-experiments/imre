//! Group enumeration and lookups through the host's POSIX group database.

use std::{
    ffi::{CStr, CString},
    mem::MaybeUninit,
    ptr,
    sync::Mutex,
};

use super::{Context, Error, Id, NONE, Result, Value, builtins, fixed_structseq, os};

pub(in crate::dynamic) fn install(ctx: &mut Context, scope: Id) {
    for name in ["getgrall", "getgrgid", "getgrnam", "struct_group"] {
        let value = ctx.builtin(&format!("grp.{name}"));
        ctx.bind(scope, name, value);
    }
}

struct Entry {
    name: CString,
    passwd: Option<CString>,
    gid: libc::gid_t,
    members: Vec<CString>,
}

impl Entry {
    /// Copy a group and its members before NSS reuses their storage.
    ///
    /// # Safety
    /// The group must be a live NSS result, including its terminated strings and member array.
    unsafe fn snapshot(group: &libc::group) -> Self {
        // SAFETY: gr_name is a required field and points to a live C string.
        let name = unsafe { CStr::from_ptr(group.gr_name) }.to_owned();
        let passwd = (!group.gr_passwd.is_null()).then(|| {
            // SAFETY: the optional password is a live C string when present.
            unsafe { CStr::from_ptr(group.gr_passwd) }.to_owned()
        });
        let mut members = Vec::new();
        let mut slot = group.gr_mem;
        // Some NSS implementations store the pointer array at an unaligned buffer offset.
        while !slot.is_null() {
            // SAFETY: the null-terminated array contains at least this pointer-sized slot.
            let member = unsafe { slot.read_unaligned() };
            if member.is_null() {
                break;
            }
            // SAFETY: member is a live C string, and a non-null entry precedes another slot.
            unsafe {
                members.push(CStr::from_ptr(member).to_owned());
                slot = slot.add(1);
            }
        }
        Self {
            name,
            passwd,
            gid: group.gr_gid,
            members,
        }
    }

    fn record(self, ctx: &mut Context) -> Result<Id> {
        let codec = crate::objects::Codec::lookup(crate::sys::filesystem_encoding()?.as_str())?;
        let mut decode = |s: &CStr| -> Result<Id> {
            Ok(ctx.text_codepoints(codec.decode_filesystem(s.to_bytes())?))
        };
        let name = decode(&self.name)?;
        let passwd = self
            .passwd
            .as_deref()
            .map(&mut decode)
            .transpose()?
            .unwrap_or(NONE);
        let members = self
            .members
            .iter()
            .map(|s| decode(s))
            .collect::<Result<_>>()?;
        let members = ctx.alloc(Value::List(members));
        let gid = ctx.int(python_gid(self.gid));
        Ok(fixed_structseq::record(
            ctx,
            fixed_structseq::GROUP,
            vec![name, passwd, gid, members],
        ))
    }
}

fn python_gid(gid: libc::gid_t) -> i64 {
    if gid == libc::gid_t::MAX {
        -1
    } else {
        i64::from(gid)
    }
}

/// Look up a user's groups without changing the credentials of the calling process.
#[cfg(target_os = "linux")]
pub(super) fn getgrouplist(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            "posix.getgrouplist() takes no keyword arguments",
        ));
    }
    let &[user, base] = args else {
        return Err(Error::new(
            "TypeError",
            format!("getgrouplist expected 2 arguments, got {}", args.len()),
        ));
    };
    let text = ctx.scalar_value(user);
    if !matches!(ctx.heap.get(text), Value::Str(_) | Value::SurrogateStr(_)) {
        return Err(Error::new(
            "TypeError",
            format!(
                "getgrouplist() argument 1 must be str, not {}",
                ctx.type_name(user)
            ),
        ));
    }
    let user = CString::new(ctx.encode_utf8_codepoints(text, false, "strict")?)
        .map_err(|_| Error::new("ValueError", "embedded null character"))?;
    let base = os::owner_id(ctx, base, true)? as libc::gid_t;
    let mut capacity: libc::c_int = 65;
    let mut groups = Vec::new();
    loop {
        groups
            .try_reserve_exact(capacity as usize - groups.len())
            .map_err(|_| Error::new("MemoryError", ""))?;
        groups.resize(capacity as usize, 0);
        let mut count = capacity;
        // SAFETY: user is NUL-terminated and groups owns capacity writable gid_t slots.
        let result =
            unsafe { libc::getgrouplist(user.as_ptr(), base, groups.as_mut_ptr(), &mut count) };
        if result >= 0 {
            groups.truncate(count as usize);
            let values = groups
                .into_iter()
                .map(|gid| ctx.int(python_gid(gid)))
                .collect();
            return Ok(ctx.alloc(Value::List(values)));
        }
        // The group database can grow between calls. Some NSS backends leave
        // the size unchanged on failure, so always make progress on a retry.
        capacity = if count > capacity {
            count
        } else {
            capacity
                .checked_mul(2)
                .ok_or_else(|| Error::new("MemoryError", ""))?
        };
    }
}

fn entries() -> Vec<Entry> {
    // setgrent/getgrent/endgrent share one cursor across all runtime contexts.
    static CURSOR: Mutex<()> = Mutex::new(());
    let _lock = CURSOR.lock().unwrap_or_else(|poison| poison.into_inner());
    struct Cursor;
    impl Drop for Cursor {
        fn drop(&mut self) {
            // SAFETY: the cursor closes while this enumeration still owns CURSOR.
            unsafe { libc::endgrent() };
        }
    }
    // SAFETY: CURSOR excludes enumeration in other contexts from reset through close.
    unsafe { libc::setgrent() };
    let _cursor = Cursor;
    let mut rows = Vec::new();
    // SAFETY: a non-null group is live until another cursor call, all protected by CURSOR.
    while let Some(group) = unsafe { libc::getgrent().as_ref() } {
        // SAFETY: the cursor has not advanced since getgrent returned this entry.
        rows.push(unsafe { Entry::snapshot(group) });
    }
    rows
}

enum Query {
    Gid(libc::gid_t),
    Name(CString),
}

/// Reentrant point lookups use caller-owned storage independently of the enumeration cursor.
fn lookup(query: &Query) -> Result<Option<Entry>> {
    // SAFETY: sysconf reads a size hint and takes no pointers.
    let hint = unsafe { libc::sysconf(libc::_SC_GETGR_R_SIZE_MAX) };
    let mut size = usize::try_from(hint).unwrap_or(1024).max(1);
    let mut storage = Vec::<u8>::new();
    loop {
        storage
            .try_reserve_exact(size - storage.len())
            .map_err(|_| Error::new("MemoryError", ""))?;
        storage.resize(size, 0);
        let mut group = MaybeUninit::uninit();
        let mut found = ptr::null_mut();
        let buf = storage.as_mut_ptr().cast();
        // SAFETY: group and found are writable outputs, the buffer spans size live bytes, and
        // a name query retains its NUL-terminated string until the call returns.
        let status = unsafe {
            match query {
                Query::Gid(gid) => {
                    libc::getgrgid_r(*gid, group.as_mut_ptr(), buf, size, &mut found)
                }
                Query::Name(name) => {
                    libc::getgrnam_r(name.as_ptr(), group.as_mut_ptr(), buf, size, &mut found)
                }
            }
        };
        if status != libc::ERANGE {
            return Ok(if status == 0 {
                // SAFETY: success returns null or the initialized group backed by our live buffer.
                unsafe { found.as_ref().map(|group| Entry::snapshot(group)) }
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
    if name == "getgrall" {
        if !kwargs.is_empty() {
            return Err(Error::new(
                "TypeError",
                "grp.getgrall() takes no keyword arguments",
            ));
        }
        if !args.is_empty() {
            return Err(Error::new(
                "TypeError",
                format!("grp.getgrall() takes no arguments ({} given)", args.len()),
            ));
        }
        let records = entries()
            .into_iter()
            .map(|entry| entry.record(ctx))
            .collect::<Result<_>>()?;
        return Ok(ctx.alloc(Value::List(records)));
    }

    let param = if name == "getgrgid" { "id" } else { "name" };
    let [Some(value)] = builtins::bind_builtin_arguments(name, args, kwargs, [param], 1)? else {
        unreachable!("group lookup requires one argument")
    };
    let query = if name == "getgrgid" {
        Query::Gid(os::owner_id(ctx, value, true)? as libc::gid_t)
    } else {
        if ctx.string_storage(value).is_none() {
            let actual = if value == NONE {
                "None"
            } else {
                ctx.type_name(value)
            };
            return Err(Error::new(
                "TypeError",
                format!("getgrnam() argument 'name' must be str, not {actual:.50}"),
            ));
        }
        Query::Name(os::c_path(ctx, value, "embedded null byte")?)
    };
    if let Some(entry) = lookup(&query)? {
        return entry.record(ctx);
    }
    let key = match query {
        Query::Gid(gid) => ctx.text(format!("getgrgid(): gid not found: {}", python_gid(gid))),
        Query::Name(_) => {
            let repr = ctx.builtin("repr");
            let rendered = ctx.call(repr, vec![value], Vec::new())?;
            let mut message: Vec<u32> = "getgrnam(): name not found: "
                .chars()
                .map(u32::from)
                .collect();
            message.extend_from_slice(&ctx.string_codepoints(rendered)?);
            ctx.text_codepoints(message)
        }
    };
    Err(ctx.lookup_error("KeyError", key))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshots_unaligned_member_pointers_before_nss_reuses_storage() {
        let entry = {
            let members = [c"first".to_owned(), c"second".to_owned()];
            let mut storage = [MaybeUninit::<*mut libc::c_char>::uninit(); 4];
            // Start one byte after a pointer-aligned base, leaving room for three pointer slots.
            let slots = storage
                .as_mut_ptr()
                .cast::<u8>()
                .wrapping_add(1)
                .cast::<*mut libc::c_char>();
            assert!(!slots.is_aligned());
            let group = libc::group {
                gr_name: c"two-members".as_ptr().cast_mut(),
                gr_passwd: ptr::null_mut(),
                gr_gid: 23,
                gr_mem: slots,
            };
            // SAFETY: the three pointer slots fit in storage, and the strings live through snapshot.
            unsafe {
                slots.write_unaligned(members[0].as_ptr().cast_mut());
                slots.add(1).write_unaligned(members[1].as_ptr().cast_mut());
                slots.add(2).write_unaligned(ptr::null_mut());
                Entry::snapshot(&group)
            }
        };
        assert_eq!(entry.members, [c"first".to_owned(), c"second".to_owned()]);
        assert!(entry.passwd.is_none());
    }
}
