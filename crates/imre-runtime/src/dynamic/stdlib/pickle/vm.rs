//! Preflight and execute callback-free pickle streams with flat indexed storage.

use super::super::super::{
    Context,
    value::{FALSE, Id, NONE, Result, TRUE, Value},
};

const PROTO: u8 = 0x80;
const FRAME: u8 = 0x95;
const STOP: u8 = b'.';
const MARK: u8 = b'(';
const EMPTY_LIST: u8 = b']';
const EMPTY_DICT: u8 = b'}';
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
const BINPUT: u8 = b'q';
const BINGET: u8 = b'h';
const LONG_BINGET: u8 = b'j';
const APPEND: u8 = b'a';
const APPENDS: u8 = b'e';
const SETITEM: u8 = b's';
const SETITEMS: u8 = b'u';

#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum Kind {
    List,
    Dict,
    Scalar,
}

impl Kind {
    fn key(self) -> bool {
        self == Self::Scalar
    }
}

pub(super) struct Layout {
    start: usize,
    end: usize,
    stack: usize,
    marks: usize,
    memo: usize,
    allocations: usize,
}

struct Cursor<'a> {
    input: &'a [u8],
    at: usize,
    end: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, length: usize) -> Option<&'a [u8]> {
        let end = self.at.checked_add(length)?;
        if end > self.end {
            return None;
        }
        let bytes = self.input.get(self.at..end)?;
        self.at = end;
        Some(bytes)
    }

    fn byte(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }

    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }

    fn length(&mut self, op: u8) -> Option<usize> {
        match op {
            SHORT_UNICODE | SHORT_BYTES => Some(usize::from(self.byte()?)),
            BINUNICODE | BINBYTES => usize::try_from(self.u32()?).ok(),
            _ => None,
        }
    }

    fn index(&mut self, op: u8) -> Option<usize> {
        match op {
            BINPUT | BINGET => Some(usize::from(self.byte()?)),
            LONG_BINGET => usize::try_from(self.u32()?).ok(),
            _ => None,
        }
    }
}

fn push<T>(values: &mut Vec<T>, value: T) -> Option<()> {
    values.try_reserve(1).ok()?;
    values.push(value);
    Some(())
}

fn base(marks: &[u32]) -> usize {
    marks.last().copied().unwrap_or(0) as usize
}

fn top(stack: &[Kind], marks: &[u32]) -> Option<Kind> {
    (stack.len() > base(marks)).then(|| *stack.last().unwrap())
}

fn pending() -> bool {
    #[cfg(unix)]
    {
        crate::signals::TRIPPED.load(std::sync::atomic::Ordering::Acquire)
    }
    #[cfg(not(unix))]
    {
        false
    }
}

