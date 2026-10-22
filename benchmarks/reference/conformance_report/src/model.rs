use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;
use std::sync::Arc;

use num_bigint::BigInt;
use serde::{Deserialize, Deserializer};

/// Positions retain Python's arbitrary integer range, with native storage for
/// the ordinary small coordinates emitted by ty.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub enum Integer {
    Small(i64),
    Big(BigInt),
}

impl<'de> Deserialize<'de> for Integer {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let number = serde_json::Number::deserialize(deserializer)?;
        if let Some(value) = number.as_i64() {
            return Ok(Self::Small(value));
        }
        let spelling = number.to_string();
        if !spelling
            .bytes()
            .enumerate()
            .all(|(index, byte)| byte.is_ascii_digit() || (index == 0 && byte == b'-'))
        {
            return Err(serde::de::Error::custom("position must be an integer"));
        }
        spelling
            .parse()
            .map(Self::Big)
            .map_err(serde::de::Error::custom)
    }
}

impl Ord for Integer {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (Self::Small(left), Self::Small(right)) => left.cmp(right),
            (Self::Big(left), Self::Big(right)) => left.cmp(right),
            (Self::Small(left), Self::Big(right)) => BigInt::from(*left).cmp(right),
            (Self::Big(left), Self::Small(right)) => left.cmp(&BigInt::from(*right)),
        }
    }
}

impl PartialOrd for Integer {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for Integer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Small(value) => value.fmt(formatter),
            Self::Big(value) => value.fmt(formatter),
        }
    }
}

#[derive(Deserialize)]
pub struct Position {
    pub line: Integer,
    pub column: Integer,
}

#[derive(Deserialize)]
pub struct Positions {
    pub begin: Position,
    #[serde(rename = "end")]
    pub _end: Position,
}

#[derive(Deserialize)]
pub struct WireLocation {
    pub path: String,
    pub positions: Positions,
}

#[derive(Deserialize)]
pub struct WireDiagnostic {
    pub check_name: String,
    pub description: String,
    pub severity: String,
    pub location: WireLocation,
}

pub struct Diagnostic {
    pub check_name: String,
    pub description: String,
    pub severity: String,
    pub path: Arc<str>,
    pub positions: Positions,
}

impl Diagnostic {
    pub fn severity_for_display(&self) -> &str {
        match self.severity.as_str() {
            "major" => "error",
            "minor" => "warning",
            _ => "unknown",
        }
    }

    pub fn fingerprint(&self) -> (&str, &str, &str, &str, &Integer, &Integer) {
        (
            &self.check_name,
            &self.description,
            &self.severity,
            &self.path,
            &self.positions.begin.line,
            &self.positions.begin.column,
        )
    }

    pub fn display(&self) -> String {
        format!(
            "{}:{}:{}: {}[{}] {}",
            self.path,
            self.positions.begin.line,
            self.positions.begin.column,
            self.severity_for_display(),
            self.check_name,
            self.description
        )
    }
}

pub struct ExpectedError {
    pub path: Arc<str>,
    pub line: Integer,
    pub optional: bool,
    pub tag: Option<String>,
    pub multi: bool,
    // The scanner still records complete upstream annotation metadata. These
    // fields do not affect the successful report's classification or rendering.
    pub _description: String,
    pub _begin_column: usize,
    pub _end_column: usize,
}

