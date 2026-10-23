//! Native parsing for a fixed declaration of long options. Parsing checks all
//! input before constructing the compiler-described, owned namespace record.

use std::io::{self, Write};
use std::rc::Rc;

use icu_properties::props::{EnumeratedProperty, GeneralCategory};
use serde::Deserialize;

use super::exceptions::{self, Error, string_repr};
use super::records::{RecordDescriptor, relar_record_new};
use super::{Object, Str, Words, fail};
use crate::objects::paths::relar_path_new;

mod help;

#[derive(Deserialize, Copy, Clone, PartialEq)]
#[serde(rename_all = "lowercase")]
enum Kind {
    Str,
    Path,
    Bool,
}

#[derive(Deserialize)]
struct OptionSpec {
    flag: String,
    kind: Kind,
    many: bool,
    required: bool,
    choices: Vec<String>,
    converter: Option<String>,
    default: Option<String>,
    path_default: bool,
    help: String,
    optional: bool,
}

#[derive(Deserialize)]
struct Parser {
    prog: String,
    raw: bool,
    options: Vec<OptionSpec>,
    subcommands: Option<Subcommands>,
}

#[derive(Deserialize)]
struct Subcommands {
    dest: String,
    choices: Vec<Child>,
}

#[derive(Deserialize)]
struct Child {
    name: String,
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
    Option(usize, Option<&'a str>),
}

enum Word<'a> {
    Option(&'a OptionSpec, Option<Vec<&'a str>>),
    Command(&'a str),
}

struct Parsed<'a> {
    words: Vec<Word<'a>>,
    extras: Vec<&'a str>,
    child: Option<usize>,
}

