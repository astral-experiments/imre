//! Object-frame ABI operations used by registered LLVM code.

use super::sequences::{permanent_handle, spread};
use super::{
    CallArguments, GUARDED_LIT, GUARDED_NAMED_OP, NAMED_OP, Output, permanent, suspension,
};
use super::{ReadKey, suspension::Suspension};
use crate::frontend::function_scope::ClassBindings;
use crate::frontend::push_codepoints;
use crate::ir;
use crate::ir::{Builtin, Constant, DisplayKind, Expr};
use crate::llvm::body::FunctionBody;
use crate::llvm::operations::{CheckedValue, LoopEdges};
use ruff_text_size::{TextRange, TextSize};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

pub(crate) enum Place {
    Name(String),
    Item(String, String),
}

/// Source bytes and byte offsets shared by both forms of registered body.
pub(crate) struct SourcePositions<'a> {
    pub(crate) text: &'a str,
    pub(crate) file: &'a str,
    newlines: Vec<usize>,
}

impl<'a> SourcePositions<'a> {
    pub(crate) fn new(text: &'a str, file: &'a str) -> Self {
        Self {
            text,
            file,
            newlines: text.match_indices('\n').map(|(offset, _)| offset).collect(),
        }
    }

    pub(crate) fn line(&self, offset: TextSize) -> usize {
        self.newlines.partition_point(|&n| n < usize::from(offset)) + 1
    }

    pub(crate) fn line_and_column(&self, offset: TextSize) -> (usize, usize) {
        let line = self.line(offset);
        let start = if line == 1 {
            0
        } else {
            self.newlines[line - 2] + 1
        };
        (line, usize::from(offset) - start)
    }
}

/// The location currently published by this emitter. Eager operands carry a full span.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum SourcePosition {
    Lines { statement: usize, expression: usize },
    Expression(TextRange),
    Implicit(TextRange),
}

impl SourcePosition {
    pub(super) fn statement(line: usize) -> Self {
        Self::Lines {
            statement: line,
            expression: line,
        }
    }
}

#[derive(Default)]
pub(crate) enum AnnotationScope {
    #[default]
    Plain,
    Method(ClassBindings),
    Class {
        globals: BTreeSet<String>,
        conditional: bool,
    },
}

/// The registered calling convention supplies object handles, scoped roots and pending errors.
pub(crate) struct RegisteredFrame<'a, 'p> {
    pub(crate) output: &'a mut Output,
    pub(crate) positions: &'p SourcePositions<'p>,
    pub(crate) unit: usize,
    pub(super) iteration: super::IterationState,
    pub(crate) loops: Vec<super::LoopTarget>,
    pub(crate) current_range: TextRange,
    /// A mixed block branches to its enclosing finally/with cleanup in the syntax visitor.
    pub(crate) cleanup_returns: Option<Vec<(String, String, TextRange)>>,
    pub(crate) async_generator: bool,
    pub(crate) class_locals: Option<ClassBindings>,
    pub(crate) annotation_values: Option<String>,
    pub(crate) annotation_scope: AnnotationScope,
    annotation_active: Option<String>,
    pub(crate) module_annotation_sites: Option<Vec<TextSize>>,
    pub(crate) body: FunctionBody,
    id: u32,
    pending_check: Option<ErrorCheck>,
    pub(crate) scope: String,
    // Handles held in native registers across statement boundaries must stay reachable.
    pub(crate) live: Vec<String>,
    /// Addressable IR temporaries whose contents survive Python statement boundaries.
    pub(crate) live_slots: Vec<usize>,
    pub(crate) private_slot_count: usize,
    pub(crate) max_live: usize,
    pub(crate) integer_owners: usize,
    pub(crate) expression_integer_start: Option<usize>,
    // A caught error abandons this expression's intermediates and preserves enclosing owners.
    integer_unwinds: BTreeMap<(usize, String), (usize, String)>,
    /// Private roots whose lexical scope exits at an existing error landing block.
    pub(super) scope_unwinds: BTreeMap<String, BTreeSet<usize>>,
    pub(crate) caught: Vec<String>,
    pub(crate) simple_call: Option<TextRange>,
    pub(super) position: SourcePosition,
    pub(crate) native_suspension: bool,
}

struct ErrorCheck {
    handler: String,
    failed: String,
    next: String,
}

impl<'a, 'p> RegisteredFrame<'a, 'p> {
    pub(crate) fn new(
        output: &'a mut Output,
        positions: &'p SourcePositions<'p>,
        native_suspension: bool,
        unit: usize,
        start: TextSize,
    ) -> Self {
        let id = output.count;
        output.count += 1;
        Self {
            output,
            positions,
            unit,
            iteration: Default::default(),
            loops: Vec::new(),
            current_range: TextRange::empty(start),
            cleanup_returns: None,
            async_generator: false,
            class_locals: None,
            annotation_values: None,
            annotation_scope: AnnotationScope::Plain,
            annotation_active: None,
            module_annotation_sites: None,
            id,
            body: FunctionBody::new(if native_suspension { "start" } else { "entry" }, "failed"),
            pending_check: None,
            scope: "%scope".into(),
            live: Vec::new(),
            live_slots: Vec::new(),
            private_slot_count: 0,
            max_live: 0,
            integer_owners: 0,
            expression_integer_start: None,
            integer_unwinds: BTreeMap::new(),
            scope_unwinds: BTreeMap::new(),
            caught: Vec::new(),
            simple_call: None,
            position: SourcePosition::statement(0),
            native_suspension,
        }
    }

    pub(crate) fn begin_module(
        &mut self,
        has_annotations: bool,
        string_annotations: bool,
        docstring: Option<&str>,
        globals: &BTreeSet<String>,
    ) {
        if has_annotations {
            if string_annotations {
                self.op("setup_module_string_annotations", &[&self.scope.clone()]);
            } else {
                // Lower the provider after collecting the body, but run its setup first.
                self.module_annotation_sites = Some(Vec::new());
                self.jump("annotations_init");
                self.label("module_body");
            }
        }
        if self.output.keep_docstrings
            && let Some(doc) = docstring
        {
            let value = self.text(doc);
            self.bind("__doc__", &value);
        }
        for name in globals {
            self.declare_global(name);
        }
    }

    pub(crate) fn finish_module(
        mut self,
        define_annotations: impl FnOnce(&mut Self, &[TextSize]),
    ) -> String {
        if let Some(sites) = self.module_annotation_sites.take() {
            self.end("ret i64 0");
            self.label("annotations_init");
            if !sites.is_empty() {
                define_annotations(&mut self, &sites);
            }
            self.op("setup_module_annotations", &[&self.scope.clone()]);
            self.jump("module_body");
        }
        self.finish(true)
    }

    pub(crate) fn finish(mut self, module: bool) -> String {
        let name = format!("@code{}", self.id);
        if !self.body.terminated {
            self.return_value("0");
        }
        let mut entry = if self.max_live == 0 {
            String::new()
        } else {
            format!("  %live = alloca [{} x i64]\n", self.max_live)
        };
        let count = self.private_slot_count;
        if count != 0 {
            writeln!(
                entry,
                "  %ir_slot0 = alloca [{count} x i64]\n  store [{count} x i64] zeroinitializer, ptr %ir_slot0"
            )
            .unwrap();
            for slot in 1..count {
                writeln!(
                    entry,
                    "  %ir_slot{slot} = getelementptr i64, ptr %ir_slot0, i64 {slot}"
                )
                .unwrap();
            }
        }
        for slot in 0..self.integer_owners {
            writeln!(
                entry,
                "  %int_owner{slot} = alloca i64\n  store i64 1, ptr %int_owner{slot}"
            )
            .unwrap();
        }
        // A loaded code object owns its identifier-like strings before any statement runs.
        if module && !self.output.interned_identifiers.is_empty() {
            let names = std::mem::take(&mut self.output.interned_identifiers)
                .into_iter()
                .collect::<Vec<_>>()
                .join("\0");
            let data = self.output.data(names.as_bytes());
            writeln!(
                entry,
                "  call i64 @imre_dyn_lit(ptr %ctx, i32 5, ptr {data}, i64 {})",
                names.len()
            )
            .unwrap();
        }
        for ((start, target), (end, label)) in std::mem::take(&mut self.integer_unwinds) {
            self.body.start(&label);
            self.release_integer_owners(start..end, true);
            self.body.jump(&target);
        }
        self.body.start("failed");
        self.release_integer_owners(0..self.integer_owners, false);
        if self.native_suspension {
            self.body
                .line("call void @imre_dyn_suspend_return(ptr %context_slot, i64 0)");
            self.body.jump("complete");
            writeln!(self.output.functions,
                "define internal ptr {name}(ptr %context_slot, i64 %scope) presplitcoroutine {{\nentry:\n{entry}{}{}{} }}",
                suspension::PROLOGUE, self.body.code, suspension::EPILOGUE).unwrap();
        } else {
            self.body.end("ret i64 0");
            writeln!(
                self.output.functions,
                "define internal i64 {name}(ptr %ctx, i64 %scope) {{\nentry:\n{entry}{}}}",
                self.body.code
            )
            .unwrap();
        }
        name
    }

    fn release_integer_owners(&mut self, slots: std::ops::Range<usize>, clear: bool) {
        for slot in slots {
            let word = self
                .body
                .instruction(format!("load i64, ptr %int_owner{slot}"));
            if clear {
                self.body.line(format!("store i64 1, ptr %int_owner{slot}"));
            }
            self.body
                .line(format!("call void @imre_int_release(i64 {word})"));
        }
    }

