//! Checked operations inside a Python function with a registered frame and object arguments.

use super::{CallArguments, Function, RegisteredFrame, permanent};
use crate::frontend::RegisteredFunction;
use crate::ir::{self, Binary, Builtin, Expr, ExprKind, Statement, Type};
use crate::llvm::operations::Operations;
use std::collections::HashMap;
use std::ops::{Deref, DerefMut};

/// Only take a body when all its operations have a lowering for the registered calling convention.
pub(crate) fn supports(checked: &RegisteredFunction) -> bool {
    let body = &checked.function.body;
    let mut supported = body.iter().all(|stmt| match stmt {
        Statement::Assign(slot, _) => checked.local_names.contains_key(slot),
        Statement::Return(_) | Statement::Expr(_) => true,
        _ => false,
    });
    let mut calls_object = false;
    ir::walk_expressions(body, &mut |expr| {
        supported &= match &expr.kind {
            ExprKind::Local(slot) => checked.local_names.contains_key(slot),
            ExprKind::None | ExprKind::Bool(_) | ExprKind::Int(_) => true,
            ExprKind::Binary(Binary::Add, left, right) => {
                left.ty == Type::Int && right.ty == Type::Int && expr.ty == Type::Int
            }
            ExprKind::Builtin(Builtin::RegisteredCall(..), operands) => {
                calls_object = true;
                !operands.is_empty()
            }
            ExprKind::Builtin(Builtin::RegisteredStatement(..), _) => true,
            ExprKind::Builtin(Builtin::RegisteredLocation(_), operands) => operands.len() == 1,
            _ => false,
        };
    });
    supported && calls_object
}

/// A checked namespace retains the declaration and lexical metadata for its physical source.
enum Context<'f, 'a, 'p> {
    Checked(&'f mut RegisteredFrame<'a, 'p>),
    Scoped(&'f mut Function<'a, 'p>),
}

impl<'a, 'p> Deref for Context<'_, 'a, 'p> {
    type Target = RegisteredFrame<'a, 'p>;

    fn deref(&self) -> &Self::Target {
        match self {
            Self::Checked(frame) => frame,
            Self::Scoped(function) => &function.frame,
        }
    }
}

impl DerefMut for Context<'_, '_, '_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        match self {
            Self::Checked(frame) => frame,
            Self::Scoped(function) => &mut function.frame,
        }
    }
}

impl<'a, 'p> Context<'_, 'a, 'p> {
    fn scoped(&mut self) -> &mut Function<'a, 'p> {
        match self {
            Self::Scoped(function) => function,
            Self::Checked(_) => unreachable!("namespace operation in a typed registered function"),
        }
    }

    fn lookup_name(&mut self, name: &str) -> String {
        match self {
            Self::Scoped(function) => function.lookup(name),
            Self::Checked(frame) => frame.named_op("lookup", &frame.scope.clone(), name, "0"),
        }
    }
}

