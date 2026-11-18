//! Validate and construct bounded flat lists of exact primitive pickle values.

use super::super::super::{
    Context,
    value::{FALSE, Id, NONE, Result, TRUE, Value},
};

const STOP: u8 = b'.';
const NONE_OP: u8 = b'N';
const TRUE_OP: u8 = 0x88;
const FALSE_OP: u8 = 0x89;
const BININT: u8 = b'J';
const BININT1: u8 = b'K';
const BININT2: u8 = b'M';
const BINFLOAT: u8 = b'G';
const SHORT_UNICODE: u8 = 0x8c;
const BINUNICODE: u8 = b'X';
const SHORT_BYTES: u8 = b'C';
const BINBYTES: u8 = b'B';
const MEMOIZE: u8 = 0x94;
const APPENDS: u8 = b'e';

// Speculation is bounded before scanning: large or complex streams go directly
// to the general VM, and a small stream can duplicate at most sixteen operations.
const MAX_FRAME_BODY: usize = 256;
const MAX_OPERATIONS: usize = 16;

pub(super) struct Layout {
    start: usize,
    end: usize,
    items: usize,
    allocations: usize,
}

struct Cursor<'a> {
    input: &'a [u8],
    at: usize,
    end: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, count: usize) -> Option<&'a [u8]> {
        let end = self.at.checked_add(count)?;
        if end > self.end {
            return None;
        }
        let value = self.input.get(self.at..end)?;
        self.at = end;
        Some(value)
    }

    fn byte(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }

    fn length(&mut self, op: u8) -> Option<usize> {
        match op {
            SHORT_UNICODE | SHORT_BYTES => Some(usize::from(self.byte()?)),
            BINUNICODE | BINBYTES => {
                usize::try_from(u32::from_le_bytes(self.take(4)?.try_into().ok()?)).ok()
            }
            _ => None,
        }
    }
}

/// Dispatch only after the list prolog and only when the text reader cannot accept the first push.
pub(super) fn starts(op: Option<u8>) -> bool {
    matches!(
        op,
        Some(
            NONE_OP
                | TRUE_OP
                | FALSE_OP
                | BININT
                | BININT1
                | BININT2
                | BINFLOAT
                | BINUNICODE
                | SHORT_BYTES
                | BINBYTES
        )
    )
}

/// Validate the entire bounded scalar grammar before allocating any Python object.
pub(super) fn preflight(input: &[u8], start: usize, end: usize) -> Option<Layout> {
    if end.checked_sub(start)? > MAX_FRAME_BODY || end > input.len() {
        return None;
    }
    let mut cursor = Cursor {
        input,
        at: start,
        end,
    };
    let mut operations = 0usize;
    let mut items = 0usize;
    let mut allocations = 1usize; // the result list is allocated first
    loop {
        let op = cursor.byte()?;
        if op == APPENDS {
            if items == 0 || cursor.byte()? != STOP || cursor.at != end {
                return None;
            }
            return Some(Layout {
                start,
                end,
                items,
                allocations,
            });
        }
        if operations == MAX_OPERATIONS {
            return None;
        }
        operations += 1;
        match op {
            MEMOIZE if items != 0 => continue,
            NONE_OP | TRUE_OP | FALSE_OP => {}
            BININT1 => {
                cursor.take(1)?;
                allocations += 1;
            }
            BININT2 => {
                cursor.take(2)?;
                allocations += 1;
            }
            BININT => {
                cursor.take(4)?;
                allocations += 1;
            }
            BINFLOAT => {
                cursor.take(8)?;
                allocations += 1;
            }
            SHORT_UNICODE | BINUNICODE | SHORT_BYTES | BINBYTES => {
                let length = cursor.length(op)?;
                let bytes = cursor.take(length)?;
                if matches!(op, SHORT_UNICODE | BINUNICODE) {
                    std::str::from_utf8(bytes).ok()?;
                }
                allocations += 1;
            }
            _ => return None,
        }
        items += 1;
    }
}

