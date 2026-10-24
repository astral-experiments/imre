//! Large nested arrays used only for lookup or truth must not allocate a node
//! for every descendant. Keep their validated span and expand it on demand.

use std::cell::OnceCell;
use std::ops::Range;
use std::rc::Rc;

use serde_json::value::RawValue;

use super::{Source, View, constants, elements, push_array};

pub(in crate::objects::json) struct Deferred {
    text: Rc<str>,
    range: Range<usize>,
    parsed: OnceCell<View>,
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
        }))
    }

    pub(super) fn truth(&self) -> bool {
        !self.text[self.range.clone()][1..]
            .trim_start()
            .starts_with(']')
    }

    pub(in crate::objects::json) fn view(&self) -> &View {
        self.parsed.get_or_init(|| {
            let original = &self.text[self.range.clone()];
            let parse = |text: &str, unmodified| {
                let mut nodes = Vec::new();
                let source = Source {
                    text,
                    original: unmodified,
                    shared: Some((&self.text, self.range.start)),
                };
                let node = match elements::parse(text, &mut nodes) {
                    Some(array) => array,
                    None => push_array(&mut nodes, text, source)?,
                };
                let index = nodes.len();
                nodes.push(node);
                Ok::<_, serde_json::Error>(View {
                    nodes: Rc::new(nodes),
                    root_items: None,
                    index,
                })
            };
            parse(original, None)
                .or_else(|error| {
                    let patched = constants::normalize(original, &error).ok_or(error)?;
                    parse(&patched, Some(original))
                })
                .expect("the complete document and these Unicode scalars were validated")
        })
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
            text: &input,
            original: None,
            shared: None,
        };
        let raw = Deferred::new(&input, source).unwrap();
        assert!(raw.truth());
        assert!(raw.parsed.get().is_none());
        let outer = raw.view();
        let Node::Array(items) = &outer.nodes[outer.index] else {
            panic!()
        };
        let Node::Deferred(inner) = &outer.nodes[items[0]] else {
            panic!()
        };
        assert!(Rc::ptr_eq(&raw.text, &inner.text));
        assert!(inner.parsed.get().is_none());
        let inner = inner.view();
        let Node::Array(items) = &inner.nodes[inner.index] else {
            panic!()
        };
        assert!(matches!(&inner.nodes[items[3000]], Node::String(s) if s.as_str() == "𝄠"));
        assert!(matches!(&inner.nodes[items[3001]], Node::Float(n) if *n == f64::INFINITY));
        assert!(!unicode_scalars(r#"["\ud834"]"#));
    }
}
