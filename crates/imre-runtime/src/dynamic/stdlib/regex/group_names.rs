use regex_syntax::ast::{Ast, Error, ErrorKind, GroupKind};

pub(super) fn valid(name: &str, bytes: bool) -> bool {
    (!bytes || name.is_ascii())
        && (ruff_python_stdlib::identifiers::is_identifier(name)
            || ruff_python_stdlib::keyword::is_keyword(name))
}

/// Recover Python's name and character offset from the parser's original source spans.
pub(super) fn diagnostic(
    text: &str,
    bytes: bool,
    points: Option<&[u32]>,
    original_position: impl Fn(usize) -> usize,
    parsed: &Result<Ast, Error>,
) -> Option<(String, usize)> {
    if !text.contains("(?P") {
        return None;
    }
    let (start, end) = match parsed {
        Err(error) => {
            let at = error.span().start.offset;
            match error.kind() {
                ErrorKind::GroupNameInvalid
                | ErrorKind::GroupNameEmpty
                | ErrorKind::GroupNameUnexpectedEof => {
                    let open = text[..at].rfind("(?")?;
                    text[open..]
                        .starts_with("(?P<")
                        .then_some((open + 4, '>'))?
                }
                ErrorKind::FlagUnrecognized
                    if text[..at].ends_with("(?") && text[at..].starts_with("P=") =>
                {
                    (at + 2, ')')
                }
                _ => return None,
            }
        }
        Ok(ast) => {
            let mut stack = vec![ast];
            let start = loop {
                match stack.pop()? {
                    Ast::Group(group) => {
                        if let GroupKind::CaptureName {
                            starts_with_p: true,
                            name,
                        } = &group.kind
                            && !valid(&name.name, bytes)
                        {
                            break name.span.start.offset;
                        }
                        stack.push(&group.ast);
                    }
                    Ast::Concat(c) => stack.extend(c.asts.iter().rev()),
                    Ast::Alternation(a) => stack.extend(a.asts.iter().rev()),
                    Ast::Repetition(r) => stack.push(&r.ast),
                    _ => {}
                }
            };
            (start, '>')
        }
    };
    let mut chars = text[start..].char_indices();
    let position = text[..start].chars().count();
    while let Some((i, ch)) = chars.next() {
        if ch == '\\' {
            if chars.next().is_none() {
                return Some((
                    "bad escape (end of pattern)".into(),
                    text.chars().count() - 1,
                ));
            }
        } else if ch == end {
            let name = &text[start..start + i];
            if name.is_empty() {
                return Some(("missing group name".into(), position));
            }
            if valid(name, bytes) {
                return None;
            }
            return Some((
                bad_source_name(name, bytes, points, original_position(position)),
                position,
            ));
        }
    }
    let message = if text.len() == start {
        "missing group name".into()
    } else {
        format!("missing {end}, unterminated name")
    };
    Some((message, position))
}

pub(super) fn bad_source_name(
    name: &str,
    bytes: bool,
    points: Option<&[u32]>,
    at: usize,
) -> String {
    if let Some(points) = points {
        let quoted =
            crate::dynamic::operators::surrogate_repr(&points[at..at + name.chars().count()]);
        format!("bad character in group name {quoted}")
    } else {
        bad_name(name, bytes)
    }
}

/// A bytes pattern reports its Latin-1 spelling through Python's ASCII representation.
pub(super) fn bad_name(name: &str, bytes: bool) -> String {
    if bytes {
        let data: Vec<_> = name.chars().map(|ch| ch as u8).collect();
        let mut quoted = String::new();
        crate::objects::write_bytes_repr(&mut quoted, &data);
        format!("bad character in group name {}", &quoted[1..])
    } else {
        format!(
            "bad character in group name {}",
            crate::objects::exceptions::string_repr(name)
        )
    }
}
