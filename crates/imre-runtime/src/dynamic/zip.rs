use super::value::{Error, Id, Result, Value};
use super::{Context, NONE};

/// One in-progress row of a possibly nested zip or map.
struct PendingRow {
    inputs: std::rc::Rc<[Id]>,
    function: Option<Id>,
    strict: bool,
    index: usize,
    values: Vec<Id>,
    exhausted: bool,
}

impl PendingRow {
    fn from_iterator(value: &Value) -> Result<Option<Self>> {
        let (inputs, strict, function) = match value {
            Value::Zip { inputs, strict } => (inputs, *strict, None),
            Value::Map {
                inputs,
                strict,
                function,
            } => (inputs, *strict, Some(*function)),
            _ => return Ok(None),
        };
        let mut values = Vec::new();
        values
            .try_reserve_exact(inputs.len())
            .map_err(|_| crate::Error::allocation())?;
        Ok(Some(Self {
            inputs: inputs.clone(),
            strict,
            function,
            index: 0,
            values,
            exhausted: false,
        }))
    }
}

impl Context {
    /// Acquire inputs left to right at construction and defer pulls and calls until iteration.
    pub(super) fn zip_or_map(
        &mut self,
        name: &str,
        class: Id,
        args: &[Id],
        kwargs: &[(String, Id)],
    ) -> Result<Id> {
        let base = self.builtin(name);
        if !self.direct_subclass(class, base) {
            return Err(self.invalid_new_subtype(name, class));
        }
        let strict = match kwargs {
            [] => false,
            [(key, value)] if key == "strict" => self.truth(*value)?,
            [(key, _)] => {
                return Err(Error::new(
                    "TypeError",
                    format!("{name}() got an unexpected keyword argument '{key}'"),
                ));
            }
            _ => {
                return Err(Error::new(
                    "TypeError",
                    format!(
                        "{name}() takes at most 1 keyword argument ({} given)",
                        kwargs.len()
                    ),
                ));
            }
        };
        let (function, sources) = if name == "map" {
            if args.len() < 2 {
                return Err(Error::new(
                    "TypeError",
                    "map() must have at least two arguments.",
                ));
            }
            (Some(args[0]), &args[1..])
        } else {
            (None, args)
        };
        let mut inputs = Vec::with_capacity(sources.len());
        for &source in sources {
            let input = self.iterator(source)?;
            self.heap.keep(input);
            inputs.push(input);
        }
        let inputs = inputs.into();
        let cursor = match function {
            Some(function) => Value::Map {
                function,
                inputs,
                strict,
            },
            None => Value::Zip { inputs, strict },
        };
        let storage = self.alloc(cursor);
        Ok(self.with_subclass_storage(class, base, storage))
    }

    /// Materialize the remaining digits of a forward text cursor mapped by the original int.
    /// A rejected character consumes its own UTF-8 bytes and leaves the remaining cursor live.
    pub(super) fn list_from_int_character_map(&mut self, map: Id) -> Option<Result<Id>> {
        let Value::Map {
            function, inputs, ..
        } = self.heap.get(map)
        else {
            return None;
        };
        if self.builtins.get("int") != Some(function) {
            return None;
        }
        let &[input] = inputs.as_ref() else {
            return None;
        };
        let super::IteratorSlice::Text(text) = self.iterator_remaining(input).ok()? else {
            return None;
        };
        let digits = crate::objects::string_integer_values(text);
        let consumed = match &digits {
            Ok(_) => text.len(),
            Err((0, _)) => return None, // A failed bulk reservation consumed no input.
            Err((consumed, _)) => *consumed,
        };
        let Value::Iterator {
            offset, sequence, ..
        } = self.heap.get_mut(input)
        else {
            unreachable!()
        };
        *offset += consumed;
        match digits {
            Ok(digits) => {
                *sequence = None;
                let items = digits
                    .into_iter()
                    .map(|digit| self.alloc(Value::Int(digit)))
                    .collect();
                Some(Ok(self.alloc(Value::List(items))))
            }
            Err((_, error)) => Some(Err(error.into())),
        }
    }

