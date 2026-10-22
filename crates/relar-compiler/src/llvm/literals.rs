//! Program-owned literals share immutable strings and compiled regexes between evaluations.

use super::*;

impl Emitter<'_, '_> {
    /// Returns an owned reference while the pool keeps its root until main exits.
    pub(super) fn pooled_literal(&mut self, contents: &[u8], ty: Type) -> Value {
        let id = self.intern_literal(contents, ty);
        let value = Value {
            ty,
            operand: self.instruction(format!("load ptr, ptr @literal_{id}")),
            small_integer: false,
        };
        self.retain(&value);
        value
    }

    pub(super) fn intern_literal(&mut self, contents: &[u8], ty: Type) -> usize {
        let pool = match ty {
            Type::Str => &mut self.string_literals,
            Type::RegexPattern => &mut self.regex_literals,
            _ => unreachable!("only strings and regexes have pooled literals"),
        };
        if let Some(&id) = pool.get(contents) {
            id
        } else {
            let id = self.strings.len();
            self.strings.push(contents.to_vec());
            pool.insert(contents.to_vec(), id);
            id
        }
    }

    /// Eager initialization has no user effects and bounds retention by source
    /// constants. Main releases locals/globals first, then these final roots on
    /// both successful and exceptional exits; there is no process-lifetime leak.
    pub(super) fn literal_pool(&self) -> String {
        let mut output = String::new();
        let literals = [
            (
                &self.string_literals,
                "relar_str_literal",
                "relar_str_release",
            ),
            (
                &self.regex_literals,
                "relar_regex_compile",
                "relar_object_release",
            ),
        ]
        .into_iter()
        .flat_map(|(pool, init, release)| {
            pool.iter()
                .map(move |(bytes, id)| (bytes, id, init, release))
        });
        for (_, id, _, _) in literals.clone() {
            writeln!(output, "@literal_{id} = internal global ptr null").unwrap();
        }
        let attributes = if self.sanitize {
            " sanitize_address"
        } else {
            ""
        };
        writeln!(
            output,
            "\ndefine internal void @relar_init_literals(){attributes} {{\nentry:"
        )
        .unwrap();
        for (bytes, id, init, _) in literals.clone() {
            writeln!(
                output,
                "  %literal_{id} = call ptr @{init}(ptr @str_{id}, i64 {})\n  store ptr %literal_{id}, ptr @literal_{id}",
                bytes.len()
            )
            .unwrap();
        }
        if !self.enum_members.is_empty() {
            writeln!(
                output,
                "  call void @relar_str_enum_table(ptr @str_enum_members, i64 {})",
                self.enum_members.len()
            )
            .unwrap();
        }
        writeln!(
            output,
            "  ret void\n}}\n\ndefine internal void @relar_clear_literals(){attributes} {{\nentry:"
        )
        .unwrap();
        if !self.enum_members.is_empty() {
            output.push_str("  call void @relar_str_enum_table(ptr null, i64 0)\n");
        }
        for (_, id, _, release) in literals {
            writeln!(
                output,
                "  %literal_{id} = load ptr, ptr @literal_{id}\n  store ptr null, ptr @literal_{id}\n  call void @{release}(ptr %literal_{id})"
            )
            .unwrap();
        }
        output.push_str("  ret void\n}\n");
        output
    }
}
