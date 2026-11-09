//! Checked operations inside a Python function with a registered frame and object arguments.

use ruff_text_size::{TextRange, TextSize};

use super::{CallArguments, Function, RegisteredFrame, permanent};
use crate::frontend::RegisteredFunction;
use crate::ir::{self, Binary, Builtin, Expr, ExprKind, Statement, Type};
use crate::llvm::body::FunctionBody;
use crate::llvm::operations::{CheckedBody, CheckedValue};
use std::collections::HashMap;

/// Only take a body when all its operations have a lowering for the registered calling convention.
pub(crate) fn supports(checked: &RegisteredFunction) -> bool {
    let body = &checked.function.body;
    let mut uses_object_operation = false;
    let mut supported = body.iter().all(|stmt| match stmt {
        Statement::Assign(slot, _) => checked.local_names.contains_key(slot),
        Statement::SetItem(receiver, _, _) => {
            uses_object_operation = true;
            receiver.ty == Type::PythonObject
        }
        Statement::Return(_) | Statement::Expr(_) => true,
        _ => false,
    });
    ir::walk_expressions(body, &mut |expr| {
        supported &= match &expr.kind {
            ExprKind::Local(slot) => checked.local_names.contains_key(slot),
            ExprKind::None
            | ExprKind::Bool(_)
            | ExprKind::Int(_)
            | ExprKind::Float(_)
            | ExprKind::Str(_)
            | ExprKind::Bytes(_) => true,
            ExprKind::Binary(Binary::Add, left, right) => {
                expr.ty == Type::PythonObject
                    || left.ty == Type::Int && right.ty == Type::Int && expr.ty == Type::Int
            }
            ExprKind::Conditional(..) => true,
            ExprKind::Builtin(Builtin::RegisteredCall(..), operands) => {
                uses_object_operation = true;
                !operands.is_empty()
            }
            ExprKind::Builtin(Builtin::RegisteredStatement(..), _) => true,
            ExprKind::Builtin(Builtin::RegisteredLocation(_), operands) => operands.len() == 1,
            ExprKind::Builtin(Builtin::RegisteredAttribute(_), operands) => operands.len() == 1,
            ExprKind::Builtin(Builtin::RegisteredSubscript(..), operands) => operands.len() == 2,
            ExprKind::Builtin(
                Builtin::RegisteredName(_)
                | Builtin::RegisteredEllipsis
                | Builtin::RegisteredString(_)
                | Builtin::RegisteredComplex(..),
                operands,
            ) => operands.is_empty(),
            ExprKind::Builtin(Builtin::RegisteredDict(entries), operands) => {
                operands.len()
                    == entries
                        .iter()
                        .map(|entry| match entry {
                            ir::DictEntryKind::Pair => 2,
                            ir::DictEntryKind::Unpack => 1,
                        })
                        .sum::<usize>()
            }
            _ => false,
        };
    });
    supported && uses_object_operation
}

/// The registered binder owns the scope, argument identities and frame.
pub(crate) struct Frame<'f, 'a, 'p> {
    code: &'f mut Function<'a, 'p>,
    names: Option<&'f HashMap<usize, String>>,
}

