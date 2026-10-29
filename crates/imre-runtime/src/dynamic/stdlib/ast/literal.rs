//! Convert the data-only Python AST accepted by ast.literal_eval.
use super::{Context, Error, Id, NODES, Result, Value, builtins, parse};
use crate::dynamic::complex::Complex;
use ruff_python_ast::token::TokenKind;
use ruff_python_parser::{Mode, ParseError, ParseErrorType};

fn source_error(kind: &str, detail: &str, line: i64) -> Error {
    Error::new(kind, format!("{detail} (<unknown>, line {line})"))
}

pub(super) fn syntax_error(error: &ParseError, source: &str, line: i64) -> Error {
    if matches!(error.error, ParseErrorType::UnexpectedIndentation) {
        return source_error("IndentationError", "unexpected indent", line);
    }
    // The native parser reports an empty expression at byte zero.
    source_error(
        "SyntaxError",
        "invalid syntax",
        if source.is_empty() { 0 } else { line },
    )
}

// Enforce parser limits on Ruff tokens, without treating punctuation in strings or
// comments as syntax. Checking before parsing also protects the native parser stack.
pub(super) fn check_source(source: &str, line_at: impl Fn(usize) -> i64) -> Result<()> {
    use TokenKind::*;
    if source.contains('\0') {
        return Err(Error::new(
            "SyntaxError",
            "source code string cannot contain null bytes",
        ));
    }
    let mut lexer = ruff_python_parser::lexer::lex(source, Mode::Expression);
    let mut delimiters = Vec::new();
    let mut operators = 0;
    loop {
        let kind = lexer.next_token();
        let offset = lexer.current_range().start().to_usize();
        match kind {
            EndOfFile => break,
            Indent => {
                return Err(source_error(
                    "IndentationError",
                    "unexpected indent",
                    line_at(offset),
                ));
            }
            Lpar | Lsqb | Lbrace => {
                if delimiters.len() >= 200 {
                    return Err(source_error(
                        "SyntaxError",
                        "too many nested parentheses",
                        line_at(offset),
                    ));
                }
                delimiters.push(offset);
            }
            Rpar | Rsqb | Rbrace => {
                delimiters.pop();
            }
            Comma => operators = 0,
            Plus | Minus | Tilde | Not | Lambda | If | DoubleStar => {
                operators += 1;
                if operators > 512 {
                    return Err(Error::new(
                        "RecursionError",
                        "maximum recursion depth exceeded during ast construction",
                    ));
                }
            }
            Int => {
                let text = &source[lexer.current_range()];
                if !matches!(
                    text.as_bytes().get(1),
                    Some(b'x' | b'X' | b'o' | b'O' | b'b' | b'B')
                ) && let Err(error) = crate::integer_text::check_input_digits(
                    text,
                    crate::integer_text::decimal_digit_limit(),
                ) {
                    let detail = Error::from(error).message;
                    return Err(source_error(
                        "SyntaxError",
                        &format!(
                            "{detail} - Consider hexadecimal for huge integer literals to avoid decimal conversion limits."
                        ),
                        line_at(offset),
                    ));
                }
            }
            _ => {}
        }
    }
    if let Some(offset) = delimiters.last() {
        let bracket = source.as_bytes()[*offset] as char;
        return Err(source_error(
            "SyntaxError",
            &format!("'{bracket}' was never closed"),
            line_at(*offset),
        ));
    }
    Ok(())
}

fn is(ctx: &Context, node: Id, class: &str) -> bool {
    ctx.instance_of(node, ctx.builtins[class])
}

