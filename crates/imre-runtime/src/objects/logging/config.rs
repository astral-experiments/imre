//! Apply public dictionary and INI configuration to live native logger objects.

use super::super::exceptions::{Result, push_string_repr, string_repr, write_string_repr};
use super::super::json::readonly::document::Value;
use super::{Error, Int, Kind, LOGGERS, Object, Str, finish, levels, name, object};
use crate::StrOwner;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::fmt::{self, Write};

#[derive(Clone, Copy)]
struct Control {
    disabled: bool,
    propagate: bool,
}

impl Default for Control {
    fn default() -> Self {
        Self {
            disabled: false,
            propagate: true,
        }
    }
}

thread_local! { static CONTROLS: RefCell<HashMap<String, Control>> = RefCell::default(); }

fn control(name: &str) -> Control {
    CONTROLS.with_borrow(|c| c.get(name).copied().unwrap_or_default())
}

/// The root can handle its own records even if root.propagate is false.
/// Re-evaluate parents on each walk: creating a logger can reparent its children.
fn reaches_root(logs: &super::Registry, mut current: &str) -> bool {
    while let Some(parent) = logs.parent_name(current) {
        if !control(current).propagate {
            return false;
        }
        current = parent;
    }
    true
}

/// A disabled logger bypasses the level decision, including its cache.
///
/// # Safety
/// The logger and exact integer remain rooted for the duration of the check.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_log_is_enabled(logger: *const Object, level: usize) -> bool {
    // SAFETY: both borrowed arguments remain live for the synchronous calls.
    unsafe { !imre_log_disabled(logger) && super::imre_log_enabled(logger, level) }
}

/// # Safety
/// The caller roots an exact native logger.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_log_disabled(logger: *const Object) -> bool {
    // SAFETY: the caller roots this logger while its name is borrowed.
    control(&unsafe { name(logger) }).disabled
}

/// # Safety
/// The caller roots an exact native logger.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_log_propagate(logger: *const Object) -> bool {
    // SAFETY: the caller roots this logger while its name is borrowed.
    control(&unsafe { name(logger) }).propagate
}

/// Change the same control that fileConfig, dictConfig and emission observe.
///
/// # Safety
/// The caller roots the logger through the update.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_log_set_propagate(value: bool, logger: *const Object) {
    // SAFETY: the input retains the native logger for this operation.
    let key = unsafe { name(logger) };
    CONTROLS
        .with_borrow_mut(|controls| controls.entry(key.to_string()).or_default().propagate = value);
}

/// Check reachable installed handlers. The fallback handler is not attached
/// to a logger and so never makes hasHandlers() true.
///
/// # Safety
/// The caller roots the logger while its current parent chain is traversed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_log_has_handlers(logger: *const Object) -> bool {
    // SAFETY: the incoming logger stays live for the walk.
    let key = unsafe { name(logger) };
    LOGGERS.with_borrow(|logs| logs.format.is_some() && reaches_root(logs, &key))
}

/// A logger with propagation stopped before root has only the default last-resort handler.
///
/// # Safety
/// Logger, level and message remain rooted through the output operation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_log_routed_emit(
    logger: *const Object,
    level: usize,
    message: *const Str,
) {
    // SAFETY: the caller holds each owner through this synchronous call.
    let logger_name = unsafe { name(logger) };
    if LOGGERS.with_borrow(|logs| reaches_root(logs, &logger_name)) {
        // SAFETY: all three original owners remain live.
        unsafe { super::imre_log_emit(logger, level, message) };
    } else {
        // SAFETY: level and message retain their exact types through the write.
        let (level, message) = unsafe { (Int::borrow(level), &*message) };
        if *level.big() >= 30.into() {
            let _ = crate::sys::write_stderr(message, true);
        }
    }
}

type Pairs = Vec<(StrOwner, Value)>;

fn pairs(mapping: &Value) -> Result<Pairs> {
    match mapping {
        Value::Table(obj) => Ok(Value::items(obj)
            .into_iter()
            .map(|(k, v)| (k, Value::read(&v)))
            .collect()),
        _ => Err(Error::new(
            Kind::TypeError,
            "expected a configuration mapping",
        )),
    }
}

fn get(pairs: &Pairs, key: &str) -> Option<Value> {
    pairs
        .iter()
        .find(|(name, _)| name.as_str() == key)
        .map(|(_, value)| value.clone())
}

