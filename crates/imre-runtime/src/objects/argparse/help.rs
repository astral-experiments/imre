//! Usage and help formatting for long options, using Python code-point widths.

use std::collections::VecDeque;
use std::fmt::Write;
use std::io::IsTerminal;

use icu_properties::props::{EnumeratedProperty, GeneralCategory, GeneralCategoryGroup};

use super::super::python_whitespace;
use super::{Kind, OptionSpec, Parser, Subcommands};

/// Parse the positive decimal width accepted by shutil.get_terminal_size().
fn positive_columns(value: &str) -> Option<usize> {
    let value = value.trim();
    let digits = value.strip_prefix('+').unwrap_or(value);
    let mut width = 0usize;
    let mut count = 0;
    for group in digits.split('_') {
        if group.is_empty() {
            return None;
        }
        for ch in group.chars() {
            let digit = crate::integer_text::decimal_digit(ch)?;
            width = width.saturating_mul(10).saturating_add(usize::from(digit));
            count += 1;
        }
    }
    // Beyond the minimum threshold, CPython's decimal conversion guard can
    // reject a width even when it consists almost entirely of leading zeroes.
    if count > 640 {
        let limit = std::env::var("PYTHONINTMAXSTRDIGITS")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(4300);
        if limit != 0 && count > limit {
            return None;
        }
    }
    (width > 0).then_some(width)
}

pub(super) fn width() -> usize {
    let columns = std::env::var("COLUMNS")
        .ok()
        .and_then(|value| positive_columns(&value))
        .or_else(|| {
            let mut size = std::mem::MaybeUninit::<libc::winsize>::uninit();
            // SAFETY: ioctl writes a winsize to this buffer only on success.
            if unsafe { libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, size.as_mut_ptr()) } == 0
            {
                // SAFETY: the successful ioctl initialized the terminal dimensions.
                let columns = usize::from(unsafe { size.assume_init() }.ws_col);
                (columns > 0).then_some(columns)
            } else {
                None
            }
        })
        .unwrap_or(80usize);
    columns.saturating_sub(2)
}

fn len(text: &str) -> usize {
    text.chars().count()
}

fn color_enabled() -> bool {
    match std::env::var("PYTHON_COLORS").as_deref() {
        Ok("0") => return false,
        Ok("1") => return true,
        _ => {}
    }
    let enabled = |name| std::env::var_os(name).is_some_and(|v| !v.is_empty());
    !enabled("NO_COLOR")
        && (enabled("FORCE_COLOR")
            || std::env::var_os("TERM").as_deref() != Some(std::ffi::OsStr::new("dumb"))
                && std::io::stdout().is_terminal())
}

fn paint(enabled: bool, code: &str, text: &str) -> String {
    if enabled {
        format!("\x1b[{code}m{text}\x1b[0m")
    } else {
        text.to_owned()
    }
}

fn option_label(label: &str, summary: bool, color: bool) -> String {
    if !color {
        return label.to_owned();
    }
    if let Some((flag, args)) = label.split_once(' ') {
        if summary {
            format!("\x1b[36m{flag} \x1b[33m{args}\x1b[0m")
        } else {
            format!(
                "{} {}",
                paint(true, "1;36", flag),
                paint(true, "1;33", args)
            )
        }
    } else {
        paint(true, if summary { "36" } else { "1;36" }, label)
    }
}

impl OptionSpec {
    fn dest(&self) -> String {
        self.flag[2..].replace('-', "_")
    }

    fn invocation(&self) -> String {
        if self.kind == Kind::Bool {
            return self.flag.clone();
        }
        let metavar = if self.choices.is_empty() {
            self.dest().to_uppercase()
        } else {
            format!("{{{}}}", self.choices.join(","))
        };
        if self.many {
            format!("{} {metavar} [{metavar} ...]", self.flag)
        } else {
            format!("{} {metavar}", self.flag)
        }
    }
}

