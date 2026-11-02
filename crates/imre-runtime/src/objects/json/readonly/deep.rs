//! Decode deeply nested standard JSON without recursing on the Rust stack.
//! Frames contain arena indices, so errors and discarded results unwind iteratively too.
use crate::StrOwner;
use std::rc::Rc;

use super::super::syntax::{self, Cursor, Failure};
use super::{Error, JsonNumber, KeyMemo, Kind, Node, Nodes, Object, Str, View, Words, number};

// Imre has one decoder. CPython's pure scanner counts Python calls and its C
// scanner checks the native stack, so their platform-dependent boundaries differ.
const MAX_OPEN_CONTAINERS: usize = 50_000;

/// A completed JSON value whose children already have handles in the caller's storage.
pub(crate) enum DecodedJson<T, S> {
    Null,
    Bool(bool),
    Number(JsonNumber),
    Constant(&'static str),
    String(S),
    Array(Vec<T>),
    Object(Vec<(S, T)>),
}

enum Frame<T, S> {
    Array(Vec<T>),
    Object { fields: Vec<(S, T)>, key: Option<S> },
}

impl<T, S> Frame<T, S> {
    fn close(&self) -> u8 {
        match self {
            Self::Array(_) => b']',
            Self::Object { .. } => b'}',
        }
    }

    fn key<E>(
        &mut self,
        cursor: &mut Cursor<'_>,
        string: &impl Fn(&str, usize) -> Result<S, Problem<E>>,
    ) -> Result<(), Problem<E>> {
        if let Self::Object { key, .. } = self {
            if cursor.peek() != Some(b'"') {
                return Err((
                    cursor.pos,
                    "Expecting property name enclosed in double quotes",
                )
                    .into());
            }
            let start = cursor.pos;
            *key = Some(string(cursor.scalar()?, start)?);
            cursor.space();
            if cursor.peek() != Some(b':') {
                return Err((cursor.pos, "Expecting ':' delimiter").into());
            }
            cursor.pos += 1;
            cursor.space();
        }
        Ok(())
    }

    fn add<E>(&mut self, value: T) -> Result<(), Problem<E>> {
        match self {
            Self::Array(items) => push(items, value),
            Self::Object { fields, key } => push(fields, (key.take().expect("parsed key"), value)),
        }
    }

