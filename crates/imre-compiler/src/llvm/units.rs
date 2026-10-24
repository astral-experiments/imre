//! Write each definition to its owner and declare its symbol in the units that use it.
use std::collections::{HashMap, HashSet};
use std::fmt::{Display, Write};

use super::*;
use crate::CodegenUnit;

pub(super) struct Output {
    units: Vec<String>,
    common: Option<String>,
    pub entry: usize,
    pub current: usize,
    functions: Vec<(usize, usize)>,
    strings: Vec<(usize, usize)>,
    next_string: Vec<usize>,
    users: HashMap<String, HashSet<usize>>,
}

impl Output {
    pub fn new(functions: &[usize], count: usize, entry: usize, split: bool) -> Self {
        let mut next = vec![0; count];
        let functions = functions
            .iter()
            .map(|&owner| {
                let local = next[owner];
                next[owner] += 1;
                (owner, local)
            })
            .collect();
        Self {
            units: vec![String::new(); count],
            common: split.then(|| "source_filename = \"imre-unit\"\n".to_owned()),
            entry,
            current: entry,
            functions,
            strings: Vec::new(),
            next_string: vec![0; count],
            users: HashMap::new(),
        }
    }

    pub fn split(&self) -> bool {
        self.common.is_some()
    }

    pub fn common(&mut self) -> &mut String {
        self.common.as_mut().unwrap_or(&mut self.units[self.entry])
    }

    pub fn entry_buffer(&mut self) -> &mut String {
        &mut self.units[self.entry]
    }

    pub fn function_owner(&self, id: usize) -> usize {
        self.functions[id].0
    }

    pub fn add_string(&mut self) {
        let next = &mut self.next_string[self.current];
        self.strings.push((self.current, *next));
        *next += 1;
    }

    pub fn string_owner(&self, id: usize) -> usize {
        self.strings[id].0
    }

    fn numbered(&self, prefix: &str, id: usize, (owner, local): (usize, usize)) -> String {
        if self.split() {
            format!("{prefix}_m{owner}_{local}")
        } else {
            format!("{prefix}_{id}")
        }
    }

    pub fn function_name(&self, id: usize) -> String {
        self.numbered("fn", id, self.functions[id])
    }

    pub fn adapter_name(&self, id: usize, bound: bool) -> String {
        let name = self.numbered("callable", id, self.functions[id]);
        format!("{name}_{}", if bound { "bound" } else { "plain" })
    }

    pub fn string_name(&self, id: usize, prefix: &str) -> String {
        self.numbered(prefix, id, self.strings[id])
    }

    // Uses are recorded during lowering. By the time definitions are written,
    // every owner knows its required linkage and which callers need a declaration.
    pub fn used(&mut self, owner: usize, name: String) -> String {
        if self.current != owner {
            self.users
                .entry(name.clone())
                .or_default()
                .insert(self.current);
        }
        name
    }

    pub fn global_ref(&mut self, name: String) -> String {
        self.used(self.entry, name)
    }

    pub fn use_literal_from_entry(&mut self, id: usize) {
        if self.current != self.entry {
            for prefix in ["str", "literal"] {
                self.users
                    .entry(self.string_name(id, prefix))
                    .or_default()
                    .insert(self.entry);
            }
        }
    }

    pub fn literal_ref(&mut self, id: usize) -> String {
        self.used(self.string_owner(id), self.string_name(id, "literal"))
    }

    pub fn function_ref(&mut self, id: usize) -> String {
        self.used(self.function_owner(id), self.function_name(id))
    }

    pub fn adapter_ref(&mut self, id: usize, bound: bool) -> String {
        self.used(self.function_owner(id), self.adapter_name(id, bound))
    }

    pub fn shared_global(&mut self, name: &str, ty: &str, value: &str) {
        if self.units.len() > 1 {
            self.users.insert(
                name.to_owned(),
                (0..self.units.len()).filter(|&u| u != self.entry).collect(),
            );
        }
        writeln!(self.global(self.entry, name, "global", ty), "{value}").unwrap();
    }

