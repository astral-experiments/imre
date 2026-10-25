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
    let mut optimize: i32 = 0;
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
                'O' => optimize = optimize.checked_add(1).ok_or("too many -O options")?,
                'B' => dont_write_bytecode = true,
                'c' => break 'options operand(flags.as_str(), args, "-c")?,
                'X' => {
                    let option = operand(flags.as_str(), args, "-X")?;
                    let value = match option.as_str() {
                        "utf8" | "utf8=1" => true,
                        "utf8=0" => false,
                        _ => return Err(format!("unsupported -X option: {option}")),
                    };
                    // CPython's preconfiguration uses the first matching -X option.
                    utf8.get_or_insert(value);
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
    let env_flag = |key| {
        if isolated {
            return 0;
        }
        env::var_os(key)
            .filter(|v| !v.is_empty())
            .map_or(0, |value| {
                // CPython treats a nonempty value that is not a nonnegative C int as 1.
                value
                    .to_str()
                    .and_then(|s| {
                        s.trim_start_matches([' ', '\t', '\n', '\x0b', '\x0c', '\r'])
                            .parse::<i32>()
                            .ok()
                    })
                    .filter(|&v| v >= 0)
                    .unwrap_or(1)
            })
    };
    let flags = CommandFlags {
        isolated,
        optimize: optimize.max(env_flag("PYTHONOPTIMIZE")),
        safe_path: isolated || env::var_os("PYTHONSAFEPATH").is_some_and(|s| !s.is_empty()),
        no_user_site: isolated || env_flag("PYTHONNOUSERSITE") != 0,
        dont_write_bytecode: dont_write_bytecode || env_flag("PYTHONDONTWRITEBYTECODE") != 0,
        utf8_mode,
    };
    let executable = env::current_exe()
        .map_err(|e| e.to_string())?
        .into_os_string()
        .into_string()
        .map_err(|_| "interpreter path is not UTF-8")?;
    Ok(PythonCommand {
        source: dedent(source),
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

/// Python 3.14 removes a shared literal space/tab prefix before compiling `-c`.
/// Lines that contain only those characters do not determine the prefix.
fn dedent(source: String) -> String {
    let mut margin: Option<&str> = None;
    for line in source.split('\n') {
        let body = line.trim_start_matches([' ', '\t']);
        if body.is_empty() {
            continue;
        }
        let indent = &line[..line.len() - body.len()];
        let shared = margin.map_or(indent.len(), |previous| {
            previous
                .bytes()
                .zip(indent.bytes())
                .take_while(|(a, b)| a == b)
                .count()
        });
        if shared == 0 {
            return source;
        }
        margin = Some(&indent[..shared]);
    }
    let Some(margin) = margin else { return source };
    let mut result = String::new();
    for (i, line) in source.split('\n').enumerate() {
        if i != 0 {
            result.push('\n');
        }
        if !line.trim_start_matches([' ', '\t']).is_empty() {
            result.push_str(&line[margin.len()..]);
        }
    }
    result
}
