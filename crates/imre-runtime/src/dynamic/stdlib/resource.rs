//! Linux process resource limits.
use super::{Context, Error, Id, Result, Value, os, os_error};

// CPython enables _FILE_OFFSET_BITS=64, including on 32-bit glibc.
#[cfg(not(all(target_env = "gnu", target_pointer_width = "32")))]
pub(super) use libc::RLIM_INFINITY as INFINITY;
#[cfg(all(target_env = "gnu", target_pointer_width = "32"))]
pub(super) use libc::RLIM64_INFINITY as INFINITY;
#[cfg(not(all(target_env = "gnu", target_pointer_width = "32")))]
use libc::{getrlimit as get_limits, rlim_t, rlimit};
#[cfg(all(target_env = "gnu", target_pointer_width = "32"))]
use libc::{getrlimit64 as get_limits, rlim64_t as rlim_t, rlimit64 as rlimit};

pub(super) fn limit(ctx: &mut Context, value: rlim_t) -> Id {
    // Only the infinity sentinel is interpreted as a signed native integer.
    let integer = if value == INFINITY {
        num_bigint::BigInt::from_signed_bytes_le(&value.to_le_bytes())
    } else {
        value.into()
    };
    ctx.alloc(Value::Int(crate::Int::new(integer)))
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
    let resource = os::c_int(ctx, arg)?;
    // CPython validates against the target ABI's resource count before calling libc.
    #[allow(deprecated)]
    let count = libc::RLIM_NLIMITS as i32;
    if !(0..count).contains(&resource) {
        return Err(Error::new("ValueError", "invalid resource specified"));
    }
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
