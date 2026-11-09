//! Instruction output and basic-block state for one LLVM function.

use std::fmt::Write as _;

pub(crate) struct FunctionBody {
    pub(crate) code: String,
    pub(crate) current: String,
    /// Destination after the calling convention has handled its outstanding owners.
    pub(crate) error_target: String,
    pub(crate) terminated: bool,
    next_value: usize,
    next_label: usize,
}

impl FunctionBody {
    pub(crate) fn new(entry: &str, error_target: &str) -> Self {
        Self {
            code: String::new(),
            current: entry.into(),
            error_target: error_target.into(),
            terminated: false,
            next_value: 0,
            next_label: 0,
        }
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
