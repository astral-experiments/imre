//! Partition exception groups using their public derive hook.

use std::collections::HashSet;

use super::Context;
use super::value::{Error, Id, NONE, Result, Value};

pub(super) const METHODS: &[&str] = &["derive", "split", "subgroup"];

enum Matcher {
    Types(Vec<Id>),
    Predicate(Id),
    Leaves(HashSet<Id>),
}

impl Matcher {
    fn new(ctx: &mut Context, condition: Id) -> Result<Self> {
        let base = ctx.builtin("BaseException");
        let valid = |ty| ctx.is_type_object(ty) && ctx.direct_subclass(ty, base);
        if valid(condition) {
            return Ok(Self::Types(vec![condition]));
        }
        if let Value::Tuple(types) = ctx.heap.get(condition) {
            if types.iter().copied().all(valid) {
                return Ok(Self::Types(types.clone()));
            }
        } else if !ctx.is_type_object(condition) && ctx.callable(condition) {
            return Ok(Self::Predicate(condition));
        }
        Err(Error::new(
            "TypeError",
            "expected an exception type, a tuple of exception types, or a callable (other than a class)",
        ))
    }

    fn matches(&self, ctx: &mut Context, exc: Id) -> Result<bool> {
        match self {
            Self::Types(types) => {
                let cls = ctx.exact_type(exc)?;
                Ok(types.iter().any(|&ty| ctx.direct_subclass(cls, ty)))
            }
            Self::Leaves(ids) => Ok(ids.contains(&exc)),
            Self::Predicate(test) => {
                let result = ctx.call(*test, vec![exc], vec![])?;
                ctx.truth(result)
            }
        }
    }
}

pub(super) fn call(
    ctx: &mut Context,
    method: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let Some((&group, args)) = args.split_first() else {
        return Err(Error::new(
            "TypeError",
            format!("unbound method BaseExceptionGroup.{method}() needs an argument"),
        ));
    };
    let Some([message, _]) = group_fields(ctx, group) else {
        return Err(Error::new(
            "TypeError",
            format!(
                "descriptor '{method}' for 'BaseExceptionGroup' objects doesn't apply to a '{}' object",
                ctx.type_name(group)
            ),
        ));
    };
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            format!(
                "{}.{method}() takes no keyword arguments",
                ctx.type_name(group)
            ),
        ));
    }
    let &[arg] = args else {
        return Err(Error::new(
            "TypeError",
            format!(
                "{}.{method}() takes exactly one argument ({} given)",
                ctx.type_name(group),
                args.len()
            ),
        ));
    };
    if method == "derive" {
        let base = ctx.builtin("BaseExceptionGroup");
        return ctx.call(base, vec![message, arg], vec![]);
    }
    let condition = Matcher::new(ctx, arg)?;
    let split = method == "split";
    let (matched, rest) = partition(ctx, group, &condition, split, 0)?;
    Ok(if split {
        ctx.alloc(Value::Tuple(vec![matched, rest]))
    } else {
        matched
    })
}

fn group_fields(ctx: &Context, value: Id) -> Option<[Id; 2]> {
    match ctx.heap.get(value) {
        Value::Raised(error) => error.group(),
        _ => None,
    }
}

fn partition(
    ctx: &mut Context,
    exc: Id,
    condition: &Matcher,
    keep_rest: bool,
    depth: usize,
) -> Result<(Id, Id)> {
    ctx.check_call_depth(depth)?;
    if condition.matches(ctx, exc)? {
        return Ok((exc, NONE));
    }
    let Some([_, children]) = group_fields(ctx, exc) else {
        return Ok((NONE, if keep_rest { exc } else { NONE }));
    };
    let Value::Tuple(children) = ctx.heap.get(children) else {
        unreachable!()
    };
    let children = children.clone();
    let mut matched = Vec::new();
    let mut rest = Vec::new();
    for child in children {
        let (yes, no) = stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || {
            partition(ctx, child, condition, keep_rest, depth + 1)
        })?;
        if yes != NONE {
            matched.push(yes);
        }
        if no != NONE {
            rest.push(no);
        }
    }
    Ok((
        derive_subset(ctx, exc, matched)?,
        derive_subset(ctx, exc, rest)?,
    ))
}

fn derive_subset(ctx: &mut Context, original: Id, children: Vec<Id>) -> Result<Id> {
    if children.is_empty() {
        return Ok(NONE);
    }
    let derive = ctx.lookup_attr(original, "derive")?;
    let items = ctx.alloc(Value::List(children));
    let result = ctx.call(derive, vec![items], vec![])?;
    if group_fields(ctx, result).is_none() {
        return Err(Error::new(
            "TypeError",
            "derive must return an instance of BaseExceptionGroup",
        ));
    }
    ctx.heap.keep(result);

    let tb = ctx.exception_traceback(original)?;
    if tb != NONE {
        ctx.set_traceback(result, tb)?;
    }
    let Value::Raised(error) = ctx.heap.get(original) else {
        unreachable!()
    };
    let cause = error
        .trace
        .as_ref()
        .and_then(|trace| trace.cause)
        .unwrap_or(NONE);
    let context = error.context;
    ctx.set_exception_cause(result, cause)?;
    let Value::Raised(error) = ctx.heap.get_mut(result) else {
        unreachable!()
    };
    let old_context = std::mem::replace(&mut error.context, context);
    ctx.heap.weak_edge_removed(old_context);

    if let Some(notes) = ctx.optional_attr(original, "__notes__")?
        && ctx.is_sequence(notes)
    {
        let copied = ctx.alloc(Value::List(Vec::new()));
        super::builtins::list_extend(ctx, copied, notes)?;
        ctx.set_attr(result, "__notes__", copied)?;
    }
    Ok(result)
}

