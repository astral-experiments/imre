//! Checked operations inside a Python function with a registered frame and object arguments.

use ruff_text_size::{TextRange, TextSize};

use super::{CallArguments, Function, RegisteredFrame, permanent};
use crate::frontend::RegisteredFunction;
use crate::ir::{self, Binary, Builtin, Expr, ExprKind, Statement, Type};
use crate::llvm::operations::Operations;
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
                left.ty == Type::Int && right.ty == Type::Int && expr.ty == Type::Int
            }
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
struct Frame<'f, 'a, 'p> {
    code: &'f mut Function<'a, 'p>,
    names: Option<&'f HashMap<usize, String>>,
}

impl Operations for Frame<'_, '_, '_> {
    type Value = String;
    type Operand = String;
    type Assignment = ();

    fn evaluate(&mut self, expr: &Expr) -> String {
        self.emit_value(expr)
    }
    fn operand(&mut self, expr: &Expr) -> String {
        self.emit_value(expr)
    }
    fn integer(&mut self, text: &str) -> String {
        match text.parse::<i64>() {
            Ok(n) => self.code.integer(n),
            Err(_) => self.code.lit(0, text.as_bytes()),
        }
    }
    fn float(&mut self, value: f64) -> String {
        self.code.lit(3, &value.to_le_bytes())
    }
    fn string(&mut self, value: &str, _: Type) -> String {
        self.code.text(value)
    }
    fn bytes(&mut self, value: &[u8]) -> String {
        self.code.lit(2, value)
    }
    fn boolean(&mut self, value: bool) -> String {
        (if value {
            permanent::TRUE
        } else {
            permanent::FALSE
        })
        .to_string()
    }
    fn none(&mut self) -> String {
        "0".into()
    }
    fn local(&mut self, slot: usize, _: &Expr) -> String {
        self.code
            .lookup(&self.names.expect("registered local names")[&slot])
    }
    fn prepare_assignment(&mut self, _: usize, _: &Expr) -> Option<()> {
        Some(())
    }
    fn assign(&mut self, slot: usize, value: String, (): ()) {
        let name = self
            .code
            .code_name(&self.names.expect("registered local names")[&slot]);
        self.code.assign_owned_name(&name, &value);
    }
    fn consume_expression(&mut self, expr: &Expr) -> bool {
        if let ExprKind::Builtin(Builtin::RegisteredStatement(range, simple), _) = &expr.kind {
            self.code.checkpoint(range);
            self.code.simple_call = *simple;
            true
        } else if matches!(expr.kind, ExprKind::Builtin(Builtin::GlobalSet(_), _)) {
            self.evaluate(expr);
            true
        } else {
            false
        }
    }
    fn discard(&mut self, value: String) {
        self.code.line(&format!(
            "call void @imre_dyn_discard_expression(ptr %ctx, i64 {value})"
        ));
    }
    fn return_result(&mut self, value: String) {
        self.code.return_value(&value);
    }
    fn terminated(&self) -> bool {
        self.code.body.terminated
    }
    fn binary_expression(&mut self, op: Binary, left: &Expr, right: &Expr, expr: &Expr) -> String {
        debug_assert!(matches!(op, Binary::Add));
        let typed = left.ty == Type::Int && right.ty == Type::Int && expr.ty == Type::Int;
        let (left, right) = self.binary_operands(left, right);
        if typed {
            self.code.checked_integer_add(&left, &right)
        } else {
            self.code.op("+", &[&left, &right])
        }
    }
    fn builtin(&mut self, op: &Builtin, operands: &[Expr], _: Type) -> String {
        match (op, operands) {
            (Builtin::RegisteredName(name), []) => self.code.lookup(name),
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
                value
            }
            (Builtin::RegisteredDefinition(site), header) => {
                let header = header
                    .iter()
                    .map(|value| self.evaluate(value))
                    .collect::<Vec<_>>();
                self.code.registered_definition(*site, &header)
            }
            (Builtin::GlobalSet(slot), [value]) => {
                let value = self.evaluate(value);
                self.assign(*slot, value, ());
                self.none()
            }
            (Builtin::RegisteredAttribute(name), [base]) => {
                let base = self.evaluate(base);
                self.code.attr(&base, name)
            }
            (Builtin::RegisteredSubscript(range, base_end), [base, key]) => {
                let names = self.names;
                self.code
                    .subscript(base, key, *range, *base_end, |code, expr| {
                        Frame { code, names }.evaluate(expr)
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
                self.code
                    .dict_display(items, |code, expr| Frame { code, names }.evaluate(expr))
            }
            (Builtin::RegisteredCall(range, callee_end, callee_name), [callee, args @ ..]) => {
                let names = self.names;
                self.code.positional_call(
                    callee,
                    args,
                    *range,
                    *callee_end,
                    *callee_name,
                    |code, expr| Frame { code, names }.evaluate(expr),
                )
            }
            _ => unreachable!("unsupported registered operation"),
        }
    }
    fn other_statement(&mut self, statement: &Statement) {
        let Statement::SetItem(receiver, index, value) = statement else {
            unreachable!("unsupported registered statement");
        };
        let value = self.evaluate(value);
        let receiver = self.evaluate(receiver);
        let index = self.evaluate(index);
        self.code.put_item(&receiver, &index, &value);
    }
    fn other_expression(&mut self, _: &Expr) -> String {
        unreachable!("unsupported registered expression")
    }
}

