//! Interpreter-owned exit callbacks; the Python module only exposes operations on the registry.

use super::{Context, Error, Id, NONE, Result, Value, copy_ids, unraisable};

pub(super) fn install(ctx: &mut Context, scope: Id) {
    for name in [
        "register",
        "unregister",
        "_clear",
        "_run_exitfuncs",
        "_ncallbacks",
    ] {
        let func = ctx.builtin(&format!("atexit.{name}"));
        ctx.bind(scope, name, func);
    }
}

pub(super) fn call(ctx: &mut Context, name: &str, args: &[Id], kw: &[(String, Id)]) -> Result<Id> {
    if name == "register" {
        let Some((&func, args)) = args.split_first() else {
            return Err(Error::new(
                "TypeError",
                "register() takes at least 1 argument (0 given)",
            ));
        };
        if !ctx.callable(func) {
            return Err(Error::new(
                "TypeError",
                "the first argument must be callable",
            ));
        }
        ctx.exit_callbacks
            .try_reserve(1)
            .map_err(|_| Error::new("MemoryError", ""))?;
        let args = ctx.alloc(Value::Tuple(copy_ids(args)?));
        let items = kw.iter().map(|(k, v)| (ctx.text(k), *v)).collect();
        let kwargs = ctx.dict(items);
        let record = ctx.alloc(Value::Tuple(vec![func, args, kwargs]));
        ctx.exit_callbacks.insert(0, record);
        return Ok(func);
    }
    if !kw.is_empty() {
        return Err(Error::new(
            "TypeError",
            format!("atexit.{name}() takes no keyword arguments"),
        ));
    }
    match (name, args) {
        ("unregister", &[target]) => unregister(ctx, target)?,
        ("_clear", []) => clear(ctx)?,
        ("_run_exitfuncs", []) => run(ctx),
        ("_ncallbacks", []) => return Ok(ctx.int(ctx.exit_callbacks.len() as i64)),
        _ => {
            return Err(Error::new(
                "TypeError",
                if name == "unregister" {
                    format!(
                        "atexit.unregister() takes exactly one argument ({} given)",
                        args.len()
                    )
                } else {
                    format!("atexit.{name}() takes no arguments ({} given)", args.len())
                },
            ));
        }
    }
    Ok(NONE)
}

fn operands(ctx: &Context, record: Id) -> (Id, Id, Id) {
    let Value::Tuple(values) = ctx.heap.get(record) else {
        unreachable!()
    };
    (values[0], values[1], values[2])
}

/// Release the list's strong references oldest first, retaining its other records while
/// a callback's finalizer or weakref hook reenters the registry.
fn release(ctx: &mut Context, mark: usize, count: usize) {
    if ctx.heap.observes_released_roots() {
        for end in (mark..mark + count).rev() {
            ctx.heap.release_temporaries_since(end);
            ctx.collect_unreachable();
        }
    }
    ctx.heap.release_temporaries_since(mark);
}

fn clear(ctx: &mut Context) -> Result<()> {
    let count = ctx.exit_callbacks.len();
    if count == 0 || !ctx.heap.observes_released_roots() {
        ctx.exit_callbacks.clear();
        return Ok(());
    }
    let mark = ctx.heap.temporary_mark();
    ctx.heap.reserve_temporaries(count)?;
    for record in ctx.exit_callbacks.drain(..) {
        ctx.heap.keep(record);
    }
    release(ctx, mark, count);
    Ok(())
}

/// Equality can insert or erase any entry. Rescan by registration identity at or below
/// the original index exactly as CPython does, then continue toward the newest entry.
fn unregister(ctx: &mut Context, target: Id) -> Result<()> {
    let mut end = ctx.exit_callbacks.len();
    while end != 0 {
        let i = end - 1;
        let record = ctx.exit_callbacks[i];
        let mark = ctx.heap.temporary_mark();
        ctx.heap.keep(record);
        let (func, _, _) = operands(ctx, record);
        let equal = match ctx.checked_equal(target, func) {
            Ok(equal) => equal,
            Err(error) => {
                ctx.heap.release_temporaries_since(mark);
                for root in error.roots() {
                    ctx.heap.keep(root);
                }
                if ctx.heap.observes_released_roots() {
                    ctx.collect_unreachable();
                }
                return Err(error);
            }
        };
        let mut next = i;
        if equal {
            let stop = end.min(ctx.exit_callbacks.len());
            if let Some(j) = ctx.exit_callbacks[..stop]
                .iter()
                .rposition(|&id| id == record)
            {
                ctx.exit_callbacks.remove(j);
                next = j;
            }
        }
        // Equality might itself have erased the record. Its finalizer can change the
        // list before CPython clamps the index for the next iteration.
        ctx.heap.release_temporaries_since(mark);
        if ctx.heap.observes_released_roots()
            && ctx.exit_callbacks.get(i) != Some(&record)
            && !ctx.exit_callbacks.contains(&record)
        {
            ctx.collect_unreachable();
        }
        end = next.min(ctx.exit_callbacks.len());
    }
    Ok(())
}

/// Invoke a rooted snapshot, leaving live registrations visible to nested runs until cleanup.
pub(super) fn run(ctx: &mut Context) {
    let mark = ctx.heap.temporary_mark();
    let snapshot = match copy_ids(&ctx.exit_callbacks).and_then(|ids| {
        ctx.heap.reserve_temporaries(ids.len())?;
        Ok(ids)
    }) {
        Ok(ids) => ids,
        Err(error) => {
            unraisable::report(
                ctx,
                error,
                false,
                None,
                "Exception ignored while copying atexit callbacks",
            );
            return;
        }
    };
    if snapshot.is_empty() {
        return;
    }
    for &record in &snapshot {
        ctx.heap.keep(record);
    }
    for &record in &snapshot {
        let entry_mark = ctx.heap.temporary_mark();
        let (func, args, kwargs) = operands(ctx, record);
        let result = ctx.items(args).and_then(|args| {
            let kwargs = ctx.keyword_args(kwargs)?;
            ctx.call(func, args, kwargs)
        });
        let release_result = ctx.heap.observes_released_roots()
            && matches!(result, Ok(value) if ctx.heap.could_own_finalizer(value, false));
        if let Err(error) = result {
            unraisable::report(
                ctx,
                error,
                false,
                Some(func),
                "Exception ignored in atexit callback",
            );
        }
        ctx.heap.release_temporaries_since(entry_mark);
        // CPython drops each callback result before invoking the next callback.
        if release_result {
            ctx.collect_unreachable();
        }
    }
    // CPython decrefs the snapshot before clearing live registrations. A callback
    // unregistered during execution can register another in __del__.
    if snapshot == ctx.exit_callbacks {
        ctx.heap.release_temporaries_since(mark);
    } else {
        release(ctx, mark, snapshot.len());
    }
    if let Err(error) = clear(ctx) {
        unraisable::report(
            ctx,
            error,
            false,
            None,
            "Exception ignored while clearing atexit callbacks",
        );
    }
}
