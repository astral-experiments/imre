//! Summarize integer additions over successive ordinals in a counted loop.
use super::{Emitter, Value};
use crate::ir::{Binary, Expr, ExprKind, Statement, Type};

/// A nonnegative contribution of the form `scale * ordinal + offset`.
#[derive(Clone, Copy, Default)]
struct Affine {
    scale: i32,
    offset: i32,
}

impl Affine {
    /// Bound every coefficient, including those of intermediate expressions.
    fn add(self, rhs: Self) -> Option<Self> {
        Some(Self {
            scale: self.scale.checked_add(rhs.scale)?,
            offset: self.offset.checked_add(rhs.offset)?,
        })
    }

    /// Only multiplication by a nonnegative constant preserves the affine form.
    fn times(self, factor: i32) -> Option<Self> {
        Some(Self {
            scale: self.scale.checked_mul(factor)?,
            offset: self.offset.checked_mul(factor)?,
        })
    }
}

/// Resolve a local only after its value has been defined in this iteration.
fn expression(expr: &Expr, target: usize, values: &[(usize, Affine)]) -> Option<Affine> {
    if expr.ty != Type::Int {
        return None;
    }
    match &expr.kind {
        ExprKind::Local(slot) if *slot == target => Some(Affine {
            scale: 1,
            offset: 0,
        }),
        ExprKind::Local(slot) => values.iter().rev().find(|(id, _)| id == slot).map(|p| p.1),
        ExprKind::Int(text) => Some(Affine {
            scale: 0,
            offset: text.parse::<i32>().ok().filter(|&n| n >= 0)?,
        }),
        ExprKind::Binary(op, a, b) => {
            let a = expression(a, target, values)?;
            let b = expression(b, target, values)?;
            match op {
                Binary::Add => a.add(b),
                Binary::Multiply if a.scale == 0 => b.times(a.offset),
                Binary::Multiply if b.scale == 0 => a.times(b.offset),
                _ => None,
            }
        }
        _ => None,
    }
}

/// Prove every write is either a fresh scalar temporary or an addition to one subtotal.
fn reduction(body: &[Statement], target: usize, locals: &[Type]) -> Option<(usize, Affine)> {
    if body.len() > 32 {
        return None;
    }
    let mut values = Vec::new();
    let mut accumulator = None;
    let mut increment = Affine::default();
    for statement in body {
        let Statement::Assign(slot, rhs) = statement else {
            return None;
        };
        if *slot == target || locals[*slot] != Type::Int || rhs.ty != Type::Int {
            return None;
        }
        if let ExprKind::Binary(Binary::Add, previous, value) = &rhs.kind
            && matches!(previous.kind, ExprKind::Local(id) if id == *slot)
        {
            if accumulator.is_some_and(|id| id != *slot) || values.iter().any(|(id, _)| id == slot)
            {
                return None;
            }
            accumulator = Some(*slot);
            increment = increment.add(expression(value, target, &values)?)?;
        } else {
            if accumulator == Some(*slot) {
                return None;
            }
            values.push((*slot, expression(rhs, target, &values)?));
        }
    }
    Some((accumulator?, increment))
}

impl Emitter<'_, '_> {
    /// Skip a bounded prefix, then execute its last iteration normally to publish every live local.
    pub(super) fn advance_affine(
        &mut self,
        body: &[Statement],
        target: usize,
        cursor: usize,
        stop: usize,
    ) {
        if !self.module.source_frames.is_empty() {
            return;
        }
        let Some((sum, increment)) = reduction(body, target, &self.locals) else {
            return;
        };
        let at = self.borrow_slot(cursor);
        let end = self.borrow_slot(stop);
        let total = self.borrow_slot(sum);
        let prefix = self.instruction(format!(
            "call {{i64,i64}} @imre_affine_range_prefix(i64 {}, i64 {}, i64 {}, i32 {}, i32 {})",
            at.operand, end.operand, total.operand, increment.scale, increment.offset
        ));
        let next = self.instruction(format!("extractvalue {{i64,i64}} {prefix}, 0"));
        let skip = self.instruction(format!("icmp ne i64 {next}, 0"));
        let fast = self.label();
        let resume = self.label();
        self.branch(&skip, &fast, &resume);
        self.start(&fast);
        let partial = self.instruction(format!("extractvalue {{i64,i64}} {prefix}, 1"));
        for (slot, operand) in [(cursor, next), (sum, partial)] {
            self.store_with_previous_small(
                slot,
                &Value {
                    ty: Type::Int,
                    operand,
                    small_integer: true,
                },
                true,
            );
        }
        self.jump(&resume);
        self.start(&resume);
    }
}
