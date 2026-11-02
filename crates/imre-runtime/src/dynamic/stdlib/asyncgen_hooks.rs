use super::{Context, Error, Id, NONE, Result, audit};

fn set_hook(ctx: &mut Context, index: usize, value: Id) -> Result<()> {
    let events = [
        "sys.set_asyncgen_hook_firstiter",
        "sys.set_asyncgen_hook_finalizer",
    ];
    audit::event(ctx, events[index], &[])?;
    let previous = std::mem::replace(&mut ctx.threads.asyncgen_hooks[index], value);
    if previous != value
        && previous != NONE
        && (ctx.heap.observes_released_roots() || ctx.heap.borrowed_hooks.contains(&previous))
    {
        ctx.collect_unreachable();
    }
    Ok(())
}

pub(super) fn set(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let names = ["firstiter", "finalizer"];
    let count = args.len() + kwargs.len();
    if count > 2 {
        let kind = if args.is_empty() { "keyword " } else { "" };
        return Err(Error::new(
            "TypeError",
            format!("function takes at most 2 {kind}arguments ({count} given)"),
        ));
    }
    let mut hooks = [args.first().copied(), args.get(1).copied()];
    for (key, value) in kwargs {
        let Some(index) = names.iter().position(|name| name == key) else {
            let mut message = format!("this function got an unexpected keyword argument '{key}'");
            if let Some(hint) = crate::objects::exceptions::keyword_suggestion(key, &names) {
                message.push_str(&format!(". Did you mean '{hint}'?"));
            }
            return Err(Error::new("TypeError", message));
        };
        if hooks[index].replace(*value).is_some() {
            return Err(Error::new(
                "TypeError",
                format!(
                    "argument for function given by name ('{key}') and position ({})",
                    index + 1
                ),
            ));
        }
    }
    let [firstiter, finalizer] = hooks;
    for (hook, name) in [(finalizer, "finalizer"), (firstiter, "firstiter")] {
        if let Some(value) = hook
            && value != NONE
            && !ctx.callable(value)
        {
            let class = ctx.type_name(value);
            let class = &class[..class.floor_char_boundary(50)];
            return Err(Error::new(
                "TypeError",
                format!("callable {name} expected, got {class}"),
            ));
        }
    }

    // Validate both hooks before changing either. CPython audits and installs
    // the finalizer first, so firstiter's audit hook can observe or veto it.
    let Some(firstiter) = firstiter else {
        if let Some(value) = finalizer {
            set_hook(ctx, 1, value)?;
        }
        return Ok(NONE);
    };
    // A replacement can deallocate the old hook before the next audit runs.
    // Keep a non-owning handle that the sweep invalidates before slot reuse.
    ctx.heap
        .borrowed_hooks
        .try_reserve(1)
        .map_err(|_| Error::new("MemoryError", ""))?;
    let saved = ctx.heap.borrowed_hooks.len();
    ctx.heap.borrowed_hooks.push(ctx.threads.asyncgen_hooks[1]);
    let result = (|| {
        if let Some(value) = finalizer {
            set_hook(ctx, 1, value)?;
        }
        if let Err(error) = set_hook(ctx, 0, firstiter) {
            for root in error.roots() {
                ctx.heap.keep(root);
            }
            let previous = ctx.heap.borrowed_hooks[saved];
            // The rollback audit can drop the last external reference as well.
            ctx.heap.keep(previous);
            set_hook(ctx, 1, previous)?;
            return Err(error);
        }
        Ok(NONE)
    })();
    ctx.heap.borrowed_hooks.pop();
    result
}