    fn replace_position(&mut self, position: SourcePosition) -> SourcePosition {
        let previous = self.position;
        if previous != position && !self.body.terminated {
            match position {
                SourcePosition::Implicit(range) => self.set_source_range(Some(range)),
                SourcePosition::Expression(range) => {
                    self.set_source_range(Some(range));
                    // A full statement span is hidden in a traceback. Mark this instruction
                    // as an expression so that its operand can be highlighted.
                    self.publish_expression_line(self.positions.line(range.start()));
                }
                SourcePosition::Lines {
                    statement,
                    expression,
                } => {
                    if !matches!(previous, SourcePosition::Lines { .. }) {
                        self.set_source_range(None);
                    }
                    if !matches!(previous, SourcePosition::Lines { statement: old, .. } if old == statement)
                    {
                        self.set_statement_line(statement);
                    }
                    if !matches!(self.position, SourcePosition::Lines { expression: old, .. } if old == expression)
                    {
                        self.publish_expression_line(expression);
                    }
                }
            }
        }
        self.position = position;
        previous
    }

    fn publish_expression_line(&mut self, line: usize) {
        self.line(&format!(
            "call void @imre_dyn_expression_line(ptr %ctx, i32 {line})"
        ));
    }

    pub(crate) fn enter_expression(&mut self, range: TextRange, start: TextSize) -> SourcePosition {
        let line = self.positions.line(start);
        let position = match self.position {
            SourcePosition::Expression(_) | SourcePosition::Implicit(_) => {
                let start = if self.positions.line(range.start()) == line {
                    range.start()
                } else {
                    start
                };
                SourcePosition::Expression(TextRange::new(start, range.end()))
            }
            SourcePosition::Lines { statement, .. } => SourcePosition::Lines {
                statement,
                expression: line,
            },
        };
        self.replace_position(position)
    }

    pub(crate) fn leave_expression(&mut self, previous: SourcePosition) {
        self.replace_position(previous);
    }

    pub(crate) fn name(&mut self, prefix: &str) -> String {
        self.body.name(prefix)
    }

    pub(crate) fn line(&mut self, line: &str) {
        self.flush_check();
        self.instruction(line);
    }

    fn instruction(&mut self, line: &str) {
        // A suspended activation outlives the Context borrow of any one resume. Only
        // the slot's address is stable. Each runtime call receives the current borrow.
        if self.native_suspension
            && let Some((before, after)) = line.split_once("ptr %ctx")
        {
            let ctx = self.name("%active_ctx");
            self.body
                .line(format!("{ctx} = load volatile ptr, ptr %context_slot"));
            self.body.line(format!("{before}ptr {ctx}{after}"));
        } else {
            self.body.line(line);
        }
    }

    pub(crate) fn value(&mut self, instruction: &str) -> String {
        if !self.native_suspension {
            self.flush_check();
            return self.body.instruction(instruction);
        }
        let value = self.name("%v");
        self.line(&format!("{value} = {instruction}"));
        value
    }

    pub(crate) fn end(&mut self, instruction: &str) {
        if !self.body.terminated {
            self.flush_check();
            self.body.end(instruction);
        }
    }

    pub(crate) fn return_value(&mut self, value: &str) {
        if self.native_suspension {
            if !self.body.terminated {
                self.line(&format!(
                    "call void @imre_dyn_suspend_return(ptr %context_slot, i64 {value})"
                ));
                self.jump("complete");
            }
        } else {
            self.end(&format!("ret i64 {value}"));
        }
    }

    pub(crate) fn jump(&mut self, label: &str) {
        if !self.body.terminated {
            self.flush_check();
            self.body.jump(label);
        }
    }

    pub(crate) fn label(&mut self, label: &str) {
        self.flush_check();
        self.body.start(label);
        if let Some(slots) = self.scope_unwinds.remove(label) {
            for slot in slots {
                self.line(&format!("store i64 0, ptr %ir_slot{slot}"));
            }
        }
    }

    pub(crate) fn branch(&mut self, condition: &str, yes: &str, no: &str) {
        if !self.body.terminated {
            self.flush_check();
            self.body.branch(condition, yes, no);
        }
    }

    pub(crate) fn checked(&mut self, invocation: &str) -> String {
        let result = self.value(invocation);
        self.check_error();
        result
    }

    pub(crate) fn check_error(&mut self) {
        let mut handler = self.body.error_target.clone();
        if let Some(start) = self.expression_integer_start
            && start < self.integer_owners
            && handler != "failed"
        {
            let count = self.integer_unwinds.len();
            let (end, label) = self
                .integer_unwinds
                .entry((start, handler))
                .or_insert_with(|| (self.integer_owners, format!("int_unwind{count}")));
            *end = self.integer_owners;
            handler = label.clone();
        }
        self.pending_check = Some(ErrorCheck {
            handler,
            failed: self.name("%v"),
            next: self.name("b"),
        });
    }

    pub(crate) fn flush_check(&mut self) {
        if let Some(ErrorCheck {
            handler,
            failed,
            next,
        }) = self.pending_check.take()
        {
            self.instruction(&format!("{failed} = call i1 @imre_dyn_failed(ptr %ctx)"));
            self.body.branch(&failed, &handler, &next);
            self.body.start(&next);
        }
    }

    /// A guarded successor can deliver the first failure to the same handler.
    pub(crate) fn take_matching_check(&mut self) -> bool {
        self.pending_check
            .take_if(|check| check.handler == self.body.error_target)
            .is_some()
    }

    /// Publish the real predecessor before referring to it in a phi.
    pub(crate) fn current_block(&mut self) -> &str {
        self.flush_check();
        &self.body.current
    }

    pub(crate) fn binary_handles(&mut self, op: ir::Binary, left: &str, right: &str) -> String {
        self.op(op.protocol(), &[left, right])
    }

    pub(crate) fn op(&mut self, name: &str, args: &[&str]) -> String {
        self.op_at(name, args, None)
    }

    pub(crate) fn op_at(&mut self, name: &str, args: &[&str], span: Option<[usize; 4]>) -> String {
        if name != "take_error" {
            self.take_matching_check();
        }
        let op = self.output.data(name.as_bytes());
        let arg = |index| *args.get(index).unwrap_or(&"0");
        let position = span.map_or_else(|| "null".into(), |span| self.output.position(span));
        self.checked(&format!(
            "call i64 @imre_dyn_op_at(ptr %ctx, ptr {op}, i64 {}, i64 {}, i64 {}, ptr {position})",
            arg(0),
            arg(1),
            arg(2)
        ))
    }

    pub(crate) fn lit(&mut self, kind: i32, bytes: &[u8]) -> String {
        let loader = if self.take_matching_check() {
            if !self.output.declarations.emitted_guarded_lit {
                self.output.functions.push_str(GUARDED_LIT);
                self.output.declarations.emitted_guarded_lit = true;
            }
            "@imre_boxed_guarded_lit"
        } else {
            "@imre_dyn_lit"
        };
        let literal = self.output.data(bytes);
        self.checked(&format!(
            "call i64 {loader}(ptr %ctx, i32 {kind}, ptr {literal}, i64 {})",
            bytes.len()
        ))
    }

    /// Emit a code-object name, including a Unicode identifier that would not intern as a literal.
    pub(crate) fn code_name(&mut self, name: &str) -> String {
        self.output.interned_identifiers.insert(name.to_owned());
        self.lit(6, name.as_bytes())
    }

    pub(crate) fn integer(&mut self, n: impl TryInto<i64> + ToString + Copy) -> String {
        if let Ok(value) = n.try_into()
            && let Some(id) = permanent::small_integer_id(value)
        {
            return id.to_string();
        }
        self.lit(0, n.to_string().as_bytes())
    }

    /// Load a code name and use it as the second operand of a boxed operation.
    pub(crate) fn named_op(&mut self, op: &str, receiver: &str, name: &str, value: &str) -> String {
        self.output.interned_identifiers.insert(name.to_owned());
        if op == "lookup" {
            // The runtime lookup propagates a pending exception before reading the scope.
            self.take_matching_check();
            let data = self.output.data(name.as_bytes());
            return self.checked(&format!(
                "call i64 @imre_dyn_lookup(ptr %ctx, i64 {receiver}, ptr {data}, i64 {})",
                name.len()
            ));
        }
        if !self.output.declarations.emitted_named_op {
            self.output.functions.push_str(NAMED_OP);
            self.output.declarations.emitted_named_op = true;
        }
        let operation = if self.take_matching_check() {
            if !self.output.declarations.emitted_guarded_named_op {
                self.output.functions.push_str(GUARDED_NAMED_OP);
                self.output.declarations.emitted_guarded_named_op = true;
            }
            "@imre_boxed_guarded_named_op"
        } else {
            "@imre_boxed_named_op"
        };
        let opcode = self.output.data(op.as_bytes());
        let data = self.output.data(name.as_bytes());
        self.checked(&format!(
            "call i64 {operation}(ptr %ctx, ptr {opcode}, i64 {receiver}, ptr {data}, i64 {}, i64 {value})",
            name.len()
        ))
    }

    /// Evaluate Python truthiness and produce the LLVM branch condition.
    pub(crate) fn exact_word(&mut self, object: &str) -> String {
        self.output.declarations.reads_exact_int_word = true;
        self.value(&format!(
            "call i64 @imre_dyn_exact_int_word(ptr %ctx, i64 {object})"
        ))
    }

