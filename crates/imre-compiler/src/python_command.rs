//! Parse Python's command-string boundary before handing the body to the native compiler.

use std::env;
use std::ffi::{OsStr, OsString};
use std::path::PathBuf;

use imre_compiler::{CommandFlags, PythonCommand};

pub(super) enum Invocation {
    Command(PythonCommand),
    Module {
        name: String,
        flags: CommandFlags,
        executable: String,
    },
    Script {
        path: PathBuf,
        flags: CommandFlags,
        executable: String,
    },
}

pub(super) enum ParseError {
    Usage(String),
    ContextWarnings(&'static str),
    #[cfg(target_os = "linux")]
    FatalStartup(&'static str),
}

impl From<String> for ParseError {
    fn from(message: String) -> Self {
        Self::Usage(message)
    }
}

impl From<&str> for ParseError {
    fn from(message: &str) -> Self {
        Self::Usage(message.into())
    }
}

enum Input {
    Command(String),
    Module(String),
    Script(PathBuf),
}

/// The command or script ends interpreter option parsing; remaining words belong to the program.
pub(super) fn parse(
    mut arg: OsString,
    args: &mut impl Iterator<Item = OsString>,
) -> Result<Invocation, ParseError> {
    let mut isolated = false;
    let mut ignore_environment = false;
    let mut context_option = None;
    let mut warn_default_encoding = false;
    let mut optimize: i32 = 0;
    let mut dont_write_bytecode = false;
    let mut unbuffered = false;
    let mut utf8 = None;
    let mut warnoptions = Vec::new();
    let mut xoptions = Vec::new();
    let mut faulthandler = false;
    let input = 'options: loop {
        if arg == "--" {
            break Input::Script(args.next().ok_or("expected a script after --")?.into());
        }
        if !arg.as_encoded_bytes().starts_with(b"-") {
            break Input::Script(arg.into());
        }
        let mut flags = arg
            .to_str()
            .and_then(|arg| arg.strip_prefix('-'))
            .ok_or("expected -c and a command string")?
            .chars();
        while let Some(flag) = flags.next() {
            match flag {
                'I' => {
                    isolated = true;
                    ignore_environment = true;
                }
                'E' => ignore_environment = true,
                'O' => optimize = optimize.checked_add(1).ok_or("too many -O options")?,
                'B' => dont_write_bytecode = true,
                'u' => unbuffered = true,
                'c' => break 'options Input::Command(operand(flags.as_str(), args, "-c")?),
                'm' => break 'options Input::Module(operand(flags.as_str(), args, "-m")?),
                'W' => {
                    warnoptions.push(operand(flags.as_str(), args, "-W")?);
                    break;
                }
                'X' => {
                    let option = operand(flags.as_str(), args, "-X")?;
                    match option.split_once('=').unwrap_or((&option, "")) {
                        ("faulthandler", _) => faulthandler = true,
                        ("cpu_count", _) if cfg!(target_os = "linux") => {}
                        ("context_aware_warnings", _) => {
                            context_option.get_or_insert_with(|| option.clone());
                        }
                        ("warn_default_encoding", _) => warn_default_encoding = true,
                        ("utf8", _) => {
                            let value = match option.as_str() {
                                "utf8" | "utf8=1" => true,
                                "utf8=0" => false,
                                _ => return Err(format!("unsupported -X option: {option}").into()),
                            };
                            // CPython's preconfiguration uses the first matching -X option.
                            utf8.get_or_insert(value);
                        }
                        _ => return Err(format!("unsupported -X option: {option}").into()),
                    }
                    xoptions.push(option);
                    break;
                }
                _ => return Err(format!("unknown interpreter option: -{flag}").into()),
            }
        }
        arg = args.next().ok_or("expected -c and a command string")?;
    };
    let utf8_mode = match utf8 {
        Some(value) => Some(value),
        None => match (!ignore_environment)
            .then(|| env::var_os("PYTHONUTF8"))
            .flatten()
        {
            Some(value) if value == "1" => Some(true),
            Some(value) if value == "0" => Some(false),
            Some(value) if !value.is_empty() => return Err("PYTHONUTF8 must be 0 or 1".into()),
            _ => None,
        },
    };
    let env_flag = |key| {
        if ignore_environment {
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
    // A launcher error must preserve the priority of settings validated earlier by CPython.
    let startup_error = |error| {
        #[cfg(target_os = "linux")]
        if !ignore_environment {
            if let Err(message) = imre_ast::startup::environment_hash_seed() {
                return ParseError::FatalStartup(message);
            }
            if let Err(message) = imre_ast::startup::environment_digit_limit() {
                return ParseError::FatalStartup(message);
            }
        }
        error
    };
    // CPython validates the environment even when a command-line option overrides it.
    #[cfg(target_os = "linux")]
    let cpu_count = {
        let invalid =
            || startup_error(ParseError::FatalStartup(imre_ast::startup::CPU_COUNT_ERROR));
        let mut cpu_count = if ignore_environment {
            0
        } else {
            imre_ast::startup::environment_cpu_count().ok_or_else(invalid)?
        };
        if let Some(option) = xoptions
            .iter()
            .find(|s| s.split('=').next() == Some("cpu_count"))
        {
            let value = option.strip_prefix("cpu_count=").ok_or_else(invalid)?;
            cpu_count = if value.is_ascii() {
                imre_ast::startup::parse_cpu_count(OsStr::new(value))
            } else {
                linux_numeric_option(value, ignore_environment)
                    .and_then(|s| s.parse::<i32>().ok())
                    .filter(|n| *n > 0)
            }
            .ok_or_else(invalid)?;
        }
        cpu_count
    };
    #[cfg(not(target_os = "linux"))]
    let cpu_count = 0;
    let mut context_aware_warnings = false;
    if !ignore_environment
        && let Some(value) = env::var_os("PYTHON_CONTEXT_AWARE_WARNINGS").filter(|s| !s.is_empty())
    {
        context_aware_warnings =
            imre_ast::startup::parse_context_flag(&value).ok_or_else(|| {
                startup_error(ParseError::ContextWarnings(
                    "PYTHON_CONTEXT_AWARE_WARNINGS=N: N is missing or invalid",
                ))
            })?;
    }
    if let Some(option) = context_option {
        context_aware_warnings = option
            .strip_prefix("context_aware_warnings=")
            .and_then(|s| context_option_value(s, ignore_environment))
            .ok_or_else(|| {
                startup_error(ParseError::ContextWarnings(
                    "-X context_aware_warnings=n: n is missing or invalid",
                ))
            })?;
    }
    let env_present =
        |key| !ignore_environment && env::var_os(key).is_some_and(|value| !value.is_empty());
    let flags = CommandFlags {
        isolated,
        ignore_environment,
        context_aware_warnings,
        cpu_count,
        optimize: optimize.max(env_flag("PYTHONOPTIMIZE")),
        safe_path: isolated || env_present("PYTHONSAFEPATH"),
        no_user_site: isolated || env_flag("PYTHONNOUSERSITE") != 0,
        dont_write_bytecode: dont_write_bytecode || env_flag("PYTHONDONTWRITEBYTECODE") != 0,
        unbuffered: unbuffered || env_flag("PYTHONUNBUFFERED") != 0,
        utf8_mode,
        warn_default_encoding: warn_default_encoding || env_present("PYTHONWARNDEFAULTENCODING"),
        has_warning_options: !warnoptions.is_empty() || env_present("PYTHONWARNINGS"),
        warnoptions,
        xoptions,
        faulthandler: faulthandler || env_present("PYTHONFAULTHANDLER"),
    };
    let executable = env::current_exe()
        .map_err(|e| e.to_string())?
        .into_os_string()
        .into_string()
        .map_err(|_| "interpreter path is not UTF-8")?;
    Ok(match input {
        Input::Command(source) => Invocation::Command(PythonCommand {
            source: dedent(source),
            flags,
            executable,
        }),
        Input::Module(name) => Invocation::Module {
            name,
            flags,
            executable,
        },
        Input::Script(path) => Invocation::Script {
            path,
            flags,
            executable,
        },
    })
}

/// CPython parses -X operands as wide characters, unlike its byte environment.
fn context_option_value(value: &str, ignore_environment: bool) -> Option<bool> {
    #[cfg(target_os = "linux")]
    if !value.is_ascii() {
        return linux_numeric_option(value, ignore_environment)
            .and_then(|s| imre_ast::startup::parse_context_flag(OsStr::new(s)));
    }
    #[cfg(not(target_os = "linux"))]
    let _ = ignore_environment;
    imre_ast::startup::parse_context_flag(OsStr::new(value))
}

#[cfg(target_os = "linux")]
fn linux_numeric_option(value: &str, ignore_environment: bool) -> Option<&str> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    unsafe extern "C" {
        fn iswspace_l(wc: libc::c_uint, locale: libc::locale_t) -> libc::c_int;
    }

    // Read LC_CTYPE without changing the process or another compiler thread's locale.
    let name = ["LC_ALL", "LC_CTYPE", "LANG"]
        .into_iter()
        .find_map(|key| env::var_os(key).filter(|v| !v.is_empty()))
        .unwrap_or_else(|| "C".into());
    let cname = CString::new(name.as_bytes()).ok()?;
    let open = |name: &std::ffi::CStr| {
        // SAFETY: the name is NUL terminated and a null base creates a new locale.
        unsafe { libc::newlocale(libc::LC_CTYPE_MASK, name.as_ptr(), std::ptr::null_mut()) }
    };
    let mut locale = open(&cname);
    if (locale.is_null() || name == "C" || name == "POSIX")
        && env::var_os("LC_ALL").is_none_or(|v| v.is_empty())
        && (ignore_environment
            || env::var_os("PYTHONCOERCECLOCALE").as_deref() != Some(OsStr::new("0")))
        && let Some(coerced) = [c"C.UTF-8", c"C.utf8", c"UTF-8"]
            .into_iter()
            .map(open)
            .find(|loc| !loc.is_null())
    {
        if !locale.is_null() {
            // SAFETY: open returned this owned locale, which is no longer needed.
            unsafe { libc::freelocale(locale) };
        }
        locale = coerced;
    }
    if locale.is_null() {
        return None;
    }
    // SAFETY: Linux wint_t represents a Unicode scalar and the locale stays live throughout.
    let rest = value.trim_start_matches(|c: char| unsafe { iswspace_l(c as u32, locale) != 0 });
    // SAFETY: this call owns the non-null locale, and the trim above has finished using it.
    unsafe { libc::freelocale(locale) };
    (!rest.is_empty()).then_some(rest)
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