/// Group words and punctuation as textwrap does before fitting each line.
fn word_chunks(word: &str, chunks: &mut VecDeque<String>) {
    // textwrap's "letter" matches [^\d\W], including nondecimal numbers.
    const NON_DECIMAL: GeneralCategoryGroup = GeneralCategoryGroup::Letter
        .union(GeneralCategoryGroup::LetterNumber)
        .union(GeneralCategoryGroup::OtherNumber);
    let word_char = |ch: char, categories: GeneralCategoryGroup| {
        ch == '_'
            || categories.contains(GeneralCategory::for_char(ch))
                && (ch.is_ascii() || !crate::integer_text::newer_printable(ch))
    };
    let letter = |ch| word_char(ch, NON_DECIMAL);
    let is_word = |ch| word_char(ch, NON_DECIMAL.union(GeneralCategoryGroup::DecimalNumber));
    let chars = word.chars().collect::<Vec<_>>();
    let mut start = 0;
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '-' {
            let mut end = i + 1;
            while end < chars.len() && chars[end] == '-' {
                end += 1;
            }
            if end - i >= 2
                && i > 0
                && (is_word(chars[i - 1]) || "!\"'&.,?".contains(chars[i - 1]))
                && end < chars.len()
                && is_word(chars[end])
            {
                if i != start {
                    chunks.push_back(chars[start..i].iter().collect())
                }
                chunks.push_back(chars[i..end].iter().collect());
                start = end;
            } else if end == i + 1
                && i >= 2
                && letter(chars[i - 1])
                && (letter(chars[i - 2]) || (i >= 3 && chars[i - 2] == '-' && letter(chars[i - 3])))
                && chars.get(i + 1).copied().is_some_and(letter)
            {
                let next = if chars.get(i + 2) == Some(&'-') {
                    i + 3
                } else {
                    i + 2
                };
                if chars.get(next).copied().is_some_and(letter) {
                    chunks.push_back(chars[start..end].iter().collect());
                    start = end;
                }
            }
            i = end;
        } else {
            i += 1;
        }
    }
    if start < chars.len() {
        chunks.push_back(chars[start..].iter().collect());
    }
}

fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut chunks = VecDeque::new();
    // argparse collapses ASCII regex whitespace, then strips Python whitespace.
    for word in text
        .trim_matches(python_whitespace)
        .split([' ', '\t', '\n', '\r', '\x0b', '\x0c'])
        .filter(|word| !word.is_empty())
    {
        if !chunks.is_empty() {
            chunks.push_back(" ".into());
        }
        word_chunks(word, &mut chunks);
    }
    let mut lines = Vec::new();
    while !chunks.is_empty() {
        if !lines.is_empty()
            && chunks
                .front()
                .is_some_and(|chunk| chunk.chars().all(python_whitespace))
        {
            chunks.pop_front();
        }
        let mut line = String::new();
        let mut last_start = 0;
        let mut size = 0;
        while let Some(chunk) = chunks.front() {
            let length = len(chunk);
            if size + length > width {
                break;
            }
            size += length;
            last_start = line.len();
            line.push_str(&chunks.pop_front().unwrap());
        }
        if let Some(chunk) = chunks.front_mut()
            && len(chunk) > width
            && size < width
        {
            let space = width - size;
            let boundary = chunk
                .char_indices()
                .nth(space)
                .map_or(chunk.len(), |(offset, _)| offset);
            let prefix = &chunk[..boundary];
            let end = prefix
                .rfind('-')
                .filter(|&pos| prefix[..pos].chars().any(|ch| ch != '-'))
                .map_or(boundary, |pos| pos + 1);
            last_start = line.len();
            line.push_str(&chunk[..end]);
            chunk.drain(..end);
        }
        // textwrap discards a trailing whitespace-only chunk in its entirety.
        if line[last_start..].chars().all(python_whitespace) {
            line.truncate(last_start);
        }
        if !line.is_empty() {
            lines.push(line);
        }
    }
    lines
}