    pub(crate) fn enter_test(
        &mut self,
        range: TextRange,
        anchor: TextSize,
    ) -> (TextRange, SourcePosition) {
        let previous = std::mem::replace(&mut self.current_range, range);
        (previous, self.enter_expression(range, anchor))
    }

    pub(crate) fn leave_test(&mut self, (range, position): (TextRange, SourcePosition)) {
        self.leave_expression(position);
        self.current_range = range;
    }

    pub(crate) fn boolean_result(&mut self, bit: &str, negate: bool) -> super::CheckedValue {
        let (yes, no) = if negate {
            (permanent::FALSE, permanent::TRUE)
        } else {
            (permanent::TRUE, permanent::FALSE)
        };
        super::CheckedValue::handle(self.value(&format!("select i1 {bit}, i64 {yes}, i64 {no}")))
    }

    pub(crate) fn begin_operand_scope(&mut self) -> (String, String) {
        let failed = self.name("b");
        let outer = std::mem::replace(&mut self.body.error_target, failed.clone());
        (failed, outer)
    }

    pub(crate) fn finish_unwind(
        &mut self,
        (failed, outer): (String, String),
        release: impl FnOnce(&mut Self),
    ) {
        let ready = self.name("b");
        self.jump(&ready);
        self.label(&failed);
        release(self);
        self.jump(&outer);
        self.label(&ready);
        self.body.error_target = outer;
    }

    pub(crate) fn finish_operand_scope(&mut self, mark: &str, targets: (String, String)) {
        self.finish_unwind(targets, |frame| frame.discard_truth(mark));
    }

    /// Keep the exact comparison result alive while releasing either operand.
    pub(crate) fn comparison_result(
        &mut self,
        mark: &str,
        op: &str,
        left: &str,
        right: &str,
    ) -> String {
        let result = self.op(op, &[left, right]);
        self.keep_operands(mark, &result, "0");
        result
    }

    pub(crate) fn truth(&mut self, value: &str) -> String {
        self.truth_at(value, None)
    }

    pub(crate) fn truth_at(&mut self, value: &str, range: Option<TextRange>) -> String {
        let span = range.and_then(|range| {
            let (line, start) = self.positions.line_and_column(range.start());
            let (end_line, end) = self.positions.line_and_column(range.end());
            (line == end_line).then_some([line, start, start, end])
        });
        // A truth callback may inspect the active operand or raise across a continued test.
        let previous = range
            .filter(|_| span.is_none() || matches!(self.position, SourcePosition::Expression(_)))
            .map(|range| self.replace_position(SourcePosition::Expression(range)));
        let boolean = self.op_at("truth", &[value], span);
        if let Some(previous) = previous {
            self.leave_expression(previous);
        }
        self.value(&format!("icmp eq i64 {boolean}, 2"))
    }

    /// End the lifetime of the operands evaluated after the mark.
    pub(crate) fn discard_truth(&mut self, mark: &str) {
        self.line(&format!(
            "call void @imre_dyn_discard_truth(ptr %ctx, i64 {mark})"
        ));
    }

    /// Pop owned operands from the top, including the elements of a last-owned sequence.
    pub(super) fn release_owned_operands(&mut self, mark: &str) {
        self.line(&format!(
            "call void @imre_dyn_unwind_display(ptr %ctx, i64 {mark})"
        ));
    }

    /// Keep just the live stack operands when Python may observe their release.
    pub(crate) fn keep_operands(&mut self, mark: &str, first: &str, second: &str) -> String {
        self.value(&format!(
            "call i64 @imre_dyn_keep_operands(ptr %ctx, i64 {mark}, i64 {first}, i64 {second})"
        ))
    }

    pub(crate) fn format_conversion(&mut self, value: &str, conversion: char) -> String {
        let flag = self.text(&conversion.to_string());
        self.format_at_field("format_conversion", &[value, &flag])
    }

    pub(crate) fn format_field(&mut self, value: &str, spec: &str) -> String {
        self.format_at_field("format", &[value, "0", spec])
    }

    fn format_at_field(&mut self, op: &str, args: &[&str]) -> String {
        self.call_position(self.current_range);
        self.op(op, args)
    }

    /// Join completed fields without dispatching to a Python string subclass's addition.
    pub(crate) fn format_join(&mut self, mark: &str, parts: Vec<String>) -> String {
        let result = parts
            .into_iter()
            .reduce(|left, right| self.op("format_concat", &[&left, &right]))
            .unwrap_or_else(|| self.text(""));
        self.line(&format!(
            "call void @imre_dyn_finish_format_join(ptr %ctx, i64 {mark}, i64 {result})"
        ));
        result
    }

    /// Save the caller's earlier operands before an expression can release a writer.
    pub(crate) fn temporary_mark(&mut self) -> String {
        self.value("call i64 @imre_dyn_truth_mark(ptr %ctx)")
    }

    pub(crate) fn expression_range_span(
        &self,
        range: TextRange,
        prefix_end: TextSize,
        opener: u8,
    ) -> Option<[usize; 4]> {
        if self.simple_call == Some(range) {
            return None;
        }
        let start = range.start().to_usize();
        let end = range.end().to_usize();
        let prefix_end = prefix_end.to_usize();
        let text = self.positions.text.as_bytes();
        if text.get(start..end)?.contains(&b'\n') {
            return None;
        }
        let split = imre_ast::traceback_delimiter(text, prefix_end, end, opener)?;
        let (line, col) = self.positions.line_and_column(range.start());
        Some([line, col, col + (split - start), col + (end - start)])
    }

    /// Pass consecutive private roots in place. Mixed or discontiguous roots need a snapshot.
    fn live_handles(&mut self, trailing: &[&str]) -> (String, usize) {
        if trailing.is_empty()
            && self.live.is_empty()
            && let Some(first) = self.live_slots.first()
            && self
                .live_slots
                .windows(2)
                .all(|pair| pair[1] == pair[0] + 1)
        {
            return (format!("%ir_slot{first}"), self.live_slots.len());
        }
        let (_, prefix) = self.handle_array(&self.live.clone());
        let mut copied = 0;
        while let Some(run) = self.live_slots[copied..]
            .chunk_by(|a, b| *b == *a + 1)
            .next()
        {
            let first = run[0];
            let len = run.len();
            let offset = prefix + copied;
            let dst = self.value(&format!("getelementptr i64, ptr %live, i64 {offset}"));
            if len == 1 {
                let value = self.value(&format!("load i64, ptr %ir_slot{first}"));
                self.line(&format!("store i64 {value}, ptr {dst}"));
            } else {
                self.line(&format!(
                    "call void @llvm.memcpy.p0.p0.i64(ptr {dst}, ptr %ir_slot{first}, i64 {}, i1 false)",
                    len * 8
                ));
            }
            copied += len;
        }
        let (ptr, count) = self.write_handles_at(prefix + copied, trailing);
        (ptr.to_owned(), count)
    }

    /// Copy evaluated handles into reusable storage in this activation's entry block.
    pub(crate) fn handle_array(&mut self, values: &[impl AsRef<str>]) -> (&'static str, usize) {
        self.write_handles_at(0, values)
    }

    fn write_handles_at(
        &mut self,
        offset: usize,
        values: &[impl AsRef<str>],
    ) -> (&'static str, usize) {
        let count = offset + values.len();
        self.max_live = self.max_live.max(count);
        for (i, value) in (offset..).zip(values) {
            let value = value.as_ref();
            let ptr = self.value(&format!("getelementptr i64, ptr %live, i64 {i}"));
            self.line(&format!("store i64 {value}, ptr {ptr}"));
        }
        (if count == 0 { "null" } else { "%live" }, count)
    }

    pub(crate) fn checkpoint(&mut self, range: TextRange) {
        self.checkpoint_with_roots(range, &[]);
    }

    /// Place temporary operands after all outer frame roots, including private slots.
    pub(super) fn checkpoint_with_roots(&mut self, range: TextRange, trailing: &[&str]) {
        let (ptr, count) = self.live_handles(trailing);
        let line = self.positions.line(range.start());
        self.position = SourcePosition::statement(line);
        self.line(&format!(
            "call void @imre_dyn_statement_with_exception(ptr %ctx, ptr {ptr}, i64 {count}, ptr {}, i32 {line}, i64 {})",
            self.positions.file, self.caught.last().map_or("0", String::as_str)
        ));
        self.check_error();
    }

    pub(super) fn position_constant(&mut self, range: TextRange) -> String {
        let (line, start) = self.positions.line_and_column(range.start());
        let (end_line, end) = self.positions.line_and_column(range.end());
        self.output.position([line, end_line, start, end])
    }

    /// A live caller retains the full range even when traceback highlighting is suppressed.
    pub(crate) fn call_position(&mut self, range: TextRange) {
        let range = match self.position {
            SourcePosition::Expression(active) if self.current_range == range => active,
            _ => range,
        };
        let position = self.position_constant(range);
        self.line(&format!(
            "call void @imre_dyn_call_position(ptr %ctx, ptr {position})"
        ));
    }

    pub(crate) fn positioned_call(
        &mut self,
        callable: &str,
        operands: CallArguments,
        mark: &str,
        range: TextRange,
        callee_end: TextSize,
        scope: &str,
    ) -> String {
        let span = self.expression_range_span(range, callee_end, b'(');
        self.call_position(range);
        self.call_in_scope(callable, operands, mark, span, scope)
    }

    /// Pass the caller's temporary root boundary and optional traceback position.
    pub(crate) fn call_at(
        &mut self,
        callable: &str,
        operands: CallArguments,
        mark: &str,
        span: Option<[usize; 4]>,
    ) -> String {
        self.call_in_scope(callable, operands, mark, span, &self.scope.clone())
    }