/// Match a single except* clause. A complete match bypasses the group's split hook.
pub(super) fn split_star(ctx: &mut Context, exception: Id, condition: Id) -> Result<(Id, Id)> {
    let types = ctx.tuple_items(condition)?;
    let types = types.as_deref().unwrap_or(std::slice::from_ref(&condition));
    let base = ctx.builtin("BaseException");
    ctx.check_exception_classes(types, base)?;
    let group = ctx.builtin("BaseExceptionGroup");
    if types.iter().any(|&ty| ctx.direct_subclass(ty, group)) {
        return Err(Error::new(
            "TypeError",
            "catching ExceptionGroup with except* is not allowed. Use except instead.",
        ));
    }
    if exception == NONE {
        return Ok((NONE, NONE));
    }
    let cls = ctx.exact_type(exception)?;
    if types.iter().any(|&ty| ctx.direct_subclass(cls, ty)) {
        if group_fields(ctx, exception).is_some() {
            return Ok((exception, NONE));
        }
        let wrapped = make_group(ctx, vec![exception])?;
        if let Some((loc, frame)) = ctx.active_frame(0)? {
            let tb = ctx.alloc(Value::Traceback {
                frame,
                next: NONE,
                lineno: i64::from(loc.line),
                span: loc.span,
            });
            ctx.set_traceback(wrapped, tb)?;
        }
        return Ok((wrapped, NONE));
    }
    if group_fields(ctx, exception).is_none() {
        return Ok((NONE, exception));
    }
    let split = ctx.lookup_attr(exception, "split")?;
    let pair = ctx.call(split, vec![condition], Vec::new())?;
    let Value::Tuple(parts) = ctx.heap.get(pair) else {
        return Err(Error::new(
            "TypeError",
            format!(
                "{}.split must return a tuple, not {}",
                ctx.type_name(exception),
                ctx.type_name(pair)
            ),
        ));
    };
    if let [matched, rest, ..] = parts.as_slice() {
        Ok((*matched, *rest))
    } else {
        Err(Error::new(
            "TypeError",
            format!(
                "{}.split must return a 2-tuple, got tuple of size {}",
                ctx.type_name(exception),
                parts.len()
            ),
        ))
    }
}

fn make_group(ctx: &mut Context, exceptions: Vec<Id>) -> Result<Id> {
    let class = ctx.builtin("BaseExceptionGroup");
    let message = ctx.text("");
    let items = ctx.alloc(Value::Tuple(exceptions));
    ctx.call(class, vec![message, items], Vec::new())
}

/// Project unchanged subgroups onto the original tree. Newly raised exceptions precede it.
pub(super) fn merge_star(ctx: &mut Context, original: Id, raised: &[Id]) -> Result<Id> {
    if group_fields(ctx, original).is_none() {
        return Ok(raised.iter().copied().find(|&e| e != NONE).unwrap_or(NONE));
    }
    let metadata = |ctx: &mut Context, id| -> Result<_> {
        let tb = ctx.exception_traceback(id)?;
        let Value::Raised(error) = ctx.heap.get(id) else {
            return Err(Error::new("TypeError", "expected a BaseException instance"));
        };
        // Public __notes__ lives in the instance dictionary. Deriving a subgroup copies it.
        Ok((
            tb,
            error.context,
            error.trace.as_ref().and_then(|t| t.cause).unwrap_or(NONE),
        ))
    };
    let original_metadata = metadata(ctx, original)?;
    let mut new = Vec::new();
    let mut leaves = HashSet::new();
    let mut pending = Vec::new();
    for &exc in raised.iter().filter(|&&e| e != NONE) {
        if metadata(ctx, exc)? != original_metadata {
            new.push(exc);
            continue;
        }
        pending.push(exc);
        while let Some(item) = pending.pop() {
            if let Some([_, children]) = group_fields(ctx, item) {
                let Value::Tuple(children) = ctx.heap.get(children) else {
                    unreachable!()
                };
                pending.extend(children);
            } else {
                leaves.insert(item);
            }
        }
    }
    if !leaves.is_empty() {
        let (remaining, _) = partition(ctx, original, &Matcher::Leaves(leaves), false, 0)?;
        if remaining != NONE {
            new.push(remaining);
        }
    }
    match new.as_slice() {
        [] => Ok(NONE),
        [only] => Ok(*only),
        _ => make_group(ctx, new),
    }
}