fn usage_lines(
    parts: &[(usize, String)],
    indent: usize,
    mut initial: usize,
    width: usize,
) -> String {
    let mut output = String::new();
    let mut first = true;
    for &(length, ref part) in parts {
        if !first && initial + 1 + length > width {
            output.push('\n');
            output.push_str(&" ".repeat(indent));
            initial = indent;
            first = true;
        }
        if !first {
            initial += 1;
            output.push(' ');
        }
        output.push_str(part);
        initial += length;
        first = false;
    }
    output
}

impl Subcommands {
    fn metavar(&self) -> String {
        format!(
            "{{{}}}",
            self.choices
                .iter()
                .map(|child| child.name.as_str())
                .collect::<Vec<_>>()
                .join(",")
        )
    }
}

/// Formats a help action at its real indentation, using the section-wide column.
fn help_row(
    output: &mut String,
    width: usize,
    position: usize,
    indent: usize,
    label: &str,
    styled: &str,
    help: &str,
) {
    write!(output, "{}{styled}", " ".repeat(indent)).unwrap();
    if help.is_empty() {
        output.push('\n');
        return;
    }
    let fits = indent + len(label) + 2 <= position;
    if !fits {
        output.push('\n');
    }
    let padding = if fits {
        position - indent - len(label)
    } else {
        position
    };
    if help.chars().all(python_whitespace) {
        if fits {
            output.push_str(&" ".repeat(padding));
            output.push('\n');
        }
        return;
    }
    output.push_str(&" ".repeat(padding));
    for (index, line) in wrap(help, width.saturating_sub(position).max(11))
        .iter()
        .enumerate()
    {
        if index != 0 {
            output.push_str(&" ".repeat(position));
        }
        writeln!(output, "{line}").unwrap();
    }
}

impl Parser {
    pub(super) fn usage(&self, width: usize) -> String {
        let color = color_enabled();
        let short = format!("[{}]", paint(color, "32", "-h"));
        let mut parts = vec![
            (len(&self.prog), paint(color, "1;35", &self.prog)),
            (4, short),
        ];
        for option in &self.options {
            let label = option.invocation();
            let length = len(&label);
            let styled = option_label(&label, true, color);
            parts.push(if option.required {
                (length, styled)
            } else {
                (length + 2, format!("[{styled}]"))
            });
        }
        let positional = self.subcommands.as_ref().map(|commands| {
            let text = format!("{} ...", commands.metavar());
            (len(&text), paint(color, "32", &text))
        });
        let prefix = "usage: ".len();
        let mut text = paint(color, "1;34", "usage: ");
        let full_width = parts.iter().map(|(width, _)| *width).sum::<usize>() + parts.len() - 1
            + positional.as_ref().map_or(0, |(len, _)| len + 1);
        if prefix + full_width <= width {
            for (i, (_, part)) in parts.iter().chain(&positional).enumerate() {
                if i != 0 {
                    text.push(' ');
                }
                text.push_str(part);
            }
        } else if (prefix + len(&self.prog)) * 4 <= 3 * width {
            text.push_str(&usage_lines(
                &parts,
                prefix + len(&self.prog) + 1,
                prefix,
                width,
            ));
            if let Some((_, part)) = positional {
                text.push('\n');
                text.push_str(&" ".repeat(prefix + len(&self.prog) + 1));
                text.push_str(&part);
            }
        } else {
            text.push_str(&parts[0].1);
            text.push('\n');
            text.push_str(&" ".repeat(prefix));
            let mut options = usage_lines(&parts[1..], prefix, prefix, width);
            if let Some((n, part)) = positional {
                let option_width: usize = parts[1..].iter().map(|(n, _)| n + 1).sum();
                if prefix + option_width + n > width {
                    options.push('\n');
                    options.push_str(&" ".repeat(prefix));
                } else {
                    options.push(' ');
                }
                options.push_str(&part);
            }
            text.push_str(&options);
        }
        text.push('\n');
        text
    }

