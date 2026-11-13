//! Registered call ABI, module constants and emitted definitions.
use crate::ir::Constant;
use crate::llvm::{ByteEscapes, write_byte_string};
use indexmap::IndexSet;
use std::collections::{BTreeSet, HashMap};
use std::fmt::Write as _;

pub(crate) mod permanent {
    include!("../../../../imre-runtime/native/boxed-handles.rs");
}

pub(crate) const ABI: &str = "\
%code_site = type { ptr, ptr, i32, i32 }
declare ptr @imre_dyn_new(i32, ptr, ptr)
declare void @imre_dyn_register(ptr, ptr, ptr, ptr)
declare i32 @imre_dyn_finish(ptr)
declare i64 @imre_dyn_lit(ptr, i32, ptr, i64)
declare i64 @imre_dyn_build_sequence(ptr, ptr, i64, i1, i1, ptr)
declare i64 @imre_dyn_build_set(ptr, ptr, i64, i64)
declare i64 @imre_dyn_display_item(ptr, i64, i64, i64, i1)
declare i64 @imre_dyn_cached_handle(ptr, ptr)
declare i64 @imre_dyn_linked_value(ptr, ptr, i64)
declare i64 @imre_dyn_linked_string(ptr, ptr, i32, ptr, i64)
declare i64 @imre_dyn_lookup(ptr, i64, ptr, i64)
declare i64 @imre_dyn_lookup_local(ptr, i64, ptr, i64)
declare i64 @imre_dyn_assign_target(ptr, i64, ptr, i64, i64)
declare i64 @imre_dyn_import_name(ptr, i64, i64, i64, i32)
declare i64 @imre_dyn_define(ptr, ptr, i64, i64, i64, i64, i64, i64, i64, i1)
declare i1 @imre_dyn_failed(ptr)
declare i64 @imre_dyn_truth_mark(ptr) memory(read)
declare void @imre_dyn_comprehension_step(ptr, i64)
declare void @imre_dyn_discard_truth(ptr, i64)
declare void @imre_dyn_unwind_operands(ptr, i64)
declare i64 @imre_dyn_interpolation(ptr, i64, i64, i64, i64)
declare i64 @imre_dyn_format_parts(ptr, i64, ptr, i64)
declare i64 @imre_dyn_keep_operands(ptr, i64, i64, i64)
declare void @imre_dyn_discard_expression(ptr, i64)
declare void @imre_dyn_named_assignment(ptr, i64, i64, i64)
declare void @imre_dyn_unwind_frame(ptr, ptr, i64, i64)
declare void @imre_dyn_assignment_target_at(ptr, i64, ptr, i64, i64)
declare i64 @imre_dyn_op_at(ptr, ptr, i64, i64, i64, ptr)
declare i64 @imre_boxed_named_op(ptr, ptr, i64, ptr, i64, i64)
declare i64 @imre_boxed_guarded_named_op(ptr, ptr, i64, ptr, i64, i64)
declare i64 @imre_boxed_guarded_lit(ptr, i32, ptr, i64)
declare i64 @imre_dyn_masked_remainder(ptr, i64, i64)
declare i64 @imre_dyn_get_slice_at(ptr, i64, i64, i64, ptr)
declare i64 @imre_dyn_get_linked_slice_at(ptr, i64, i64, i64, ptr, ptr)
declare void @imre_dyn_call_position(ptr, ptr)
declare i64 @imre_dyn_contains_literal_tuple(ptr, i64, ptr, i64, i32)
declare i64 @imre_dyn_scoped_call_at(ptr, i64, i64, i64, i64, ptr, i64)
declare i64 @imre_dyn_positional_method_at(ptr, i64, ptr, i64, i64, ptr, i64)
declare i64 @imre_dyn_partition_field_at(ptr, i64, ptr, i32, ptr, i64, ptr, i64)
declare i64 @imre_dyn_string_method_target(ptr, i64, ptr, ptr)
declare i64 @imre_dyn_get_int_item(ptr, i64, i64, i64, ptr)
declare i64 @imre_dyn_unpack_pair(ptr, i64, ptr)
declare i1 @imre_dyn_plain_items_cursor(ptr, i64) memory(read)
declare i64 @imre_dyn_next_pair(ptr, i64, ptr, i1)
declare i32 @imre_dyn_list_cursor_pull(ptr, i64, ptr, i1)
declare i64 @imre_dyn_builtin_method_target(ptr, i64, ptr, ptr)
declare i64 @imre_dyn_dict_get_target(ptr, i64, ptr)
declare i64 @imre_dyn_positional_call_at(ptr, i64, ptr, i64, i64, ptr, i64)
declare i64 @imre_dyn_keyword_call_at(ptr, i64, ptr, i64, ptr, i64, i64, ptr, i64)
declare void @imre_dyn_expression_line(ptr, i32)
declare void @imre_dyn_statement_with_exception(ptr, ptr, i64, ptr, i32, i64)
declare void @llvm.memcpy.p0.p0.i64(ptr noalias nocapture writeonly, ptr noalias nocapture readonly, i64, i1 immarg)
";

