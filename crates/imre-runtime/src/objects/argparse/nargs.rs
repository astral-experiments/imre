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
        let mut label = String::new();
        self.write_label(&mut label, metavar).unwrap();
        label
    }

    pub(super) fn write_label(
        self,
        out: &mut impl std::fmt::Write,
        metavar: &str,
    ) -> std::fmt::Result {
        match self {
            Self::One => out.write_str(metavar),
            Self::Optional => write!(out, "[{metavar}]"),
            Self::ZeroOrMore => write!(out, "[{metavar} ...]"),
            Self::OneOrMore => write!(out, "{metavar} [{metavar} ...]"),
            Self::Fixed(count) => {
                for i in 0..count {
                    if i != 0 {
                        out.write_char(' ')?;
                    }
                    out.write_str(metavar)?;
                }
                Ok(())
            }
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