enum Stop<'a> {
    Help(&'a Parser),
    Error(&'a Parser, String),
}

impl Parser {
    fn token<'a>(&'a self, arg: &'a str) -> Token<'a> {
        if arg == "--" {
            return Token::End;
        }
        let Some(tail) = arg.strip_prefix('-') else {
            return Token::Value;
        };
        if tail.is_empty() {
            return Token::Value;
        }
        let (flag, explicit) = arg
            .split_once('=')
            .map_or((arg, None), |(flag, value)| (flag, Some(value)));
        if matches!(flag, "-h" | "--help") {
            return Token::Help(explicit);
        }
        if let Some(index) = self.options.iter().position(|option| option.flag == flag) {
            return Token::Option(index, explicit);
        }
        if arg.starts_with("--") {
            let help = "--help".starts_with(flag);
            let matches = self
                .options
                .iter()
                .enumerate()
                .filter(|(_, option)| option.flag.starts_with(flag));
            let mut names = if help { vec!["--help"] } else { Vec::new() };
            let mut option = None;
            for (index, matched) in matches {
                names.push(&matched.flag);
                option = Some(index);
            }
            if names.len() > 1 {
                return Token::Ambiguous(names);
            }
            if help {
                return Token::Help(explicit);
            }
            if let Some(index) = option {
                return Token::Option(index, explicit);
            }
        } else if let Some(suffix) = arg.strip_prefix("-h") {
            // argparse collects single-dash switches before invoking help.
            let suffix = suffix.trim_start_matches('h');
            return Token::Help(
                suffix
                    .strip_prefix('=')
                    .or_else(|| suffix.starts_with('-').then_some(suffix)),
            );
        }
        let numeric = tail
            .strip_prefix('.')
            .unwrap_or(tail)
            .chars()
            .next()
            .is_some_and(|first| {
                // Python's negative-number matcher starts with -\.?\d.
                GeneralCategory::for_char(first) == GeneralCategory::DecimalNumber
            });
        if numeric || arg.contains(' ') {
            Token::Value
        } else {
            Token::Unknown
        }
    }

    fn parse<'a>(&'a self, args: &[&'a str]) -> Result<Parsed<'a>, Stop<'a>> {
        let mut values = vec![None; self.options.len()];
        let mut extras = Vec::new();
        let mut index = 0;
        let mut selected = None;
        let mut end_options = false;
        while let Some(&arg) = args.get(index) {
            index += 1;
            let (slot, explicit) = match if end_options {
                Token::Value
            } else {
                self.token(arg)
            } {
                Token::Help(None) => return Err(Stop::Help(self)),
                Token::Help(Some(value)) => {
                    return Err(Stop::Error(
                        self,
                        format!(
                            "argument -h/--help: ignored explicit argument {}",
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
                Token::End if self.subcommands.is_some() => {
                    end_options = true;
                    continue;
                }
                Token::Value if let Some(commands) = &self.subcommands => {
                    let Some((child, definition)) = commands
                        .choices
                        .iter()
                        .enumerate()
                        .find(|(_, child)| child.name == arg)
                    else {
                        let choices = commands
                            .choices
                            .iter()
                            .map(|child| string_repr(&child.name))
                            .collect::<Vec<_>>()
                            .join(", ");
                        return Err(Stop::Error(
                            self,
                            format!(
                                "argument {}: invalid choice: {} (choose from {choices})",
                                commands.dest,
                                string_repr(arg)
                            ),
                        ));
                    };
                    let mut parsed = definition.parser.parse(&args[index..])?;
                    extras.append(&mut parsed.extras);
                    selected = Some((child, arg, parsed.words));
                    break;
                }
                Token::End => {
                    extras.extend_from_slice(&args[index - 1..]);
                    break;
                }
                Token::Value | Token::Unknown => {
                    extras.push(arg);
                    continue;
                }
                Token::Option(slot, value) => (slot, value),
            };
            let option = &self.options[slot];
            let taken = if let Some(value) = explicit {
                if option.kind == Kind::Bool {
                    return Err(Stop::Error(
                        self,
                        format!(
                            "argument {}: ignored explicit argument {}",
                            option.flag,
                            string_repr(value)
                        ),
                    ));
                }
                vec![value]
            } else if option.kind == Kind::Bool {
                Vec::new()
            } else {
                let start = index;
                while index < args.len() && matches!(self.token(args[index]), Token::Value) {
                    index += 1;
                    if !option.many {
                        break;
                    }
                }
                if index == start {
                    let message = if option.many {
                        "expected at least one argument"
                    } else {
                        "expected one argument"
                    };
                    return Err(Stop::Error(
                        self,
                        format!("argument {}: {message}", option.flag),
                    ));
                }
                args[start..index].to_vec()
            };
            if !option.choices.is_empty()
                && let Some(value) = taken
                    .iter()
                    .find(|&&value| !option.choices.iter().any(|choice| choice == value))
            {
                let choices = option
                    .choices
                    .iter()
                    .map(|choice| string_repr(choice))
                    .collect::<Vec<_>>()
                    .join(", ");
                return Err(Stop::Error(
                    self,
                    format!(
                        "argument {}: invalid choice: {} (choose from {choices})",
                        option.flag,
                        string_repr(value)
                    ),
                ));
            }
            values[slot] = Some(taken);
        }
        let mut missing = self
            .options
            .iter()
            .zip(&values)
            .filter(|(option, value)| option.required && value.is_none())
            .map(|(option, _)| option.flag.as_str())
            .collect::<Vec<_>>();
        if let Some(commands) = &self.subcommands
            && selected.is_none()
        {
            missing.push(&commands.dest);
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
        let mut words: Vec<_> = self
            .options
            .iter()
            .zip(values)
            .map(|(option, value)| Word::Option(option, value))
            .collect();
        let child = selected.map(|(child, name, mut rest)| {
            words.push(Word::Command(name));
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

fn scalar(kind: Kind, text: &str) -> usize {
    let text = Rc::new(Str::new(text));
    match kind {
        Kind::Str => Rc::into_raw(text) as usize,
        // SAFETY: the source remains live until Path takes an owner or normalizes it.
        Kind::Path => unsafe { relar_path_new(Rc::as_ptr(&text)) as usize },
        Kind::Bool => unreachable!(),
    }
}

impl OptionSpec {
    fn word(&self, args: Option<Vec<&str>>) -> usize {
        if self.kind == Kind::Bool {
            let value = args
                .is_some()
                .then_some(true)
                .or_else(|| self.default.as_deref().map(|value| value == "True"));
            return value.map_or(0, |value| usize::from(value) + usize::from(self.optional));
        }
        if self.many {
            let Some(args) = args else { return 0 };
            let mut values = Words::new(if self.kind == Kind::Path { 5 } else { 3 });
            values
                .values
                .extend(args.into_iter().map(|value| scalar(self.kind, value)));
            return Rc::into_raw(values.into_object()) as usize;
        }
        args.as_ref()
            .and_then(|args| args.first().copied())
            .or(self.default.as_deref())
            .map_or(0, |value| scalar(self.kind, value))
    }
}

/// Parses the current contents of sys.argv[1..] into the fixed namespace layout.
///
/// # Safety
/// The compiler roots the inputs. `layout` and each descriptor in `children`
/// match the inferred fields for that parser. The child table contains one
/// descriptor per declared subcommand in order, or is null for a flat parser.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn relar_argparse_parse(
    layout: *const RecordDescriptor,
    children: *const *const RecordDescriptor,
    schema: *const Str,
    description: *const Str,
    argv: *const Object,
) -> *mut usize {
    // SAFETY: these immutable inputs remain rooted during parsing and construction.
    let (schema, description, argv) = unsafe { (&*schema, &*description, &*argv) };
    let parser: Parser = serde_json::from_str(schema)
        .unwrap_or_else(|_| fail("SystemError", "invalid argparse schema"));
    // SAFETY: argv is a rooted list[str]; take independent string owners before
    // reading the argument values or constructing the output namespace.
    let owners = unsafe {
        argv.0.with(|storage| {
            let sequence = storage.sequence();
            sequence.check_kind(3);
            sequence
                .values
                .iter()
                .skip(1)
                .map(|&word| super::acquire_rc::<Str>(word))
                .collect::<Vec<_>>()
        })
    };
    let args = owners
        .iter()
        .map(|value| value.as_str())
        .collect::<Vec<_>>();
    let parsed = match parser.parse(&args).and_then(|parsed| {
        if parsed.extras.is_empty() {
            Ok(parsed)
        } else {
            Err(Stop::Error(
                &parser,
                format!("unrecognized arguments: {}", parsed.extras.join(" ")),
            ))
        }
    }) {
        Ok(values) => values,
        Err(Stop::Help(active)) => {
            let help = active.help(if std::ptr::eq(active, &parser) {
                description
            } else {
                ""
            });
            if crate::write_output(|writer| {
                writer.write_all(help.trim_end_matches('\n').as_bytes())
            })
            .is_err()
            {
                return Error::new(exceptions::Kind::OSError, "could not write standard output")
                    .returning(std::ptr::null_mut());
            }
            return Error::new(exceptions::Kind::SystemExit, "0").returning(std::ptr::null_mut());
        }
        Err(Stop::Error(active, error)) => {
            let _ = crate::flush_stdout();
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
            return Error::new(exceptions::Kind::SystemExit, "2").returning(std::ptr::null_mut());
        }
    };
    // SAFETY: every slot receives exactly one owner of its declared kind. All
    // input validation is complete, and option conversion invokes no user code.
    unsafe {
        let layout = parsed.child.map_or(layout, |index| *children.add(index));
        let layout_ref = &*layout;
        if layout_ref.length != parsed.words.len() {
            fail("SystemError", "argparse namespace layout mismatch");
        }
        let result = relar_record_new(layout);
        for (index, field) in parsed.words.into_iter().enumerate() {
            let word = match field {
                Word::Option(option, values) => option.word(values),
                Word::Command(value) => scalar(Kind::Str, value),
            };
            let dest = result.add(index);
            match *layout_ref.kinds.add(index) {
                3 | 5 => dest.cast::<*const ()>().write(word as *const ()),
                _ => dest.write(word),
            }
        }
        result
    }
}