    pub(crate) fn call_in_scope(
        &mut self,
        callable: &str,
        operands: CallArguments,
        mark: &str,
        span: Option<[usize; 4]>,
        scope: &str,
    ) -> String {
        let position = span.map_or_else(|| "null".into(), |span| self.output.position(span));
        let (function, args) = match operands {
            CallArguments::PartitionField {
                array,
                index,
                get_position,
            } => (
                "imre_dyn_partition_field_at",
                format!("ptr {array}, i32 {index}, ptr {get_position}"),
            ),
            CallArguments::FixedKeywords {
                array,
                positional,
                names,
                keywords,
            } => (
                "imre_dyn_keyword_call_at",
                format!("ptr {array}, i64 {positional}, ptr {names}, i64 {keywords}"),
            ),
            CallArguments::Packed { args, kwargs } => (
                "imre_dyn_scoped_call_at",
                format!("i64 {args}, i64 {kwargs}"),
            ),
            CallArguments::Positional {
                array: ptr,
                count,
                receiver_slot,
            } => (
                if receiver_slot {
                    "imre_dyn_positional_method_at"
                } else {
                    "imre_dyn_positional_call_at"
                },
                format!("ptr {ptr}, i64 {count}"),
            ),
        };
        self.checked(&format!("call i64 @{function}(ptr %ctx, i64 {callable}, {args}, i64 {mark}, ptr {position}, i64 {scope})"))
    }

    pub(crate) fn assign_owned_name(
        &mut self,
        scope: &str,
        name: &str,
        value: &str,
        mark: Option<&str>,
    ) {
        let replaced = self.op("assign_target", &[scope, name, value]);
        self.release_replaced(&replaced, mark.map(|mark| (mark, value)));
    }

    /// An item/attribute setter can replace an owner without reporting which object it dropped.
    pub(crate) fn release_target(&mut self, mark: Option<&str>) {
        self.release_replaced("2", mark.map(|mark| (mark, "0")));
    }

    /// Preserve earlier expression operands when present, or the statement's explicit live handles.
    pub(crate) fn release_replaced(&mut self, replaced: &str, expression: Option<(&str, &str)>) {
        if let Some((mark, value)) = expression {
            self.line(&format!("call void @imre_dyn_named_assignment(ptr %ctx, i64 {mark}, i64 {value}, i64 {replaced})"));
        } else {
            let (ptr, count) = self.live_handles(&[]);
            self.line(&format!("call void @imre_dyn_assignment_target(ptr %ctx, ptr {ptr}, i64 {count}, i64 {replaced})"));
        }
    }
}

impl RegisteredFrame<'_, '_> {
    pub(crate) fn text(&mut self, value: &str) -> String {
        if !value.is_empty() && interned_spelling(value) {
            self.output.interned_identifiers.insert(value.to_owned());
        }
        self.lit(1, value.as_bytes())
    }

    pub(crate) fn lookup(&mut self, name: &str) -> String {
        let op = match &self.class_locals {
            Some(class) if self.annotation_values.is_some() && class.globals.contains(name) => {
                "lookup_annotation_global"
            }
            Some(class) if self.annotation_values.is_some() && class.locals.contains(name) => {
                "lookup_class_annotation_global"
            }
            Some(_) if self.annotation_values.is_some() => "lookup_class_annotation",
            Some(class) if class.locals.contains(name) => "lookup_class_local",
            _ => "lookup",
        };
        self.named_op(op, &self.scope.clone(), name, "0")
    }

    pub(crate) fn bind(&mut self, name: &str, value: &str) {
        self.named_op("assign", &self.scope.clone(), name, value);
    }

    pub(crate) fn attr(&mut self, object: &str, name: &str) -> String {
        self.named_op("attr", object, name, "0")
    }

    pub(crate) fn namespace_import(&mut self, import: &ir::NamespaceImport) {
        match import {
            ir::NamespaceImport::Modules(modules) => {
                for item in modules {
                    let mut module = self.import_name(&item.name, "0", 0);
                    if item.bind_leaf {
                        for part in item.name.split('.').skip(1) {
                            let member = self.code_name(part);
                            module = self.op("from", &[&module, &member]);
                        }
                    }
                    self.bind(&item.binding, &module);
                }
            }
            ir::NamespaceImport::From {
                module,
                level,
                names,
            } => {
                let fromlist = self.op("list", &[]);
                let members: Vec<_> = names.iter().map(|(name, _)| self.code_name(name)).collect();
                for member in &members {
                    self.op("append", &[&fromlist, member]);
                }
                let fromlist = self.op("tuple", &[&fromlist]);
                let module = self.import_name(module, &fromlist, *level);
                for ((name, binding), member) in names.iter().zip(members) {
                    if name == "*" {
                        self.op("import_star", &[&self.scope.clone(), &module]);
                    } else {
                        let value = self.op("from", &[&module, &member]);
                        self.bind(binding, &value);
                    }
                }
            }
        }
    }

    pub(crate) fn import_name(&mut self, name: &str, fromlist: &str, level: u32) -> String {
        let name = self.code_name(name);
        self.checked(&format!(
            "call i64 @imre_dyn_import_name(ptr %ctx, i64 {}, i64 {name}, i64 {fromlist}, i32 {level})",
            self.scope
        ))
    }

    pub(crate) fn begin_annotations(&mut self) {
        // The format argument belongs to this callback, even if the class also defines it.
        let format = self.lookup("format");
        self.op("annotation_format", &[&format]);
        let (globals, conditional) = match &self.annotation_scope {
            AnnotationScope::Class {
                globals,
                conditional,
            } => (globals.clone(), *conditional),
            scope => {
                if let AnnotationScope::Method(class) = scope {
                    self.class_locals = Some(class.clone());
                }
                self.annotation_values = Some(self.op("dict", &[]));
                self.scope = self.op("annotation_scope", &[&self.scope.clone()]);
                return;
            }
        };
        if conditional {
            self.annotation_active = Some(self.lookup("__conditional_annotations__"));
        }
        let dict = self.lookup("__classdict__");
        let local = self.op("class_annotation_locals", &[&self.scope.clone(), &dict]);
        for name in globals {
            let name = self.code_name(&name);
            self.op("declare_global", &[&local, &name]);
        }
        let result = self.op("dict", &[]);
        self.live.extend([local.clone(), result.clone()]);
        self.scope = local;
        self.annotation_values = Some(result);
    }

    pub(crate) fn take_annotation_result(&mut self) -> String {
        self.annotation_values
            .take()
            .expect("annotation dictionary")
    }

    // Equal constants are merged across nested code objects in one compilation.
    pub(crate) fn await_at(
        &mut self,
        value: &str,
        mode: &str,
        range: TextRange,
        mark: &str,
    ) -> String {
        self.call_position(range);
        let iterator = self.op("await_iter", &[value, mode]);
        let result = self.suspend_at(&iterator, Suspension::Await, range, mark);
        self.keep_operands(mark, &result, "0");
        result
    }

    pub(crate) fn retained_loop_header(&mut self, header: TextSize) {
        if self.native_suspension && !self.iteration.in_expression {
            // A retained frame exposes the statement header on each pull, including exhaustion.
            self.set_statement_line(self.positions.line(header));
        }
    }

    /// Branch before binding the target. An async pull delivers its item while awaiting.
    pub(crate) fn iteration_edge(
        &mut self,
        cursor: &str,
        kind: ir::IterNextKind,
        header: TextSize,
        has_else: bool,
        item: &str,
        exhausted: &str,
    ) -> Option<String> {
        match kind {
            ir::IterNextKind::Generic => {
                self.retained_loop_header(header);
                let more = self.op(if has_else { "more_else" } else { "more" }, &[cursor]);
                let ready = self.value(&format!("icmp eq i64 {more}, {}", permanent::TRUE));
                self.branch(&ready, item, exhausted);
                None
            }
            ir::IterNextKind::Async(pull) => Some(self.async_pull(cursor, pull, item, exhausted)),
            _ => unreachable!("registered iterator kind"),
        }
    }

    pub(crate) fn iteration_item(&mut self, cursor: &str, pulled: Option<String>) -> String {
        pulled.unwrap_or_else(|| self.op("next", &[cursor]))
    }

    /// Only the implicit __anext__ call and its await can exhaust an async for.
    pub(crate) fn async_pull(
        &mut self,
        iter: &str,
        pull: ir::AsyncPull,
        body: &str,
        exhausted: &str,
    ) -> String {
        if let Some(header) = pull.header {
            self.set_statement_line(self.positions.line(header));
        }
        let handler = self.name("b");
        let outer = std::mem::replace(&mut self.body.error_target, handler.clone());
        let mark = self.temporary_mark();
        let next = self.op("async_next_iter", &[iter]);
        let item = self.suspend_at(&next, Suspension::Await, pull.range, &mark);
        self.flush_check();
        self.body.error_target = outer;
        self.jump(body);
        self.label(&handler);
        self.discard_truth(&mark);
        let error = self.op("take_error", &[]);
        let name = self.text("StopAsyncIteration");
        let class = self.op("builtin", &[&name]);
        let matches = self.op("matches", &[&error, &class]);
        let stop = self.value(&format!("icmp eq i64 {matches}, 2"));
        let propagate = self.name("b");
        self.branch(&stop, exhausted, &propagate);
        self.label(&propagate);
        self.raise(&error, None, true);
        item
    }

