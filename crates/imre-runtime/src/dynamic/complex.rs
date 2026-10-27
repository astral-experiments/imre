//! Complex numbers crossing the runtime-loaded Python boundary.

use super::Context;
use super::builtins::bind;
use super::value::{Error, Id, Result, Value};

#[derive(Clone, Copy, PartialEq)]
pub(super) struct Complex {
    pub real: f64,
    pub imag: f64,
}

impl Complex {
    pub(super) fn repr(self) -> String {
        let imag = crate::floats::render(self.imag);
        let imag = imag.strip_suffix(".0").unwrap_or(&imag);
        if self.real.to_bits() == 0 {
            return format!("{imag}j");
        }
        let real = crate::floats::render(self.real);
        let real = real.strip_suffix(".0").unwrap_or(&real);
        let sign = if imag.starts_with('-') { "" } else { "+" };
        format!("({real}{sign}{imag}j)")
    }
}

impl Context {
    /// Format an invalid argument's type as Python's `%T` formatter does.
    fn complex_argument_type(&self, value: Id) -> String {
        let name = self.type_name(value);
        let Value::Instance { class, .. } = self.heap.get(value) else {
            return name.to_owned();
        };
        let text_attr = |name| {
            self.own_class_attr(*class, name)
                .and_then(|value| self.str_value(value).ok())
        };
        let name = text_attr("__qualname__").unwrap_or(name);
        match text_attr("__module__") {
            Some(module) if !matches!(module, "builtins" | "__main__") => {
                format!("{module}.{name}")
            }
            _ => name.to_owned(),
        }
    }

    /// Convert a real numeric operand without truncating floats or overflowing large integers.
    fn real_component(&self, value: Id) -> Result<Option<f64>> {
        match self.heap.get(value) {
            Value::Float(f) => Ok(Some(*f)),
            Value::Int(_) | Value::Bool(_) => {
                Ok(Some(crate::floats::int_to_float(&self.as_int(value)?)?))
            }
            _ => Ok(None),
        }
    }

    pub(super) fn new_complex(&mut self, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
        let [mut real, imag] = bind("complex", args, kwargs, ["real", "imag"])?;
        let single = args.len() == 1 && kwargs.is_empty();
        if single {
            match self.heap.get(args[0]) {
                Value::Complex(_) => return Ok(args[0]),
                Value::Str(_) => {
                    return Err(Error::new(
                        "NotImplementedError",
                        "complex string conversion is unsupported",
                    ));
                }
                _ => {}
            }
        }
        if let Some(value) = real
            && let Some(method) = self.bound_slot(value, "__complex__")?
        {
            let result = self.call(method, Vec::new(), Vec::new())?;
            let Value::Complex(number) = *self.heap.get(result) else {
                return Err(Error::new(
                    "TypeError",
                    format!(
                        "__complex__ returned non-complex (type {})",
                        self.type_name(result)
                    ),
                ));
            };
            if single {
                return Ok(self.alloc(Value::Complex(number)));
            }
            real = Some(result);
        }
        // Check both component types before invoking either real-number conversion.
        for (argument, name) in [(real, "real"), (imag, "imag")] {
            let Some(value) = argument else { continue };
            if matches!(self.heap.get(value), Value::Complex(_)) {
                return Err(Error::new(
                    "NotImplementedError",
                    "complex-valued real or imag arguments are unsupported",
                ));
            }
            if !matches!(
                self.heap.get(value),
                Value::Float(_) | Value::Int(_) | Value::Bool(_)
            ) && self.slot(value, "__float__").is_none()
                && self.slot(value, "__index__").is_none()
            {
                let ty = self.complex_argument_type(value);
                let message = if single {
                    format!("complex() argument must be a string or a number, not {ty}")
                } else {
                    format!("complex() argument '{name}' must be a real number, not {ty}")
                };
                return Err(Error::new("TypeError", message));
            }
        }
        let mut component = |argument: Option<Id>| -> Result<f64> {
            let Some(value) = argument else {
                return Ok(0.0);
            };
            if let Some(number) = self.real_component(value)? {
                return Ok(number);
            }
            let Some(number) = super::builtins::from_numeric(self, value)? else {
                return Err(Error::new(
                    "TypeError",
                    format!("must be real number, not {}", self.type_name(value)),
                ));
            };
            let Value::Float(number) = *self.heap.get(number) else {
                unreachable!()
            };
            Ok(number)
        };
        let number = Complex {
            real: component(real)?,
            imag: component(imag)?,
        };
        Ok(self.alloc(Value::Complex(number)))
    }

    /// Compare an integer exactly, even when it cannot be represented as binary64.
    pub(super) fn complex_equal(&self, number: Complex, other: Id) -> bool {
        match self.heap.get(other) {
            Value::Complex(value) => number == *value,
            Value::Float(value) => number.imag == 0.0 && number.real == *value,
            Value::Int(_) | Value::Bool(_) if number.imag == 0.0 => {
                crate::floats::compare(&self.as_int(other).unwrap(), number.real) == 0
            }
            _ => false,
        }
    }

    /// Real operands contribute only on the real axis, preserving signed imaginary zero.
    pub(super) fn complex_sum(&mut self, add: bool, left: Id, right: Id) -> Result<Id> {
        let apply = |a, b| if add { a + b } else { a - b };
        let number = match (self.heap.get(left), self.heap.get(right)) {
            (Value::Complex(a), Value::Complex(b)) => Complex {
                real: apply(a.real, b.real),
                imag: apply(a.imag, b.imag),
            },
            (Value::Complex(a), _) if let Some(b) = self.real_component(right)? => Complex {
                real: apply(a.real, b),
                imag: a.imag,
            },
            (_, Value::Complex(b)) if let Some(a) = self.real_component(left)? => Complex {
                real: apply(a, b.real),
                imag: if add { b.imag } else { -b.imag },
            },
            _ => {
                return Err(Error::new(
                    "TypeError",
                    format!(
                        "unsupported operand type(s) for {}: '{}' and '{}'",
                        if add { "+" } else { "-" },
                        self.type_name(left),
                        self.type_name(right)
                    ),
                ));
            }
        };
        Ok(self.alloc(Value::Complex(number)))
    }
}
