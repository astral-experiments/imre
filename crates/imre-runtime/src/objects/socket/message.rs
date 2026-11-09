//! Vectored socket I/O and control messages using the target's native socket ABI.
use super::{Deadline, Error, Kind, Object, Result, SocketAddress, SocketName, errno_error, fd};
use super::{socket_call_interruptible, unpack_socket_name};
use allocator_api2::boxed::Box;
use std::{io, ptr, slice};

const SOCKLEN_LIMIT: usize = i32::MAX as usize;
// SAFETY: CMSG_LEN only computes a layout size for this constant payload length.
const HEADER_LEN: usize = unsafe { libc::CMSG_LEN(0) as usize };
// SAFETY: CMSG_SPACE only computes a layout size for this constant payload length.
const PADDED_MIN: usize = unsafe { libc::CMSG_SPACE(1) as usize };

pub(crate) fn cmsg_len(size: usize) -> Result<usize> {
    if size > SOCKLEN_LIMIT - HEADER_LEN {
        return Err(Error::new(
            Kind::OverflowError,
            "CMSG_LEN() argument out of range",
        ));
    }
    // SAFETY: the bound above accounts for the header and prevents integer overflow.
    Ok(unsafe { libc::CMSG_LEN(size as u32) } as usize)
}

pub(crate) fn cmsg_space(size: usize) -> Result<usize> {
    if size > SOCKLEN_LIMIT - PADDED_MIN {
        return Err(Error::new(
            Kind::OverflowError,
            "CMSG_SPACE() argument out of range",
        ));
    }
    // SAFETY: the bound above also allows for padding before and after the payload.
    Ok(unsafe { libc::CMSG_SPACE(size as u32) } as usize)
}

/// Size a Python ancillary payload using the same checked ABI layout in both execution paths.
pub(crate) fn control_size(length: isize, padded: bool) -> Result<usize> {
    let size = usize::try_from(length).map_err(|_| {
        let name = if padded { "CMSG_SPACE" } else { "CMSG_LEN" };
        Error::new(
            Kind::OverflowError,
            format!("{name}() argument out of range"),
        )
    })?;
    if padded {
        cmsg_space(size)
    } else {
        cmsg_len(size)
    }
}

/// # Safety
/// The caller keeps the Python integer word alive for the duration of the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_socket_control_size(length: usize, padded: bool) -> usize {
    // SAFETY: the input word is rooted by the typed caller.
    let integer = unsafe { super::Int::borrow(length) };
    let result = crate::signed_index(&integer)
        .ok_or_else(|| {
            Error::new(
                Kind::OverflowError,
                "Python int too large to convert to C ssize_t",
            )
        })
        .and_then(|n| control_size(n, padded))
        .map(|n| super::word(n as i32));
    super::finish(result, super::word(0))
}

pub(crate) struct Control {
    // A header array provides libc's required alignment; len is the exact capacity in bytes.
    storage: Box<[libc::cmsghdr]>,
    len: usize,
}

impl Control {
    fn new(len: usize) -> Result<Self> {
        let count = len.div_ceil(size_of::<libc::cmsghdr>());
        let storage =
            Box::<[libc::cmsghdr]>::try_new_zeroed_slice(count).map_err(|_| Error::allocation())?;
        // SAFETY: every field of cmsghdr accepts zero and the allocation is fully zeroed.
        let storage = unsafe { storage.assume_init() };
        Ok(Self { storage, len })
    }

    fn data(&mut self) -> *mut libc::c_void {
        if self.len == 0 {
            ptr::null_mut()
        } else {
            self.storage.as_mut_ptr().cast()
        }
    }

    pub(crate) fn from_items(items: &[(i32, i32, &[u8])]) -> Result<Self> {
        let mut len = 0;
        for (_, _, data) in items {
            let space = cmsg_space(data.len())
                .map_err(|_| Error::new(Kind::OSError, "ancillary data item too large"))?;
            len += space;
            if len > SOCKLEN_LIMIT {
                return Err(Error::new(Kind::OSError, "too much ancillary data"));
            }
        }
        let mut buf = Self::new(len)?;
        let mut offset = 0;
        for &(level, kind, data) in items {
            // SAFETY: storage is header-aligned, and CMSG_SPACE places each subsequent header
            // at its ABI alignment. The allocation includes all headers, data and padding.
            unsafe {
                let header = buf
                    .storage
                    .as_mut_ptr()
                    .cast::<u8>()
                    .add(offset)
                    .cast::<libc::cmsghdr>();
                (*header).cmsg_len = libc::CMSG_LEN(data.len() as u32) as _;
                (*header).cmsg_level = level;
                (*header).cmsg_type = kind;
                ptr::copy_nonoverlapping(data.as_ptr(), libc::CMSG_DATA(header), data.len());
                offset += libc::CMSG_SPACE(data.len() as u32) as usize;
            }
        }
        Ok(buf)
    }

