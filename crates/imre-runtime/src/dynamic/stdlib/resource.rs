//! Linux process resource limits.
use super::{Context, Error, Id, NONE, Result, Value, audit, os, os_error};
use num_bigint::{BigInt, Sign};

// CPython enables _FILE_OFFSET_BITS=64, including on 32-bit glibc.
#[cfg(not(all(target_env = "gnu", target_pointer_width = "32")))]
pub(super) use libc::RLIM_INFINITY as INFINITY;
#[cfg(all(target_env = "gnu", target_pointer_width = "32"))]
pub(super) use libc::RLIM64_INFINITY as INFINITY;
#[cfg(not(all(target_env = "gnu", target_pointer_width = "32")))]
use libc::{getrlimit as get_limits, rlim_t, rlimit, setrlimit as set_limits};
#[cfg(all(target_env = "gnu", target_pointer_width = "32"))]
use libc::{
    getrlimit64 as get_limits, rlim64_t as rlim_t, rlimit64 as rlimit, setrlimit64 as set_limits,
};

pub(super) fn limit(ctx: &mut Context, value: rlim_t) -> Id {
    // Only the infinity sentinel is interpreted as a signed native integer.
    let integer = if value == INFINITY {
        BigInt::from_signed_bytes_le(&value.to_le_bytes())
    } else {
        value.into()
    };
    ctx.alloc(Value::Int(crate::Int::new(integer)))
}

fn resource_id(ctx: &mut Context, arg: Id) -> Result<i32> {
    let resource = os::c_int(ctx, arg)?;
    // CPython validates against the target ABI's resource count before calling libc.
    #[allow(deprecated)]
    let count = libc::RLIM_NLIMITS as i32;
    if !(0..count).contains(&resource) {
        return Err(Error::new("ValueError", "invalid resource specified"));
    }
    Ok(resource)
}

pub(super) fn getrlimit(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            "resource.getrlimit() takes no keyword arguments",
        ));
    }
    let &[arg] = args else {
        return Err(Error::new(
            "TypeError",
            format!(
                "resource.getrlimit() takes exactly one argument ({} given)",
                args.len()
            ),
        ));
    };
    let resource = resource_id(ctx, arg)?;
    let mut limits = rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: limits is writable and matches the selected libc entry point.
    if unsafe { get_limits(resource as _, &mut limits) } == -1 {
        return Err(os_error(std::io::Error::last_os_error()));
    }
    let current = limit(ctx, limits.rlim_cur);
    let maximum = limit(ctx, limits.rlim_max);
    Ok(ctx.alloc(Value::Tuple(vec![current, maximum])))
}

fn as_limit(ctx: &mut Context, value: Id) -> Result<rlim_t> {
    let integer = ctx.index_integer(value)?;
    let n = integer.big();
    if n.sign() == Sign::Minus {
        // CPython also accepts the signed spelling of the platform infinity sentinel.
        if *n == BigInt::from_signed_bytes_le(&INFINITY.to_le_bytes()) {
            return Ok(INFINITY);
        }
        return Err(Error::new("ValueError", "Cannot convert negative int"));
    }
    rlim_t::try_from(n.as_ref()).map_err(|_| {
        Error::new(
            "OverflowError",
            "Python int too large to convert to C rlim_t",
        )
    })
}

pub(super) fn setrlimit(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            "resource.setrlimit() takes no keyword arguments",
        ));
    }
    let &[arg, limits] = args else {
        return Err(Error::new(
            "TypeError",
            format!("setrlimit expected 2 arguments, got {}", args.len()),
        ));
    };
    let resource = resource_id(ctx, arg)?;
    let number = ctx.int(i64::from(resource));
    audit::event(ctx, "resource.setrlimit", &[number, limits])?;

    // Keep the full snapshot alive across iteration and both __index__ callbacks.
    let iterator = ctx.iterator(limits)?;
    let snapshot = ctx.collect_generator(iterator, true)?;
    let Value::Tuple(items) = ctx.heap.get(snapshot) else {
        unreachable!()
    };
    let &[current, maximum] = items.as_slice() else {
        return Err(Error::new("ValueError", "expected a tuple of 2 integers"));
    };
    let limits = rlimit {
        rlim_cur: as_limit(ctx, current)?,
        rlim_max: as_limit(ctx, maximum)?,
    };
    // SAFETY: limits has the target ABI's layout and is live for the call.
    if unsafe { set_limits(resource as _, &limits) } == -1 {
        let error = std::io::Error::last_os_error();
        return Err(match error.raw_os_error() {
            Some(libc::EINVAL) => Error::new("ValueError", "current limit exceeds maximum limit"),
            Some(libc::EPERM) => Error::new("ValueError", "not allowed to raise maximum limit"),
            _ => os_error(error),
        });
    }
    Ok(NONE)
}