/// Prove all stack, frame, memo, UTF-8 and dictionary-key operations before allocating Python objects.
pub(super) fn preflight(input: &[u8]) -> Option<Layout> {
    if input.len() > u32::MAX as usize || input.first().copied()? != PROTO {
        return None;
    }
    let protocol = *input.get(1)?;
    if !matches!(protocol, 2 | 4 | 5) {
        return None;
    }
    let (at, end, framed) = if input.get(2).copied() == Some(FRAME) {
        if protocol < 4 {
            return None;
        }
        let length = u64::from_le_bytes(input.get(3..11)?.try_into().ok()?);
        let end = 11usize.checked_add(usize::try_from(length).ok()?)?;
        if end > input.len() {
            return None;
        }
        (11, end, true)
    } else {
        (2, input.len(), false)
    };
    let mut cursor = Cursor { input, at, end };
    let mut instructions = 0usize;
    let mut stack = Vec::new();
    let mut marks = Vec::new();
    let mut memo = Vec::new();
    let mut max_stack = 0;
    let mut max_marks = 0;
    let mut allocations = 0usize;
    loop {
        if instructions & 1023 == 0 && pending() {
            return None;
        }
        let op = cursor.byte()?;
        if op == STOP {
            if !marks.is_empty() || stack.len() != 1 || (framed && cursor.at != end) {
                return None;
            }
            return Some(Layout {
                start: at,
                end: cursor.at,
                stack: max_stack,
                marks: max_marks,
                memo: memo.len(),
                allocations,
            });
        }
        let mut new = None;
        match op {
            EMPTY_LIST => new = Some(Kind::List),
            EMPTY_DICT => new = Some(Kind::Dict),
            NONE_OP | TRUE_OP | FALSE_OP => new = Some(Kind::Scalar),
            BININT1 => {
                cursor.take(1)?;
                new = Some(Kind::Scalar);
            }
            BININT2 => {
                cursor.take(2)?;
                new = Some(Kind::Scalar);
            }
            BININT => {
                cursor.take(4)?;
                new = Some(Kind::Scalar);
            }
            BINFLOAT => {
                cursor.take(8)?;
                new = Some(Kind::Scalar);
            }
            SHORT_UNICODE | BINUNICODE | SHORT_BYTES | BINBYTES => {
                let length = cursor.length(op)?;
                let bytes = cursor.take(length)?;
                if matches!(op, SHORT_UNICODE | BINUNICODE) {
                    std::str::from_utf8(bytes).ok()?;
                }
                new = Some(Kind::Scalar);
            }
            MARK => {
                push(&mut marks, u32::try_from(stack.len()).ok()?)?;
                max_marks = max_marks.max(marks.len());
            }
            MEMOIZE => push(&mut memo, top(&stack, &marks)?)?,
            BINPUT => {
                if protocol != 2 || cursor.index(op)? != memo.len() {
                    return None;
                }
                push(&mut memo, top(&stack, &marks)?)?;
            }
            BINGET | LONG_BINGET => {
                let index = cursor.index(op)?;
                new = Some(*memo.get(index)?);
            }
            APPEND => {
                let start = base(&marks);
                if stack.len().checked_sub(start)? < 2 || stack[stack.len() - 2] != Kind::List {
                    return None;
                }
                stack.pop();
            }
            APPENDS | SETITEMS => {
                let mark = usize::try_from(marks.pop()?).ok()?;
                if mark <= base(&marks) {
                    return None;
                }
                if op == APPENDS {
                    if stack[mark - 1] != Kind::List {
                        return None;
                    }
                } else {
                    if stack[mark - 1] != Kind::Dict || (stack.len() - mark) % 2 != 0 {
                        return None;
                    }
                    if !stack[mark..]
                        .as_chunks::<2>()
                        .0
                        .iter()
                        .all(|pair| pair[0].key())
                    {
                        return None;
                    }
                }
                stack.truncate(mark);
            }
            SETITEM => {
                let count = stack.len().checked_sub(base(&marks))?;
                if count < 3
                    || stack[stack.len() - 3] != Kind::Dict
                    || !stack[stack.len() - 2].key()
                {
                    return None;
                }
                stack.truncate(stack.len() - 2);
            }
            _ => return None,
        }
        if let Some(kind) = new {
            push(&mut stack, kind)?;
            max_stack = max_stack.max(stack.len());
            if !matches!(op, NONE_OP | TRUE_OP | FALSE_OP | BINGET | LONG_BINGET) {
                allocations = allocations.checked_add(1)?;
            }
        }
        instructions += 1;
    }
}

