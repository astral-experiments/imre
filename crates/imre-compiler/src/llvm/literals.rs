//! Program-owned literals share immutable values between evaluations.

use super::*;

fn lazy_constructor(ty: Type) -> Option<&'static str> {
    match ty {
        Type::Bytes => Some("imre_bytes_literal"),
        Type::Int => Some("imre_int_literal"),
        _ => None,
    }
}

fn immutable_literal(value: &Expr) -> bool {
    match &value.kind {
        ExprKind::Int(_)
        | ExprKind::Float(_)
        | ExprKind::Bool(_)
        | ExprKind::None
        | ExprKind::Str(_)
        | ExprKind::Bytes(_) => true,
        ExprKind::Negate(inner)
        | ExprKind::Not(inner)
        | ExprKind::Present(inner)
        | ExprKind::Absent(inner) => immutable_literal(inner),
        ExprKind::Binary(_, left, right)
            if matches!(
                value.ty,
                Type::Int | Type::Float | Type::Bool | Type::Str | Type::Bytes
            ) =>
        {
            immutable_literal(left) && immutable_literal(right)
        }
        ExprKind::Tuple(values) => values.iter().all(immutable_literal),
        _ => false,
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

    pub(super) fn tuple(&mut self, ty: Type, values: &[Expr], named_root: Option<&str>) -> Value {
        let root = if let Some(name) = named_root {
            // Imports and default arguments can read the same module object in
            // different codegen units. Define and clear its root exactly once.
            let root = self.output.global_ref(name.to_owned());
            let roots = self.tuple_literals.entry(self.output.entry).or_default();
            if !roots.contains(&root) {
                roots.push(root.clone());
            }
            root
        } else if matches!(ty, Type::Tuple(_)) && values.iter().all(immutable_literal) {
            let unit = self.output.current;
            let roots = self.tuple_literals.entry(unit).or_default();
            let root = self
                .output
                .global_ref(format!("tuple_literal_{unit}_{}", roots.len()));
            roots.push(root.clone());
            root
        } else {
            return self.record_from_fields(ty, values);
        };
        self.rooted_record(ty, &root, |emitter| emitter.record_from_fields(ty, values))
    }

    /// Give the root the constructor's owner. Each read receives its own reference.
    pub(super) fn rooted_record(
        &mut self,
        ty: Type,
        root: &str,
        build: impl FnOnce(&mut Self) -> Value,
    ) -> Value {
        let prior = self.instruction(format!("load ptr, ptr @{root}"));
        let missing = self.instruction(format!("icmp eq ptr {prior}, null"));
        let (construct, ready) = (self.label(), self.label());
        self.branch(&missing, &construct, &ready);
        self.start(&construct);
        let object = build(self);
        self.line(format!("store ptr {}, ptr @{root}", object.operand));
        self.forget(&object);
        self.jump(&ready);
        self.start(&ready);
        let value = Value {
            ty,
            operand: self.instruction(format!("load ptr, ptr @{root}")),
            small_integer: false,
        };
        self.retain(&value);
        value
    }

    /// Construct a new owner, even when all field values are literals.
    pub(super) fn record_from_fields(&mut self, ty: Type, values: &[Expr]) -> Value {
        let object = self.record(ty);
        for (field, value) in values.iter().enumerate() {
            let value = self.operand(value);
            self.set_field(&object, field, &value.value);
            self.release_operand(&value);
        }
        object
    }

    /// Main releases the literal roots after locals/globals on either exit.
    /// Bytes and heap integers initialize on first use to preserve cold branches.
    pub(super) fn literal_pool(&mut self) {
        let pools = [
            (
                &self.string_literals,
                Type::Str,
                "imre_str_static_literal",
                "imre_str_release",
            ),
            (
                &self.regex_literals,
                Type::RegexPattern,
                "imre_regex_compile",
                "imre_object_release",
            ),
            (
                &self.bytes_literals,
                Type::Bytes,
                "imre_bytes_literal",
                "imre_bytes_release",
            ),
            (
                &self.integer_literals,
                Type::Int,
                "imre_int_literal",
                "imre_int_release",
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
        for root in self
            .tuple_literals
            .values()
            .flatten()
            .chain(&self.static_functions)
        {
            self.output
                .global(self.output.entry, root, "global", "ptr")
                .push_str("null\n");
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
            "define internal void @imre_init_literals(){attributes} {{\nentry:"
        )
        .unwrap();
        for (bytes, id, ty, init, _) in literals.clone() {
            if lazy_constructor(ty).is_some() {
                continue;
            }
            // Stock CPython interns ASCII name characters and its Latin-1
            // character singletons. Distinct equal dynamic strings stay distinct.
            let init = if ty == Type::Str
                && self.uses_string_interning
                && (bytes
                    .iter()
                    .all(|b| b.is_ascii_alphanumeric() || *b == b'_')
                    || matches!(bytes.as_slice(), [_] | [0xc2..=0xc3, 0x80..=0xbf]))
            {
                "imre_str_interned_literal"
            } else {
                init
            };
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
                "  call void @imre_enum_display_table(ptr @str_enum_members, i64 {}, ptr @enum_display_styles)",
                self.enum_members.len()
            )
            .unwrap();
        }
        self.output.entry_buffer().push_str("  ret void\n}\n");
        self.output.common().push('\n');
        writeln!(
            self.output.entry_buffer(),
            "define internal void @imre_clear_literals(){attributes} {{\nentry:"
        )
        .unwrap();
        if !self.enum_members.is_empty() {
            self.output
                .entry_buffer()
                .push_str("  call void @imre_str_enum_table(ptr null, i64 0)\n");
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
        for root in self
            .tuple_literals
            .values()
            .flatten()
            .chain(&self.static_functions)
        {
            writeln!(
                self.output.entry_buffer(),
                "  %{root} = load ptr, ptr @{root}\n  store ptr null, ptr @{root}\n  call void @imre_record_release(ptr %{root})"
            )
            .unwrap();
        }
        self.output.entry_buffer().push_str("  ret void\n}\n");
    }
}
