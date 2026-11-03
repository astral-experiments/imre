//! Validate Python pattern escapes and preserve positions when replacing a named scalar.
use super::*;

pub(super) struct Spelling {
    pub text: String,
    // End of each replacement in the normalized and original spellings, in characters.
    ends: Vec<(usize, usize)>,
}

impl Spelling {
    pub fn position(&self, at: usize) -> usize {
        let count = self.ends.partition_point(|&(after, _)| after <= at);
        let (after, original) = self.ends[..count].last().copied().unwrap_or_default();
        original + (at - after)
    }
}

pub(super) fn normalize(
    text: &str,
    bytes: bool,
    flags: i64,
    diagnose_syntax: bool,
    points: Option<&[u32]>,
) -> std::result::Result<Option<Spelling>, (String, usize)> {
    if !text.as_bytes().windows(2).any(|pair| match pair {
        [b'\\', b'N' | b'x' | b'u' | b'U' | b'0'..=b'9'] => true,
        [b'(', b'?'] | [b'{', b'0'..=b'9'] => diagnose_syntax,
        _ => false,
    }) {
        return Ok(None);
    }
    let leading = flags::parse(text.as_bytes(), flags, bytes);
    let skip = text[..leading.skip].chars().count();
    let escapes = imre_regex::pattern_escapes(
        &text[leading.skip..],
        leading.bits & VERBOSE != 0,
        alphabet(bytes, bytes || leading.bits & ASCII != 0, leading.bits),
        leading.bits & DOTALL != 0,
        flags::scoped_verbose,
        group_names::valid,
    );
    if let Some((error, start)) = escapes.error {
        let at = skip + start;
        return Err(match error {
            imre_regex::EscapeError::Syntax(message, offset) => (message, at + offset),
            imre_regex::EscapeError::UnknownGroup(name) => {
                let repr = crate::objects::exceptions::string_repr(&name);
                (format!("unknown group name {repr}"), at)
            }
            imre_regex::EscapeError::InvalidGroupName(name) => {
                (group_names::bad_source_name(&name, bytes, points, at), at)
            }
            imre_regex::EscapeError::Undefined(name) => {
                let repr = crate::objects::exceptions::string_repr(&name);
                (format!("undefined character name {repr}"), at)
            }
        });
    }
    if diagnose_syntax && let Some(open) = escapes.rejected_header {
        let at = skip + open;
        let byte = text.char_indices().nth(at).unwrap().0;
        if let Some((message, offset)) = flags::header_error(&text[byte..], bytes) {
            return Err((message, at + offset));
        }
    }
    if escapes.named_scalars.is_empty() {
        return Ok(None);
    }
    let mut source = String::with_capacity(text.len());
    let mut ends = Vec::with_capacity(escapes.named_scalars.len());
    let mut input = text.chars();
    let (mut original, mut after) = (0, 0);
    for (start, end, scalar) in escapes.named_scalars {
        let plain = skip + start - original;
        source.extend(input.by_ref().take(plain));
        for _ in start..end {
            input.next();
        }
        let point = u32::from(scalar);
        let before = source.len();
        if point <= 0xffff {
            write!(source, r"\u{point:04x}").unwrap();
        } else {
            write!(source, r"\U{point:08x}").unwrap();
        }
        after += plain + source.len() - before;
        original = skip + end;
        ends.push((after, original));
    }
    source.extend(input);
    Ok(Some(Spelling { text: source, ends }))
}
