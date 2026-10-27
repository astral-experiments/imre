//! Bound actions retain their converted values until the namespace takes ownership.

use crate::StrOwner;
use std::borrow::Cow;

use num_bigint::BigInt;
use num_traits::{Signed, Zero};
use serde::{Deserialize, Deserializer};

use super::{
    Action, Error, Kind, OptionSpec, Parser, Stop, Words, imre_path_new, owned_text, string_repr,
};
use crate::{Int, floats, parse_decimal_int};

#[derive(Deserialize)]
pub(super) struct IntegerRange(
    #[serde(deserialize_with = "source_integer")] BigInt,
    #[serde(deserialize_with = "source_integer")] BigInt,
    #[serde(deserialize_with = "source_integer")] BigInt,
);

fn source_integer<'de, D: Deserializer<'de>>(input: D) -> Result<BigInt, D::Error> {
    crate::parse_integer_literal(&String::deserialize(input)?)
        .ok_or_else(|| serde::de::Error::custom("invalid integer literal"))
}

impl IntegerRange {
    fn contains(&self, n: &Int) -> bool {
        let n = n.big();
        let n = n.as_ref();
        (if self.2.is_positive() {
            &self.0 <= n && n < &self.1
        } else {
            &self.0 >= n && n > &self.1
        }) && ((n - &self.0) % &self.2).is_zero()
    }

    pub(super) fn iter(&self) -> impl Iterator<Item = BigInt> + '_ {
        std::iter::successors(Some(self.0.clone()), |n| Some(n + &self.2)).take_while(|n| {
            if self.2.is_positive() {
                n < &self.1
            } else {
                n > &self.1
            }
        })
    }
}

pub(super) enum Bound<'a> {
    Absent,
    Bool(bool),
    Enum(usize),
    Scalar(Scalar<'a>),
    List(Vec<Scalar<'a>>),
}

pub(super) enum Scalar<'a> {
    Text(&'a str),
    Int(Int),
    Float(f64),
}

impl<'a> Scalar<'a> {
    fn convert(parser: &'a Parser, option: &OptionSpec, text: &'a str) -> Result<Self, Stop<'a>> {
        let value = match option.kind {
            Kind::Str | Kind::Path | Kind::Class => Some(Self::Text(text)),
            Kind::Int => {
                if text.len() > 640
                    && !super::decimal_digits_allowed(
                        text.chars()
                            .filter(|&ch| crate::integer_text::decimal_digit(ch).is_some())
                            .count(),
                    )
                {
                    None
                } else {
                    parse_decimal_int(text)
                        .map_err(Stop::Exception)?
                        .map(Self::Int)
                }
            }
            Kind::Float => floats::parse(text).map(Self::Float),
            Kind::Bool => unreachable!(),
        };
        value.ok_or_else(|| {
            Stop::Error(
                parser,
                format!(
                    "argument {}: invalid {} value: {}",
                    option.error_name(),
                    option.converter.as_deref().unwrap(),
                    string_repr(text)
                ),
            )
        })
    }

    fn text(&self) -> Cow<'_, str> {
        match self {
            Self::Text(s) => Cow::Borrowed(s),
            Self::Int(n) => Cow::Owned(n.big().to_string()),
            Self::Float(n) => Cow::Owned(floats::render(*n).to_string()),
        }
    }

    fn check(&self, option: &OptionSpec) -> Result<(), String> {
        if !option.has_choices {
            return Ok(());
        }
        let text = self.text();
        let matches = if let (Some(range), Self::Int(n)) = (&option.range, self) {
            range.contains(n)
        } else {
            option.choices.iter().any(|choice| match self {
                Self::Float(n) => floats::parse(choice) == Some(*n),
                _ => choice == text.as_ref(),
            })
        };
        if matches {
            return Ok(());
        }
        let choices = option.choices_text(", ", true);
        Err(format!(
            "argument {}: invalid choice: {} (choose from {choices})",
            option.error_name(),
            string_repr(&text)
        ))
    }

    fn word(self, kind: Kind, optional: bool) -> Result<usize, Error> {
        Ok(match self {
            Self::Int(value) if kind == Kind::Str => super::super::unions::integer(value),
            Self::Int(value) => value.into_raw(),
            Self::Float(value) if optional => floats::imre_float_box(value),
            Self::Float(value) => value.to_bits() as usize,
            Self::Text(value) => {
                let text = owned_text(value)?;
                if kind == Kind::Path {
                    // SAFETY: the source remains live until Path takes an owner or normalizes it.
                    unsafe { imre_path_new(StrOwner::as_ptr(&text)) as usize }
                } else {
                    StrOwner::into_raw(text) as usize
                }
            }
        })
    }
}