#[derive(Clone, Copy, Default)]
pub(crate) struct Declarations {
    pub(crate) declared_suspension: bool,
    pub(crate) declared_statement_line: bool,
    pub(crate) declared_definition_position: bool,
    pub(crate) unwind_slots: bool,
    pub(crate) registered_expression: bool,
    pub(crate) exec_operand: bool,
    pub(super) integers: crate::llvm::body::IntegerFunctions,
    pub(crate) reads_exact_int_word: bool,
    pub(super) releases_integer_owner: bool,
}

#[derive(Clone, Default)]
pub(crate) struct Output {
    pub(crate) declarations: Declarations,
    pub(crate) diagnostic: Option<String>,
    pub(crate) keep_docstrings: bool,
    pub(crate) constant_slots: HashMap<(usize, Constant), usize>,
    pub(crate) strings: IndexSet<Vec<u8>>,
    pub(crate) interned_identifiers: BTreeSet<String>,
    pub(crate) positions: HashMap<[usize; 4], usize>,
    pub(crate) constants: String,
    pub(crate) functions: String,
    pub(crate) count: u32,
    pub(crate) whole: Option<Box<Output>>,
    pub(crate) whole_unit: usize,
}

impl Output {
    // Private operands can have different numbers in the standalone unit and whole dump.
    // Source bytes are escaped in data constants. Only compiler-created operands carry NUL.
    fn operand(local: String, whole: String) -> String {
        if local == whole {
            local
        } else {
            format!("\0{local}\0{whole}\0")
        }
    }

    pub(crate) fn code_symbol(&self, kind: &str, id: u32) -> String {
        let local = format!("@{kind}{id}");
        match &self.whole {
            Some(whole) => Self::operand(local, format!("@{kind}{}", whole.count + id)),
            None => local,
        }
    }

    pub(crate) fn declare(&mut self, flag: fn(&mut Declarations) -> &mut bool, text: &'static str) {
        if let Some(whole) = &mut self.whole {
            whole.declare(flag, text);
        }
        if !std::mem::replace(flag(&mut self.declarations), true) {
            self.functions.push_str(text);
        }
    }

    pub(crate) fn place_function(&mut self, start: usize) {
        if let Some(whole) = &mut self.whole {
            place_operands(&mut self.functions, start, &mut whole.functions);
        }
    }

    pub(crate) fn place_site(&mut self, start: usize) {
        if let Some(whole) = &mut self.whole {
            place_operands(&mut self.constants, start, &mut whole.constants);
        }
    }

    pub(crate) fn finish_placement(
        &mut self,
        target: &mut Output,
        entry: &mut String,
        whole_entry: &mut String,
    ) {
        place_operands(entry, 0, whole_entry);
        let mut whole = self.whole.take().unwrap();
        whole.count += self.count;
        whole
            .declarations
            .integers
            .include(self.declarations.integers);
        whole.declarations.reads_exact_int_word |= self.declarations.reads_exact_int_word;
        whole.declarations.releases_integer_owner |= self.declarations.releases_integer_owner;
        *target = *whole;
    }

