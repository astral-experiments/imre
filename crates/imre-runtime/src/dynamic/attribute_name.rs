//! The Python object used as an attribute name and its already validated spelling.

use super::{Context, Id, Result, builtins};
use std::borrow::Cow;

#[derive(Clone, Copy)]
pub(super) enum AttributeName<'a> {
    Text(&'a str),
    /// A static method operand whose Python code name is needed only by lookup hooks.
    Linked(&'a str),
    Python { key: Id, text: Option<&'a str> },
}

impl<'a> AttributeName<'a> {
    pub(super) fn from_lookup(text: &'a str, original: Option<Id>) -> Self {
        original.map_or(Self::Text(text), |key| Self::Python {
            key,
            text: Some(text),
        })
    }

    pub(super) fn original(self) -> Option<Id> {
        match self {
            Self::Text(_) | Self::Linked(_) => None,
            Self::Python { key, .. } => Some(key),
        }
    }

    pub(super) fn key(self, ctx: &mut Context) -> Id {
        match self {
            Self::Text(text) | Self::Linked(text) => ctx.text(text),
            Self::Python { key, .. } => key,
        }
    }

    pub(super) fn text(self, ctx: &Context) -> Result<Cow<'a, str>> {
        match self {
            Self::Text(text)
            | Self::Linked(text)
            | Self::Python {
                text: Some(text), ..
            } => Ok(Cow::Borrowed(text)),
            Self::Python { key, text: None } => builtins::attribute_name(ctx, key).map(Cow::Owned),
        }
    }

    pub(super) fn nonexact_key(self, ctx: &Context) -> Option<Id> {
        self.original()
            .filter(|&key| ctx.string_storage(key) != Some(key))
    }
}
