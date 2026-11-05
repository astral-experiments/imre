//! Complex numbers crossing the runtime-loaded Python boundary.

use super::Context;
use super::builtins::bind;
use super::value::{Error, Id, Result, Value};

pub(super) const METHODS: &[&str] = &[
    "__complex__",
    "__mul__",
    "__rmul__",
    "__truediv__",
    "__rtruediv__",
    "__pow__",
    "__rpow__",
];

#[derive(Clone, Copy, PartialEq)]
pub(super) struct Complex {
    pub real: f64,
    pub imag: f64,
}

impl From<f64> for Complex {
    fn from(real: f64) -> Self {
        Self { real, imag: 0.0 }
    }
}

impl Complex {
    fn parse(source: &str) -> Option<Self> {
        let mut body = source.trim();
        if let Some(inner) = body.strip_prefix('(') {
            body = inner.strip_suffix(')')?.trim();
        }
        // float() accepts surrounding whitespace, but neither component may have it here.
        if body.contains(char::is_whitespace) {
            return None;
        }
        let Some(coefficient) = body.strip_suffix(['j', 'J']) else {
            return Some(Self {
                real: crate::floats::parse(body)?,
                imag: 0.0,
            });
        };
        // A sign following an exponent marker belongs to the same component.
        let separator = coefficient
            .as_bytes()
            .windows(2)
            .position(|pair| matches!(pair[1], b'+' | b'-') && !matches!(pair[0], b'e' | b'E'));
        let (real, imaginary) = if let Some(index) = separator {
            let (real, imaginary) = coefficient.split_at(index + 1);
            (crate::floats::parse(real)?, imaginary)
        } else {
            (0.0, coefficient)
        };
        let imag = match imaginary {
            "" | "+" => 1.0,
            "-" => -1.0,
            number => crate::floats::parse(number)?,
        };
        Some(Self { real, imag })
    }

    /// CPython diagnoses misplaced underscores before parsing the complex grammar.
    /// Its Unicode-to-ASCII conversion stops at the first untranslatable character.
    fn invalid_underscores(source: impl Iterator<Item = char>, ascii: bool) -> bool {
        let mut chars = source.peekable();
        let mut previous_digit = false;
        let mut saw_underscore = false;
        while let Some(ch) = chars.next() {
            if ch == '\0' {
                return saw_underscore;
            }
            if ch == '_' {
                saw_underscore = true;
                if !previous_digit
                    || chars
                        .peek()
                        .copied()
                        .and_then(crate::integer_text::decimal_digit)
                        .is_none()
                {
                    return true;
                }
            }
            previous_digit = crate::integer_text::decimal_digit(ch).is_some();
            if !ascii && ch >= '\x7f' && !previous_digit && !ch.is_whitespace() {
                break;
            }
        }
        false
    }

    fn is_infinite(self) -> bool {
        self.real.is_infinite() || self.imag.is_infinite()
    }

    /// Replace infinite components with signed ones and all other components with signed zeros.
    fn infinity_direction(self) -> Self {
        let component = |x: f64| (if x.is_infinite() { 1.0_f64 } else { 0.0 }).copysign(x);
        Self {
            real: component(self.real),
            imag: component(self.imag),
        }
    }

    fn product(self, other: Self) -> Self {
        let ac = self.real * other.real;
        let bd = self.imag * other.imag;
        let ad = self.real * other.imag;
        let bc = self.imag * other.real;
        let result = Self {
            real: ac - bd,
            imag: ad + bc,
        };
        if result.real.is_nan()
            && result.imag.is_nan()
            && (self.is_infinite()
                || other.is_infinite()
                || [ac, bd, ad, bc].into_iter().any(f64::is_infinite))
        {
            // Recover infinities lost to NaNs or intermediate overflow (C11 Annex G.5.1).
            let factor = |z: Self| {
                if z.is_infinite() {
                    return z.infinity_direction();
                }
                let component = |x: f64| if x.is_nan() { 0.0_f64.copysign(x) } else { x };
                Self {
                    real: component(z.real),
                    imag: component(z.imag),
                }
            };
            let a = factor(self);
            let b = factor(other);
            return Self {
                real: f64::INFINITY * (a.real * b.real - a.imag * b.imag),
                imag: f64::INFINITY * (a.real * b.imag + a.imag * b.real),
            };
        }
        result
    }

