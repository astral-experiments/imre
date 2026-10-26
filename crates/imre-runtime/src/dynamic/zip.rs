use super::Context;
use super::value::{Error, Id, Result, Value};

impl Context {
    /// Retain a complete row between the loop's availability check and its next operation.
    pub(super) fn zip_has_item(&mut self, id: Id) -> Result<bool> {
        let Value::Zip {
            inputs,
            strict,
            pending,
        } = self.heap.get(id)
        else {
            unreachable!("zip iterator")
        };
        if pending.is_some() {
            return Ok(true);
        }
        if inputs.is_empty() {
            return Ok(false);
        }
        let inputs = inputs.clone();
        let strict = *strict;
        let mut row = Vec::with_capacity(inputs.len());
        for (index, &input) in inputs.iter().enumerate() {
            let Some(value) = self.next_item(input)? else {
                if strict {
                    if index != 0 {
                        return Err(length_mismatch(index, "shorter"));
                    }
                    for (other, &input) in inputs.iter().enumerate().skip(1) {
                        if self.next_item(input)?.is_some() {
                            return Err(length_mismatch(other, "longer"));
                        }
                    }
                }
                return Ok(false);
            };
            self.heap.keep(value);
            row.push(value);
        }
        let tuple = self.alloc(Value::Tuple(row));
        if let Value::Zip { pending, .. } = self.heap.get_mut(id) {
            *pending = Some(tuple);
        }
        Ok(true)
    }
}

/// Describe the first input that fails the strict length check.
fn length_mismatch(index: usize, comparison: &str) -> Error {
    let previous = if index == 1 {
        String::from("argument 1")
    } else {
        format!("arguments 1-{index}")
    };
    Error::new(
        "ValueError",
        format!(
            "zip() argument {} is {comparison} than {previous}",
            index + 1
        ),
    )
}