impl OptionSpec {
    pub(super) fn choices_text(&self, separator: &str, quoted: bool) -> String {
        let labels: Box<dyn Iterator<Item = Cow<'_, str>> + '_> = match &self.range {
            Some(range) => Box::new(range.iter().map(|n| Cow::Owned(n.to_string()))),
            None => Box::new(self.choices.iter().map(|s| Cow::Borrowed(s.as_str()))),
        };
        let mut result = String::new();
        for (i, label) in labels.enumerate() {
            if i != 0 {
                result.push_str(separator);
            }
            if quoted {
                result.push_str(&string_repr(&label));
            } else {
                result.push_str(&label);
            }
        }
        result
    }

    /// Apply a zero-argument option to the current namespace value. Repeated
    /// short options in a cluster have the same effect as separate occurrences.
    pub(super) fn flag_value<'a>(
        &'a self,
        parser: &'a Parser,
        previous: Option<Bound<'a>>,
        spelling: &str,
    ) -> Result<Bound<'a>, Stop<'a>> {
        if self.action == Action::BooleanOptional {
            return Ok(Bound::Bool(!spelling.starts_with("--no-")));
        }
        if self.action == Action::Count {
            let n = match previous {
                Some(Bound::Scalar(Scalar::Int(n))) => n,
                None => self
                    .default
                    .as_deref()
                    .map(parse_decimal_int)
                    .transpose()
                    .map_err(Stop::Exception)?
                    .flatten()
                    .unwrap_or_else(|| Int::from_usize(0)),
                _ => unreachable!("count produces integers"),
            };
            let next = Int::try_new(n.big().as_ref() + 1)
                .map_err(|_| Stop::Exception(Error::allocation()))?;
            return Ok(Bound::Scalar(Scalar::Int(next)));
        }
        self.bind(parser, Some(&[]))
    }

    pub(super) fn bind<'a>(
        &'a self,
        parser: &'a Parser,
        args: Option<&[&'a str]>,
    ) -> Result<Bound<'a>, Stop<'a>> {
        let invalid = |message| Stop::Error(parser, message);
        let missing = args.is_none() || self.positional() && args.is_some_and(<[_]>::is_empty);
        if missing && let Some(member) = self.enum_default {
            return Ok(Bound::Enum(member));
        }
        if self.kind == Kind::Bool {
            let value = if args.is_some() {
                &self.constant
            } else {
                &self.default
            };
            let value = value.as_deref().map(|text| text == "True");
            return Ok(value.map_or(Bound::Absent, Bound::Bool));
        }
        if self.nargs.list() {
            if missing && self.list_default {
                return Ok(Bound::Scalar(Scalar::Text(
                    self.default.as_deref().unwrap(),
                )));
            }
            let Some(args) = args else {
                return Ok(Bound::Absent);
            };
            let mut values = Vec::new();
            values.try_reserve_exact(args.len())?;
            for argument in args {
                values.push(Scalar::convert(parser, self, argument)?);
            }
            for value in &values {
                value.check(self).map_err(invalid)?;
            }
            return Ok(Bound::List(values));
        }
        let (value, check, literal_integer) = if missing {
            (self.default.as_deref(), false, self.int_default)
        } else if let Some(value) = args.and_then(|values| values.first()) {
            (Some(*value), true, false)
        } else {
            (self.constant.as_deref(), false, self.int_constant)
        };
        let Some(text) = value else {
            return Ok(Bound::Absent);
        };
        let value = if literal_integer {
            Scalar::Int(
                parse_decimal_int(text)
                    .map_err(Stop::Exception)?
                    .expect("canonical integer literal"),
            )
        } else {
            Scalar::convert(parser, self, text)?
        };
        if check {
            value.check(self).map_err(invalid)?;
        }
        Ok(Bound::Scalar(value))
    }

    pub(super) fn word(&self, value: Bound<'_>) -> Result<usize, Error> {
        Ok(match value {
            Bound::Absent => 0,
            Bound::Enum(member) => (member << 3) | 5,
            Bound::Bool(value) => usize::from(value) + usize::from(self.optional),
            Bound::Scalar(value) => value.word(self.kind, self.optional)?,
            Bound::List(values) if self.kind == Kind::Int => {
                let mut ints = Vec::new();
                ints.try_reserve_exact(values.len())
                    .map_err(|_| Error::allocation())?;
                ints.extend(values.into_iter().map(|value| match value {
                    Scalar::Int(n) => n,
                    _ => unreachable!(),
                }));
                let list = crate::try_new_list(ints).map_err(|_| Error::allocation())?;
                crate::ListOwner::into_raw(list) as usize
            }
            Bound::List(values) => {
                let mut words = Words::new(match self.kind {
                    Kind::Str => 3,
                    Kind::Path => 5,
                    Kind::Float => 7,
                    _ => unreachable!(),
                });
                words
                    .values
                    .try_reserve_exact(values.len())
                    .map_err(|_| Error::allocation())?;
                for value in values {
                    words.values.push(value.word(self.kind, false)?);
                }
                let word = crate::ObjectOwner::into_raw(words.into_object()) as usize;
                // The closed str | list[str] union reserves tag 2 for the list.
                if self.list_default { word | 2 } else { word }
            }
        })
    }
}
