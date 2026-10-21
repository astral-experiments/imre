//! Branchless absolute values for pure integer conditional-negation idioms.

use crate::ir::{Compare, Expr, ExprKind, Type};

/// Recognizes `-x if x < 0 else x` and equivalent zero comparisons. All reads
/// must name the same exact integer local, so evaluation cannot call user code,
/// change the slot, or lose a side effect from the unselected branch.
pub(super) fn absolute_operand<'a>(test: &'a Expr, yes: &Expr, no: &Expr) -> Option<&'a Expr> {
    let ExprKind::Compare(values, operations) = &test.kind else {
        return None;
    };
    let ([left, right], [operation]) = (values.as_slice(), operations.as_slice()) else {
        return None;
    };
    let zero = |value: &Expr| matches!(&value.kind, ExprKind::Int(value) if value == "0");
    let (slot, negative, operand) = match (&left.kind, &right.kind) {
        (ExprKind::Local(slot), _) if left.ty == Type::Int && zero(right) => (
            *slot,
            match operation {
                Compare::Less | Compare::LessEqual => true,
                Compare::Greater | Compare::GreaterEqual => false,
                _ => return None,
            },
            left,
        ),
        (_, ExprKind::Local(slot)) if right.ty == Type::Int && zero(left) => (
            *slot,
            match operation {
                Compare::Greater | Compare::GreaterEqual => true,
                Compare::Less | Compare::LessEqual => false,
                _ => return None,
            },
            right,
        ),
        _ => return None,
    };
    let (negated, unchanged) = if negative { (yes, no) } else { (no, yes) };
    let ExprKind::Negate(value) = &negated.kind else {
        return None;
    };
    let same = |value: &Expr| {
        value.ty == Type::Int && matches!(value.kind, ExprKind::Local(other) if other == slot)
    };
    // Only the test's operand has facts valid before choosing an arm. A local
    // inside one arm may carry a narrower, branch-specific integer interval.
    (same(value) && same(unchanged)).then_some(operand)
}

/// Returns an owned absolute value. Negating the minimum tagged-small integer
/// promotes through the ordinary arithmetic helper; positive heap integers
/// retain their existing owner. The common path uses a select without a sign
/// branch, and wrapping subtraction is harmless for its unselected operand.
pub(super) const INTEGER_ABSOLUTE_HELPER: &str = r#"
define internal i64 @int_abs(i64 %value) alwaysinline {
entry:
  %tag = and i64 %value, 1
  %small = icmp eq i64 %tag, 1
  %fits = icmp ne i64 %value, -9223372036854775807
  %fast = and i1 %small, %fits
  br i1 %fast, label %select, label %slow
select:
  %negative = icmp slt i64 %value, 1
  %negated = sub i64 2, %value
  %result = select i1 %negative, i64 %negated, i64 %value
  %tagged = or i64 %result, 1
  ret i64 %tagged
slow:
  %order = call i32 @compare_int(i64 %value, i64 1)
  %below = icmp slt i32 %order, 0
  br i1 %below, label %negate, label %retain
negate:
  %positive = call i64 @int_sub(i64 1, i64 %value)
  ret i64 %positive
retain:
  call void @retain_int(i64 %value)
  ret i64 %value
}
"#;

#[cfg(test)]
mod tests {
    use crate::{CompileOptions, compile};

    #[test]
    fn recognizes_only_pure_same_local_integer_absolute_values() {
        for (expression, optimized) in [
            ("-value if value < 0 else value", true),
            ("-value if value <= 0 else value", true),
            ("value if value > 0 else -value", true),
            ("value if value >= 0 else -value", true),
            ("-value if 0 > value else value", true),
            ("-value if 0 >= value else value", true),
            ("value if 0 < value else -value", true),
            ("value if 0 <= value else -value", true),
            ("-value if value < 1 else value", false),
            ("-value if value == 0 else value", false),
            ("-value if value < 0 else other", false),
            ("-other if value < 0 else value", false),
            ("value if value < 0 else -value", false),
        ] {
            let source =
                format!("def magnitude(value: int, other: int) -> int:\n    return {expression}\n");
            let output = compile(&source, &CompileOptions::default()).unwrap();
            assert_eq!(
                output.contains("call i64 @int_abs(i64 %"),
                optimized,
                "{expression}"
            );
        }
    }
}