    pub(crate) fn suspend_at(
        &mut self,
        value: &str,
        kind: Suspension,
        range: TextRange,
        mark: &str,
    ) -> String {
        self.call_position(range);
        self.line(&format!(
            "call void @imre_dyn_suspend_operands(ptr %context_slot, i64 {value}, i64 {mark}, i32 {})",
            kind as i32
        ));
        let saved = self.value("call token @llvm.coro.save(ptr %handle)");
        let branch = self.value(&format!(
            "call i8 @llvm.coro.suspend(token {saved}, i1 false)"
        ));
        let resume = self.name("b");
        self.end(&format!(
            "switch i8 {branch}, label %parked [ i8 0, label %{resume} i8 1, label %discard ]"
        ));
        self.label(&resume);
        self.checked("call i64 @imre_dyn_resume_input(ptr %context_slot)")
    }
}

pub(crate) struct AnnotationTarget {
    key: String,
    after: Option<String>,
}

impl RegisteredFrame<'_, '_> {
    pub(crate) fn annotation_target(
        &mut self,
        index: Option<usize>,
        name: &str,
        range: TextRange,
    ) -> AnnotationTarget {
        let after = index.map(|index| {
            let active = self
                .annotation_active
                .clone()
                .unwrap_or_else(|| self.lookup("__conditional_annotations__"));
            let index = self.integer(index);
            let present = self.op("in", &[&index, &active]);
            let condition = self.truth(&present);
            let yes = self.name("b");
            let after = self.name("b");
            self.branch(&condition, &yes, &after);
            self.label(&yes);
            after
        });
        let key = self.code_name(name);
        if matches!(self.annotation_scope, AnnotationScope::Class { .. }) {
            self.checkpoint(range);
        }
        AnnotationTarget { key, after }
    }

    pub(crate) fn store_annotation(&mut self, target: AnnotationTarget, value: &str) {
        let AnnotationTarget { key, after } = target;
        let values = self
            .annotation_values
            .as_ref()
            .expect("annotation dictionary")
            .clone();
        self.op("put", &[&values, &key, value]);
        if let Some(after) = after {
            self.jump(&after);
            self.label(&after);
        }
    }
}

/// Builds the positional and keyword containers while the caller evaluates each operand.
pub(crate) struct ArgumentPack<'a> {
    layout: &'a ir::PackedArguments,
    callable: Option<&'a str>,
    positional: String,
    keywords: String,
    group: Option<String>,
    deferred: Option<String>,
}

pub(crate) enum ArgumentSlot<'a> {
    Positional,
    Spread(String),
    Deferred,
    Keyword {
        name: &'a str,
        last: bool,
    },
    Mapping {
        mark: String,
        edge: (String, String),
    },
}

impl<'a> ArgumentPack<'a> {
    pub(crate) fn new(
        frame: &mut RegisteredFrame<'_, '_>,
        layout: &'a ir::PackedArguments,
        callable: Option<&'a str>,
    ) -> Self {
        // A call converts its lone * operand after the keywords. Class bases expand immediately.
        let deferred =
            (callable.is_some() && layout.starred == [true]).then(|| frame.temporary_mark());
        let positional = if deferred.is_some() {
            String::new()
        } else {
            frame.op("list", &[])
        };
        Self {
            layout,
            callable,
            positional,
            keywords: "0".into(),
            group: None,
            deferred,
        }
    }

    pub(crate) fn prepare(
        &mut self,
        frame: &mut RegisteredFrame<'_, '_>,
        index: usize,
    ) -> ArgumentSlot<'a> {
        if let Some(starred) = self.layout.starred.get(index) {
            return if self.deferred.is_some() {
                ArgumentSlot::Deferred
            } else if *starred {
                ArgumentSlot::Spread(frame.temporary_mark())
            } else {
                ArgumentSlot::Positional
            };
        }
        let index = index - self.layout.starred.len();
        let keywords = &self.layout.keywords;
        if index == 0 {
            self.keywords = frame.op("dict", &[]);
        }
        if let Some(name) = &keywords[index] {
            // Evaluate a group of named operands following ** before checking duplicates.
            if index > 0 && keywords[index - 1].is_none() {
                self.group = Some(frame.op("dict", &[]));
            }
            ArgumentSlot::Keyword {
                name,
                last: keywords.get(index + 1).is_none_or(Option::is_none),
            }
        } else {
            ArgumentSlot::Mapping {
                mark: frame.temporary_mark(),
                edge: frame.open_operand_unwind(),
            }
        }
    }

    pub(crate) fn insert(
        &mut self,
        frame: &mut RegisteredFrame<'_, '_>,
        slot: ArgumentSlot<'_>,
        value: &str,
    ) {
        match slot {
            ArgumentSlot::Positional => {
                frame.op("append", &[&self.positional, value]);
            }
            ArgumentSlot::Deferred => self.positional = value.to_owned(),
            ArgumentSlot::Spread(mark) => {
                frame.pack_operand("extend", &[&self.positional, value]);
                frame.discard_truth(&mark);
            }
            ArgumentSlot::Keyword { name, last } => {
                frame.named_op(
                    "keyword",
                    self.group.as_ref().unwrap_or(&self.keywords),
                    name,
                    value,
                );
                if last && let Some(group) = self.group.take() {
                    self.merge(frame, &group);
                }
            }
            ArgumentSlot::Mapping { mark, edge } => {
                self.merge(frame, value);
                frame.discard_truth(&mark);
                frame.close_unwind(edge, |frame| frame.discard_truth(&mark));
            }
        }
    }

    fn merge(&self, frame: &mut RegisteredFrame<'_, '_>, value: &str) {
        let (operation, callable) = self
            .callable
            .map_or(("class_keywords", "0"), |c| ("keywords", c));
        frame.pack_operand(operation, &[&self.keywords, value, callable]);
    }

    pub(crate) fn finish(self, frame: &mut RegisteredFrame<'_, '_>) -> (String, String) {
        let positional = if let Some(mark) = self.deferred {
            let values = frame.pack_operand(
                "spread_arguments",
                &[&self.positional, self.callable.unwrap()],
            );
            frame.keep_operands(&mark, &values, &self.keywords);
            values
        } else {
            self.positional
        };
        (positional, self.keywords)
    }
}

