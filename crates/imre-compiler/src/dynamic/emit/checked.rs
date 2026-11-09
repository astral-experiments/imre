//! Checked operations inside a Python function with a registered frame and object arguments.

use super::{CallArguments, Function, permanent};
use crate::frontend::RegisteredFunction;
use crate::ir::{self, Binary, Builtin, Expr, ExprKind, Statement, Type};
use crate::llvm::operations::Operations;

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

/// The registered binder owns the scope, argument identities and frame.
struct Frame<'f, 'a, 'p> {
    code: &'f mut Function<'a, 'p>,
    checked: &'f RegisteredFunction,
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
        self.code.lookup(&self.checked.local_names[&slot])
    }
    fn prepare_assignment(&mut self, _: usize, _: &Expr) -> Option<()> {
        Some(())
    }
    fn assign(&mut self, slot: usize, value: String, (): ()) {
        let name = self.code.code_name(&self.checked.local_names[&slot]);
        self.code.assign_owned_name(&name, &value);
    }
    fn consume_expression(&mut self, expr: &Expr) -> bool {
        if let ExprKind::Builtin(Builtin::RegisteredStatement(range, simple), _) = &expr.kind {
            self.code.checkpoint(range);
            self.code.simple_call = *simple;
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
        self.code.terminated
    }
    fn binary_expression(&mut self, op: Binary, left: &Expr, right: &Expr, _: &Expr) -> String {
        debug_assert!(matches!(op, Binary::Add));
        let (left, right) = self.binary_operands(left, right);
        self.code.checked_integer_add(&left, &right)
    }
    fn builtin(&mut self, op: &Builtin, operands: &[Expr], _: Type) -> String {
        if let (Builtin::RegisteredLocation(position), [value]) = (op, operands) {
            let checked = self.checked;
            return self
                .code
                .with_expression_line(*position, |code| Frame { code, checked }.evaluate(value));
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
            code: self,
            checked,
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
