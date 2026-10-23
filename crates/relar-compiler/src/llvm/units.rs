//! Partition Relar's emitted LLVM definitions. Only symbols referenced from another
//! unit lose local linkage; record descriptors and mutable roots keep one address.
use std::collections::{HashMap, HashSet};

use super::*;
use crate::CodegenUnit;

pub(super) struct Owners<'a> {
    pub functions: &'a [usize],
    pub entry: usize,
    /// Each byte array and its optional pooled string/regex root share an ID.
    pub strings: Vec<usize>,
}

struct Definition<'a> {
    name: &'a str,
    unit: usize,
    body: &'a str,
}

fn symbols(text: &str) -> impl Iterator<Item = (usize, &str)> {
    text.match_indices('@').map(move |(start, _)| {
        let tail = &text[start + 1..];
        let end = tail
            .bytes()
            .take_while(|c| c.is_ascii_alphanumeric() || b"_.$-".contains(c))
            .count();
        (start, &tail[..end])
    })
}

fn numbered(name: &str, prefix: &str) -> Option<usize> {
    name.strip_prefix(prefix)?.parse().ok()
}

fn adapter(name: &str) -> Option<(usize, &str)> {
    let (id, flavor) = name.strip_prefix("callable_")?.split_once('_')?;
    Some((id.parse().ok()?, flavor))
}

impl Owners<'_> {
    fn function(&self, name: &str) -> Option<usize> {
        if let Some(id) = numbered(name, "fn_").or_else(|| adapter(name).map(|(id, _)| id)) {
            Some(self.functions[id])
        } else if matches!(
            name,
            "main" | "relar_init_literals" | "relar_clear_literals"
        ) {
            Some(self.entry)
        } else {
            // Integer, sequence and nullable helpers are private, inlineable code
            // in every unit. They address the same externally declared state.
            None
        }
    }

    fn global(&self, name: &str) -> usize {
        numbered(name, "str_")
            .or_else(|| numbered(name, "literal_"))
            .map_or(self.entry, |i| self.strings[i])
    }
}

/// The LLVM emitter uses a global for each scalar, named type, array or struct.
/// Declarations must retain that exact type, including nested aggregate types.
fn global_type(value: &str) -> &str {
    let mut depth = 0usize;
    for (i, c) in value.bytes().enumerate() {
        match c {
            b'[' | b'{' => depth += 1,
            b']' | b'}' => depth -= 1,
            b' ' if depth == 0 => return &value[..i],
            _ => {}
        }
    }
    value
}

impl Definition<'_> {
    fn declaration(&self) -> String {
        if self.body.starts_with("define ") {
            let header = self.body.lines().next().unwrap();
            let signature = header
                .strip_prefix("define internal ")
                .expect("generated cross-unit function")
                .split_once(')')
                .unwrap()
                .0;
            return format!("declare hidden {signature})\n");
        }
        let (_, rhs) = self.body.split_once(" = ").unwrap();
        let (kind, value) = rhs
            .split_once("constant ")
            .map(|(_, v)| ("constant", v))
            .or_else(|| rhs.split_once("global ").map(|(_, v)| ("global", v)))
            .expect("generated global or constant");
        format!(
            "@{} = external hidden {kind} {}\n",
            self.name,
            global_type(value)
        )
    }

    fn exported(&self) -> String {
        if self.body.starts_with("define ") {
            self.body.replacen("define internal ", "define hidden ", 1)
        } else {
            let (name, rhs) = self.body.split_once(" = ").unwrap();
            let exported = rhs
                .strip_prefix("internal ")
                .or_else(|| rhs.strip_prefix("private "))
                .unwrap();
            format!("{name} = hidden {exported}")
        }
    }
}

/// Number generated symbols within their source file so introducing a literal
/// or helper in an earlier module doesn't renumber references throughout later units.
fn local_ids(owners: &[usize], units: usize) -> Vec<String> {
    let mut next = vec![0; units];
    owners
        .iter()
        .map(|&m| {
            let id = next[m];
            next[m] += 1;
            format!("m{m}_{id}")
        })
        .collect()
}

fn rename(code: &str, names: &HashMap<&str, String>) -> String {
    let mut out = String::with_capacity(code.len());
    let mut last = 0;
    for (at, name) in symbols(code) {
        if let Some(mapped) = names.get(name) {
            out.push_str(&code[last..at + 1]);
            out.push_str(mapped);
            last = at + 1 + name.len();
        }
    }
    out.push_str(&code[last..]);
    out
}

