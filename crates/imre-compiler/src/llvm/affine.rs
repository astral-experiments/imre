//! Summarize integer additions over successive ordinals in a counted loop.
use super::{Emitter, Value};
use crate::ir::{Binary, Builtin, Compare, Expr, ExprKind, Function, Statement, Type};

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

/// Lengths of immutable literals initialized before the first control-flow statement.
pub(super) fn literal_lengths(function: &Function, writes: &[u8]) -> Vec<(usize, i32)> {
    let mut lengths = Vec::new();
    for statement in &function.body {
        let Statement::Assign(slot, value) = statement else {
            break;
        };
        if writes[*slot] != 1 || function.params.contains(slot) {
            continue;
        }
        let length = match &value.kind {
            ExprKind::Str(text) => text.chars().count(),
            ExprKind::Bytes(bytes) => bytes.len(),
            _ => continue,
        };
        if let Ok(length) = i32::try_from(length) {
            lengths.push((*slot, length));
        }
    }
    lengths
}

struct Reduction<'a> {
    target: usize,
    lengths: &'a [(usize, i32)],
    values: Vec<(usize, Affine)>,
    flags: Vec<(usize, Cycle)>,
    sum: Option<usize>,
    increment: Affine,
    cycle: Cycle,
}

/// The number of occurrences or the accumulated bonus over one complete period.
#[derive(Clone, Copy)]
struct Cycle {
    period: i64,
    count: i64,
}