// CPython's AST repr prints three levels of fields and the first and last list items.
fn node_repr(ctx: &mut Context, node: Id, depth: usize, parents: &mut Vec<Id>) -> Result<String> {
    if !is(ctx, node, "ast.AST") {
        return ctx.checked_format(node, true);
    }
    let name = ctx.type_name(node).to_owned();
    if depth == 0 || parents.contains(&node) {
        return Ok(format!("{name}(...)"));
    }
    parents.push(node);
    let spec = NODES
        .iter()
        .rev()
        .find(|s| is(ctx, node, &format!("ast.{}", s.name)))
        .unwrap();
    let mut fields = Vec::new();
    for &(field, _) in spec.fields {
        let value = match ctx.lookup_attr(node, field) {
            Ok(value) => value,
            Err(e) if e.kind == "AttributeError" => continue,
            Err(e) => return Err(e),
        };
        let repr = match ctx.heap.get(value) {
            Value::List(items) | Value::Tuple(items) => {
                let (open, close) = if matches!(ctx.heap.get(value), Value::List(_)) {
                    ("[", "]")
                } else {
                    ("(", ")")
                };
                let first = items.first().copied();
                let last = (items.len() > 1).then(|| *items.last().unwrap());
                let omitted = items.len() > 2;
                let mut parts = Vec::new();
                if let Some(first) = first {
                    parts.push(node_repr(ctx, first, depth - 1, parents)?);
                }
                if omitted {
                    parts.push("...".into());
                }
                if let Some(last) = last {
                    parts.push(node_repr(ctx, last, depth - 1, parents)?);
                }
                format!("{open}{}{close}", parts.join(", "))
            }
            _ => node_repr(ctx, value, depth - 1, parents)?,
        };
        fields.push(format!("{field}={repr}"));
    }
    parents.pop();
    Ok(format!("{name}({})", fields.join(", ")))
}

fn malformed(ctx: &mut Context, node: Id) -> Result<Id> {
    let line = match ctx.lookup_attr(node, "lineno") {
        Ok(line) if ctx.truth(line)? => format!(" on line {}", ctx.display(line)),
        Ok(_) => String::new(),
        Err(e) if e.kind == "AttributeError" => String::new(),
        Err(e) => return Err(e),
    };
    let repr = node_repr(ctx, node, 3, &mut Vec::new())?;
    Err(Error::new(
        "ValueError",
        format!("malformed node or string{line}: {repr}"),
    ))
}

fn number(ctx: &mut Context, node: Id) -> Result<Id> {
    if is(ctx, node, "ast.Constant") {
        let value = ctx.lookup_attr(node, "value")?;
        if matches!(
            ctx.heap.get(value),
            Value::Int(_) | Value::Float(_) | Value::Complex(_)
        ) {
            return Ok(value);
        }
    }
    malformed(ctx, node)
}

fn signed_number(ctx: &mut Context, node: Id) -> Result<Id> {
    if is(ctx, node, "ast.UnaryOp") {
        let op = ctx.lookup_attr(node, "op")?;
        if is(ctx, op, "ast.UAdd") || is(ctx, op, "ast.USub") {
            let operand = ctx.lookup_attr(node, "operand")?;
            let value = number(ctx, operand)?;
            if is(ctx, op, "ast.UAdd") {
                return Ok(value);
            }
            let negated = match ctx.heap.get(value) {
                Value::Int(x) => Value::Int(crate::integer_binary(&crate::Int::from(0), x, 1)?),
                Value::Float(x) => Value::Float(-x),
                Value::Complex(z) => Value::Complex(Complex {
                    real: -z.real,
                    imag: -z.imag,
                }),
                _ => unreachable!(),
            };
            return Ok(ctx.alloc(negated));
        }
    }
    number(ctx, node)
}

