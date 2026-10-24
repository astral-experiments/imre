use super::BigUint;

use crate::big_digit::{self, BigDigit, BigDigits};

use alloc::borrow::Cow;
use alloc::collections::TryReserveError;
use alloc::vec::Vec;
use core::mem;
use core::ops::{Shl, ShlAssign, Shr, ShrAssign};
use num_traits::{PrimInt, Zero};

#[inline]
pub(super) fn biguint_shl<T: PrimInt>(n: Cow<'_, BigUint>, shift: T) -> BigUint {
    if shift < T::zero() {
        panic!("attempt to shift left with negative");
    }
    if n.is_zero() {
        return n.into_owned();
    }
    let bits = T::from(big_digit::BITS).unwrap();
    let digits = (shift / bits).to_usize().expect("capacity overflow");
    let shift = (shift % bits).to_u8().unwrap();
    biguint_shl2(n, digits, shift)
}

fn biguint_shl2(n: Cow<'_, BigUint>, digits: usize, shift: u8) -> BigUint {
    let mut data = match digits {
        0 => n.into_owned().data,
        _ => {
            let len = digits.saturating_add(n.data.len() + 1);
            let mut data = Vec::with_capacity(len);
            data.resize(digits, 0);
            data.extend(n.data.iter());
            BigDigits::from_vec(data)
        }
    };

    let carry = shl_bits(&mut data[digits..], shift);
    if carry != 0 {
        data.push(carry);
    }
    BigUint { data }
}

fn shl_bits(digits: &mut [BigDigit], shift: u8) -> BigDigit {
    let mut carry = 0;
    if shift > 0 {
        let carry_shift = big_digit::BITS - shift;
        for elem in digits {
            let new_carry = *elem >> carry_shift;
            *elem = (*elem << shift) | carry;
            carry = new_carry;
        }
    }
    carry
}

impl BigUint {
    /// Shifts left by `shift` bits, returning an error if the result cannot be allocated.
    pub fn try_shl(&self, shift: usize) -> Result<Self, TryReserveError> {
        if self.is_zero() {
            return Ok(Self::ZERO);
        }
        let whole = shift / big_digit::BITS as usize;
        let bits = (shift % big_digit::BITS as usize) as u8;
        let mut data = Vec::new();
        data.try_reserve_exact(whole + self.data.len() + usize::from(bits != 0))?;
        data.resize(whole, 0);
        data.extend_from_slice(&self.data);
        let carry = shl_bits(&mut data[whole..], bits);
        if carry != 0 {
            data.push(carry);
        }
        Ok(Self {
            data: BigDigits::from_vec(data),
        })
    }
}

#[inline]
fn biguint_shr<T: PrimInt>(n: Cow<'_, BigUint>, shift: T) -> BigUint {
    if shift < T::zero() {
        panic!("attempt to shift right with negative");
    }
    if n.is_zero() {
        return n.into_owned();
    }
    let bits = T::from(big_digit::BITS).unwrap();
    let digits = (shift / bits).to_usize().unwrap_or(usize::MAX);
    let shift = (shift % bits).to_u8().unwrap();
    biguint_shr2(n, digits, shift)
}

fn biguint_shr2(n: Cow<'_, BigUint>, digits: usize, shift: u8) -> BigUint {
    if digits >= n.data.len() {
        return match n {
            Cow::Borrowed(_) => BigUint::ZERO,
            Cow::Owned(mut n) => {
                n.set_zero();
                n
            }
        };
    }
    let mut data = match n {
        Cow::Borrowed(n) => BigDigits::from_slice(&n.data[digits..]),
        Cow::Owned(mut n) => {
            n.data.drain_front(digits);
            n.data.shrink();
            n.data
        }
    };

    if shift > 0 {
        let mut borrow = 0;
        let borrow_shift = big_digit::BITS - shift;
        for elem in data.iter_mut().rev() {
            let new_borrow = *elem << borrow_shift;
            *elem = (*elem >> shift) | borrow;
            borrow = new_borrow;
        }
        // Assuming we were normal before, only one
        // most-significant digit might be off now.
        if !data.is_normal() {
            data.pop();
        }
    }

    BigUint { data }
}

macro_rules! impl_shift {
    (@ref $Shx:ident :: $shx:ident, $ShxAssign:ident :: $shx_assign:ident, $rhs:ty) => {
        impl $Shx<&$rhs> for BigUint {
            type Output = BigUint;

            #[inline]
            fn $shx(self, rhs: &$rhs) -> BigUint {
                $Shx::$shx(self, *rhs)
            }
        }
        impl $Shx<&$rhs> for &BigUint {
            type Output = BigUint;

            #[inline]
            fn $shx(self, rhs: &$rhs) -> BigUint {
                $Shx::$shx(self, *rhs)
            }
        }
        impl $ShxAssign<&$rhs> for BigUint {
            #[inline]
            fn $shx_assign(&mut self, rhs: &$rhs) {
                $ShxAssign::$shx_assign(self, *rhs);
            }
        }
    };
    ($($rhs:ty),+) => {$(
        impl Shl<$rhs> for BigUint {
            type Output = BigUint;

            #[inline]
            fn shl(self, rhs: $rhs) -> BigUint {
                biguint_shl(Cow::Owned(self), rhs)
            }
        }
        impl Shl<$rhs> for &BigUint {
            type Output = BigUint;

            #[inline]
            fn shl(self, rhs: $rhs) -> BigUint {
                biguint_shl(Cow::Borrowed(self), rhs)
            }
        }
        impl ShlAssign<$rhs> for BigUint {
            #[inline]
            fn shl_assign(&mut self, rhs: $rhs) {
                let n = mem::replace(self, Self::ZERO);
                *self = n << rhs;
            }
        }
        impl_shift! { @ref Shl::shl, ShlAssign::shl_assign, $rhs }

        impl Shr<$rhs> for BigUint {
            type Output = BigUint;

            #[inline]
            fn shr(self, rhs: $rhs) -> BigUint {
                biguint_shr(Cow::Owned(self), rhs)
            }
        }
        impl Shr<$rhs> for &BigUint {
            type Output = BigUint;

            #[inline]
            fn shr(self, rhs: $rhs) -> BigUint {
                biguint_shr(Cow::Borrowed(self), rhs)
            }
        }
        impl ShrAssign<$rhs> for BigUint {
            #[inline]
            fn shr_assign(&mut self, rhs: $rhs) {
                let n = mem::replace(self, Self::ZERO);
                *self = n >> rhs;
            }
        }
        impl_shift! { @ref Shr::shr, ShrAssign::shr_assign, $rhs }
    )*};
}

impl_shift! { u8, u16, u32, u64, u128, usize }
impl_shift! { i8, i16, i32, i64, i128, isize }
