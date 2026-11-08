//! Own a Linux mapping and, when requested, its independently duplicated descriptor.
use crate::dynamic::stdlib::{Context, Error, Id, Result, Value, audit, builtins, os, os_error};
use std::io;
use std::mem::MaybeUninit;
use std::ops::Range;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::ptr::NonNull;
use std::rc::Rc;

unsafe extern "C" {
    fn imre_mmap_read(dst: *mut u8, map: *const u8, len: usize);
    fn imre_mmap_write(map: *mut u8, src: *const u8, len: usize);
    fn imre_mmap_copy(dst: *mut u8, step: i64, src: *const u8, len: usize);
}

pub(in crate::dynamic) struct Region {
    address: NonNull<u8>,
    len: usize,
    file: Option<OwnedFd>,
    readonly: bool,
    pub(super) sync: bool,
}

impl Region {
    pub(in crate::dynamic) fn len(&self) -> usize {
        self.len
    }

    pub(in crate::dynamic) fn readonly(&self) -> bool {
        self.readonly
    }

    fn as_ptr(&self) -> *mut u8 {
        self.address.as_ptr()
    }

    pub(in crate::dynamic) fn read_into(&self, start: usize, dst: &mut [u8]) -> Result<()> {
        let source = self.readable_ptr(start, dst.len())?;
        // SAFETY: the region bounds are checked and dst is owned for the entire copy.
        unsafe { imre_mmap_read(dst.as_mut_ptr(), source, dst.len()) };
        Ok(())
    }

    pub(in crate::dynamic) fn write_from(&self, start: usize, src: &[u8]) -> Result<()> {
        let destination = self.writable_ptr(start, src.len())?;
        // SAFETY: the destination is a checked writable range of this live mapping.
        unsafe { imre_mmap_write(destination, src.as_ptr(), src.len()) };
        Ok(())
    }

    /// Transfer from another live mapping without creating a Rust slice of either region.
    pub(super) fn copy_from_map(
        &self,
        start: i64,
        step: i64,
        source: &Region,
        range: Range<usize>,
    ) -> Result<()> {
        let count = range.len();
        if count == 0 {
            return Ok(());
        }
        let source = source.readable_ptr(range.start, count)?;
        let target = self.writable_ptr(start as usize, 1)?;
        // A monotonic stride keeps every access between the two checked endpoints.
        let last = i128::from(start) + (count - 1) as i128 * i128::from(step);
        let last = usize::try_from(last)
            .map_err(|_| Error::new("IndexError", "mmap index out of range"))?;
        self.range(last, 1)?;
        // SAFETY: both regions stay mapped, and the complete source and destination are in bounds.
        unsafe { imre_mmap_copy(target, step, source, count) };
        Ok(())
    }

    pub(in crate::dynamic) fn readable_ptr(&self, start: usize, size: usize) -> Result<*const u8> {
        Ok(self.range(start, size)?.cast_const())
    }

    pub(in crate::dynamic) fn writable_ptr(&self, start: usize, size: usize) -> Result<*mut u8> {
        if self.readonly {
            return Err(Error::new("TypeError", "cannot modify read-only memory"));
        }
        self.range(start, size)
    }

    fn range(&self, start: usize, size: usize) -> Result<*mut u8> {
        if start > self.len || size > self.len - start {
            return Err(Error::new("IndexError", "mmap index out of range"));
        }
        Ok(self.as_ptr().wrapping_add(start))
    }

    pub(in crate::dynamic) fn size(&self) -> Result<i64> {
        let fd = self.file.as_ref().map_or(-1, AsRawFd::as_raw_fd);
        let (_, size) = file_status(fd).map_err(os_error)?;
        Ok(size)
    }

    pub(in crate::dynamic) fn flush(&self, offset: usize, size: usize) -> Result<()> {
        if offset > self.len || size > self.len - offset {
            return Err(Error::new("ValueError", "flush values out of range"));
        }
        // SAFETY: The entire range belongs to this live region. The kernel checks alignment.
        if unsafe { libc::msync(self.as_ptr().add(offset).cast(), size, libc::MS_SYNC) } == -1 {
            return Err(os_error(io::Error::last_os_error()));
        }
        Ok(())
    }
}

