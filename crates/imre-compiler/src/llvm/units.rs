//! Write each definition to its owner and declare its symbol in the units that use it.
use std::collections::{HashMap, HashSet};
use std::fmt::{Display, Write};

use super::*;
use crate::CodegenUnit;

pub(super) struct Output<'units> {
    /// Whether a reference to this named type also needs its destruction descriptor.
    pub(super) record_types: HashMap<Type, bool>,
    units: &'units mut [CodegenUnit],
    integers: Vec<body::IntegerFunctions>,
    /// Named LLVM layouts must precede instructions that use them.
    common: String,
    split: bool,
    pub entry: usize,
    pub current: usize,
    functions: Vec<(usize, usize)>,
    strings: Vec<(usize, usize)>,
    next_string: Vec<usize>,
    users: HashMap<String, HashSet<usize>>,
    local_helpers: HashSet<(usize, &'static str)>,
}

impl<'units> Output<'units> {
    pub(super) fn new(
        functions: impl IntoIterator<Item = usize>,
        units: &'units mut [CodegenUnit],
        entry: usize,
        split: bool,
    ) -> Self {
        #[cfg(test)]
        crate::single_emission_tests::record_llvm_start(None);
        let count = units.len();
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
            units,
            integers: vec![body::IntegerFunctions::default(); count],
            common: if split {
                "source_filename = \"imre-unit\"\n"
            } else {
                ""
            }
            .to_owned(),
            split,
            entry,
            current: entry,
            functions,
            strings: Vec::new(),
            next_string: vec![0; count],
            users: HashMap::new(),
            local_helpers: HashSet::new(),
        }
    }

    pub(super) fn include_integers(&mut self, function: body::IntegerFunctions) {
        self.integers[self.current].include(function);
    }

    pub fn split(&self) -> bool {
        self.split
    }

    pub fn common(&mut self) -> &mut String {
        &mut self.common
    }

    pub fn local_helper(&mut self, name: &'static str, definition: &str) {
        if self.local_helpers.insert((self.current, name)) {
            self.units[self.current].llvm_ir.push_str(definition);
        }
    }

    pub fn entry_buffer(&mut self) -> &mut String {
        &mut self.units[self.entry].llvm_ir
    }

    fn function_location(&self, id: usize) -> (usize, usize) {
        if self.split() {
            self.functions[id]
        } else {
            (0, id)
        }
    }

    pub fn function_owner(&self, id: usize) -> usize {
        self.function_location(id).0
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
        let name = self.numbered("fn", id, self.function_location(id));
        if self.units.len() > 1 {
            format!("{name}.local")
        } else {
            name
        }
    }

    /// Finish the checked function in its owning unit as soon as its frame is complete.
    pub(super) fn finish_function(
        &mut self,
        id: usize,
        function: &Function,
        attrs: &str,
        code: &str,
    ) {
        let name = self.function_name(id);
        let owner = self.function_owner(id);
        let params = function
            .params
            .iter()
            .enumerate()
            .map(|(i, &slot)| format!("{} %arg{i}", function.locals[slot].llvm()))
            .collect::<Vec<_>>()
            .join(", ");
        self.units[owner].llvm_ir.push('\n');
        self.define_function(
            owner,
            &name,
            function.returns.llvm(),
            &params,
            attrs,
            |unit| {
                unit.push_str(code);
            },
        );
    }

    pub fn adapter_name(&self, id: usize, bound: bool) -> String {
        let name = self.numbered("callable", id, self.function_location(id));
        format!("{name}_{}", if bound { "bound" } else { "plain" })
    }

    pub fn async_name(&self, id: usize) -> String {
        format!(
            "{}_async",
            self.numbered("callable", id, self.function_location(id))
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

    /// A foreign use can appear after its body has finished, including a class descriptor.
    pub fn function_ref(&mut self, module: &Module, id: usize) -> String {
        let (owner, local) = self.function_location(id);
        let target = self.function_name(id);
        if self.current == owner {
            return target;
        }
        let symbol = self.numbered("fn", id, (owner, local));
        let users = self.users.entry(symbol.clone()).or_default();
        if users.insert(self.current) {
            let function = &module.functions[id];
            let returns = function.returns.llvm();
            let params = function
                .params
                .iter()
                .map(|&slot| function.locals[slot].llvm())
                .collect::<Vec<_>>()
                .join(", ");
            if users.len() == 1 {
                writeln!(
                    self.units[owner].llvm_ir,
                    "@{symbol} = hidden alias {returns} ({params}), ptr @{target}"
                )
                .unwrap();
            }
            writeln!(
                self.units[self.current].llvm_ir,
                "declare hidden {returns} @{symbol}({params})"
            )
            .unwrap();
        }
        symbol
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
        write_byte_string(output, bytes.iter().copied(), ByteEscapes::All);
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
                    self.units[unit].llvm_ir,
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
        write!(
            self.units[owner].llvm_ir,
            "@{name} = {linkage} {kind} {ty} "
        )
        .unwrap();
        &mut self.units[owner].llvm_ir
    }

    pub(super) fn define_function(
        &mut self,
        owner: usize,
        name: &str,
        returns: &str,
        params: &str,
        attrs: &str,
        write_body: impl FnOnce(&mut String),
    ) {
        let linkage = if let Some(users) = self.users.get(name) {
            for &unit in users {
                writeln!(
                    self.units[unit].llvm_ir,
                    "declare hidden {returns} @{name}({params})"
                )
                .unwrap();
            }
            "hidden"
        } else {
            "internal"
        };
        body::write_definition(
            &mut self.units[owner].llvm_ir,
            format_args!("{linkage} {returns} @{name}({params}){attrs}"),
            write_body,
        );
    }

    pub(super) fn emit(
        mut self,
        module: &Module,
        ranges: &RangeFacts<'_>,
        sanitize: bool,
        program: crate::callables::Layouts,
    ) {
        emit_program(module, ranges, sanitize, &mut self, program);
        for (integers, unit) in self.integers.into_iter().zip(self.units.iter_mut()) {
            integers.write(&mut unit.llvm_ir, false);
            unit.llvm_ir.insert_str(0, &self.common);
        }
    }
}

impl crate::ProgramOutput {
    pub(crate) fn emit_fixed(
        &mut self,
        graph: &crate::modules::SourceGraph,
        fixed: &mut crate::FixedProgram,
        sanitize: bool,
        program: crate::callables::Layouts,
    ) {
        fixed.prepare_native_sources(graph);
        let (module, owners) = &fixed.ir;
        let split = matches!(self.layout, crate::CodegenLayout::Split);
        let source = &graph.modules[graph.entry];
        let (sources, entry, owners) = if split {
            (
                graph.modules.as_slice(),
                (graph.entry, source.name.as_str()),
                owners.as_slice(),
            )
        } else {
            (std::slice::from_ref(source), (0, "__main__"), &[][..])
        };
        let Ok(()) = self.emit_sources(
            sources.iter().map(Ok::<_, std::convert::Infallible>),
            entry,
            split,
            program,
            |_, _, source, separate| Ok((source.name.as_str(), separate.then(String::new))),
            |program, units, entry| {
                let output = Output::new(owners.iter().copied(), units, entry, split);
                let ranges = crate::ranges::analyze(module);
                output.emit(module, &ranges, sanitize, program);
                Ok(None)
            },
        );
    }
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
            "from math_ops import add, magnitude\ndef negative(x: float) -> bool:\n    return x < 0\nprint(add(1, magnitude(-2)), negative(4))\n",
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
                ir.matches("define internal i32 @int_sign(").count(),
                usize::from(ir.contains("call i32 @int_sign("))
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
    fn a_missing_key_links_the_foreign_repr_discovered_after_its_body() {
        let temp = tempfile::tempdir().unwrap();
        let entry = temp.path().join("main.py");
        fs::write(
            &entry,
            "from leaf import Detail\nrecords = {Detail(1): 0}\nrecords[Detail(2)]\n",
        )
        .unwrap();
        fs::write(
            temp.path().join("leaf.py"),
            concat!(
                "from dataclasses import dataclass\n\n",
                "@dataclass(frozen=True)\n",
                "class Detail:\n",
                "    n: int\n",
                "    def __repr__(self) -> str:\n",
                "        return f\"detail-{self.n}\"\n",
            ),
        )
        .unwrap();
        let program =
            crate::compile_file_units(&entry, None, &crate::CompileOptions::default()).unwrap();
        assert!(
            program.units[0]
                .llvm_ir
                .contains("declare hidden ptr @fn_m1_0(ptr)")
        );
        let owner = &program.units[1].llvm_ir;
        assert!(owner.contains("@fn_m1_0 = hidden alias ptr (ptr), ptr @fn_m1_0.local"));
        assert!(owner.contains("define internal ptr @fn_m1_0.local("));
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
