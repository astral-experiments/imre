//! Parse Python's command-string boundary before handing the body to the native compiler.

use std::env;
use std::ffi::OsString;

use imre_compiler::{CommandFlags, PythonCommand};

/// The command operand ends interpreter option parsing; every remaining word belongs to the program.
pub(super) fn parse(
    mut arg: OsString,
    args: &mut impl Iterator<Item = OsString>,
) -> Result<PythonCommand, String> {
    let mut isolated = false;
    let mut dont_write_bytecode = false;
    let mut utf8 = None;
    let source = 'options: loop {
        let mut flags = arg
            .to_str()
            .and_then(|arg| arg.strip_prefix('-'))
            .ok_or("expected -c and a command string")?
            .chars();
        while let Some(flag) = flags.next() {
            match flag {
                'I' => isolated = true,
                'B' => dont_write_bytecode = true,
                'c' => break 'options operand(flags.as_str(), args, "-c")?,
                'X' => {
                    let option = operand(flags.as_str(), args, "-X")?;
                    utf8 = Some(match option.as_str() {
                        "utf8" | "utf8=1" => true,
                        "utf8=0" => false,
                        _ => return Err(format!("unsupported -X option: {option}")),
                    });
                    break;
                }
                _ => return Err(format!("unknown interpreter option: -{flag}")),
            }
        }
        arg = args.next().ok_or("expected -c and a command string")?;
    };
    let utf8_mode = match utf8 {
        Some(value) => value,
        None => match (!isolated).then(|| env::var_os("PYTHONUTF8")).flatten() {
            Some(value) if value == "1" => true,
            Some(value) if value == "0" => false,
            Some(value) if !value.is_empty() => return Err("PYTHONUTF8 must be 0 or 1".into()),
            _ => ["LC_ALL", "LC_CTYPE", "LANG"]
                .iter()
                .filter_map(env::var_os)
                .find(|value| !value.is_empty())
                .is_none_or(|locale| locale == "C" || locale == "POSIX"),
        },
    };
    let enabled = |key| {
        !isolated
            && env::var_os(key).is_some_and(|value| {
                !value.is_empty()
                    && value
                        .to_str()
                        .and_then(|s| s.trim_start().parse::<i32>().ok())
                        != Some(0)
            })
    };
    let flags = CommandFlags {
        isolated,
        safe_path: isolated || env::var_os("PYTHONSAFEPATH").is_some_and(|s| !s.is_empty()),
        no_user_site: isolated || enabled("PYTHONNOUSERSITE"),
        dont_write_bytecode: dont_write_bytecode || enabled("PYTHONDONTWRITEBYTECODE"),
        utf8_mode,
    };
    let executable = env::current_exe()
        .map_err(|e| e.to_string())?
        .into_os_string()
        .into_string()
        .map_err(|_| "interpreter path is not UTF-8")?;
    Ok(PythonCommand {
        source,
        flags,
        executable,
    })
}

fn operand(
    rest: &str,
    args: &mut impl Iterator<Item = OsString>,
    option: &str,
) -> Result<String, String> {
    if !rest.is_empty() {
        return Ok(rest.to_owned());
    }
    args.next()
        .ok_or_else(|| format!("{option} requires an argument"))?
        .into_string()
        .map_err(|_| format!("{option} requires a UTF-8 argument"))
}
