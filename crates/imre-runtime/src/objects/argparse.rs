//! Native parsing for a fixed declaration of long options. Parsing checks all
//! input before constructing the compiler-described, owned namespace record.

use crate::StrOwner;
use std::borrow::Cow;
use std::collections::{HashSet, TryReserveError};
use std::io::{self, Write};

use serde::Deserialize;

use super::exceptions::{self, Error, string_repr};
use super::records::{RecordDescriptor, imre_record_new, imre_record_release};
use super::{Object, Str, Words, fail};
use crate::objects::paths::imre_path_new;

mod binding;
mod help;
mod nargs;

use binding::{Bound, IntegerRange};
use nargs::{Nargs, positional_counts};

use crate::integer_text::decimal_digits_allowed;

fn owned_text(value: &str) -> Result<crate::StrOwner, Error> {
    Str::try_new(value)
        .ok()
        .and_then(|text| StrOwner::try_new(text).ok())
        .ok_or_else(Error::allocation)
}

#[derive(Deserialize, Copy, Clone, PartialEq)]
#[serde(rename_all = "lowercase")]
enum Kind {
    Int,
    Float,
    Str,
    Path,
    Bool,
    Class,
}

#[derive(Deserialize, Copy, Clone, PartialEq)]
#[serde(rename_all = "snake_case")]
enum Action {
    Store,
    StoreConst,
    StoreTrue,
    StoreFalse,
    BooleanOptional,
    Count,
}

#[derive(Deserialize)]
struct OptionSpec {
    action: Action,
    flag: String,
    dest: String,
    positional: bool,
    #[serde(default)]
    aliases: Vec<String>,
    enum_default: Option<usize>,
    kind: Kind,
    nargs: Nargs,
    constant: Option<String>,
    list_default: bool,
    required: bool,
    choices: Vec<String>,
    range: Option<IntegerRange>,
    has_choices: bool,
    int_default: bool,
    int_constant: bool,
    float_default: bool,
    float_constant: bool,
    converter: Option<String>,
    default: Option<String>,
    path_default: bool,
    help: String,
    optional: bool,
    group: Option<usize>,
    mutex: Option<usize>,
}

#[derive(Deserialize)]
struct ArgumentGroup {
    title: Option<String>,
    description: Option<String>,
}

#[derive(Deserialize)]
struct Parser {
    #[serde(default)]
    prog: String,
    raw: bool,
    prefixes: String,
    help_flags: Option<[String; 2]>,
    abbrev: bool,
    options: Vec<OptionSpec>,
    #[serde(default)]
    groups: Vec<ArgumentGroup>,
    #[serde(default)]
    mutex_required: Vec<bool>,
    subcommands: Option<Subcommands>,
}

#[derive(Deserialize)]
struct Subcommands {
    group: Option<ArgumentGroup>,
    dest: Option<String>,
    required: bool,
    metavar: Option<String>,
    help: Option<String>,
    choices: Vec<Child>,
}

#[derive(Deserialize)]
struct Child {
    name: String,
    aliases: Vec<String>,
    help: Option<String>,
    #[serde(flatten)]
    parser: Parser,
}

