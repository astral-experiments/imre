//! Preserve missing standard descriptors across the compiler's execs on Linux.

// Rust fills closed standard descriptors with inheritable /dev/null handles during
// startup. Reserve the same slots before lang_start so the kernel closes them on
// exec, whether the next program is a compiler tool or the generated Python image.
#[used]
#[unsafe(link_section = ".init_array.00001")]
static RESERVE_CLOSED_STDIO: extern "C" fn() = reserve_closed_stdio;

extern "C" fn reserve_closed_stdio() {
    // SAFETY: this ELF initializer uses only libc calls before Rust startup. open
    // claims an unused descriptor atomically and never overwrites an inherited one.
    unsafe {
        if libc::getauxval(libc::AT_SECURE) != 0 {
            return;
        }
        for fd in libc::STDIN_FILENO..=libc::STDERR_FILENO {
            if libc::fcntl(fd, libc::F_GETFD) >= 0 || *libc::__errno_location() != libc::EBADF {
                continue;
            }
            let reserved = loop {
                let fd = libc::open(c"/dev/null".as_ptr(), libc::O_RDWR | libc::O_CLOEXEC);
                if fd >= 0 {
                    break fd;
                }
                if *libc::__errno_location() != libc::EINTR {
                    // Let Rust handle an unavailable /dev/null during its usual startup.
                    return;
                }
            };
            // Another initializer may have filled the gap in the meantime.
            if reserved > libc::STDERR_FILENO {
                libc::close(reserved);
            }
        }
    }
}