    pub(crate) fn integer_prelude(&mut self) {
        self.functions.push_str(crate::llvm::BRANCH_WEIGHTS);
        if self.declarations.reads_exact_int_word {
            self.functions
                .push_str("declare i64 @imre_dyn_exact_int_word(ptr, i64)\n");
        }
        if self.declarations.integers.is_empty() {
            return;
        }
        self.functions.push_str(concat!(
            "declare i64 @imre_dyn_take_int(ptr, i64)\n",
            "declare void @imre_int_retain(i64)\n",
            "declare void @imre_int_release(i64)\n",
            "declare i64 @imre_int_binary(i64, i64, i32)\n",
        ));
        self.declarations.integers.write(&mut self.functions, true);
        if self.declarations.releases_integer_owner {
            crate::llvm::write_integer_reference(&mut self.functions, "release", &[true]);
        }
    }

    pub(crate) fn data(&mut self, bytes: &[u8]) -> String {
        self.place_data(bytes, None)
    }

    /// Use this output's symbol number and share the written initializer with the whole dump.
    fn place_data(&mut self, bytes: &[u8], initializer: Option<&str>) -> String {
        let (index, initializer) = if let Some(index) = self.strings.get_index_of(bytes) {
            (index, initializer)
        } else {
            let (index, _) = self.strings.insert_full(bytes.to_vec());
            write!(
                self.constants,
                "@str{index} = private unnamed_addr constant [{} x i8] ",
                bytes.len() + 1
            )
            .unwrap();
            let start = self.constants.len();
            if let Some(initializer) = initializer {
                self.constants.push_str(initializer);
            } else {
                write_byte_string(
                    &mut self.constants,
                    bytes.iter().copied().chain([0]),
                    ByteEscapes::NonPrintable,
                );
            }
            (index, Some(&self.constants[start..]))
        };
        let local = format!("@str{index}");
        match &mut self.whole {
            Some(whole) => Self::operand(local, whole.place_data(bytes, initializer)),
            None => local,
        }
    }

    pub(crate) fn position(&mut self, span: [usize; 4]) -> String {
        let next = self.positions.len();
        let index = *self.positions.entry(span).or_insert(next);
        if index == next {
            let [line, start, split, end] = span;
            writeln!(self.constants, "@pos{index} = private unnamed_addr constant [4 x i32] [i32 {line}, i32 {start}, i32 {split}, i32 {end}]").unwrap();
        }
        let local = format!("@pos{index}");
        match &mut self.whole {
            Some(whole) => Self::operand(local, whole.position(span)),
            None => local,
        }
    }

    pub(crate) fn constant_slot(&mut self, unit: usize, key: Constant) -> String {
        let other = self
            .whole
            .as_mut()
            .map(|whole| whole.constant_slot(self.whole_unit, key.clone()));
        let next = self.constant_slots.len();
        let index = *self.constant_slots.entry((unit, key)).or_insert(next);
        if index == next {
            writeln!(self.constants, "@const_slot{index} = private constant i8 0").unwrap();
        }
        let local = format!("@const_slot{index}");
        match other {
            Some(whole) => Self::operand(local, whole),
            None => local,
        }
    }
}

/// Place a completed definition. Instructions and local SSA names are identical in both units.
fn place_operands(local: &mut String, start: usize, whole: &mut String) {
    let body = &local[start..];
    if !body.contains('\0') {
        whole.push_str(body);
        return;
    }
    let mut resolved = String::with_capacity(body.len());
    let mut pieces = body.split('\0');
    while let Some(common) = pieces.next() {
        resolved.push_str(common);
        whole.push_str(common);
        if let Some(operand) = pieces.next() {
            resolved.push_str(operand);
            whole.push_str(pieces.next().expect("paired LLVM operand"));
        }
    }
    local.truncate(start);
    local.push_str(&resolved);
}

/// Pass fixed operands in native storage or pack expanded operands in Python containers.
pub(crate) enum CallArguments {
    FixedKeywords {
        array: &'static str,
        positional: usize,
        names: String,
        keywords: usize,
    },
    Packed {
        args: String,
        kwargs: String,
    },
    Positional {
        array: &'static str,
        count: usize,
        receiver_slot: bool,
    },
    PartitionField {
        array: &'static str,
        index: u32,
        get_position: String,
    },
}