fn convert(ctx: &mut Context, node: Id, depth: usize) -> Result<Id> {
    // Hand-built nodes can be cyclic or deeper than a parsed literal.
    if depth >= 256 {
        return Err(Error::new(
            "RecursionError",
            "maximum recursion depth exceeded",
        ));
    }
    if is(ctx, node, "ast.Constant") {
        return ctx.lookup_attr(node, "value");
    }
    if is(ctx, node, "ast.Tuple") || is(ctx, node, "ast.List") || is(ctx, node, "ast.Set") {
        let elts = ctx.lookup_attr(node, "elts")?;
        let iter = ctx.iterator(elts)?;
        if is(ctx, node, "ast.Set") {
            let set = builtins::call(ctx, "set", &[], &[])?;
            while let Some(elt) = ctx.next_item(iter)? {
                let value = convert(ctx, elt, depth + 1)?;
                builtins::set_add(ctx, set, value)?;
            }
            return Ok(set);
        }
        ctx.heap.keep(iter);
        let result = ctx.alloc(if is(ctx, node, "ast.Tuple") {
            Value::Tuple(Vec::new())
        } else {
            Value::List(Vec::new())
        });
        let mark = ctx.heap.temporary_mark();
        while let Some(elt) = ctx.next_item(iter)? {
            let value = convert(ctx, elt, depth + 1)?;
            let (Value::List(values) | Value::Tuple(values)) = ctx.heap.get_mut(result) else {
                unreachable!();
            };
            values
                .try_reserve(1)
                .map_err(|_| Error::new("MemoryError", ""))?;
            values.push(value);
            ctx.heap.release_temporaries_since(mark);
        }
        return Ok(result);
    }
    if is(ctx, node, "ast.Dict") {
        ctx.heap.keep(node);
        let keys = ctx.lookup_attr(node, "keys")?;
        let key_count = builtins::call(ctx, "len", &[keys], &[])?;
        let values = ctx.lookup_attr(node, "values")?;
        let value_count = builtins::call(ctx, "len", &[values], &[])?;
        if !ctx.equal(key_count, value_count) {
            return malformed(ctx, node);
        }
        // CPython fetches the fields again for zip(map(...), map(...)).
        let keys = ctx.lookup_attr(node, "keys")?;
        let keys = ctx.iterator(keys)?;
        ctx.heap.keep(keys);
        let values = ctx.lookup_attr(node, "values")?;
        let values = ctx.iterator(values)?;
        ctx.heap.keep(values);
        let dict = ctx.dict(Vec::new());
        let mark = ctx.heap.temporary_mark();
        while let Some(key) = ctx.next_item(keys)? {
            let key = convert(ctx, key, depth + 1)?;
            ctx.heap.keep(key);
            let Some(value) = ctx.next_item(values)? else {
                break;
            };
            let value = convert(ctx, value, depth + 1)?;
            ctx.heap.keep(value);
            ctx.set_dictionary_key(dict, key, value, None, true)?;
            ctx.heap.release_temporaries_since(mark);
        }
        return Ok(dict);
    }
    if is(ctx, node, "ast.Call") {
        let func = ctx.lookup_attr(node, "func")?;
        if is(ctx, func, "ast.Name") {
            let id = ctx.lookup_attr(func, "id")?;
            if matches!(ctx.heap.get(id), Value::Str(s) if s == "set") {
                let args = ctx.lookup_attr(node, "args")?;
                let keywords = ctx.lookup_attr(node, "keywords")?;
                if [args, keywords]
                    .into_iter()
                    .all(|id| matches!(ctx.heap.get(id), Value::List(v) if v.is_empty()))
                {
                    return builtins::call(ctx, "set", &[], &[]);
                }
            }
        }
    }
    if is(ctx, node, "ast.BinOp") {
        let op = ctx.lookup_attr(node, "op")?;
        if is(ctx, op, "ast.Add") || is(ctx, op, "ast.Sub") {
            let left = ctx.lookup_attr(node, "left")?;
            let left = signed_number(ctx, left)?;
            let right = ctx.lookup_attr(node, "right")?;
            let right = number(ctx, right)?;
            if matches!(ctx.heap.get(left), Value::Int(_) | Value::Float(_))
                && matches!(ctx.heap.get(right), Value::Complex(_))
            {
                return ctx.complex_sum(is(ctx, op, "ast.Add"), left, right);
            }
        }
    }
    signed_number(ctx, node)
}

pub(super) fn evaluate(ctx: &mut Context, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
    let [node] = builtins::bind("literal_eval", args, kwargs, ["node_or_string"])?;
    let mut node = node.ok_or_else(|| {
        Error::new(
            "TypeError",
            "literal_eval() missing 1 required positional argument: 'node_or_string'",
        )
    })?;
    if let Value::Str(s) = ctx.heap.get(node) {
        let source = s.trim_start_matches([' ', '\t']).to_owned();
        node = parse::parse_source(ctx, &source, "<unknown>", "eval", true)?;
    }
    if is(ctx, node, "ast.Expression") {
        node = ctx.lookup_attr(node, "body")?;
    }
    convert(ctx, node, 0)
}
