//! The default tomli_w text format for the JSON subset of TOML values.

use super::{Error, Int, Kind, Node, Str, View};

fn append(out: &mut String, parts: &[&str]) -> Result<(), Error> {
    let extra = parts
        .iter()
        .try_fold(0usize, |size, part| size.checked_add(part.len()))
        .ok_or_else(Error::allocation)?;
    out.try_reserve(extra).map_err(|_| Error::allocation())?;
    for part in parts {
        out.push_str(part);
    }
    Ok(())
}

fn integer(value: &Int, out: &mut String) -> Result<(), Error> {
    let mut buffer = itoa::Buffer::new();
    if let Some(value) = value.as_small() {
        return append(out, &[buffer.format(value)]);
    }

    // Convert borrowed binary limbs into groups of nine decimal digits.
    const BASE: u64 = 1_000_000_000;
    let value = value.big();
    let mut chunks = Vec::<u32>::new();
    for limb in value.iter_u32_digits().rev() {
        let mut carry = u64::from(limb);
        for chunk in &mut chunks {
            let word = (u64::from(*chunk) << 32) + carry;
            *chunk = (word % BASE) as u32;
            carry = word / BASE;
        }
        while carry != 0 {
            push(&mut chunks, (carry % BASE) as u32)?;
            carry /= BASE;
        }
    }

    if value.sign() == num_bigint::Sign::Minus {
        append(out, &["-"])?;
    }
    let mut chunks = chunks.into_iter().rev();
    append(out, &[buffer.format(chunks.next().unwrap_or(0))])?;
    for chunk in chunks {
        let digits = buffer.format(chunk);
        append(out, &[&"000000000"[..9 - digits.len()], digits])?;
    }
    Ok(())
}

fn quoted(value: &str, out: &mut String) -> Result<(), Error> {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    append(out, &["\""])?;
    let mut start = 0;
    let mut unicode = *b"\\u0000";
    for (i, byte) in value.bytes().enumerate() {
        let escape = match byte {
            8 => "\\b",
            b'\n' => "\\n",
            12 => "\\f",
            b'\r' => "\\r",
            b'"' => "\\\"",
            b'\\' => "\\\\",
            b'\t' => continue,
            0..=31 | 127 => {
                unicode[4] = HEX[usize::from(byte >> 4)];
                unicode[5] = HEX[usize::from(byte & 15)];
                std::str::from_utf8(&unicode).unwrap()
            }
            _ => continue,
        };
        // Every replacement starts and ends on an ASCII byte.
        append(out, &[&value[start..i], escape])?;
        start = i + 1;
    }
    append(out, &[&value[start..], "\""])
}

fn key(value: &str, out: &mut String) -> Result<(), Error> {
    if !value.is_empty()
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_'))
    {
        append(out, &[value])
    } else {
        quoted(value, out)
    }
}

