use super::{Rc, Str, Words};
use std::cmp::Ordering;

/// Each UTF-8 row stores its common byte prefix with the previous row and its
/// remaining bytes. A two-bit side tag lets equal rows serve both inputs.
pub(super) struct Packed {
    data: Box<[u8]>,
    counts: (usize, usize),
}

fn put(mut n: usize, bytes: &mut Vec<u8>) {
    while n >= 128 {
        bytes.push((n as u8) | 128);
        n >>= 7;
    }
    bytes.push(n as u8);
}

fn take(bytes: &mut &[u8]) -> usize {
    let mut value = 0;
    for shift in (0..usize::BITS).step_by(7) {
        let (&byte, rest) = bytes.split_first().expect("complete snapshot varint");
        *bytes = rest;
        value |= usize::from(byte & 127) << shift;
        if byte < 128 {
            return value;
        }
    }
    unreachable!("snapshot lengths fit usize")
}

impl Packed {
    pub(super) fn new(left: &Words, right: &Words) -> Self {
        let counts = (left.values.len(), right.values.len());
        let mut left = left.strings().map(|(_, s)| s.as_bytes()).peekable();
        let mut right = right.strings().map(|(_, s)| s.as_bytes()).peekable();
        let mut data = Vec::new();
        let mut previous: &[u8] = &[];
        loop {
            let order = match (left.peek(), right.peek()) {
                (None, None) => break,
                (Some(_), None) => Ordering::Less,
                (None, Some(_)) => Ordering::Greater,
                (Some(l), Some(r)) => l.cmp(r),
            };
            let (side, row) = match order {
                Ordering::Less => (1, left.next().unwrap()),
                Ordering::Greater => (2, right.next().unwrap()),
                Ordering::Equal => {
                    right.next();
                    (3, left.next().unwrap())
                }
            };
            let prefix = if std::ptr::eq(previous, row) {
                row.len()
            } else {
                previous.iter().zip(row).take_while(|(l, r)| l == r).count()
            };
            data.push(side);
            put(prefix, &mut data);
            put(row.len() - prefix, &mut data);
            data.extend_from_slice(&row[prefix..]);
            previous = row;
        }
        Self {
            data: data.into_boxed_slice(),
            counts,
        }
    }

    pub(super) fn unpack(&self) -> (Words, Words) {
        let (mut left, mut right) = (Words::new(3), Words::new(3));
        left.values.reserve(self.counts.0);
        right.values.reserve(self.counts.1);
        let mut data = &*self.data;
        let mut row = Vec::new();
        let mut string = Rc::new(Str::new(""));
        while let Some((&side, rest)) = data.split_first() {
            data = rest;
            let prefix = take(&mut data);
            let len = take(&mut data);
            let (suffix, rest) = data.split_at(len);
            data = rest;
            // A repeated large string must not gain one allocation per occurrence.
            if prefix != row.len() || len != 0 {
                row.truncate(prefix);
                row.extend_from_slice(suffix);
                string = Rc::new(Str::new(
                    std::str::from_utf8(&row).expect("complete UTF-8 row"),
                ));
            }
            if side & 1 != 0 {
                left.values.push(Rc::into_raw(Rc::clone(&string)) as usize);
            }
            if side & 2 != 0 {
                right.values.push(Rc::into_raw(Rc::clone(&string)) as usize);
            }
        }
        left.update_header();
        right.update_header();
        (left, right)
    }
}
