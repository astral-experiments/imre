//! The compact pickle constructor preserves civil components before any arithmetic normalization.
use super::*;

/// Ordinary instances use local microseconds since ordinal one. Packed instances retain
/// CPython's field bytes, which can describe dates or clocks outside constructor ranges.
#[derive(Clone, Copy)]
pub(in crate::objects) enum Civil {
    Scalar(i64),
    Packed([u8; 10]),
}

impl Civil {
    pub(super) fn from_micros(local: i128) -> Self {
        // Canonical civil values lie between 0001-01-01 and 9999-12-31.
        Self::Scalar(i64::try_from(local).expect("civil microseconds fit i64"))
    }

    pub(super) fn from_ordinal(ordinal: i128) -> Self {
        Self::from_micros((ordinal - 1) * DAY)
    }

    pub(super) fn ymd(self) -> [i32; 3] {
        match self {
            Self::Scalar(local) => {
                let (y, m, d) = ymd(i128::from(local).div_euclid(DAY) + 1);
                [y, m, d]
            }
            Self::Packed(b) => [
                i32::from(u16::from_be_bytes([b[0], b[1]])),
                b[2].into(),
                b[3].into(),
            ],
        }
    }

    pub(super) fn clock(self, fold: i32) -> [i128; 5] {
        match self {
            Self::Scalar(local) => time_fields(i128::from(local), fold),
            Self::Packed(b) => [
                b[4].into(),
                b[5].into(),
                b[6].into(),
                u32::from_be_bytes([0, b[7], b[8], b[9]]).into(),
                fold.into(),
            ],
        }
    }

    pub(super) fn fields(self, fold: i32) -> [i128; 8] {
        let [y, m, d] = self.ymd().map(i128::from);
        let [h, mi, s, us, f] = self.clock(fold);
        [y, m, d, h, mi, s, us, f]
    }

    pub(super) fn ordinal(self) -> i128 {
        match self {
            Self::Scalar(local) => i128::from(local).div_euclid(DAY) + 1,
            Self::Packed(_) => {
                let [y, m, d] = self.ymd();
                ordinal(y, m, d)
            }
        }
    }

    pub(super) fn micros(self) -> i128 {
        match self {
            Self::Scalar(local) => i128::from(local),
            Self::Packed(_) => {
                let [h, mi, s, us, _] = self.clock(0);
                (self.ordinal() - 1) * DAY + ((h * 60 + mi) * 60 + s) * 1_000_000 + us
            }
        }
    }

    pub(super) fn compare(self, other: Self) -> std::cmp::Ordering {
        match (self, other) {
            (Self::Scalar(a), Self::Scalar(b)) => a.cmp(&b),
            (Self::Packed(a), Self::Packed(b)) => a.cmp(&b),
            _ => self.fields(0).cmp(&other.fields(0)),
        }
    }
}
