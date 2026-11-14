//! Recover string spellings and code points that the decoded AST does not retain.
use compact_str::CompactString;
use ruff_python_ast::{
    self as ast, StringFlags,
    token::{Token, TokenKind, Tokens},
};
use ruff_text_size::{Ranged, TextRange, TextSize};
use std::collections::HashMap;

/// String methods exposed by the boxed runtime's native method descriptors.
pub const STR_METHODS: &[&str] = &[
    "__add__",
    "__contains__",
    "__eq__",
    "__getitem__",
    "__hash__",
    "__str__",
    "__format__",
    "__mod__",
    "__ne__",
    "__repr__",
    "__rmod__",
    "capitalize",
    "casefold",
    "center",
    "count",
    "encode",
    "endswith",
    "expandtabs",
    "find",
    "format",
    "format_map",
    "index",
    "isalnum",
    "isascii",
    "isalpha",
    "isdigit",
    "isidentifier",
    "islower",
    "isnumeric",
    "isprintable",
    "isspace",
    "istitle",
    "isupper",
    "join",
    "ljust",
    "lower",
    "lstrip",
    "partition",
    "removeprefix",
    "removesuffix",
    "replace",
    "rfind",
    "rindex",
    "rjust",
    "rpartition",
    "rsplit",
    "rstrip",
    "split",
    "splitlines",
    "startswith",
    "strip",
    "swapcase",
    "title",
    "translate",
    "upper",
    "zfill",
];

/// Raw and byte literals have no cooked Unicode escapes.
pub fn cooked_content(token: &Token) -> Option<TextRange> {
    if !matches!(
        token.kind(),
        TokenKind::String | TokenKind::FStringMiddle | TokenKind::TStringMiddle
    ) {
        return None;
    }
    let flags = token.unwrap_string_flags();
    if flags.is_raw_string() || flags.is_byte_string() {
        return None;
    }
    Some(if token.kind() == TokenKind::String {
        TextRange::new(
            token.start() + flags.opener_len(),
            token.end() - flags.closer_len(),
        )
    } else {
        token.range()
    })
}

/// Original code points rendered as U+FFFD in Ruff's decoded, non-raw string.
/// Non-Unicode escapes (including escaped backslashes) cannot produce a
/// surrogate or U+FFFD. Unknown escapes preserve the escaped character.
pub fn replacements(source: &str) -> impl Iterator<Item = u32> + '_ {
    let mut chars = source.chars();
    std::iter::from_fn(move || {
        loop {
            let mut point = u32::from(chars.next()?);
            if point == u32::from('\\') {
                point = match chars.next()? {
                    introducer @ ('u' | 'U') => {
                        let width = if introducer == 'u' { 4 } else { 8 };
                        let rest = chars.as_str();
                        let hex = rest.get(..width)?;
                        chars = rest[width..].chars();
                        u32::from_str_radix(hex, 16).ok()?
                    }
                    'N' => {
                        let (name, rest) = chars.as_str().strip_prefix('{')?.split_once('}')?;
                        chars = rest.chars();
                        u32::from(unicode_names2::character(name)?)
                    }
                    other => u32::from(other),
                };
            }
            if point == 0xfffd || (0xd800..=0xdfff).contains(&point) {
                return Some(point);
            }
        }
    })
}

/// Debug fields default to repr when they have no explicit format spec.
pub fn interpolation_conversion(field: &ast::InterpolatedElement) -> Option<char> {
    field
        .conversion
        .to_char()
        .or_else(|| (field.debug_text.is_some() && field.format_spec.is_none()).then_some('r'))
}

/// Text retained by a template literal independently of evaluating its fields.
#[derive(Debug, Clone)]
pub struct TemplateFieldText {
    pub expression: String,
    pub debug_prefix: Option<String>,
}

