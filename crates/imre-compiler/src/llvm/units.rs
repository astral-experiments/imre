//! Write each definition to its owner and declare its symbol in the units that use it.
use std::collections::{HashMap, HashSet};
use std::fmt::{Display, Write};

use super::*;
use crate::CodegenUnit;

pub(super) struct Output {
    /// Whether a reference to this named type also needs its destruction descriptor.
    pub(super) record_types: HashMap<Type, bool>,
    units: Vec<String>,
    integers: Vec<body::IntegerFunctions>,
    common: Option<String>,
    pub entry: usize,
    pub current: usize,
    functions: Vec<(usize, usize)>,
    strings: Vec<(usize, usize)>,
    next_string: Vec<usize>,
    users: HashMap<String, HashSet<usize>>,
    local_helpers: HashSet<(usize, &'static str)>,
}

impl Output {
    fn new(
        functions: impl IntoIterator<Item = usize>,
        count: usize,
        entry: usize,
        split: bool,
    ) -> Self {
        #[cfg(test)]
        crate::single_emission_tests::record_llvm_start();
        let mut next = vec![0; count];
        let functions = functions
            .into_iter()
            .map(|owner| {
                let local = next[owner];
                next[owner] += 1;
                (owner, local)
            })
            .collect();
        Self {
            record_types: HashMap::new(),
            units: vec![String::new(); count],
            integers: vec![body::IntegerFunctions::default(); count],
            common: split.then(|| "source_filename = \"imre-unit\"\n".to_owned()),
            entry,
            current: entry,
            functions,
            strings: Vec::new(),
            next_string: vec![0; count],
            users: HashMap::new(),
            local_helpers: HashSet::new(),
        }
    }

    pub(super) fn whole(functions: usize) -> Self {
        Self::new(std::iter::repeat_n(0, functions), 1, 0, false)
    }

    pub(super) fn include_integers(&mut self, function: body::IntegerFunctions) {
        self.integers[self.current].include(function);
    }

    pub fn split(&self) -> bool {
        self.common.is_some()
    }

    pub fn common(&mut self) -> &mut String {
        self.common.as_mut().unwrap_or(&mut self.units[self.entry])
    }

