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
    if locale_dependent(pattern, flags) {
        return Err(original);
    }
    let Ok((mut source, groups)) = imre_regex::backtracking_source(
        pattern,
        flags & VERBOSE != 0,
        alphabet(bytes, ascii, flags),
        flags & DOTALL != 0,
    ) else {
        return Err(original);
    };
    // Raw, octal, and hex high-byte literals all translate to \xNN. Even in
    // bytes mode the VM encodes these literals as UTF-8.
    if bytes
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
    let matcher = Lookahead::new(&source, groups, mode, bytes && flags & IGNORECASE != 0)?;
    Ok((source, Matcher::Lookahead(Box::new(matcher))))
}

pub(super) struct Lookahead {
    search: Regex,
    mode: BytesMode,
    fold_bytes: bool,
    anchored: OnceCell<Result<Regex, Error>>,
    nonempty: OnceCell<Result<Regex, Error>>,
    pub(super) groups: GroupInfo,
}

impl Lookahead {
    fn new(
        source: &str,
        groups: GroupInfo,
        mode: BytesMode,
        fold_bytes: bool,
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
            fold_bytes,
            groups,
            anchored: OnceCell::new(),
            nonempty: OnceCell::new(),
        })
    }

    pub(super) fn with_source(&self, source: &str) -> Result<Self, String> {
        Self::new(source, self.groups.clone(), self.mode, self.fold_bytes)
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
            if *count != before && matches!(expr, Expr::Repeat { hi, .. } if *hi > 1) {
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
        let haystack = &input.haystack()[..input.end()];
        // Bytes IGNORECASE folds ASCII only. Folding the subject keeps backreferences
        // exact for captures mixing ASCII and high bytes. Spans still index the original.
        let haystack = if self.fold_bytes && haystack.iter().any(u8::is_ascii_uppercase) {
            Cow::Owned(haystack.to_ascii_lowercase())
        } else {
            Cow::Borrowed(haystack)
        };
        if let Some(found) = regex
            .captures_from_pos(haystack.as_ref(), input.start())
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
