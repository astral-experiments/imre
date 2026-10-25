use std::ops::Range;
use std::sync::LazyLock;

use regex::Regex;
use ruff_python_ast::Expr;
use ruff_python_parser::parse_expression;

/// Locate the original bytes spelling Python's four extra whitespace characters.
/// ty 0.0.14 splits namedtuple field strings with Rust's Unicode whitespace rules.
pub(super) fn separators(fields: &Expr, source: &str, ranges: &mut Vec<Range<usize>>) {
    let Expr::StringLiteral(fields) = fields else {
        return;
    };
    let separator = |text: &str| matches!(text.as_bytes(), [0x1c..=0x1f]);
    for part in &fields.value {
        if !part.as_str().contains(['\x1c', '\x1d', '\x1e', '\x1f']) {
            continue;
        }
        let content = part.content_range();
        let start = usize::from(content.start());
        let text = &source[content];
        if part.flags.prefix().is_raw() {
            ranges.extend(
                text.match_indices(['\x1c', '\x1d', '\x1e', '\x1f'])
                    .map(|(i, _)| start + i..start + i + 1),
            );
            continue;
        }
        static ESCAPE_OR_SEPARATOR: LazyLock<Regex> = LazyLock::new(|| {
            Regex::new(r"(?s)\\(?:[0-7]{1,3}|x[[:xdigit:]]{2}|u[[:xdigit:]]{4}|U[[:xdigit:]]{8}|N\{[^}]*\}|.)|[\x1c-\x1f]").unwrap()
        });
        for spelling in ESCAPE_OR_SEPARATOR.find_iter(text) {
            let raw = spelling.as_str();
            if separator(raw) || (raw.starts_with('\\')
                && parse_expression(&format!("'{raw}'")).is_ok_and(|parsed|
                    matches!(parsed.expr(), Expr::StringLiteral(s) if separator(s.value.to_str()))))
            {
                ranges.push(start + spelling.start()..start + spelling.end());
            }
        }
    }
}
