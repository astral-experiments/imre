//! Execution state for lazily resumed generator expressions.
use super::{Context, Error, FrameOwner, Id, Result, Value};

impl Context {
    pub(super) fn template_iterator(&mut self, strings: Id, interpolations: Id) -> Result<Id> {
        let strings = self.iterator(strings)?;
        let interpolations = self.iterator(interpolations)?;
        Ok(self.alloc(Value::TemplateIter {
            strings,
            interpolations,
            from_strings: true,
        }))
    }

    /// Alternate the original tuple iterators, skipping empty strings without calling Python.
    pub(super) fn next_template_item(&mut self, id: Id) -> Result<Option<Id>> {
        let Value::TemplateIter {
            strings,
            interpolations,
            from_strings,
        } = *self.heap.get(id)
        else {
            unreachable!("template iterator")
        };
        let source = if from_strings {
            strings
        } else {
            interpolations
        };
        let mut item = self.next_result(source)?;
        let mut next_from_strings = !from_strings;
        if from_strings && item.is_some_and(|text| {
            matches!(self.heap.get(self.scalar_value(text)), Value::Str(s) if s.is_empty())
        }) {
            item = self.next_result(interpolations)?;
            next_from_strings = true;
        }
        let Value::TemplateIter { from_strings, .. } = self.heap.get_mut(id) else {
            unreachable!("template iterator")
        };
        *from_strings = next_from_strings;
        Ok(item)
    }

    /// Pull the item before reading the index, since its iterator can reenter enumerate.
    pub(super) fn next_enumerated(&mut self, id: Id, input: Id) -> Result<Option<Id>> {
        let Some(item) = self.next_result(input)? else {
            return Ok(None);
        };
        let Value::Enumerate { next_index, .. } = self.heap.get_mut(id) else {
            unreachable!("enumerate iterator")
        };
        let following = crate::integer_binary(next_index, &crate::Int::from(1), 0)?;
        let index = std::mem::replace(next_index, following);
        let index = self.alloc(Value::Int(index));
        Ok(Some(self.alloc(Value::Tuple(vec![index, item]))))
    }

    pub(super) fn generator(&mut self, function: Id, iter: Id) -> Result<Id> {
        let Value::Function {
            code,
            scope,
            class_cell,
            ..
        } = *self.heap.get(function)
        else {
            return Err(Error::new(
                "TypeError",
                "expected a compiled generator body",
            ));
        };
        let locals = self.parsed_parameters(code)?.locals.clone();
        let scope = self.comprehension_scope_with_locals(scope, locals);
        let Value::Scope(frame) = self.heap.get_mut(scope) else {
            unreachable!()
        };
        frame.class_cell = class_cell;
        self.bind(scope, ".0", iter);
        self.new_suspended(code, scope, FrameOwner::Function(function))
    }
}