impl Frame<'_, '_, '_> {
    fn evaluate(&mut self, expr: &Expr) -> CheckedValue {
        CheckedBody::registered(Frame {
            code: self.code,
            names: self.names,
        })
        .emit_value(expr)
    }

    fn evaluate_handle(&mut self, expr: &Expr) -> String {
        let value = self.evaluate(expr);
        self.materialize(&value)
    }

    pub(crate) fn lookup_local(&mut self, slot: usize) -> String {
        self.code
            .lookup(&self.names.expect("registered local names")[&slot])
    }

    pub(crate) fn assign(&mut self, slot: usize, value: CheckedValue) {
        let handle = self.materialize(&value);
        let name = &self.names.expect("registered local names")[&slot];
        self.bind(name, &handle);
    }
    pub(crate) fn body(&mut self) -> &mut FunctionBody {
        self.code.flush_check();
        &mut self.code.body
    }

    pub(crate) fn declare_int_add(&mut self) {
        self.code.declare_integer_arithmetic();
    }

    pub(crate) fn integer(&mut self, text: &str, integer: Option<i64>) -> CheckedValue {
        match integer {
            Some(n) => CheckedValue::rooted_integer(self.code.integer(n), n),
            None => CheckedValue::handle(self.code.lit(0, text.as_bytes())),
        }
    }

    pub(crate) fn string(&mut self, value: &str) -> CheckedValue {
        CheckedValue::handle(self.code.text(value))
    }

    pub(crate) fn float(&mut self, value: f64) -> CheckedValue {
        CheckedValue::handle(self.code.lit(3, &value.to_le_bytes()))
    }

    pub(crate) fn bytes(&mut self, value: &[u8]) -> CheckedValue {
        CheckedValue::handle(self.code.lit(2, value))
    }

    pub(crate) fn boolean(&mut self, value: bool) -> CheckedValue {
        CheckedValue::handle(
            (if value {
                permanent::TRUE
            } else {
                permanent::FALSE
            })
            .to_string(),
        )
    }

    pub(crate) fn consume_expression(&mut self, expr: &Expr) -> bool {
        if let ExprKind::Builtin(Builtin::RegisteredStatement(range, simple), _) = &expr.kind {
            self.code.checkpoint(range);
            self.code.simple_call = *simple;
            true
        } else if matches!(
            expr.kind,
            ExprKind::Builtin(Builtin::GlobalSet(_) | Builtin::RegisteredImport(_), _)
        ) {
            self.evaluate(expr);
            true
        } else {
            false
        }
    }

    pub(crate) fn discard_expression(&mut self, value: &CheckedValue) {
        // Only the Python edge can produce an unawaited coroutine.
        let handle = match value {
            CheckedValue::Handle { id, .. } => Some(id),
            CheckedValue::IntOrHandle { fallback, .. } => Some(fallback),
            CheckedValue::Native(..) => None,
        };
        if let Some(handle) = handle {
            self.code.line(&format!(
                "call void @imre_dyn_discard_expression(ptr %ctx, i64 {handle})"
            ));
        }
    }

    pub(crate) fn return_result(&mut self, value: CheckedValue) {
        let value = self.materialize(&value);
        self.code.return_value(&value);
    }

    pub(crate) fn terminated(&self) -> bool {
        self.code.body.terminated
    }

    pub(crate) fn release_word(&mut self, owner: usize) {
        let word = self.code.value(&format!("load i64, ptr %int_owner{owner}"));
        self.code
            .line(&format!("store i64 1, ptr %int_owner{owner}"));
        self.code
            .line(&format!("call void @imre_int_release(i64 {word})"));
    }

    pub(crate) fn select(&mut self, test: &Expr, yes: &Expr, no: &Expr) -> CheckedValue {
        let names = self.names;
        self.code.select(
            |code, yes, no| {
                let condition = code.test_with_temporaries(|code| {
                    let handle = Frame { code, names }.evaluate_handle(test);
                    code.truth(&handle)
                });
                code.branch(&condition, yes, no);
            },
            |code| Frame { code, names }.evaluate(yes),
            |code| Frame { code, names }.evaluate(no),
        )
    }

    pub(crate) fn builtin(&mut self, op: &Builtin, operands: &[Expr]) -> CheckedValue {
        let handle = match (op, operands) {
            (Builtin::RegisteredName(name), []) => self.code.lookup(name),
            (Builtin::RegisteredSuspend(kind, range), [value]) => {
                let names = self.names;
                self.code.suspend_value(*kind, *range, |code| {
                    Frame { code, names }.evaluate_handle(value)
                })
            }
            (Builtin::RegisteredEllipsis, []) => permanent::ELLIPSIS.to_string(),
            (Builtin::RegisteredComplex(real, imag), []) => self
                .code
                .lit(4, &[real.to_le_bytes(), imag.to_le_bytes()].concat()),
            (Builtin::RegisteredString(points), []) if self.code.source.has_templates => {
                self.code.linked_string(points.clone())
            }
            (Builtin::RegisteredString(points), []) => self.code.lit(7, points),
            (Builtin::RegisteredLocation(range), [value]) => {
                let previous_range = std::mem::replace(&mut self.code.current_range, *range);
                let previous_line = self.code.enter_expression(range.start());
                let value = self.evaluate(value);
                self.code.leave_expression(previous_line);
                self.code.current_range = previous_range;
                return value;
            }
            (Builtin::RegisteredDefinition(site), header) => {
                let header = header
                    .iter()
                    .map(|value| self.evaluate_handle(value))
                    .collect::<Vec<_>>();
                self.code.registered_definition(*site, &header)
            }
            (Builtin::RegisteredImport(import), []) => {
                self.code.namespace_import(import);
                "0".to_owned()
            }
            (Builtin::GlobalSet(slot), [value]) => {
                let value = self.evaluate_handle(value);
                let name = self.names.expect("registered local names")[slot].clone();
                self.bind(&name, &value);
                return CheckedValue::handle("0".into());
            }
            (Builtin::RegisteredAttribute(name), [base]) => {
                let base = self.evaluate_handle(base);
                self.code.attr(&base, name)
            }
            (Builtin::RegisteredSubscript(range, base_end), [base, key]) => {
                let names = self.names;
                self.code
                    .subscript(base, key, *range, *base_end, |code, expr| {
                        Frame { code, names }.evaluate_handle(expr)
                    })
            }
            (Builtin::RegisteredDict(entries), values) => {
                let mut values = values.iter();
                let items = entries.iter().map(|entry| {
                    let key = (*entry == ir::DictEntryKind::Pair)
                        .then(|| values.next().expect("checked dictionary key"));
                    (key, values.next().expect("checked dictionary value"))
                });
                let names = self.names;
                self.code.dict_display(items, |code, expr| {
                    Frame { code, names }.evaluate_handle(expr)
                })
            }
            _ => unreachable!("unsupported registered operation"),
        };
        CheckedValue::handle(handle)
    }

    pub(crate) fn bind(&mut self, name: &str, handle: &str) {
        let name = self.code.code_name(name);
        self.code.assign_owned_name(&name, handle);
    }

    pub(crate) fn call_mark(&mut self, callee_name: Option<ruff_text_size::TextRange>) -> String {
        self.code.begin_positional_call(callee_name)
    }

    pub(crate) fn call(
        &mut self,
        callable: &str,
        args: &[String],
        mark: &str,
        range: ruff_text_size::TextRange,
        callee_end: ruff_text_size::TextSize,
    ) -> String {
        self.code
            .finish_positional_call(callable, args, mark, range, callee_end)
    }

    pub(crate) fn exact_word(&mut self, handle: &str) -> String {
        self.code.value(&format!(
            "call i64 @imre_dyn_exact_int_word(ptr %ctx, i64 {handle})"
        ))
    }

    pub(crate) fn own_word(&mut self, word: &str) -> usize {
        let slot = self.code.integer_owners;
        self.code.integer_owners += 1;
        self.code
            .line(&format!("store i64 {word}, ptr %int_owner{slot}"));
        slot
    }

    pub(crate) fn materialize(&mut self, value: &CheckedValue) -> String {
        match value {
            CheckedValue::Handle { id, .. } => id.clone(),
            CheckedValue::IntOrHandle {
                word,
                fallback,
                owner,
            } => {
                let frame = &mut self.code;
                let native = frame.name("box_int");
                let done = frame.name("handle");
                let exact = frame.value(&format!("icmp ne i64 {word}, 0"));
                let mut join = crate::llvm::operations::ValueJoin::default();
                frame.flush_check();
                join.incoming(&mut frame.body, CheckedValue::handle(fallback.clone()));
                frame.branch(&exact, &native, &done);
                frame.label(&native);
                let boxed = frame.move_integer(*owner, word);
                frame.flush_check();
                join.jump(&mut frame.body, &done, CheckedValue::handle(boxed));
                frame.label(&done);
                let Some(CheckedValue::Handle { id, .. }) = join.finish(&mut frame.body) else {
                    unreachable!("both conversion edges hold Python handles")
                };
                id
            }
            CheckedValue::Native(value, owner) => {
                let (word, small) = value.int_word(owner.is_some());
                if let Some(slot) = owner {
                    return self.code.move_integer(*slot, word);
                }
                self.code.declare_integer_arithmetic();
                if !small {
                    self.code
                        .line(&format!("call void @imre_int_retain(i64 {word})"));
                }
                self.code.checked(&format!(
                    "call i64 @imre_dyn_take_int(ptr %ctx, i64 {word})"
                ))
            }
        }
    }

    pub(crate) fn protocol_add(&mut self, left: &str, right: &str) -> String {
        self.code.op("+", &[left, right])
    }

    pub(crate) fn assign_item(&mut self, receiver: &Expr, index: &Expr, value: &Expr) {
        let (receiver, index, value) = crate::llvm::operations::assignment_operands(
            self,
            receiver,
            index,
            value,
            Self::evaluate_handle,
        );
        self.code.put_item(&receiver, &index, &value);
    }
}

