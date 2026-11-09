//! Object-frame ABI operations used by registered LLVM code.

use super::{
    CallArguments, GUARDED_LIT, GUARDED_NAMED_OP, NAMED_OP, Output, permanent, suspension,
};
use crate::llvm::body::FunctionBody;
use ruff_text_size::{TextRange, TextSize};
use std::fmt::Write as _;

/// Source bytes and byte offsets shared by both forms of registered body.
pub(super) struct SourcePositions<'a> {
    pub(super) text: &'a str,
    pub(super) file: &'a str,
    newlines: Vec<usize>,
}

impl<'a> SourcePositions<'a> {
    pub(super) fn new(text: &'a str, file: &'a str) -> Self {
        Self {
            text,
            file,
            newlines: text.match_indices('\n').map(|(offset, _)| offset).collect(),
        }
    }

    pub(super) fn line(&self, offset: TextSize) -> usize {
        self.newlines.partition_point(|&n| n < usize::from(offset)) + 1
    }

    pub(super) fn line_and_column(&self, offset: TextSize) -> (usize, usize) {
        let line = self.line(offset);
        let start = if line == 1 {
            0
        } else {
            self.newlines[line - 2] + 1
        };
        (line, usize::from(offset) - start)
    }
}

/// The registered calling convention supplies object handles, scoped roots and pending errors.
pub(super) struct RegisteredFrame<'a, 'p> {
    pub(super) output: &'a mut Output,
    positions: &'p SourcePositions<'p>,
    pub(super) body: FunctionBody,
    id: u32,
    pending_check: Option<ErrorCheck>,
    pub(super) scope: String,
    // Handles held in native registers across statement boundaries must stay reachable.
    pub(super) live: Vec<String>,
    pub(super) max_live: usize,
    pub(super) caught: Vec<String>,
    pub(super) simple_call: Option<TextRange>,
    pub(super) expression_line: usize,
    pub(super) native_suspension: bool,
}

struct ErrorCheck {
    handler: String,
    failed: String,
    next: String,
}

impl<'a, 'p> RegisteredFrame<'a, 'p> {
    pub(super) fn new(
        output: &'a mut Output,
        positions: &'p SourcePositions<'p>,
        native_suspension: bool,
    ) -> Self {
        let id = output.count;
        output.count += 1;
        Self {
            output,
            positions,
            id,
            body: FunctionBody::new(if native_suspension { "start" } else { "entry" }, "failed"),
            pending_check: None,
            scope: "%scope".into(),
            live: Vec::new(),
            max_live: 0,
            caught: Vec::new(),
            simple_call: None,
            expression_line: 0,
            native_suspension,
        }
    }

