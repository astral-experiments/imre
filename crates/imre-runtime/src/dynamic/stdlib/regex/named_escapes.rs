//! Preserve source positions when replacing Python's named escapes with hexadecimal literals.
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
) -> std::result::Result<Option<Spelling>, (String, usize)> {
    if !text.contains(r"\N") {
        return Ok(None);
    }
    let leading = flags::parse(text.as_bytes(), flags, bytes);
    let skip = text[..leading.skip].chars().count();
    let names = imre_regex::named_escapes(
        &text[leading.skip..],
        leading.bits & VERBOSE != 0,
        alphabet(bytes, bytes || leading.bits & ASCII != 0, leading.bits),
        leading.bits & DOTALL != 0,
        flags::scoped_verbose,
        group_names::valid,
    );
    if let Some((error, start)) = names.error {
        let at = skip + start;
        return Err(match error {
            imre_regex::NamedEscapeError::Syntax(message, offset) => (message.into(), at + offset),
            imre_regex::NamedEscapeError::Undefined(name) => {
                let repr = crate::objects::exceptions::string_repr(&name);
                (format!("undefined character name {repr}"), at)
            }
        });
    }
    if names.scalars.is_empty() {
        return Ok(None);
    }
    let mut source = String::with_capacity(text.len());
    let mut ends = Vec::with_capacity(names.scalars.len());
    let mut input = text.chars();
    let (mut original, mut after) = (0, 0);
    for (start, end, scalar) in names.scalars {
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
