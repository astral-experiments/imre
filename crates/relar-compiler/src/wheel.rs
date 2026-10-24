use std::ffi::OsString;
use std::fs;
use std::process::{Command, ExitCode};

use super::{PYTHON_HEADER, program_exit_code};

/// Run the setuptools target bundled with this executable under the build host's Python.
pub(super) fn run(mut args: impl Iterator<Item = OsString>) -> Result<ExitCode, String> {
    let mut python = OsString::from("python3");
    let mut options = Vec::new();
    while let Some(arg) = args.next() {
        if arg == "--python" {
            python = args.next().ok_or("--python requires an executable")?;
        } else if let Some(value) = arg.to_str().and_then(|arg| arg.strip_prefix("--python=")) {
            python = value.into();
        } else {
            options.push(arg);
        }
    }
    if python.is_empty() {
        return Err("--python requires an executable".into());
    }
    let temp = tempfile::tempdir().map_err(|e| e.to_string())?;
    let dir = temp.path();
    for (name, contents) in [
        ("Python.h", PYTHON_HEADER),
        ("frontend.py", include_str!("wheel/frontend.py")),
        (
            "setuptools_target.py",
            include_str!("wheel/setuptools_target.py"),
        ),
    ] {
        fs::write(dir.join(name), contents).map_err(|e| e.to_string())?;
    }
    Command::new(&python)
        .arg(dir.join("frontend.py"))
        .args(options)
        .status()
        .map(program_exit_code)
        .map_err(|e| {
            format!(
                "Could not run {} for the wheel build: {e}",
                python.display()
            )
        })
}
