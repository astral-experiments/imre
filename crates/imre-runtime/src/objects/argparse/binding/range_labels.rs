//! Render range choices without allocating inside BigInt addition or Display.

use std::cmp::Ordering;
use std::fmt::{self, Write};

use num_bigint::{BigInt, Sign};

use super::IntegerRange;

/// Little-endian groups of nine decimal digits. Zero has no groups.
struct Decimal {
    negative: bool,
    groups: Vec<u32>,
}

const BASE: i64 = 1_000_000_000;

impl Decimal {
    fn new(value: &BigInt) -> Result<Self, fmt::Error> {
        let mut groups = Vec::<u32>::new();
        for limb in value.iter_u32_digits().rev() {
            let mut carry = u64::from(limb);
            for group in &mut groups {
                let word = (u64::from(*group) << 32) + carry;
                *group = (word % BASE as u64) as u32;
                carry = word / BASE as u64;
            }
            while carry != 0 {
                groups.try_reserve(1).map_err(|_| fmt::Error)?;
                groups.push((carry % BASE as u64) as u32);
                carry /= BASE as u64;
            }
        }
        Ok(Self {
            negative: value.sign() == Sign::Minus,
            groups,
        })
    }

    fn magnitude_cmp(&self, other: &Self) -> Ordering {
        self.groups
            .len()
            .cmp(&other.groups.len())
            .then_with(|| self.groups.iter().rev().cmp(other.groups.iter().rev()))
    }

    fn cmp(&self, other: &Self) -> Ordering {
        match (self.negative, other.negative) {
            (true, false) => Ordering::Less,
            (false, true) => Ordering::Greater,
            (true, true) => other.magnitude_cmp(self),
            (false, false) => self.magnitude_cmp(other),
        }
    }

    fn advance(&mut self, step: &Self) -> fmt::Result {
        let add = self.negative == step.negative;
        let reverse = !add && self.magnitude_cmp(step).is_lt();
        let length = self.groups.len().max(step.groups.len());
        self.groups
            .try_reserve(length - self.groups.len())
            .map_err(|_| fmt::Error)?;
        self.groups.resize(length, 0);

        let mut carry = 0;
        for (index, group) in self.groups.iter_mut().enumerate() {
            let a = i64::from(*group);
            let b = i64::from(step.groups.get(index).copied().unwrap_or(0));
            let value = if add {
                a + b
            } else if reverse {
                b - a
            } else {
                a - b
            } + carry;
            *group = value.rem_euclid(BASE) as u32;
            carry = value.div_euclid(BASE);
        }
        if carry > 0 {
            self.groups.try_reserve(1).map_err(|_| fmt::Error)?;
            self.groups.push(carry as u32);
        }
        while self.groups.last() == Some(&0) {
            self.groups.pop();
        }
        self.negative = !self.groups.is_empty()
            && if reverse {
                step.negative
            } else {
                self.negative
            };
        Ok(())
    }

    fn write(&self, out: &mut impl Write) -> fmt::Result {
        let Some((highest, rest)) = self.groups.split_last() else {
            return out.write_char('0');
        };
        if self.negative {
            out.write_char('-')?;
        }
        write!(out, "{highest}")?;
        for group in rest.iter().rev() {
            write!(out, "{group:09}")?;
        }
        Ok(())
    }
}

impl IntegerRange {
    pub(in super::super) fn write_choices(&self, out: &mut impl Write) -> fmt::Result {
        out.write_char('{')?;
        let direction = self.0.cmp(&self.1);
        if matches!(
            (direction, self.2.sign()),
            (Ordering::Less, Sign::Plus) | (Ordering::Greater, Sign::Minus)
        ) {
            let mut current = Decimal::new(&self.0)?;
            let stop = Decimal::new(&self.1)?;
            let step = Decimal::new(&self.2)?;
            loop {
                current.write(out)?;
                current.advance(&step)?;
                if current.cmp(&stop) != direction {
                    break;
                }
                out.write_char(',')?;
            }
        }
        out.write_char('}')
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_labels_cross_sign_and_decimal_group_boundaries() {
        let large = BigInt::from(1u32) << 143usize;
        let base = BigInt::from(BASE);
        let starts = [
            -&large,
            -&base,
            (-1).into(),
            0.into(),
            1.into(),
            base.clone(),
            large.clone(),
        ];
        let steps = [-&large, -&base - 1, (-1).into(), 1.into(), &base + 1, large];
        for start in &starts {
            for step in &steps {
                for count in [-2, 0, 1, 3] {
                    let range = IntegerRange(start.clone(), start + step * count, step.clone());
                    let expected = format!(
                        "{{{}}}",
                        range
                            .iter()
                            .map(|n| n.to_string())
                            .collect::<Vec<_>>()
                            .join(",")
                    );
                    let mut actual = String::new();
                    range.write_choices(&mut actual).unwrap();
                    assert_eq!(actual, expected);
                }
            }
        }
    }
}