impl ExpectedError {
    pub fn key(&self) -> String {
        if let Some(tag) = &self.tag {
            format!("{}:{tag}", self.path)
        } else {
            format!("{}:{}", self.path, self.line)
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Old,
    New,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Change {
    Added,
    Removed,
    Changed,
    Unchanged,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Classification {
    TruePositive,
    FalsePositive,
    TrueNegative,
    FalseNegative,
}

impl Classification {
    pub fn title(self, verb: &str) -> String {
        let prefix = match self {
            Self::TruePositive => "True positives",
            Self::FalsePositive => "False positives",
            Self::TrueNegative => "True negatives",
            Self::FalseNegative => "False negatives",
        };
        format!("{prefix} {verb}")
    }
}

pub struct TestCase {
    pub key: String,
    pub old: Vec<Diagnostic>,
    pub new: Vec<Diagnostic>,
    pub expected: Vec<ExpectedError>,
}

impl TestCase {
    fn empty(key: String) -> Self {
        Self {
            key,
            old: Vec::new(),
            new: Vec::new(),
            expected: Vec::new(),
        }
    }

    pub fn diagnostics(&self, source: Side) -> &[Diagnostic] {
        if source == Side::Old {
            &self.old
        } else {
            &self.new
        }
    }

    pub fn optional(&self) -> bool {
        !self.expected.is_empty() && self.expected.iter().all(|error| error.optional)
    }

    pub fn multi(&self) -> bool {
        !self.expected.is_empty() && self.expected.iter().all(|error| error.multi)
    }

    pub fn path(&self) -> &str {
        if let Some(diagnostic) = self.new.first().or_else(|| self.old.first()) {
            &diagnostic.path
        } else {
            &self.expected[0].path
        }
    }

    pub fn change(&self) -> Change {
        if !self.new.is_empty() && self.old.is_empty() {
            Change::Added
        } else if !self.old.is_empty() && self.new.is_empty() {
            Change::Removed
        } else if !self.old.is_empty() && !self.new.is_empty() {
            if self.old.len() != self.new.len() {
                return Change::Changed;
            }
            let mut old: Vec<_> = self.old.iter().map(Diagnostic::fingerprint).collect();
            let mut new: Vec<_> = self.new.iter().map(Diagnostic::fingerprint).collect();
            old.sort_unstable();
            new.sort_unstable();
            if old == new {
                Change::Unchanged
            } else {
                Change::Changed
            }
        } else {
            Change::Unchanged
        }
    }

    pub fn classify(&self, source: Side) -> Classification {
        let diagnostics = self.diagnostics(source);
        if !diagnostics.is_empty() {
            if self.optional() {
                return Classification::TruePositive;
            }
            if !self.expected.is_empty() {
                let distinct: HashSet<_> = diagnostics
                    .iter()
                    .map(|d| &d.positions.begin.line)
                    .collect();
                let maximum = if self.multi() { self.expected.len() } else { 1 };
                if (1..=maximum).contains(&distinct.len()) {
                    return Classification::TruePositive;
                }
            }
            Classification::FalsePositive
        } else if !self.expected.is_empty() && !self.optional() {
            Classification::FalseNegative
        } else {
            Classification::TrueNegative
        }
    }
}

pub fn filename(path: &str) -> &str {
    std::path::Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("")
}

pub fn group_diagnostics(
    old: Vec<Diagnostic>,
    new: Vec<Diagnostic>,
    expected: Vec<ExpectedError>,
) -> Vec<TestCase> {
    let mut tagged_lines = HashMap::new();
    for error in &expected {
        if let Some(tag) = &error.tag {
            tagged_lines.insert(
                (filename(&error.path).to_owned(), error.line.clone()),
                tag.clone(),
            );
        }
    }
    let mut groups = BTreeMap::new();
    for (source, diagnostics) in [(Side::Old, old), (Side::New, new)] {
        for diagnostic in diagnostics {
            let tag = tagged_lines.get(&(
                filename(&diagnostic.path).to_owned(),
                diagnostic.positions.begin.line.clone(),
            ));
            let key = if let Some(tag) = tag {
                format!("{}:{tag}", diagnostic.path)
            } else {
                format!("{}:{}", diagnostic.path, diagnostic.positions.begin.line)
            };
            let group = groups
                .entry(key.clone())
                .or_insert_with(|| TestCase::empty(key));
            if source == Side::Old {
                group.old.push(diagnostic);
            } else {
                group.new.push(diagnostic);
            }
        }
    }
    for error in expected {
        let key = error.key();
        groups
            .entry(key.clone())
            .or_insert_with(|| TestCase::empty(key))
            .expected
            .push(error);
    }
    groups.into_values().collect()
}

#[derive(Default)]
pub struct Statistics {
    pub true_positives: i64,
    pub false_positives: i64,
    pub false_negatives: i64,
    pub total_diagnostics: i64,
}

impl Statistics {
    pub fn precision(&self) -> f64 {
        let denominator = self.true_positives + self.false_positives;
        if denominator > 0 {
            self.true_positives as f64 / denominator as f64
        } else {
            0.0
        }
    }

    pub fn recall(&self) -> f64 {
        let denominator = self.true_positives + self.false_negatives;
        if denominator > 0 {
            self.true_positives as f64 / denominator as f64
        } else {
            0.0
        }
    }

    pub fn passes(&self) -> bool {
        self.false_positives == 0 && self.false_negatives == 0
    }
}

pub fn compute_stats<'a>(
    cases: impl IntoIterator<Item = &'a TestCase>,
    source: Side,
) -> Statistics {
    let mut stats = Statistics::default();
    for case in cases {
        match case.classify(source) {
            Classification::TruePositive => stats.true_positives += 1,
            Classification::FalsePositive => stats.false_positives += 1,
            Classification::FalseNegative => stats.false_negatives += 1,
            Classification::TrueNegative => (),
        }
        stats.total_diagnostics += case.diagnostics(source).len() as i64;
    }
    stats
}

pub struct FileStats<'a> {
    pub path: &'a str,
    pub old: Statistics,
    pub new: Statistics,
}

impl FileStats<'_> {
    pub fn total_change(&self) -> i64 {
        (self.new.true_positives - self.old.true_positives).abs()
            + (self.new.false_positives - self.old.false_positives).abs()
            + (self.new.false_negatives - self.old.false_negatives).abs()
    }
}