/// Stream the already validated immutable input; every operation is exact and callback-free.
pub(super) fn decode(ctx: &mut Context, data: Id, layout: Layout) -> Result<Id> {
    let mut stack = Vec::new();
    let mut marks = Vec::new();
    let mut memo = Vec::new();
    stack
        .try_reserve_exact(layout.stack)
        .map_err(|_| crate::Error::memory_error())?;
    marks
        .try_reserve_exact(layout.marks)
        .map_err(|_| crate::Error::memory_error())?;
    memo.try_reserve_exact(layout.memo)
        .map_err(|_| crate::Error::memory_error())?;
    ctx.heap
        .reserve_temporaries(layout.allocations.saturating_add(1))?;
    let roots = ctx.heap.temporary_mark();
    let Value::Bytes(bytes) = ctx.heap.get(data) else {
        unreachable!()
    };
    let (pointer, length) = (bytes.as_ptr(), bytes.len());
    // SAFETY: the source frame or call operands root this exact immutable bytes object.
    // Preflight proved no operation can invoke Python or mutate this backing allocation.
    let input = unsafe { std::slice::from_raw_parts(pointer, length) };
    let mut cursor = Cursor {
        input,
        at: layout.start,
        end: layout.end,
    };
    loop {
        let op = cursor.byte().unwrap();
        if op == STOP {
            break;
        }
        let new = match op {
            EMPTY_LIST => Some(ctx.alloc(Value::List(Vec::new()))),
            EMPTY_DICT => Some(ctx.dict(Vec::new())),
            NONE_OP => Some(NONE),
            TRUE_OP => Some(TRUE),
            FALSE_OP => Some(FALSE),
            BININT1 => Some(ctx.int(i64::from(cursor.byte().unwrap()))),
            BININT2 => Some(ctx.int(i64::from(u16::from_le_bytes(
                cursor.take(2).unwrap().try_into().unwrap(),
            )))),
            BININT => Some(ctx.int(i64::from(i32::from_le_bytes(
                cursor.take(4).unwrap().try_into().unwrap(),
            )))),
            BINFLOAT => Some(ctx.alloc(Value::Float(f64::from_be_bytes(
                cursor.take(8).unwrap().try_into().unwrap(),
            )))),
            SHORT_UNICODE | BINUNICODE => {
                let length = cursor.length(op).unwrap();
                // SAFETY: preflight validated this exact immutable range as UTF-8.
                let text = unsafe { std::str::from_utf8_unchecked(cursor.take(length).unwrap()) };
                Some(ctx.text(text))
            }
            SHORT_BYTES | BINBYTES => {
                let length = cursor.length(op).unwrap();
                Some(ctx.alloc(Value::Bytes(cursor.take(length).unwrap().to_vec())))
            }
            MARK => {
                marks.push(stack.len());
                None
            }
            MEMOIZE | BINPUT => {
                if op == BINPUT {
                    cursor.index(op).unwrap();
                }
                memo.push(*stack.last().unwrap());
                None
            }
            BINGET | LONG_BINGET => Some(memo[cursor.index(op).unwrap()]),
            APPEND => {
                let item = stack.pop().unwrap();
                let list = *stack.last().unwrap();
                let Value::List(items) = ctx.heap.get_mut(list) else {
                    unreachable!()
                };
                items
                    .try_reserve(1)
                    .map_err(|_| crate::Error::memory_error())?;
                items.push(item);
                None
            }
            APPENDS => {
                let mark = marks.pop().unwrap();
                let list = stack[mark - 1];
                let Value::List(items) = ctx.heap.get_mut(list) else {
                    unreachable!()
                };
                items
                    .try_reserve(stack.len() - mark)
                    .map_err(|_| crate::Error::memory_error())?;
                items.extend_from_slice(&stack[mark..]);
                stack.truncate(mark);
                None
            }
            SETITEM => {
                let value = stack.pop().unwrap();
                let key = stack.pop().unwrap();
                ctx.put_item(*stack.last().unwrap(), key, value)?;
                None
            }
            SETITEMS => {
                let mark = marks.pop().unwrap();
                let dict = stack[mark - 1];
                for pair in stack[mark..].as_chunks::<2>().0 {
                    ctx.put_item(dict, pair[0], pair[1])?;
                }
                stack.truncate(mark);
                None
            }
            _ => unreachable!("validated opcode"),
        };
        if let Some(value) = new {
            stack.push(value);
        }
    }
    let result = stack.pop().unwrap();
    ctx.heap.keep_call_result(roots, result);
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preflight_proves_nested_containers_and_rejects_risky_keys_and_sparse_memo() {
        let alias = [
            PROTO, 5, EMPTY_LIST, MEMOIZE, MARK, EMPTY_LIST, MEMOIZE, BINGET, 1, APPENDS, STOP,
        ];
        assert!(preflight(&alias).is_some());
        let risky = [PROTO, 5, EMPTY_DICT, EMPTY_LIST, NONE_OP, SETITEM, STOP];
        assert!(preflight(&risky).is_none());
        let sparse = [PROTO, 2, EMPTY_LIST, BINPUT, 5, STOP];
        assert!(preflight(&sparse).is_none());
    }

    #[test]
    fn decode_preserves_nested_aliases_and_python_equal_dictionary_keys() {
        let mut ctx = crate::dynamic::ffi::new_context(serde_json::json!({"paths": []}), ["imre"]);
        let alias = [
            PROTO,
            5,
            EMPTY_LIST,
            MEMOIZE,
            MARK,
            EMPTY_LIST,
            MEMOIZE,
            BINGET,
            1,
            EMPTY_DICT,
            SHORT_UNICODE,
            4,
            b's',
            b'a',
            b'm',
            b'e',
            BINGET,
            1,
            SETITEM,
            APPENDS,
            STOP,
        ];
        let input = ctx.alloc(Value::Bytes(alias.to_vec()));
        let output = decode(&mut ctx, input, preflight(&alias).unwrap()).unwrap();
        let Value::List(items) = ctx.heap.get(output) else {
            panic!("list")
        };
        assert_eq!(items[0], items[1]);
        let (child, dict) = (items[0], items[2]);
        assert_eq!(ctx.dict_named_item(dict, "same"), Some(child));

        let collision = [
            PROTO, 5, EMPTY_DICT, MARK, TRUE_OP, BININT1, 11, BININT1, 1, BININT1, 22, SETITEMS,
            STOP,
        ];
        let input = ctx.alloc(Value::Bytes(collision.to_vec()));
        let output = decode(&mut ctx, input, preflight(&collision).unwrap()).unwrap();
        let Value::Dict(entries) = ctx.heap.get(output) else {
            panic!("dict")
        };
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].0, TRUE);
        assert!(
            matches!(ctx.heap.get(entries[0].1), Value::Int(value) if value.as_small() == Some(22))
        );
    }

    #[test]
    fn streamed_protocol2_consumes_memo_indices_and_stops_before_trailing_bytes() {
        let mut ctx = crate::dynamic::ffi::new_context(serde_json::json!({"paths": []}), ["imre"]);
        let pickle = [
            PROTO, 2, EMPTY_LIST, BINPUT, 0, MARK, BININT1, 7, BINGET, 0, APPENDS, STOP, 0xff,
        ];
        let input = ctx.alloc(Value::Bytes(pickle.to_vec()));
        let output = decode(&mut ctx, input, preflight(&pickle).unwrap()).unwrap();
        let Value::List(items) = ctx.heap.get(output) else {
            panic!("list")
        };
        assert!(matches!(ctx.heap.get(items[0]), Value::Int(value) if value.as_small() == Some(7)));
        assert_eq!(items[1], output);
    }

    #[test]
    fn streamed_frame_treats_stop_inside_payload_as_data() {
        let mut ctx = crate::dynamic::ffi::new_context(serde_json::json!({"paths": []}), ["imre"]);
        let mut pickle = vec![PROTO, 5, FRAME];
        pickle.extend_from_slice(&5u64.to_le_bytes());
        pickle.extend_from_slice(&[SHORT_BYTES, 2, STOP, b'x', STOP]);
        let input = ctx.alloc(Value::Bytes(pickle.clone()));
        let output = decode(&mut ctx, input, preflight(&pickle).unwrap()).unwrap();
        assert!(matches!(ctx.heap.get(output), Value::Bytes(bytes) if bytes == &[STOP, b'x']));

        pickle[3] = 6;
        pickle.push(0xff);
        assert!(preflight(&pickle).is_none());
    }

    #[test]
    fn streamed_protocol2_keeps_opcode_valued_indices_and_payloads_in_their_fields() {
        let mut ctx = crate::dynamic::ffi::new_context(serde_json::json!({"paths": []}), ["imre"]);
        let mut pickle = vec![PROTO, 2, EMPTY_LIST, BINPUT, 0, MARK];
        let mut last_index_at = 0;
        for index in 1u8..=MARK {
            let value = if index == MARK {
                i32::from_le_bytes([FRAME, MARK, STOP, 0])
            } else {
                1000 + i32::from(index)
            };
            pickle.push(BININT);
            pickle.extend_from_slice(&value.to_le_bytes());
            pickle.push(BINPUT);
            last_index_at = pickle.len();
            pickle.push(index);
        }
        pickle.extend_from_slice(&[BINGET, MARK, LONG_BINGET]);
        pickle.extend_from_slice(&u32::from(MARK).to_le_bytes());
        pickle.extend_from_slice(&[BINGET, 0, APPENDS, STOP, 0xff]);

        let input = ctx.alloc(Value::Bytes(pickle.clone()));
        let output = decode(&mut ctx, input, preflight(&pickle).unwrap()).unwrap();
        let Value::List(items) = ctx.heap.get(output) else {
            panic!("list")
        };
        assert_eq!(items.len(), usize::from(MARK) + 3);
        assert_eq!(items[usize::from(MARK) - 1], items[usize::from(MARK)]);
        assert_eq!(items[usize::from(MARK)], items[usize::from(MARK) + 1]);
        assert_eq!(items[usize::from(MARK) + 2], output);
        assert!(
            matches!(ctx.heap.get(items[usize::from(MARK) - 1]), Value::Int(value)
            if value.as_small() == Some(i64::from(i32::from_le_bytes([FRAME, MARK, STOP, 0]))))
        );

        pickle[last_index_at] = MARK - 1;
        assert!(
            preflight(&pickle).is_none(),
            "a repeated BINPUT index must fall back"
        );
    }

    #[test]
    fn streamed_frame_bounds_payloads_aliases_and_trailing_bytes() {
        let mut ctx = crate::dynamic::ffi::new_context(serde_json::json!({"paths": []}), ["imre"]);
        let mut payload = vec![EMPTY_LIST, MEMOIZE, MARK, BINUNICODE];
        let text = ".(é".as_bytes();
        payload.extend_from_slice(&(text.len() as u32).to_le_bytes());
        payload.extend_from_slice(text);
        payload.extend_from_slice(&[MEMOIZE, BINBYTES]);
        let bytes = [STOP, FRAME, BINPUT, MARK];
        payload.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
        payload.extend_from_slice(&bytes);
        payload.extend_from_slice(&[BINGET, 1, APPENDS, STOP]);
        let mut pickle = vec![PROTO, 5, FRAME];
        pickle.extend_from_slice(&(payload.len() as u64).to_le_bytes());
        pickle.extend_from_slice(&payload);
        pickle.push(0xff); // outside the completed frame and ignored after STOP

        let input = ctx.alloc(Value::Bytes(pickle.clone()));
        let output = decode(&mut ctx, input, preflight(&pickle).unwrap()).unwrap();
        let Value::List(items) = ctx.heap.get(output) else {
            panic!("list")
        };
        assert_eq!(items.len(), 3);
        assert_eq!(items[0], items[2]);
        assert!(matches!(ctx.heap.get(items[0]), Value::Str(value) if value == ".(é"));
        assert!(matches!(ctx.heap.get(items[1]), Value::Bytes(value) if value == &bytes));

        let mut truncated = pickle.clone();
        truncated.truncate(11 + payload.len() - 1);
        assert!(preflight(&truncated).is_none());
        pickle[3..11].copy_from_slice(&((payload.len() + 1) as u64).to_le_bytes());
        assert!(
            preflight(&pickle).is_none(),
            "STOP must end the declared frame"
        );
    }
}