pub fn template_fields<'a>(
    source: &str,
    tokens: &Tokens,
    syntax: impl Into<ast::ModRef<'a>>,
) -> HashMap<TextSize, TemplateFieldText> {
    use ast::visitor::{self, Visitor};
    struct Fields<'a> {
        source: &'a str,
        tokens: &'a Tokens,
        fields: HashMap<TextSize, TemplateFieldText>,
    }
    impl Fields<'_> {
        /// Remove comments in this field while keeping newlines, string contents
        /// and comments inside nested string literals verbatim.
        fn text(&self, range: TextRange) -> String {
            let mut output = String::new();
            let mut start = range.start();
            let mut nested = 0;
            for token in self.tokens.in_range(range) {
                match token.kind() {
                    TokenKind::TStringStart | TokenKind::FStringStart => nested += 1,
                    TokenKind::TStringEnd | TokenKind::FStringEnd => nested -= 1,
                    TokenKind::Comment if nested == 0 => {
                        output.push_str(&self.source[TextRange::new(start, token.start())]);
                        start = token.end();
                    }
                    _ => {}
                }
            }
            output.push_str(&self.source[TextRange::new(start, range.end())]);
            if output.contains('\r') {
                output = output.replace("\r\n", "\n").replace('\r', "\n");
            }
            output
        }

        fn record(&mut self, field: &ast::InterpolatedElement) {
            // Ruff omits outer grouping parentheses from the expression range.
            // The delimiter also bounds whitespace and explicit line joins.
            let mut suffix = self
                .tokens
                .after(field.expression.end())
                .iter()
                .filter(|t| !t.kind().is_trivia() && t.kind() != TokenKind::Rpar);
            let Some(delimiter) = suffix.next() else {
                return;
            };
            let start = field.start() + TextSize::new(1);
            let debug_prefix = match delimiter.kind() {
                TokenKind::Equal => suffix
                    .next()
                    .map(|end| self.text(TextRange::new(start, end.start()))),
                TokenKind::Exclamation | TokenKind::Colon | TokenKind::Rbrace => None,
                _ => return,
            };
            let mut expression = self.text(TextRange::new(start, delimiter.start()));
            expression.truncate(expression.trim_end().len());
            self.fields.insert(
                field.start(),
                TemplateFieldText {
                    expression,
                    debug_prefix,
                },
            );

            if let Some(spec) = &field.format_spec {
                for field in spec.elements.interpolations() {
                    self.record(field);
                }
            }
        }
    }
    impl<'a> Visitor<'a> for Fields<'_> {
        fn visit_expr(&mut self, expr: &'a ast::Expr) {
            stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || {
                visitor::walk_expr(self, expr)
            });
        }

        fn visit_t_string(&mut self, template: &'a ast::TString) {
            for field in template.elements.interpolations() {
                self.record(field);
            }
            visitor::walk_t_string(self, template);
        }
    }
    let mut fields = Fields {
        source,
        tokens,
        fields: HashMap::new(),
    };
    match syntax.into() {
        ast::ModRef::Module(module) => fields.visit_body(&module.body),
        ast::ModRef::Expression(expr) => fields.visit_expr(&expr.body),
    }
    fields.fields
}

pub fn template_expressions<'a>(
    source: &str,
    tokens: &Tokens,
    syntax: impl Into<ast::ModRef<'a>>,
) -> HashMap<TextSize, String> {
    template_fields(source, tokens, syntax)
        .into_iter()
        .map(|(start, field)| (start, field.expression))
        .collect()
}

/// Limit the field metadata to forms supported by both native evaluators.
pub fn template_format_spec(
    field: &ast::InterpolatedElement,
) -> Result<Option<&ast::InterpolatedStringLiteralElement>, &'static str> {
    if field.debug_text.is_some() {
        return Err("debug t-string interpolation");
    }
    match field.format_spec.as_deref().map(|spec| &spec.elements[..]) {
        None | Some([]) => Ok(None),
        Some([ast::InterpolatedStringElement::Literal(literal)]) => Ok(Some(literal)),
        _ => Err("nested t-string format spec"),
    }
}

/// Recognize the line boundaries used by Python's str.splitlines.
pub fn is_line_boundary(point: u32) -> bool {
    matches!(point, 0x0a..=0x0d | 0x1c..=0x1e | 0x85 | 0x2028 | 0x2029)
}

/// Python also treats the four ASCII information separators as whitespace.
pub fn python_whitespace(value: char) -> bool {
    value.is_whitespace() || matches!(value, '\u{001c}'..='\u{001f}')
}