fn unsupported(what: &str) -> Error {
    Error::new(
        Kind::NotImplementedError,
        format!("logging configuration does not yet support {what}"),
    )
}

/// Format the rejected version using Python's str rules, including repr of
/// nested strings. User-provided containers must not consume the Rust stack.
fn display(value: Value) -> String {
    if let Value::String(s) = value {
        return s.to_string();
    }
    enum Part {
        Value(Value),
        Text(&'static str),
        Key(StrOwner),
    }
    let mut pending = vec![Part::Value(value)];
    let mut out = String::new();
    while let Some(part) = pending.pop() {
        match part {
            Part::Text(s) => out.push_str(s),
            Part::Key(s) | Part::Value(Value::String(s)) => push_string_repr(&s, &mut out),
            Part::Value(Value::Null) => out.push_str("None"),
            Part::Value(Value::Bool(b)) => out.push_str(if b { "True" } else { "False" }),
            Part::Value(Value::Integer(i)) => out.push_str(&i.big().to_string()),
            Part::Value(Value::Float(f)) => out.push_str(&crate::floats::render(f)),
            Part::Value(Value::Datetime(s)) => out.push_str(&s),
            Part::Value(Value::Array(a)) => {
                out.push('[');
                pending.push(Part::Text("]"));
                for (i, v) in Value::elements(&a).into_iter().enumerate().rev() {
                    pending.push(Part::Value(Value::read(&v)));
                    if i != 0 {
                        pending.push(Part::Text(", "));
                    }
                }
            }
            Part::Value(Value::Table(t)) => {
                out.push('{');
                pending.push(Part::Text("}"));
                for (i, (k, v)) in Value::items(&t).into_iter().enumerate().rev() {
                    pending.push(Part::Value(Value::read(&v)));
                    pending.push(Part::Text(": "));
                    pending.push(Part::Key(k));
                    if i != 0 {
                        pending.push(Part::Text(", "));
                    }
                }
            }
        }
    }
    out
}

fn level(value: &Value) -> Result<Int> {
    match value {
        Value::String(s) => levels::named_level(s),
        Value::Integer(i) => Ok(i.clone()),
        Value::Bool(b) => Ok(Int::small(i64::from(*b)).unwrap()),
        _ => Err(Error::new(
            Kind::TypeError,
            "Level not an integer or a valid string",
        )),
    }
}

/// Look up the existing logger, preserving aliases. All configured level
/// writes use the same setter as public Logger.setLevel.
fn logger_level(name: &str, level: Option<Int>) {
    let key = Str::from(name);
    // SAFETY: the string is live through lookup and each result transfers an owner.
    let logger =
        unsafe { crate::ObjectOwner::from_raw(super::imre_log_get(&key as *const Str as usize)) };
    if let Some(level) = level {
        // SAFETY: both owners remain in this scope until after the level write.
        unsafe { super::imre_log_set_level(crate::ObjectOwner::as_ptr(&logger), level.0) };
    }
}

fn configure_logger(name: &str, spec: &Value, incremental: bool, root: bool) -> Result<()> {
    let cfg = pairs(spec)?;
    let value = get(&cfg, "level");
    let threshold = value
        .as_ref()
        .filter(|v| !matches!(v, Value::Null))
        .map(level)
        .transpose()?;
    logger_level(name, threshold);
    if !incremental {
        for key in ["handlers", "filters"] {
            if get(&cfg, key).is_some_and(|v| v.truth()) {
                return Err(unsupported(key));
            }
        }
        if root {
            LOGGERS.with_borrow_mut(|logs| {
                logs.format = None;
                logs.datefmt = None;
            });
        }
    }
    if !root {
        CONTROLS.with_borrow_mut(|controls| {
            let entry = controls.entry(name.to_owned()).or_default();
            entry.disabled = false;
            if let Some(v) = get(&cfg, "propagate").filter(|v| !matches!(v, Value::Null)) {
                entry.propagate = v.truth();
            }
        });
    }
    Ok(())
}

fn previous_loggers(configured: &HashSet<String>, old: &[String], disable: bool) {
    for name in old {
        if configured.contains(name) {
            continue;
        }
        if name
            .match_indices('.')
            .any(|(end, _)| configured.contains(&name[..end]))
        {
            logger_level(name, Int::small(0));
            CONTROLS.with_borrow_mut(|map| map.entry(name.clone()).or_default().propagate = true);
        } else {
            CONTROLS.with_borrow_mut(|map| map.entry(name.clone()).or_default().disabled = disable);
        }
    }
}

fn configure(cfg: Pairs) -> Result<()> {
    let version = get(&cfg, "version")
        .ok_or_else(|| Error::new(Kind::ValueError, "dictionary doesn't specify a version"))?;
    if !(matches!(&version, Value::Integer(i) if i.as_small() == Some(1))
        || matches!(&version, Value::Bool(true) | Value::Float(1.0)))
    {
        return Err(Error::new(
            Kind::ValueError,
            format!("Unsupported version: {}", display(version)),
        ));
    }
    let incremental = get(&cfg, "incremental").is_some_and(|v| v.truth());
    if let Some(v) = get(&cfg, "handlers")
        && v.truth()
    {
        if incremental && let Some((name, _)) = pairs(&v)?.first() {
            return Err(Error::new(
                Kind::ValueError,
                format!("No handler found with name {}", string_repr(name)),
            ));
        }
        return Err(unsupported("handler construction"));
    }
    if !incremental {
        for section in ["formatters", "filters"] {
            if get(&cfg, section).is_some_and(|v| v.truth()) {
                return Err(unsupported(section));
            }
        }
    }
    let old = LOGGERS.with_borrow(|r| {
        r.entries
            .keys()
            .filter(|n| *n != "root")
            .cloned()
            .collect::<Vec<_>>()
    });
    let mut configured = HashSet::new();
    if let Some(loggers) = get(&cfg, "loggers") {
        for (name, spec) in pairs(&loggers)? {
            configure_logger(&name, &spec, incremental, false).map_err(|err| {
                if err.is_kind(Kind::NotImplementedError) {
                    return err;
                }
                Error::new(
                    Kind::ValueError,
                    format!("Unable to configure logger {}", string_repr(&name)),
                )
                .with_cause(err)
            })?;
            configured.insert(name.to_string());
        }
    }
    if !incremental {
        previous_loggers(
            &configured,
            &old,
            get(&cfg, "disable_existing_loggers").is_none_or(|v| v.truth()),
        );
    }
    if let Some(root) = get(&cfg, "root")
        && root.truth()
    {
        configure_logger("root", &root, incremental, true).map_err(|err| {
            if err.is_kind(Kind::NotImplementedError) {
                err
            } else {
                Error::new(Kind::ValueError, "Unable to configure root logger").with_cause(err)
            }
        })?;
    }
    Ok(())
}

/// # Safety
/// The compiler supplies an owned document map for the duration of the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_log_dict_config(configuration: *const Object) {
    // SAFETY: the caller roots an exact document until configuration completes.
    let value = unsafe { Value::read(object(configuration)) };
    finish(pairs(&value).and_then(configure), ());
}

