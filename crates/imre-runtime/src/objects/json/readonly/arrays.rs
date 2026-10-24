//! Large nested arrays used only for lookup or truth must not allocate a node
//! for every descendant. Keep their validated span and expand it on demand.

use std::cell::{Cell, OnceCell};
use std::ops::Range;
use std::rc::Rc;

use serde_json::value::RawValue;

use super::{Error, Nodes, Source, View, constants, elements, push, push_array};

pub(in crate::objects::json) struct Deferred {
    text: Rc<str>,
    range: Range<usize>,
    parsed: OnceCell<View>,
    length: OnceCell<usize>,
}

impl Deferred {
    pub(super) fn new(raw: &str, source: Source<'_>) -> Option<Rc<Self>> {
        // Sealing small arrays immediately avoids an extra owner on ordinary rows.
        if raw.len() < 4096 {
            return None;
        }
        let original = source.token(raw);
        let (text, start) = if let Some((owner, start)) = source.shared {
            (
                Rc::clone(owner),
                start + (raw.as_ptr() as usize - source.text.as_ptr() as usize),
            )
        } else {
            if !unicode_scalars(original) {
                return None;
            }
            (Rc::<str>::from(original), 0)
        };
        Some(Rc::new(Self {
            text,
            range: start..start + raw.len(),
            parsed: OnceCell::new(),
            length: OnceCell::new(),
        }))
    }

    pub(super) fn truth(&self) -> bool {
        !self.text[self.range.clone()][1..]
            .trim_start()
            .starts_with(']')
    }

    pub(super) fn len(&self) -> usize {
        *self.length.get_or_init(|| {
            if !self.truth() {
                return 0;
            }
            // The span has already been validated, including Python's non-finite tokens.
            let mut count = 1;
            let mut depth = 0usize;
            let mut quoted = false;
            let mut bytes = self.text[self.range.clone()].bytes().skip(1);
            while let Some(byte) = bytes.next() {
                match byte {
                    b'\\' if quoted => {
                        bytes.next();
                    }
                    b'"' => quoted = !quoted,
                    b'[' | b'{' if !quoted => depth += 1,
                    b']' | b'}' if !quoted && depth != 0 => depth -= 1,
                    b',' if !quoted && depth == 0 => count += 1,
                    _ => {}
                }
            }
            count
        })
    }

    pub(in crate::objects::json) fn try_view(&self) -> Result<&View, Error> {
        if let Some(view) = self.parsed.get() {
            return Ok(view);
        }
        let original = &self.text[self.range.clone()];
        let refused = Cell::new(false);
        let parse = |text: &str, unmodified| {
            let mut nodes = Vec::new();
            let source = Source {
                text,
                original: unmodified,
                shared: Some((&self.text, self.range.start)),
                refused: &refused,
            };
            // Escaped strings use the bounded decoder on their validated raw spans.
            let flat = if text.as_bytes().contains(&b'\\') {
                None
            } else {
                elements::parse(text, &mut nodes, &refused)
            };
            let node = match flat {
                Some(array) => array,
                None if refused.get() => {
                    return Err(super::allocation_error::<serde_json::Error>(&refused));
                }
                None => push_array(&mut nodes, text, source)?,
            };
            let index = nodes.len();
            push(&mut nodes, node, &refused)?;
            Ok::<_, serde_json::Error>(View {
                nodes: Nodes::Document(Rc::new(nodes)),
                index,
            })
        };
        let result = match parse(original, None) {
            Err(error) if !refused.get() => match constants::normalize(original, &error)? {
                Some(patched) => parse(&patched, Some(original)),
                None => Err(error),
            },
            other => other,
        };
        if refused.get() {
            return Err(Error::allocation());
        }
        let view = result.expect("the complete document and its Unicode scalars were validated");
        Ok(self.parsed.get_or_init(|| view))
    }

    pub(in crate::objects::json) fn view(&self) -> &View {
        self.try_view()
            .ok()
            .expect("document view allocation failed")
    }
}

/// RawValue validates escapes but leaves UTF-16 surrogate pairing to string
/// decoding. Defer only a representable span; the existing typed parse gives
/// its original error if a string contains an unpaired surrogate.
fn unicode_scalars(mut text: &str) -> bool {
    let possible = |text: &str| text.contains("\\ud") || text.contains("\\uD");
    if !possible(text) {
        return true;
    }
    while let Some(at) = memchr::memchr(b'"', text.as_bytes()) {
        let Some(Ok(token)) = serde_json::Deserializer::from_str(&text[at..])
            .into_iter::<&RawValue>()
            .next()
        else {
            return false;
        };
        let word = token.get();
        if possible(word) && serde_json::from_str::<String>(word).is_err() {
            return false;
        }
        text = &text[at + word.len()..];
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::objects::json::readonly::Node;

    #[test]
    fn unused_arrays_remain_raw_and_nested_reads_share_the_validated_span() {
        let input = format!(r#"[[{}"\ud834\udd20",Infinity]]"#, "0,".repeat(3000));
        let source = Source {
            refused: &Cell::new(false),
            text: &input,
            original: None,
            shared: None,
        };
        let raw = Deferred::new(&input, source).unwrap();
        assert!(raw.truth());
        assert_eq!(raw.len(), 1);
        assert!(raw.parsed.get().is_none());
        let outer = raw.view();
        let Node::Array(items) = &outer.nodes[outer.index] else {
            panic!()
        };
        let Node::Deferred(inner) = &outer.nodes[items[0]] else {
            panic!()
        };
        assert!(Rc::ptr_eq(&raw.text, &inner.text));
        assert_eq!(inner.len(), 3002);
        assert!(inner.parsed.get().is_none());
        let inner = inner.view();
        let Node::Array(items) = &inner.nodes[inner.index] else {
            panic!()
        };
        assert!(matches!(&inner.nodes[items[3000]], Node::String(s) if s.as_str() == "𝄠"));
        assert!(matches!(&inner.nodes[items[3001]], Node::Float(n) if *n == f64::INFINITY));
        assert!(!unicode_scalars(r#"["\ud834"]"#));
    }

    #[test]
    fn raw_length_ignores_quoted_and_nested_separators() {
        for (body, count) in [
            ("", 0),
            (r#"{"\ud834\udd20": [1, 2]}, ",\"\\]}", [[3,4]], NaN"#, 4),
        ] {
            let text = format!("[{}{body}]", " ".repeat(4096));
            let raw = Deferred::new(
                &text,
                Source {
                    text: &text,
                    original: None,
                    shared: None,
                    refused: &Cell::new(false),
                },
            )
            .unwrap();
            assert_eq!(raw.len(), count);
            assert!(raw.parsed.get().is_none());
            let view = raw.try_view().ok().unwrap();
            let Node::Array(items) = &view.nodes[view.index] else {
                panic!();
            };
            assert_eq!(items.len(), count);
        }
    }
}
