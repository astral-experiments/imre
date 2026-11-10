use super::value::{Error, Id, Result, Value};
use super::{Context, NONE};

/// Keep the original callable for tracing and for rows that need Python dispatch.
#[derive(Clone, Copy)]
pub(super) enum MapFunction {
    Python(Id),
    IntCharacters(Id),
}

impl MapFunction {
    pub(super) fn callable(self) -> Id {
        match self {
            Self::Python(id) | Self::IntCharacters(id) => id,
        }
    }

    fn apply(self, ctx: &mut Context, values: Vec<Id>) -> Result<Id> {
        if let Self::IntCharacters(_) = self
            && let [value] = values.as_slice()
            && let Value::Str(character) = ctx.heap.get(*value)
            && let Some(digit) = crate::integer_text::single_decimal_digit(character)
        {
            return Ok(ctx.int(i64::from(digit)));
        }
        ctx.call(self.callable(), values, Vec::new())
    }
}

/// One in-progress row of a possibly nested zip or map.
struct PendingRow {
    inputs: std::rc::Rc<[Id]>,
    function: Option<MapFunction>,
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
                function: if class == base
                    && let [source] = sources
                    && matches!(self.heap.get(*source), Value::Str(_))
                    && self.builtins.get("int") == Some(&function)
                {
                    MapFunction::IntCharacters(function)
                } else {
                    MapFunction::Python(function)
                },
                inputs,
                strict,
            },
            None => Value::Zip { inputs, strict },
        };
        let storage = self.alloc(cursor);
        Ok(self.with_subclass_storage(class, base, storage))
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
                    function.apply(self, row.values).map(Some)
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
        let roots = self.heap.temporary_mark();
        self.heap.keep(predicate);
        self.heap.keep(input);
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