    fn finish(self) -> DecodedJson<T, S> {
        match self {
            Self::Array(items) => DecodedJson::Array(items),
            Self::Object { fields, .. } => DecodedJson::Object(fields),
        }
    }
}

/// Distinguish document errors from exceptions raised while building a value.
pub(crate) enum JsonTokenError<E> {
    Parse(Error),
    Build(E),
}

enum Problem<E> {
    Syntax(Failure),
    String(usize, serde_json::Error),
    Parse(Error),
    Build(E),
}

impl<E> From<Failure> for Problem<E> {
    fn from(failure: Failure) -> Self {
        Self::Syntax(failure)
    }
}

fn push<T, E>(items: &mut Vec<T>, value: T) -> Result<(), Problem<E>> {
    items
        .try_reserve(1)
        .map_err(|_| Problem::Parse(Error::allocation()))?;
    items.push(value);
    Ok(())
}

fn string(token: &str, start: usize) -> Result<Str, Problem<Error>> {
    serde_json::from_str::<String>(token)
        .map(Str::from)
        .map_err(|e| Problem::String(start, e))
}

fn parse<T, S, E>(
    cursor: &mut Cursor<'_>,
    string: impl Fn(&str, usize) -> Result<S, Problem<E>>,
    mut build: impl FnMut(DecodedJson<T, S>) -> Result<T, E>,
) -> Result<T, Problem<E>> {
    let digit_limit = crate::integer_text::decimal_digit_limit();
    let mut frames = Vec::new();
    loop {
        let mut node = match cursor.peek() {
            Some(open @ (b'[' | b'{')) => {
                if frames.len() >= MAX_OPEN_CONTAINERS {
                    return Err(Problem::Parse(Error::new(
                        Kind::RecursionError,
                        "maximum recursion depth exceeded",
                    )));
                }
                cursor.pos += 1;
                cursor.space();
                let mut frame = if open == b'[' {
                    Frame::Array(Vec::new())
                } else {
                    Frame::Object {
                        fields: Vec::new(),
                        key: None,
                    }
                };
                if cursor.peek() != Some(frame.close()) {
                    frame.key(cursor, &string)?;
                    push(&mut frames, frame)?;
                    continue;
                }
                cursor.pos += 1;
                frame.finish()
            }
            _ => {
                let start = cursor.pos;
                match cursor.scalar()? {
                    "true" => DecodedJson::Bool(true),
                    "false" => DecodedJson::Bool(false),
                    "null" => DecodedJson::Null,
                    "NaN" => DecodedJson::Constant("NaN"),
                    "Infinity" => DecodedJson::Constant("Infinity"),
                    "-Infinity" => DecodedJson::Constant("-Infinity"),
                    token if token.starts_with('"') => DecodedJson::String(string(token, start)?),
                    token => {
                        if digit_limit != 0
                            && token.len() > digit_limit as usize
                            && !token.contains(['.', 'e', 'E'])
                        {
                            let digits = token.strip_prefix('-').unwrap_or(token);
                            crate::integer_text::check_input_digits(digits, digit_limit)
                                .map_err(Problem::Parse)?;
                        }
                        DecodedJson::Number(number(token))
                    }
                }
            }
        };
        loop {
            let value = build(node).map_err(Problem::Build)?;
            let Some(frame) = frames.last_mut() else {
                return Ok(value);
            };
            cursor.space();
            frame.add(value)?;
            match cursor.peek() {
                Some(close) if close == frame.close() => {
                    cursor.pos += 1;
                    node = frames.pop().unwrap().finish();
                }
                Some(b',') => {
                    let comma = cursor.pos;
                    cursor.pos += 1;
                    cursor.space();
                    if cursor.peek() == Some(frame.close()) {
                        let message = match frame {
                            Frame::Array(_) => "Illegal trailing comma before end of array",
                            Frame::Object { .. } => "Illegal trailing comma before end of object",
                        };
                        return Err((comma, message).into());
                    }
                    frame.key(cursor, &string)?;
                    break;
                }
                _ => return Err((cursor.pos, "Expecting ',' delimiter").into()),
            }
        }
    }
}

impl<E> Problem<E> {
    fn finish(self, text: &str, owner: impl FnOnce() -> Option<StrOwner>) -> JsonTokenError<E> {
        let (at, message) = match self {
            Problem::Parse(error) => return JsonTokenError::Parse(error),
            Problem::Build(error) => return JsonTokenError::Build(error),
            Problem::Syntax((at, message)) => (at, std::borrow::Cow::Borrowed(message)),
            Problem::String(start, error) => {
                let message = error.to_string();
                let message = message
                    .rsplit_once(" at line ")
                    .map_or(message.as_str(), |(msg, _)| msg);
                (
                    text.floor_char_boundary(start + error.column().saturating_sub(1)),
                    std::borrow::Cow::Owned(message.to_owned()),
                )
            }
        };
        JsonTokenError::Parse(
            owner().map_or_else(Error::allocation, |doc| syntax::at_error(doc, at, &message)),
        )
    }
}

/// Scan one value at a validated UTF-8 byte boundary. String bodies and the end
/// of the root (before trailing whitespace) are byte offsets in the original document.
pub(crate) fn decode_json_tokens_at<T, E>(
    text: &str,
    start: usize,
    owner: impl FnOnce() -> Option<StrOwner>,
    build: impl FnMut(DecodedJson<T, std::ops::Range<usize>>) -> Result<T, E>,
) -> Result<(T, usize), JsonTokenError<E>> {
    debug_assert!(text.is_char_boundary(start));
    let mut cursor = Cursor {
        source: text,
        pos: start,
    };
    let value = parse(
        &mut cursor,
        |token, start| Ok(start + 1..start + token.len() - 1),
        build,
    )
    .map_err(|err| err.finish(text, owner))?;
    Ok((value, cursor.pos))
}

pub(super) fn loads(
    text: &str,
    owner: impl FnOnce() -> Option<StrOwner>,
) -> Result<*const Object, Error> {
    let keys = KeyMemo::default();
    let mut nodes = Vec::new();
    let mut cursor = Cursor {
        source: text,
        pos: 0,
    };
    cursor.space();
    let index = parse(&mut cursor, string, |value| {
        let node = match value {
            DecodedJson::Null => Node::Null,
            DecodedJson::Bool(value) => Node::Bool(value),
            DecodedJson::Number(value) => value.into(),
            DecodedJson::Constant(raw) => number(raw).into(),
            DecodedJson::String(value) => Node::String(StrOwner::new(value)),
            DecodedJson::Array(items) => Node::Array(items),
            DecodedJson::Object(fields) => {
                Node::Object(keys.fields(fields).map_err(|_| Error::allocation())?)
            }
        };
        let index = nodes.len();
        nodes.try_reserve(1).map_err(|_| Error::allocation())?;
        nodes.push(node);
        Ok(index)
    })
    .and_then(|index| {
        cursor.space();
        if cursor.pos == text.len() {
            Ok(index)
        } else {
            Err((cursor.pos, "Extra data").into())
        }
    })
    .map_err(|problem| match problem.finish(text, owner) {
        JsonTokenError::Parse(error) | JsonTokenError::Build(error) => error,
    })?;
    // A standard root list, unlike its nested views, may be mutated by its caller.
    if let Node::Array(children) = &mut nodes[index] {
        let children = std::mem::take(children);
        let nodes = Rc::new(nodes);
        let document = Nodes::Deep(nodes.clone());
        let mut result = Words::new(5);
        result
            .values
            .try_reserve_exact(children.len())
            .map_err(|_| Error::allocation())?;
        result.values.extend(
            children
                .into_iter()
                .map(|i| View::handle(&document, i) as usize),
        );
        Ok(View {
            nodes: Nodes::DeepRoot(nodes, result.into_object()),
            index,
        }
        .into_handle())
    } else {
        Ok(View {
            nodes: Nodes::Deep(Rc::new(nodes)),
            index,
        }
        .into_handle())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefix_ends_before_whitespace_and_distinguishes_constants_from_overflow() {
        let text = "π{ \"x\" : [ NaN, Infinity, -Infinity, 1e999 ] }   tail";
        let mut literals = Vec::new();
        let result = decode_json_tokens_at(
            text,
            "π".len(),
            || None,
            |value| {
                match value {
                    DecodedJson::Constant(raw) => literals.push(raw),
                    DecodedJson::Number(JsonNumber::Float(n)) => assert_eq!(n, f64::INFINITY),
                    DecodedJson::Array(values) => assert_eq!(values.len(), 4),
                    DecodedJson::Object(fields) => assert_eq!(&text[fields[0].0.clone()], "x"),
                    _ => panic!("unexpected token"),
                }
                Ok::<_, ()>(())
            },
        );
        assert!(matches!(result, Ok(((), end)) if &text[end..] == "   tail"));
        assert_eq!(literals, ["NaN", "Infinity", "-Infinity"]);
    }

    #[test]
    fn prefix_preserves_error_positions_and_builder_exceptions() {
        for (text, at) in [("π null", 1), ("π[ { \"x\" :  ?} ]", 12)] {
            let result = decode_json_tokens_at(
                text,
                "π".len(),
                || Some(StrOwner::new(Str::new(text))),
                |_| Ok::<_, ()>(()),
            );
            assert!(matches!(result, Err(JsonTokenError::Parse(e))
                if e.json_decode_fields() == Some(("Expecting value", text, at))));
        }
        let raised = Rc::new(());
        let result = decode_json_tokens_at("[0, ?]", 0, || None, |_| Err::<(), _>(raised.clone()));
        assert!(matches!(result, Err(JsonTokenError::Build(e)) if Rc::ptr_eq(&raised, &e)));
    }
}
