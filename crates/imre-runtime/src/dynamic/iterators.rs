//! Iteration for compiled Python loops and lazily resumed generator expressions.
use super::{Context, Error, Id, Result, Value};

impl Context {
    /// Public iter() returns the actual Python iterator; native loops separately
    /// buffer one item for their more/next operations.
    pub(super) fn python_iter(&mut self, value: Id) -> Result<Id> {
        if let Some(method) = self.bound_slot(value, "__iter__")? {
            let iterator = self.call(method, Vec::new(), Vec::new())?;
            if self.is_native_iterator(iterator) || self.slot(iterator, "__next__").is_some() {
                return Ok(iterator);
            }
            return Err(Error::new(
                "TypeError",
                format!(
                    "iter() returned non-iterator of type '{}'",
                    self.type_name(iterator)
                ),
            ));
        }
        if self.is_native_iterator(value) {
            return Ok(value);
        }
        if let Value::Range { start, stop, step } = *self.heap.get(value) {
            let remaining = super::builtins::range_len(start, stop, step)?;
            return Ok(self.alloc(Value::RangeIterator {
                next: start,
                step,
                remaining,
            }));
        }
        let (sequence, kind) = match self.heap.get(value) {
            Value::List(_) => (value, "list_iterator"),
            Value::Tuple(_) => (value, "tuple_iterator"),
            Value::Bytes(_) => (value, "bytes_iterator"),
            Value::ByteArray(_) => (value, "bytearray_iterator"),
            _ => {
                let values = self.items(value)?;
                (self.alloc(Value::Tuple(values)), "iterator")
            }
        };
        Ok(self.alloc(Value::Iterator {
            sequence: Some(sequence),
            offset: 0,
            kind,
        }))
    }

    fn is_native_iterator(&self, value: Id) -> bool {
        matches!(
            self.heap.get(value),
            Value::Generator { .. }
                | Value::AstWalk(_)
                | Value::RangeIterator { .. }
                | Value::PythonIterator { .. }
                | Value::Iterator { .. }
        )
    }

    pub(super) fn iterator(&mut self, value: Id) -> Result<Id> {
        let source = self.python_iter(value)?;
        if self.is_native_iterator(source) {
            Ok(source)
        } else {
            Ok(self.alloc(Value::PythonIterator {
                source,
                buffered: None,
            }))
        }
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
        let module = self.scope_data(scope).module.clone();
        let scope = self.scope(&module, Some(scope));
        let Value::Scope(frame) = self.heap.get_mut(scope) else {
            unreachable!()
        };
        frame.class_cell = class_cell;
        frame.first_argument = Some(".0".to_owned());
        let source = match *self.heap.get(iter) {
            Value::PythonIterator { source, .. } => source,
            _ => iter,
        };
        self.bind(scope, ".0", source);
        self.bind(scope, ".iterator", iter);
        Ok(self.alloc(Value::Generator {
            code,
            scope,
            buffered: None,
            running: false,
            exhausted: false,
        }))
    }

    pub(super) fn iterator_more(&mut self, iter: Id) -> Result<bool> {
        let (code, scope) = match *self.heap.get(iter) {
            Value::Iterator { .. } | Value::AstWalk(_) => return self.iterator_has_item(iter),
            Value::RangeIterator { remaining, .. } => return Ok(remaining != 0),
            Value::PythonIterator {
                buffered: Some(_), ..
            } => return Ok(true),
            Value::PythonIterator { source, .. } => match self.iterator_next(source) {
                Ok(item) => {
                    let Value::PythonIterator { buffered, .. } = self.heap.get_mut(iter) else {
                        unreachable!()
                    };
                    *buffered = Some(item);
                    return Ok(true);
                }
                Err(e) if e.kind == "StopIteration" => return Ok(false),
                Err(e) => return Err(e),
            },
            Value::Generator { running: true, .. } => {
                return Err(Error::new("ValueError", "generator already executing"));
            }
            Value::Generator {
                exhausted: true, ..
            } => return Ok(false),
            Value::Generator {
                buffered: Some(_), ..
            } => return Ok(true),
            Value::Generator { code, scope, .. } => (code, scope),
            _ => return Err(Error::new("TypeError", "expected iterator")),
        };
        let Value::Generator { running, .. } = self.heap.get_mut(iter) else {
            unreachable!()
        };
        *running = true;
        let result =
            self.execute(self.code_object(code).entry, scope)
                .map(|id| match self.heap.get(id) {
                    Value::None => None,
                    Value::Yielded(item) => Some(*item),
                    _ => unreachable!("compiled generator step result"),
                });
        let Value::Generator {
            running,
            buffered,
            exhausted,
            ..
        } = self.heap.get_mut(iter)
        else {
            unreachable!()
        };
        *running = false;
        *exhausted = !matches!(result, Ok(Some(_)));
        match result {
            Ok(item) => {
                *buffered = item;
                Ok(item.is_some())
            }
            Err(e) if e.kind == "StopIteration" => {
                Err(Error::new("RuntimeError", "generator raised StopIteration"))
            }
            Err(e) => Err(e),
        }
    }

    pub(super) fn iterator_next(&mut self, iter: Id) -> Result<Id> {
        if let Some(method) = self.bound_slot(iter, "__next__")? {
            return self.call(method, Vec::new(), Vec::new());
        }
        if matches!(
            self.heap.get(iter),
            Value::Iterator { .. } | Value::AstWalk(_)
        ) {
            return self
                .next_item(iter)?
                .ok_or_else(|| Error::new("StopIteration", ""));
        }
        if !self.iterator_more(iter)? {
            return Err(Error::new("StopIteration", ""));
        }
        match self.heap.get_mut(iter) {
            Value::Generator { buffered, .. } | Value::PythonIterator { buffered, .. } => {
                Ok(buffered.take().unwrap())
            }
            Value::RangeIterator {
                next,
                step,
                remaining,
            } => {
                let item = *next;
                *remaining -= 1;
                if *remaining > 0 {
                    *next += *step;
                }
                Ok(self.int(item))
            }
            _ => unreachable!("checked iterator"),
        }
    }
}
