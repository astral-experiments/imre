//! Linux device-number conversions used by os.major, os.minor and os.makedev.
use num_traits::{Signed, ToPrimitive};

use super::{Context, Error, Id, Result, stat};

// glibc defines NODEV as (dev_t)-1. musl has no NODEV sentinel.
const HAS_NODEV: bool = cfg!(target_env = "gnu");

/// Convert an indexed argument to dev_t before validating either makedev component.
fn device_number(ctx: &mut Context, value: Id) -> Result<libc::dev_t> {
    let index = ctx.index_integer(value)?;
    if HAS_NODEV && index.to_i64() == Some(-1) {
        return Ok(libc::dev_t::MAX);
    }
    let number = index.big();
    if number.is_negative() {
        return Err(Error::new(
            "OverflowError",
            "can't convert negative int to unsigned",
        ));
    }
    number
        .to_u64()
        .ok_or_else(|| Error::new("OverflowError", "int too big to convert"))
}

fn component(value: libc::dev_t) -> Result<libc::c_uint> {
    if HAS_NODEV && value == libc::dev_t::MAX {
        return Ok(libc::c_uint::MAX);
    }
    libc::c_uint::try_from(value).map_err(|_| {
        Error::new(
            "OverflowError",
            "Python int too large to convert to C unsigned int",
        )
    })
}

pub(in crate::dynamic) fn call(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    if !kwargs.is_empty() {
        return Err(Error::new(
            "TypeError",
            format!("posix.{name}() takes no keyword arguments"),
        ));
    }
    if name == "makedev" {
        let [major, minor] = args else {
            return Err(Error::new(
                "TypeError",
                format!("makedev expected 2 arguments, got {}", args.len()),
            ));
        };
        let major = device_number(ctx, *major)?;
        let minor = device_number(ctx, *minor)?;
        let device = libc::makedev(component(major)?, component(minor)?);
        return Ok(if HAS_NODEV && device == libc::dev_t::MAX {
            ctx.int(-1)
        } else {
            stat::integer(ctx, device)
        });
    }
    let [device] = args else {
        return Err(Error::new(
            "TypeError",
            format!(
                "posix.{name}() takes exactly one argument ({} given)",
                args.len()
            ),
        ));
    };
    let device = device_number(ctx, *device)?;
    let part = if name == "major" {
        libc::major(device)
    } else {
        libc::minor(device)
    };
    Ok(ctx.int(if HAS_NODEV && part == libc::c_uint::MAX {
        -1
    } else {
        i64::from(part)
    }))
}
