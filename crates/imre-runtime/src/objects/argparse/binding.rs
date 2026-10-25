//! Bound actions retain their converted values until the namespace takes ownership.

use std::borrow::Cow;
use std::rc::Rc;

use super::{Kind, OptionSpec, Str, Words, imre_path_new, string_repr};
use crate::{Int, List, floats, parse_decimal_int};

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
    fn convert(option: &OptionSpec, text: &'a str) -> Result<Self, String> {
        let value = match option.kind {
            Kind::Str | Kind::Path => Some(Self::Text(text)),
            Kind::Int => parse_decimal_int(text).map(Self::Int),
            Kind::Float => floats::parse(text).map(Self::Float),
            Kind::Bool => unreachable!(),
        };
        value.ok_or_else(|| {
            format!(
                "argument {}: invalid {} value: {}",
                option.error_name(),
                option.converter.as_deref().unwrap(),
                string_repr(text)
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
        if option.choices.iter().any(|choice| match self {
            Self::Float(n) => floats::parse(choice) == Some(*n),
            _ => choice == text.as_ref(),
        }) {
            return Ok(());
        }
        let choices = option
            .choices
            .iter()
            .map(|c| string_repr(c))
            .collect::<Vec<_>>()
            .join(", ");
        Err(format!(
            "argument {}: invalid choice: {} (choose from {choices})",
            option.error_name(),
            string_repr(&text)
        ))
    }

    fn word(self, kind: Kind, optional: bool) -> usize {
        match self {
            Self::Int(value) => value.into_raw(),
            Self::Float(value) if optional => floats::imre_float_box(value),
            Self::Float(value) => value.to_bits() as usize,
            Self::Text(value) => {
                let text = Rc::new(Str::new(value));
                if kind == Kind::Path {
                    // SAFETY: the source remains live until Path takes an owner or normalizes it.
                    unsafe { imre_path_new(Rc::as_ptr(&text)) as usize }
                } else {
                    Rc::into_raw(text) as usize
                }
            }
        }
    }
}

impl OptionSpec {
    pub(super) fn bind<'a>(&'a self, args: Option<&[&'a str]>) -> Result<Bound<'a>, String> {
        let missing = args.is_none() || self.positional() && args.is_some_and(<[_]>::is_empty);
        if missing && let Some(member) = self.enum_default {
            return Ok(Bound::Enum(member));
        }
        if self.kind == Kind::Bool {
            let value = args
                .is_some()
                .then_some(true)
                .or_else(|| self.default.as_deref().map(|value| value == "True"));
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
            let values = args
                .iter()
                .map(|value| Scalar::convert(self, value))
                .collect::<Result<Vec<_>, _>>()?;
            for value in &values {
                value.check(self)?;
            }
            return Ok(Bound::List(values));
        }
        let (value, check) = if missing {
            (self.default.as_deref(), false)
        } else if let Some(value) = args.and_then(|values| values.first()) {
            (Some(*value), true)
        } else {
            (self.constant.as_deref(), false)
        };
        let Some(text) = value else {
            return Ok(Bound::Absent);
        };
        let value = Scalar::convert(self, text)?;
        if check {
            value.check(self)?;
        }
        Ok(Bound::Scalar(value))
    }

    pub(super) fn word(&self, value: Bound<'_>) -> usize {
        match value {
            Bound::Absent => 0,
            Bound::Enum(member) => (member << 3) | 5,
            Bound::Bool(value) => usize::from(value) + usize::from(self.optional),
            Bound::Scalar(value) => value.word(self.kind, self.optional),
            Bound::List(values) if self.kind == Kind::Int => {
                let ints = values
                    .into_iter()
                    .map(|value| match value {
                        Scalar::Int(n) => n,
                        _ => unreachable!(),
                    })
                    .collect();
                Rc::into_raw(Rc::new(List::new(ints))) as usize
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
                    .extend(values.into_iter().map(|value| value.word(self.kind, false)));
                let word = Rc::into_raw(words.into_object()) as usize;
                // The closed str | list[str] union reserves tag 2 for the list.
                if self.list_default { word | 2 } else { word }
            }
        }
    }
}