enum Token<'a> {
    Value,
    End,
    Unknown,
    Ambiguous(Vec<&'a str>),
    Help(Option<&'a str>),
    Option(usize, &'a str, Option<&'a str>),
    Cluster(char, &'a str),
}

enum Word<'a> {
    Option(&'a OptionSpec, Bound<'a>),
    Command(Option<&'a str>),
}

impl Child {
    fn spellings(&self) -> impl Iterator<Item = &str> {
        std::iter::once(self.name.as_str()).chain(self.aliases.iter().map(String::as_str))
    }

    fn label(&self) -> String {
        if self.aliases.is_empty() {
            self.name.clone()
        } else {
            format!("{} ({})", self.name, self.aliases.join(", "))
        }
    }
}

impl Subcommands {
    fn spellings(&self) -> impl Iterator<Item = &str> {
        let mut seen = HashSet::new();
        self.choices
            .iter()
            .flat_map(Child::spellings)
            .filter(move |name| seen.insert(*name))
    }

    fn error_name(&self) -> Option<String> {
        self.metavar
            .clone()
            .or_else(|| self.dest.clone())
            .or_else(|| (!self.choices.is_empty()).then(|| self.metavar()))
    }
}

struct Parsed<'a> {
    words: Vec<Word<'a>>,
    extras: Vec<Cow<'a, str>>,
    child: Option<usize>,
}

enum Stop<'a> {
    Help(&'a Parser),
    Error(&'a Parser, String),
    Exception(Error),
}

impl From<TryReserveError> for Stop<'_> {
    fn from(_: TryReserveError) -> Self {
        Self::Exception(Error::allocation())
    }
}

impl Parser {
    fn normalize_numbers(&mut self) {
        for option in &mut self.options {
            let integer = |text: &mut String| {
                if !text
                    .strip_prefix('-')
                    .unwrap_or(text)
                    .bytes()
                    .all(|ch| ch.is_ascii_digit())
                {
                    *text = crate::parse_integer_literal(text)
                        .expect("integer literal from schema")
                        .to_string();
                }
            };
            if option.kind == Kind::Int {
                option.choices.iter_mut().for_each(integer);
            }
            if option.int_default
                && let Some(default) = &mut option.default
            {
                integer(default);
            }
            if option.int_constant
                && let Some(constant) = &mut option.constant
            {
                integer(constant);
            }
            if option.kind != Kind::Float {
                continue;
            }
            let format = |text: &mut String| {
                *text = crate::floats::render(crate::floats::parse(text).expect("literal float"))
                    .to_string();
            };
            for choice in &mut option.choices {
                format(choice);
            }
            if option.float_default
                && let Some(default) = &mut option.default
            {
                format(default);
            }
            if option.float_constant
                && let Some(constant) = &mut option.constant
            {
                format(constant);
            }
        }
        if let Some(commands) = &mut self.subcommands {
            for child in &mut commands.choices {
                child.parser.normalize_numbers();
            }
        }
    }

    fn flags(&self) -> impl Iterator<Item = (&str, Option<usize>)> {
        self.help_flags
            .iter()
            .flatten()
            .map(|flag| (flag.as_str(), None))
            .chain(
                self.options
                    .iter()
                    .enumerate()
                    .filter(|(_, o)| !o.positional())
                    .flat_map(|(i, o)| o.flags().map(move |flag| (flag, Some(i)))),
            )
    }

    fn help_name(&self) -> String {
        self.help_flags.as_ref().expect("help action").join("/")
    }

    /// Resolve exact spellings first, then enumerate every possible long
    /// abbreviation and short cluster in declaration order.
    fn token<'a>(&'a self, arg: &'a str) -> Token<'a> {
        if arg == "--" {
            return Token::End;
        }
        let mut chars = arg.chars();
        let Some(prefix) = chars.next().filter(|&c| self.prefixes.contains(c)) else {
            return Token::Value;
        };
        let token = |flag, slot, value| match slot {
            None => Token::Help(value),
            Some(i) => Token::Option(i, flag, value),
        };
        if let Some((flag, slot)) = self.flags().find(|(s, _)| *s == arg) {
            return token(flag, slot, None);
        }
        let Some(second) = chars.next() else {
            return Token::Value;
        };
        let (head, explicit) = arg
            .split_once('=')
            .map_or((arg, None), |(h, v)| (h, Some(v)));
        if explicit.is_some()
            && let Some((flag, slot)) = self.flags().find(|(s, _)| *s == head)
        {
            return token(flag, slot, explicit);
        }
        let long = self.prefixes.contains(second);
        let short = &arg[..prefix.len_utf8() + second.len_utf8()];
        let mut matches = Vec::new();
        for (flag, slot) in self.flags() {
            if !long && flag == short {
                matches.push((flag, Token::Cluster(prefix, &arg[prefix.len_utf8()..])));
            } else if self.abbrev && flag.starts_with(head) {
                matches.push((flag, token(flag, slot, explicit)));
            }
        }
        if matches.len() > 1 {
            return Token::Ambiguous(matches.into_iter().map(|(flag, _)| flag).collect());
        }
        if let Some((_, found)) = matches.pop() {
            return found;
        }
        let numeric = |s: &str| {
            s.strip_prefix('-').is_some_and(|s| {
                s.strip_prefix('.')
                    .unwrap_or(s)
                    .chars()
                    .next()
                    .and_then(crate::integer_text::decimal_digit)
                    .is_some()
            })
        };
        if (numeric(arg) && !self.flags().any(|(name, _)| numeric(name))) || arg.contains(' ') {
            Token::Value
        } else {
            Token::Unknown
        }
    }

    fn select_group<'a>(&'a self, seen: &mut [Option<usize>], slot: usize) -> Result<(), Stop<'a>> {
        if let Some(group) = self.options[slot].mutex {
            if let Some(other) = seen[group]
                && other != slot
            {
                return Err(Stop::Error(
                    self,
                    format!(
                        "argument {}: not allowed with argument {}",
                        self.options[slot].error_name(),
                        self.options[other].error_name()
                    ),
                ));
            }
            seen[group] = Some(slot);
        }
        Ok(())
    }

    /// Resolve the whole short-option cluster before invoking its actions in order.
    fn apply_flags<'a>(
        &'a self,
        flags: impl IntoIterator<Item = Option<(usize, &'a str)>>,
        seen: &mut [Option<usize>],
        values: &mut [Option<Bound<'a>>],
    ) -> Result<(), Stop<'a>> {
        for flag in flags {
            let Some((slot, spelling)) = flag else {
                return Err(Stop::Help(self));
            };
            self.select_group(seen, slot)?;
            values[slot] =
                Some(self.options[slot].flag_value(self, values[slot].take(), spelling)?);
        }
        Ok(())
    }

    fn parse<'a>(&'a self, args: &[&'a str]) -> Result<Parsed<'a>, Stop<'a>> {
        let mut values = self.options.iter().map(|_| None).collect::<Vec<_>>();
        let mut seen = Vec::new();
        seen.try_reserve_exact(self.mutex_required.len())?;
        seen.resize(self.mutex_required.len(), None);
        let mut extras = Vec::new();
        let mut index = 0;
        let mut selected = None;
        let marker = args.iter().position(|&arg| arg == "--");
        let positional_slots = self
            .options
            .iter()
            .enumerate()
            .filter_map(|(slot, option)| option.positional().then_some(slot))
            .collect::<Vec<_>>();
        let arities = positional_slots
            .iter()
            .map(|&slot| self.options[slot].nargs)
            .chain(self.subcommands.is_some().then_some(Nargs::One))
            .collect::<Vec<_>>();
        let mut next_positional = 0;
        let is_value = |i: usize| {
            marker.is_some_and(|at| i >= at) || matches!(self.token(args[i]), Token::Value)
        };
        'parse: loop {
            if index == args.len() || is_value(index) {
                let mut end = index;
                while end < args.len() && is_value(end) {
                    end += 1;
                }
                let available =
                    end - index - usize::from(marker.is_some_and(|m| (index..end).contains(&m)));
                for count in
                    positional_counts(&arities[next_positional..], available, end < args.len())
                {
                    let slot = positional_slots.get(next_positional);
                    next_positional += 1;
                    if marker == Some(index) {
                        index += 1;
                    }
                    let Some(&slot) = slot else {
                        let commands = self.subcommands.as_ref().unwrap();
                        let name = args[index];
                        let Some((child, definition)) = commands
                            .choices
                            .iter()
                            .enumerate()
                            .find(|(_, child)| child.spellings().any(|word| word == name))
                        else {
                            let choices = commands
                                .spellings()
                                .map(string_repr)
                                .collect::<Vec<_>>()
                                .join(", ");
                            let prefix = commands
                                .error_name()
                                .map_or_else(String::new, |name| format!("argument {name}: "));
                            return Err(Stop::Error(
                                self,
                                format!(
                                    "{prefix}invalid choice: {} (choose from {choices})",
                                    string_repr(name)
                                ),
                            ));
                        };
                        let mut parsed = definition.parser.parse(&args[index + 1..])?;
                        extras.try_reserve(parsed.extras.len())?;
                        extras.append(&mut parsed.extras);
                        selected = Some((child, name, parsed.words));
                        break 'parse;
                    };
                    let mut taken = Vec::new();
                    taken.try_reserve_exact(count)?;
                    for _ in 0..count {
                        if marker == Some(index) {
                            index += 1;
                        }
                        taken.push(args[index]);
                        index += 1;
                    }
                    if marker == Some(index) {
                        index += 1;
                    }
                    values[slot] = Some(self.options[slot].bind(self, Some(&taken))?);
                    if !taken.is_empty() {
                        self.select_group(&mut seen, slot)?;
                    }
                }
                extras.try_reserve(end - index)?;
                extras.extend(args[index..end].iter().copied().map(Cow::Borrowed));
                index = end;
                if index == args.len() {
                    break;
                }
            }
            let arg = args[index];
            index += 1;
            let mut cluster = Vec::new();
            let (slot, spelling, explicit) = match self.token(arg) {
                Token::Help(None) => return Err(Stop::Help(self)),
                Token::Help(Some(value)) => {
                    return Err(Stop::Error(
                        self,
                        format!(
                            "argument {}: ignored explicit argument {}",
                            self.help_name(),
                            string_repr(value)
                        ),
                    ));
                }
                Token::Ambiguous(names) => {
                    return Err(Stop::Error(
                        self,
                        format!("ambiguous option: {arg} could match {}", names.join(", ")),
                    ));
                }
                Token::End | Token::Value => unreachable!("positional run already consumed"),
                Token::Unknown => {
                    extras.try_reserve(1)?;
                    extras.push(Cow::Borrowed(arg));
                    continue;
                }
                Token::Cluster(prefix, mut suffix) => {
                    let mut take = None;
                    while !suffix.is_empty() {
                        let letter = suffix.chars().next().unwrap();
                        let rest = &suffix[letter.len_utf8()..];
                        let spelling = format!("{prefix}{letter}");
                        let Some((matched, slot)) =
                            self.flags().find(|(name, _)| *name == spelling)
                        else {
                            extras.try_reserve(1)?;
                            extras.push(Cow::Owned(format!("{prefix}{suffix}")));
                            break;
                        };
                        let option = slot.map(|slot| (slot, &self.options[slot]));
                        if let Some((slot, option)) = option
                            && option.takes_value()
                        {
                            take = Some((
                                slot,
                                matched,
                                (!rest.is_empty()).then(|| rest.strip_prefix('=').unwrap_or(rest)),
                            ));
                            break;
                        }
                        // argparse resolves the entire cluster (including errors)
                        // before invoking its first action, which may be help.
                        if rest.starts_with('=')
                            || rest
                                .chars()
                                .next()
                                .is_some_and(|c| self.prefixes.contains(c))
                        {
                            let name = option.map_or_else(
                                || self.help_name(),
                                |(_, option)| option.error_name(),
                            );
                            return Err(Stop::Error(
                                self,
                                format!(
                                    "argument {name}: ignored explicit argument {}",
                                    string_repr(rest.strip_prefix('=').unwrap_or(rest))
                                ),
                            ));
                        }
                        cluster.try_reserve(1)?;
                        cluster.push(slot.map(|slot| (slot, matched)));
                        suffix = rest;
                    }
                    let Some(take) = take else {
                        self.apply_flags(cluster, &mut seen, &mut values)?;
                        continue;
                    };
                    take
                }
                Token::Option(slot, spelling, value) => (slot, spelling, value),
            };
            let option = &self.options[slot];
            let taken = if let Some(value) = explicit {
                if !option.takes_value() {
                    return Err(Stop::Error(
                        self,
                        format!(
                            "argument {}: ignored explicit argument {}",
                            option.error_name(),
                            string_repr(value)
                        ),
                    ));
                }
                if option.nargs.bounds().0 > 1 {
                    return Err(Stop::Error(
                        self,
                        format!(
                            "argument {}: {}",
                            option.error_name(),
                            option.nargs.expected()
                        ),
                    ));
                }
                explicit.as_slice()
            } else if !option.takes_value() {
                self.apply_flags([Some((slot, spelling))], &mut seen, &mut values)?;
                continue;
            } else {
                let start = index;
                let (min, max) = option.nargs.bounds();
                while index < args.len()
                    && index - start < max
                    && marker != Some(index)
                    && (matches!(option.nargs, Nargs::Fixed(_))
                        || matches!(self.token(args[index]), Token::Value))
                {
                    index += 1;
                }
                if index - start < min {
                    return Err(Stop::Error(
                        self,
                        format!(
                            "argument {}: {}",
                            option.error_name(),
                            option.nargs.expected()
                        ),
                    ));
                }
                &args[start..index]
            };
            self.apply_flags(cluster, &mut seen, &mut values)?;
            values[slot] = Some(option.bind(self, Some(taken))?);
            self.select_group(&mut seen, slot)?;
        }
        let mut missing = Vec::new();
        let mut words = Vec::with_capacity(values.len());
        for (option, value) in self.options.iter().zip(values) {
            let bound = match value {
                Some(value) => value,
                None if option.required => {
                    missing.push(option.error_name());
                    continue;
                }
                None => option.bind(self, None)?,
            };
            words.push(Word::Option(option, bound));
        }
        if let Some(commands) = &self.subcommands
            && commands.required
            && selected.is_none()
        {
            let name = commands.error_name().ok_or_else(|| {
                Stop::Exception(Error::new(
                    exceptions::Kind::TypeError,
                    format!(
                        "sequence item {}: expected str instance, NoneType found",
                        missing.len()
                    ),
                ))
            })?;
            missing.push(name);
        }
        if !missing.is_empty() {
            return Err(Stop::Error(
                self,
                format!(
                    "the following arguments are required: {}",
                    missing.join(", ")
                ),
            ));
        }
        for (id, required) in self.mutex_required.iter().enumerate() {
            if *required && seen[id].is_none() {
                let names = self
                    .options
                    .iter()
                    .filter(|o| o.mutex == Some(id))
                    .map(OptionSpec::error_name)
                    .collect::<Vec<_>>()
                    .join(" ");
                return Err(Stop::Error(
                    self,
                    format!("one of the arguments {names} is required"),
                ));
            }
        }
        if self
            .subcommands
            .as_ref()
            .is_some_and(|commands| commands.dest.is_some())
        {
            words.push(Word::Command(selected.as_ref().map(|(_, name, _)| *name)));
        }
        let child = selected.map(|(child, _, mut rest)| {
            words.append(&mut rest);
            child
        });
        Ok(Parsed {
            words,
            extras,
            child,
        })
    }
}

