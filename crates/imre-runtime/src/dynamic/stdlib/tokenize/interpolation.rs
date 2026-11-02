//! Track replacement fields to restore CPython's empty format-specifier tokens.
use super::K;

pub(super) struct Interpolation {
    pub(super) start: usize,
    pub(super) middle: K,
    pub(super) triple: bool,
    depth: usize,
    // For each replacement field, the depth just inside '{' and whether it saw ':'.
    fields: Vec<(usize, bool)>,
}

impl Interpolation {
    pub(super) fn new(start: usize, kind: K, depth: usize, triple: bool) -> Self {
        let middle = if kind == K::FStringStart {
            K::FStringMiddle
        } else {
            K::TStringMiddle
        };
        Self {
            start,
            middle,
            triple,
            depth,
            fields: Vec::new(),
        }
    }

    pub(super) fn name(&self) -> &'static str {
        if self.middle == K::FStringMiddle {
            "f-string"
        } else {
            "t-string"
        }
    }

    pub(super) fn in_format(&self, depth: usize) -> bool {
        self.fields.last() == Some(&(depth, true))
    }

    pub(super) fn empty_middle(
        &mut self,
        kind: K,
        depth: usize,
        previous: K,
        doubled: bool,
    ) -> bool {
        let format = self.in_format(depth);
        match kind {
            K::Lbrace if depth == self.depth || format => {
                self.fields.push((depth + 1, false));
                format && doubled && previous != self.middle
            }
            K::Rbrace if self.fields.last().is_some_and(|f| f.0 == depth) => {
                self.fields.pop();
                format && previous != self.middle
            }
            K::Colon => {
                if let Some((level, in_format)) = self.fields.last_mut()
                    && *level == depth
                {
                    *in_format = true;
                }
                false
            }
            _ => false,
        }
    }
}
