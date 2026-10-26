//! Large nested arrays used only for lookup or truth must not allocate a node
//! for every descendant. Keep their validated span and expand it on demand.

use crate::StrOwner;
use std::cell::{Cell, OnceCell};
use std::ops::Range;
use std::rc::Rc;

use serde_json::value::RawValue;

use super::{Error, KeyMemo, Nodes, Source, View, elements, push, push_array};

pub(in crate::objects::json) struct Deferred {
    text: StrOwner,
    normalized: Option<(Rc<String>, usize)>,
    range: Range<usize>,
    parsed: OnceCell<View>,
    keys: KeyMemo,
    length: OnceCell<usize>,
}

impl Deferred {
    pub(super) fn new(
        raw: &str,
        source: Source<'_>,
    ) -> Result<Option<Rc<Self>>, serde_json::Error> {
        // Sealing small arrays immediately avoids an extra owner on ordinary rows.
        if raw.len() < 4096 {
            return Ok(None);
        }
        let original = source.token(raw);
        let (text, start) = if let Some((owner, start)) = source.shared {
            (
                StrOwner::clone(owner),
                start + (raw.as_ptr() as usize - source.text.as_ptr() as usize),
            )
        } else {
            if !unicode_scalars(original) {
                return Ok(None);
            }
            let text = super::Str::try_new(original)
                .map_err(|_| super::allocation_error(source.refused))?;
            (
                StrOwner::try_new(text)
                    .map_err(|_| super::allocation_error::<serde_json::Error>(source.refused))?,
                0,
            )
        };
        // The parser has already validated the same-width substitutions. Keep the
        // spelling for this span so a later read never has to allocate it again.
        let normalized = source.original.and_then(|(_, patched)| {
            if raw == original {
                return None;
            }
            if source.shared.is_none() && raw.len() < patched.len() / 2 {
                // Avoid pinning a large document for one small surviving array.
                let mut span = String::new();
                if span.try_reserve_exact(raw.len()).is_ok() {
                    span.push_str(raw);
                    return Some((Rc::new(span), 0));
                }
            }
            Some((
                Rc::clone(patched),
                raw.as_ptr() as usize - patched.as_ptr() as usize,
            ))
        });
        Ok(Some(Rc::new(Self {
            text,
            normalized,
            range: start..start + raw.len(),
            parsed: OnceCell::new(),
            length: OnceCell::new(),
            keys: source
                .keys
                .shared()
                .map_err(|_| super::allocation_error::<serde_json::Error>(source.refused))?,
        })))
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
        let parse = |text: &str, unmodified: Option<(&str, &Rc<String>)>| {
            let mut nodes = Vec::new();
            let source = Source {
                text,
                original: unmodified,
                keys: &self.keys,
                shared: Some((&self.text, self.range.start)),
                refused: &refused,
            };
            // Escaped strings use the bounded decoder on their validated raw spans.
            let flat = if text.as_bytes().contains(&b'\\') {
                None
            } else {
                elements::parse(text, &mut nodes, &self.keys, &refused)
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
        let result = match &self.normalized {
            Some((patched, start)) => parse(
                &patched[*start..*start + original.len()],
                Some((original, patched)),
            ),
            None => match parse(original, None) {
                Err(error) if !refused.get() => {
                    match super::constants::normalize(original, &error)? {
                        Some(patched) => {
                            let patched = Rc::new(patched);
                            parse(&patched, Some((original, &patched)))
                        }
                        None => Err(error),
                    }
                }
                other => other,
            },
        };
        if refused.get() {
            return Err(Error::allocation());
        }
        let view = result.expect("the complete document and its Unicode scalars were validated");
        Ok(self.parsed.get_or_init(|| view))
    }

    #[cfg(test)]
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
        let error = serde_json::from_str::<&RawValue>(&input).unwrap_err();
        let patched = Rc::new(
            super::super::constants::normalize(&input, &error)
                .ok()
                .unwrap()
                .unwrap(),
        );
        let source = Source {
            refused: &Cell::new(false),
            text: &patched,
            original: Some((&input, &patched)),
            shared: None,
            keys: &KeyMemo::default(),
        };
        let raw = Deferred::new(&patched, source).unwrap().unwrap();
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
        assert!(StrOwner::ptr_eq(&raw.text, &inner.text));
        assert!(Rc::ptr_eq(
            &raw.normalized.as_ref().unwrap().0,
            &inner.normalized.as_ref().unwrap().0,
        ));
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
                    keys: &KeyMemo::default(),
                    refused: &Cell::new(false),
                },
            )
            .unwrap()
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
