//! Build the runtime's Unicode and platform constant tables.
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use regex_syntax::hir::{Class, HirKind};

/// Use the Unicode classes from the pinned regex engine, including singleton properties.
fn unicode_class(pattern: &str) -> regex_syntax::hir::ClassUnicode {
    let hir = regex_syntax::Parser::new()
        .parse(pattern)
        .expect("valid Unicode class");
    match hir.into_kind() {
        HirKind::Class(Class::Unicode(class)) => class,
        HirKind::Literal(literal) => {
            let text = std::str::from_utf8(&literal.0).expect("Unicode literal");
            let point = text.chars().next().expect("singleton character class");
            assert_eq!(text.len(), point.len_utf8());
            regex_syntax::hir::ClassUnicode::new([regex_syntax::hir::ClassUnicodeRange::new(
                point, point,
            )])
        }
        _ => panic!("expected a Unicode character class"),
    }
}

fn write_class_table(out_dir: &Path, file: &str, pattern: &str) {
    let class = unicode_class(pattern);
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
    std::fs::write(out_dir.join(file), table).unwrap();
}

/// Combine the Unicode 16.0 General_Category classes shipped by regex-syntax 0.8.11.
/// The regex engine only indexes scalar values. Python also accepts lone surrogates.
fn write_general_categories(out_dir: &Path) {
    const NAMES: [&str; 30] = [
        "Cn", "Cs", "Cc", "Cf", "Co", "Ll", "Lm", "Lo", "Lt", "Lu", "Mc", "Me", "Mn", "Nd", "Nl",
        "No", "Pc", "Pd", "Pe", "Pf", "Pi", "Po", "Ps", "Sc", "Sk", "Sm", "So", "Zl", "Zp", "Zs",
    ];
    let mut categories = vec![0u8; 0x110000]; // Cn is the UCD default.
    categories[0xd800..=0xdfff].fill(1); // Cs has no scalar values.
    for (index, name) in NAMES.iter().enumerate().skip(2) {
        let class = unicode_class(&format!(r"\p{{gc={name}}}"));
        for range in class.ranges() {
            categories[range.start() as usize..=range.end() as usize].fill(index as u8);
        }
    }
    let mut table = format!("(&{NAMES:?}, &[\n");
    for (point, &category) in categories.iter().enumerate() {
        if point == 0 || category != categories[point - 1] {
            writeln!(table, "    ({point:#x}, {category}),").unwrap();
        }
    }
    table.push_str("])\n");
    std::fs::write(out_dir.join("unicode_general_categories.rs"), table).unwrap();
}

// Keep target-specific constants in C so their names and values come from the target headers.
fn add_confname_table(c: &mut String, table: &str, names: &str) {
    writeln!(c, "static const struct entry {table}_entries[] = {{").unwrap();
    for name in names.lines().filter(|line| !line.starts_with('#')) {
        writeln!(c, "#ifdef _{name}\n{{\"{name}\", _{name}}},\n#endif").unwrap();
    }
    writeln!(
        c,
        r#"}};
const struct entry *imre_platform_{table}_entries(size_t *length) {{
    *length = sizeof({table}_entries) / sizeof({table}_entries[0]);
    return {table}_entries;
}}"#
    )
    .unwrap();
}

fn add_configuration_queries(c: &mut String) {
    c.push_str("#include <unistd.h>\n");
    add_confname_table(c, "confstr", include_str!("native/confstr-names.txt"));
    add_confname_table(c, "sysconf", include_str!("native/sysconf-names.txt"));
    add_confname_table(c, "pathconf", include_str!("native/pathconf-names.txt"));
    c.push_str(
        r#"long imre_platform_sysconf(int name, int *error) {
    errno = 0;
    long value = sysconf(name);
    *error = value == -1 ? errno : 0;
    return value;
}
long imre_platform_pathconf(const char *path, int fd, int name, int *error) {
    errno = 0;
    long value = path == NULL ? fpathconf(fd, name) : pathconf(path, name);
    *error = value == -1 ? errno : 0;
    return value;
}
"#,
    );
}

