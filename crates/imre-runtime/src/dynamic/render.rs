use std::collections::HashSet;
use std::fmt::Write as _;

use super::{Context, Error, Id, Value};

#[derive(Clone, Copy)]
enum Members<'a> {
    Sequence(&'a [Id]),
    Dictionary(&'a [(Id, Id)]),
    Named(&'a [String], &'a [Id]),
}

impl Members<'_> {
    fn len(self) -> usize {
        match self {
            Self::Sequence(xs) => xs.len(),
            Self::Dictionary(xs) => xs.len(),
            Self::Named(names, values) => names.len().min(values.len()),
        }
    }
}

enum Part<'a> {
    Value(Id, bool),
    Text(&'a str),
    Members(Members<'a>, usize),
    Leave(Id, &'static str),
    ContextEnd(Id),
}

impl Context {
    pub(super) fn repr(&self, id: Id) -> String {
        self.render_value(id, true)
    }

    pub(super) fn display(&self, id: Id) -> String {
        self.render_value(id, false)
    }

    fn render_value(&self, id: Id, quoted: bool) -> String {
        let mut output = String::new();
        let mut work = Vec::new();
        let mut first = Some(Part::Value(id, quoted));
        // Only ancestors on the current path are recursive. The same child can
        // appear again after its closing delimiter and must be rendered in full.
        let mut active = HashSet::new();
        while let Some(part) = first.take().or_else(|| work.pop()) {
            match part {
                Part::Text(text) => output.push_str(text),
                Part::Leave(id, end) => {
                    active.remove(&id);
                    output.push_str(end);
                }
                Part::ContextEnd(id) => {
                    write!(output, " at {id:#x}>").expect("writing into a string cannot fail");
                }
                Part::Members(members, i) => {
                    if i == members.len() {
                        continue;
                    }
                    if i > 0 {
                        output.push_str(", ");
                    }
                    work.push(Part::Members(members, i + 1));
                    match members {
                        Members::Sequence(values) => work.push(Part::Value(values[i], true)),
                        Members::Dictionary(pairs) => {
                            let (key, value) = pairs[i];
                            work.push(Part::Value(value, true));
                            work.push(Part::Text(": "));
                            work.push(Part::Value(key, true));
                        }
                        Members::Named(names, values) => {
                            output.push_str(&names[i]);
                            output.push('=');
                            work.push(Part::Value(values[i], true));
                        }
                    }
                }
                Part::Value(id, quoted) => {
                    let value = self.heap.get(id);
                    let (members, start, end, recursive) = match value {
                        Value::List(items) => (Members::Sequence(items), "[", "]", "[...]"),
                        Value::Tuple(items) => (
                            Members::Sequence(items),
                            "(",
                            if items.len() == 1 { ",)" } else { ")" },
                            "(...)",
                        ),
                        Value::Set(items) if items.is_empty() => {
                            output.push_str("set()");
                            continue;
                        }
                        Value::Set(items) => (Members::Sequence(items), "{", "}", "set(...)"),
                        Value::Dict(pairs) => (Members::Dictionary(pairs), "{", "}", "{...}"),
                        Value::Instance {
                            dictionary: Some(backing),
                            ..
                        } => {
                            work.push(Part::Value(*backing, false));
                            continue;
                        }
                        Value::NamedTuple {
                            class: Some(class),
                            names,
                            values,
                            ..
                        } => {
                            output.push_str(class);
                            output.push('(');
                            work.push(Part::Text(")"));
                            work.push(Part::Members(Members::Named(names, values), 0));
                            continue;
                        }
                        Value::ContextVar { name, default } => {
                            output.push_str("<ContextVar name=");
                            work.push(Part::ContextEnd(id));
                            if let Some(default) = default {
                                work.push(Part::Value(*default, true));
                                work.push(Part::Text(" default="));
                            }
                            work.push(Part::Value(*name, true));
                            continue;
                        }
                        Value::ContextToken { variable, used, .. } => {
                            output.push_str(if *used {
                                "<Token used var="
                            } else {
                                "<Token var="
                            });
                            work.push(Part::ContextEnd(id));
                            work.push(Part::Value(*variable, true));
                            continue;
                        }
                        Value::Function { fields, .. } => {
                            output.push_str("<function ");
                            work.push(Part::Text(">"));
                            work.push(Part::Value(fields.qualname, false));
                            continue;
                        }
                        _ => {
                            let atom = Self::render_atom(id, value, quoted);
                            if output.is_empty() && work.is_empty() {
                                return atom;
                            }
                            output.push_str(&atom);
                            continue;
                        }
                    };
                    if active.insert(id) {
                        output.push_str(start);
                        work.push(Part::Leave(id, end));
                        work.push(Part::Members(members, 0));
                    } else {
                        output.push_str(recursive);
                    }
                }
            }
        }
        output
    }

    fn render_atom(id: Id, value: &Value, quoted: bool) -> String {
        match value {
            Value::Context(_) => format!("<_contextvars.Context object at {id:#x}>"),
            Value::TokenMissing => "<Token.MISSING>".to_owned(),
            Value::None => "None".into(),
            Value::Ellipsis => "Ellipsis".into(),
            Value::NotImplemented => "NotImplemented".into(),
            Value::Bool(true) => "True".into(),
            Value::Bool(false) => "False".into(),
            Value::Int(n) => n.to_string(),
            Value::Float(n) => crate::floats::render(*n).to_string(),
            Value::Calendar { value, .. } => {
                if quoted {
                    value.repr()
                } else {
                    value.text()
                }
            }
            Value::Str(s) if quoted => {
                let mut out = String::new();
                crate::objects::exceptions::write_string_repr(s, &mut out)
                    .expect("writing into a string cannot fail");
                out
            }
            Value::Str(s) => s.clone(),
            Value::Bytes(bytes) => format!(
                "b'{}'",
                bytes
                    .iter()
                    .flat_map(|&x| std::ascii::escape_default(x))
                    .map(char::from)
                    .collect::<String>()
            ),
            Value::ByteArray(bytes) => {
                let quote = if bytes.contains(&b'\'') && !bytes.contains(&b'"') {
                    '"'
                } else {
                    '\''
                };
                let mut output = format!("bytearray(b{quote}");
                for &byte in bytes {
                    if byte == b'"' {
                        output.push('"');
                    } else {
                        output.extend(std::ascii::escape_default(byte).map(char::from));
                    }
                }
                output.push(quote);
                output.push(')');
                output
            }
            Value::Raised(Error { message, .. }) => message.clone(),
            Value::Class { name, .. } | Value::Native(name) => format!("<class '{name}'>"),
            Value::Module { name, .. } => format!("<module '{name}'>"),
            Value::AttrGetter(paths) => {
                let arguments = paths
                    .iter()
                    .map(|path| {
                        let mut quoted = String::new();
                        crate::objects::exceptions::write_string_repr(&path.join("."), &mut quoted)
                            .expect("writing into a string cannot fail");
                        quoted
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("operator.attrgetter({arguments})")
            }
            _ => "<object>".to_owned(),
        }
    }
}
