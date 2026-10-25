use std::ffi::{CStr, CString};

/// Resolve a system account by name or id using caller-owned NSS storage.
pub(super) fn account(user: bool, id: u32, name: Option<&str>) -> Option<(u32, String)> {
    let name = name.map(CString::new).transpose().ok()?;
    let mut buf = vec![0u8; 4096];
    macro_rules! lookup {
        ($entry:ty, $by_name:path, $by_id:path, $id:ident, $name:ident) => {{
            let mut entry = std::mem::MaybeUninit::<$entry>::uninit();
            let mut found = std::ptr::null_mut();
            // SAFETY: caller storage and the C name are valid for each complete libc lookup.
            let status = unsafe {
                match &name {
                    Some(s) => $by_name(
                        s.as_ptr(),
                        entry.as_mut_ptr(),
                        buf.as_mut_ptr().cast(),
                        buf.len(),
                        &mut found,
                    ),
                    None => $by_id(
                        id,
                        entry.as_mut_ptr(),
                        buf.as_mut_ptr().cast(),
                        buf.len(),
                        &mut found,
                    ),
                }
            };
            if status == libc::ERANGE {
                buf.resize(buf.len().checked_mul(2)?, 0);
                continue;
            }
            if status != 0 || found.is_null() {
                return None;
            }
            // SAFETY: a successful non-null result initialized both the record and its name.
            unsafe {
                let entry = entry.assume_init();
                return Some((
                    entry.$id,
                    CStr::from_ptr(entry.$name).to_string_lossy().into_owned(),
                ));
            }
        }};
    }
    loop {
        if user {
            lookup!(
                libc::passwd,
                libc::getpwnam_r,
                libc::getpwuid_r,
                pw_uid,
                pw_name
            );
        } else {
            lookup!(
                libc::group,
                libc::getgrnam_r,
                libc::getgrgid_r,
                gr_gid,
                gr_name
            );
        }
    }
}