/// # Safety
/// The caller roots a string-keyed dictionary whose homogeneous scalar values
/// match the native kind. Shallow reads acquire owners before configuration.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_log_typed_dict_config(map: *const Object, kind: usize) {
    // SAFETY: the compiler supplies the scalar kind as a tagged small integer.
    let pairs = unsafe {
        let kind = Int::borrow(kind).as_small().unwrap();
        object(map).0.with(|s| {
            s.dictionary()
                .string_entries()
                .map(|(k, word)| {
                    let value = match kind {
                        0 => Value::Null,
                        1 => Value::Bool(word != 0),
                        2 => Value::Integer((*Int::borrow(word)).clone()),
                        3 => Value::String(crate::StrOwner::acquire_word(word)),
                        7 => Value::Float(f64::from_bits(word as u64)),
                        _ => unreachable!("compiler checked the configuration value kind"),
                    };
                    (StrOwner::new(k.clone()), value)
                })
                .collect()
        })
    };
    finish(configure(pairs), ());
}

struct Quoted<'a>(&'a str);

impl fmt::Display for Quoted<'_> {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        write_string_repr(self.0, out)
    }
}

#[derive(Default)]
struct IniDiagnostic(String);

impl Write for IniDiagnostic {
    fn write_str(&mut self, value: &str) -> fmt::Result {
        self.0.try_reserve(value.len()).map_err(|_| fmt::Error)?;
        self.0.push_str(value);
        Ok(())
    }
}

fn ini_error(args: fmt::Arguments<'_>) -> Error {
    let mut message = IniDiagnostic::default();
    match message.write_fmt(args) {
        Ok(()) => Error::new(Kind::RuntimeError, message.0),
        Err(_) => Error::allocation(),
    }
}

