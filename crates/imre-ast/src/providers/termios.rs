//! Attribute flags, control characters and speeds shared by both import paths.
macro_rules! constants {
    ($($name:ident),+ $(,)?) => {
        &[$((stringify!($name), concat!("termios.", stringify!($name)), libc::$name as i64)),+]
    };
}

/// Public name, compiler-qualified symbol, and value from the platform ABI.
pub const CONSTANTS: &[(&str, &str, i64)] = constants!(
    TCSANOW, TCSADRAIN, TCSAFLUSH, TCIFLUSH, TCOFLUSH, TCIOFLUSH, TCOOFF, TCOON, TCIOFF, TCION,
    NCCS, VINTR, VQUIT, VERASE, VKILL, VEOF, VTIME, VMIN, VSTART, VSTOP, VSUSP, VEOL, VEOL2,
    VREPRINT, VDISCARD, VWERASE, VLNEXT, IGNBRK, BRKINT, IGNPAR, PARMRK, INPCK, ISTRIP, INLCR,
    IGNCR, ICRNL, IXON, IXANY, IXOFF, OPOST, ONLCR, CSIZE, CS5, CS6, CS7, CS8, CSTOPB, CREAD,
    PARENB, PARODD, HUPCL, CLOCAL, CRTSCTS, ISIG, ICANON, ECHO, ECHOE, ECHOK, ECHONL, NOFLSH,
    TOSTOP, IEXTEN, B0, B50, B75, B110, B134, B150, B200, B300, B600, B1200, B2400, B4800, B9600,
    B19200, B38400, B57600, B115200, B230400,
);