impl Reduction<'_> {
    /// Resolve a scalar only after it was initialized in this iteration.
    fn expression(&self, expr: &Expr) -> Option<Affine> {
        if expr.ty != Type::Int {
            return None;
        }
        match &expr.kind {
            ExprKind::Local(slot) if *slot == self.target => Some(Affine {
                scale: 1,
                offset: 0,
            }),
            ExprKind::Local(slot) => self
                .values
                .iter()
                .rev()
                .find(|(id, _)| id == slot)
                .map(|p| p.1),
            ExprKind::Int(text) => Some(Affine {
                scale: 0,
                offset: text.parse::<i32>().ok().filter(|&n| n >= 0)?,
            }),
            ExprKind::Builtin(Builtin::Len, args) => {
                let [arg] = args.as_slice() else { return None };
                let ExprKind::Local(slot) = arg.kind else {
                    return None;
                };
                let &(_, offset) = self.lengths.iter().find(|(id, _)| *id == slot)?;
                Some(Affine { scale: 0, offset })
            }
            ExprKind::Binary(op, a, b) => {
                let a = self.expression(a)?;
                let b = self.expression(b)?;
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

    /// Count equal or unequal residues in a full cycle, independently of its starting ordinal.
    fn predicate(&self, expr: &Expr) -> Option<Cycle> {
        if let ExprKind::Local(slot) = expr.kind {
            return self
                .flags
                .iter()
                .rev()
                .find(|(id, _)| *id == slot)
                .map(|p| p.1);
        }
        let ExprKind::Compare(values, ops) = &expr.kind else {
            return None;
        };
        let ([lhs, rhs], [op]) = (values.as_slice(), ops.as_slice()) else {
            return None;
        };
        let ExprKind::Binary(Binary::Modulo, index, divisor) = &lhs.kind else {
            return None;
        };
        if !matches!(index.kind, ExprKind::Local(i) if i == self.target) {
            return None;
        }
        let (ExprKind::Int(p), ExprKind::Int(r)) = (&divisor.kind, &rhs.kind) else {
            return None;
        };
        let period = p.parse::<i64>().ok().filter(|p| (1..=1023).contains(p))?;
        let residue = r.parse::<i64>().ok()?;
        let hits = i64::from((0..period).contains(&residue));
        let count = match op {
            Compare::Equal => hits,
            Compare::NotEqual => period - hits,
            _ => return None,
        };
        Some(Cycle { period, count })
    }

    /// All additions must read and update the same accumulator.
    fn addend(&mut self, slot: usize, rhs: &Expr) -> Option<Affine> {
        let ExprKind::Binary(Binary::Add, previous, value) = &rhs.kind else {
            return None;
        };
        if !matches!(previous.kind, ExprKind::Local(id) if id == slot)
            || self.sum.is_some_and(|id| id != slot)
            || self.values.iter().any(|(id, _)| *id == slot)
            || slot == self.target
            || rhs.ty != Type::Int
        {
            return None;
        }
        self.sum = Some(slot);
        self.expression(value)
    }

    /// Conditional contributions must be nonnegative constants.
    fn bonus(&mut self, arm: &[Statement]) -> Option<i64> {
        let mut bonus = 0i64;
        for statement in arm {
            let Statement::Assign(slot, rhs) = statement else {
                return None;
            };
            let value = self.addend(*slot, rhs)?;
            if value.scale != 0 {
                return None;
            }
            bonus = bonus.checked_add(i64::from(value.offset))?;
        }
        Some(bonus)
    }

    /// Accept only scalar temporaries, accumulator additions and bounded periodic tests.
    fn statement(&mut self, stmt: &Statement, locals: &[Type]) -> Option<()> {
        match stmt {
            Statement::If(test, yes, no) => {
                let cycle = self.predicate(test)?;
                if self.cycle.period != 1 && self.cycle.period != cycle.period {
                    return None;
                }
                let yes = self.bonus(yes)?;
                let no = self.bonus(no)?;
                self.cycle.count = self
                    .cycle
                    .count
                    .checked_mul(cycle.period / self.cycle.period)?
                    .checked_add(yes.checked_mul(cycle.count)?)?
                    .checked_add(no.checked_mul(cycle.period - cycle.count)?)?;
                self.cycle.period = cycle.period;
            }
            Statement::Assign(slot, rhs) if *slot != self.target && locals[*slot] == rhs.ty => {
                if rhs.ty == Type::Bool {
                    let flag = self.predicate(rhs)?;
                    self.flags.push((*slot, flag));
                } else if matches!(&rhs.kind, ExprKind::Binary(Binary::Add,a,_) if matches!(a.kind, ExprKind::Local(id) if id==*slot))
                {
                    let value = self.addend(*slot, rhs)?;
                    self.increment = self.increment.add(value)?;
                } else {
                    if self.sum == Some(*slot) {
                        return None;
                    }
                    let value = self.expression(rhs)?;
                    self.values.push((*slot, value));
                }
            }
            _ => return None,
        }
        Some(())
    }
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
        if body.len() > 32 {
            return;
        }
        let mut plan = Reduction {
            target,
            lengths: &self.affine_literal_lengths,
            values: Vec::new(),
            flags: Vec::new(),
            sum: None,
            increment: Affine::default(),
            cycle: Cycle {
                period: 1,
                count: 0,
            },
        };
        if !body
            .iter()
            .all(|s| plan.statement(s, &self.locals).is_some())
        {
            return;
        }
        let count = 1023 / plan.cycle.period * plan.cycle.period;
        let scale = i64::from(plan.increment.scale) * count;
        let Some(offset) = (count / plan.cycle.period)
            .checked_mul(plan.cycle.count)
            .and_then(|x| {
                x.checked_add(i64::from(plan.increment.offset) * count + scale * (count - 1) / 2)
            })
        else {
            return;
        };
        let Some(sum) = plan.sum else {
            return;
        };
        let at = self.borrow_slot(cursor);
        let end = self.borrow_slot(stop);
        let total = self.borrow_slot(sum);
        let (helper, definition, extent) = if count == 1023 {
            (
                "imre_affine_fixed_prefix",
                AFFINE_FIXED_PREFIX,
                String::new(),
            )
        } else {
            (
                "imre_affine_range_prefix",
                AFFINE_RANGE_PREFIX,
                format!("i64 {count}, "),
            )
        };
        self.output
            .local_helper("imre_affine_checked_prefix", AFFINE_CHECKED_PREFIX);
        self.output.local_helper(helper, definition);
        let prefix = self.instruction(format!(
            "call {{i64,i64}} @{helper}(i64 {}, i64 {}, i64 {}, {extent}i64 {scale}, i64 {offset})",
            at.operand, end.operand, total.operand
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

const AFFINE_FIXED_PREFIX: &str = r"
; Skip complete periods (at most 1,023 iterations) and publish locals in the ordinary body.
; Nonnegative i32 ordinals and coefficients keep each scalar expression small.
; The chunk subtotal is computed in i128 before narrowing to a tagged integer.
; Keep the usual fixed extent constant when this module also emits shorter periods.
define internal {i64,i64} @imre_affine_fixed_prefix(i64 %at, i64 %end, i64 %total, i64 %scale, i64 %offset) noinline nounwind memory(none) {
  %result = call {i64,i64} @imre_affine_checked_prefix(i64 %at, i64 %end, i64 %total, i64 1023, i64 %scale, i64 %offset)
  ret {i64,i64} %result
}
";

const AFFINE_RANGE_PREFIX: &str = r"
define internal {i64,i64} @imre_affine_range_prefix(i64 %at, i64 %end, i64 %total, i64 %count, i64 %scale, i64 %offset) noinline nounwind memory(none) {
  %result = call {i64,i64} @imre_affine_checked_prefix(i64 %at, i64 %end, i64 %total, i64 %count, i64 %scale, i64 %offset)
  ret {i64,i64} %result
}
";

const AFFINE_CHECKED_PREFIX: &str = r"
define internal {i64,i64} @imre_affine_checked_prefix(i64 %at, i64 %end, i64 %total, i64 %count, i64 %scale, i64 %offset) alwaysinline nounwind memory(none) {
  %tags0 = and i64 %at, %end
  %tags = and i64 %tags0, %total
  %small = trunc i64 %tags to i1
  %i = ashr i64 %at, 1
  %stop = ashr i64 %end, 1
  %sum = ashr i64 %total, 1
  %bounded = icmp ule i64 %i, 2147482624
  %last = add i64 %i, %count
  %within = icmp slt i64 %last, %stop
  %valid = and i1 %small, %bounded
  %chunk = and i1 %valid, %within
  br i1 %chunk, label %check_sum, label %fallback
check_sum:
  %a = zext i64 %scale to i128
  %b = zext i64 %offset to i128
  %x = zext i64 %i to i128
  %linear = mul i128 %x, %a
  %delta = add i128 %linear, %b
  %initial = sext i64 %sum to i128
  %partial = add i128 %initial, %delta
  %fits = icmp sle i128 %partial, 4611686018427387903
  br i1 %fits, label %success, label %fallback
success:
  %advance = shl i64 %count, 1
  %new_at = add i64 %at, %advance
  %n = trunc i128 %partial to i64
  %shift = shl i64 %n, 1
  %new_total = or i64 %shift, 1
  %r0 = insertvalue {i64,i64} zeroinitializer, i64 %new_at, 0
  %r = insertvalue {i64,i64} %r0, i64 %new_total, 1
  ret {i64,i64} %r
fallback:
  ret {i64,i64} zeroinitializer
}
";
