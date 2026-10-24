//! The default tomli_w text format for the JSON subset of TOML values.

use std::fmt::Write;

use super::{Error, Kind, Node, Str, View};

fn quoted(value: &str) -> String {
    let mut out = String::from("\"");
    for ch in value.chars() {
        match ch {
            '\x08' => out.push_str("\\b"),
            '\n' => out.push_str("\\n"),
            '\x0c' => out.push_str("\\f"),
            '\r' => out.push_str("\\r"),
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\t' => out.push(ch),
            '\0'..='\x1f' | '\x7f' => write!(out, "\\u{:04x}", u32::from(ch)).unwrap(),
            _ => out.push(ch),
        }
    }
    out.push('"');
    out
}

pub(in crate::objects) fn key(value: &str) -> String {
    if !value.is_empty()
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_'))
    {
        value.to_owned()
    } else {
        quoted(value)
    }
}

/// JSON parsers keep duplicate keys in their source positions. A Python dict
/// keeps the first insertion position and replaces its value on subsequent writes.
fn entries(node: &Node) -> impl Iterator<Item = (&Str, usize)> {
    let Node::Object(fields) = node else {
        unreachable!("an exact JSON object")
    };
    fields
        .iter()
        .enumerate()
        .filter(move |(index, (name, _))| !fields[..*index].iter().any(|(seen, _)| seen == name))
        .map(move |(_, (name, _))| (name, node.lookup(name).unwrap()))
}

impl View {
    pub(in crate::objects) fn toml_table(
        &self,
        name: &str,
        output: &mut String,
    ) -> Result<(), Error> {
        self.table(self.index, &key(name), false, output)
    }

    fn table(&self, index: usize, name: &str, array: bool, out: &mut String) -> Result<(), Error> {
        let mut values = Vec::new();
        let mut tables = Vec::new();
        for (name, child) in entries(&self.nodes[index]) {
            match &self.nodes[child] {
                Node::Object(_) => tables.push((name, child, false)),
                Node::Array(items)
                    if !items.is_empty()
                        && items
                            .iter()
                            .all(|&i| matches!(self.nodes[i], Node::Object(_))) =>
                {
                    let mut inline = true;
                    for &item in items {
                        let mut rendered = String::new();
                        self.literal(item, 0, &mut rendered)?;
                        if rendered.chars().count() + 5 > 100 || rendered.contains('\n') {
                            inline = false;
                            break;
                        }
                    }
                    if inline {
                        values.push((name, child));
                    } else {
                        tables.extend(items.iter().map(|&item| (name, item, true)));
                    }
                }
                _ => values.push((name, child)),
            }
        }
        let mut written = array || !name.is_empty() && (!values.is_empty() || tables.is_empty());
        if array {
            writeln!(out, "[[{name}]]").unwrap();
        } else if written {
            writeln!(out, "[{name}]").unwrap();
        }
        for (key_name, child) in values {
            write!(out, "{} = ", key(key_name)).unwrap();
            self.literal(child, 0, out)?;
            out.push('\n');
            written = true;
        }
        for (part, child, array) in tables {
            if written {
                out.push('\n');
            }
            let part = key(part);
            let path = if name.is_empty() {
                part
            } else {
                format!("{name}.{part}")
            };
            self.table(child, &path, array, out)?;
            written = true;
        }
        Ok(())
    }

    fn literal(&self, index: usize, depth: usize, out: &mut String) -> Result<(), Error> {
        match &self.nodes[index] {
            Node::Null => {
                return Err(Error::new(
                    Kind::TypeError,
                    "Object of type 'NoneType' is not TOML serializable",
                ));
            }
            Node::Bool(v) => out.push_str(if *v { "true" } else { "false" }),
            Node::Integer(i) => {
                if let Some(value) = i.as_small() {
                    write!(out, "{value}").unwrap();
                } else {
                    write!(out, "{}", i.big()).unwrap();
                }
            }
            Node::Float(f) => out.push_str(&crate::floats::render(*f)),
            Node::String(s) => out.push_str(&quoted(s)),
            Node::Array(items) => {
                out.push('[');
                if !items.is_empty() {
                    out.push('\n');
                    for &item in items {
                        for _ in 0..=depth {
                            out.push_str("    ");
                        }
                        self.literal(item, depth + 1, out)?;
                        out.push_str(",\n");
                    }
                    for _ in 0..depth {
                        out.push_str("    ");
                    }
                }
                out.push(']');
            }
            object @ Node::Object(_) => {
                out.push('{');
                let mut empty = true;
                for (k, child) in entries(object) {
                    if empty {
                        out.push(' ');
                    } else {
                        out.push_str(", ");
                    }
                    write!(out, "{} = ", key(k)).unwrap();
                    self.literal(child, 0, out)?;
                    empty = false;
                }
                if !empty {
                    out.push(' ');
                }
                out.push('}');
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Nodes, push_node};
    use super::*;
    use std::rc::Rc;

    fn render(json: &str) -> Result<String, Error> {
        let mut nodes = Vec::new();
        let raw: &serde_json::value::RawValue = serde_json::from_str(json).unwrap();
        let index = push_node(&mut nodes, raw).unwrap();
        let value = View {
            nodes: Nodes::Document(Rc::new(nodes)),
            index,
        };
        let mut output = String::new();
        value.toml_table("outer", &mut output)?;
        Ok(output)
    }

    #[test]
    fn default_toml_writer_preserves_order_escaping_and_array_table_selection() {
        let json = r#"{"bare":false,"t.x":{"alpha":"\u0001\t\r\n","array":[[1,2],[]]},"tables":[{"a":1},{"b":"ok"}],"bare":true}"#;
        assert_eq!(
            render(json).ok().unwrap(),
            "[outer]\nbare = true\ntables = [\n    { a = 1 },\n    { b = \"ok\" },\n]\n\n[outer.\"t.x\"]\nalpha = \"\\u0001\t\\r\\n\"\narray = [\n    [\n        1,\n        2,\n    ],\n    [],\n]\n"
        );
        let wide = "v".repeat(110);
        let json = format!(r#"{{"children":[{{"k":"{wide}"}},{{"k":"é"}}]}}"#);
        assert_eq!(
            render(&json).ok().unwrap(),
            format!("[[outer.children]]\nk = \"{wide}\"\n\n[[outer.children]]\nk = \"é\"\n")
        );
        assert!(render(r#"{"value":null}"#).is_err());
    }
}
