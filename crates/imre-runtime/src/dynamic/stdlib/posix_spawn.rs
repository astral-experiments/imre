//! Spawn a waitable Linux child with the caller's ordered POSIX file actions.
use super::{Context, Error, Id, NONE, Result, audit, builtins, os, posixpath};
use crate::objects::CStrings;

mod actions;
mod arguments;
mod attributes;

pub(in crate::dynamic) fn initialize(ctx: &mut Context, scope: Id) {
    for name in ["posix_spawn", "posix_spawnp"] {
        let value = ctx.builtin(&format!("posix.{name}"));
        ctx.bind(scope, name, value);
    }
    for (name, value) in [
        ("POSIX_SPAWN_OPEN", 0),
        ("POSIX_SPAWN_CLOSE", 1),
        ("POSIX_SPAWN_DUP2", 2),
    ] {
        let value = ctx.int(value);
        ctx.bind(scope, name, value);
    }
    if actions::has_closefrom() {
        let value = ctx.int(3);
        ctx.bind(scope, "POSIX_SPAWN_CLOSEFROM", value);
    }
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