type Sections<'a> = HashMap<&'a str, HashMap<Str, Str>>;

fn ini<'a>(source: &'a str, filename: &str) -> Result<Sections<'a>> {
    let mut sections = Sections::new();
    let mut section = "";
    let mut last = None::<(Str, usize)>;
    let mut blank_lines = 0;
    let mut errors = IniDiagnostic::default();
    for (i, raw) in source.split_inclusive('\n').enumerate() {
        let text = raw.trim();
        if text.is_empty() {
            blank_lines += 1;
            continue;
        }
        if text.starts_with(['#', ';']) {
            continue;
        }
        let indent = raw.len() - raw.trim_start().len();
        if let Some((key, previous_indent)) = &last
            && indent > *previous_indent
        {
            let option = sections.get_mut(section).unwrap().get_mut(key).unwrap();
            option
                .try_reserve(text.len() + blank_lines + 1)
                .map_err(|_| Error::allocation())?;
            option.extend(std::iter::repeat_n('\n', blank_lines + 1));
            blank_lines = 0;
            option.push_str(text);
            continue;
        }
        blank_lines = 0;
        if let Some(header) = text
            .strip_prefix('[')
            .and_then(|s| s.rsplit_once(']'))
            .map(|(h, _)| h)
        {
            if sections.contains_key(header) {
                return Err(ini_error(format_args!(
                    "{filename} is invalid: While reading from {} [line {:2}]: section {} already exists",
                    Quoted(filename),
                    i + 1,
                    Quoted(header)
                )));
            }
            sections.try_reserve(1).map_err(|_| Error::allocation())?;
            section = header;
            sections.insert(section, HashMap::new());
            last = None;
            continue;
        }
        if section.is_empty() {
            return Err(ini_error(format_args!(
                "{filename} is invalid: File contains no section headers.\nfile: {}, line: {}\n{}",
                Quoted(filename),
                i + 1,
                Quoted(raw)
            )));
        }
        if let Some(pos) = text.find(['=', ':'])
            && pos != 0
        {
            let key = super::super::text::lowercase(text[..pos].trim_end())
                .map_err(|_| Error::allocation())?;
            let options = sections.get_mut(section).unwrap();
            if options.contains_key(&key) {
                return Err(ini_error(format_args!(
                    "{filename} is invalid: While reading from {} [line {:2}]: option {} in section {} already exists",
                    Quoted(filename),
                    i + 1,
                    Quoted(&key),
                    Quoted(section)
                )));
            }
            options.try_reserve(1).map_err(|_| Error::allocation())?;
            let retained = Str::try_new(key.as_str()).map_err(|_| Error::allocation())?;
            let value =
                Str::try_new(text[pos + 1..].trim_start()).map_err(|_| Error::allocation())?;
            options.insert(key, value);
            last = Some((retained, indent));
        } else {
            if errors.0.is_empty() {
                write!(
                    errors,
                    "{filename} is invalid: Source contains parsing errors: {}",
                    Quoted(filename)
                )
                .map_err(|_| Error::allocation())?;
            }
            write!(errors, "\n\t[line {:2}]: {}", i + 1, Quoted(raw))
                .map_err(|_| Error::allocation())?;
            last = None;
        }
    }
    if !errors.0.is_empty() {
        return Err(Error::new(Kind::RuntimeError, errors.0));
    }
    Ok(sections)
}

