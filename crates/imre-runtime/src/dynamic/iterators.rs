//! Execution state for lazily resumed generator expressions.
use super::{Context, Error, FrameOwner, Id, NONE, Result, Value};

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

    pub(super) fn generator(&mut self, function: Id, iter: Id, targets: Id) -> Result<Id> {
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
        let scope = self.comprehension_scope(scope, targets)?;
        let Value::Scope(frame) = self.heap.get_mut(scope) else {
            unreachable!()
        };
        frame.class_cell = class_cell;
        frame.first_argument = Some(".0".to_owned());
        let source = match *self.heap.get(iter) {
            Value::LoopIterator { source, .. } => source,
            _ => iter,
        };
        self.bind(scope, ".0", source);
        self.bind(scope, ".iterator", iter);
        Ok(self.new_generator(code, scope, FrameOwner::Function(function)))
    }

    /// Runs the compiled body until it yields one item, returns, or raises.
    pub(super) fn next_generator_item(&mut self, generator: Id) -> Result<Option<Id>> {
        let Value::Generator {
            code,
            scope,
            running,
            offset,
            ..
        } = self.heap.get_mut(generator)
        else {
            unreachable!()
        };
        if *running {
            return Err(Error::new("ValueError", "generator already executing"));
        }
        if offset.is_none() {
            return Ok(None);
        }
        *running = true;
        let (code, scope) = (*code, *scope);
        let result = self
            .execute(code, scope, FrameOwner::Generator(generator))
            .map(|id| match self.heap.get(id) {
                Value::None => None,
                Value::Yielded(item) => Some(*item),
                _ => unreachable!("compiled generator step result"),
            });
        let Value::Generator {
            running,
            offset,
            scope,
            owner,
            frame,
            ..
        } = self.heap.get_mut(generator)
        else {
            unreachable!()
        };
        *running = false;
        if matches!(result, Ok(Some(_))) {
            // Native comprehension code keeps its own loop cursor in the scope.
            *offset = Some(1);
        } else {
            *offset = None;
            *scope = NONE;
            *frame = None;
            *owner = FrameOwner::Module(NONE);
        }
        result.map_err(|error| self.frame_error(error, code))
    }

    /// Preserve an escaping StopIteration as the cause and context of the frame's RuntimeError.
    pub(super) fn frame_error(&mut self, error: Error, code: Id) -> Error {
        if !self.is_stop_iteration(&error) {
            return error;
        }
        let cause = self.raised_instance(error);
        let message = if self.code_object(code).is_coroutine() {
            "coroutine raised StopIteration"
        } else {
            "generator raised StopIteration"
        };
        let mut error = Error::new("RuntimeError", message);
        error.tracing().cause = Some(cause);
        error.context = cause;
        error
    }
}
