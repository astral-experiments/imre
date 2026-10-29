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
        args: &[Id],
        kwargs: &[(String, Id)],
    ) -> Result<Id> {
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
        Ok(self.alloc(cursor))
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
                    self.call(function, row.values, Vec::new()).map(Some)
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
    pub(super) fn next_filtered(&mut self, predicate: Id, input: Id) -> Result<Option<Id>> {
        loop {
            let mark = self.heap.temporary_mark();
            let Some(item) = self.next_result(input)? else {
                return Ok(None);
            };
            self.heap.keep(item);
            let selected = if predicate == NONE {
                self.truth(item)
            } else {
                self.call(predicate, vec![item], Vec::new())
                    .and_then(|value| self.truth(value))
            };
            self.heap.release_temporaries_since(mark);
            if selected? {
                return Ok(Some(item));
            }
        }
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
        if !matches!(
            (owner, self.heap.get(cursor)),
            ("map", Value::Map { .. })
                | ("filter", Value::Filter { .. })
                | ("enumerate", Value::Enumerate { .. })
        ) {
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
            "__next__" => self
                .next_result(cursor)?
                .ok_or_else(|| Error::new("StopIteration", "")),
            _ => unreachable!("iterator slot"),
        }
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