fn file_config(path: &str, disable: bool, encoding: Option<&Str>) -> Result<()> {
    let meta = std::fs::metadata(path)
        .map_err(|_| Error::new(Kind::FileNotFoundError, format!("{path} doesn't exist")))?;
    if meta.len() == 0 {
        return Err(Error::new(
            Kind::RuntimeError,
            format!("{path} is an empty file"),
        ));
    }
    let bytes = std::fs::read(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::OutOfMemory && error.raw_os_error().is_none() {
            Error::allocation()
        } else {
            Error::string_key(Str::from("formatters"))
        }
    })?;
    let encoding = encoding
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_else(|| "utf-8".into());
    let source = match encoding.as_str() {
        "utf-8" | "utf8" | "utf_8" => String::from_utf8(bytes).map_err(|err| {
            let reason = err.utf8_error();
            super::super::bytes::decode_owned_error(
                crate::BytesOwner::new(err.into_bytes()),
                reason,
            )
        })?,
        "latin-1" | "latin1" | "iso-8859-1" => {
            let capacity = bytes.len() + bytes.iter().filter(|b| !b.is_ascii()).count();
            let mut text = String::new();
            text.try_reserve_exact(capacity)
                .map_err(|_| Error::allocation())?;
            text.extend(bytes.into_iter().map(char::from));
            text
        }
        _ => return Err(unsupported(&format!("file encoding {encoding}"))),
    };
    let source = if source.contains('\r') {
        let mut bytes = source.into_bytes();
        let mut previous_cr = false;
        bytes.retain_mut(|byte| {
            let skip = previous_cr && *byte == b'\n';
            previous_cr = *byte == b'\r';
            if previous_cr {
                *byte = b'\n';
            }
            !skip
        });
        String::from_utf8(bytes).expect("normalizing ASCII line endings preserves UTF-8")
    } else {
        source
    };
    let sections = ini(&source, path)?;
    let section = |name: &str| {
        sections
            .get(name)
            .ok_or_else(|| Error::string_key(Str::from(name)))
    };
    let option = |name: &str, key: &str| -> Result<&str> {
        section(name)?
            .get(key)
            .or_else(|| sections.get("DEFAULT").and_then(|d| d.get(key)))
            .map(Str::as_str)
            .ok_or_else(|| Error::string_key(Str::from(key)))
    };
    for group in ["formatters", "handlers"] {
        if !option(group, "keys")?.trim().is_empty() {
            return Err(unsupported(group));
        }
    }
    let names: Vec<_> = option("loggers", "keys")?
        .split(',')
        .map(str::trim)
        .collect();
    if !names.contains(&"root") {
        return Err(Error::new(
            Kind::ValueError,
            "list.remove(x): x not in list",
        ));
    }
    if let Some(level) = section("logger_root")?.get("level") {
        logger_level(
            "root",
            Some(levels::named_level(&Str::from(level.as_str()))?),
        );
    }
    if !option("logger_root", "handlers")?.is_empty() {
        return Err(unsupported("root handlers"));
    }
    LOGGERS.with_borrow_mut(|r| {
        r.format = None;
        r.datefmt = None;
    });
    let old = LOGGERS.with_borrow(|r| {
        r.entries
            .keys()
            .filter(|n| *n != "root")
            .cloned()
            .collect::<Vec<_>>()
    });
    let mut configured = HashSet::new();
    for key in names.into_iter().filter(|k| *k != "root") {
        let section_name = format!("logger_{key}");
        let cfg = section(&section_name)?;
        let name = option(&section_name, "qualname")?;
        let threshold = cfg
            .get("level")
            .map(|s| levels::named_level(&Str::from(s.as_str())))
            .transpose()?;
        logger_level(name, threshold);
        if !option(&section_name, "handlers")?.is_empty() {
            return Err(unsupported("named handlers"));
        }
        let propagate = cfg
            .get("propagate")
            .map(|x| {
                x.parse::<i64>().map(|v| v != 0).map_err(|_| {
                    Error::new(
                        Kind::ValueError,
                        format!("invalid literal for int() with base 10: {}", string_repr(x)),
                    )
                })
            })
            .transpose()?
            .unwrap_or(true);
        CONTROLS.with_borrow_mut(|m| {
            m.insert(
                name.to_owned(),
                Control {
                    disabled: false,
                    propagate,
                },
            );
        });
        configured.insert(name.to_owned());
    }
    previous_loggers(&configured, &old, disable);
    Ok(())
}

/// # Safety
/// The compiler roots the path and optional encoding string for this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_log_file_config(
    path: *const Str,
    _defaults: u8,
    disable: bool,
    encoding: usize,
) {
    // SAFETY: nonzero encoding is the exact string payload of the optional word.
    let (path, encoding) = unsafe { (&*path, (encoding as *const Str).as_ref()) };
    finish(file_config(path, disable, encoding), ());
}

#[test]
fn ini_preserves_empty_lines_inside_multiline_values() {
    let sections = ini(
        "[logger]\nname=alpha\n\n# comment\n   \n    beta\n\nafter=next\n",
        "test.ini",
    )
    .unwrap_or_else(|_| panic!("valid INI fixture should parse"));
    assert_eq!(sections["logger"]["name"].as_str(), "alpha\n\n\nbeta");
}
