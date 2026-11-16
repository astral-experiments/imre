//! Bound, constraint, and default values on declared type parameters.

use super::super::{Context, Error, Id, NONE, Result, Value, value::VisitReferences};
use super::AliasDefinition;

#[derive(Clone, Copy)]
pub(in super::super) enum ParameterValue {
    Eager(Id),
    Lazy(LazyEvaluation),
}

pub(in super::super) use imre_ast::TypeParameterField as ParameterField;

/// A declared alias value or parameter field keeps its callable even after a successful read.
#[derive(Clone, Copy)]
pub(in super::super) struct LazyEvaluation {
    pub(super) evaluator: Id,
    cached: Option<Id>,
}

impl LazyEvaluation {
    pub(in super::super) fn new(evaluator: Id) -> Self {
        Self {
            evaluator,
            cached: None,
        }
    }

    pub(super) fn read(
        self,
        ctx: &mut Context,
        owner: Id,
        field: Option<ParameterField>,
    ) -> Result<Id> {
        if let Some(value) = self.cached {
            return Ok(value);
        }
        ctx.heap.reserve_temporaries(1)?;
        // The owner's reference visitor keeps the evaluator and its closure live through the call.
        ctx.heap.keep(owner);
        let value = ctx.call(self.evaluator, Vec::new(), Vec::new())?;
        let slot = match (ctx.heap.get_mut(owner), field) {
            (
                Value::TypeAlias {
                    definition: AliasDefinition::Lazy(lazy),
                    ..
                },
                None,
            ) => lazy,
            (owner, Some(field)) => {
                let Some(ParameterValue::Lazy(lazy)) = parameter_slot(field, owner) else {
                    unreachable!("declared type parameter field")
                };
                lazy
            }
            _ => unreachable!("declared type alias"),
        };
        slot.cached = Some(value);
        Ok(value)
    }

    pub(in super::super) fn visit(self, visit: &mut impl VisitReferences) {
        visit.push(self.evaluator);
        visit.extend(self.cached);
    }
}

fn parameter_slot(field: ParameterField, parameter: &mut Value) -> Option<&mut ParameterValue> {
    match (field, parameter) {
        (ParameterField::Bound, Value::TypeVar(var)) => Some(&mut var.bound),
        (ParameterField::Constraints, Value::TypeVar(var)) => Some(&mut var.constraints),
        (ParameterField::Default, Value::TypeVar(var)) => Some(&mut var.default),
        (ParameterField::Default, Value::ParamSpec(spec)) => Some(&mut spec.default),
        (ParameterField::Default, Value::TypeVarTuple { default, .. }) => Some(default),
        _ => None,
    }
}

impl ParameterValue {
    pub(in super::super) fn read(
        self,
        ctx: &mut Context,
        owner: Id,
        field: ParameterField,
    ) -> Result<Id> {
        match self {
            Self::Eager(value) => Ok(value),
            Self::Lazy(lazy) => lazy.read(ctx, owner, Some(field)),
        }
    }

    pub(in super::super) fn has_default(self, ctx: &Context) -> bool {
        !matches!(self, Self::Eager(value) if matches!(ctx.heap.get(value), Value::NoDefault))
    }

    pub(in super::super) fn evaluator(
        self,
        declared: bool,
        field: ParameterField,
        ctx: &mut Context,
    ) -> Id {
        match self {
            Self::Lazy(lazy) => lazy.evaluator,
            Self::Eager(_) if declared => NONE,
            Self::Eager(value) => match field {
                ParameterField::Bound if value == NONE => NONE,
                ParameterField::Constraints if matches!(ctx.heap.get(value), Value::Tuple(items) if items.is_empty()) => {
                    NONE
                }
                _ => ctx.alloc(Value::ConstEvaluator(value)),
            },
        }
    }

    pub(in super::super) fn visit(self, visit: &mut impl VisitReferences) {
        match self {
            Self::Eager(value) => visit.push(value),
            Self::Lazy(lazy) => lazy.visit(visit),
        }
    }
}

pub(in super::super) fn set_parameter_evaluator(
    ctx: &mut Context,
    parameter: Id,
    field: Id,
    evaluator: Id,
) -> Result<Id> {
    let field = ParameterField::from_name(ctx.str_value(field)?)
        .ok_or_else(|| Error::new("SystemError", "unknown type parameter field"))?;
    let slot = parameter_slot(field, ctx.heap.get_mut(parameter))
        .ok_or_else(|| Error::new("SystemError", "invalid type parameter for evaluator"))?;
    *slot = ParameterValue::Lazy(LazyEvaluation::new(evaluator));
    Ok(NONE)
}
