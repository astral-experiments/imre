//! Registered call ABI, module constants and emitted definitions.
use crate::ir::Constant;
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
declare void @imre_dyn_unwind_display(ptr, i64)
declare i64 @imre_dyn_cached_handle(ptr, ptr)
declare i64 @imre_dyn_linked_value(ptr, ptr, i64)
declare i64 @imre_dyn_linked_string(ptr, ptr, i32, ptr, i64)
declare i64 @imre_dyn_lookup(ptr, i64, ptr, i64)
declare i64 @imre_dyn_import_name(ptr, i64, i64, i64, i32)
declare i64 @imre_dyn_mark_class_annotation(ptr, i64, i64, i32)
declare i64 @imre_dyn_define(ptr, ptr, i64, i64, i64, i64, i64, i64, i64, i1)
declare i1 @imre_dyn_failed(ptr)
declare i64 @imre_dyn_truth_mark(ptr)
declare void @imre_dyn_comprehension_step(ptr, i64)
declare void @imre_dyn_discard_truth(ptr, i64)
declare i64 @imre_dyn_keep_operands(ptr, i64, i64, i64)
declare void @imre_dyn_discard_expression(ptr, i64)
declare void @imre_dyn_named_assignment(ptr, i64, i64, i64)
declare void @imre_dyn_assignment_target(ptr, ptr, i64, i64)
declare i64 @imre_dyn_op_at(ptr, ptr, i64, i64, i64, ptr)
declare i64 @imre_dyn_get_slice_at(ptr, i64, i64, i64, ptr)
declare i64 @imre_dyn_get_linked_slice_at(ptr, i64, i64, i64, ptr, ptr)
declare void @imre_dyn_call_position(ptr, ptr)
declare i64 @imre_dyn_contains_literal_tuple(ptr, i64, ptr, i64, i32)
declare i64 @imre_dyn_scoped_call_at(ptr, i64, i64, i64, i64, ptr, i64)
declare i64 @imre_dyn_positional_method_at(ptr, i64, ptr, i64, i64, ptr, i64)
declare i64 @imre_dyn_partition_field_at(ptr, i64, ptr, i32, ptr, i64, ptr, i64)
declare i64 @imre_dyn_string_method_target(ptr, i64, ptr, ptr)
declare i64 @imre_dyn_unpack_pair(ptr, i64, ptr)
declare i64 @imre_dyn_next_pair(ptr, i64, ptr, i1)
declare i64 @imre_dyn_builtin_method_target(ptr, i64, ptr, ptr)
declare i64 @imre_dyn_dict_get_target(ptr, i64, ptr)
declare i64 @imre_dyn_positional_call_at(ptr, i64, ptr, i64, i64, ptr, i64)
declare i64 @imre_dyn_keyword_call_at(ptr, i64, ptr, i64, ptr, i64, i64, ptr, i64)
declare void @imre_dyn_expression_line(ptr, i32)
declare void @imre_dyn_statement_with_exception(ptr, ptr, i64, ptr, i32, i64)
";

// Keep repeated code-name loads and their error edge out of each caller.
pub(crate) const NAMED_OP: &str = "\
define internal i64 @imre_boxed_named_op(ptr %ctx, ptr %op, i64 %receiver, ptr %name, i64 %len, i64 %value) noinline {
entry:
  %key = call i64 @imre_dyn_lit(ptr %ctx, i32 6, ptr %name, i64 %len)
  %failed = call i1 @imre_dyn_failed(ptr %ctx)
  br i1 %failed, label %error, label %apply
error:
  ret i64 0
apply:
  %result = tail call i64 @imre_dyn_op_at(ptr %ctx, ptr %op, i64 %receiver, i64 %key, i64 %value, ptr null)
  ret i64 %result
}
";

pub(crate) const GUARDED_NAMED_OP: &str = "\
define internal i64 @imre_boxed_guarded_named_op(ptr %ctx, ptr %op, i64 %receiver, ptr %name, i64 %len, i64 %value) noinline {
entry:
  %pending = call i1 @imre_dyn_failed(ptr %ctx)
  br i1 %pending, label %error, label %apply
error:
  ret i64 0
apply:
  %result = tail call i64 @imre_boxed_named_op(ptr %ctx, ptr %op, i64 %receiver, ptr %name, i64 %len, i64 %value)
  ret i64 %result
}
";

pub(crate) const GUARDED_LIT: &str = "\
define internal i64 @imre_boxed_guarded_lit(ptr %ctx, i32 %kind, ptr %data, i64 %len) noinline {
entry:
  %pending = call i1 @imre_dyn_failed(ptr %ctx)
  br i1 %pending, label %error, label %load
error:
  ret i64 0
load:
  %value = tail call i64 @imre_dyn_lit(ptr %ctx, i32 %kind, ptr %data, i64 %len)
  ret i64 %value
}
";

