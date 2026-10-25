use std::ffi::OsString;
use std::fs;
use std::path::Path;
use std::process::{Command, ExitCode};

use imre_compiler::{NATIVE_ARCHIVE_EXTENSION, PYTHON_VERSION};
use serde_json::json;

use super::{PYTHON_HEADER, program_exit_code};

const TARGET_USAGE: &str = "Usage: imre build-target [--output-dir DIRECTORY]

Print the native wheel target as JSON. With --output-dir, also write target.json,
Python.h, and the build adapters to the specified directory.";

/// Report the target, optionally exporting the build files embedded in this compiler.
pub(super) fn report(mut args: impl Iterator<Item = OsString>) -> Result<ExitCode, String> {
    let directory = match args.next().as_deref() {
        None => None,
        Some(arg) if arg == "--help" || arg == "-h" => {
            println!("{TARGET_USAGE}");
            return Ok(ExitCode::SUCCESS);
        }
        Some(arg) if arg == "--output-dir" => Some(
            args.next()
                .filter(|path| !path.is_empty())
                .ok_or(TARGET_USAGE)?,
        ),
        _ => return Err(TARGET_USAGE.into()),
    };
    if args.next().is_some() {
        return Err(TARGET_USAGE.into());
    }
    print!("{}", build_target(directory.as_deref().map(Path::new))?);
    Ok(ExitCode::SUCCESS)
}

/// Describe the archive that this compiler can link on its native platform.
fn build_target(directory: Option<&Path>) -> Result<String, String> {
    let platform = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", arch @ ("x86_64" | "aarch64")) => format!("linux_{arch}"),
        ("macos", "aarch64") => {
            // Use the build host as a conservative deployment target for native archives.
            let version = Command::new("/usr/bin/sw_vers")
                .env("SYSTEM_VERSION_COMPAT", "0")
                .arg("-productVersion")
                .output()
                .map_err(|e| format!("could not determine macOS version: {e}"))?;
            let major = std::str::from_utf8(&version.stdout)
                .ok()
                .and_then(|s| s.trim().split('.').next()?.parse::<u16>().ok())
                .filter(|&n| version.status.success() && n >= 11)
                .ok_or("could not determine Apple Silicon macOS wheel platform")?;
            format!("macosx_{major}_0_arm64")
        }
        (os, arch) => return Err(format!("unsupported native wheel platform: {os}/{arch}")),
    };
    let implementation = "imre";
    let python_tag = format!(
        "{implementation}{}{}",
        PYTHON_VERSION.major, PYTHON_VERSION.minor
    );
    let abi_tag = "imre1";
    let mut target = json!({
        "implementation": implementation,
        "python_version": PYTHON_VERSION.to_string(),
        "python_tag": python_tag,
        "abi_tag": abi_tag,
        "platform_tag": platform,
        "wheel_tag": format!("{python_tag}-{abi_tag}-{platform}"),
        "link_mode": "static",
        "archive_suffix": format!(".{NATIVE_ARCHIVE_EXTENSION}"),
    });
    if let Some(dir) = directory {
        for (file, contents) in [
            ("include/Python.h", PYTHON_HEADER),
            (
                "setuptools/pyproject.toml",
                include_str!("wheel/setuptools/pyproject.toml"),
            ),
            (
                "setuptools/imre_setuptools.py",
                include_str!("wheel/setuptools/imre_setuptools.py"),
            ),
            (
                "setuptools/LICENSE-MIT",
                include_str!("../../../LICENSE-MIT"),
            ),
            (
                "setuptools/LICENSE-APACHE",
                include_str!("../../../LICENSE-APACHE"),
            ),
            (
                "scikit/FindPython.cmake",
                include_str!("wheel/scikit/FindPython.cmake"),
            ),
            (
                "scikit/Imre.cmake",
                include_str!("wheel/scikit/Imre.cmake"),
            ),
        ] {
            let path = dir.join(file);
            fs::create_dir_all(path.parent().unwrap())
                .and_then(|()| fs::write(&path, contents))
                .map_err(|e| format!("could not export {}: {e}", path.display()))?;
        }
        let dir = dir.canonicalize().map_err(|e| e.to_string())?;
        for (key, path) in [
            ("include_dir", "include"),
            ("setuptools_adapter", "setuptools"),
            ("cmake_project_include", "scikit/Imre.cmake"),
        ] {
            target[key] = serde_json::to_value(dir.join(path)).map_err(|e| e.to_string())?;
        }
    }
    let json = format!("{target:#}\n");
    if let Some(dir) = directory {
        fs::write(dir.join("target.json"), &json)
            .map_err(|e| format!("could not write {}/target.json: {e}", dir.display()))?;
    }
    Ok(json)
}

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
    build_target(Some(dir))?;
    fs::write(dir.join("frontend.py"), include_str!("wheel/frontend.py"))
        .map_err(|e| e.to_string())?;
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
