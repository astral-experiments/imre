//! Build the runtime's Unicode and platform errno tables.
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use regex_syntax::hir::{Class, HirKind};

/// Use the same pinned Unicode version as the runtime's regex engine.
fn write_alnum_table(out_dir: &Path) {
    let hir = regex_syntax::Parser::new()
        .parse(r"[\p{L}\p{N}]")
        .expect("valid Unicode alphanumeric class");
    let HirKind::Class(Class::Unicode(class)) = hir.kind() else {
        panic!("expected a Unicode character class");
    };
    let mut table = String::from("&[\n");
    for range in class.ranges() {
        writeln!(
            table,
            "    ({:#x}, {:#x}),",
            u32::from(range.start()),
            u32::from(range.end())
        )
        .unwrap();
    }
    table.push_str("]\n");
    std::fs::write(out_dir.join("unicode_alnum.rs"), table).unwrap();
}

fn main() {
    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    write_alnum_table(&out_dir);
    const SYMBOLS: &str = include_str!("native/errno-symbols.txt");
    println!("cargo:rerun-if-changed=native/errno-symbols.txt");
    let mut c = String::from(
        "#include <stddef.h>\n#include <errno.h>\n#ifdef _WIN32\n#include <winsock2.h>\n#endif\n",
    );
    c.push_str("#if defined(__wasi__) && !defined(ESHUTDOWN)\n#define ESHUTDOWN EPIPE\n#endif\n");
    c.push_str(
        "struct entry { const char *name; int code; };\nstatic const struct entry entries[] = {\n",
    );
    for line in SYMBOLS.lines().filter(|line| !line.starts_with('#')) {
        let mut words = line.split_whitespace();
        let name = words.next().expect("errno name");
        let fallback = words.next();
        let winsock = words.next() == Some("winsock");
        if winsock {
            writeln!(
                c,
                "#if defined(_WIN32) && defined({})\n{{\"{name}\", {}}},\n#elif defined({name})",
                fallback.unwrap(),
                fallback.unwrap()
            )
            .unwrap();
        } else {
            writeln!(c, "#ifdef {name}").unwrap();
        }
        writeln!(c, "{{\"{name}\",{name}}},").unwrap();
        if let Some(other) = fallback
            && !winsock
        {
            writeln!(c, "#elif defined({other})\n{{\"{name}\",{other}}},").unwrap();
        }
        c.push_str("#endif\n");
    }
    c.push_str("{NULL,0}};\nconst struct entry *imre_platform_errno_entries(size_t *len) { *len=sizeof(entries)/sizeof(entries[0])-1; return entries; }\n");
    let path = out_dir.join("errno.c");
    std::fs::write(&path, c).unwrap();
    cc::Build::new().file(path).compile("imre_errno_headers");
}