fn push<T>(items: &mut Vec<T>, item: T) -> Result<(), Error> {
    items.try_reserve(1).map_err(|_| Error::allocation())?;
    items.push(item);
    Ok(())
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
        if self.nodes.deep() {
            super::deep_values::check_toml_depth(self)?;
        }
        let mut path = String::new();
        key(name, &mut path)?;
        self.table(self.index, &path, false, output)
    }

    pub(super) fn table(
        &self,
        index: usize,
        name: &str,
        array: bool,
        out: &mut String,
    ) -> Result<(), Error> {
        let mut values = Vec::new();
        let mut tables = Vec::new();
        for (name, child) in entries(&self.nodes[index]) {
            let (view, child) = self.try_resolved(child)?;
            match &view.nodes[child] {
                Node::Object(_) => push(&mut tables, (name, view, child, false))?,
                Node::Array(items)
                    if !items.is_empty()
                        && items
                            .iter()
                            .all(|&i| matches!(view.nodes[i], Node::Object(_))) =>
                {
                    let mut inline = true;
                    for &item in items {
                        let mut rendered = String::new();
                        view.literal(item, 0, &mut rendered)?;
                        if rendered.chars().count() + 5 > 100 || rendered.contains('\n') {
                            inline = false;
                            break;
                        }
                    }
                    if inline {
                        push(&mut values, (name, view, child))?;
                    } else {
                        tables
                            .try_reserve(items.len())
                            .map_err(|_| Error::allocation())?;
                        tables.extend(items.iter().map(|&item| (name, view, item, true)));
                    }
                }
                _ => push(&mut values, (name, view, child))?,
            }
        }
        let mut written = array || !name.is_empty() && (!values.is_empty() || tables.is_empty());
        if array {
            append(out, &["[[", name, "]]\n"])?;
        } else if written {
            append(out, &["[", name, "]\n"])?;
        }
        for (key_name, view, child) in values {
            let start = out.len();
            // dump() writes completed lines even when a later value fails.
            if let Err(error) = view.assignment(key_name, child, out) {
                out.truncate(start);
                return Err(error);
            }
            written = true;
        }
        for (part, view, child, array) in tables {
            if written {
                append(out, &["\n"])?;
            }
            let mut path = String::new();
            if !name.is_empty() {
                append(&mut path, &[name, "."])?;
            }
            key(part, &mut path)?;
            view.table(child, &path, array, out)?;
            written = true;
        }
        Ok(())
    }

    fn assignment(&self, name: &str, index: usize, out: &mut String) -> Result<(), Error> {
        key(name, out)?;
        append(out, &[" = "])?;
        self.literal(index, 0, out)?;
        append(out, &["\n"])
    }

    fn literal(&self, index: usize, depth: usize, out: &mut String) -> Result<(), Error> {
        match &self.nodes[index] {
            Node::Deferred(array) => {
                let view = array.try_view()?;
                return view.literal(view.index, depth, out);
            }
            Node::Null => {
                return Err(Error::new(
                    Kind::TypeError,
                    "Object of type 'NoneType' is not TOML serializable",
                ));
            }
            Node::Bool(v) => append(out, &[if *v { "true" } else { "false" }])?,
            Node::Integer(i) => integer(i, out)?,
            Node::Float(f) => append(out, &[&crate::floats::render(*f)])?,
            Node::ConstantNan => append(out, &["nan"])?,
            Node::String(s) => quoted(s, out)?,
            Node::Datetime(date) => append(out, &[date])?,
            Node::Array(items) => {
                append(out, &["["])?;
                if !items.is_empty() {
                    append(out, &["\n"])?;
                    for &item in items {
                        for _ in 0..=depth {
                            append(out, &["    "])?;
                        }
                        self.literal(item, depth + 1, out)?;
                        append(out, &[",\n"])?;
                    }
                    for _ in 0..depth {
                        append(out, &["    "])?;
                    }
                }
                append(out, &["]"])?;
            }
            object @ Node::Object(_) => {
                append(out, &["{"])?;
                let mut empty = true;
                for (k, child) in entries(object) {
                    if empty {
                        append(out, &[" "])?;
                    } else {
                        append(out, &[", "])?;
                    }
                    key(k, out)?;
                    append(out, &[" = "])?;
                    self.literal(child, 0, out)?;
                    empty = false;
                }
                if !empty {
                    append(out, &[" "])?;
                }
                append(out, &["}"])?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Nodes, Source, push_node};
    use super::*;
    use std::rc::Rc;

    fn view(json: &str) -> View {
        let mut nodes = Vec::new();
        let raw: &serde_json::value::RawValue = serde_json::from_str(json).unwrap();
        let index = push_node(
            &mut nodes,
            raw,
            Source {
                refused: &std::cell::Cell::new(false),
                text: json,
                original: None,
                shared: None,
                keys: &super::super::KeyMemo::default(),
            },
        )
        .unwrap();
        View {
            nodes: Nodes::Document(Rc::new(nodes)),
            index,
        }
    }

    fn render(json: &str) -> Result<String, Error> {
        let mut output = String::new();
        view(json).toml_table("outer", &mut output)?;
        Ok(output)
    }

    #[test]
    fn failed_assignment_preserves_completed_lines() {
        let mut output = String::new();
        let doc = view(r#"{"ok":1,"broken":[1,null]}"#);
        assert!(doc.toml_table("outer", &mut output).is_err());
        assert_eq!(output, "[outer]\nok = 1\n");
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
        // The same array-table decision must hold across an eagerly parsed or deferred span.
        let rows = vec![format!(r#"{{"k":"{wide}"}}"#); 40].join(",");
        let json = format!(r#"{{"children":[{rows}]}}"#);
        assert_eq!(
            render(&json).ok().unwrap(),
            vec![format!("[[outer.children]]\nk = \"{wide}\"\n"); 40].join("\n"),
        );
        assert!(render(r#"{"value":null}"#).is_err());
    }
}
