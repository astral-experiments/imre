//! Independent streaming Luhn implementation with the Python driver's protocol.

mod unicode;

use std::hint::black_box;
use std::io::{self, Write};

struct Error {
    kind: &'static str,
    message: String,
}

impl Error {
    fn write(&self, output: &mut impl Write) -> io::Result<()> {
        writeln!(
            output,
            "ERROR\n{}\n{}\nFalse\nNone\n",
            self.kind, self.message
        )
    }
}

/// UTF-8 failures precede digit conversion, including a later invalid byte.
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
        Error {
            kind: "UnicodeDecodeError",
            message: format!("'utf-8' codec can't decode {location}: {reason}"),
        }
    })
}

/// Python repr for one scalar character using the pinned Unicode16 printability
/// boundaries; combining marks and variation selectors remain printable.
fn quoted(character: char) -> String {
    let quote = if character == '\'' { '"' } else { '\'' };
    let escaped = match character {
        '\n' => "\\n".into(),
        '\r' => "\\r".into(),
        '\t' => "\\t".into(),
        '\\' => "\\\\".into(),
        value
            if unicode::NONPRINTABLE_EDGES.partition_point(|&edge| edge <= value as u32) % 2
                == 1 =>
        {
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

fn invalid(character: char) -> Error {
    Error {
        kind: "ValueError",
        message: format!(
            "invalid literal for int() with base 10: {}",
            quoted(character)
        ),
    }
}

fn digit(character: char) -> Result<u32, Error> {
    let value = character as u32;
    let index = unicode::ZEROES.partition_point(|&zero| zero <= value);
    if index > 0 {
        let value = value - unicode::ZEROES[index - 1];
        if value < 10 {
            return Ok(value);
        }
    }
    Err(invalid(character))
}

/// Validate left to right, matching eager map(int, text), while assigning Luhn
/// weights from the right. Generating a check digit adds a virtual final zero.
fn checksum(text: &str, extra_zero: bool) -> Result<u32, Error> {
    if text.is_ascii() {
        let parity = (text.len() % 2) ^ usize::from(extra_zero);
        let mut total = 0;
        // Bounded partial sums avoid integer overflow for arbitrary input sizes.
        // Even-sized chunks preserve the original index parity.
        for chunk in text.as_bytes().chunks(4096) {
            let mut partial = 0_u32;
            for (index, &byte) in chunk.iter().enumerate() {
                if !byte.is_ascii_digit() {
                    return Err(invalid(byte as char));
                }
                let digit = u32::from(byte - b'0');
                let doubled = 2 * digit;
                partial += if index % 2 == parity {
                    doubled - 9 * u32::from(doubled >= 10)
                } else {
                    digit
                };
            }
            total = (total + partial) % 10;
        }
        return Ok(total);
    }
    let parity = (text.chars().count() % 2) ^ usize::from(extra_zero);
    let mut total = 0;
    for (index, character) in text.chars().enumerate() {
        let digit = digit(character)?;
        let doubled = 2 * digit;
        total += if index % 2 == parity {
            doubled - 9 * u32::from(doubled >= 10)
        } else {
            digit
        };
        total %= 10;
    }
    Ok(total)
}

/// Inputs and completed results cross black boxes on every iteration so LLVM
/// cannot replace repeated calls with one call and checksum multiplication.
fn transform(operation: &str, payload: &[u8], repetitions: u64) -> Result<(String, u128), Error> {
    let text = utf8(payload)?;
    let mut total = 0_u128;
    let mut number = 0;
    let mut valid = false;
    let mut result = String::new();
    match operation {
        "checksum" => {
            for _ in 0..repetitions {
                number = black_box(checksum(black_box(text), false)?);
                total += u128::from(number);
            }
            result = number.to_string();
        }
        "verify" => {
            for _ in 0..repetitions {
                valid = black_box(checksum(black_box(text), false)? == 0);
                total += u128::from(valid);
            }
            result = if valid { "True" } else { "False" }.into();
        }
        "generate" => {
            for _ in 0..repetitions {
                number = black_box((10 - checksum(black_box(text), true)?) % 10);
                total += u128::from(number);
            }
            result = number.to_string();
        }
        "append" => {
            for _ in 0..repetitions {
                let input = black_box(text);
                let number = (10 - checksum(input, true)?) % 10;
                let mut output = String::with_capacity(input.len() + 1);
                output.push_str(input);
                output.push(char::from(b'0' + number as u8));
                result = black_box(output);
                total += result.chars().count() as u128;
            }
        }
        _ => {
            return Err(Error {
                kind: "ValueError",
                message: "unknown Luhn operation".into(),
            });
        }
    }
    Ok((result, total))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let arguments: Vec<_> = std::env::args().collect();
    let [_, operation, source, destination, repetitions] = arguments.as_slice() else {
        return Err("expected operation, input path, output path, and repetitions".into());
    };
    let repetitions: u64 = repetitions.parse()?;
    if repetitions == 0 {
        return Err("repetitions must be positive".into());
    }
    let payload = std::fs::read(source)?;
    let mut stdout = io::stdout().lock();
    match transform(operation, &payload, repetitions) {
        Ok((result, total)) => {
            std::fs::write(destination, result)?;
            writeln!(stdout, "OK\n{total}")?;
        }
        Err(error) => error.write(&mut stdout)?,
    }
    Ok(())
}
