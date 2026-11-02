//! Match translated text lookaheads with their ordinary numbered capture slots.
use std::cell::OnceCell;

use fancy_regex::{CompileError, Error, Regex, RegexBuilder};
use regex_automata::util::captures::{Captures, GroupInfo};
use regex_automata::util::primitives::NonMaxUsize;
use regex_automata::{Anchored, Input, PatternID};

use super::{
    ASCII, DOTALL, IGNORECASE, MULTILINE, Matcher, VERBOSE, alphabet, casefold, group_names,
};

/// Compile only grammar accepted by the Python lexer. Flags apply to assertions and their suffix.
pub(super) fn compile(
    pattern: &str,
    flags: i64,
    original: String,
) -> Result<(String, Matcher), String> {
    if !(pattern.contains("(?=") || pattern.contains("(?!")) {
        return Err(original);
    }
    let ascii = flags & ASCII != 0;
    let (mut source, groups) = imre_regex::lookahead_source(
        pattern,
        flags & VERBOSE != 0,
        alphabet(false, ascii, flags),
        flags & DOTALL != 0,
    )
    .map_err(|_| original)?;
    if flags & IGNORECASE != 0 {
        if source.contains(r"\k<") || source.contains("(?(") {
            return Err("case-insensitive lookahead group references are unsupported".into());
        }
        // Normalized literal punctuation is escaped. Substituting same-width group
        // openers lets the normal case folder read literals at their original offsets.
        let syntax = source
            .replace("(?=", "(?:")
            .replace("(?!", "(?:")
            .replace("(?>", "(?:");
        source = casefold::expand_with_syntax(&source, &syntax, ascii)?;
    }
    let dollar = if flags & MULTILINE != 0 {
        r"(?=\n|\z)"
    } else {
        r"(?=\n?\z)"
    };
    source = source.replace('$', dollar);
    let mut modes = String::new();
    if flags & IGNORECASE != 0 && !ascii {
        modes.push('i');
    }
    if flags & MULTILINE != 0 {
        modes.push('m');
    }
    if flags & DOTALL != 0 {
        modes.push('s');
    }
    if !modes.is_empty() {
        source = format!("(?{modes}:{source})");
    }
    if let Some(name) = groups
        .iter()
        .flatten()
        .find(|name| !group_names::valid(name, false))
    {
        return Err(group_names::bad_name(name, false));
    }
    let groups = GroupInfo::new([groups]).map_err(|e| e.to_string())?;
    let matcher = Lookahead::new(&source, groups)?;
    Ok((source, Matcher::Lookahead(Box::new(matcher))))
}

pub(super) struct Lookahead {
    search: Regex,
    anchored: OnceCell<Result<Regex, Error>>,
    nonempty: OnceCell<Result<Regex, Error>>,
    pub(super) groups: GroupInfo,
}

impl Lookahead {
    pub(super) fn new(source: &str, groups: GroupInfo) -> Result<Self, String> {
        let search = Regex::new(source).map_err(|e| e.to_string())?;
        if search.captures_len() != groups.group_len(PatternID::ZERO) {
            return Err("lookahead would change the Python capture groups".into());
        }
        Ok(Self {
            search,
            groups,
            anchored: OnceCell::new(),
            nonempty: OnceCell::new(),
        })
    }

    /// Preserve the prefix for anchors, and make endpos the assertion's end of input.
    pub(super) fn captures(
        &self,
        input: Input<'_>,
        nonempty: bool,
        captures: &mut Captures,
    ) -> Result<(), String> {
        captures.clear();
        let regex = if nonempty || input.get_anchored() == Anchored::Yes {
            let cache = if nonempty {
                &self.nonempty
            } else {
                &self.anchored
            };
            match cache.get_or_init(|| {
                RegexBuilder::new(&format!(r"\G(?:{})", self.search.as_str()))
                    .find_not_empty(nonempty)
                    .build()
            }) {
                Ok(regex) => regex,
                Err(Error::CompileError(err))
                    if nonempty && matches!(**err, CompileError::PatternCanNeverMatch) =>
                {
                    return Ok(());
                }
                Err(err) => return Err(err.to_string()),
            }
        } else {
            &self.search
        };
        let text = std::str::from_utf8(&input.haystack()[..input.end()])
            .expect("lookahead is only compiled for text patterns");
        if let Some(found) = regex
            .captures_from_pos(text, input.start())
            .map_err(|e| e.to_string())?
        {
            captures.set_pattern(Some(PatternID::ZERO));
            for (slot, group) in captures.slots_mut().chunks_exact_mut(2).zip(found.iter()) {
                if let Some(m) = group {
                    slot[0] = NonMaxUsize::new(m.start());
                    slot[1] = NonMaxUsize::new(m.end());
                } else {
                    slot.fill(None);
                }
            }
        }
        Ok(())
    }
}