/// Length of the common literal space/tab prefix of the supplied nonblank lines.
pub fn common_indent<'a>(mut lines: impl Iterator<Item = &'a str>) -> usize {
    let first = lines.next().unwrap_or("");
    let mut margin = &first.as_bytes()[..first.len() - first.trim_start_matches([' ', '\t']).len()];
    for line in lines {
        if margin.is_empty() {
            break;
        }
        let common = margin
            .iter()
            .zip(line.as_bytes())
            .take_while(|(a, b)| a == b)
            .count();
        margin = &margin[..common];
    }
    margin.len()
}

/// Apply textwrap.dedent to an ordinary Unicode scalar string.
pub fn dedent(text: &str) -> CompactString {
    let blank = |line: &str| line.chars().all(python_whitespace);
    let margin = common_indent(text.split('\n').filter(|line| !blank(line)));
    // Indentation and blank lines can account for nearly the entire input.
    let mut result = CompactString::default();
    for (index, line) in text.split('\n').enumerate() {
        if index != 0 {
            result.push('\n');
        }
        if !blank(line) {
            result.push_str(&line[margin..]);
        }
    }
    result
}

/// CPython expands tabs at eight-character stops, then removes the common
/// margin after the first line. Blank lines and the final newline survive.
pub fn clean_doc(doc: &str) -> String {
    let expanded = ruff_python_trivia::expand_tabs(doc);
    let mut lines = expanded.split('\n');
    let first = lines.next().unwrap_or_default().trim_start_matches(' ');
    let margin = lines
        .clone()
        .filter_map(|line| {
            let content = line.trim_start_matches(' ');
            (!content.is_empty()).then_some(line.len() - content.len())
        })
        .min()
        .unwrap_or(0);
    let mut cleaned = first.to_owned();
    for line in lines {
        cleaned.push('\n');
        let indent = line.len() - line.trim_start_matches(' ').len();
        cleaned.push_str(&line[margin.min(indent)..]);
    }
    cleaned
}

/// Return the first result of checking module, function and class docstrings.
pub fn find_docstring<'a, T>(
    body: &'a [ast::Stmt],
    module_docstring: bool,
    check: impl Fn(&'a ast::ExprStringLiteral) -> Option<T>,
) -> Option<T> {
    use ast::visitor::{Visitor, walk_stmt};

    struct Docs<F, T> {
        check: F,
        found: Option<T>,
    }
    impl<F, T> Docs<F, T> {
        fn check<'a>(&mut self, body: &'a [ast::Stmt])
        where
            F: Fn(&'a ast::ExprStringLiteral) -> Option<T>,
        {
            if let Some(ast::Stmt::Expr(first)) = body.first()
                && let ast::Expr::StringLiteral(text) = first.value.as_ref()
            {
                self.found = (self.check)(text);
            }
        }
    }
    impl<'a, T, F: Fn(&'a ast::ExprStringLiteral) -> Option<T>> Visitor<'a> for Docs<F, T> {
        fn visit_stmt(&mut self, stmt: &'a ast::Stmt) {
            if self.found.is_some() {
                return;
            }
            match stmt {
                ast::Stmt::ClassDef(c) => self.check(&c.body),
                ast::Stmt::FunctionDef(f) => self.check(&f.body),
                _ => {}
            }
            stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || walk_stmt(self, stmt));
        }
        fn visit_expr(&mut self, _: &'a ast::Expr) {}
    }
    let mut docs = Docs { check, found: None };
    if module_docstring {
        docs.check(body);
    }
    docs.visit_body(body);
    docs.found
}

/// Select a bucket for three ASCII bytes, ignoring ASCII letter case.
/// The compiler records literal key prefixes and the runtime tests input prefixes.
#[inline]
pub fn ascii_case_prefix_bit(bytes: &[u8]) -> Option<u64> {
    let prefix = bytes.first_chunk::<3>()?;
    if !prefix.is_ascii() {
        return None;
    }
    let [a, b, c] = prefix.map(|byte| u32::from(byte.to_ascii_lowercase()));
    Some(1_u64 << (((a * 31 + b) * 31 + c) & 63))
}
