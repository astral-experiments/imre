//! Backtrack over Python assertions and capture references while preserving the public slots.
use std::borrow::Cow;
use std::cell::OnceCell;

use fancy_regex::{BytesMode, CompileError, Error, Regex, RegexBuilder};
use regex_automata::util::captures::{Captures, GroupInfo};
use regex_automata::util::primitives::NonMaxUsize;
use regex_automata::{Anchored, Input, PatternID};

use super::{
    ASCII, DOTALL, IGNORECASE, MULTILINE, Matcher, VERBOSE, alphabet, group_names, locale_dependent,
};

/// Compile only grammar accepted by the Python lexer. Flags apply to assertions and their suffix.
pub(super) fn compile(
    pattern: &str,
    flags: i64,
    bytes: bool,
    original: String,
) -> Result<(String, Matcher), String> {
    let ascii = bytes || flags & ASCII != 0;
    let locale_case = locale_dependent(pattern, flags);
    let fold = if locale_case {
        #[cfg(target_os = "linux")]
        {
            // CPython tests a negated locale set separately against lower and upper.
            // Word membership and boundaries also need the locale's own classifier.
            if !bytes
                || flags & IGNORECASE == 0
                || pattern.contains("[^")
                || locale_dependent(pattern, super::LOCALE)
            {
                return Err(original);
            }
            ByteFold::Locale
        }
        #[cfg(not(target_os = "linux"))]
        return Err(original);
    } else if bytes && flags & IGNORECASE != 0 {
        ByteFold::Ascii
    } else {
        ByteFold::None
    };
    let Ok((mut source, groups)) = imre_regex::backtracking_source(
        pattern,
        flags & VERBOSE != 0,
        alphabet(bytes, ascii, flags),
        flags & DOTALL != 0,
    ) else {
        return Err(original);
    };
    // The Python tokenizer emits \k<N> only for a real reference. Literal
    // backslashes are hex-escaped, including within assertions or classes.
    if locale_case && !source.contains(r"\k<") {
        return Err(original);
    }
    // Locale matching folds the subject at run time, while the compiled VM
    // pattern only folds ASCII. A high-byte literal may need a locale mapping.
    if locale_case
        && source
            .split(r"\x")
            .skip(1)
            .any(|tail| matches!(tail.as_bytes().first(), Some(b'8'..=b'9' | b'a'..=b'f')))
    {
        return Err(original);
    }
    let dollar = if flags & MULTILINE != 0 {
        r"(?=\n|\z)"
    } else {
        r"(?=\n?\z)"
    };
    source = source.replace('$', dollar);
    let mut modes = String::new();
    if flags & IGNORECASE != 0 && (bytes || !ascii) {
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
        .find(|name| !group_names::valid(name, bytes))
    {
        return Err(group_names::bad_name(name, bytes));
    }
    let groups = GroupInfo::new([groups]).map_err(|e| e.to_string())?;
    let mode = if bytes {
        BytesMode::Ascii
    } else {
        BytesMode::Unicode
    };
    let matcher = Lookahead::new(&source, groups, mode, fold)?;
    Ok((source, Matcher::Lookahead(Box::new(matcher))))
}

#[derive(Clone, Copy)]
enum ByteFold {
    None,
    Ascii,
    #[cfg(target_os = "linux")]
    Locale,
}

impl ByteFold {
    fn subject(self, input: &[u8]) -> Result<Cow<'_, [u8]>, String> {
        match self {
            #[cfg(target_os = "linux")]
            Self::Locale => {
                // The compiled expression contains ASCII literals and classes. They
                // retain their meaning as long as locale folding never crosses the
                // ASCII boundary. Backreferences compare the real locale lowercase.
                let lower = crate::sys::with_process_locale(|| {
                    let mut table = [0u8; 256];
                    for byte in 0..=u8::MAX {
                        // SAFETY: both ctype calls accept an unsigned char promoted to int.
                        let (lo, hi) = unsafe {
                            (
                                libc::tolower(i32::from(byte)) as u8,
                                libc::toupper(i32::from(byte)) as u8,
                            )
                        };
                        if byte.is_ascii() {
                            if lo != byte.to_ascii_lowercase() || hi != byte.to_ascii_uppercase() {
                                return None;
                            }
                        } else if lo.is_ascii() || hi.is_ascii() {
                            return None;
                        }
                        table[usize::from(byte)] = lo;
                    }
                    Some(table)
                })
                .ok_or("unsupported case mapping for byte backreferences in the current locale")?;
                if input.iter().any(|&b| lower[usize::from(b)] != b) {
                    Ok(Cow::Owned(
                        input.iter().map(|&b| lower[usize::from(b)]).collect(),
                    ))
                } else {
                    Ok(Cow::Borrowed(input))
                }
            }
            Self::Ascii if input.iter().any(u8::is_ascii_uppercase) => {
                Ok(Cow::Owned(input.to_ascii_lowercase()))
            }
            _ => Ok(Cow::Borrowed(input)),
        }
    }
}

