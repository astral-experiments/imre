//! Program-owned immutable strings avoid allocating at each literal evaluation.

use super::*;

impl Emitter<'_, '_> {
    /// Returns an independent owner of a string rooted until either main exit.
    /// Pooling changes no observable identity: string `is` is unsupported, and
    /// consuming concatenation still checks uniqueness before reusing storage.
    pub(super) fn string_literal(&mut self, contents: &[u8]) -> Value {
        let id = if let Some(&id) = self.string_literals.get(contents) {
            id
        } else {
            let id = self.strings.len();
            self.strings.push(contents.to_vec());
            self.string_literals.insert(contents.to_vec(), id);
            id
        };
        let value = Value {
            ty: Type::Str,
            operand: self.instruction(format!("load ptr, ptr @string_literal_{id}")),
            small_integer: false,
        };
        self.retain(&value);
        value
    }

    /// Eager initialization has no user effects and bounds retention by source
    /// constants. Main releases locals/globals first, then these final roots on
    /// both successful and exceptional exits; there is no process-lifetime leak.
    pub(super) fn string_literal_pool(&self) -> String {
        let mut output = String::new();
        for id in self.string_literals.values() {
            writeln!(output, "@string_literal_{id} = internal global ptr null").unwrap();
        }
        let attributes = if self.sanitize {
            " sanitize_address"
        } else {
            ""
        };
        writeln!(
            output,
            "\ndefine internal void @relar_init_string_literals(){attributes} {{\nentry:"
        )
        .unwrap();
        for (bytes, id) in &self.string_literals {
            writeln!(
                output,
                "  %literal_{id} = call ptr @relar_str_literal(ptr @str_{id}, i64 {})\n  store ptr %literal_{id}, ptr @string_literal_{id}",
                bytes.len()
            )
            .unwrap();
        }
        writeln!(
            output,
            "  ret void\n}}\n\ndefine internal void @relar_clear_string_literals(){attributes} {{\nentry:"
        )
        .unwrap();
        for id in self.string_literals.values() {
            writeln!(
                output,
                "  %literal_{id} = load ptr, ptr @string_literal_{id}\n  store ptr null, ptr @string_literal_{id}\n  call void @relar_str_release(ptr %literal_{id})"
            )
            .unwrap();
        }
        output.push_str("  ret void\n}\n");
        output
    }
}
