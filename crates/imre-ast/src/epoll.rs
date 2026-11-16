//! Event masks and flags from the Linux epoll ABI.

pub const FLAGS: &[(&str, u32)] = &[
    ("EPOLLIN", libc::EPOLLIN as u32),
    ("EPOLLOUT", libc::EPOLLOUT as u32),
    ("EPOLLPRI", libc::EPOLLPRI as u32),
    ("EPOLLERR", libc::EPOLLERR as u32),
    ("EPOLLHUP", libc::EPOLLHUP as u32),
    ("EPOLLRDHUP", libc::EPOLLRDHUP as u32),
    ("EPOLLET", libc::EPOLLET as u32),
    ("EPOLLONESHOT", libc::EPOLLONESHOT as u32),
    ("EPOLLRDNORM", libc::EPOLLRDNORM as u32),
    ("EPOLLRDBAND", libc::EPOLLRDBAND as u32),
    ("EPOLLWRNORM", libc::EPOLLWRNORM as u32),
    ("EPOLLWRBAND", libc::EPOLLWRBAND as u32),
    ("EPOLLMSG", libc::EPOLLMSG as u32),
    #[cfg(has_epollexclusive)]
    ("EPOLLEXCLUSIVE", libc::EPOLLEXCLUSIVE as u32),
    ("EPOLLWAKEUP", libc::EPOLLWAKEUP as u32),
    ("EPOLL_CLOEXEC", libc::EPOLL_CLOEXEC as u32),
];
