//! Allocate a contiguous run of positional values in declaration order.

use serde::Deserialize;

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub(super) enum Nargs {
    #[default]
    One,
    Optional,
    ZeroOrMore,
    OneOrMore,
    Fixed(usize),
}

impl Nargs {
    pub(super) fn bounds(self) -> (usize, usize) {
        match self {
            Self::One => (1, 1),
            Self::Optional => (0, 1),
            Self::ZeroOrMore => (0, usize::MAX),
            Self::OneOrMore => (1, usize::MAX),
            Self::Fixed(n) => (n, n),
        }
    }

    pub(super) fn list(self) -> bool {
        matches!(self, Self::ZeroOrMore | Self::OneOrMore | Self::Fixed(_))
    }

    pub(super) fn expected(self) -> String {
        match self {
            Self::One => "expected one argument".into(),
            Self::OneOrMore => "expected at least one argument".into(),
            Self::Fixed(1) => "expected 1 argument".into(),
            Self::Fixed(n) => format!("expected {n} arguments"),
            Self::Optional | Self::ZeroOrMore => unreachable!(),
        }
    }

    pub(super) fn label(self, metavar: &str) -> String {
        match self {
            Self::One => metavar.into(),
            Self::Optional => format!("[{metavar}]"),
            Self::ZeroOrMore => format!("[{metavar} ...]"),
            Self::OneOrMore => format!("{metavar} [{metavar} ...]"),
            Self::Fixed(n) => std::iter::repeat_n(metavar, n)
                .collect::<Vec<_>>()
                .join(" "),
        }
    }

    pub(super) fn spelling(self) -> String {
        match self {
            Self::One => "None".into(),
            Self::Optional => "?".into(),
            Self::ZeroOrMore => "*".into(),
            Self::OneOrMore => "+".into(),
            Self::Fixed(n) => n.to_string(),
        }
    }
}

/// Match the longest prefix of the remaining declarations, then give each
/// action as many words as it can take while reserving the later minima.
/// Zero-width matches just before an option stay available for the next run.
pub(super) fn positional_counts(
    actions: &[Nargs],
    available: usize,
    followed_by_option: bool,
) -> Vec<usize> {
    let mut remaining = available;
    let mut prefix = 0;
    for action in actions {
        let Some(left) = remaining.checked_sub(action.bounds().0) else {
            break;
        };
        remaining = left;
        prefix += 1;
    }
    let mut counts = Vec::with_capacity(prefix);
    for action in &actions[..prefix] {
        let (min, max) = action.bounds();
        let extra = remaining.min(max - min);
        counts.push(min + extra);
        remaining -= extra;
    }
    if followed_by_option && remaining == 0 {
        counts.truncate(counts.iter().rposition(|&n| n != 0).map_or(0, |i| i + 1));
    }
    counts
}