#[derive(Clone, Copy, Default)]
pub(crate) struct Declarations {
    pub(crate) declared_suspension: bool,
    pub(crate) declared_statement_line: bool,
    pub(crate) declared_definition_position: bool,
    pub(crate) emitted_named_op: bool,
    pub(crate) emitted_guarded_lit: bool,
    pub(crate) emitted_guarded_named_op: bool,
    pub(crate) registered_expression: bool,
    pub(crate) checked_integer_add: bool,
    pub(crate) checked_integer_subtract: bool,
    pub(crate) checked_integer_multiply: bool,
    pub(crate) reads_exact_int_word: bool,
}

#[derive(Clone, Default)]
pub(crate) struct Output {
    pub(crate) declarations: Declarations,
    pub(crate) diagnostic: Option<String>,
    pub(crate) keep_docstrings: bool,
    pub(crate) constant_slots: HashMap<(usize, Constant), usize>,
    pub(crate) strings: HashMap<Vec<u8>, u32>,
    pub(crate) interned_identifiers: BTreeSet<String>,
    pub(crate) positions: HashMap<[usize; 4], usize>,
    pub(crate) constants: String,
    pub(crate) functions: String,
    pub(crate) count: u32,
}

impl Output {
    pub(crate) fn integer_prelude(&mut self) {
        use crate::ir::Binary;
        if self.declarations.reads_exact_int_word {
            self.functions.push_str("declare i64 @imre_dyn_exact_int_word(ptr, i64)\n");
        }
        let helpers = [
            (self.declarations.checked_integer_add, Binary::Add, "sadd"),
            (
                self.declarations.checked_integer_subtract,
                Binary::Subtract,
                "ssub",
            ),
            (
                self.declarations.checked_integer_multiply,
                Binary::Multiply,
                "smul",
            ),
        ];
        if !helpers.iter().any(|(used, _, _)| *used) {
            return;
        }
        self.functions.push_str(concat!(
            "declare i64 @imre_dyn_take_int(ptr, i64)\n",
            "declare void @imre_int_retain(i64)\n",
            "declare void @imre_int_release(i64)\n",
            "declare i64 @imre_int_binary(i64, i64, i32)\n",
            "!0 = !{!\"branch_weights\", i32 1, i32 2000}\n",
            "!1 = !{!\"branch_weights\", i32 2000, i32 1}\n",
        ));
        for (used, op, intrinsic) in helpers {
            if used {
                writeln!(
                    self.functions,
                    "declare {{ i64, i1 }} @llvm.{intrinsic}.with.overflow.i64(i64, i64)"
                )
                .unwrap();
                self.functions
                    .push_str(&crate::llvm::integer_arithmetic_helper(op));
            }
        }
    }

    /// Roll back private code sites when an optional annotation expression cannot be compiled.
    pub(crate) fn try_native_body(
        &mut self,
        emit: impl FnOnce(&mut Self) -> String,
    ) -> Option<String> {
        let code = self.count;
        let functions = self.functions.len();
        let constants = self.constants.len();
        let strings = self.strings.len() as u32;
        let positions = self.positions.len();
        let slots = self.constant_slots.len();
        let declarations = self.declarations;
        let names = std::mem::take(&mut self.interned_identifiers);
        let outer_diagnostic = self.diagnostic.take();
        let site = emit(self);
        let added_names = std::mem::replace(&mut self.interned_identifiers, names);
        if std::mem::replace(&mut self.diagnostic, outer_diagnostic).is_none() {
            self.interned_identifiers.extend(added_names);
            return Some(site);
        }
        self.count = code;
        self.functions.truncate(functions);
        self.constants.truncate(constants);
        self.strings.retain(|_, id| *id < strings);
        self.positions.retain(|_, id| *id < positions);
        self.constant_slots.retain(|_, id| *id < slots);
        self.declarations = declarations;
        None
    }

    pub(crate) fn data(&mut self, bytes: &[u8]) -> String {
        let count = self.strings.len() as u32;
        let index = *self.strings.entry(bytes.to_vec()).or_insert(count);
        if index == count {
            write!(
                self.constants,
                "@str{index} = private unnamed_addr constant [{} x i8] c\"",
                bytes.len() + 1
            )
            .unwrap();
            for &byte in bytes {
                if (b' '..=b'~').contains(&byte) && !matches!(byte, b'"' | b'\\') {
                    self.constants.push(char::from(byte));
                } else {
                    write!(self.constants, "\\{byte:02X}").unwrap();
                }
            }
            self.constants.push_str("\\00\"\n");
        }
        format!("@str{index}")
    }

    pub(crate) fn position(&mut self, span: [usize; 4]) -> String {
        let next = self.positions.len();
        let index = *self.positions.entry(span).or_insert(next);
        if index == next {
            let [line, start, split, end] = span;
            writeln!(self.constants, "@pos{index} = private unnamed_addr constant [4 x i32] [i32 {line}, i32 {start}, i32 {split}, i32 {end}]").unwrap();
        }
        format!("@pos{index}")
    }
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
