//! Box the rows of the same lazy format cursors used by the typed runtime.

use std::ops::Range;

use super::super::{
    Context,
    value::{Error, FormatKind, Id, NONE, Result, Value},
};
use crate::format_grammar::{self, Key};

enum FormatRow {
    Markup(format_grammar::Markup),
    Field(bool, Key),
}

impl Context {
    /// Return the parser, or the immediate first key and a lazy cursor over the remaining lookups.
    pub(super) fn format_cursor(&mut self, input: Id, split: bool) -> Result<Id> {
        let text = self.str_value(input)?;
        if !split {
            return Ok(self.alloc(Value::FormatIterator {
                input,
                offset: 0,
                kind: FormatKind::Markup,
            }));
        }
        let (key, offset) = format_grammar::first(text).map_err(|e| Error::new("ValueError", e))?;
        let key = self.format_key(input, key)?;
        let rest = self.alloc(Value::FormatIterator {
            input,
            offset,
            kind: FormatKind::Field,
        });
        Ok(self.alloc(Value::Tuple(vec![key, rest])))
    }

    fn format_span(&mut self, input: Id, span: Option<Range<usize>>) -> Result<Id> {
        match span {
            Some(span) => {
                let value = self.str_value(input)?[span].to_owned();
                Ok(self.text(value))
            }
            None => Ok(NONE),
        }
    }

    fn format_key(&mut self, input: Id, key: Key) -> Result<Id> {
        match key {
            Key::Integer(n) => Ok(self.alloc(Value::Int(crate::Int::from_usize(n)))),
            Key::Text(span) => self.format_span(input, Some(span)),
        }
    }

    /// Advance one grammar token even on failure, matching the cursor's observable error order.
    pub(in crate::dynamic) fn next_format_row(&mut self, cursor: Id) -> Result<Option<Id>> {
        let Value::FormatIterator {
            input,
            mut offset,
            kind,
        } = *self.heap.get(cursor)
        else {
            unreachable!()
        };
        let text = self.str_value(input)?;
        let row = match kind {
            FormatKind::Markup => {
                format_grammar::markup(text, &mut offset).map(|part| part.map(FormatRow::Markup))
            }
            FormatKind::Field => format_grammar::field(text, &mut offset)
                .map(|part| part.map(|(attribute, key)| FormatRow::Field(attribute, key))),
        };
        let Value::FormatIterator { offset: pos, .. } = self.heap.get_mut(cursor) else {
            unreachable!()
        };
        *pos = offset;
        let Some(row) = row.map_err(|e| Error::new("ValueError", e))? else {
            return Ok(None);
        };
        let fields = match row {
            FormatRow::Markup(row) => vec![
                self.format_span(input, Some(row.literal))?,
                self.format_span(input, row.field)?,
                self.format_span(input, row.spec)?,
                self.format_span(input, row.conversion)?,
            ],
            FormatRow::Field(attribute, key) => vec![
                if attribute { super::TRUE } else { super::FALSE },
                self.format_key(input, key)?,
            ],
        };
        Ok(Some(self.alloc(Value::Tuple(fields))))
    }
}