/// Share operand ordering and unwind edges while each caller supplies its own operand emitter.
pub(crate) trait FrameEmitter<'a, 'p>:
    std::ops::DerefMut<Target = RegisteredFrame<'a, 'p>> + Sized
{
    /// Class argument expansion is interleaved with the next Python operand.
    fn class_arguments(
        &mut self,
        layout: &ir::PackedArguments,
        mut operand: impl FnMut(&mut Self, usize) -> String,
    ) -> (String, String) {
        let mut pack = ArgumentPack::new(self, layout, None);
        for index in 0..layout.starred.len() + layout.keywords.len() {
            let slot = pack.prepare(self, index);
            let value = operand(self, index);
            pack.insert(self, slot, &value);
        }
        pack.finish(self)
    }

    fn discard_operand(&mut self, evaluate: impl FnOnce(&mut Self) -> String) {
        let mark = self.temporary_mark();
        evaluate(self);
        self.discard_truth(&mark);
    }

    /// Append each interpolation after its value and spec have been evaluated. An
    /// unavailable field ends this path after the preceding Python operations.
    fn template<T>(
        &mut self,
        segments: &[Vec<u8>],
        fields: &[T],
        mut field: impl FnMut(&mut Self, &T) -> std::ops::ControlFlow<String, String>,
    ) -> String {
        use std::ops::ControlFlow;

        let strings = self.emit_string_tuple(segments.to_vec());
        let parts = self.op("list", &[]);
        for input in fields {
            match field(self, input) {
                ControlFlow::Continue(value) => {
                    self.op("append", &[&parts, &value]);
                }
                ControlFlow::Break(exit) => return exit,
            }
        }
        self.op("build_template", &[&strings, &parts])
    }

    fn push_loop(&mut self, edges: LoopEdges, step_depth: usize, exit_depth: usize) {
        self.loops.push(super::LoopTarget {
            edges,
            step_depth,
            exit_depth,
        });
    }

    fn unwind_loop(&mut self, stmt: &ir::Statement, unwind: impl FnOnce(&mut Self, usize)) {
        let target = self.loops.last().expect("loop transfer");
        let (label, depth) = match stmt {
            ir::Statement::Break => (&target.edges.done, target.exit_depth),
            ir::Statement::Continue => (&target.edges.step, target.step_depth),
            ir::Statement::Exhaust => (&target.edges.exhausted, target.step_depth),
            _ => unreachable!("loop transfer"),
        };
        let label = label.clone();
        unwind(self, depth);
        self.jump(&label);
    }

    /// A bare raise transfers the active exception immediately. An explicit raise
    /// roots its evaluated exception until the caller has evaluated the cause.
    fn raise_operands<T>(
        &mut self,
        exception: Option<T>,
        cause: Option<T>,
        operand: impl Fn(&mut Self, T) -> String,
    ) {
        let Some(exception) = exception else {
            let active = self.caught.last().cloned().unwrap_or_else(|| "0".into());
            self.raise(&active, None, true);
            return;
        };
        let statement = self.current_range;
        let exception = operand(self, exception);
        self.live.push(exception);
        let cause = cause.map(|value| operand(self, value));
        let exception = self.live.pop().expect("pending explicit raise");
        self.replace_position(SourcePosition::Expression(statement));
        self.raise(&exception, cause.as_deref(), false);
    }

    fn with_operand_source<T>(&mut self, expr: &ir::Expr, emit: impl FnOnce(&mut Self) -> T) -> T {
        if let ir::ExprKind::Builtin(ir::Builtin::RegisteredLocation(range, _), _) = &expr.kind {
            self.with_source_range(*range, emit)
        } else {
            emit(self)
        }
    }

    fn with_source_range<T>(&mut self, range: TextRange, emit: impl FnOnce(&mut Self) -> T) -> T {
        let previous = self.replace_position(SourcePosition::Expression(range));
        let result = emit(self);
        self.leave_expression(previous);
        result
    }

    fn test_with_temporaries(&mut self, test: impl FnOnce(&mut Self) -> String) -> String {
        let mark = self.temporary_mark();
        let condition = test(self);
        self.discard_truth(&mark);
        condition
    }

    fn contains_literal(&mut self, value: &str, items: &[String], negate: bool) -> String {
        let (array, count) = self.handle_array(items);
        let negate = u32::from(negate);
        self.checked(&format!("call i64 @imre_dyn_contains_literal_tuple(ptr %ctx, i64 {value}, ptr {array}, i64 {count}, i32 {negate})"))
    }

    /// The activation keeps its live handles until the next Python statement.
    /// Release the iterable when its cursor exists, and the cursor and local scope
    /// when the comprehension delivers its result to the enclosing expression.
    fn iteration_result(&mut self, emit: impl FnOnce(&mut Self) -> String) -> String {
        if !self.native_suspension {
            return emit(self);
        }
        let mark = self.temporary_mark();
        let result = self.unwind_operands(&mark, emit);
        self.keep_operands(&mark, &result, "0");
        result
    }

    fn unwind_operands<T>(&mut self, mark: &str, body: impl FnOnce(&mut Self) -> T) -> T {
        self.unwind_with(|f| f.discard_truth(mark), body)
    }

    /// Release incomplete operands in evaluation-stack order before entering the handler.
    fn unwind_stack_operands<T>(&mut self, mark: &str, body: impl FnOnce(&mut Self) -> T) -> T {
        self.unwind_with(
            |frame| {
                frame.line(&format!(
                    "call void @imre_dyn_unwind_operands(ptr %ctx, i64 {mark})"
                ));
            },
            body,
        )
    }

    fn suspend_value(
        &mut self,
        kind: crate::ir::SuspendValue,
        range: TextRange,
        evaluate: impl FnOnce(&mut Self) -> String,
    ) -> String {
        let mark = self.temporary_mark();
        self.unwind_operands(&mark, |code| {
            let value = evaluate(code);
            match kind {
                crate::ir::SuspendValue::Await => code.await_at(&value, "0", range, &mark),
                crate::ir::SuspendValue::Delegate => {
                    code.suspend_at(&value, Suspension::Delegate, range, &mark)
                }
                crate::ir::SuspendValue::Yield => {
                    let mode = if code.async_generator {
                        Suspension::AsyncYield
                    } else {
                        Suspension::Yield
                    };
                    code.suspend_at(&value, mode, range, &mark)
                }
            }
        })
    }

    fn slice_object(&mut self, [start, stop, step]: [String; 3]) -> String {
        self.op("slice_object", &[&start, &stop, &step])
    }

    fn read_item(
        &mut self,
        base: &str,
        key: ReadKey,
        range: TextRange,
        base_end: TextSize,
    ) -> String {
        let span = self.expression_range_span(range, base_end, b'[');
        match key {
            ReadKey::Object(key) => self.op_at("get", &[base, &key], span),
            ReadKey::Bounds { start, stop, slot } => {
                self.take_matching_check();
                let position =
                    span.map_or_else(|| "null".into(), |span| self.output.position(span));
                if let Some(slot) = slot {
                    self.checked(&format!("call i64 @imre_dyn_get_linked_slice_at(ptr %ctx, i64 {base}, i64 {start}, i64 {stop}, ptr {slot}, ptr {position})"))
                } else {
                    self.checked(&format!("call i64 @imre_dyn_get_slice_at(ptr %ctx, i64 {base}, i64 {start}, i64 {stop}, ptr {position})"))
                }
            }
        }
    }

    fn write(&mut self, place: &Place, value: &str) {
        match place {
            Place::Name(n) => {
                let scope = self.scope.clone();
                self.op("assign", &[&scope, n, value])
            }
            Place::Item(v, k) => self.put_item(v, k, value),
        };
    }

    fn inplace(&mut self, op: &str, left: &str, right: &str) -> String {
        self.op(&format!("inplace:{op}"), &[left, right])
    }

    fn delete_item(&mut self, receiver: &str, key: &str) -> String {
        self.op("delitem", &[receiver, key])
    }

    fn put_item(&mut self, receiver: &str, index: &str, value: &str) -> String {
        self.op("put", &[receiver, index, value])
    }

    /// Construct a Python dictionary with CPython's BUILD_MAP, MAP_ADD and DICT_UPDATE order.
    fn dict_display<'e, T: 'e>(
        &mut self,
        items: impl Iterator<Item = (Option<&'e T>, &'e T)>,
        mut evaluate: impl FnMut(&mut Self, &T) -> String,
    ) -> String {
        // CPython's codegen_dict flushes on the pair *after* the buffered operands exceed
        // its stack-use guideline. codegen_subdict streams any chunk over that guideline.
        const STACK_USE_GUIDELINE: usize = 30;
        let mut items = items.peekable();
        if items.peek().is_none() {
            return self.op("dict", &[]);
        }

        let mark = self.temporary_mark();
        self.unwind_operands(&mark, |f| {
            let mut result = None;
            while let Some((key, value)) = items.next() {
                let Some(key) = key else {
                    let dict = result.get_or_insert_with(|| f.op("dict", &[]));
                    let mapping = evaluate(f, value);
                    f.op("update", &[dict, &mapping]);
                    f.keep_operands(&mark, dict, "0");
                    continue;
                };

                let mut chunk = vec![(key, value)];
                while chunk.len() < STACK_USE_GUIDELINE / 2 + 2
                    && let Some((Some(key), value)) = items.next_if(|(key, _)| key.is_some())
                {
                    chunk.push((key, value));
                }
                let streamed = (2 * chunk.len() > STACK_USE_GUIDELINE).then(|| f.op("dict", &[]));
                let mut pending = Vec::new();
                for (key, value) in chunk {
                    let key = evaluate(f, key);
                    let value = evaluate(f, value);
                    if let Some(part) = &streamed {
                        f.op("put", &[part, &key, &value]);
                        f.keep_operands(&mark, part, result.as_deref().unwrap_or("0"));
                    } else {
                        pending.push((key, value));
                    }
                }
                let part = streamed.unwrap_or_else(|| {
                    let part = f.op("dict", &[]);
                    for (key, value) in pending {
                        f.op("put", &[&part, &key, &value]);
                    }
                    f.keep_operands(&mark, &part, result.as_deref().unwrap_or("0"));
                    part
                });
                if let Some(dict) = &result {
                    f.op("update", &[dict, &part]);
                    f.keep_operands(&mark, dict, "0");
                } else {
                    result = Some(part);
                }
            }
            result.expect("nonempty dictionary display")
        })
    }

    fn object_operation<'e, T: 'e>(
        &mut self,
        op: &Builtin,
        target: &'e T,
        mut args: impl Iterator<Item = &'e T>,
        mut evaluate: impl FnMut(&mut Self, &T) -> String,
    ) -> String {
        match op {
            Builtin::RegisteredDeleteItem => {
                let mark = self.temporary_mark();
                let targets = self.begin_operand_scope();
                let receiver = evaluate(self, target);
                let key_mark = self.keep_operands(&mark, &receiver, "0");
                let key = evaluate(self, args.next().expect("indexed deletion key"));
                self.keep_operands(&key_mark, &key, "0");
                let result = self.delete_item(&receiver, &key);
                self.finish_unwind(targets, |frame| frame.release_owned_operands(&mark));
                self.release_owned_operands(&mark);
                result
            }
            Builtin::RegisteredCall(call) => self.call_value(call, target, args, evaluate),
            Builtin::RegisteredAttribute {
                action,
                name,
                range,
            } => self.attribute_operation(
                *action,
                name,
                *range,
                std::iter::once(target).chain(args),
                evaluate,
            ),
            _ => unreachable!("selected Python object operation"),
        }
    }

    fn begin_unary(&mut self) -> (String, (String, String)) {
        (self.temporary_mark(), self.begin_operand_scope())
    }

    /// Consume an evaluated operand using the temporary roots established before its evaluation.
    fn finish_unary(
        &mut self,
        (mark, edge): (String, (String, String)),
        op: ir::ObjectUnary,
        result: CheckedValue,
    ) -> CheckedValue {
        if op != ir::ObjectUnary::Not
            && let CheckedValue::Handle { id, .. } = &result
        {
            self.keep_operands(&mark, id, "0");
        } else {
            self.discard_truth(&mark);
        }
        self.finish_operand_scope(&mark, edge);
        result
    }

    /// Evaluate the callable before its arguments. A lone *iterable is converted
    /// at invocation, after the keywords; a mixed positional list expands each * in place.
    fn call_value<'e, T: 'e>(
        &mut self,
        call: &ir::RegisteredCall,
        target: &T,
        mut args: impl Iterator<Item = &'e T>,
        mut evaluate: impl FnMut(&mut Self, &T) -> String,
    ) -> String {
        use ir::{RegisteredArguments, RegisteredCallTarget};

        let mark = self.temporary_mark();
        if self.native_suspension
            && let RegisteredCallTarget::Callable {
                suspended_name: Some(name),
                ..
            } = &call.target
        {
            self.call_position(*name);
        }
        let (scope, target) = if call.explicit_scope {
            let scope = evaluate(self, target);
            (scope, args.next().expect("scoped call has a target"))
        } else {
            (self.scope.clone(), target)
        };
        let (callable, receiver, callee_end) = match &call.target {
            RegisteredCallTarget::Callable { end, .. } => (evaluate(self, target), None, *end),
            RegisteredCallTarget::Method { name, attribute } => self.unwind_operands(&mark, |f| {
                let receiver = evaluate(f, target);
                let direct = matches!(call.arguments, RegisteredArguments::Positional);
                let (callee, bound) = if let SourcePosition::Expression(active) = f.position {
                    let lookup =
                        TextRange::new(active.start().max(attribute.start()), attribute.end());
                    f.with_source_range(lookup, |f| {
                        f.method_target(&receiver, name, lookup, direct)
                    })
                } else {
                    f.method_target(&receiver, name, *attribute, direct)
                };
                f.keep_operands(&mark, &callee, bound.as_deref().unwrap_or("0"));
                (callee, bound, attribute.end())
            }),
        };
        let operands = match &call.arguments {
            RegisteredArguments::Positional => {
                let receiver_slot = receiver.is_some();
                let values: Vec<_> = receiver
                    .into_iter()
                    .chain(args.map(|a| evaluate(self, a)))
                    .collect();
                let (array, count) = self.handle_array(&values);
                if let Some((index, range)) = call.partition_field {
                    let span = self.expression_range_span(range, call.range.end(), b'[');
                    let get_position =
                        span.map_or_else(|| "null".into(), |span| self.output.position(span));
                    debug_assert!(receiver_slot && count == 2);
                    CallArguments::PartitionField {
                        array,
                        index,
                        get_position,
                    }
                } else {
                    CallArguments::Positional {
                        array,
                        count,
                        receiver_slot,
                    }
                }
            }
            RegisteredArguments::Keywords { positional, names } => {
                let values: Vec<_> = args.map(|a| evaluate(self, a)).collect();
                let (array, _) = self.handle_array(&values);
                CallArguments::FixedKeywords {
                    array,
                    positional: *positional,
                    keywords: names.len(),
                    names: self.output.data(names.join("\0").as_bytes()),
                }
            }
            RegisteredArguments::Packed(pack) => self.unwind_operands(&mark, |f| {
                let mut values = ArgumentPack::new(f, pack, Some(&callable));
                for (index, arg) in args.enumerate() {
                    let slot = values.prepare(f, index);
                    let value = evaluate(f, arg);
                    values.insert(f, slot, &value);
                }
                let (args, kwargs) = values.finish(f);
                CallArguments::Packed { args, kwargs }
            }),
        };
        self.positioned_call(&callable, operands, &mark, call.range, callee_end, &scope)
    }

    /// Execute a descriptor operation at the original target, using the same live receiver
    /// for both halves of augmented assignment. A mutation releases its operand window on
    /// success and unwinds it before transferring an exception to the handler.
    fn attribute_operation<'e, T: 'e>(
        &mut self,
        action: ir::AttributeAction,
        name: &str,
        range: ruff_text_size::TextRange,
        mut operands: impl Iterator<Item = &'e T>,
        mut evaluate: impl FnMut(&mut Self, &T) -> String,
    ) -> String {
        use ir::AttributeAction;
        let first = operands.next().expect("attribute operand");
        let mark = self.temporary_mark();
        if action == AttributeAction::Load {
            return self.unwind_operands(&mark, |code| {
                let receiver = evaluate(code, first);
                let value = code.attribute_value(&receiver, name, range);
                code.keep_operands(&mark, &value, "0");
                value
            });
        }
        self.unwind_operands(&mark, |code| {
            match action {
                AttributeAction::Store => {
                    let value = evaluate(code, first);
                    code.store_attribute(name, range, &value, |code| {
                        evaluate(code, operands.next().expect("attribute receiver"))
                    });
                }
                AttributeAction::Delete => {
                    let receiver = evaluate(code, first);
                    code.write_attribute(&receiver, name, range, None);
                }
                AttributeAction::InPlace(op) => {
                    let receiver = evaluate(code, first);
                    let right_mark = code.temporary_mark();
                    let value = code.unwind_operands(&right_mark, |code| {
                        let left = code.attribute_value(&receiver, name, range);
                        let right = evaluate(code, operands.next().expect("in-place RHS"));
                        let value = code.inplace(op, &left, &right);
                        // The binary operation consumed both operands before STORE_ATTR.
                        code.keep_operands(&right_mark, &value, "0");
                        value
                    });
                    // STORE_ATTR consumes its receiver before the value being stored.
                    let receiver_mark = code.keep_operands(&mark, &value, &receiver);
                    code.unwind_operands(&receiver_mark, |code| {
                        code.write_attribute(&receiver, name, range, Some(&value));
                        code.discard_truth(&receiver_mark);
                    });
                }
                AttributeAction::Load => unreachable!(),
            }
            code.discard_truth(&mark);
        });
        permanent::NONE.to_string()
    }

    fn store_attribute(
        &mut self,
        name: &str,
        range: ruff_text_size::TextRange,
        value: &str,
        receiver: impl FnOnce(&mut Self) -> String,
    ) {
        // The RHS is already owned and must outlive evaluation and disposal of the receiver.
        let mark = self.temporary_mark();
        self.unwind_operands(&mark, |code| {
            let receiver = receiver(code);
            code.write_attribute(&receiver, name, range, Some(value));
            code.discard_truth(&mark);
        });
    }

    fn unwind_with<T>(
        &mut self,
        release: impl FnOnce(&mut RegisteredFrame<'a, 'p>),
        body: impl FnOnce(&mut Self) -> T,
    ) -> T {
        let targets = self.begin_operand_scope();
        let result = body(self);
        self.finish_unwind(targets, release);
        result
    }

    fn checked_sequence(
        &mut self,
        values: &[Expr],
        display: DisplayKind,
        evaluate: impl FnMut(&mut Self, &Expr) -> String,
    ) -> String {
        let tuple = display == DisplayKind::Tuple;
        if display == DisplayKind::Set {
            return self.sequence_display(values, display, None, spread, evaluate);
        }
        if values.is_empty() {
            return if tuple {
                permanent::EMPTY_TUPLE.to_string()
            } else {
                self.op("list", &[])
            };
        }
        let slot = tuple
            .then(|| {
                values
                    .iter()
                    .map(Constant::from_expr)
                    .collect::<Option<Vec<_>>>()
            })
            .flatten()
            .map(|keys| self.constant_slot(Constant::Tuple(keys)));
        if let Some(handles) = values
            .iter()
            .map(permanent_handle)
            .collect::<Option<Vec<_>>>()
        {
            let data = self.output.data(
                &handles
                    .iter()
                    .flat_map(|id| id.to_le_bytes())
                    .collect::<Vec<_>>(),
            );
            let slot = slot.as_deref().unwrap_or("null");
            let count = handles.len();
            return self.checked(&format!("call i64 @imre_dyn_build_sequence(ptr %ctx, ptr {data}, i64 {count}, i1 true, i1 {tuple}, ptr {slot})"));
        }
        let build = |code: &mut Self| {
            code.sequence_display(values, display, slot.as_deref(), spread, evaluate)
        };
        let Some(slot) = slot.as_deref() else {
            return build(self);
        };
        self.cached_tuple(slot, build)
    }

    fn cached_tuple(&mut self, slot: &str, build: impl FnOnce(&mut Self) -> String) -> String {
        // A loaded tuple is a constant of the code unit. Reuse it before evaluating
        // any literal loaders or allocating a temporary container.
        let cached = self.value(&format!(
            "call i64 @imre_dyn_cached_handle(ptr %ctx, ptr {slot})"
        ));
        let hit = self.value(&format!("icmp ne i64 {cached}, 0"));
        let previous = self.current_block().to_owned();
        let load = self.name("b");
        let done = self.name("b");
        self.branch(&hit, &done, &load);
        self.label(&load);
        let result = build(self);
        let loaded = self.current_block().to_owned();
        self.jump(&done);
        self.label(&done);
        self.value(&format!(
            "phi i64 [ {cached}, %{previous} ], [ {result}, %{loaded} ]"
        ))
    }

    /// Evaluate small prefixes before allocating. Once built, the list owns every consumed operand.
    fn sequence_display<T>(
        &mut self,
        items: &[T],
        display: DisplayKind,
        slot: Option<&str>,
        spread: impl Fn(&T) -> Option<&T>,
        mut evaluate: impl FnMut(&mut Self, &T) -> String,
    ) -> String {
        let tuple = display == DisplayKind::Tuple;
        let mark = self.temporary_mark();
        let unwind = |code: &mut RegisteredFrame<'a, 'p>| code.release_owned_operands(&mark);
        self.unwind_with(unwind, |code| {
            let initial = |code: &mut Self, values: Vec<String>, complete| {
                let slot = if complete { slot } else { None };
                let container = if display == DisplayKind::Set {
                    let (data, count) = code.handle_array(&values);
                    code.checked(&format!("call i64 @imre_dyn_build_set(ptr %ctx, ptr {data}, i64 {count}, i64 {mark})"))
                } else {
                    code.build_sequence(&values, tuple && complete, slot)
                };
                if !complete {
                    code.keep_operands(&mark, &container, "0");
                }
                container
            };
            let append = |code: &mut Self, list: &String, _, value: String, extend| {
                if extend && code.native_suspension {
                    let range = code.current_range;
                    code.call_position(range);
                }
                code.take_matching_check();
                code.checked(&format!("call i64 @imre_dyn_display_item(ptr %ctx, i64 {list}, i64 {value}, i64 {mark}, i1 {extend})"));
            };
            let (mut result, complete) = crate::llvm::operations::sequences::build(
                code, items, spread, &mut evaluate, initial, append,
            );
            if !complete && tuple {
                let operation = if slot.is_some() { "constant_tuple" } else { "tuple" };
                result = code.op(operation, &[&result]);
                if let Some(slot) = slot {
                    result = code.checked(&format!("call i64 @imre_dyn_linked_value(ptr %ctx, ptr {slot}, i64 {result})"));
                }
            }
            code.keep_operands(&mark, &result, "0");
            result
        })
    }
}