    /// Pull one row, preserving source errors and the strict mismatch probe order.
    pub(super) fn next_zip_or_map(&mut self, id: Id) -> Result<Option<Id>> {
        let mut row = PendingRow::from_iterator(self.heap.get(id))?.expect("zip or map iterator");
        if row.inputs.is_empty() {
            return Ok(None);
        }
        let mut parents = Vec::new();
        'pull: loop {
            let input = row.inputs[row.index];
            let mut result = match PendingRow::from_iterator(self.heap.get(input))? {
                Some(child) if !child.inputs.is_empty() => {
                    parents
                        .try_reserve(1)
                        .map_err(|_| crate::Error::allocation())?;
                    parents.push(row);
                    row = child;
                    continue;
                }
                Some(_) => Ok(None),
                None => self.next_result(input),
            };
            loop {
                // Strict cursors handle StopIteration from both a leaf and a child map callback.
                let value = match result {
                    Ok(value) => value,
                    Err(error) if row.strict && self.is_stop_iteration(&error) => None,
                    Err(error) => {
                        let Some(parent) = parents.pop() else {
                            return Err(error);
                        };
                        row = parent;
                        result = Err(error);
                        continue;
                    }
                };
                let name = if row.function.is_some() { "map" } else { "zip" };
                if let Some(value) = value {
                    if row.exhausted {
                        return Err(length_mismatch(name, row.index, "longer"));
                    }
                    self.heap.reserve_temporaries(1)?;
                    self.heap.keep(value);
                    row.values.push(value);
                } else {
                    if !row.exhausted && row.strict && row.index != 0 {
                        return Err(length_mismatch(name, row.index, "shorter"));
                    }
                    row.exhausted = true;
                }
                row.index += 1;
                if row.index < row.inputs.len() && (!row.exhausted || row.strict) {
                    continue 'pull;
                }
                result = if row.exhausted {
                    Ok(None)
                } else if let Some(function) = row.function {
                    if let [value] = row.values.as_slice()
                        && let Value::Str(character) = self.heap.get(*value)
                        && matches!(self.heap.get(function), Value::Native(name) if name == "int")
                        && let Some(digit) = crate::integer_text::single_decimal_digit(character)
                    {
                        Ok(Some(self.int(i64::from(digit))))
                    } else {
                        self.call(function, row.values, Vec::new()).map(Some)
                    }
                } else {
                    Ok(Some(self.alloc(Value::Tuple(row.values))))
                };
                let Some(parent) = parents.pop() else {
                    return result;
                };
                row = parent;
            }
        }
    }

    /// Keep each candidate alive across its predicate and return the original item.
    pub(super) fn next_filtered(
        &mut self,
        filter: Id,
        predicate: Id,
        input: Id,
    ) -> Result<Option<Id>> {
        let roots = self.heap.temporary_mark();
        // The filter owns its predicate and input throughout a reentrant pull.
        self.heap.keep(filter);
        let step = self.heap.temporary_mark();
        let mut limit = self.heap.streaming_collection_limit();
        let result = (|| loop {
            self.heap.release_temporaries_since(step);
            self.collect_streaming_garbage(&mut limit);
            let Some(item) = self.next_result(input)? else {
                return Ok(None);
            };
            self.heap.keep(item);
            let selected = if predicate == NONE {
                self.truth(item)?
            } else {
                let value = self.call(predicate, vec![item], Vec::new())?;
                self.truth(value)?
            };
            if selected {
                return Ok(Some(item));
            }
        })();
        self.heap.release_temporaries_since(roots);
        result
    }

    /// Apply the shared iterator slots to a receiver of the defining builtin type.
    pub(super) fn iterator_method(
        &mut self,
        owner: &str,
        name: &str,
        args: &[Id],
        kwargs: &[(String, Id)],
    ) -> Result<Id> {
        let Some((&cursor, rest)) = args.split_first() else {
            return Err(Error::new(
                "TypeError",
                format!("descriptor '{name}' of '{owner}' object needs an argument"),
            ));
        };
        let storage = if matches!(owner, "filter" | "map" | "zip") {
            self.scalar_value(cursor)
        } else {
            cursor
        };
        if self.heap.get(storage).intrinsic_type_name() != Some(owner) {
            return Err(Error::new(
                "TypeError",
                format!(
                    "descriptor '{name}' requires a '{owner}' object but received a '{}'",
                    self.type_name(cursor)
                ),
            ));
        }
        if !kwargs.is_empty() {
            return Err(Error::new(
                "TypeError",
                format!("wrapper {name}() takes no keyword arguments"),
            ));
        }
        if !rest.is_empty() {
            return Err(Error::new(
                "TypeError",
                format!("expected 0 arguments, got {}", rest.len()),
            ));
        }
        match name {
            "__iter__" => Ok(cursor),
            "__length_hint__" => self.length_hint(cursor, None),
            "__next__" => self
                .next_result(storage)?
                .ok_or_else(|| Error::new("StopIteration", "")),
            "__reduce__" => {
                let Value::DictIterator(state) = self.heap.get(cursor) else {
                    unreachable!("ordered dictionary iterator reduction");
                };
                self.reduce_iterator_copy(Value::DictIterator(*state))
            }
            _ => unreachable!("iterator slot"),
        }
    }

    /// Walking a copy preserves the live cursor, including when iteration raises.
    pub(super) fn reduce_iterator_copy(&mut self, copy: Value) -> Result<Id> {
        let snapshot = self.alloc(copy);
        let items = super::builtins::call(self, "list", &[snapshot], &[])?;
        let args = self.alloc(Value::Tuple(vec![items]));
        let iter = self.builtin("iter");
        Ok(self.alloc(Value::Tuple(vec![iter, args])))
    }
}

/// Describe the first input that fails the strict length check.
fn length_mismatch(name: &str, index: usize, comparison: &str) -> Error {
    let previous = if index == 1 {
        String::from("argument 1")
    } else {
        format!("arguments 1-{index}")
    };
    Error::new(
        "ValueError",
        format!(
            "{name}() argument {} is {comparison} than {previous}",
            index + 1
        ),
    )
}