    pub(super) fn help(&self, description: &str) -> String {
        let width = width();
        let mut output = self.usage(width);
        if !description.is_empty() {
            let description = if description.contains("%(prog)") {
                expand(description, &self.prog, None)
            } else {
                description.to_owned()
            };
            let description = if self.raw {
                description
            } else {
                wrap(&description, width.max(11)).join("\n")
            };
            if !description.is_empty() {
                write!(output, "\n{description}\n").unwrap();
            }
        }
        let color = color_enabled();
        let mut rows = vec![(
            "-h, --help".to_owned(),
            "show this help message and exit".to_owned(),
        )];
        rows.extend(self.options.iter().map(|option| {
            (
                option.invocation(),
                expand(&option.help, &self.prog, Some(option)),
            )
        }));
        let mut max_header = rows.iter().map(|(label, _)| len(label)).max().unwrap();
        if let Some(commands) = &self.subcommands {
            max_header = max_header.max(len(&commands.metavar()));
            for child in &commands.choices {
                if child.help.is_some() {
                    max_header = max_header.max(2 + len(&child.name));
                }
            }
        }
        let position = (max_header + 4)
            .min(24)
            .min(width.saturating_sub(20).max(4));
        if let Some(commands) = &self.subcommands {
            writeln!(
                output,
                "\n{}",
                paint(color, "1;34", "positional arguments:")
            )
            .unwrap();
            let label = commands.metavar();
            help_row(
                &mut output,
                width,
                position,
                2,
                &label,
                &paint(color, "1;32", &label),
                "",
            );
            for child in &commands.choices {
                if let Some(help) = &child.help {
                    help_row(
                        &mut output,
                        width,
                        position,
                        4,
                        &child.name,
                        &paint(color, "1;32", &child.name),
                        &expand(help, &self.prog, None),
                    );
                }
            }
        }
        writeln!(output, "\n{}", paint(color, "1;34", "options:")).unwrap();
        for (label, help) in rows {
            let styled = if label == "-h, --help" {
                format!(
                    "{}, {}",
                    paint(color, "1;32", "-h"),
                    paint(color, "1;36", "--help")
                )
            } else {
                option_label(&label, false, color)
            };
            help_row(&mut output, width, position, 2, &label, &styled, &help);
        }
        // HelpFormatter collapses three or more newlines throughout the report,
        // including blank lines at the edges of a raw module docstring.
        while output.contains("\n\n\n") {
            output = output.replace("\n\n\n", "\n\n");
        }
        output.truncate(output.trim_end_matches('\n').len());
        output.push('\n');
        output
    }
}

fn expand(template: &str, prog: &str, option: Option<&OptionSpec>) -> String {
    let mut result = String::new();
    let mut rest = template;
    while let Some((prefix, next)) = rest.split_once('%') {
        result.push_str(prefix);
        if let Some(after) = next.strip_prefix('%') {
            result.push('%');
            rest = after;
            continue;
        }
        if let Some((name, after)) = next
            .strip_prefix('(')
            .and_then(|field| field.split_once(")s"))
        {
            let value = match (name, option) {
                ("prog", _) => prog.to_owned(),
                ("dest", Some(option)) => option.dest(),
                ("type", Some(option)) => option.converter.as_deref().unwrap_or("None").to_owned(),
                ("default", Some(option)) if option.path_default => {
                    super::super::paths::normalize(option.default.as_deref().unwrap()).to_string()
                }
                ("default", Some(option)) => option.default.as_deref().unwrap_or("None").to_owned(),
                ("choices", Some(option)) => {
                    if option.choices.is_empty() {
                        "None".into()
                    } else {
                        option.choices.join(", ")
                    }
                }
                ("required", Some(option)) => {
                    if option.required {
                        "True".into()
                    } else {
                        "False".into()
                    }
                }
                ("nargs", Some(option)) => {
                    if option.many {
                        "+".into()
                    } else if option.kind == Kind::Bool {
                        "0".into()
                    } else {
                        "None".into()
                    }
                }
                _ => unreachable!("compile-time argparse template check"),
            };
            result.push_str(&value);
            rest = after;
            continue;
        }
        result.push('%');
        rest = next;
    }
    result.push_str(rest);
    result
}