    pub fn global(
        &mut self,
        owner: usize,
        name: &str,
        kind: &str,
        ty: impl Display,
    ) -> &mut String {
        let linkage = if let Some(users) = self.users.get(name) {
            for &unit in users {
                let declaration = if kind == "global" {
                    "global"
                } else {
                    "constant"
                };
                writeln!(
                    self.units[unit],
                    "@{name} = external hidden {declaration} {ty}"
                )
                .unwrap();
            }
            "hidden"
        } else if kind == "global" {
            "internal"
        } else {
            "private"
        };
        write!(self.units[owner], "@{name} = {linkage} {kind} {ty} ").unwrap();
        &mut self.units[owner]
    }

    pub fn function(
        &mut self,
        owner: usize,
        name: &str,
        returns: &str,
        params: &str,
        attrs: &str,
    ) -> &mut String {
        let linkage = if let Some(users) = self.users.get(name) {
            for &unit in users {
                writeln!(
                    self.units[unit],
                    "declare hidden {returns} @{name}({params})"
                )
                .unwrap();
            }
            "hidden "
        } else if name == "main" {
            ""
        } else {
            "internal "
        };
        writeln!(
            self.units[owner],
            "define {linkage}{returns} @{name}({params}){attrs} {{"
        )
        .unwrap();
        &mut self.units[owner]
    }

    fn finish(self) -> Vec<String> {
        let Some(common) = self.common else {
            return self.units;
        };
        self.units
            .into_iter()
            .map(|body| {
                let mut unit = String::with_capacity(common.len() + body.len());
                unit.push_str(&common);
                unit.push_str(&body);
                unit
            })
            .collect()
    }
}

pub(crate) fn emit_units(
    module: &Module,
    ranges: &RangeFacts<'_>,
    sanitize: bool,
    functions: &[usize],
    names: &[String],
    entry: usize,
) -> Result<Vec<CodegenUnit>, String> {
    let mut output = Output::new(functions, names.len(), entry, true);
    emit_program(module, ranges, sanitize, &mut output)?;
    Ok(names
        .iter()
        .zip(output.finish())
        .map(|(name, llvm_ir)| CodegenUnit {
            name: name.clone(),
            llvm_ir,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use std::fs;

    #[test]
    fn adapters_and_literal_roots_declare_only_their_foreign_uses() {
        let temp = tempfile::tempdir().unwrap();
        let entry = temp.path().join("main.py");
        fs::write(
            &entry,
            "from leaf import choose\nfrom spare import count\nprint(choose()())\nprint(count())\n",
        )
        .unwrap();
        fs::write(temp.path().join("leaf.py"), "from typing import Callable\ndef label() -> str:\n    return 'hello'\ndef choose() -> Callable[[], str]:\n    return label\n").unwrap();
        fs::write(
            temp.path().join("spare.py"),
            "def count() -> int:\n    return 3\n",
        )
        .unwrap();
        let units = crate::compile_file_units(&entry, None, &crate::CompileOptions::default())
            .unwrap()
            .units
            .into_iter()
            .map(|u| (u.name, u.llvm_ir))
            .collect::<std::collections::HashMap<_, _>>();
        assert!(units["leaf"].contains("define hidden ptr @callable_m"));
        assert!(units["main"].contains("declare hidden ptr @callable_m"));
        assert!(
            units["leaf"].contains("@layout_callable = external hidden constant { i64, ptr, ptr }")
        );
        assert!(units["main"].contains("@layout_callable = hidden constant"));
        assert!(units["main"].contains(" = external hidden constant [5 x i8]"));
        assert!(units["main"].contains(" = external hidden global ptr"));
        assert!(!units["spare"].contains("@callable_m"));
        assert!(!units["spare"].contains("@literal_m"));
    }

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