/// Both entry forms share the registered object ABI, instruction writer and checked IR traversal.
struct Frame<'f, 'a, 'p> {
    code: Context<'f, 'a, 'p>,
    names: &'f HashMap<usize, String>,
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
    fn string(&mut self, value: &str, _: Type) -> String {
        self.code.scoped().text(value)
    }

    fn new_dict(&mut self, _: Type, _: Type, _: Type) -> String {
        self.code.op("dict", &[])
    }

    fn insert_entry(&mut self, dict: &String, key: String, value: String) {
        self.code.op("put", &[dict, &key, &value]);
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
        self.code.lookup_name(&self.names[&slot])
    }
    fn prepare_assignment(&mut self, _: usize, _: &Expr) -> Option<()> {
        Some(())
    }
    fn assign(&mut self, slot: usize, value: String, (): ()) {
        let name = self.code.code_name(&self.names[&slot]);
        self.code.assign_owned_name(&name, &value);
    }
    fn consume_expression(&mut self, expr: &Expr) -> bool {
        if let ExprKind::Builtin(Builtin::RegisteredStatement(range, simple), _) = &expr.kind {
            match &mut self.code {
                Context::Scoped(function) => function.checkpoint(range),
                Context::Checked(frame) => frame.checkpoint(*range),
            }
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
    fn binary_expression(&mut self, op: Binary, left: &Expr, right: &Expr, _: &Expr) -> String {
        debug_assert!(matches!(op, Binary::Add));
        let (left, right) = self.binary_operands(left, right);
        self.code.checked_integer_add(&left, &right)
    }
    fn builtin(&mut self, op: &Builtin, operands: &[Expr], _: Type) -> String {
        if let (Builtin::RegisteredLocation(position), [value]) = (op, operands) {
            let previous_range = match &mut self.code {
                Context::Scoped(f) => Some(std::mem::replace(&mut f.current_range, *position)),
                Context::Checked(_) => None,
            };
            let previous_line = self.code.enter_expression(position.start());
            let result = self.evaluate(value);
            self.code.leave_expression(previous_line);
            if let Some(previous) = previous_range {
                self.code.scoped().current_range = previous;
            }
            return result;
        }
        match op {
            Builtin::DictGet => {
                let [object, index] = operands else {
                    unreachable!("checked subscription")
                };
                let ExprKind::Builtin(Builtin::RegisteredLocation(base), _) = &object.kind else {
                    unreachable!("registered subscription has a physical receiver")
                };
                let range = self.code.scoped().current_range;
                let span = self.code.expression_range_span(range, base.end(), b'[');
                let (object, index) = self.binary_operands(object, index);
                return self.code.op_at("get", &[&object, &index], span);
            }
            Builtin::RegisteredDefinition(site) => {
                let header = operands
                    .iter()
                    .map(|expr| self.evaluate(expr))
                    .collect::<Vec<_>>();
                return self.code.scoped().registered_definition(*site, &header);
            }
            Builtin::GlobalGet(slot) => return self.code.lookup_name(&self.names[slot]),
            Builtin::GlobalSet(slot) => {
                let value = self.evaluate(&operands[0]);
                self.assign(*slot, value, ());
                return self.none();
            }
            _ => {}
        }
        let Builtin::RegisteredCall(range, callee_end) = op else {
            unreachable!("unsupported registered operation")
        };
        let mark = self.code.temporary_mark();
        let (callee, args) = operands.split_first().expect("checked call has a callee");
        let (callable, values) = self.call_arguments(callee, args);
        let (array, count) = self.code.handle_array(&values);
        let span = self.code.expression_range_span(*range, *callee_end, b'(');
        self.code.call_position(*range);
        self.code.call_at(
            &callable,
            CallArguments::Positional {
                array,
                count,
                receiver_slot: false,
            },
            &mark,
            span,
        )
    }
    fn other_statement(&mut self, _: &Statement) {
        unreachable!("unsupported registered statement")
    }
    fn other_expression(&mut self, _: &Expr) -> String {
        unreachable!("unsupported registered expression")
    }
}

impl Function<'_, '_> {
    pub(super) fn checked_body(&mut self, checked: &RegisteredFunction) {
        Frame {
            code: Context::Scoped(self),
            names: &checked.local_names,
        }
        .emit_block(&checked.function.body, |_| {});
    }

    pub(super) fn checked_operand(
        &mut self,
        value: &Expr,
        names: &HashMap<usize, String>,
    ) -> String {
        Frame {
            code: Context::Scoped(self),
            names,
        }
        .evaluate(value)
    }
}

impl RegisteredFrame<'_, '_> {
    pub(super) fn checked_body(&mut self, checked: &RegisteredFunction) {
        Frame {
            code: Context::Checked(self),
            names: &checked.local_names,
        }
        .emit_block(&checked.function.body, |_| {});
    }

    fn checked_integer_add(&mut self, left: &str, right: &str) -> String {
        if !self.output.declarations.checked_integer_arithmetic {
            self.output.declarations.checked_integer_arithmetic = true;
            self.output.functions.push_str(
                r#"
declare i64 @imre_dyn_exact_int_word(ptr, i64)
declare i64 @imre_dyn_take_int(ptr, i64)
declare i64 @imre_int_binary(i64, i64, i32)
declare { i64, i1 } @llvm.sadd.with.overflow.i64(i64, i64)
!0 = !{!"branch_weights", i32 1, i32 2000}
!1 = !{!"branch_weights", i32 2000, i32 1}
"#,
            );
            self.output
                .functions
                .push_str(&crate::llvm::integer_arithmetic_helper(Binary::Add));
        }
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
