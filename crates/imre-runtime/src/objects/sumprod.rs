//! Native list adapters for the shared sum-of-products reducer.
use super::exceptions::{Error, Kind, Result, finish};
use super::{Object, object};
use crate::sumprod::{Number, Operands, Side, reduce};
use crate::{Int, List, integer_binary};

enum Scalar {
    Int(Int),
    Float(f64),
}
struct Lists<I> {
    left: I,
    right: I,
}

impl<I: Iterator<Item = Scalar>> Operands for Lists<I> {
    type Value = Scalar;
    type Error = Error;

    fn next(&mut self, side: Side) -> Result<Option<Scalar>> {
        Ok(match side {
            Side::Left => self.left.next(),
            Side::Right => self.right.next(),
        })
    }

    fn number<'a>(&'a self, value: &'a Scalar) -> Number<'a> {
        match value {
            Scalar::Int(x) => Number::Int(x),
            Scalar::Float(x) => Number::Float(*x),
        }
    }

    fn integer(&mut self, value: i64) -> Scalar {
        Scalar::Int(Int::from(value))
    }

    fn float(&mut self, value: f64) -> Scalar {
        Scalar::Float(value)
    }

    fn multiply(&mut self, x: &Scalar, y: &Scalar) -> Result<Scalar> {
        match (x, y) {
            (Scalar::Int(x), Scalar::Int(y)) => integer_binary(x, y, 2).map(Scalar::Int),
            (Scalar::Float(x), Scalar::Float(y)) => Ok(Scalar::Float(x * y)),
            _ => unreachable!("the input lists have the same element type"),
        }
    }

    fn add(&mut self, x: &Scalar, y: &Scalar) -> Result<Scalar> {
        match (x, y) {
            (Scalar::Int(x), Scalar::Int(y)) => Ok(Scalar::Int(x.add(y))),
            (Scalar::Float(x), Scalar::Float(y)) => Ok(Scalar::Float(x + y)),
            // Integer zero is added to the first float subtotal or product.
            (Scalar::Int(x), Scalar::Float(y)) => {
                Ok(Scalar::Float(crate::floats::int_to_float(x)? + y))
            }
            _ => unreachable!("a float total cannot receive an integer from float lists"),
        }
    }

    fn value_error(&self, message: &'static str) -> Error {
        Error::new(Kind::ValueError, message)
    }
}

/// Reduce two native lists of exact integers.
///
/// # Safety
/// Both lists stay rooted. No operation can call Python while their storage is
/// borrowed. The caller owns the integer returned on success.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_sumprod_int_lists(left: *const List, right: *const List) -> usize {
    // SAFETY: both lists stay rooted and the reduction uses only exact integers.
    let result = unsafe {
        let (left, right) = (&*left, &*right);
        left.0.with_pair(&right.0, |a, b| {
            let ints = |x: &Int| Scalar::Int(x.clone());
            reduce(&mut Lists {
                left: a.iter().map(ints),
                right: b.iter().map(ints),
            })
        })
    };
    finish(
        result.map(|total| match total {
            Scalar::Int(i) => i.into_raw(),
            _ => unreachable!("sum of integer products is an integer"),
        }),
        1,
    )
}

/// Both pointers must be rooted lists of exact floats throughout the reduction.
unsafe fn reduce_float_lists(left: *const Object, right: *const Object) -> Result<Scalar> {
    // SAFETY: exact floats have no callbacks; neither shared borrow escapes the call.
    unsafe {
        object(left).0.with_pair(&object(right).0, |a, b| {
            let float = |word: &usize| Scalar::Float(f64::from_bits(*word as u64));
            reduce(&mut Lists {
                left: a.sequence()?.values.iter().map(float),
                right: b.sequence()?.values.iter().map(float),
            })
        })
    }
}

/// Reduce two native float lists when the result is known to be a float.
///
/// # Safety
/// Both objects are rooted lists of exact floats, and the compiler proves that
/// at least one is nonempty. Equal lengths therefore imply a nonempty reduction.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_sumprod_float_lists(
    left: *const Object,
    right: *const Object,
) -> f64 {
    // SAFETY: the checked caller supplies two rooted float lists.
    let result = unsafe { reduce_float_lists(left, right) };
    finish(
        result.map(|total| match total {
            Scalar::Float(x) => x,
            _ => unreachable!("the successful reduction is nonempty"),
        }),
        0.0,
    )
}

/// Return an owning scalar view when an empty reduction may yield integer zero.
///
/// # Safety
/// Both pointers must be rooted lists of exact floats. The caller owns the result.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_sumprod_number_lists(
    left: *const Object,
    right: *const Object,
) -> *const Object {
    // SAFETY: the checked caller supplies two rooted float lists.
    let result = unsafe { reduce_float_lists(left, right) };
    finish(
        result.map(|value| {
            // SAFETY: the scalar stays live while the view acquires its concrete value.
            unsafe {
                match value {
                    Scalar::Int(i) => super::boxed_values::imre_box_value(2, i.0),
                    Scalar::Float(f) => {
                        super::boxed_values::imre_box_value(7, f.to_bits() as usize)
                    }
                }
            }
        }),
        std::ptr::null(),
    )
}
