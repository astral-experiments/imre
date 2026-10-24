//! Program-owned literals share immutable values between evaluations.

use super::*;

fn lazy_constructor(ty: Type) -> Option<&'static str> {
    match ty {
        Type::Bytes => Some("relar_bytes_literal"),
        Type::Int => Some("relar_int_literal"),
        _ => None,
    }
}

impl Emitter<'_, '_> {
    /// Returns an owned reference while the pool keeps its root until main exits.
    pub(super) fn pooled_literal(&mut self, contents: &[u8], ty: Type) -> Value {
        let id = self.intern_literal(contents, ty);
        let name = self.output.literal_ref(id);
        let repr = ty.llvm();
        let operand = if let Some(init) = lazy_constructor(ty) {
            let bytes = self.output.string_name(id, "str");
            let bytes = self.output.used(self.output.string_owner(id), bytes);
            self.instruction(format!(
                "call {repr} @{init}_once(ptr @{name}, ptr @{bytes}, i64 {})",
                contents.len()
            ))
        } else {
            self.instruction(format!("load {repr}, ptr @{name}"))
        };
        let value = Value {
            ty,
            operand,
            small_integer: false,
        };
        self.retain(&value);
        value
    }

    pub(super) fn intern_literal(&mut self, contents: &[u8], ty: Type) -> usize {
        let pool = match ty {
            Type::Str => &mut self.string_literals,
            Type::Bytes => &mut self.bytes_literals,
            Type::Int => &mut self.integer_literals,
            Type::RegexPattern => &mut self.regex_literals,
            _ => unreachable!("unsupported pooled literal type"),
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

    /// Main releases the literal roots after locals/globals on either exit.
    /// Bytes and heap integers initialize on first use to preserve cold branches.
    pub(super) fn literal_pool(&mut self) {
        let pools = [
            (
                &self.string_literals,
                Type::Str,
                "relar_str_literal",
                "relar_str_release",
            ),
            (
                &self.regex_literals,
                Type::RegexPattern,
                "relar_regex_compile",
                "relar_object_release",
            ),
            (
                &self.bytes_literals,
                Type::Bytes,
                "relar_bytes_literal",
                "relar_bytes_release",
            ),
            (
                &self.integer_literals,
                Type::Int,
                "relar_int_literal",
                "relar_int_release",
            ),
        ];
        let literals = pools.into_iter().flat_map(|(pool, ty, init, release)| {
            pool.iter()
                .map(move |(bytes, id)| (bytes, id, ty, init, release))
        });
        for (_, id, ty, _, _) in literals.clone() {
            let owner = self.output.string_owner(*id);
            let name = self.output.string_name(*id, "literal");
            writeln!(
                self.output.global(owner, &name, "global", ty.llvm()),
                "{}",
                ty.initial()
            )
            .unwrap();
        }
        let attributes = if self.sanitize {
            " sanitize_address"
        } else {
            ""
        };
        for (pool, ty, init, _) in pools {
            if pool.is_empty() || lazy_constructor(ty).is_none() {
                continue;
            }
            let repr = ty.llvm();
            let initial = ty.initial();
            // Heap integers never equal tagged zero. Bytes owners are non-null.
            writeln!(
                self.output.common(),
                "
define internal {repr} @{init}_once(ptr %root, ptr %data, i64 %length){attributes} {{
entry:
  %cached = load {repr}, ptr %root
  %empty = icmp eq {repr} %cached, {initial}
  br i1 %empty, label %initialize, label %ready
initialize:
  %fresh = call {repr} @{init}(ptr %data, i64 %length)
  store {repr} %fresh, ptr %root
  br label %ready
ready:
  %value = phi {repr} [ %cached, %entry ], [ %fresh, %initialize ]
  ret {repr} %value
}}"
            )
            .unwrap();
        }
        self.output.common().push('\n');
        writeln!(
            self.output.entry_buffer(),
            "define internal void @relar_init_literals(){attributes} {{\nentry:"
        )
        .unwrap();
        for (bytes, id, ty, init, _) in literals.clone() {
            if lazy_constructor(ty).is_some() {
                continue;
            }
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
        for (_, id, ty, _, release) in literals {
            let repr = ty.llvm();
            let initial = ty.initial();
            let root = self.output.string_name(*id, "literal");
            writeln!(
                self.output.entry_buffer(),
                "  %literal_{id} = load {repr}, ptr @{root}\n  store {repr} {initial}, ptr @{root}\n  call void @{release}({repr} %literal_{id})"
            )
            .unwrap();
        }
        self.output.entry_buffer().push_str("  ret void\n}\n");
    }
}
