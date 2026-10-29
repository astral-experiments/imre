//! Runtime unions preserve argument order for display and unordered membership for comparisons.
use super::{Context, Error, Id, NONE, Result, Value, typing};

#[derive(Clone, Copy)]
pub(super) struct Union {
    pub args: Id,
    pub members: Id,
    pub unhashable: Option<Id>,
    pub parameters: Id,
}

fn tuple_items(ctx: &Context, id: Id) -> &[Id] {
    let Value::Tuple(items) = ctx.heap.get(id) else {
        unreachable!()
    };
    items
}

impl Union {
    /// Hash the frozenset of members, preserving errors from originally unhashable arguments.
    pub(super) fn hash(self, ctx: &mut Context) -> Result<i64> {
        let Some(unhashable) = self.unhashable else {
            return ctx.dictionary_hash(self.members);
        };
        let items = tuple_items(ctx, unhashable).to_vec();
        for &item in &items {
            ctx.dictionary_hash(item)?;
        }
        Err(Error::new(
            "TypeError",
            format!("union contains {} unhashable elements", items.len()),
        ))
    }

    /// Compare the frozen member sets and the two sequences of unhashable members.
    pub(super) fn checked_equal(self, other: Self, ctx: &mut Context) -> Result<bool> {
        if !ctx.dictionary_equal(self.members, other.members)? {
            return Ok(false);
        }
        let (Some(left), Some(right)) = (self.unhashable, other.unhashable) else {
            return Ok(self.unhashable == other.unhashable);
        };
        let count = tuple_items(ctx, left).len();
        if count != tuple_items(ctx, right).len() {
            return Ok(false);
        }
        for (source, target) in [(left, right), (right, left)] {
            for i in 0..count {
                let item = tuple_items(ctx, source)[i];
                if !ctx.contains(item, target)? {
                    return Ok(false);
                }
            }
        }
        Ok(true)
    }

    pub(super) fn structural_equal(
        self,
        other: Self,
        ctx: &Context,
        mut equal: impl FnMut(Id, Id) -> bool,
    ) -> bool {
        if !equal(self.members, other.members) {
            return false;
        }
        match (self.unhashable, other.unhashable) {
            (Some(a), Some(b)) => {
                let (a, b) = (tuple_items(ctx, a), tuple_items(ctx, b));
                a.len() == b.len() && a.iter().all(|&x| b.iter().any(|&y| equal(x, y)))
            }
            (a, b) => a == b,
        }
    }
}

/// typing.Union and types.UnionType publish the same class on Python 3.14.
pub(super) fn class(ctx: &mut Context) -> Id {
    typing::named_class(ctx, "typing", "Union")
}

fn accepts(ctx: &Context, id: Id) -> bool {
    id == NONE
        || ctx.builtin_type_name(id) == Some("type")
        || matches!(
            ctx.heap.get(id),
            Value::GenericAlias(_) | Value::TypeVar(_) | Value::Union(_)
        )
        || matches!(ctx.heap.get(id), Value::Native(n) if matches!(n.as_str(), "typing.Any" | "typing.Callable"))
}

/// Flatten nested unions left to right and discard repeated types.
pub(super) fn make(ctx: &mut Context, inputs: &[Id]) -> Result<Id> {
    let mut flattened = Vec::new();
    for &input in inputs {
        if matches!(ctx.heap.get(input), Value::Instance { .. })
            && let Some(&typing) = ctx.modules.get("typing")
            && let Value::Module { scope, .. } = *ctx.heap.get(typing)
            && let Some(check) = ctx.scope_name(scope, "_type_check")
        {
            let message = ctx.text("Union[arg, ...]: each arg must be a type.");
            ctx.call(check, vec![input, message], Vec::new())?;
        }
        if matches!(ctx.heap.get(input), Value::Str(_)) {
            return Err(Error::new(
                "NotImplementedError",
                "string Union arguments require typing.ForwardRef",
            ));
        }
        let input = if input == NONE {
            ctx.builtin("NoneType")
        } else {
            input
        };
        let items = match ctx.heap.get(input) {
            Value::Union(nested) => tuple_items(ctx, nested.args),
            _ => std::slice::from_ref(&input),
        };
        flattened
            .try_reserve(items.len())
            .map_err(|_| Error::new("MemoryError", ""))?;
        flattened.extend_from_slice(items);
    }
    let members = ctx.alloc(Value::Set {
        entries: Default::default(),
        frozen: true,
    });
    ctx.heap.keep(members);
    let mut args = Vec::new();
    let mut unhashable = Vec::new();
    'member: for arg in flattened {
        if ctx.dictionary_hash(arg).is_ok() {
            if super::builtins::set_member(ctx, members, arg, false)? {
                continue;
            }
            super::builtins::set_add(ctx, members, arg)?;
        } else {
            for &previous in &unhashable {
                if ctx.dictionary_equal(previous, arg)? {
                    continue 'member;
                }
            }
            unhashable
                .try_reserve(1)
                .map_err(|_| Error::new("MemoryError", ""))?;
            unhashable.push(arg);
        }
        args.try_reserve(1)
            .map_err(|_| Error::new("MemoryError", ""))?;
        args.push(arg);
    }
    match args.as_slice() {
        [] => Err(Error::new("TypeError", "Cannot take a Union of no types.")),
        [single] => Ok(*single),
        _ => {
            let unhashable = (!unhashable.is_empty()).then(|| ctx.alloc(Value::Tuple(unhashable)));
            let parameters = typing::variables(ctx, &args);
            let parameters = ctx.alloc(Value::Tuple(parameters));
            let args = ctx.alloc(Value::Tuple(args));
            Ok(ctx.alloc(Value::Union(Union {
                args,
                members,
                unhashable,
                parameters,
            })))
        }
    }
}

pub(super) fn binary(ctx: &mut Context, left: Id, right: Id) -> Option<Result<Id>> {
    if matches!(ctx.heap.get(left), Value::Union(_) | Value::TypeVar(_))
        || matches!(ctx.heap.get(right), Value::Union(_) | Value::TypeVar(_))
        || (accepts(ctx, left) && accepts(ctx, right) && (left != NONE || right != NONE))
    {
        return Some(make(ctx, &[left, right]));
    }
    if ctx.builtin_type_name(left) == Some("type") || ctx.builtin_type_name(right) == Some("type") {
        let type_name = |value| {
            ctx.builtin_type_name(value)
                .unwrap_or_else(|| ctx.type_name(value))
        };
        return Some(Err(Error::new(
            "TypeError",
            format!(
                "unsupported operand type(s) for |: '{}' and '{}'",
                type_name(left),
                type_name(right)
            ),
        )));
    }
    None
}

pub(super) fn subscribe(ctx: &mut Context, target: Id, index: Id) -> Option<Result<Id>> {
    if ctx.classes.get("typing.Union") != Some(&target) {
        return None;
    }
    Some(match ctx.heap.get(index) {
        Value::Tuple(args) => super::copy_ids(args).and_then(|args| make(ctx, &args)),
        _ => make(ctx, &[index]),
    })
}