impl<'a, 'p, T> FrameEmitter<'a, 'p> for T where
    T: std::ops::DerefMut<Target = RegisteredFrame<'a, 'p>>
{
}

impl RegisteredFrame<'_, '_> {
    pub(crate) fn delete_name(&mut self, name: &str, range: TextRange) -> String {
        let name = self.code_name(name);
        let (line, start) = self.positions.line_and_column(range.start());
        let (_, end) = self.positions.line_and_column(range.end());
        self.op_at(
            "delname",
            &[&self.scope.clone(), &name],
            Some([line, start, start, end]),
        )
    }

    pub(crate) fn declare_global(&mut self, name: &str) {
        let name = self.code_name(name);
        self.op("declare_global", &[&self.scope.clone(), &name]);
    }
}

impl RegisteredFrame<'_, '_> {
    pub(crate) fn invoke(&mut self, callable: &str, values: &[&str]) -> String {
        self.invoke_at(callable, values, None)
    }

    pub(crate) fn invoke_at(
        &mut self,
        callable: &str,
        values: &[&str],
        span: Option<[usize; 4]>,
    ) -> String {
        let mark = self.temporary_mark();
        let (ptr, count) = self.handle_array(values);
        self.call_at(
            callable,
            CallArguments::Positional {
                array: ptr,
                count,
                receiver_slot: false,
            },
            &mark,
            span,
        )
    }