impl Drop for Region {
    fn drop(&mut self) {
        // SAFETY: This is the original address and length, owned until the final reference drops.
        unsafe { libc::munmap(self.as_ptr().cast(), self.len) };
    }
}

fn file_status(fd: i32) -> io::Result<(libc::mode_t, i64)> {
    let mut status = MaybeUninit::uninit();
    // SAFETY: fstat initializes status on success and validates the descriptor itself.
    if unsafe { libc::fstat(fd, status.as_mut_ptr()) } == -1 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: fstat returned successfully and initialized status.
    let status = unsafe { status.assume_init() };
    #[allow(clippy::useless_conversion, reason = "off_t can be narrower than i64")]
    let size = i64::from(status.st_size);
    Ok((status.st_mode, size))
}

pub(in crate::dynamic) fn create(
    ctx: &mut Context,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    if args.len() > 6 {
        return Err(Error::new(
            "TypeError",
            format!(
                "function takes at most 6 positional arguments ({} given)",
                args.len()
            ),
        ));
    }
    let [Some(fd), Some(length), flags, prot, access, offset, track] =
        builtins::bind_builtin_arguments(
            "mmap",
            args,
            kwargs,
            [
                "fileno", "length", "flags", "prot", "access", "offset", "trackfd",
            ],
            2,
        )?
    else {
        unreachable!()
    };
    let fd = ctx.index_c_int(fd)?;
    let length = os::ssize_t(ctx, length)?;
    let mut flags = flags
        .map(|v| ctx.index_c_int(v))
        .transpose()?
        .unwrap_or(libc::MAP_SHARED);
    let mut prot = prot
        .map(|v| ctx.index_c_int(v))
        .transpose()?
        .unwrap_or(libc::PROT_READ | libc::PROT_WRITE);
    let access = access.map(|v| ctx.index_c_int(v)).transpose()?.unwrap_or(0);
    let offset = offset
        .map(|v| ctx.index_c_long(v))
        .transpose()?
        .unwrap_or(0);
    let track = track.map(|v| ctx.truth(v)).transpose()?.unwrap_or(true);
    let mut len = usize::try_from(length)
        .map_err(|_| Error::new("OverflowError", "memory mapped length must be positive"))?;
    if offset < 0 {
        return Err(Error::new(
            "OverflowError",
            "memory mapped offset must be positive",
        ));
    }
    if access != 0 && (flags != libc::MAP_SHARED || prot != libc::PROT_READ | libc::PROT_WRITE) {
        return Err(Error::new(
            "ValueError",
            "mmap can't specify both access and flags, prot.",
        ));
    }
    match access {
        0 => {}
        1 => prot = libc::PROT_READ,
        2 => {}
        3 => flags = libc::MAP_PRIVATE,
        _ => return Err(Error::new("ValueError", "mmap invalid access parameter.")),
    }
    if prot & libc::PROT_READ == 0
        || prot & !(libc::PROT_READ | libc::PROT_WRITE) != 0
        || flags & !(libc::MAP_SHARED | libc::MAP_PRIVATE | libc::MAP_ANONYMOUS) != 0
    {
        return Err(Error::new(
            "NotImplementedError",
            "unsupported mmap flags or protection",
        ));
    }
    let readonly = prot & libc::PROT_WRITE == 0;
    let audit_access = if access == 0 && readonly { 1 } else { access };
    let audit_args = [
        ctx.int(i64::from(fd)),
        ctx.int(length as i64),
        ctx.int(i64::from(audit_access)),
        ctx.int(offset),
    ];
    audit::event(ctx, "mmap.__new__", &audit_args)?;

    if fd == -1 {
        flags |= libc::MAP_ANONYMOUS;
    }
    let file = if fd != -1 && track {
        crate::objects::os::dup(fd).map(|duplicate| {
            // SAFETY: dup returned a new, independently owned descriptor.
            Some(unsafe { OwnedFd::from_raw_fd(duplicate) })
        })
    } else {
        Ok(None)
    };
    // Validate and map the same open file description. If duplication failed, the
    // original descriptor can still diagnose an invalid extent before that error.
    let fd = match &file {
        Ok(Some(file)) => file.as_raw_fd(),
        _ => fd,
    };
    let kernel_offset = libc::off_t::try_from(offset)
        .map_err(|_| Error::new("OverflowError", "mmap offset is too large"))?;
    if fd != -1
        && let Ok((mode, file_size)) = file_status(fd)
        && mode & libc::S_IFMT == libc::S_IFREG
    {
        if len == 0 {
            if file_size == 0 {
                return Err(Error::new("ValueError", "cannot mmap an empty file"));
            }
            if offset >= file_size {
                return Err(Error::new(
                    "ValueError",
                    "mmap offset is greater than file size",
                ));
            }
            len = isize::try_from(file_size - offset)
                .map_err(|_| Error::new("ValueError", "mmap length is too large"))?
                as usize;
        } else if offset > file_size || len as i64 > file_size - offset {
            return Err(Error::new(
                "ValueError",
                "mmap length is greater than file size",
            ));
        }
    }
    let file = file?;
    // SAFETY: No fixed-address flags pass the check above. The kernel selects the address.
    let raw = unsafe { libc::mmap(std::ptr::null_mut(), len, prot, flags, fd, kernel_offset) };
    if raw == libc::MAP_FAILED {
        return Err(os_error(io::Error::last_os_error()));
    }
    let Some(address) = NonNull::new(raw.cast()) else {
        // SAFETY: mmap succeeded and returned this address and length.
        unsafe { libc::munmap(raw, len) };
        return Err(Error::new("OSError", "mmap returned a null address"));
    };
    let region = Region {
        address,
        len,
        file,
        readonly,
        sync: !readonly && access != 3,
    };
    ctx.heap.alloc_descriptor_owner(Value::Mmap {
        region: Some(Rc::new(region)),
        pos: 0,
    })
}