pub(crate) fn emit_units(
    module: &Module,
    ranges: &RangeFacts<'_>,
    sanitize: bool,
    functions: &[usize],
    names: &[String],
    entry: usize,
) -> Result<Vec<CodegenUnit>, String> {
    let mut owners = Owners {
        functions,
        entry,
        strings: Vec::new(),
    };
    let ir = emit_program(module, ranges, sanitize, Some(&mut owners))?;
    let mut remaining = ir.as_str();
    let mut common = String::from("source_filename = \"relar-unit\"\n");
    let mut common_refs = HashSet::new();
    let mut defs = Vec::new();
    let mut refs = vec![HashSet::new(); names.len()];
    // Relar emits its own restricted LLVM syntax: function headers and top-level
    // globals are single lines, and function-closing braces begin a line.
    while !remaining.is_empty() {
        let function = remaining.starts_with("define ");
        let end = if function {
            remaining
                .find("\n}\n")
                .ok_or("unterminated emitted LLVM function")?
                + 3
        } else {
            remaining.find('\n').map_or(remaining.len(), |end| end + 1)
        };
        let (body, rest) = remaining.split_at(end);
        remaining = rest;
        let origin = if function || body.starts_with('@') {
            let (_, name) = symbols(body).next().ok_or("missing emitted LLVM symbol")?;
            let unit = if function {
                owners.function(name)
            } else {
                Some(owners.global(name))
            };
            unit.map(|unit| Definition { name, unit, body })
        } else {
            None
        };
        if let Some(def) = origin {
            refs[def.unit].extend(symbols(body).skip(1).map(|(_, name)| name));
            defs.push(def);
        } else {
            common.push_str(body);
            common_refs.extend(symbols(body).map(|(_, name)| name));
        }
    }
    let function_ids = local_ids(functions, names.len());
    let string_ids = local_ids(&owners.strings, names.len());
    let mut renames = HashMap::new();
    for def in &defs {
        let name = def.name;
        let mapped = if let Some(id) = numbered(name, "fn_") {
            Some(format!("fn_{}", function_ids[id]))
        } else if let Some((id, flavor)) = adapter(name) {
            Some(format!("callable_{}_{flavor}", function_ids[id]))
        } else if let Some(id) = numbered(name, "str_") {
            Some(format!("str_{}", string_ids[id]))
        } else {
            numbered(name, "literal_").map(|id| format!("literal_{}", string_ids[id]))
        };
        if let Some(mapped) = mapped {
            renames.insert(name, mapped);
        }
    }
    let mut units = vec![common; names.len()];
    for def in &defs {
        let shared = (names.len() > 1 && common_refs.contains(def.name))
            || refs
                .iter()
                .enumerate()
                .any(|(unit, used)| unit != def.unit && used.contains(def.name));
        if shared {
            units[def.unit].push_str(&def.exported());
            let declaration = def.declaration();
            for (unit, output) in units.iter_mut().enumerate() {
                if unit != def.unit
                    && (common_refs.contains(def.name) || refs[unit].contains(def.name))
                {
                    output.push_str(&declaration);
                }
            }
        } else {
            units[def.unit].push_str(def.body);
        }
    }
    Ok(names
        .iter()
        .zip(units)
        .map(|(name, ir)| CodegenUnit {
            name: name.clone(),
            llvm_ir: rename(&ir, &renames),
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use std::fs;

    #[test]
    fn literal_body_and_caller_abi_invalidate_their_own_units() {
        let temp = tempfile::tempdir().unwrap();
        let entry = temp.path().join("main.py");
        let leaf = temp.path().join("leaf.py");
        fs::write(&entry, "from leaf import label\nprint(label())\n").unwrap();
        let build = || {
            crate::compile_file_units(&entry, None, &crate::CompileOptions::default())
                .unwrap()
                .units
                .into_iter()
                .map(|u| (u.name, u.llvm_ir))
                .collect::<std::collections::HashMap<_, _>>()
        };
        fs::write(&leaf, "def label() -> str:\n    return 'alpha'\n").unwrap();
        let original = build();
        fs::write(&leaf, "def label() -> str:\n    return 'bravo'\n").unwrap();
        let edit = build();
        assert_eq!(original["main"], edit["main"]);
        assert_ne!(original["leaf"], edit["leaf"]);
        fs::write(&leaf, "def label() -> int:\n    return 1\n").unwrap();
        let abi = build();
        assert_ne!(edit["main"], abi["main"]);
        assert_ne!(edit["leaf"], abi["leaf"]);
    }
}