    pub(super) fn finish(mut self, module: bool) -> String {
        let name = format!("@code{}", self.id);
        if !self.body.terminated {
            self.return_value("0");
        }
        let mut entry = if self.max_live == 0 {
            String::new()
        } else {
            format!("  %live = alloca [{} x i64]\n", self.max_live)
        };
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
        if self.native_suspension {
            writeln!(self.output.functions,
                "define internal ptr {name}(ptr %context_slot, i64 %scope) presplitcoroutine {{\nentry:\n{entry}{}{}{} }}",
                suspension::PROLOGUE, self.body.code, suspension::EPILOGUE).unwrap();
        } else {
            self.body.start("failed");
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

    pub(super) fn enter_expression(&mut self, position: TextSize) -> Option<usize> {
        let line = self.positions.line(position);
        let previous = self.expression_line;
        if previous == line {
            return None;
        }
        self.expression_line = line;
        self.line(&format!(
            "call void @imre_dyn_expression_line(ptr %ctx, i32 {line})"
        ));
        Some(previous)
    }

    pub(super) fn leave_expression(&mut self, previous: Option<usize>) {
        if let Some(line) = previous {
            self.expression_line = line;
            if !self.body.terminated {
                self.line(&format!(
                    "call void @imre_dyn_expression_line(ptr %ctx, i32 {line})"
                ));
            }
        }
    }

    pub(super) fn name(&mut self, prefix: &str) -> String {
        self.body.name(prefix)
    }

    pub(super) fn line(&mut self, line: &str) {
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

    pub(super) fn value(&mut self, instruction: &str) -> String {
        if !self.native_suspension {
            self.flush_check();
            return self.body.instruction(instruction);
        }
        let value = self.name("%v");
        self.line(&format!("{value} = {instruction}"));
        value
    }

    pub(super) fn end(&mut self, instruction: &str) {
        if !self.body.terminated {
            self.flush_check();
            self.body.end(instruction);
        }
    }

    pub(super) fn return_value(&mut self, value: &str) {
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

    pub(super) fn jump(&mut self, label: &str) {
        if !self.body.terminated {
            self.flush_check();
            self.body.jump(label);
        }
    }

    pub(super) fn label(&mut self, label: &str) {
        self.flush_check();
        self.body.start(label);
    }

    pub(super) fn branch(&mut self, condition: &str, yes: &str, no: &str) {
        if !self.body.terminated {
            self.flush_check();
            self.body.branch(condition, yes, no);
        }
    }

    pub(super) fn checked(&mut self, invocation: &str) -> String {
        let result = self.value(invocation);
        self.check_error();
        result
    }

    pub(super) fn check_error(&mut self) {
        self.pending_check = Some(ErrorCheck {
            handler: self.body.error_target.clone(),
            failed: self.name("%v"),
            next: self.name("b"),
        });
    }

    pub(super) fn flush_check(&mut self) {
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
    pub(super) fn take_matching_check(&mut self) -> bool {
        self.pending_check
            .take_if(|check| check.handler == self.body.error_target)
            .is_some()
    }

    /// Publish the real predecessor before referring to it in a phi.
    pub(super) fn current_block(&mut self) -> &str {
        self.flush_check();
        &self.body.current
    }

    pub(super) fn op(&mut self, name: &str, args: &[&str]) -> String {
        self.op_at(name, args, None)
    }

    pub(super) fn op_at(&mut self, name: &str, args: &[&str], span: Option<[usize; 4]>) -> String {
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

    pub(super) fn lit(&mut self, kind: i32, bytes: &[u8]) -> String {
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
    pub(super) fn code_name(&mut self, name: &str) -> String {
        self.output.interned_identifiers.insert(name.to_owned());
        self.lit(6, name.as_bytes())
    }

    pub(super) fn integer(&mut self, n: impl TryInto<i64> + ToString + Copy) -> String {
        if let Ok(value) = n.try_into()
            && let Some(id) = permanent::small_integer_id(value)
        {
            return id.to_string();
        }
        self.lit(0, n.to_string().as_bytes())
    }

    /// Load a code name and use it as the second operand of a boxed operation.
    pub(super) fn named_op(&mut self, op: &str, receiver: &str, name: &str, value: &str) -> String {
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

    /// Save the caller's earlier operands before an expression can release a writer.
    pub(super) fn temporary_mark(&mut self) -> String {
        self.value("call i64 @imre_dyn_truth_mark(ptr %ctx)")
    }

    pub(super) fn expression_range_span(
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

    /// Store the handles that must survive the next statement or assignment target.
    fn live_handles(&mut self) -> (&'static str, usize) {
        self.handle_array(&self.live.clone())
    }

    /// Copy evaluated handles into reusable storage in this activation's entry block.
    pub(super) fn handle_array(&mut self, values: &[impl AsRef<str>]) -> (&'static str, usize) {
        let count = values.len();
        self.max_live = self.max_live.max(count);
        for (i, value) in values.iter().enumerate() {
            let value = value.as_ref();
            let ptr = self.value(&format!("getelementptr i64, ptr %live, i64 {i}"));
            self.line(&format!("store i64 {value}, ptr {ptr}"));
        }
        (if count == 0 { "null" } else { "%live" }, count)
    }

    pub(super) fn checkpoint(&mut self, range: TextRange) {
        let (ptr, count) = self.live_handles();
        let line = self.positions.line(range.start());
        self.expression_line = line;
        self.line(&format!(
            "call void @imre_dyn_statement_with_exception(ptr %ctx, ptr {ptr}, i64 {count}, ptr {}, i32 {line}, i64 {})",
            self.positions.file, self.caught.last().map_or("0", String::as_str)
        ));
        self.check_error();
    }

    /// A live caller retains the full range even when traceback highlighting is suppressed.
    pub(super) fn call_position(&mut self, range: TextRange) {
        let (start_line, start_col) = self.positions.line_and_column(range.start());
        let (end_line, end_col) = self.positions.line_and_column(range.end());
        let position = self
            .output
            .position([start_line, end_line, start_col, end_col]);
        self.line(&format!(
            "call void @imre_dyn_call_position(ptr %ctx, ptr {position})"
        ));
    }

    /// Pass the caller's temporary root boundary and optional traceback position.
    pub(super) fn call_at(
        &mut self,
        callable: &str,
        operands: CallArguments,
        mark: &str,
        span: Option<[usize; 4]>,
    ) -> String {
        let position = span.map_or_else(|| "null".into(), |span| self.output.position(span));
        let scope = &self.scope;
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

    pub(super) fn assign_owned_name(&mut self, name: &str, value: &str) {
        let replaced = self.op("assign_target", &[&self.scope.clone(), name, value]);
        self.release_replaced(&replaced);
    }

    /// Collect discarded owners while retaining the operands needed after the transfer.
    pub(super) fn release_replaced(&mut self, replaced: &str) {
        let (ptr, count) = self.live_handles();
        self.line(&format!("call void @imre_dyn_assignment_target(ptr %ctx, ptr {ptr}, i64 {count}, i64 {replaced})"));
    }
}
