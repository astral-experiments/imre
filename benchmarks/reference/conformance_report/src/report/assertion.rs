//! Python representations in Ruff's five-case assertion sample.

use std::fmt::{Display, Write};
use unicode_general_category::get_general_category;

use crate::Error;
use crate::model::{Diagnostic, ExpectedError, Integer, TestCase};

fn quoted(text: &str) -> String {
    let quote = if text.contains('\'') && !text.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut result = String::from(quote);
    for ch in text.chars() {
        match ch {
            '\t' => result.push_str("\\t"),
            '\n' => result.push_str("\\n"),
            '\r' => result.push_str("\\r"),
            _ if ch == quote || ch == '\\' => {
                result.push('\\');
                result.push(ch);
            }
            // Python prints ASCII space and escapes every other Unicode
            // separator (Z*) or control, format, private-use or unassigned (C*) character.
            _ if ch != ' '
                && get_general_category(ch)
                    .abbreviation()
                    .starts_with(['C', 'Z']) =>
            {
                let code = u32::from(ch);
                match code {
                    0..=0xff => write!(result, "\\x{code:02x}"),
                    0x100..=0xffff => write!(result, "\\u{code:04x}"),
                    _ => write!(result, "\\U{code:08x}"),
                }
                .unwrap();
            }
            _ => result.push(ch),
        }
    }
    result.push(quote);
    result
}

fn list(items: impl Iterator<Item = String>) -> String {
    format!("[{}]", items.collect::<Vec<_>>().join(", "))
}

fn location(
    path: &str,
    line: &Integer,
    column: impl Display,
    end_line: &Integer,
    end_column: impl Display,
) -> String {
    format!(
        "Location(path=PosixPath({}), positions=Positions(begin=Position(line={line}, column={column}), end=Position(line={end_line}, column={end_column})))",
        quoted(path)
    )
}

fn diagnostics(items: &[Diagnostic], source: &str) -> String {
    list(items.iter().map(|d| {
        let begin = &d.positions.begin;
        let end = &d.positions._end;
        format!("TyDiagnostic(check_name={}, description={}, severity={}, location={}, source={source})",
            quoted(&d.check_name), quoted(&d.description), quoted(&d.severity),
            location(&d.path, &begin.line, &begin.column, &end.line, &end.column))
    }))
}

fn expected(error: &ExpectedError) -> String {
    let truth = |flag| if flag { "True" } else { "False" };
    format!(
        "ExpectedError(description={}, location={}, optional={}, tag={}, multi={})",
        quoted(&error._description),
        location(
            &error.path,
            &error.line,
            error._begin_column,
            &error.line,
            error._end_column
        ),
        truth(error.optional),
        error
            .tag
            .as_deref()
            .map_or_else(|| "None".to_owned(), quoted),
        truth(error.multi)
    )
}

#[cold]
pub(super) fn missing_true_positives(cases: &[TestCase]) -> Error {
    let sample = list(cases.iter().take(5).map(|case| {
        format!(
            "TestCase(key={}, old={}, new={}, expected={})",
            quoted(&case.key),
            diagnostics(&case.old, "<Source.OLD: 'old'>"),
            diagnostics(&case.new, "<Source.NEW: 'new'>"),
            list(case.expected.iter().map(expected))
        )
    }));
    Error::assertion(format!(
        "Expected ty to have at least one true positive.\nSample of grouped diagnostics: {sample}"
    ))
}