/// Invert the pinned single-byte decoding tables into compact, directly indexed Unicode pages.
fn write_code_pages(out_dir: &Path) {
    let names_file = "src/objects/linecache/codec_names.txt";
    let tables_file = "src/objects/linecache/charmaps.bin";
    println!("cargo:rerun-if-changed={names_file}");
    println!("cargo:rerun-if-changed={tables_file}");
    let names = std::fs::read_to_string(names_file).unwrap();
    let tables = std::fs::read(tables_file).unwrap();
    let mut output = String::new();
    // Every encoder uses the same zero row for empty pages.
    let mut rows = vec![[0_u8; 256]];
    for (index, name) in [
        (4, "cp1251"),
        (5, "cp424"),
        (6, "cp437"),
        (8, "koi8_u"),
        (9, "cp1252"),
        (10, "iso8859_15"),
        (12, "iso8859_2"),
    ] {
        let (_, table) = names
            .lines()
            .filter_map(|line| line.split_once(' '))
            .find(|(alias, _)| *alias == name)
            .unwrap();
        let table: usize = table.parse().unwrap();
        let mut pages = [0_u8; 256];
        let mut page_count = 0;
        let mut ascii_identity = true;
        for (byte, encoded) in tables[table * 512..(table + 1) * 512]
            .chunks_exact(2)
            .enumerate()
        {
            let scalar = u16::from_le_bytes([encoded[0], encoded[1]]);
            if (byte < 128 || scalar < 128) && usize::from(scalar) != byte {
                ascii_identity = false;
            }
            if scalar == 0xfffe {
                continue;
            }
            let page = usize::from(scalar >> 8);
            page_count = page_count.max(page + 1);
            if byte == 0 {
                continue; // Its unique meaning comes from the decoding table.
            }
            let row = &mut pages[page];
            if *row == 0 {
                *row = u8::try_from(rows.len()).expect("native code-page row index fits in a byte");
                rows.push([0; 256]);
            }
            rows[usize::from(*row)][usize::from(scalar & 255)] = byte as u8;
        }
        let symbol = name.to_ascii_uppercase();
        writeln!(output, "static {symbol}: CodePage = CodePage {{ index: {index}, module_name: {name:?}, decode: Charmap::at({table}), ascii_identity: {ascii_identity}, pages: &{:?} }};", &pages[..page_count]).unwrap();
    }
    writeln!(
        output,
        "static CODE_PAGE_ROWS: [[u8; 256]; {}] = {rows:?};",
        rows.len()
    )
    .unwrap();
    std::fs::write(out_dir.join("code_pages.rs"), output).unwrap();
}

fn main() {
    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    write_code_pages(&out_dir);
    write_class_table(&out_dir, "unicode_alnum.rs", r"[\p{L}\p{N}]");
    write_class_table(&out_dir, "unicode_alpha.rs", r"\p{L}");
    write_class_table(&out_dir, "unicode_identifier_start.rs", r"[_\p{XID_Start}]");
    write_class_table(
        &out_dir,
        "unicode_identifier_continue.rs",
        r"\p{XID_Continue}",
    );
    write_general_categories(&out_dir);
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
    if matches!(
        std::env::var("CARGO_CFG_TARGET_OS").as_deref(),
        Ok("linux" | "macos")
    ) {
        // h_errno may be a macro rather than an exported variable.
        c.push_str("#include <netdb.h>\nint imre_socket_host_errno(void) { return h_errno; }\n");
    }
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        // Darwin exposes this action modifier through its termios header.
        c.push_str("#include <termios.h>\nint imre_platform_tcsasoft(void) { return TCSASOFT; }\n");
    }
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
        println!("cargo:rerun-if-changed=native/confstr-names.txt");
        println!("cargo:rerun-if-changed=native/sysconf-names.txt");
        println!("cargo:rerun-if-changed=native/pathconf-names.txt");
        add_configuration_queries(&mut c);
    }
    c.push_str(
        "#include <limits.h>\n#ifndef NGROUPS_MAX\n#define NGROUPS_MAX 64\n#endif\n\
         size_t imre_platform_max_groups(void) { return NGROUPS_MAX; }\n",
    );
    let path = out_dir.join("platform.c");
    std::fs::write(&path, c).unwrap();
    println!("cargo:rerun-if-changed=native/ssl_capabilities.c");
    println!("cargo:rerun-if-changed=native/locale_decode.c");
    println!("cargo:rerun-if-changed=native/faulthandler.c");
    cc::Build::new()
        .file("native/faulthandler.c")
        .file("native/ssl_capabilities.c")
        .file(path)
        .file("native/locale_decode.c")
        .compile("imre_errno_headers");
}