    /// Divide by the larger denominator component to avoid squaring large or small values.
    /// A real dividend has no imaginary term, which matters for signed zeros and infinities.
    fn quotient(real: f64, imag: Option<f64>, denominator: Self) -> Result<Self> {
        let c = denominator.real;
        let d = denominator.imag;
        let (x, y, scale) = if c.abs() >= d.abs() {
            if c == 0.0 {
                return Err(Error::new("ZeroDivisionError", "division by zero"));
            }
            let ratio = d / c;
            let (x, y) = match imag {
                Some(b) => (real + b * ratio, b - real * ratio),
                None => (real, -real * ratio),
            };
            (x, y, c + d * ratio)
        } else if d.abs() >= c.abs() {
            let ratio = c / d;
            let (x, y) = match imag {
                Some(b) => (real * ratio + b, b * ratio - real),
                None => (real * ratio, -real),
            };
            (x, y, c * ratio + d)
        } else {
            // A denominator component is NaN, but the other may still be infinite.
            (f64::NAN, f64::NAN, f64::NAN)
        };
        let result = Self {
            real: x / scale,
            imag: y / scale,
        };
        if result.real.is_nan() && result.imag.is_nan() {
            // Recover infinities in a complex dividend and signed zeros for an infinite divisor.
            if let Some(b) = imag {
                let numerator = Self { real, imag: b };
                if numerator.is_infinite() && c.is_finite() && d.is_finite() {
                    let a = numerator.infinity_direction();
                    return Ok(Self {
                        real: f64::INFINITY * (a.real * c + a.imag * d),
                        imag: f64::INFINITY * (a.imag * c - a.real * d),
                    });
                }
            }
            if denominator.is_infinite() && real.is_finite() && imag.is_none_or(f64::is_finite) {
                let b = denominator.infinity_direction();
                let (x, y) = match imag {
                    Some(a) => (real * b.real + a * b.imag, a * b.real - real * b.imag),
                    None => (real * b.real, -real * b.imag),
                };
                return Ok(Self {
                    real: 0.0 * x,
                    imag: 0.0 * y,
                });
            }
        }
        Ok(result)
    }