impl OptionSpec {
    fn takes_value(&self) -> bool {
        self.action == Action::Store
    }

    fn flags(&self) -> impl Iterator<Item = &str> {
        std::iter::once(self.flag.as_str()).chain(self.aliases.iter().map(String::as_str))
    }

    fn positional(&self) -> bool {
        self.positional
    }

    fn error_name(&self) -> String {
        self.flags().collect::<Vec<_>>().join("/")
    }
}

impl Parser {
    fn from_schema(schema: &str) -> Result<Self, Error> {
        let mut parser: Self = serde_json::from_str(schema)
            .unwrap_or_else(|_| fail("SystemError", "invalid argparse schema"));
        parser.normalize_numbers();
        parser.set_child_programs()?;
        Ok(parser)
    }
}

/// Format help or usage, returning the text or writing it to the requested stream.
///
/// # Safety
/// The compiler roots the descriptor and description; output mode 3 also roots
/// a native text stream in `file`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_argparse_text(
    schema: *const Str,
    description: *const Str,
    usage: bool,
    output: u32,
    file: *const Object,
) -> *const Str {
    // SAFETY: both strings stay rooted while the formatter reads them.
    let (schema, description) = unsafe { (&*schema, &*description) };
    let parser = match Parser::from_schema(schema) {
        Ok(parser) => parser,
        Err(error) => return error.returning(std::ptr::null_mut()),
    };
    let text: Str = if usage {
        parser.usage(help::width())
    } else {
        match parser.help(description) {
            Ok(help) => help,
            Err(error) => return error.returning(std::ptr::null()),
        }
    }
    .into();
    match output {
        0 => return StrOwner::into_raw(StrOwner::new(text)),
        1 | 2 => {
            // SAFETY: the native stream borrows the live string. A successful
            // write returns an owned character count; zero indicates failure.
            let count = unsafe { crate::sys::imre_sys_stream_write(output - 1, &text) };
            if count != 0 {
                drop(crate::Int(count));
            }
        }
        3 => {
            // SAFETY: file has the exact native text stream type.
            unsafe { super::files::imre_text_write_discard(file, &text) };
        }
        _ => unreachable!(),
    }
    // _print_message ignores OSError from write(), but closed files raise
    // ValueError and that exception must still reach the caller.
    if let Some(error) = exceptions::take_pending() {
        // SAFETY: the pending exception stays rooted for the kind check.
        if unsafe {
            exceptions::imre_exception_matches(
                crate::ObjectOwner::as_ptr(&error),
                exceptions::Kind::OSError as u32,
            )
        } == 0
        {
            exceptions::replace_pending(Some(error));
        }
    }
    std::ptr::null()
}

