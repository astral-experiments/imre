//! Python argument conversion for the optional OpenSSL key derivation functions.
use num_traits::{Signed, ToPrimitive};

use super::{Context, Error, Id, NONE, Result, Value, builtins, openssl};

fn input_bytes(ctx: &Context, id: Id) -> Result<&[u8]> {
    ctx.native_buffer_bytes(id)
}

/// Integer conversions may mutate either buffer or try to release a memoryview.
fn with_inputs<T>(
    ctx: &mut Context,
    password: Id,
    salt: Id,
    derive: impl FnOnce(&mut Context) -> Result<T>,
) -> Result<T> {
    ctx.check_buffer_exporter(password)?;
    input_bytes(ctx, password)?;
    ctx.with_buffer_export(password, |ctx| {
        ctx.check_buffer_exporter(salt)?;
        input_bytes(ctx, salt)?;
        ctx.with_buffer_export(salt, derive)
    })
}

fn check_lengths(ctx: &Context, password: Id, salt: Id) -> Result<()> {
    for (name, id) in [("password", password), ("salt", salt)] {
        if input_bytes(ctx, id)?.len() > i32::MAX as usize {
            return Err(Error::new("OverflowError", format!("{name} is too long.")));
        }
    }
    Ok(())
}

fn positive_int(value: i64, label: &str) -> Result<i32> {
    if value < 1 {
        return Err(Error::new(
            "ValueError",
            format!("{label} must be greater than 0."),
        ));
    }
    i32::try_from(value).map_err(|_| Error::new("OverflowError", format!("{label} is too great.")))
}

pub(in crate::dynamic) fn pbkdf2_hmac(
    ctx: &mut Context,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let [
        Some(name),
        Some(password),
        Some(salt),
        Some(iterations),
        dklen,
    ] = builtins::bind_builtin_arguments(
        "pbkdf2_hmac",
        args,
        kwargs,
        ["hash_name", "password", "salt", "iterations", "dklen"],
        4,
    )?
    else {
        unreachable!("bound required KDF arguments")
    };

    let name = ctx.string_storage(name).ok_or_else(|| {
        let kind = if name == NONE {
            "None"
        } else {
            ctx.type_name(name)
        };
        Error::new(
            "TypeError",
            format!("pbkdf2_hmac() argument 'hash_name' must be str, not {kind}"),
        )
    })?;
    if matches!(ctx.heap.get(name), Value::SurrogateStr(_)) {
        ctx.encode_utf8_codepoints(name, false, "strict")?;
    }
    if ctx.str_value(name)?.contains('\0') {
        return Err(Error::new("ValueError", "embedded null character"));
    }

    let key = with_inputs(ctx, password, salt, |ctx| {
        let iterations = ctx.index_c_long(iterations)?;
        let provider = openssl::required()?;
        let digest = provider.digest(ctx.str_value(name)?)?;
        check_lengths(ctx, password, salt)?;
        let iterations = positive_int(iterations, "iteration value")?;
        let length = match dklen {
            None | Some(NONE) => i64::from(digest.size()),
            Some(length) => ctx.index_c_long(length)?,
        };
        let length = positive_int(length, "key length")?;
        digest.pbkdf2(
            input_bytes(ctx, password)?,
            input_bytes(ctx, salt)?,
            iterations,
            length,
        )
    })?;
    Ok(ctx.alloc(Value::Bytes(key)))
}

fn unsigned_long(ctx: &mut Context, id: Id) -> Result<u64> {
    let value = ctx.index_integer(id)?;
    let number = value.big();
    if number.is_negative() {
        return Err(Error::new("ValueError", "Cannot convert negative int"));
    }
    number
        .to_u64()
        .ok_or_else(|| Error::new("OverflowError", "Python int too large for C unsigned long"))
}

pub(in crate::dynamic) fn scrypt(
    ctx: &mut Context,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    if args.len() > 1 && args.len() + kwargs.len() <= 7 {
        return Err(Error::new(
            "TypeError",
            format!(
                "scrypt() takes exactly 1 positional argument ({} given)",
                args.len()
            ),
        ));
    }
    let [
        Some(password),
        Some(salt),
        Some(n),
        Some(r),
        Some(p),
        maxmem,
        dklen,
    ] = builtins::bind_builtin_arguments(
        "scrypt",
        args,
        kwargs,
        ["password", "salt", "n", "r", "p", "maxmem", "dklen"],
        5,
    )?
    else {
        unreachable!("bound required KDF arguments")
    };

    let key = with_inputs(ctx, password, salt, |ctx| {
        let n = unsigned_long(ctx, n)?;
        let r = unsigned_long(ctx, r)?;
        let p = unsigned_long(ctx, p)?;
        let maxmem = maxmem
            .map(|id| ctx.index_c_long(id))
            .transpose()?
            .unwrap_or(0);
        let dklen = dklen
            .map(|id| ctx.index_c_long(id))
            .transpose()?
            .unwrap_or(64);
        check_lengths(ctx, password, salt)?;
        if n < 2 || !n.is_power_of_two() {
            return Err(Error::new("ValueError", "n must be a power of 2."));
        }
        if !(0..=i64::from(i32::MAX)).contains(&maxmem) {
            return Err(Error::new(
                "ValueError",
                format!("maxmem must be positive and smaller than {}", i32::MAX),
            ));
        }
        if !(1..=i64::from(i32::MAX)).contains(&dklen) {
            return Err(Error::new(
                "ValueError",
                format!("dklen must be greater than 0 and smaller than {}", i32::MAX),
            ));
        }
        let provider = openssl::required()?;
        provider.scrypt(
            input_bytes(ctx, password)?,
            input_bytes(ctx, salt)?,
            n,
            r,
            p,
            maxmem,
            dklen,
        )
    })?;
    Ok(ctx.alloc(Value::Bytes(key)))
}
