pub(crate) mod scanner_benchmark;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use serde::Deserialize;
use serde::de::DeserializeOwned;

use crate::model::{Diagnostic, ExpectedError, Integer, WireDiagnostic, filename};
use crate::{Error, Result, read_text};

#[derive(Deserialize)]
struct PathAlias {
    raw: String,
    path: String,
}

#[derive(Deserialize)]
struct Manifest {
    suite_revision: String,
    test_files: Vec<String>,
    diagnostic_paths: Vec<PathAlias>,
}

pub struct SourceText {
    pub path: Arc<str>,
    pub text: String,
}

pub struct Inputs {
    pub revision: String,
    pub paths: HashMap<String, Arc<str>>,
    pub sources: Vec<SourceText>,
    pub old: String,
    pub new: String,
}

/// Apply the same object/array depth bound as Relar's typed JSON decoder.
fn check_depth(text: &str) -> Result<()> {
    let mut quoted = false;
    let mut escaped = false;
    let mut depth = 0usize;
    for byte in text.bytes() {
        if quoted {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                quoted = false;
            }
        } else {
            match byte {
                b'"' => quoted = true,
                b'[' | b'{' => {
                    depth += 1;
                    if depth > 128 {
                        return Err(Error::value("JSON nesting exceeds 128 levels"));
                    }
                }
                b']' | b'}' => depth = depth.saturating_sub(1),
                _ => (),
            }
        }
    }
    Ok(())
}

fn decode<T: DeserializeOwned>(text: &str) -> Result<T> {
    check_depth(text)?;
    let mut decoder = serde_json::Deserializer::from_str(text);
    decoder.disable_recursion_limit();
    let value = T::deserialize(&mut decoder).map_err(|error| Error::value(error.to_string()))?;
    decoder
        .end()
        .map_err(|error| Error::value(error.to_string()))?;
    Ok(value)
}

pub fn read_inputs(manifest_file: &str, old_file: &str, new_file: &str) -> Result<Inputs> {
    let manifest: Manifest = decode(&read_text(manifest_file)?)?;
    let mut paths = HashMap::new();
    for alias in manifest.diagnostic_paths {
        if paths.insert(alias.raw, Arc::from(alias.path)).is_some() {
            return Err(Error::value("duplicate diagnostic path alias"));
        }
    }
    let mut seen = HashSet::new();
    let mut sources = Vec::with_capacity(manifest.test_files.len());
    for path in manifest.test_files {
        if !seen.insert(path.clone()) {
            return Err(Error::value("duplicate conformance test path"));
        }
        let text = read_text(&path)?;
        sources.push(SourceText {
            path: Arc::from(path),
            text,
        });
    }
    Ok(Inputs {
        revision: manifest.suite_revision,
        paths,
        sources,
        old: read_text(old_file)?,
        new: read_text(new_file)?,
    })
}

fn python_whitespace(character: char) -> bool {
    character.is_whitespace() || matches!(character, '\u{1c}'..='\u{1f}')
}

fn line_break(character: char) -> bool {
    matches!(
        character,
        '\n' | '\r' | '\u{b}' | '\u{c}' | '\u{1c}'..='\u{1e}' | '\u{85}' | '\u{2028}' | '\u{2029}'
    )
}

struct Annotation<'a> {
    start: usize,
    end: usize,
    start_byte: usize,
    optional: bool,
    tag: Option<&'a str>,
    multi: bool,
    description: &'a str,
}

/// Scan raw text directly; malformed optional tag groups fall back to the
/// shorter bare-E match exactly as the upstream regular expression does.
fn annotation(line: &str) -> Option<Annotation<'_>> {
    for (start, _) in line.match_indices('#') {
        let suffix = line[start + 1..].trim_start_matches(python_whitespace);
        let Some(after_e) = suffix.strip_prefix('E') else {
            continue;
        };
        let optional = after_e.starts_with('?');
        let mut rest = if optional { &after_e[1..] } else { after_e };
        let mut tag = None;
        let mut multi = false;
        if let Some(after_bracket) = rest.strip_prefix('[') {
            if let Some(close) = after_bracket.find(']') {
                let candidate = &after_bracket[..close];
                let (name, repeated) = candidate
                    .strip_suffix('+')
                    .map_or((candidate, false), |name| (name, true));
                if !name.is_empty() && !name.contains('+') {
                    tag = Some(name);
                    multi = repeated;
                    rest = &after_bracket[close + 1..];
                }
            }
        }
        let mut end = line.len() - rest.len();
        let mut description = "Missing";
        if let Some(after_colon) = rest.trim_start_matches(python_whitespace).strip_prefix(':') {
            let candidate = after_colon.trim_start_matches(python_whitespace);
            if !candidate.is_empty() {
                description = candidate;
            }
            end = line.len();
        }
        return Some(Annotation {
            start: line[..start].chars().count(),
            end: line[..end].chars().count(),
            start_byte: start,
            optional,
            tag,
            multi,
            description,
        });
    }
    None
}

pub fn collect_expected(sources: &[SourceText]) -> Result<Vec<ExpectedError>> {
    let mut expected = Vec::new();
    for source in sources {
        // read_text has already normalized CRLF and CR, just like Python's
        // text reader. split_terminator drops the one trailing empty line.
        for (index, line) in source.text.split_terminator(line_break).enumerate() {
            let Some(found) = annotation(line) else {
                continue;
            };
            let prefix = line[..found.start_byte].trim_start_matches(python_whitespace);
            if prefix.is_empty() || prefix.starts_with('#') {
                continue;
            }
            expected.push(ExpectedError {
                path: Arc::clone(&source.path),
                line: Integer::Small(index as i64 + 1),
                optional: found.optional,
                multi: found.multi,
                tag: found
                    .tag
                    .map(|tag| format!("{}:{tag}", filename(&source.path))),
                _description: found.description.to_owned(),
                _begin_column: found.start,
                _end_column: found.end,
            });
        }
    }
    if expected.is_empty() {
        return Err(Error::assertion(
            "Failed to discover any expected diagnostics!",
        ));
    }
    Ok(expected)
}

pub fn collect_diagnostics(
    text: &str,
    paths: &HashMap<String, Arc<str>>,
) -> Result<Vec<Diagnostic>> {
    let wire: Vec<WireDiagnostic> = decode(text)?;
    let mut result = Vec::new();
    for diagnostic in wire {
        if diagnostic.severity != "major" {
            continue;
        }
        let path = paths
            .get(&diagnostic.location.path)
            .ok_or_else(|| Error::key(&diagnostic.location.path))?;
        let description = diagnostic
            .description
            .replace(&format!("{}: ", diagnostic.check_name), "");
        result.push(Diagnostic {
            check_name: diagnostic.check_name,
            description,
            severity: diagnostic.severity,
            path: Arc::clone(path),
            positions: diagnostic.location.positions,
        });
    }
    Ok(result)
}
