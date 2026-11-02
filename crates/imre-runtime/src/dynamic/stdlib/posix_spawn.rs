//! Spawn a waitable Linux child with the caller's ordered POSIX file actions.
use super::{Context, Error, Id, NONE, Result, audit, builtins, os, posixpath};
use crate::objects::CStrings;

mod actions;
mod arguments;
mod attributes;

/// The os facade copies the current posix namespace, including any reassigned exports.
pub(in crate::dynamic) fn initialize(
    ctx: &mut Context,
    scope: Id,
    posix: Option<Id>,
) -> Result<()> {
    for (name, number) in [
        ("posix_spawn", None),
        ("posix_spawnp", None),
        ("POSIX_SPAWN_OPEN", Some(0)),
        ("POSIX_SPAWN_CLOSE", Some(1)),
        ("POSIX_SPAWN_DUP2", Some(2)),
        ("POSIX_SPAWN_CLOSEFROM", Some(3)),
    ] {
        let value = match posix {
            Some(module) => match ctx.lookup_attr(module, name) {
                Ok(value) => value,
                Err(error) if ctx.is_attribute_error(&error) => continue,
                Err(error) => return Err(error),
            },
            None if name == "POSIX_SPAWN_CLOSEFROM" && !actions::has_closefrom() => continue,
            None => match number {
                Some(code) => ctx.int(code),
                None => ctx.builtin(&format!("posix.{name}")),
            },
        };
        ctx.bind(scope, name, value);
    }
    Ok(())
}

pub(super) fn call(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let &[path, argv, env] = args else {
        return Err(Error::new(
            "TypeError",
            format!(
                "{name}() takes exactly 3 positional arguments ({} given)",
                args.len()
            ),
        ));
    };
    let [
        file_actions,
        pgroup,
        resetids,
        setsid,
        sigmask,
        sigdef,
        scheduler,
    ] = builtins::bind_method_arguments(
        name,
        &[],
        kwargs,
        [
            "file_actions",
            "setpgroup",
            "resetids",
            "setsid",
            "setsigmask",
            "setsigdef",
            "scheduler",
        ],
        0,
    )?;
    let path = posixpath::path_arg(
        ctx,
        path,
        &format!("{name}: path should be string, bytes or os.PathLike"),
    )?;
    ctx.heap.keep(path);
    let executable = os::c_path(
        ctx,
        path,
        &format!("{name}: embedded null character in path"),
    )?;
    let resetids = resetids.map(|v| ctx.truth(v)).transpose()?.unwrap_or(false);
    let setsid = setsid.map(|v| ctx.truth(v)).transpose()?.unwrap_or(false);
    let (argv_strings, env_strings) = arguments::arguments(ctx, name, argv, env, scheduler)?;
    let argv_strings = CStrings::new(argv_strings)?;
    let env_strings = env_strings.map(CStrings::new).transpose()?;
    let actions = actions::FileActions::parse(ctx, file_actions.unwrap_or(NONE))?;
    let attributes = attributes::Attributes::new(
        ctx,
        pgroup.unwrap_or(NONE),
        resetids,
        setsid,
        sigmask,
        sigdef,
        scheduler.unwrap_or(NONE),
    )?;
    audit::event(ctx, "os.posix_spawn", &[path, argv, env])?;
    match argv_strings.spawn(
        &executable,
        name == "posix_spawnp",
        env_strings.as_ref(),
        actions.as_raw(),
        attributes.as_raw(),
    ) {
        Ok(pid) => Ok(ctx.int(i64::from(pid))),
        Err(err) => Err(ctx.with_errno_filename(crate::objects::errno_error(err).into(), path)?),
    }
}
