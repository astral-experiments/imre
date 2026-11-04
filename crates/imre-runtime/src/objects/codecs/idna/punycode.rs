//! RFC 3492 Bootstring with the strict decoding errors of CPython's Punycode codec.

use super::LabelError;

const DIGITS: &[u8; 36] = b"abcdefghijklmnopqrstuvwxyz0123456789";
const BASE: usize = DIGITS.len();
const INITIAL_BIAS: usize = 72;
const INITIAL_CODE_POINT: usize = 128;

fn threshold(k: usize, bias: usize) -> usize {
    k.saturating_sub(bias).clamp(1, 26)
}

fn adapt(delta: usize, first: bool, points: usize) -> usize {
    let mut delta = delta / if first { 700 } else { 2 };
    delta += delta / points;
    let mut bias = 0;
    while delta > 455 {
        delta /= 35;
        bias += BASE;
    }
    bias + BASE * delta / (delta + 38)
}

fn encode_delta(mut delta: usize, bias: usize, out: &mut Vec<u8>) {
    let mut k = BASE;
    loop {
        let t = threshold(k, bias);
        if delta < t {
            out.push(DIGITS[delta]);
            return;
        }
        out.push(DIGITS[t + (delta - t) % (BASE - t)]);
        delta = (delta - t) / (BASE - t);
        k += BASE;
    }
}

pub(super) fn encode(label: &str) -> Vec<u8> {
    let mut out: Vec<_> = label.bytes().filter(u8::is_ascii).collect();
    let basic = out.len();
    if basic > 0 {
        out.push(b'-');
    }
    let count = label.chars().count();
    let mut handled = basic;
    let mut n = INITIAL_CODE_POINT;
    let mut bias = INITIAL_BIAS;
    let mut delta = 0;
    while handled < count {
        let next = label
            .chars()
            .map(|c| c as usize)
            .filter(|&c| c >= n)
            .min()
            .unwrap();
        delta += (next - n) * (handled + 1);
        n = next;
        for c in label.chars().map(|c| c as usize) {
            if c < n {
                delta += 1;
            } else if c == n {
                encode_delta(delta, bias, &mut out);
                bias = adapt(delta, handled == basic, handled + 1);
                handled += 1;
                delta = 0;
            }
        }
        delta += 1;
        n += 1;
    }
    out
}

fn decode_delta(data: &[u8], cursor: &mut usize, bias: usize) -> Result<usize, LabelError> {
    let mut value = 0usize;
    let mut weight = 1usize;
    let mut k = BASE;
    loop {
        let Some(&byte) = data.get(*cursor) else {
            return Err(LabelError::new(
                *cursor,
                *cursor + 1,
                "incomplete punycode string",
            ));
        };
        let digit = match byte.to_ascii_uppercase() {
            upper @ b'A'..=b'Z' => upper - b'A',
            n @ b'0'..=b'9' => n - b'0' + 26,
            _ => {
                return Err(LabelError::new(
                    *cursor,
                    *cursor + 1,
                    format!("Invalid extended code point '{byte}'"),
                ));
            }
        } as usize;
        *cursor += 1;
        // Consume the entire integer even after overflow. A bad digit or a
        // truncated tail takes precedence over an unrepresentable position.
        value = value.saturating_add(weight.saturating_mul(digit));
        let t = threshold(k, bias);
        if digit < t {
            return Ok(value);
        }
        weight = weight.saturating_mul(BASE - t);
        k += BASE;
    }
}

pub(super) fn decode(data: &[u8]) -> Result<Vec<u32>, LabelError> {
    let mut out = Vec::new();
    let mut cursor = 0;
    if let Some(delimiter) = data.iter().rposition(|&b| b == b'-') {
        for (i, &b) in data[..delimiter].iter().enumerate() {
            if !b.is_ascii() {
                return Err(LabelError::new(i, i + 1, "ordinal not in range(128)"));
            }
            out.push(u32::from(b));
        }
        cursor = delimiter + 1;
    }
    let offset = cursor;
    let mut n = INITIAL_CODE_POINT;
    let mut bias = INITIAL_BIAS;
    let mut index = 0usize;
    while cursor < data.len() {
        let first = cursor == offset;
        let delta = decode_delta(data, &mut cursor, bias)?;
        index = index.saturating_add(delta);
        let count = out.len() + 1;
        n = n.saturating_add(index / count);
        if n > 0x10ffff {
            // CPython reports the insertion position, which can exceed both
            // the input length and the platform's signed exception index.
            let end = offset.saturating_add(index);
            if end > isize::MAX as usize {
                return Err(LabelError::overflow());
            }
            return Err(LabelError::new(
                end - 1,
                end,
                format!("Invalid character U+{n:x}"),
            ));
        }
        index %= count;
        out.insert(index, n as u32);
        index += 1;
        bias = adapt(delta, first, count);
    }
    Ok(out)
}
