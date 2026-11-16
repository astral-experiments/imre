//! Exhaust the producer's ten possible values to prove a map is pure, total,
//! and representable by a byte lookup. Unknown IR always keeps the original loop.

use crate::ir::{Binary, Expr, ExprKind, Statement, Type};
use ruff_python_ast::UnaryOp;

pub(super) fn table(
    term: &Expr,
    item: usize,
    protected: &[usize],
    local_count: usize,
) -> Option<[u8; 10]> {
    let mut evaluator = Evaluator {
        locals: vec![None; local_count],
        protected,
    };
    let mut table = [0; 10];
    for (digit, entry) in table.iter_mut().enumerate() {
        evaluator.locals.fill(None);
        evaluator.locals[item] = Some(digit as i64);
        *entry = evaluator.value(term)?.try_into().ok()?;
    }
    Some(table)
}

struct Evaluator<'a> {
    locals: Vec<Option<i64>>,
    protected: &'a [usize],
}

impl Evaluator<'_> {
    fn write(&mut self, slot: usize, value: Option<i64>) -> Option<()> {
        if self.protected.contains(&slot) {
            return None;
        }
        self.locals[slot] = value;
        Some(())
    }

    fn statements(&mut self, body: &[Statement]) -> Option<()> {
        for stmt in body {
            match stmt {
                Statement::Assign(slot, expr) => {
                    let v = self.value(expr)?;
                    self.write(*slot, Some(v))?;
                }
                Statement::Clear(slot) => self.write(*slot, None)?,
                Statement::Move(to, from) => {
                    let v = self.locals[*from]?;
                    self.write(*from, None)?;
                    self.write(*to, Some(v))?;
                }
                Statement::Scope { body, clear } => {
                    self.statements(body)?;
                    self.clear(clear)?;
                }
                _ => return None,
            }
        }
        Some(())
    }

    fn clear(&mut self, clear: &[usize]) -> Option<()> {
        for &slot in clear {
            self.write(slot, None)?;
        }
        Some(())
    }

    /// Only exact small-integer operations may disappear. A zero divisor,
    /// heap integer, uninitialized temporary, or call disqualifies the map.
    fn value(&mut self, expr: &Expr) -> Option<i64> {
        if expr.ty != Type::Int {
            return None;
        }
        let n = match &expr.kind {
            ExprKind::Local(slot) => self.locals[*slot]?,
            ExprKind::Int(n) => n.parse().ok()?,
            ExprKind::Unary(UnaryOp::USub, a) => self.value(a)?.checked_neg()?,
            ExprKind::Binary(op, left, right) => {
                let a = self.value(left)?;
                let b = self.value(right)?;
                match op {
                    Binary::Add => a.checked_add(b)?,
                    Binary::Subtract => a.checked_sub(b)?,
                    Binary::Multiply => a.checked_mul(b)?,
                    Binary::FloorDivide if b > 0 => a.div_euclid(b),
                    Binary::Modulo if b > 0 => a.rem_euclid(b),
                    _ => return None,
                }
            }
            ExprKind::Scope { body, value, clear } => {
                self.statements(body)?;
                let v = self.value(value)?;
                self.clear(clear)?;
                v
            }
            _ => return None,
        };
        (-(1_i64 << 62)..1_i64 << 62).contains(&n).then_some(n)
    }
}
