//! Independent Base45 codec with the same file/checksum protocol as the Python driver.

use std::hint::black_box;
use std::io::{self, Write};

const ALPHABET: &[u8; 45] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ $%*+-./:";
const INVALID: u8 = u8::MAX;
const DECODE: [u8; 256] = {
    let mut table = [INVALID; 256];
    let mut index = 0;
    while index < ALPHABET.len() {
        table[ALPHABET[index] as usize] = index as u8;
        index += 1;
    }
    table
};

struct Error {
    kind: &'static str,
    message: String,
    cause: Option<Box<Error>>,
}

impl Error {
    fn new(kind: &'static str, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            cause: None,
        }
    }

    /// The upstream decoder wraps alphabet, range, and byte-decoding failures.
    fn wrapped(self) -> Self {
        Self {
            kind: "ValueError",
            message: "Invalid base45 string".into(),
            cause: Some(Box::new(self)),
        }
    }

    fn write(&self, output: &mut impl Write) -> io::Result<()> {
        writeln!(output, "ERROR\n{}\n{}", self.kind, self.message)?;
        if let Some(cause) = &self.cause {
            writeln!(output, "True\n{}\n{}", cause.kind, cause.message)
        } else {
            writeln!(output, "False\nNone\n")
        }
    }
}

/// Standard UTF-8 error bounds also identify Python's failing byte sequence.
fn utf8(bytes: &[u8]) -> Result<&str, Error> {
    std::str::from_utf8(bytes).map_err(|error| {
        let start = error.valid_up_to();
        let end = error
            .error_len()
            .map_or(bytes.len(), |length| start + length);
        let reason = if error.error_len().is_none() {
            "unexpected end of data"
        } else if !(0xc2..=0xf4).contains(&bytes[start]) {
            "invalid start byte"
        } else {
            "invalid continuation byte"
        };
        let location = if end == start + 1 {
            format!("byte 0x{:02x} in position {start}", bytes[start])
        } else {
            format!("bytes in position {start}-{}", end - 1)
        };
        Error::new(
            "UnicodeDecodeError",
            format!("'utf-8' codec can't decode {location}: {reason}"),
        )
    })
}

/// Diagnostic adapter for one invalid alphabet character. This uses Rust's
/// Unicode printability, not a Python repr table; validation records that boundary.
fn diagnostic_key_message(character: char) -> String {
    let quote = if character == '\'' { '"' } else { '\'' };
    let escaped = match character {
        '\n' => "\\n".into(),
        '\r' => "\\r".into(),
        '\t' => "\\t".into(),
        '\\' => "\\\\".into(),
        value if value.is_control() || value.escape_debug().to_string().starts_with("\\u{") => {
            let codepoint = value as u32;
            if codepoint <= 0xff {
                format!("\\x{codepoint:02x}")
            } else if codepoint <= 0xffff {
                format!("\\u{codepoint:04x}")
            } else {
                format!("\\U{codepoint:08x}")
            }
        }
        value => value.to_string(),
    };
    format!("{quote}{escaped}{quote}")
}

fn encode(input: &[u8]) -> Vec<u8> {
    let capacity = input.len() / 2 * 3 + input.len() % 2 * 2;
    let mut output = Vec::with_capacity(capacity);
    let (pairs, remainder) = input.as_chunks::<2>();
    for pair in pairs {
        let value = u16::from_be_bytes([pair[0], pair[1]]) as usize;
        output.push(ALPHABET[value % 45]);
        output.push(ALPHABET[value / 45 % 45]);
        output.push(ALPHABET[value / (45 * 45)]);
    }
    if let [last] = remainder {
        output.push(ALPHABET[*last as usize % 45]);
        output.push(ALPHABET[*last as usize / 45]);
    }
    output
}

/// Deferring range failures lets a later invalid character win, as in the eager
/// upstream comprehension, while lookup and byte production share a single pass.
fn decode(text: &str) -> Result<Vec<u8>, Error> {
    let input = text.as_bytes();
    let mut output = Vec::with_capacity(input.len() / 3 * 2 + input.len() % 3 / 2);
    let (triples, remainder) = input.as_chunks::<3>();
    let mut range_error = false;
    for (offset, triple) in triples.iter().enumerate() {
        let digits = triple.map(|byte| DECODE[byte as usize]);
        if let Some(index) = digits.iter().position(|&digit| digit == INVALID) {
            let character = text[offset * 3 + index..].chars().next().unwrap();
            return Err(Error::new("KeyError", diagnostic_key_message(character)));
        }
        let value = digits[0] as u32 + digits[1] as u32 * 45 + digits[2] as u32 * 45 * 45;
        range_error |= value > u16::MAX as u32;
        output.extend_from_slice(&(value as u16).to_be_bytes());
    }
    for (index, byte) in remainder.iter().enumerate() {
        if DECODE[*byte as usize] == INVALID {
            let character = text[triples.len() * 3 + index..].chars().next().unwrap();
            return Err(Error::new("KeyError", diagnostic_key_message(character)));
        }
    }
    if remainder.len() == 1 {
        return Err(Error::new("ValueError", "Invalid base45 string"));
    }
    if let [first, second] = remainder {
        let value = DECODE[*first as usize] as u16 + DECODE[*second as usize] as u16 * 45;
        range_error |= value > u8::MAX as u16;
        output.push(value as u8);
    }
    if range_error {
        Err(Error::new("ValueError", ""))
    } else {
        Ok(output)
    }
}

/// Each iteration performs a fresh codec call and contributes its result to the
/// checksum. Black boxes prevent LLVM from collapsing identical repeated work.
fn repeated(
    repetitions: usize,
    mut operation: impl FnMut() -> Result<Vec<u8>, Error>,
) -> Result<(Vec<u8>, u128), Error> {
    let mut result = Vec::new();
    let mut checksum = 0_u128;
    for _ in 0..repetitions {
        result = black_box(operation()?);
        checksum += result.len() as u128;
        if let (Some(first), Some(last)) = (result.first(), result.last()) {
            checksum += *first as u128 + *last as u128;
        }
    }
    Ok((result, checksum))
}

fn transform(
    operation: &str,
    payload: &[u8],
    repetitions: usize,
) -> Result<(Vec<u8>, u128), Error> {
    match operation {
        "encode" => repeated(repetitions, || Ok(encode(black_box(payload)))),
        "decode-bytes" => repeated(repetitions, || {
            utf8(black_box(payload))
                .and_then(decode)
                .map_err(Error::wrapped)
        }),
        "decode-text" => {
            let text = utf8(payload)?;
            repeated(repetitions, || {
                decode(black_box(text).trim_end_matches('\n')).map_err(Error::wrapped)
            })
        }
        _ => Err(Error::new("ValueError", "unknown codec operation")),
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let arguments: Vec<_> = std::env::args().collect();
    let [_, operation, source, destination, repetitions] = arguments.as_slice() else {
        return Err("expected operation, input path, output path, and repetitions".into());
    };
    let repetitions: usize = repetitions.parse()?;
    if repetitions == 0 {
        return Err("repetitions must be positive".into());
    }
    let payload = std::fs::read(source)?;
    let mut stdout = io::stdout().lock();
    match transform(operation, &payload, repetitions) {
        Ok((result, checksum)) => {
            std::fs::write(destination, result)?;
            writeln!(stdout, "OK\n{checksum}")?;
        }
        Err(error) => error.write(&mut stdout)?,
    }
    Ok(())
}