    fn items(&self) -> ControlItems<'_> {
        // SAFETY: recvmsg can only shrink len; the entire allocation was initialized.
        let bytes = unsafe { slice::from_raw_parts(self.storage.as_ptr().cast(), self.len) };
        ControlItems {
            bytes,
            malformed: false,
        }
    }
}

struct ControlItems<'a> {
    bytes: &'a [u8],
    malformed: bool,
}

impl<'a> Iterator for ControlItems<'a> {
    type Item = (i32, i32, &'a [u8]);

    fn next(&mut self) -> Option<Self::Item> {
        if self.bytes.len() < size_of::<libc::cmsghdr>() {
            return None;
        }
        // SAFETY: the checked prefix contains a full initialized cmsghdr.
        let header = unsafe { self.bytes.as_ptr().cast::<libc::cmsghdr>().read_unaligned() };
        #[allow(
            clippy::unnecessary_cast,
            reason = "cmsg_len is socklen_t on some targets"
        )]
        let len = header.cmsg_len as usize;
        if len < HEADER_LEN || self.bytes.len() < HEADER_LEN {
            self.malformed = true;
            self.bytes = &[];
            return None;
        }
        let end = len.min(self.bytes.len());
        let data = &self.bytes[HEADER_LEN..end];
        self.malformed |= end != len;
        // SAFETY: data.len() is at most the validated ancillary buffer size (INT_MAX).
        let step = unsafe { libc::CMSG_SPACE(data.len() as u32) } as usize;
        self.bytes = &self.bytes[step.min(self.bytes.len())..];
        Some((header.cmsg_level, header.cmsg_type, data))
    }
}

/// Own received SCM_RIGHTS fds until the complete Python result has been built.
pub(crate) struct ReceivedControl {
    control: Control,
    owns_fds: bool,
}