    /// The optional message has already been evaluated on the failure edge.
    /// Construct the exception at the keyword and raise it at the test expression.
    pub(crate) fn assertion_failure(
        &mut self,
        statement: TextSize,
        test: TextSize,
        message: Option<&str>,
    ) {
        let statement_line = self.positions.line(statement);
        self.set_statement_line(statement_line);
        let name = self.text("AssertionError");
        let class = self.op("builtin", &[&name]);
        let error = match message {
            Some(value) => self.invoke(&class, &[value]),
            None => class,
        };
        let test_line = self.positions.line(test);
        if test_line != statement_line {
            self.set_statement_line(test_line);
        }
        self.raise(&error, None, false);
    }

    pub(crate) fn raise(&mut self, exc: &str, cause: Option<&str>, reraise: bool) {
        // Raising always transfers control to the enclosing handler.
        let op = self
            .output
            .data(if reraise { b"reraise" } else { b"raise" });
        let explicit = if cause.is_some() { 2 } else { 0 };
        let cause = cause.unwrap_or("0");
        self.line(&format!(
            "call i64 @imre_dyn_op_at(ptr %ctx, ptr {op}, i64 {exc}, i64 {cause}, i64 {explicit}, ptr null)"
        ));
        self.jump(&self.body.error_target.clone());
    }

    pub(crate) fn set_statement_line(&mut self, line: usize) {
        self.position = SourcePosition::statement(line);
        if !self.output.declarations.declared_statement_line {
            self.output
                .functions
                .push_str("declare void @imre_dyn_set_statement_line(ptr, i32)\n");
            self.output.declarations.declared_statement_line = true;
        }
        self.line(&format!(
            "call void @imre_dyn_set_statement_line(ptr %ctx, i32 {line})"
        ));
    }
}

impl RegisteredFrame<'_, '_> {
    pub(crate) fn attribute_position(&mut self, range: ruff_text_size::TextRange) {
        if self.native_suspension {
            self.call_position(range);
        }
    }

    pub(crate) fn attribute_value(
        &mut self,
        receiver: &str,
        name: &str,
        range: ruff_text_size::TextRange,
    ) -> String {
        self.attribute_position(range);
        self.attr(receiver, name)
    }

    pub(crate) fn write_attribute(
        &mut self,
        receiver: &str,
        name: &str,
        range: ruff_text_size::TextRange,
        value: Option<&str>,
    ) -> String {
        self.attribute_position(range);
        match value {
            Some(value) => self.named_op("setattr", receiver, name, value),
            None => self.named_op("delattr", receiver, name, "0"),
        }
    }

    /// Resolve the descriptor before evaluating the call arguments. A direct target
    /// also supplies its receiver through the frame's reusable scratch slot.
    pub(crate) fn method_target(
        &mut self,
        receiver: &str,
        name: &str,
        range: ruff_text_size::TextRange,
        direct: bool,
    ) -> (String, Option<String>) {
        self.attribute_position(range);
        let lookup = if direct && name == "get" {
            Some(format!(
                "call i64 @imre_dyn_dict_get_target(ptr %ctx, i64 {receiver}, ptr %live)"
            ))
        } else if direct && !name.starts_with('_') && imre_ast::strings::STR_METHODS.contains(&name)
        {
            let symbol = self.output.data(format!("str.{name}").as_bytes());
            Some(format!(
                "call i64 @imre_dyn_string_method_target(ptr %ctx, i64 {receiver}, ptr {symbol}, ptr %live)"
            ))
        } else if direct && !name.starts_with("__") {
            let symbol = self.output.data(name.as_bytes());
            Some(format!(
                "call i64 @imre_dyn_builtin_method_target(ptr %ctx, i64 {receiver}, ptr {symbol}, ptr %live)"
            ))
        } else {
            None
        };
        if let Some(lookup) = lookup {
            // Method lookup propagates a pending receiver error before inspecting the handle.
            self.take_matching_check();
            let target = self.checked(&lookup);
            // Nested argument calls can reuse the entry-block scratch space.
            let bound = self.value("load i64, ptr %live");
            (target, Some(bound))
        } else {
            (self.named_op("method", receiver, name, "0"), None)
        }
    }

    pub(super) fn open_operand_unwind(&mut self) -> (String, String) {
        let failed = self.name("b");
        let outer = std::mem::replace(&mut self.body.error_target, failed.clone());
        (failed, outer)
    }

    pub(super) fn close_unwind(
        &mut self,
        (failed, outer): (String, String),
        release: impl FnOnce(&mut Self),
    ) {
        let ready = self.name("b");
        self.jump(&ready);
        self.label(&failed);
        release(self);
        self.jump(&outer);
        self.label(&ready);
        self.body.error_target = outer;
    }

    pub(crate) fn pack_operand(&mut self, operation: &str, args: &[&str]) -> String {
        if self.native_suspension {
            self.call_position(self.current_range);
        }
        self.op(operation, args)
    }

    /// Equal literals share a slot within a source unit. The address remains distinct after linking
    /// independently compiled providers, and the runtime holds its value in the current heap.
    pub(crate) fn constant_slot(&mut self, key: Constant) -> String {
        let output = &mut self.output;
        let next = output.constant_slots.len();
        let index = *output
            .constant_slots
            .entry((self.unit, key))
            .or_insert(next);
        if index == next {
            writeln!(
                output.constants,
                "@const_slot{index} = private constant i8 0"
            )
            .unwrap();
        }
        format!("@const_slot{index}")
    }

    pub(crate) fn build_sequence(
        &mut self,
        values: &[String],
        tuple: bool,
        slot: Option<&str>,
    ) -> String {
        let (array, count) = self.handle_array(values);
        let slot = slot.unwrap_or("null");
        self.checked(&format!("call i64 @imre_dyn_build_sequence(ptr %ctx, ptr {array}, i64 {count}, i1 false, i1 {tuple}, ptr {slot})"))
    }

    pub(crate) fn source_text(&mut self, text: &str, points: Option<Vec<u8>>) -> String {
        let mut chars = text.chars();
        let short = chars
            .next()
            .is_none_or(|ch| ch <= '\u{ff}' && chars.next().is_none());
        if short || interned_spelling(text) {
            return self.text(text);
        }
        let points = points.unwrap_or_else(|| {
            let mut points = Vec::new();
            push_codepoints(&mut points, text, None);
            points
        });
        let data = self.output.data(text.as_bytes());
        self.cached_string(1, data, text.len(), points)
    }

    pub(crate) fn linked_string(&mut self, points: Vec<u8>) -> String {
        let (chunks, remainder) = points.as_chunks::<4>();
        debug_assert!(remainder.is_empty());
        let text = chunks
            .iter()
            .map(|bytes| char::from_u32(u32::from_le_bytes(*bytes)))
            .collect::<Option<String>>();
        if let Some(text) = text {
            return self.source_text(&text, Some(points));
        }
        let data = self.output.data(&points);
        self.cached_string(7, data, points.len(), points)
    }

    pub(crate) fn cached_string(
        &mut self,
        kind: i32,
        data: String,
        len: usize,
        points: Vec<u8>,
    ) -> String {
        let slot = self.constant_slot(Constant::Text(points));
        self.take_matching_check();
        self.checked(&format!(
            "call i64 @imre_dyn_linked_string(ptr %ctx, ptr {slot}, i32 {kind}, ptr {data}, i64 {len})"
        ))
    }
}

pub(crate) fn interned_spelling(value: &str) -> bool {
    value
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'_')
}
