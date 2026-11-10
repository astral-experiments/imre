//! Validate the constant keys before compiling a mapping pattern's subpatterns.
use std::collections::HashSet;
use std::fmt::Write;

use num_bigint::BigInt;
use num_traits::ToPrimitive;
use ruff_python_ast::{self as ast, str::TripleQuotes};
use ruff_python_literal::escape::{AsciiEscape, UnicodeEscape};
use ruff_text_size::Ranged;

use super::{BindingError, PatternMessage};
use crate::numbers::{complex_repr, float_repr, parse_integer_literal};

pub(super) fn check(mapping: &ast::PatternMatchMapping, source: &str) -> Result<(), BindingError> {
    let mut seen = HashSet::new();
    for expr in &mapping.keys {
        // Dotted keys are compared by the mapping operation when the case runs.
        if expr.is_attribute_expr() {
            continue;
        }
        let literal = Literal::from_expr(expr, source).ok_or_else(|| {
            (
                mapping.range(),
                "mapping pattern keys may only match literals and attribute lookups"
                    .to_owned()
                    .into(),
            )
        })?;
        if !seen.insert(literal.key()) {
            return Err((mapping.range(), literal.duplicate()));
        }
    }
    Ok(())
}

/// The real component must be convertible when Python folds a complex pattern.
pub(super) fn real_part(expr: &ast::Expr, source: &str) -> Option<f64> {
    match Literal::from_expr(expr, source)? {
        Literal::Int(i) => i.to_f64().filter(|f| f.is_finite()),
        Literal::Float(f) => Some(f),
        _ => None,
    }
}

enum Literal<'a> {
    None,
    Bool(bool),
    Int(BigInt),
    Float(f64),
    Complex(f64, f64),
    Str(&'a str, Vec<u32>),
    Bytes(Vec<u8>),
}

// An integral real component with zero imaginary part has its exact integer
// value as key, even beyond the precision of binary64 or a machine integer.
#[derive(Hash, PartialEq, Eq)]
enum Key<'a> {
    None,
    Int(BigInt),
    Complex(u64, u64),
    Str(&'a str, Vec<u32>),
    Bytes(Vec<u8>),
}

impl<'a> Literal<'a> {
    fn from_expr(expr: &'a ast::Expr, source: &str) -> Option<Self> {
        match expr {
            ast::Expr::NoneLiteral(_) => Some(Self::None),
            ast::Expr::BooleanLiteral(b) => Some(Self::Bool(b.value)),
            ast::Expr::StringLiteral(s) => {
                let text = s.value.to_str();
                let mut points = Vec::new();
                if text.contains('\u{fffd}') {
                    for part in &s.value {
                        let mut replacements =
                            crate::strings::replacements(&source[part.content_range()]);
                        for _ in part.value.matches('\u{fffd}') {
                            points.push(if part.flags.prefix().is_raw() {
                                0xfffd
                            } else {
                                replacements.next().unwrap_or(0xfffd)
                            });
                        }
                    }
                }
                Some(Self::Str(text, points))
            }
            ast::Expr::BytesLiteral(b) => Some(Self::Bytes(b.value.bytes().collect())),
            ast::Expr::NumberLiteral(n) => Some(match &n.value {
                ast::Number::Int(i) => Self::Int(if let Some(i) = i.as_u64() {
                    BigInt::from(i)
                } else {
                    parse_integer_literal(&i.to_string())?
                }),
                ast::Number::Float(f) => Self::Float(*f),
                ast::Number::Complex { real, imag } => Self::Complex(*real, *imag),
            }),
            ast::Expr::UnaryOp(u) if u.op == ast::UnaryOp::USub => {
                Some(match Self::from_expr(&u.operand, source)? {
                    Self::Int(i) => Self::Int(-i),
                    Self::Float(f) => Self::Float(-f),
                    Self::Complex(r, i) => Self::Complex(-r, -i),
                    _ => return None,
                })
            }
            ast::Expr::BinOp(b) => {
                // The only binary form in the grammar is a signed real +/- an imaginary literal.
                let real = real_part(&b.left, source)?;
                let Self::Complex(r, i) = Self::from_expr(&b.right, source)? else {
                    return None;
                };
                match b.op {
                    ast::Operator::Add => Some(Self::Complex(real + r, i)),
                    ast::Operator::Sub => Some(Self::Complex(real - r, -i)),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    fn key(&self) -> Key<'a> {
        let (real, imag) = match self {
            Self::None => return Key::None,
            Self::Str(s, points) => return Key::Str(s, points.clone()),
            Self::Bytes(b) => return Key::Bytes(b.clone()),
            Self::Bool(b) => return Key::Int(BigInt::from(*b)),
            Self::Int(i) => return Key::Int(i.clone()),
            Self::Float(f) => (*f, 0.0),
            Self::Complex(r, i) => (*r, *i),
        };
        if imag == 0.0
            && let Some(value) = crate::numbers::integral_float(real)
        {
            return Key::Int(value);
        }
        let bits = |f: f64| if f == 0.0 { 0 } else { f.to_bits() };
        Key::Complex(bits(real), bits(imag))
    }

    fn duplicate(self) -> PatternMessage {
        let repr = match self {
            Self::None => "None".to_owned(),
            Self::Bool(b) => (if b { "True" } else { "False" }).to_owned(),
            Self::Int(i) => return PatternMessage::DuplicateInteger(i),
            Self::Float(f) => float_repr(f).to_string(),
            Self::Complex(r, i) => complex_repr(r, i),
            Self::Bytes(b) => format!("{}", AsciiEscape::new_repr(&b).bytes_repr(TripleQuotes::No)),
            Self::Str(text, points) => string_repr(text, &points),
        };
        super::duplicate_key(repr).into()
    }
}

fn string_repr(text: &str, points: &[u32]) -> String {
    let rendered = format!(
        "{}",
        UnicodeEscape::new_repr(text).str_repr(TripleQuotes::No)
    );
    if points.is_empty() {
        return rendered;
    }
    let mut points = points.iter();
    let mut result = String::new();
    for ch in rendered.chars() {
        if ch == '\u{fffd}'
            && let Some(&point) = points.next()
            && point != 0xfffd
        {
            write!(result, "\\u{point:04x}").unwrap();
        } else {
            result.push(ch);
        }
    }
    result
}