fn page_size() -> usize {
    // SAFETY: _SC_PAGESIZE is a process-wide read-only configuration query on Linux.
    unsafe { libc::sysconf(libc::_SC_PAGESIZE) as usize }
}

/// A size callback released the mapping. Linux checks alignment before coverage.
/// An aligned empty range succeeds, while a nonempty range has no backing pages.
pub(super) fn flush_after_close(offset: usize, size: usize) -> Result<()> {
    if !offset.is_multiple_of(page_size()) {
        return Err(os_error(io::Error::from_raw_os_error(libc::EINVAL)));
    }
    if size != 0 {
        return Err(os_error(io::Error::from_raw_os_error(libc::ENOMEM)));
    }
    Ok(())
}

pub(in crate::dynamic) fn install(ctx: &mut Context, scope: Id) {
    let page_size = page_size();
    for (name, number) in [
        ("PAGESIZE", page_size as i64),
        ("ALLOCATIONGRANULARITY", page_size as i64),
        ("ACCESS_DEFAULT", 0),
        ("ACCESS_READ", 1),
        ("ACCESS_WRITE", 2),
        ("ACCESS_COPY", 3),
        ("MAP_SHARED", libc::MAP_SHARED.into()),
        ("MAP_PRIVATE", libc::MAP_PRIVATE.into()),
        ("MAP_ANON", libc::MAP_ANONYMOUS.into()),
        ("MAP_ANONYMOUS", libc::MAP_ANONYMOUS.into()),
        ("PROT_READ", libc::PROT_READ.into()),
        ("PROT_WRITE", libc::PROT_WRITE.into()),
    ] {
        let value = ctx.int(number);
        ctx.bind(scope, name, value);
    }
    let ty = ctx.builtin("mmap.mmap");
    ctx.bind(scope, "mmap", ty);
    let err = ctx.builtin("OSError");
    ctx.bind(scope, "error", err);
}
