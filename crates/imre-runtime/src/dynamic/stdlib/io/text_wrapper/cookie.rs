//! CPython packs a safe byte offset and decoder replay instructions into a 21-byte integer.

use num_bigint::{BigInt, Sign};

use super::{Context, Error, Id, Result, Value};

#[derive(Clone, Copy, Default)]
pub(super) struct Cookie {
    pub start: i64,
    pub flags: i32,
    pub feed: i32,
    pub skip: i32,
    pub eof: bool,
}

impl Cookie {
    pub(super) fn parse(value: &crate::Int) -> Result<Self> {
        let n = value.big();
        if n.sign() == Sign::Minus || n.bits() > 168 {
            return Err(Error::new("OverflowError", "int too big to convert"));
        }
        let mut digits = n.iter_u32_digits();
        let [low, high, flags, feed, skip, eof] =
            std::array::from_fn(|_| digits.next().unwrap_or(0));
        let cookie = Self {
            start: i64::from(low) | (i64::from(high) << 32),
            flags: flags as i32,
            feed: feed as i32,
            skip: skip as i32,
            eof: eof != 0,
        };
        if cookie.skip < 0 || (cookie.skip > 0 && cookie.feed < 0) {
            return Err(Error::new("OSError", "can't restore logical file position"));
        }
        Ok(cookie)
    }

    pub(super) fn value(self, ctx: &mut Context) -> Id {
        if self.start >= 0 && self.flags == 0 && self.feed == 0 && self.skip == 0 && !self.eof {
            return ctx.int(self.start);
        }
        let words = vec![
            self.start as u32,
            (self.start as u64 >> 32) as u32,
            self.flags as u32,
            self.feed as u32,
            self.skip as u32,
            u32::from(self.eof),
        ];
        ctx.alloc(Value::Int(crate::Int::new(BigInt::new(Sign::Plus, words))))
    }
}