impl Function<'_, '_> {
    pub(super) fn positional_call<T>(
        &mut self,
        callee: &T,
        args: &[T],
        range: TextRange,
        callee_end: TextSize,
        suspended_name: Option<TextRange>,
        evaluate: impl FnMut(&mut Self, &T) -> String,
    ) -> String {
        let mark = self.temporary_mark();
        if self.native_suspension
            && let Some(name) = suspended_name
        {
            self.suspension_position(name);
        }
        let (callee, args) = crate::llvm::operations::call_arguments(self, callee, args, evaluate);
        let (array, count) = self.handle_array(&args);
        let span = self.expression_range_span(range, callee_end, b'(');
        self.call_position(range);
        self.call_at(
            &callee,
            CallArguments::Positional {
                array,
                count,
                receiver_slot: false,
            },
            &mark,
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
        Frame {
            code: self,
            names: None,
        }
        .evaluate(expression)
    }

    pub(super) fn checked_body(&mut self, checked: &RegisteredFunction) {
        Frame {
            code: self,
            names: Some(&checked.local_names),
        }
        .emit_block(&checked.function.body, |_| {});
    }
}

impl RegisteredFrame<'_, '_> {
    fn checked_integer_add(&mut self, left: &str, right: &str) -> String {
        self.output.declarations.checked_integer_arithmetic = true;
        let a = self.value(&format!(
            "call i64 @imre_dyn_exact_int_word(ptr %ctx, i64 {left})"
        ));
        let b = self.value(&format!(
            "call i64 @imre_dyn_exact_int_word(ptr %ctx, i64 {right})"
        ));
        let a_exact = self.value(&format!("icmp ne i64 {a}, 0"));
        let b_exact = self.value(&format!("icmp ne i64 {b}, 0"));
        let exact = self.value(&format!("and i1 {a_exact}, {b_exact}"));
        let native = self.name("int_add");
        let object = self.name("object_add");
        let done = self.name("sum");
        self.branch(&exact, &native, &object);
        self.label(&native);
        let sum = self.value(&format!("call i64 @int_add(i64 {a}, i64 {b})"));
        let boxed = self.checked(&format!("call i64 @imre_dyn_take_int(ptr %ctx, i64 {sum})"));
        let native_end = self.current_block().to_owned();
        self.jump(&done);
        self.label(&object);
        let dynamic = self.op("+", &[left, right]);
        let object_end = self.current_block().to_owned();
        self.jump(&done);
        self.label(&done);
        self.value(&format!(
            "phi i64 [ {boxed}, %{native_end} ], [ {dynamic}, %{object_end} ]"
        ))
    }
}