/// Parses an explicit string list or the current sys.argv[1..] into a namespace.
///
/// # Safety
/// The compiler roots the inputs. `layout` and each descriptor in `children`
/// match the inferred fields for that parser. The child table contains one
/// descriptor per declared subcommand in order, or is null for a flat parser.
/// For parse_known_args, `pair_layout` describes (namespace, list[str]); otherwise it is null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_argparse_parse(
    layout: *const RecordDescriptor,
    children: *const *const RecordDescriptor,
    pair_layout: *const RecordDescriptor,
    schema: *const Str,
    description: *const Str,
    argv: *const Object,
    process_argv: bool,
) -> *mut usize {
    // SAFETY: these immutable inputs remain rooted during parsing and construction.
    let (schema, description, argv) = unsafe { (&*schema, &*description, &*argv) };
    let parser = match Parser::from_schema(schema) {
        Ok(parser) => parser,
        Err(error) => return error.returning(std::ptr::null_mut()),
    };
    let mut owners = Vec::new();
    let mut args = Vec::new();
    // SAFETY: argv is a rooted list[str]; take independent string owners before
    // reading the argument values or constructing the output namespace.
    let reserved: Result<(), Error> = unsafe {
        argv.0.with(|storage| {
            let sequence = storage.sequence()?;
            sequence.check_kind(3);
            let words = sequence.values.iter().skip(usize::from(process_argv));
            owners.try_reserve_exact(words.len()).map_err(|_| Error::allocation())?;
            args.try_reserve_exact(words.len()).map_err(|_| Error::allocation())?;
            owners.extend(words.map(|&word| crate::StrOwner::acquire_word(word)));
            Ok(())
        })
    };
    if let Err(error) = reserved {
        return error.returning(std::ptr::null_mut());
    }
    args.extend(owners.iter().map(|value| value.as_str()));
    let parsed = match parser.parse(&args).and_then(|parsed| {
        if !pair_layout.is_null() || parsed.extras.is_empty() {
            Ok(parsed)
        } else {
            Err(Stop::Error(
                &parser,
                format!("unrecognized arguments: {}", parsed.extras.join(" ")),
            ))
        }
    }) {
        Ok(values) => values,
        Err(Stop::Exception(error)) => return error.returning(std::ptr::null_mut()),
        Err(Stop::Help(active)) => {
            let help = match active.help(if std::ptr::eq(active, &parser) {
                description
            } else {
                ""
            }) {
                Ok(help) => help,
                Err(error) => return error.returning(std::ptr::null_mut()),
            };
            if crate::write_output(|writer| {
                writer.write_all(help.trim_end_matches('\n').as_bytes())
            })
            .is_err()
            {
                return Error::new(exceptions::Kind::OSError, "could not write standard output")
                    .returning(std::ptr::null_mut());
            }
            return Error::exit_status(0).returning(std::ptr::null_mut());
        }
        Err(Stop::Error(active, error)) => {
            let _ = crate::flush_stdout();
            let _ = crate::sys::flush_stderr();
            if writeln!(
                io::stderr().lock(),
                "{}{}: error: {}",
                active.usage(help::width()),
                active.prog,
                error
            )
            .is_err()
            {
                return Error::new(exceptions::Kind::OSError, "could not write standard error")
                    .returning(std::ptr::null_mut());
            }
            return Error::exit_status(2).returning(std::ptr::null_mut());
        }
    };
    let mut extras = Words::new(3);
    if !pair_layout.is_null() {
        if extras
            .values
            .try_reserve_exact(parsed.extras.len())
            .is_err()
        {
            return Error::allocation().returning(std::ptr::null_mut());
        }
        for value in &parsed.extras {
            let text = match owned_text(value) {
                Ok(text) => text,
                Err(error) => return error.returning(std::ptr::null_mut()),
            };
            extras.values.push(StrOwner::into_raw(text) as usize);
        }
    }
    // SAFETY: every slot receives exactly one owner of its declared kind. All
    // input validation is complete, and option conversion invokes no user code.
    unsafe {
        let layout = parsed.child.map_or(layout, |index| *children.add(index));
        let layout_ref = &*layout;
        if layout_ref.length != parsed.words.len() {
            fail("SystemError", "argparse namespace layout mismatch");
        }
        let result = imre_record_new(layout);
        for (index, field) in parsed.words.into_iter().enumerate() {
            let word = match field {
                Word::Option(option, values) => option.word(values),
                Word::Command(value) => value.map_or(Ok(0), |name| {
                    owned_text(name).map(|text| StrOwner::into_raw(text) as usize)
                }),
            };
            let word = match word {
                Ok(word) => word,
                Err(error) => {
                    // Unfilled record slots are zeroed so its initialized fields can be released.
                    imre_record_release(result);
                    return error.returning(std::ptr::null_mut());
                }
            };
            let dest = result.add(index);
            match *layout_ref.kinds.add(index) {
                3..=5 => dest.cast::<*const ()>().write(word as *const ()),
                _ => dest.write(word),
            }
        }
        if pair_layout.is_null() {
            return result;
        }
        let pair = imre_record_new(pair_layout);
        pair.cast::<*mut usize>().write(result);
        pair.add(1)
            .cast::<*const Object>()
            .write(crate::ObjectOwner::into_raw(extras.into_object()));
        pair
    }
}
