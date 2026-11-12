//! Instruction output and basic-block state for one LLVM function.

use super::Binary;
use std::fmt::{Display, Write as _};

/// Definitions referenced by the checked instructions written to one LLVM unit.
#[derive(Clone, Copy, Default)]
pub(crate) struct IntegerFunctions {
    arithmetic: u32,
}

impl IntegerFunctions {
    pub(super) fn require(&mut self, op: Binary) {
        self.arithmetic |= 1 << op as u32;
    }

    pub(super) fn include(&mut self, other: Self) {
        self.arithmetic |= other.arithmetic;
    }

    pub(super) fn is_empty(self) -> bool {
        self.arithmetic == 0
    }

    pub(super) fn write(self, output: &mut String, declare_overflow: bool) {
        let uses = |op: Binary| self.arithmetic & (1 << op as u32) != 0;
        if uses(Binary::Modulo) {
            output.push_str(&super::division_helper(Binary::Modulo));
        }
        for op in [Binary::Add, Binary::Subtract, Binary::Multiply] {
            if uses(op) {
                super::write_integer_arithmetic(output, op, declare_overflow);
            }
        }
    }
}

#[derive(Clone)]
pub(crate) struct FunctionBody {
    pub(crate) code: String,
    /// Entry instructions whose storage requirements are discovered while emitting the body.
    pub(super) entry: String,
    pub(crate) current: String,
    /// Destination after the calling convention has handled its outstanding owners.
    pub(crate) error_target: String,
    pub(crate) terminated: bool,
    next_value: usize,
    next_label: usize,
    pub(super) integers: IntegerFunctions,
}

impl FunctionBody {
    pub(crate) fn new(entry: &str, error_target: &str) -> Self {
        Self {
            code: String::new(),
            entry: String::new(),
            current: entry.into(),
            error_target: error_target.into(),
            terminated: false,
            next_value: 0,
            next_label: 0,
            integers: IntegerFunctions::default(),
        }
    }

    /// Open the process entry in its owning unit. Only programs that consume
    /// the process arguments need them in the LLVM signature.
    pub(super) fn start_main(&mut self, argv: bool, attrs: &str) {
        let params = if argv { "i32 %argc, ptr %argv" } else { "" };
        writeln!(self.code, "define i32 @main({params}){attrs} {{").unwrap();
        self.start("entry");
    }

    /// The final runtime operation supplies the executable's exit status.
    pub(super) fn finish_main(&mut self, call: &str) {
        self.line(format!("%status = call i32 {call}"));
        self.end("ret i32 %status");
        self.code.push_str("}\n");
    }

    pub(super) fn reset_names(&mut self) {
        self.next_value = 0;
        self.next_label = 0;
    }

    pub(crate) fn name(&mut self, prefix: &str) -> String {
        let n = self.next_value;
        self.next_value += 1;
        format!("{prefix}{n}")
    }

    pub(super) fn label(&mut self) -> String {
        let n = self.next_label;
        self.next_label += 1;
        format!("b{n}")
    }

    pub(crate) fn line(&mut self, line: impl AsRef<str>) {
        writeln!(self.code, "  {}", line.as_ref()).unwrap();
    }

    pub(crate) fn instruction(&mut self, instruction: impl AsRef<str>) -> String {
        let value = self.name("%v");
        self.line(format!("{value} = {}", instruction.as_ref()));
        value
    }

    pub(super) fn masked_remainder(&mut self, dividend: &str, mask: i64) -> String {
        self.instruction(format!("and i64 {dividend}, {mask}"))
    }

    /// Both ABIs call the same tagged arithmetic helper after obtaining exact integers.
    pub(crate) fn integer_arithmetic(
        &mut self,
        op: super::Binary,
        left: &str,
        right: &str,
    ) -> super::Value {
        let helper = match op {
            super::Binary::Add => "int_add",
            super::Binary::Subtract => "int_sub",
            super::Binary::Multiply => "int_mul",
            super::Binary::Modulo => "int_mod",
            _ => unreachable!("expected checked integer operation"),
        };
        self.integers.require(op);
        super::Value {
            ty: super::Type::Int,
            operand: self.instruction(format!("call i64 @{helper}(i64 {left}, i64 {right})")),
            small_integer: false,
        }
    }

    pub(super) fn stack_slot(
        &mut self,
        name: impl Display,
        ty: impl Display,
        initial: Option<&str>,
    ) {
        write_slot(&mut self.code, name, ty, initial);
    }

    pub(super) fn entry_slot(
        &mut self,
        name: impl Display,
        ty: impl Display,
        initial: Option<&str>,
    ) {
        write_slot(&mut self.entry, name, ty, initial);
    }

    pub(crate) fn start(&mut self, label: &str) {
        writeln!(self.code, "{label}:").unwrap();
        self.current = label.into();
        self.terminated = false;
    }

    pub(crate) fn end(&mut self, terminator: impl AsRef<str>) {
        self.line(terminator);
        self.terminated = true;
    }

    pub(crate) fn jump(&mut self, label: &str) {
        self.end(format!("br label %{label}"));
    }

    pub(crate) fn branch(&mut self, condition: &str, yes: &str, no: &str) {
        self.conditional(condition, yes, no, None);
    }

    pub(super) fn conditional(
        &mut self,
        condition: &str,
        yes: &str,
        no: &str,
        expected: Option<bool>,
    ) {
        let metadata = match expected {
            Some(true) => ", !prof !1",
            Some(false) => ", !prof !0",
            None => "",
        };
        self.end(format!(
            "br i1 {condition}, label %{yes}, label %{no}{metadata}"
        ));
    }
}

/// A parameter or scratch slot has no initial owner. Other slots start with the
/// empty value of their representation before any path can release or replace it.
fn write_slot(out: &mut String, name: impl Display, ty: impl Display, initial: Option<&str>) {
    writeln!(out, "  {name} = alloca {ty}").unwrap();
    if let Some(value) = initial {
        writeln!(out, "  store {ty} {value}, ptr {name}").unwrap();
    }
}