    /// Small integer powers use repeated squaring. Other exponents use the principal argument.
    pub(super) fn power(self, exponent: Self) -> Result<Self> {
        let domain_error =
            || Error::new("ZeroDivisionError", "zero to a negative or complex power");
        let result = if exponent.imag == 0.0
            && exponent.real.trunc() == exponent.real
            && exponent.real.abs() <= 100.0
        {
            let mut n = exponent.real.abs() as u32;
            let mut factor = self;
            let mut result = Self::from(1.0);
            while n != 0 {
                if n & 1 != 0 {
                    result = result.product(factor);
                }
                n >>= 1;
                factor = factor.product(factor);
            }
            if exponent.real < 0.0 {
                Self::quotient(1.0, Some(0.0), result).map_err(|_| domain_error())?
            } else {
                result
            }
        } else if self.real == 0.0 && self.imag == 0.0 {
            if exponent.imag != 0.0 || exponent.real < 0.0 {
                return Err(domain_error());
            }
            Self::from(0.0)
        } else {
            let radius = self.real.hypot(self.imag);
            let argument = self.imag.atan2(self.real);
            let mut length = radius.powf(exponent.real);
            let mut phase = argument * exponent.real;
            if exponent.imag != 0.0 {
                length *= (-argument * exponent.imag).exp();
                phase += exponent.imag * radius.ln();
            }
            // CPython propagates libm's domain error for sin or cos of an infinite phase.
            if phase.is_infinite() {
                return Err(domain_error());
            }
            Self {
                real: length * phase.cos(),
                imag: length * phase.sin(),
            }
        };
        if result.is_infinite() {
            return Err(Error::new("OverflowError", "complex exponentiation"));
        }
        Ok(result)
    }

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
    pub(super) fn real_component(&self, value: Id) -> Result<Option<f64>> {
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
            let value = args[0];
            if matches!(self.heap.get(value), Value::Complex(_)) {
                return Ok(value);
            }
            if let Some(storage) = self.string_storage(value) {
                let misplaced = match self.heap.get(storage) {
                    Value::Str(source) => {
                        if let Some(number) = Complex::parse(source) {
                            return Ok(self.alloc(Value::Complex(number)));
                        }
                        Complex::invalid_underscores(source.chars(), source.is_ascii())
                    }
                    // A surrogate ends CPython's numeric text conversion.
                    Value::SurrogateStr(points) => Complex::invalid_underscores(
                        points.iter().map_while(|&p| char::from_u32(p)),
                        false,
                    ),
                    _ => unreachable!("string storage"),
                };
                let message = if misplaced {
                    format!(
                        "could not convert string to complex: {}",
                        self.checked_format(value, true)?
                    )
                } else {
                    "complex() arg is a malformed string".to_owned()
                };
                return Err(Error::new("ValueError", message));
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
        // Only the single positional form reads the stored double of a float subclass.
        let convert = if single {
            super::builtins::from_numeric
        } else {
            super::builtins::from_numeric_slots
        };
        let mut component = |argument: Option<Id>| -> Result<f64> {
            let Some(value) = argument else {
                return Ok(0.0);
            };
            if let Some(number) = self.real_component(value)? {
                return Ok(number);
            }
            let Some(number) = convert(self, value)? else {
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

    /// Convert the two numeric operands before rejecting a non-None modulus.
    pub(super) fn complex_power(
        &mut self,
        base: Id,
        exponent: Id,
        modulus: Option<Id>,
    ) -> Result<Option<Id>> {
        let number = |id| {
            let value = self.scalar_value(id);
            match self.heap.get(value) {
                Value::Complex(z) => Ok(Some(*z)),
                _ => self.real_component(value).map(|n| n.map(Complex::from)),
            }
        };
        let Some(base) = number(base)? else {
            return Ok(None);
        };
        let Some(exponent) = number(exponent)? else {
            return Ok(None);
        };
        if modulus.is_some_and(|value| value != super::value::NONE) {
            return Err(Error::new("ValueError", "complex modulo"));
        }
        Ok(Some(self.alloc(Value::Complex(base.power(exponent)?))))
    }

    /// Keep real operands real during complex arithmetic, preserving signed zeros and infinities.
    pub(super) fn complex_product_or_quotient(
        &mut self,
        op: &str,
        left: Id,
        right: Id,
    ) -> Result<Option<Id>> {
        debug_assert!(matches!(op, "*" | "/"));
        let left = self.scalar_value(left);
        let right = self.scalar_value(right);
        let scale = |z: Complex, n| Complex {
            real: z.real * n,
            imag: z.imag * n,
        };
        let number = match (self.heap.get(left), self.heap.get(right)) {
            (Value::Complex(a), Value::Complex(b)) => {
                if op == "*" {
                    a.product(*b)
                } else {
                    Complex::quotient(a.real, Some(a.imag), *b)?
                }
            }
            (Value::Complex(a), _) if let Some(b) = self.real_component(right)? => {
                if op == "*" {
                    scale(*a, b)
                } else {
                    if b == 0.0 {
                        return Err(Error::new("ZeroDivisionError", "division by zero"));
                    }
                    Complex {
                        real: a.real / b,
                        imag: a.imag / b,
                    }
                }
            }
            (_, Value::Complex(b)) if let Some(a) = self.real_component(left)? => {
                if op == "*" {
                    scale(*b, a)
                } else {
                    Complex::quotient(a, None, *b)?
                }
            }
            _ => return Ok(None),
        };
        Ok(Some(self.alloc(Value::Complex(number))))
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