impl ReceivedControl {
    pub(crate) fn new(size: usize) -> Result<Self> {
        if size > SOCKLEN_LIMIT {
            return Err(Error::new(
                Kind::ValueError,
                "invalid ancillary data buffer length",
            ));
        }
        Ok(Self {
            control: Control::new(size)?,
            owns_fds: false,
        })
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = (i32, i32, &[u8])> {
        self.control.items()
    }

    pub(crate) fn malformed(&self) -> bool {
        let mut items = self.control.items();
        items.by_ref().for_each(drop);
        items.malformed
    }

    pub(crate) fn release_fds(&mut self) {
        self.owns_fds = false;
    }
}

impl Drop for ReceivedControl {
    fn drop(&mut self) {
        if self.owns_fds {
            for (level, kind, data) in self.iter() {
                if level == libc::SOL_SOCKET && kind == libc::SCM_RIGHTS {
                    let (descriptors, _) = data.as_chunks();
                    for &fd in descriptors {
                        let _ = super::close_fd(libc::c_int::from_ne_bytes(fd));
                    }
                }
            }
        }
    }
}

/// Send the supplied vector and optional control records in one syscall.
///
/// # Safety
/// All iovec pointers must remain readable for their lengths until this call returns.
pub(crate) unsafe fn send(
    socket: &Object,
    iov: &mut [libc::iovec],
    control: &mut Control,
    flags: i32,
    address: Option<&SocketAddress>,
    until: Option<Deadline>,
) -> Result<usize> {
    // SAFETY: msghdr contains pointers and integers; only the fields used below are nonzero.
    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    if let Some(address) = address {
        let (addr, len) = address.raw_parts();
        msg.msg_name = addr.cast_mut().cast();
        msg.msg_namelen = len;
    }
    msg.msg_iov = iov.as_mut_ptr();
    msg.msg_iovlen = i32::try_from(iov.len())
        .map_err(|_| Error::new(Kind::OSError, "sendmsg() argument 1 is too long"))?
        as _;
    msg.msg_control = control.data();
    msg.msg_controllen = control.len as _;
    let raw = fd(socket);
    socket_call_interruptible(raw, libc::POLLOUT, until, || {
        // SAFETY: control, address and all caller-owned iovec ranges live through retries.
        usize::try_from(unsafe { libc::sendmsg(raw, &msg, flags) })
            .map_err(|_| io::Error::last_os_error())
    })
    .map_err(errno_error)
}

/// Receive into the supplied ranges and return the kernel's count, control records, flags and peer.
///
/// # Safety
/// All iovec ranges must remain writable for their lengths until this call returns.
/// Ranges may overlap each other, but no Rust reference may be live over their contents.
pub(crate) unsafe fn recv(
    socket: &Object,
    iov: &mut [libc::iovec],
    received: &mut ReceivedControl,
    flags: i32,
    until: Option<Deadline>,
) -> Result<(usize, i32, SocketName)> {
    let control = &mut received.control;
    // SAFETY: zero is a valid empty msghdr and initialized sockaddr_storage.
    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    // SAFETY: zero initializes the integer fields and padding of the output address.
    let mut addr: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
    msg.msg_name = (&raw mut addr).cast();
    msg.msg_control = control.data();
    msg.msg_iov = iov.as_mut_ptr();
    msg.msg_iovlen = i32::try_from(iov.len())
        .map_err(|_| Error::new(Kind::OSError, "recvmsg_into() argument 1 is too long"))?
        as _;
    let raw = fd(socket);
    let count = socket_call_interruptible(raw, libc::POLLIN, until, || {
        msg.msg_namelen = size_of_val(&addr) as _;
        msg.msg_controllen = control.len as _;
        msg.msg_flags = 0;
        // SAFETY: msg points into initialized address/control storage and pinned caller buffers.
        usize::try_from(unsafe { libc::recvmsg(raw, &mut msg, flags) })
            .map_err(|_| io::Error::last_os_error())
    })
    .map_err(errno_error)?;
    control.len = control.len.min(msg.msg_controllen as usize);
    received.owns_fds = true;
    let peer = unpack_socket_name(&addr, msg.msg_namelen.min(size_of_val(&addr) as _))?;
    Ok((count, msg.msg_flags, peer))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::{AsRawFd, RawFd};

    /// Identify the file currently occupying a descriptor in this process.
    fn file_identity(fd: RawFd) -> Option<(libc::dev_t, libc::ino_t)> {
        let mut stat = std::mem::MaybeUninit::uninit();
        // SAFETY: fstat checks the descriptor and receives writable storage for one stat.
        if unsafe { libc::fstat(fd, stat.as_mut_ptr()) } == -1 {
            assert_eq!(io::Error::last_os_error().raw_os_error(), Some(libc::EBADF));
            return None;
        }
        // SAFETY: a successful fstat initialized the result.
        let stat = unsafe { stat.assume_init() };
        Some((stat.st_dev, stat.st_ino))
    }

    #[test]
    fn dropping_unreturned_rights_closes_the_received_fd() {
        let (tx, rx) = super::super::socketpair(libc::AF_UNIX, libc::SOCK_STREAM, 0)
            .ok()
            .expect("message sockets");
        let path =
            std::env::temp_dir().join(format!("imre-scm-rights-{}", getrandom::u64().unwrap()));
        let owner = std::fs::File::create_new(&path).unwrap();
        std::fs::remove_file(path).unwrap();
        let expected = file_identity(owner.as_raw_fd()).unwrap();
        let rights = owner.as_raw_fd().to_ne_bytes();
        let mut byte = *b"!";
        let mut iov = [libc::iovec {
            iov_base: byte.as_mut_ptr().cast(),
            iov_len: byte.len(),
        }];
        let record = [(libc::SOL_SOCKET, libc::SCM_RIGHTS, rights.as_slice())];
        let mut outgoing = Control::from_items(&record).ok().unwrap();
        let mut control = ReceivedControl::new(cmsg_space(rights.len()).ok().unwrap())
            .ok()
            .unwrap();
        // SAFETY: the single iovec is live and accessible for both synchronous calls.
        unsafe {
            send(&tx, &mut iov, &mut outgoing, 0, None, None)
                .ok()
                .expect("sendmsg");
            recv(&rx, &mut iov, &mut control, 0, None)
                .ok()
                .expect("recvmsg");
        }
        let (level, kind, data) = control.iter().next().expect("received rights");
        assert_eq!(
            (level, kind, data.len()),
            (libc::SOL_SOCKET, libc::SCM_RIGHTS, rights.len())
        );
        let received = libc::c_int::from_ne_bytes(data.try_into().unwrap());
        assert_eq!(file_identity(received), Some(expected));
        drop(control);
        // A parallel fork can inherit the file, and a sibling thread can reuse the fd.
        assert_ne!(file_identity(received), Some(expected));
    }
}
