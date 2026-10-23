//! Independent native live reporter; report computation stays in this crate.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use relar_benchmark_platform as platform;

use crate::model::WireDiagnostic;
use crate::parse::{Inputs, SourceText};
use crate::{Error, Options, Result};

impl From<platform::Error> for Error {
    fn from(error: platform::Error) -> Self {
        Self {
            kind: error.kind,
            message: error.message,
        }
    }
}

struct LiveOptions {
    suite: String,
    old_ty: String,
    new_ty: String,
    output: String,
    revision: String,
    format: String,
    force_summary_table: bool,
    timeout: f64,
}

fn options(arguments: &[String]) -> Result<LiveOptions> {
    let mut result = LiveOptions {
        suite: String::new(),
        old_ty: String::new(),
        new_ty: String::new(),
        output: String::new(),
        revision: "835b70f3f795f8e061b1f7621178515b0a77a0ad".into(),
        format: "github".into(),
        force_summary_table: false,
        timeout: 15.0,
    };
    let mut index = 1;
    while index < arguments.len() {
        let flag = &arguments[index];
        if flag == "--force-summary-table" {
            result.force_summary_table = true;
            index += 1;
            continue;
        }
        let value = arguments
            .get(index + 1)
            .ok_or_else(|| Error::value(format!("expected a value after {flag}")))?;
        match flag.as_str() {
            "--suite" => result.suite.clone_from(value),
            "--old-ty" => result.old_ty.clone_from(value),
            "--new-ty" => result.new_ty.clone_from(value),
            "--output" => result.output.clone_from(value),
            "--suite-revision" => result.revision.clone_from(value),
            "--format" => result.format.clone_from(value),
            "--timeout" => {
                result.timeout = value.parse().map_err(|_| Error::value("invalid timeout"))?
            }
            _ => return Err(Error::value(format!("unknown argument: {flag}"))),
        }
        index += 2;
    }
    if [
        &result.suite,
        &result.old_ty,
        &result.new_ty,
        &result.output,
    ]
    .iter()
    .any(|value| value.is_empty())
    {
        return Err(Error::value(
            "expected --suite CHECKOUT/conformance --old-ty BIN --new-ty BIN --output REPORT",
        ));
    }
    if !matches!(result.format.as_str(), "github" | "diff") {
        return Err(Error::value("format must be github or diff"));
    }
    if !result.timeout.is_finite() || result.timeout <= 0.0 {
        return Err(Error::value("timeout must be finite and positive"));
    }
    Ok(result)
}

struct Workspace(PathBuf);

impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = platform::remove_directory(&self.0);
    }
}

fn discover(suite: &Path) -> Result<Vec<SourceText>> {
    let groups_file = suite.join("src/test_groups.toml");
    let groups = platform::test_groups(&crate::read_text(&groups_file.to_string_lossy())?)?;
    let directory = suite.join("tests");
    let mut sources = Vec::new();
    // Keep upstream's .py-then-.pyi order and each directory traversal's order.
    for extension in ["py", "pyi"] {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = path.file_name().unwrap().to_string_lossy();
            if path.extension().is_none_or(|value| value != extension)
                || !groups.contains_key(name.split('_').next().unwrap())
            {
                continue;
            }
            let path = path.canonicalize().map_err(Error::io)?;
            let path = path.to_string_lossy();
            sources.push(SourceText {
                path: Arc::from(path.as_ref()),
                text: crate::read_text(&path)?,
            });
        }
    }
    if sources.is_empty() {
        return Err(Error::value("no conformance test files discovered"));
    }
    Ok(sources)
}

fn arguments(suite: &Path, sources: &[SourceText]) -> Vec<String> {
    let mut result: Vec<_> = [
        "check",
        "--python-version=3.12",
        "--output-format=gitlab",
        "--ignore=assert-type-unspellable-subtype",
        "--error=ambiguous-protocol-member",
        "--error=invalid-enum-member-annotation",
        "--error=invalid-legacy-positional-parameter",
        "--error=mismatched-type-name",
        "--error=invalid-named-tuple-override",
        "--error=deprecated",
        "--error=redundant-final-classvar",
        "--exit-zero",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    result.push(format!(
        "--extra-search-path={}",
        suite.join("tests").display()
    ));
    result.extend(sources.iter().map(|source| source.path.to_string()));
    result
}

fn analyzer(
    binary: &Path,
    arguments: &[String],
    suite: &Path,
    environment: &BTreeMap<String, String>,
    timeout: f64,
) -> Result<String> {
    let argv = std::iter::once(binary.to_string_lossy().into_owned())
        .chain(arguments.iter().cloned())
        .collect();
    Ok(platform::run(platform::ProcessRequest {
        argv,
        cwd: suite.to_string_lossy().into_owned(),
        env: environment.clone(),
        timeout,
        check: true,
    })?
    .stdout)
}

fn paths(text: &str, suite: &Path, result: &mut HashMap<String, Arc<str>>) -> Result<()> {
    let diagnostics: Vec<WireDiagnostic> =
        serde_json::from_str(text).map_err(|error| Error::value(error.to_string()))?;
    for diagnostic in diagnostics {
        let raw = diagnostic.location.path;
        let path = Path::new(&raw);
        let path = if path.is_absolute() {
            path.to_owned()
        } else {
            suite.join(path)
        };
        result.insert(
            raw.to_string(),
            Arc::from(platform::resolve(&path)?.to_string_lossy().as_ref()),
        );
    }
    Ok(())
}

/// Run discovery, native children, report computation, and atomic publication.
pub fn run(arguments: &[String]) -> Result<()> {
    let options = options(arguments)?;
    let suite = Path::new(&options.suite)
        .canonicalize()
        .map_err(Error::io)?;
    let old_ty = Path::new(&options.old_ty)
        .canonicalize()
        .map_err(Error::io)?;
    let new_ty = Path::new(&options.new_ty)
        .canonicalize()
        .map_err(Error::io)?;
    let output = Path::new(&options.output);
    let parent = output
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let workspace = Workspace(platform::temporary_directory(
        parent,
        ".relar-conformance-",
    )?);
    let sources = discover(&suite)?;
    let expected = crate::parse::collect_expected(&sources)?;
    let arguments = self::arguments(&suite, &sources);
    let environment = std::env::vars().collect();
    let mut aliases = HashMap::new();
    let old = analyzer(&old_ty, &arguments, &suite, &environment, options.timeout)?;
    paths(&old, &suite, &mut aliases)?;
    let new = analyzer(&new_ty, &arguments, &suite, &environment, options.timeout)?;
    paths(&new, &suite, &mut aliases)?;
    let inputs = Inputs {
        revision: options.revision,
        paths: aliases,
        sources,
        old,
        new,
    };
    let report_options = Options {
        manifest: String::new(),
        old: String::new(),
        new: String::new(),
        format: options.format,
        force_summary_table: options.force_summary_table,
        output: None,
        compute: false,
        rounds: 1,
    };
    let report = crate::render_diagnostics(&inputs, &report_options, expected)?;
    let staged = workspace.0.join("report.md");
    crate::write_text(&staged.to_string_lossy(), &report)?;
    std::fs::rename(staged, output).map_err(Error::io)?;
    platform::remove_directory(&workspace.0)?;
    std::mem::forget(workspace);
    Ok(())
}
