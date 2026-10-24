//! Program-owned literals share immutable strings and compiled regexes between evaluations.

use super::*;

impl Emitter<'_, '_> {
    /// Returns an owned reference while the pool keeps its root until main exits.
    pub(super) fn pooled_literal(&mut self, contents: &[u8], ty: Type) -> Value {
        let id = self.intern_literal(contents, ty);
        let name = self.output.literal_ref(id);
        let value = Value {
            ty,
            operand: self.instruction(format!("load ptr, ptr @{name}")),
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
            self.output.add_string();
            // The entry bootstrap initializes and clears every pooled root.
            self.output.use_literal_from_entry(id);
            pool.insert(contents.to_vec(), id);
            id
        }
    }

    /// Eager initialization has no user effects and bounds retention by source
    /// constants. Main releases locals/globals first, then these final roots on
    /// both successful and exceptional exits; there is no process-lifetime leak.
    pub(super) fn literal_pool(&mut self) {
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
            let owner = self.output.string_owner(*id);
            let name = self.output.string_name(*id, "literal");
            self.output
                .global(owner, &name, "global", "ptr")
                .push_str("null\n");
        }
        let attributes = if self.sanitize {
            " sanitize_address"
        } else {
            ""
        };
        self.output.common().push('\n');
        writeln!(
            self.output.entry_buffer(),
            "define internal void @relar_init_literals(){attributes} {{\nentry:"
        )
        .unwrap();
        for (bytes, id, init, _) in literals.clone() {
            let bytes_name = self.output.string_name(*id, "str");
            let root = self.output.string_name(*id, "literal");
            writeln!(
                self.output.entry_buffer(),
                "  %literal_{id} = call ptr @{init}(ptr @{bytes_name}, i64 {})\n  store ptr %literal_{id}, ptr @{root}",
                bytes.len()
            )
            .unwrap();
        }
        if !self.enum_members.is_empty() {
            writeln!(
                self.output.entry_buffer(),
                "  call void @relar_str_enum_table(ptr @str_enum_members, i64 {})",
                self.enum_members.len()
            )
            .unwrap();
        }
        self.output.entry_buffer().push_str("  ret void\n}\n");
        self.output.common().push('\n');
        writeln!(
            self.output.entry_buffer(),
            "define internal void @relar_clear_literals(){attributes} {{\nentry:"
        )
        .unwrap();
        if !self.enum_members.is_empty() {
            self.output
                .entry_buffer()
                .push_str("  call void @relar_str_enum_table(ptr null, i64 0)\n");
        }
        for (_, id, _, release) in literals {
            let root = self.output.string_name(*id, "literal");
            writeln!(
                self.output.entry_buffer(),
                "  %literal_{id} = load ptr, ptr @{root}\n  store ptr null, ptr @{root}\n  call void @{release}(ptr %literal_{id})"
            )
            .unwrap();
        }
        self.output.entry_buffer().push_str("  ret void\n}\n");
    }
}