pub(super) struct Lookahead {
    search: Regex,
    mode: BytesMode,
    fold: ByteFold,
    anchored: OnceCell<Result<Regex, Error>>,
    nonempty: OnceCell<Result<Regex, Error>>,
    pub(super) groups: GroupInfo,
}

impl Lookahead {
    fn new(
        source: &str,
        groups: GroupInfo,
        mode: BytesMode,
        fold: ByteFold,
    ) -> Result<Self, String> {
        let search = RegexBuilder::new(source)
            .bytes_mode(mode)
            .build()
            .map_err(|e| e.to_string())?;
        if search.captures_len() != groups.group_len(PatternID::ZERO) {
            return Err("backtracking would change the Python capture groups".into());
        }
        Ok(Self {
            search,
            mode,
            fold,
            groups,
            anchored: OnceCell::new(),
            nonempty: OnceCell::new(),
        })
    }

    pub(super) fn with_source(&self, source: &str) -> Result<Self, String> {
        Self::new(source, self.groups.clone(), self.mode, self.fold)
    }

    pub(super) fn total_close_order(&self) -> bool {
        self.search.captures_len() <= 2 || self.close_order().is_some()
    }

    /// On a winning path without re-entry, participating captures close in
    /// syntax-tree postorder, even when a lookaround crosses a sibling's span.
    pub(super) fn close_order(&self) -> Option<Vec<usize>> {
        use fancy_regex::Expr;
        fn visit(expr: &Expr, count: &mut usize, order: &mut Vec<usize>) -> Option<()> {
            let before = *count;
            let group = matches!(expr, Expr::Group(_)).then(|| {
                *count += 1;
                *count
            });
            for child in expr.children_iter() {
                visit(child, count, order)?;
            }
            // A captured operand closes after its descendants on every iteration.
            if *count != before
                && matches!(expr, Expr::Repeat { child, hi, .. }
                if *hi > 1 && !matches!(child.as_ref(), Expr::Group(_)))
            {
                return None;
            }
            order.extend(group);
            Some(())
        }
        let tree = Expr::parse_tree(self.search.as_str()).ok()?;
        if tree.contains_subroutines {
            return None;
        }
        let mut order = Vec::new();
        let mut count = 0;
        visit(&tree.expr, &mut count, &mut order)?;
        (count + 1 == self.search.captures_len()).then_some(order)
    }

    /// Preserve the prefix for anchors, and make endpos the assertion's end of input.
    pub(super) fn captures(
        &self,
        input: Input<'_>,
        nonempty: bool,
        captures: &mut Captures,
    ) -> Result<(), String> {
        captures.clear();
        // Captures still index the original subject one byte per folded byte.
        let haystack = self.fold.subject(&input.haystack()[..input.end()])?;
        let regex = if nonempty || input.get_anchored() == Anchored::Yes {
            let cache = if nonempty {
                &self.nonempty
            } else {
                &self.anchored
            };
            match cache.get_or_init(|| {
                RegexBuilder::new(&format!(r"\G(?:{})", self.search.as_str()))
                    .bytes_mode(self.mode)
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
        if let Some(found) = regex
            .captures_from_pos(haystack.as_ref(), input.start())
            .map_err(|e| e.to_string())?
        {
            captures.set_pattern(Some(PatternID::ZERO));
            let (slots, _) = captures.slots_mut().as_chunks_mut::<2>();
            for (slot, group) in slots.iter_mut().zip(found.iter()) {
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
