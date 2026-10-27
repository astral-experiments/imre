use super::Context;
use super::value::{Error, Id, Result, Value};

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
        let (inputs, strict, function, name) = match self.heap.get(id) {
            Value::Zip { inputs, strict } => (inputs.clone(), *strict, None, "zip"),
            Value::Map {
                inputs,
                strict,
                function,
            } => (inputs.clone(), *strict, Some(*function), "map"),
            _ => unreachable!("zip or map iterator"),
        };
        if inputs.is_empty() {
            return Ok(None);
        }
        let mut row = Vec::with_capacity(inputs.len());
        for (index, &input) in inputs.iter().enumerate() {
            let value = if strict {
                self.next_item(input)?
            } else {
                self.next_result(input)?
            };
            let Some(value) = value else {
                if strict {
                    if index != 0 {
                        return Err(length_mismatch(name, index, "shorter"));
                    }
                    for (other, &input) in inputs.iter().enumerate().skip(1) {
                        if self.next_item(input)?.is_some() {
                            return Err(length_mismatch(name, other, "longer"));
                        }
                    }
                }
                return Ok(None);
            };
            self.heap.keep(value);
            row.push(value);
        }
        match function {
            Some(function) => self.call(function, row, Vec::new()).map(Some),
            None => Ok(Some(self.alloc(Value::Tuple(row)))),
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
            ("map", Value::Map { .. }) | ("enumerate", Value::Enumerate { .. })
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
