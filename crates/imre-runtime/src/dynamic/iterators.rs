//! Execution state for lazily resumed generator expressions.
use super::{Context, Error, FrameOwner, Id, NONE, Result, Value};

impl Context {
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
        let scope = self.comprehension_scope(scope);
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
        Ok(self.alloc(Value::Generator {
            owner: FrameOwner::Function(function),
            code,
            scope,
            iterators: Vec::new(),
            running: false,
            offset: Some(0),
        }))
    }

    /// Runs the compiled body until it yields one item, returns, or raises.
    pub(super) fn next_generator_item(&mut self, generator: Id) -> Result<Option<Id>> {
        let Value::Generator {
            owner,
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
        let (code, scope, owner) = (*code, *scope, *owner);
        let result = self
            .execute(code, scope, owner)
            .map(|id| match self.heap.get(id) {
                Value::None => None,
                Value::Yielded(item) => Some(*item),
                _ => unreachable!("compiled generator step result"),
            });
        let Value::Generator {
            running,
            offset,
            owner,
            ..
        } = self.heap.get_mut(generator)
        else {
            unreachable!()
        };
        *running = false;
        if !matches!(result, Ok(Some(_))) {
            *offset = None;
            *owner = FrameOwner::Module(NONE);
        }
        result.map_err(|error| self.generator_error(error))
    }

    /// Preserve an escaping StopIteration as the cause and context of the generator's RuntimeError.
    pub(super) fn generator_error(&mut self, error: Error) -> Error {
        if !self.is_stop_iteration(&error) {
            return error;
        }
        let cause = self.raised_instance(error);
        let mut error = Error::new("RuntimeError", "generator raised StopIteration");
        let trace = error.tracing();
        trace.cause = Some(cause);
        trace.context = Some(cause);
        error
    }
}
