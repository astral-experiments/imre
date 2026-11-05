use std::env;

fn main() {
    println!("cargo::rustc-check-cfg=cfg(has_epollexclusive)");
    println!("cargo::rerun-if-changed=src/epoll_flags.h");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
        let flags = cc::Build::new().file("src/epoll_flags.h").expand();
        if flags
            .split(|&c| c == b'\n')
            .any(|line| line.trim_ascii() == b"imre_has_epollexclusive")
        {
            println!("cargo::rustc-cfg=has_epollexclusive");
        }
    }
}