impl Function<'_, '_> {
    fn namespace_import(&mut self, import: &ir::NamespaceImport) {
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
                        self.frame
                            .op("import_star", &[&self.scope.clone(), &module]);
                    } else {
                        let value = self.op("from", &[&module, &member]);
                        self.bind(binding, &value);
                    }
                }
            }
        }
    }

    fn import_name(&mut self, name: &str, fromlist: &str, level: u32) -> String {
        let name = self.code_name(name);
        self.frame.checked(&format!(
            "call i64 @imre_dyn_import_name(ptr %ctx, i64 {}, i64 {name}, i64 {fromlist}, i32 {level})",
            self.scope
        ))
    }

    pub(super) fn positional_call<T>(
        &mut self,
        callee: &T,
        args: &[T],
        range: TextRange,
        callee_end: TextSize,
        suspended_name: Option<TextRange>,
        evaluate: impl FnMut(&mut Self, &T) -> String,
    ) -> String {
        let mark = self.begin_positional_call(suspended_name);
        let (callee, args) = crate::llvm::operations::call_arguments(self, callee, args, evaluate);
        self.finish_positional_call(&callee, &args, &mark, range, callee_end)
    }

    fn begin_positional_call(&mut self, suspended_name: Option<TextRange>) -> String {
        let mark = self.temporary_mark();
        if self.native_suspension
            && let Some(name) = suspended_name
        {
            self.suspension_position(name);
        }
        mark
    }

    fn finish_positional_call(
        &mut self,
        callable: &str,
        args: &[String],
        mark: &str,
        range: TextRange,
        callee_end: TextSize,
    ) -> String {
        let (array, count) = self.handle_array(args);
        let span = self.expression_range_span(range, callee_end, b'(');
        self.call_position(range);
        self.call_at(
            callable,
            CallArguments::Positional {
                array,
                count,
                receiver_slot: false,
            },
            mark,
            span,
        )
    }

    pub(super) fn subscript<T>(
        &mut self,
        base: &T,
        key: &T,
        range: TextRange,
        base_end: TextSize,
        mut evaluate: impl FnMut(&mut Self, &T) -> String,
    ) -> String {
        let base = evaluate(self, base);
        let key = evaluate(self, key);
        let span = self.expression_range_span(range, base_end, b'[');
        self.op_at("get", &[&base, &key], span)
    }

    /// Construct a Python dictionary with CPython's BUILD_MAP, MAP_ADD and DICT_UPDATE order.
    pub(super) fn dict_display<'e, T: 'e>(
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

    /// Emit a registered expression whose names are resolved through this live frame.
    pub(super) fn registered_value(&mut self, expression: &Expr) -> String {
        let first = self.integer_owners;
        let outer = self.expression_integer_start.replace(first);
        let result = Frame {
            code: self,
            names: None,
        }
        .evaluate_handle(expression);
        self.expression_integer_start = outer;
        result
    }

    pub(super) fn checked_body(&mut self, checked: &RegisteredFunction) {
        CheckedBody::registered(Frame {
            code: self,
            names: Some(&checked.local_names),
        })
        .emit_function(&checked.function);
    }
}

impl RegisteredFrame<'_, '_> {
    /// The heap takes the reference even if adding its temporary root fails.
    fn move_integer(&mut self, slot: usize, word: &str) -> String {
        self.line(&format!("store i64 1, ptr %int_owner{slot}"));
        self.checked(&format!(
            "call i64 @imre_dyn_take_int(ptr %ctx, i64 {word})"
        ))
    }

    fn declare_integer_arithmetic(&mut self) {
        self.output.declarations.checked_integer_arithmetic = true;
    }
}
