//! Pickle's stack-machine opcodes. Object traversal and memo ownership belong to the caller.

use std::io::Write;

use super::{Error, Result};
use crate::dynamic::builtins;

pub(super) fn int(out: &mut Vec<u8>, protocol: u8, number: &crate::Int) -> Result<()> {
    let small = number.to_i64().and_then(|n| i32::try_from(n).ok());
    if protocol == 0 || (protocol == 1 && small.is_none()) {
        let text = crate::integer_text::decimal_string(number)?;
        out.push(if small.is_some() { b'I' } else { b'L' });
        out.extend_from_slice(text.as_bytes());
        if small.is_none() {
            out.push(b'L');
        }
        out.push(b'\n');
        return Ok(());
    }
    match small {
        Some(n @ 0..=255) => out.extend([b'K', n as u8]),
        Some(n @ 256..=65535) => {
            out.push(b'M');
            out.extend((n as u16).to_le_bytes());
        }
        Some(n) => {
            out.push(b'J');
            out.extend(n.to_le_bytes());
        }
        None => {
            let signed = number.big().to_signed_bytes_le();
            if let Ok(length) = u8::try_from(signed.len()) {
                out.extend([0x8a, length]); // LONG1
            } else {
                let length = i32::try_from(signed.len())
                    .map_err(|_| Error::new("OverflowError", "int too large to pickle"))?;
                out.push(0x8b); // LONG4
                out.extend(length.to_le_bytes());
            }
            out.extend(signed);
        }
    }
    Ok(())
}

pub(super) fn float(out: &mut Vec<u8>, protocol: u8, value: f64) {
    if protocol == 0 {
        out.push(b'F'); // FLOAT
        out.extend_from_slice(crate::floats::render(value).as_bytes());
        out.push(b'\n');
    } else {
        out.push(b'G'); // BINFLOAT
        out.extend_from_slice(&value.to_be_bytes());
    }
}

pub(super) fn boolean(out: &mut Vec<u8>, protocol: u8, value: bool) {
    if protocol < 2 {
        out.extend_from_slice(if value { b"I01\n" } else { b"I00\n" });
    } else {
        out.push(if value { 0x88 } else { 0x89 }); // NEWTRUE, NEWFALSE
    }
}

pub(super) fn string(out: &mut Vec<u8>, protocol: u8, text: &str) -> Result<()> {
    if protocol == 0 {
        out.push(b'V'); // UNICODE uses raw-unicode-escape on one line.
        for c in text.chars() {
            let code = u32::from(c);
            if code <= 0xff && !matches!(c, '\0' | '\n' | '\r' | '\\' | '\x1a') {
                out.push(code as u8);
            } else if code <= 0xffff {
                write!(out, "\\u{code:04x}").unwrap();
            } else {
                write!(out, "\\U{code:08x}").unwrap();
            }
        }
        out.push(b'\n');
        return Ok(());
    }

    let len = text.len();
    if protocol >= 4 && u8::try_from(len).is_ok() {
        out.extend([0x8c, len as u8]); // SHORT_BINUNICODE
    } else if let Ok(len) = u32::try_from(len) {
        out.push(b'X'); // BINUNICODE
        out.extend(len.to_le_bytes());
    } else if protocol >= 4 {
        out.push(0x8d); // BINUNICODE8
        out.extend((len as u64).to_le_bytes());
    } else {
        return Err(Error::new("OverflowError", "string too large to pickle"));
    }
    out.extend_from_slice(text.as_bytes());
    Ok(())
}

pub(super) fn bytes(out: &mut Vec<u8>, protocol: u8, bytes: &[u8]) -> Result<()> {
    let len = bytes.len();
    if let Ok(short) = u8::try_from(len) {
        out.extend([b'C', short]); // SHORT_BINBYTES
    } else if let Ok(length) = u32::try_from(len) {
        out.push(b'B'); // BINBYTES
        out.extend(length.to_le_bytes());
    } else if protocol >= 4 {
        out.push(0x8e); // BINBYTES8
        out.extend((len as u64).to_le_bytes());
    } else {
        return Err(Error::new(
            "OverflowError",
            "bytes object is too large to pickle with protocol 3",
        ));
    }
    out.try_reserve(len)
        .map_err(|_| crate::Error::allocation())?;
    out.extend_from_slice(bytes);
    Ok(())
}

pub(super) fn tuple_start(out: &mut Vec<u8>, protocol: u8, len: usize) {
    if protocol == 0 || (protocol == 1 && len > 0) || len > 3 {
        out.push(b'('); // MARK
    }
}

pub(super) fn tuple_end(out: &mut Vec<u8>, protocol: u8, len: usize) {
    out.push(match (protocol, len) {
        (1.., 0) => b')',                 // EMPTY_TUPLE
        (2.., 1..=3) => 0x84 + len as u8, // TUPLE1, TUPLE2, TUPLE3
        _ => b't',                        // TUPLE
    });
}

/// Replace the in-progress elements with a tuple that a recursive child memoized.
pub(super) fn recursive_tuple(out: &mut Vec<u8>, protocol: u8, len: usize, index: u32) {
    if protocol == 0 || (protocol >= 2 && len <= 3) {
        // The text protocol also needs a POP for its MARK.
        out.extend(std::iter::repeat_n(b'0', len + usize::from(protocol == 0)));
    } else {
        out.push(b'1'); // POP_MARK
    }
    memo_get(out, protocol, index);
}

