//! Decode deeply nested standard JSON without recursing on the Rust stack.
//! Frames contain arena indices, so errors and discarded results unwind iteratively too.
use crate::StrOwner;
use std::rc::Rc;

use super::super::syntax::{self, Cursor, Failure};
use super::{Error, KeyMemo, Kind, Node, Nodes, Object, Str, View, Words, number};

// Imre has one decoder. CPython's pure scanner counts Python calls and its C
// scanner checks the native stack, so their platform-dependent boundaries differ.
const MAX_OPEN_CONTAINERS: usize = 50_000;

enum Frame {
    Array(Vec<usize>),
    Object {
        fields: Vec<(Str, usize)>,
        key: Option<Str>,
    },
}

impl Frame {
    fn close(&self) -> u8 {
        match self {
            Self::Array(_) => b']',
            Self::Object { .. } => b'}',
        }
    }

    fn key(&mut self, cursor: &mut Cursor<'_>) -> Result<(), Problem> {
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
        }
        Ok(())
    }

    fn add(&mut self, value: usize) -> Result<(), Problem> {
        match self {
            Self::Array(items) => push(items, value),
            Self::Object { fields, key } => push(fields, (key.take().expect("parsed key"), value)),
        }
    }

    fn finish(self, keys: &KeyMemo) -> Result<Node, Problem> {
        Ok(match self {
            Self::Array(items) => Node::Array(items),
            Self::Object { fields, .. } => {
                Node::Object(keys.fields(fields).map_err(|_| Problem::Memory)?)
            }
        })
    }
}

enum Problem {
    Syntax(Failure),
    String(usize, serde_json::Error),
    Exhausted,
    Memory,
}

impl From<Failure> for Problem {
    fn from(failure: Failure) -> Self {
        Self::Syntax(failure)
    }
}

fn push<T>(items: &mut Vec<T>, value: T) -> Result<(), Problem> {
    items.try_reserve(1).map_err(|_| Problem::Memory)?;
    items.push(value);
    Ok(())
}

fn string(token: &str, start: usize) -> Result<Str, Problem> {
    serde_json::from_str::<String>(token)
        .map(Str::from)
        .map_err(|e| Problem::String(start, e))
}

fn parse(text: &str) -> Result<(Vec<Node>, usize), Problem> {
    let mut cursor = Cursor {
        source: text,
        pos: 0,
    };
    let keys = KeyMemo::default();
    let mut frames = Vec::new();
    let mut nodes = Vec::new();
    loop {
        cursor.space();
        let mut node = match cursor.peek() {
            Some(open @ (b'[' | b'{')) => {
                if frames.len() >= MAX_OPEN_CONTAINERS {
                    return Err(Problem::Exhausted);
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
                    frame.key(&mut cursor)?;
                    push(&mut frames, frame)?;
                    continue;
                }
                cursor.pos += 1;
                frame.finish(&keys)?
            }
            _ => {
                let start = cursor.pos;
                match cursor.scalar()? {
                    "true" => Node::Bool(true),
                    "false" => Node::Bool(false),
                    "null" => Node::Null,
                    token if token.starts_with('"') => {
                        Node::String(StrOwner::new(string(token, start)?))
                    }
                    token => number(token),
                }
            }
        };
        loop {
            let index = nodes.len();
            push(&mut nodes, node)?;
            cursor.space();
            let Some(frame) = frames.last_mut() else {
                if cursor.pos != text.len() {
                    return Err((cursor.pos, "Extra data").into());
                }
                return Ok((nodes, index));
            };
            frame.add(index)?;
            match cursor.peek() {
                Some(close) if close == frame.close() => {
                    cursor.pos += 1;
                    node = frames.pop().unwrap().finish(&keys)?;
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
                    frame.key(&mut cursor)?;
                    break;
                }
                _ => return Err((cursor.pos, "Expecting ',' delimiter").into()),
            }
        }
    }
}

pub(super) fn loads(
    text: &str,
    owner: impl FnOnce() -> Option<StrOwner>,
) -> Result<*const Object, Error> {
    let (mut nodes, index) = parse(text).map_err(|problem| {
        let (at, message) = match problem {
            Problem::Exhausted => {
                return Error::new(Kind::RecursionError, "maximum recursion depth exceeded");
            }
            Problem::Memory => return Error::allocation(),
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
        owner().map_or_else(Error::allocation, |doc| syntax::at_error(doc, at, &message))
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
