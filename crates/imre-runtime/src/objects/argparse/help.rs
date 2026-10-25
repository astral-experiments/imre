//! Usage and help formatting for long options, using Python code-point widths.

use std::collections::VecDeque;
use std::fmt::Write;
use std::io::IsTerminal;

use icu_properties::props::{EnumeratedProperty, GeneralCategory, GeneralCategoryGroup};

use super::super::python_whitespace;
use super::{ArgumentGroup, Kind, OptionSpec, Parser, Subcommands};

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
    (width > 0 && super::decimal_digits_allowed(count)).then_some(width)
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

impl OptionSpec {
    fn dest(&self) -> String {
        self.dest.clone()
    }

    fn label(&self, summary: bool, color: bool) -> String {
        let mut metavar = if !self.has_choices {
            if self.positional() {
                self.dest()
            } else {
                self.dest().to_uppercase()
            }
        } else {
            format!("{{{}}}", self.choices_text(",", false))
        };
        if self.positional() {
            if summary {
                metavar = self.nargs.label(&metavar);
            }
            return paint(color, if summary { "32" } else { "1;32" }, &metavar);
        }
        metavar = self.nargs.label(&metavar);
        let option_color = |flag: &str, bold: bool| match (flag.chars().count() > 2, bold) {
            (true, true) => "1;36",
            (true, false) => "36",
            (false, true) => "1;32",
            (false, false) => "32",
        };
        if summary {
            if self.kind == Kind::Bool {
                return paint(color, option_color(&self.flag, false), &self.flag);
            }
            return if color {
                format!(
                    "\x1b[{}m{} \x1b[33m{metavar}\x1b[0m",
                    option_color(&self.flag, false),
                    self.flag
                )
            } else {
                format!("{} {metavar}", self.flag)
            };
        }
        let mut label = self
            .flags()
            .map(|flag| paint(color, option_color(flag, true), flag))
            .collect::<Vec<_>>()
            .join(", ");
        if self.kind != Kind::Bool {
            label.push(' ');
            label.push_str(&paint(color, "1;33", &metavar));
        }
        label
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
    pub(super) fn metavar(&self) -> String {
        self.metavar
            .clone()
            .unwrap_or_else(|| format!("{{{}}}", self.spellings().collect::<Vec<_>>().join(",")))
    }
    fn write_rows(
        &self,
        output: &mut String,
        prog: &str,
        width: usize,
        position: usize,
        color: bool,
    ) {
        let label = self.metavar();
        help_row(
            output,
            width,
            position,
            2,
            &label,
            &paint(color, "1;32", &label),
            &expand(self.help.as_deref().unwrap_or_default(), prog, None),
        );
        for child in &self.choices {
            if let Some(help) = &child.help {
                let label = child.label();
                help_row(
                    output,
                    width,
                    position,
                    4,
                    &label,
                    &paint(color, "1;32", &label),
                    &expand(help, prog, None),
                );
            }
        }
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
    /// Subparser names use the parent's uncolored usage without its options.
    pub(super) fn set_child_programs(&mut self) {
        let Some(commands) = &mut self.subcommands else {
            return;
        };
        let mut parts = vec![(len(&self.prog), self.prog.clone())];
        parts.extend(self.options.iter().filter(|o| o.positional()).map(|o| {
            let label = o.label(true, false);
            (len(&label), label)
        }));
        let width = width();
        let prog = if parts.len() == 1 {
            self.prog.clone()
        } else if parts.iter().map(|(length, _)| length + 1).sum::<usize>() - 1 > width
            && len(&self.prog) * 4 > width.saturating_mul(3)
        {
            format!("{}\n{}", self.prog, usage_lines(&parts[1..], 0, 0, width))
        } else {
            usage_lines(&parts, len(&self.prog) + 1, 0, width)
        };
        for child in &mut commands.choices {
            child.parser.prog = format!("{} {}", prog.trim_matches(python_whitespace), child.name);
        }
    }

    fn description(&self, text: &str, width: usize, indent: usize) -> String {
        let text = if text.contains("%(prog)") {
            expand(text, &self.prog, None)
        } else {
            text.to_owned()
        };
        let prefix = " ".repeat(indent);
        if self.raw {
            let mut offsets = super::super::LineOffsets::new(&text);
            let mut result = String::new();
            while let Some(line) = offsets.next(&text) {
                result.push_str(&prefix);
                result.push_str(&text[line.start..offsets.start]);
            }
            result
        } else {
            // _format_text subtracts the current indent before textwrap.fill;
            // textwrap then counts its initial/subsequent indent in that width.
            let width = width.saturating_sub(indent).max(11);
            wrap(&text, width.saturating_sub(indent).max(1))
                .into_iter()
                .map(|line| format!("{prefix}{line}"))
                .collect::<Vec<_>>()
                .join("\n")
        }
    }

    pub(super) fn usage(&self, width: usize) -> String {
        let color = color_enabled();
        let mut parts = vec![(len(&self.prog), paint(color, "1;35", &self.prog))];
        if let Some([short, _]) = &self.help_flags {
            parts.push((len(short) + 2, format!("[{}]", paint(color, "32", short))));
        }
        for option in self.options.iter().filter(|o| !o.positional()) {
            let length = len(&option.label(true, false));
            let styled = option.label(true, color);
            parts.push(if option.required {
                (length, styled)
            } else {
                (length + 2, format!("[{styled}]"))
            });
        }
        let mut positional: Vec<_> = self
            .options
            .iter()
            .filter(|o| o.positional())
            .map(|option| (len(&option.label(true, false)), option.label(true, color)))
            .collect();
        if let Some(commands) = &self.subcommands {
            let word = format!("{} ...", commands.metavar());
            positional.push((len(&word), paint(color, "32", &word)));
        }
        let prefix = "usage: ".len();
        let mut text = paint(color, "1;34", "usage: ");
        let width_of = |parts: &[(usize, String)]| parts.iter().map(|(n, _)| n + 1).sum::<usize>();
        if (parts.len() == 1 && positional.is_empty())
            || prefix + width_of(&parts) + width_of(&positional) - 1 <= width
        {
            text.push_str(
                &parts
                    .iter()
                    .chain(&positional)
                    .map(|(_, part)| part.as_str())
                    .collect::<Vec<_>>()
                    .join(" "),
            );
        } else {
            if parts.len() == 1 {
                parts.append(&mut positional);
            }
            let short_prog = (prefix + len(&self.prog)) * 4 <= 3 * width;
            let indent = if short_prog {
                prefix + len(&self.prog) + 1
            } else {
                prefix
            };
            if short_prog {
                text.push_str(&usage_lines(&parts, indent, prefix, width));
            } else {
                text.push_str(&parts[0].1);
                text.push('\n');
                text.push_str(&" ".repeat(indent));
                if prefix + width_of(&parts[1..]) + width_of(&positional) - 1 <= width {
                    parts.append(&mut positional);
                }
                text.push_str(&usage_lines(&parts[1..], indent, indent, width));
            }
            if !positional.is_empty() {
                text.push('\n');
                text.push_str(&" ".repeat(indent));
                text.push_str(&usage_lines(&positional, indent, indent, width));
            }
        }
        text.push('\n');
        text
    }

    fn open_group(&self, output: &mut String, group: &ArgumentGroup, width: usize, color: bool) {
        output.push('\n');
        if let Some(title) = &group.title {
            writeln!(output, "{}", paint(color, "1;34", &format!("{title}:"))).unwrap();
        }
        if let Some(text) = &group.description {
            writeln!(output, "{}\n", self.description(text, width, 2)).unwrap();
        }
    }

    pub(super) fn help(&self, description: &str) -> String {
        let width = width();
        let mut output = self.usage(width);
        if !description.is_empty() {
            let description = self.description(description, width, 0);
            if !description.is_empty() {
                write!(output, "\n{description}\n").unwrap();
            }
        }
        let color = color_enabled();
        let mut rows = Vec::new();
        if let Some([short, long]) = &self.help_flags {
            rows.push((
                format!("{short}, {long}"),
                format!(
                    "{}, {}",
                    paint(color, "1;32", short),
                    paint(color, "1;36", long)
                ),
                "show this help message and exit".to_owned(),
            ));
        }
        let mut positionals = Vec::new();
        let mut grouped = vec![Vec::new(); self.groups.len()];
        for option in &self.options {
            let row = (
                option.label(false, false),
                option.label(false, color),
                expand(&option.help, &self.prog, Some(option)),
            );
            if let Some(group) = option.group {
                grouped[group].push(row);
            } else if option.positional() {
                positionals.push(row);
            } else {
                rows.push(row);
            }
        }
        let mut max_header = rows
            .iter()
            .chain(&positionals)
            .chain(grouped.iter().flatten())
            .map(|(label, _, _)| len(label))
            .max()
            .unwrap_or(0);
        if let Some(commands) = &self.subcommands {
            max_header = max_header.max(len(&commands.metavar()));
            for child in &commands.choices {
                if child.help.is_some() {
                    max_header = max_header.max(2 + len(&child.label()));
                }
            }
        }
        let position = (max_header + 4)
            .min(24)
            .min(width.saturating_sub(20).max(4));
        let inline = self
            .subcommands
            .as_ref()
            .filter(|commands| commands.group.is_none());
        if !positionals.is_empty() || inline.is_some() {
            writeln!(
                output,
                "\n{}",
                paint(color, "1;34", "positional arguments:")
            )
            .unwrap();
            for (label, styled, help) in positionals {
                help_row(&mut output, width, position, 2, &label, &styled, &help);
            }
        }
        if let Some(commands) = inline {
            commands.write_rows(&mut output, &self.prog, width, position, color);
        }
        if !rows.is_empty() {
            writeln!(output, "\n{}", paint(color, "1;34", "options:")).unwrap();
        }
        for (label, styled, help) in rows {
            help_row(&mut output, width, position, 2, &label, &styled, &help);
        }
        for (group, rows) in self.groups.iter().zip(grouped) {
            if rows.is_empty() && group.description.is_none() {
                continue;
            }
            self.open_group(&mut output, group, width, color);
            for (label, styled, help) in rows {
                help_row(&mut output, width, position, 2, &label, &styled, &help);
            }
        }
        if let Some(commands) = &self.subcommands
            && let Some(group) = &commands.group
        {
            self.open_group(&mut output, group, width, color);
            commands.write_rows(&mut output, &self.prog, width, position, color);
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
                ("const", Some(option)) => option.constant.as_deref().unwrap_or("None").to_owned(),
                ("choices", Some(option)) => {
                    if !option.has_choices {
                        "None".into()
                    } else {
                        option.choices_text(", ", false)
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
                    if option.kind == Kind::Bool {
                        "0".into()
                    } else {
                        option.nargs.spelling()
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