pub(super) fn memo_put(out: &mut Vec<u8>, protocol: u8, id: u32) {
    if protocol >= 4 {
        out.push(0x94); // MEMOIZE
    } else if protocol == 0 {
        writeln!(out, "p{id}").unwrap();
    } else if let Ok(id) = u8::try_from(id) {
        out.extend([b'q', id]);
    } else {
        out.push(b'r');
        out.extend(id.to_le_bytes());
    }
}

pub(super) fn memo_get(out: &mut Vec<u8>, protocol: u8, id: u32) {
    if protocol == 0 {
        writeln!(out, "g{id}").unwrap();
    } else if let Ok(id) = u8::try_from(id) {
        out.extend([b'h', id]);
    } else {
        out.push(b'j');
        out.extend(id.to_le_bytes());
    }
}

pub(super) fn global(
    out: &mut Vec<u8>,
    protocol: u8,
    module: &str,
    name: &str,
    fix_imports: bool,
) -> Result<()> {
    debug_assert!(protocol < 4);
    let (module, name) = if protocol < 3 && fix_imports {
        legacy_global(module, name)?
    } else {
        (module, name)
    };
    if module.contains('\n') || name.contains('\n') {
        return Err(Error::new(
            "PicklingError",
            "pickle GLOBAL identifiers cannot contain newlines",
        ));
    }
    if protocol < 3 && (!module.is_ascii() || !name.is_ascii()) {
        return Err(Error::new(
            "PicklingError",
            format!(
                "non-ASCII pickle GLOBAL identifiers need protocol 3 or later (using {protocol})"
            ),
        ));
    }
    out.push(b'c'); // GLOBAL
    out.extend_from_slice(module.as_bytes());
    out.push(b'\n');
    out.extend_from_slice(name.as_bytes());
    out.push(b'\n');
    Ok(())
}

// Protocols 0–2 use _compat_pickle's Python 2 names when fix_imports is enabled.
// Other modules that need a legacy translation must be handled explicitly before
// the generic, unchanged module name can be used.
fn legacy_global<'a>(module: &'a str, name: &'a str) -> Result<(&'a str, &'a str)> {
    let unsupported = || {
        Error::new(
            "NotImplementedError",
            format!("legacy pickle name for {module}.{name}"),
        )
    };
    if module == "builtins" {
        return Ok(match name {
            "int" => ("__builtin__", "long"),
            "str" => ("__builtin__", "unicode"),
            "chr" => ("__builtin__", "unichr"),
            "range" => ("__builtin__", "xrange"),
            "zip" => ("itertools", "izip"),
            "map" => ("itertools", "imap"),
            "filter" => ("itertools", "ifilter"),
            "bool" | "bytearray" | "bytes" | "complex" | "dict" | "float" | "frozenset"
            | "getattr" | "iter" | "list" | "object" | "set" | "slice" | "super" | "tuple"
            | "type" | "Ellipsis" | "NotImplemented" => ("__builtin__", name),
            _ if builtins::is_builtin_function(name) => ("__builtin__", name),
            _ => return Err(unsupported()),
        });
    }
    if matches!(module, "_functools" | "functools") && name == "reduce" {
        return Ok(("__builtin__", "reduce"));
    }
    if module == "itertools" {
        return Ok(match name {
            "filterfalse" => (module, "ifilterfalse"),
            "zip_longest" => (module, "izip_longest"),
            _ => (module, name),
        });
    }
    if module == "collections" {
        return Ok(match name {
            "UserDict" => ("UserDict", "IterableUserDict"),
            "UserList" => ("UserList", "UserList"),
            "UserString" => ("UserString", "UserString"),
            _ => (module, name),
        });
    }
    if module == "collections.abc" {
        return Ok(("_abcoll", name));
    }
    if module == "copyreg" {
        return Ok(("copy_reg", name));
    }
    const LEGACY_MODULES: &[&str] = &[
        "_bz2",
        "_dbm",
        "_dummy_thread",
        "_functools",
        "_gdbm",
        "_markupbase",
        "_pickle",
        "_socket",
        "_thread",
        "configparser",
        "dbm",
        "dbm.bsd",
        "dbm.dumb",
        "dbm.gnu",
        "dbm.ndbm",
        "html.entities",
        "html.parser",
        "http.client",
        "http.cookiejar",
        "http.cookies",
        "http.server",
        "multiprocessing.connection",
        "multiprocessing.context",
        "multiprocessing.popen_fork",
        "queue",
        "reprlib",
        "socket",
        "socketserver",
        "subprocess",
        "sys",
        "test.support",
        "tkinter",
        "tkinter.colorchooser",
        "tkinter.commondialog",
        "tkinter.constants",
        "tkinter.dialog",
        "tkinter.dnd",
        "tkinter.filedialog",
        "tkinter.font",
        "tkinter.messagebox",
        "tkinter.scrolledtext",
        "tkinter.simpledialog",
        "tkinter.ttk",
        "urllib.error",
        "urllib.parse",
        "urllib.request",
        "urllib.robotparser",
        "winreg",
        "xmlrpc.client",
        "xmlrpc.server",
    ];
    if LEGACY_MODULES.contains(&module) {
        Err(unsupported())
    } else {
        Ok((module, name))
    }
}