    pub fn local_helper(&mut self, name: &'static str, definition: &str) {
        if self.local_helpers.insert((self.current, name)) {
            self.units[self.current].push_str(definition);
        }
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

    pub fn async_name(&self, id: usize) -> String {
        format!(
            "{}_async",
            self.numbered("callable", id, self.functions[id])
        )
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

    /// Record a named LLVM type used in a field access or allocation.
    pub fn record_type(&mut self, ty: Type) -> String {
        self.record_types.entry(ty).or_insert(false);
        record_name(ty)
    }

    /// Reference the destruction descriptor from the current codegen unit.
    pub fn record_layout(&mut self, ty: Type) -> String {
        self.record_types.insert(ty, true);
        self.global_ref(format!("layout_{}", record_name(ty)))
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

    pub fn byte_constant(&mut self, owner: usize, name: &str, bytes: &[u8]) {
        let output = self.global(
            owner,
            name,
            "unnamed_addr constant",
            format_args!("[{} x i8]", bytes.len()),
        );
        output.push_str("c\"");
        for byte in bytes {
            write!(output, "\\{byte:02X}").unwrap();
        }
        output.push_str("\"\n");
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

    pub(super) fn emit(
        mut self,
        module: &Module,
        ranges: &RangeFacts<'_>,
        sanitize: bool,
        program: NativeProgram,
    ) -> Result<Vec<String>, Unsupported> {
        emit_program(module, ranges, sanitize, &mut self, program)?;
        for (integers, unit) in self.integers.into_iter().zip(&mut self.units) {
            integers.write(unit, false);
        }
        let Some(common) = self.common else {
            return Ok(self.units);
        };
        Ok(self
            .units
            .into_iter()
            .map(|body| {
                let mut unit = String::with_capacity(common.len() + body.len());
                unit.push_str(&common);
                unit.push_str(&body);
                unit
            })
            .collect())
    }
}

pub(crate) fn emit_units(
    graph: &crate::modules::SourceGraph,
    (module, owners): crate::CheckedIr,
    program: NativeProgram,
    layout: &crate::CodegenLayout,
    sanitize: bool,
) -> Result<Vec<CodegenUnit>, Unsupported> {
    let split = matches!(layout, crate::CodegenLayout::Split);
    let output = if split {
        Output::new(owners, graph.modules.len(), graph.entry, true)
    } else {
        Output::whole(module.functions.len())
    };
    let ranges = crate::ranges::analyze(&module);
    Ok(output
        .emit(&module, &ranges, sanitize, program)?
        .into_iter()
        .enumerate()
        .map(|(id, llvm_ir)| CodegenUnit {
            name: if split {
                graph.modules[id].name.clone()
            } else {
                "__main__".to_owned()
            },
            llvm_ir,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use std::fs;

    #[test]
    fn loop_helpers_are_local_to_the_unit_that_calls_them() {
        let temp = tempfile::tempdir().unwrap();
        let entry = temp.path().join("main.py");
        fs::write(
            &entry,
            "from loops import add_rows\nprint(add_rows(0, 10, 0))\n",
        )
        .unwrap();
        fs::write(
            temp.path().join("loops.py"),
            include_str!("../../../../tests/fixtures/primitives/affine_range.py"),
        )
        .unwrap();
        let program =
            crate::compile_file_units(&entry, None, &crate::CompileOptions::default()).unwrap();
        assert_eq!(program.units.len(), 2);
        for unit in program.units {
            let uses = unit
                .llvm_ir
                .matches("call {i64,i64} @imre_affine_fixed_prefix")
                .count();
            let definitions = unit
                .llvm_ir
                .matches("define internal {i64,i64} @imre_affine_fixed_prefix")
                .count();
            assert_eq!(uses > 0, unit.name == "loops");
            assert_eq!(definitions, usize::from(uses > 0));
            assert!(!unit.llvm_ir.contains("@imre_repeat_fadd"));
        }
    }

    #[test]
    fn arithmetic_definitions_follow_calls_in_their_own_unit() {
        let temp = tempfile::tempdir().unwrap();
        let entry = temp.path().join("main.py");
        fs::write(
            &entry,
            "from math_ops import add, magnitude\nprint(add(1, magnitude(-2)))\n",
        )
        .unwrap();
        fs::write(temp.path().join("math_ops.py"), "def add(a: int, b: int) -> int:\n    return a + b\ndef magnitude(n: int) -> int:\n    return -n if n < 0 else n\n").unwrap();
        let compiled =
            crate::compile_file_units(&entry, None, &crate::CompileOptions::default()).unwrap();
        for unit in compiled.units {
            let ir = unit.llvm_ir;
            for helper in ["add", "sub", "mul", "mod", "abs", "abs_slow"] {
                let defined = ir
                    .matches(&format!("define internal i64 @int_{helper}("))
                    .count();
                assert_eq!(
                    defined,
                    usize::from(ir.contains(&format!("call i64 @int_{helper}("))),
                    "{}: {helper}",
                    unit.name
                );
            }
            assert_eq!(ir.contains("call i64 @int_add("), unit.name == "math_ops");
            assert_eq!(
                ir.matches("define internal i32 @compare_int(").count(),
                usize::from(ir.contains("call i32 @compare_int("))
            );
        }
    }

    #[test]
    fn tagged_floor_division_is_defined_only_in_its_calling_unit() {
        let temp = tempfile::tempdir().unwrap();
        let entry = temp.path().join("main.py");
        fs::write(&entry, "from ratios import divide\nprint(divide(-7, 3))\n").unwrap();
        fs::write(
            temp.path().join("ratios.py"),
            "def divide(a: int, b: int) -> int:\n    return (a // b) // b\n",
        )
        .unwrap();

        let compiled =
            crate::compile_file_units(&entry, None, &crate::CompileOptions::default()).unwrap();
        assert_eq!(compiled.units.len(), 2);
        for unit in compiled.units {
            let calls = unit.llvm_ir.matches("call i64 @int_floordiv(").count();
            assert_eq!(calls, if unit.name == "ratios" { 2 } else { 0 });
            assert_eq!(
                unit.llvm_ir
                    .matches("define internal i64 @int_floordiv(")
                    .count(),
                usize::from(calls > 0)
            );
        }
    }

    #[test]
    fn sequence_layout_follows_inline_reads() {
        let options = crate::CompileOptions::default();
        assert!(
            !crate::compile("print(42)", &options)
                .unwrap()
                .contains("@sequence_layout")
        );

        let temp = tempfile::tempdir().unwrap();
        let entry = temp.path().join("main.py");
        fs::write(&entry, "from lists import nonempty\nfrom spare import answer\nprint(nonempty([0.5]))\nprint(answer())\n").unwrap();
        fs::write(
            temp.path().join("lists.py"),
            "def nonempty(values: list[float]) -> bool:\n    return bool(values)\n",
        )
        .unwrap();
        fs::write(
            temp.path().join("spare.py"),
            "def answer() -> int:\n    return 42\n",
        )
        .unwrap();
        let units = crate::compile_file_units(&entry, None, &options)
            .unwrap()
            .units;
        assert_eq!(units.len(), 3);
        for unit in units {
            let ir = unit.llvm_ir;
            assert_eq!(
                ir.contains("define internal i32 @sequence_header"),
                unit.name == "lists"
            );
            match unit.name.as_str() {
                "lists" => assert!(ir.contains("@sequence_layout = external hidden global")),
                "main" => {
                    assert!(ir.contains("call void @imre_typed_start(i32 0, ptr @sequence_layout)"))
                }
                _ => assert!(!ir.contains("@sequence_layout")),
            }
        }
    }

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
    fn async_calls_and_exception_types_are_visible_to_their_callers() {
        let temp = tempfile::tempdir().unwrap();
        let entry = temp.path().join("main.py");
        fs::write(
            &entry,
            "import asyncio\nfrom leaf import fail\nasyncio.run(fail())\n",
        )
        .unwrap();
        fs::write(
            temp.path().join("leaf.py"),
            "class Failure(Exception):\n    pass\nasync def fail() -> None:\n    raise Failure('failed')\n",
        ).unwrap();
        let units = crate::compile_file_units(&entry, None, &crate::CompileOptions::default())
            .unwrap()
            .units
            .into_iter()
            .map(|u| (u.name, u.llvm_ir))
            .collect::<std::collections::HashMap<_, _>>();
        assert!(units["main"].contains("declare hidden i64 @callable_m"));
        assert!(units["leaf"].contains("define hidden i64 @callable_m"));
        assert!(units["leaf"].contains("@exception_type_256 = external hidden constant"));
        assert!(units["main"].contains("@exception_type_256 = hidden constant"));
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