/// Replay validated primitive pushes, preserving the VM's owner-first allocation and APPENDS failure boundary.
pub(super) fn decode(ctx: &mut Context, data: Id, layout: Layout) -> Result<Id> {
    let mut values = Vec::new();
    // The general VM reserves an ID stack of owner + all pushes before allocating
    // the owner. Keep that same capacity and recoverable reservation boundary.
    values
        .try_reserve_exact(layout.items + 1)
        .map_err(|_| crate::Error::memory_error())?;
    ctx.heap.reserve_temporaries(layout.allocations + 1)?;
    let roots = ctx.heap.temporary_mark();
    let Value::Bytes(bytes) = ctx.heap.get(data) else {
        unreachable!()
    };
    let (pointer, length) = (bytes.as_ptr(), bytes.len());
    // SAFETY: the source frame or call operands root this exact immutable Bytes allocation, whose
    // separate backing buffer survives slot-vector growth. Full preflight proved
    // every operation exact and callback-free; no mutation or collection can run.
    let input = unsafe { std::slice::from_raw_parts(pointer, length) };
    let mut cursor = Cursor {
        input,
        at: layout.start,
        end: layout.end,
    };
    let owner = ctx.alloc(Value::List(Vec::new()));
    loop {
        let op = cursor.byte().unwrap();
        if op == APPENDS {
            let stop = cursor.byte().unwrap();
            debug_assert_eq!(stop, STOP);
            let Value::List(items) = ctx.heap.get_mut(owner) else {
                unreachable!()
            };
            items
                .try_reserve(values.len())
                .map_err(|_| crate::Error::memory_error())?;
            items.extend_from_slice(&values);
            break;
        }
        let value = match op {
            MEMOIZE => continue,
            NONE_OP => NONE,
            TRUE_OP => TRUE,
            FALSE_OP => FALSE,
            BININT1 => ctx.int(i64::from(cursor.byte().unwrap())),
            BININT2 => ctx.int(i64::from(u16::from_le_bytes(
                cursor.take(2).unwrap().try_into().unwrap(),
            ))),
            BININT => ctx.int(i64::from(i32::from_le_bytes(
                cursor.take(4).unwrap().try_into().unwrap(),
            ))),
            BINFLOAT => ctx.alloc(Value::Float(f64::from_be_bytes(
                cursor.take(8).unwrap().try_into().unwrap(),
            ))),
            SHORT_UNICODE | BINUNICODE => {
                let length = cursor.length(op).unwrap();
                // SAFETY: the same rooted immutable payload passed UTF-8 validation in preflight.
                let text = unsafe { std::str::from_utf8_unchecked(cursor.take(length).unwrap()) };
                ctx.text(text)
            }
            SHORT_BYTES | BINBYTES => {
                let length = cursor.length(op).unwrap();
                ctx.alloc(Value::Bytes(cursor.take(length).unwrap().to_vec()))
            }
            _ => unreachable!("validated primitive opcode"),
        };
        values.push(value);
    }
    ctx.heap.keep_call_result(roots, owner);
    Ok(owner)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(body: &[u8]) -> Vec<u8> {
        let mut payload = vec![b']', MEMOIZE, b'('];
        payload.extend_from_slice(body);
        let mut input = vec![0x80, 5, 0x95];
        input.extend_from_slice(&(payload.len() as u64).to_le_bytes());
        input.extend_from_slice(&payload);
        input
    }

    #[test]
    fn primitive_list_preserves_scalar_values_singletons_and_owner_roots() {
        let mut body = vec![NONE_OP, TRUE_OP, FALSE_OP, BININT1, 0, BININT2];
        body.extend_from_slice(&256u16.to_le_bytes());
        body.push(BININT);
        body.extend_from_slice(&(-7i32).to_le_bytes());
        body.push(BINFLOAT);
        body.extend_from_slice(&3.125f64.to_be_bytes());
        for text in ["é", "é", "Ā"] {
            body.extend_from_slice(&[SHORT_UNICODE, text.len() as u8]);
            body.extend_from_slice(text.as_bytes());
        }
        body.extend_from_slice(&[
            MEMOIZE,
            MEMOIZE,
            SHORT_BYTES,
            3,
            STOP,
            b'(',
            0x95,
            APPENDS,
            STOP,
        ]);
        let input = frame(&body);
        let layout = preflight(&input, 14, input.len()).unwrap();
        assert_eq!(layout.items, 11);
        let mut ctx = crate::dynamic::ffi::new_context(serde_json::json!({"paths": []}), ["imre"]);
        let data = ctx.alloc(Value::Bytes(input));
        let mark = ctx.heap.temporary_mark();
        let owner = decode(&mut ctx, data, layout).unwrap();
        assert_eq!(ctx.heap.temporary_mark(), mark + 1);
        assert_eq!(*ctx.heap.temporary_root(mark), owner);
        let Value::List(items) = ctx.heap.get(owner) else {
            panic!("list")
        };
        assert_eq!(&items[..3], &[NONE, TRUE, FALSE]);
        assert_eq!(items[7], items[8]); // Latin-1 singleton
        assert_ne!(items[8], items[9]);
        assert!(
            matches!(ctx.heap.get(items[5]), Value::Int(value) if value.as_small() == Some(-7))
        );
        assert!(matches!(ctx.heap.get(items[6]), Value::Float(value) if *value == 3.125));
        assert!(matches!(ctx.heap.get(items[9]), Value::Str(value) if value == "Ā"));
        assert!(
            matches!(ctx.heap.get(items[10]), Value::Bytes(value) if value == &[STOP, b'(', 0x95])
        );
        assert!(owner < items[6] && owner < items[9] && owner < items[10]);
    }

    #[test]
    fn primitive_list_bounds_speculation_and_rejects_unsupported_or_invalid_streams() {
        for body in [
            vec![MEMOIZE, APPENDS, STOP],
            vec![NONE_OP, b'h', 0, APPENDS, STOP],
            vec![NONE_OP, b']', APPENDS, STOP],
            vec![NONE_OP, SHORT_UNICODE, 1, 0xff, APPENDS, STOP],
            vec![NONE_OP, BININT, 1, 2, APPENDS, STOP],
        ] {
            let input = frame(&body);
            assert!(preflight(&input, 14, input.len()).is_none());
        }
        let mut too_many = vec![NONE_OP; MAX_OPERATIONS + 1];
        too_many.extend_from_slice(&[APPENDS, STOP]);
        let input = frame(&too_many);
        assert!(preflight(&input, 14, input.len()).is_none());
        let input = frame(&vec![NONE_OP; MAX_FRAME_BODY + 1]);
        assert!(preflight(&input, 14, input.len()).is_none());
    }
}
