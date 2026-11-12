//! Native bisection on a Python sequence. The caller owns the comparison key.

use super::{Context, Error, Id, NONE, Result, Value};

pub(in crate::dynamic) fn method_metadata(
    name: &str,
) -> Option<(&'static str, Option<&'static str>)> {
    let doc = match name {
        "bisect_left" => {
            r#"Return the index where to insert item x in list a, assuming a is sorted.

The return value i is such that all e in a[:i] have e < x, and all e in
a[i:] have e >= x.  So if x already appears in the list, a.insert(i, x) will
insert just before the leftmost x already there.

Optional args lo (default 0) and hi (default len(a)) bound the
slice of a to be searched.

A custom key function can be supplied to customize the sort order."#
        }
        "bisect_right" => {
            r#"Return the index where to insert item x in list a, assuming a is sorted.

The return value i is such that all e in a[:i] have e <= x, and all e in
a[i:] have e > x.  So if x already appears in the list, a.insert(i, x) will
insert just after the rightmost x already there.

Optional args lo (default 0) and hi (default len(a)) bound the
slice of a to be searched.

A custom key function can be supplied to customize the sort order."#
        }
        "insort_left" => {
            r#"Insert item x in list a, and keep it sorted assuming a is sorted.

If x is already in a, insert it to the left of the leftmost x.

Optional args lo (default 0) and hi (default len(a)) bound the
slice of a to be searched.

A custom key function can be supplied to customize the sort order."#
        }
        "insort_right" => {
            r#"Insert item x in list a, and keep it sorted assuming a is sorted.

If x is already in a, insert it to the right of the rightmost x.

Optional args lo (default 0) and hi (default len(a)) bound the
slice of a to be searched.

A custom key function can be supplied to customize the sort order."#
        }
        _ => return None,
    };
    Some(("($module, /, a, x, lo=0, hi=None, *, key=None)", Some(doc)))
}

pub(in crate::dynamic) fn install(ctx: &mut Context, module: Id, scope: Id) {
    let doc = ctx.text(
        r#"Bisection algorithms.

This module provides support for maintaining a list in sorted order without
having to sort the list after each insertion. For long lists of items with
expensive comparison operations, this can be an improvement over the more
common approach.
"#,
    );
    ctx.bind(scope, "__doc__", doc);
    for name in ["bisect_left", "bisect_right", "insort_left", "insort_right"] {
        let callable = ctx.builtin(&format!("_bisect.{name}"));
        let function = ctx.alloc(Value::Method {
            callable,
            receiver: module,
            explicit: false,
        });
        ctx.bind(scope, name, function);
    }
}

/// Return the insertion point using the sequence's item slot, including on an empty range.
fn insertion_point(
    ctx: &mut Context,
    sequence: Id,
    needle: Id,
    mut lo: i64,
    mut hi: i64,
    key: Option<Id>,
    left: bool,
) -> Result<i64> {
    if lo < 0 {
        return Err(Error::new("ValueError", "lo must be non-negative"));
    }
    if hi == -1 {
        hi = ctx.sequence_protocol_len(sequence)? as i64;
    }
    ctx.require_sequence_item_slot(sequence)?;
    let mut compare_class = ctx.heap.get(needle).instance_class();
    let mark = ctx.heap.temporary_mark();
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        let index = ctx.int(mid);
        let item = ctx.get_item(sequence, index)?;
        let item = match key {
            Some(key) => {
                let result = ctx.call(key, vec![item], Vec::new());
                ctx.release_with_result(mark, result)?
            }
            None => item,
        };
        ctx.heap.keep(item);
        let compared = ctx.heap.temporary_mark();
        let less = (|| {
            let (a, b) = if left { (item, needle) } else { (needle, item) };
            // The accelerator tries the homogeneous comparison slot directly.
            // Its first decline retries normal rich comparison for the rest of this search.
            let direct = if compare_class.is_some()
                && ctx.heap.get(item).instance_class() == compare_class
            {
                let result = ctx.operator_slot(a, "__lt__", b)?;
                if result.is_none() {
                    compare_class = None;
                }
                result
            } else {
                None
            };
            let less = match direct {
                Some(less) => less,
                None => ctx.binary("<", a, b)?,
            };
            ctx.truth(less).map(super::builtins::boolean)
        })();
        let less = ctx.release_with_result(compared, less);
        let less = ctx.release_with_result(mark, less)?;
        if less == super::builtins::boolean(left) {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    Ok(lo)
}

/// Random.choices searches cumulative weights with the same right-hand tie rule.
pub(super) fn right(ctx: &mut Context, sequence: Id, needle: Id, hi: i64) -> Result<i64> {
    insertion_point(ctx, sequence, needle, 0, hi, None, false)
}

pub(super) fn call(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let params = ["a", "x", "lo", "hi", "key"];
    let [sequence, x, lo, hi, key] = if args.len() > 4 {
        super::builtins::bind_method_arguments(name, args, kwargs, params, 4)?
    } else {
        super::builtins::bind_builtin_arguments(name, args, kwargs, params, 2)?
    };
    let sequence = sequence.unwrap();
    let x = x.unwrap();
    let lo = lo
        .map(|n| super::os::ssize_t(ctx, n))
        .transpose()?
        .unwrap_or(0) as i64;
    let hi = match hi.filter(|&n| n != NONE) {
        Some(n) => {
            let index = ctx.index_value(n)?.ok_or_else(|| {
                Error::new(
                    "TypeError",
                    format!(
                        "argument should be integer or None, not '{}'",
                        ctx.type_name(n)
                    ),
                )
            })?;
            crate::signed_index(&index).ok_or_else(|| {
                Error::new(
                    "OverflowError",
                    "Python int too large to convert to C ssize_t",
                )
            })? as i64
        }
        None => -1,
    };
    let key = key.filter(|&f| f != NONE);
    let insert = name.starts_with("insort");
    let mark = ctx.heap.temporary_mark();
    let needle = match (insert, key) {
        (true, Some(key)) => ctx.call(key, vec![x], Vec::new())?,
        _ => x,
    };
    ctx.heap.keep(needle);
    let index = insertion_point(ctx, sequence, needle, lo, hi, key, name.ends_with("_left"))
        .map(|index| ctx.int(index));
    let index = ctx.release_with_result(mark, index)?;
    if insert {
        let method = ctx.lookup_attr(sequence, "insert")?;
        ctx.call(method, vec![index, x], Vec::new())?;
        Ok(NONE)
    } else {
        Ok(index)
    }
}
