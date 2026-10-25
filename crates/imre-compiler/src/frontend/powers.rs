//! Integer power has a fixed result type with a modulus or a known exponent sign.

use super::*;

/// Python bools participate in arithmetic as zero and one.
pub(super) fn integer(value: Expr) -> Expr {
    if value.ty == Type::Bool {
        Expr {
            ty: Type::Int,
            kind: ExprKind::Builtin(Builtin::Int, vec![value]),
        }
    } else {
        value
    }
}

impl Checker<'_> {
    pub(super) fn integer_power(
        &self,
        base: &Expr,
        exponent: &Expr,
        modular: bool,
        node: &impl Ranged,
    ) -> Result<(Builtin, Type), String> {
        if !matches!(base.ty, Type::Int | Type::Bool)
            || !matches!(exponent.ty, Type::Int | Type::Bool)
        {
            return Err(error(
                self.source,
                node,
                "native integer power requires integer operands",
            ));
        }
        if modular {
            return Ok((Builtin::ModularPower, Type::Int));
        }
        let negative = match &exponent.kind {
            ExprKind::Int(value) => value.starts_with('-'),
            ExprKind::Negate(value) if let ExprKind::Int(integer) = &value.kind => {
                integer != "0" && !integer.starts_with('-')
            }
            ExprKind::Builtin(Builtin::Len | Builtin::Abs, _) => false,
            _ if exponent.ty == Type::Bool => false,
            _ => {
                return Err(error(
                    self.source,
                    node,
                    "power without a modulus needs a known exponent sign to distinguish int and float results",
                ));
            }
        };
        Ok(if negative {
            (Builtin::NegativeIntegerPower, Type::Float)
        } else {
            (Builtin::IntegerPower, Type::Int)
        })
    }

    pub(super) fn pow_call(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        let params = ["base", "exp", "mod"].map(|name| Parameter {
            name: name.into(),
            ty: Type::Int,
            kind: ParameterKind::PositionalOrKeyword,
            default: (name == "mod").then_some(Expr {
                ty: Type::None,
                kind: ExprKind::None,
            }),
            sign_only: false,
        });
        let mut args = self.arguments_with(call, &params, None, |checker, _, argument| {
            checker.expr(argument)
        })?;
        let modulus = &args.values[args.order[2]];
        if !matches!(modulus.ty, Type::Int | Type::Bool | Type::None) {
            return Err(error(
                self.source,
                call,
                "native integer power requires an integer modulus or None",
            ));
        }
        let (operation, ty) = self.integer_power(
            &args.values[args.order[0]],
            &args.values[args.order[1]],
            modulus.ty != Type::None,
            call,
        )?;
        args.values = args.values.into_iter().map(integer).collect();
        Ok(self.ordered_builtin(operation, ty, args))
    }
}
