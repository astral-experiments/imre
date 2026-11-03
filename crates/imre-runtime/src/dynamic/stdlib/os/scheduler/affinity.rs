//! Dynamically sized Linux CPU sets in the libc ABI's native-word bit order.
use super::super::c_int;
use crate::dynamic::{Context, Error, Id, NONE, Result, Value, builtins};
use std::{io, mem::size_of_val};

type Word = libc::c_ulong;
const WORD_BITS: usize = Word::BITS as usize;

fn grow(mask: &mut Vec<Word>, words: usize) -> Result<()> {
    mask.try_reserve(words - mask.len())
        .map_err(|_| Error::new("MemoryError", ""))?;
    mask.resize(words, 0);
    Ok(())
}

pub(in crate::dynamic::stdlib) fn get(
    ctx: &mut Context,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let pid = builtins::one_positional_arg("posix.sched_getaffinity", args, kwargs)?;
    let pid = c_int(ctx, pid)?;
    let mut mask = vec![0 as Word];
    loop {
        // SAFETY: the aligned buffer holds the full byte length supplied to libc.
        let code = unsafe {
            libc::sched_getaffinity(pid, size_of_val(mask.as_slice()), mask.as_mut_ptr().cast())
        };
        if code == 0 {
            break;
        }
        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::EINVAL) {
            return Err(crate::objects::errno_error(error).into());
        }
        if mask.len() > i32::MAX as usize / (2 * WORD_BITS) {
            return Err(Error::new(
                "OverflowError",
                "could not allocate a large enough CPU set",
            ));
        }
        let next = mask.len() * 2;
        grow(&mut mask, next)?;
    }
    let result = ctx.alloc(Value::Set {
        entries: Default::default(),
        frozen: false,
    });
    for (index, mut bits) in mask.into_iter().enumerate() {
        while bits != 0 {
            let cpu = index * WORD_BITS + bits.trailing_zeros() as usize;
            let value = ctx.int(cpu as i64);
            builtins::set_add(ctx, result, value)?;
            bits &= bits - 1;
        }
    }
    Ok(result)
}

fn cpu_mask(ctx: &mut Context, source: Id) -> Result<Vec<Word>> {
    let iterator = ctx.iterator(source)?;
    ctx.heap.keep(iterator);
    let mark = ctx.heap.temporary_mark();
    let mut mask = vec![0 as Word];
    while let Some(item) = ctx.next_item(iterator)? {
        let Some(number) = ctx.index_value(item)? else {
            let ty = builtins::call(ctx, "type", &[item], &[])?;
            let repr = builtins::call(ctx, "repr", &[ty], &[])?;
            return Err(Error::new(
                "TypeError",
                format!(
                    "expected an iterator of ints, but iterator yielded {}",
                    ctx.str_value(repr)?
                ),
            ));
        };
        let cpu = number
            .to_i64()
            .and_then(|n| libc::c_long::try_from(n).ok())
            .ok_or_else(|| {
                Error::new("OverflowError", "Python int too large to convert to C long")
            })?;
        ctx.discard_truth(mark);
        if cpu < 0 {
            return Err(Error::new("ValueError", "negative CPU number"));
        }
        if cpu >= i32::MAX as libc::c_long {
            return Err(Error::new("OverflowError", "CPU number too large"));
        }
        let cpu = cpu as usize;
        let index = cpu / WORD_BITS;
        if index >= mask.len() {
            grow(&mut mask, index + 1)?;
        }
        mask[index] |= 1 << (cpu % WORD_BITS);
    }
    Ok(mask)
}

pub(in crate::dynamic::stdlib) fn set(
    ctx: &mut Context,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            "posix.sched_setaffinity() takes no keyword arguments",
        ));
    }
    let &[pid, source] = args else {
        return Err(Error::new(
            "TypeError",
            format!("sched_setaffinity expected 2 arguments, got {}", args.len()),
        ));
    };
    let pid = c_int(ctx, pid)?;
    let mark = ctx.heap.temporary_mark();
    let mask = cpu_mask(ctx, source)?;
    // The exhausted iterator may have a finalizer. Release it before changing affinity.
    ctx.discard_truth(mark);
    // SAFETY: libc reads an aligned array of native words bounded by the supplied byte count.
    if unsafe { libc::sched_setaffinity(pid, size_of_val(mask.as_slice()), mask.as_ptr().cast()) }
        != 0
    {
        return Err(crate::objects::errno_error(io::Error::last_os_error()).into());
    }
    Ok(NONE)
}
