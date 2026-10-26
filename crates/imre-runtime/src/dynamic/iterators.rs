//! Execution state for lazily resumed generator expressions.
use super::{Context, Error, Id, Result, Value};

impl Context {
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
        let module = self.scope_data(scope).module.clone();
        let scope = self.scope(&module, Some(scope));
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
            code,
            scope,
            running: false,
            offset: Some(0),
        }))
    }

    /// Runs the compiled body until it yields one item, returns, or raises.
    pub(super) fn next_generator_item(&mut self, generator: Id) -> Result<Option<Id>> {
        let Value::Generator {
            code,
            scope,
            running,
            offset,
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
        let result = self.execute(code, scope).map(|id| match self.heap.get(id) {
            Value::None => None,
            Value::Yielded(item) => Some(*item),
            _ => unreachable!("compiled generator step result"),
        });
        let Value::Generator {
            running, offset, ..
        } = self.heap.get_mut(generator)
        else {
            unreachable!()
        };
        *running = false;
        if !matches!(result, Ok(Some(_))) {
            *offset = None;
        }
        match result {
            Err(e) if self.is_stop_iteration(&e) => {
                Err(Error::new("RuntimeError", "generator raised StopIteration"))
            }
            result => result,
        }
    }
}
