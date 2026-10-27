//! Native operating-system errors and Python-constructed OSError payloads.

use super::builtins;
use super::operators::append_rendered_text as append;
use super::value::{Error, Id, NONE, Result, Value};
use super::{Context, copy_ids};
use crate::objects::exceptions::OsErrorArgs;
use crate::shared::Shared;

#[derive(Clone, Debug)]
pub(super) enum Args {
    Native {
        source: OsErrorArgs,
        // Cache the four boxed fields while keeping the original native diagnostic intact.
        fields: std::cell::Cell<Option<Id>>,
    },
    Python(Fields),
}

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Fields {
    errno: Option<Id>,
    strerror: Option<Id>,
    filename: Option<Id>,
    filename2: Option<Id>,
    written: Option<i64>,
}

#[derive(Clone, Copy)]
pub(super) enum Fragment {
    Text(&'static str),
    Value(Id, bool),
}

impl Fields {
    /// Snapshot the values used by OSError.__str__ before the first user conversion.
    pub(super) fn fragments(self) -> Option<[Option<Fragment>; 8]> {
        use Fragment::{Text, Value};
        if self.filename.is_none() && (self.errno.is_none() || self.strerror.is_none()) {
            return None;
        }
        let second = self.filename.and(self.filename2);
        Some([
            Some(Text("[Errno ")),
            Some(Value(self.errno.unwrap_or(NONE), false)),
            Some(Text("] ")),
            Some(Value(self.strerror.unwrap_or(NONE), false)),
            self.filename.map(|_| Text(": ")),
            self.filename.map(|id| Value(id, true)),
            second.map(|_| Text(" -> ")),
            second.map(|id| Value(id, true)),
        ])
    }
}

impl Args {
    pub(super) fn new(value: Self) -> Result<Shared<Self>> {
        Shared::try_new(value).map_err(|_| Error::new("MemoryError", ""))
    }

    pub(super) fn roots(&self) -> impl Iterator<Item = Id> {
        let fields = match self {
            Self::Native { fields, .. } => {
                return [fields.get(), None, None, None].into_iter().flatten();
            }
            Self::Python(fields) => *fields,
        };
        [
            fields.errno,
            fields.strerror,
            fields.filename,
            fields.filename2,
        ]
        .into_iter()
        .flatten()
    }
}

fn subclass(code: i64) -> Option<&'static str> {
    crate::objects::errno_entries().find_map(|(name, value)| {
        if i64::from(value) != code {
            return None;
        }
        Some(match name {
            "EAGAIN" | "EALREADY" | "EINPROGRESS" | "EWOULDBLOCK" => "BlockingIOError",
            "EPIPE" | "ESHUTDOWN" => "BrokenPipeError",
            "ECHILD" => "ChildProcessError",
            "ECONNABORTED" => "ConnectionAbortedError",
            "ECONNREFUSED" => "ConnectionRefusedError",
            "ECONNRESET" => "ConnectionResetError",
            "EEXIST" => "FileExistsError",
            "ENOENT" => "FileNotFoundError",
            "EISDIR" => "IsADirectoryError",
            "ENOTDIR" => "NotADirectoryError",
            "EINTR" => "InterruptedError",
            "EACCES" | "EPERM" | "ENOTCAPABLE" => "PermissionError",
            "ESRCH" => "ProcessLookupError",
            "ETIMEDOUT" => "TimeoutError",
            _ => return None,
        })
    })
}

impl Context {
    fn written_count(&mut self, value: Id) -> Result<Option<i64>> {
        let count = self.index_integer(value)?.to_i64().ok_or_else(|| {
            Error::new(
                "ValueError",
                format!(
                    "cannot fit '{}' into an index-sized integer",
                    self.type_name(value)
                ),
            )
        })?;
        Ok((count != -1).then_some(count))
    }

    fn os_subclass(&mut self, code: Id) -> Result<Option<&'static str>> {
        let scalar = self.scalar_value(code);
        if !matches!(self.heap.get(scalar), Value::Int(_) | Value::Bool(_)) {
            return Ok(None);
        }
        if scalar == code {
            return Ok(self.as_int(code)?.to_i64().and_then(subclass));
        }
        // The errno map has distinct small integer keys. A subclass can change its hash or equality.
        let hash = self.dictionary_hash(code).map_err(|error| {
            if error.class.is_none() {
                self.key_hash_error(code, "dict key", error)
            } else {
                error
            }
        })?;
        let Some(kind) = subclass(hash) else {
            return Ok(None);
        };
        let key = self.int(hash);
        Ok(self.checked_equal(key, code)?.then_some(kind))
    }

    /// A Python initializer inherits parsing from OSError.__init__ only with OSError's allocator.
    pub(super) fn deferred_os_init(&self, class: Id) -> bool {
        let python_slot = |name: &str| {
            self.find_in_mro(class, |base| {
                matches!(self.heap.get(base), Value::Class { .. })
                    .then(|| self.own_class_attr(base, name))
                    .flatten()
            })
            .is_some()
        };
        python_slot("__init__") && !python_slot("__new__")
    }

    pub(super) fn os_exception(
        &mut self,
        kind: &str,
        class: Option<Id>,
        args: &[Id],
        prior: Option<&Args>,
    ) -> Result<Error> {
        let mut kind = kind;
        let mut exposed = args;
        // Reinitialization replaces errno and strerror; omitted paths and the write count survive.
        let mut fields = match prior {
            Some(Args::Python(old)) => Fields {
                filename: old.filename,
                filename2: old.filename2,
                written: old.written,
                ..Fields::default()
            },
            _ => Fields::default(),
        };
        if (2..=5).contains(&args.len()) {
            let [errno, description] = [args[0], args[1]];
            if kind == "OSError" && class.is_none() {
                kind = self.os_subclass(errno)?.unwrap_or(kind);
            }
            fields.errno = Some(errno);
            fields.strerror = Some(description);
            if let Some(&filename) = args.get(2).filter(|&&value| value != NONE) {
                let numeric = matches!(
                    self.heap.get(self.scalar_value(filename)),
                    Value::Int(_) | Value::Bool(_) | Value::Float(_) | Value::Complex(_)
                ) || ["__index__", "__int__", "__float__"]
                    .iter()
                    .any(|slot| self.slot(filename, slot).is_some());
                if kind == "BlockingIOError" && class.is_none() && numeric {
                    fields.written = self.written_count(filename)?;
                } else {
                    fields.filename = Some(filename);
                    if let Some(&path) = args.get(4).filter(|&&value| value != NONE) {
                        fields.filename2 = Some(path);
                    }
                    exposed = &args[..2];
                }
            }
        }
        let mut error = Error::new("", "");
        append(&mut error.kind, kind)?;
        error.class = class;
        error.args = Some(self.alloc(Value::Tuple(copy_ids(exposed)?)));
        if fields.errno.is_some()
            || fields.filename.is_some()
            || fields.filename2.is_some()
            || fields.written.is_some()
        {
            error.os_args = Some(Args::new(Args::Python(fields))?);
        }
        Ok(error)
    }

    /// Render Python-owned fields only when printing an uncaught exception.
    pub(super) fn report_os_error(&mut self, error: Error) {
        let current = error
            .trace
            .as_ref()
            .and_then(|trace| trace.instance)
            .filter(|&id| matches!(self.heap.get(id), Value::Raised(_)));
        let (id, mut traced) = match current {
            Some(id) => (id, Some(error)),
            None => (self.raised_instance(error), None),
        };
        self.heap.keep(id);
        let mut message = String::new();
        let result = builtins::call(self, "str", &[id], &[])
            .and_then(|value| append(&mut message, self.str_value(value)?));
        let result = if result.is_ok() {
            message
        } else {
            "<exception str() failed>".to_owned()
        };
        let error = match traced.as_mut() {
            Some(original) => original,
            None => match self.heap.get_mut(id) {
                Value::Raised(error) => error,
                _ => unreachable!(),
            },
        };
        error.message = result;
        eprintln!("{error}");
    }

    pub(super) fn is_os_error(&self, error: &Error) -> bool {
        if error.os_args.is_some() {
            return true;
        }
        match error.class {
            Some(class) => self
                .find_in_mro(class, |base| match self.heap.get(base) {
                    Value::Native(name)
                        if builtins::exception_subclass(name, "OSError") == Some(true) =>
                    {
                        Some(())
                    }
                    _ => None,
                })
                .is_some(),
            None => builtins::exception_subclass(&error.kind, "OSError") == Some(true),
        }
    }

    fn native_os_fields(
        &mut self,
        source: &OsErrorArgs,
        saved: &std::cell::Cell<Option<Id>>,
    ) -> Result<Fields> {
        let tuple = match saved.get() {
            Some(tuple) => tuple,
            None => {
                let errno = self.int(source.errno);
                let mut string = |value: &str| -> Result<Id> {
                    let mut owned = String::new();
                    append(&mut owned, value)?;
                    Ok(self.text(owned))
                };
                let message = string(&source.description)?;
                let paths = source.text_paths.as_ref();
                let first = paths
                    .and_then(|p| p[0].as_deref())
                    .map(&mut string)
                    .transpose()?
                    .unwrap_or(NONE);
                let second = paths
                    .and_then(|p| p[1].as_deref())
                    .map(string)
                    .transpose()?
                    .unwrap_or(NONE);
                let tuple = self.alloc(Value::Tuple(copy_ids(&[errno, message, first, second])?));
                saved.set(Some(tuple));
                tuple
            }
        };
        let Value::Tuple(values) = self.heap.get(tuple) else {
            unreachable!()
        };
        Ok(Fields {
            errno: Some(values[0]),
            strerror: Some(values[1]),
            filename: (values[2] != NONE).then_some(values[2]),
            filename2: (values[3] != NONE).then_some(values[3]),
            written: None,
        })
    }

    pub(super) fn os_attribute(&mut self, object: Id, name: &str) -> Result<Option<Id>> {
        if !matches!(
            name,
            "args" | "errno" | "strerror" | "filename" | "filename2" | "characters_written"
        ) {
            return Ok(None);
        }
        let Value::Raised(error) = self.heap.get(object) else {
            return Ok(None);
        };
        if !self.is_os_error(error) {
            return Ok(None);
        }
        if name == "args"
            && let Some(tuple) = error.args
        {
            return Ok(Some(tuple));
        }
        let data = error.os_args.clone();
        let f = match data.as_deref() {
            Some(Args::Native { source, fields }) => self.native_os_fields(source, fields)?,
            Some(Args::Python(fields)) => *fields,
            None => Fields::default(),
        };
        if name == "args" {
            let (Some(errno), Some(description)) = (f.errno, f.strerror) else {
                return Ok(None);
            };
            let tuple = self.alloc(Value::Tuple(copy_ids(&[errno, description])?));
            let Value::Raised(error) = self.heap.get_mut(object) else {
                unreachable!()
            };
            error.args = Some(tuple);
            return Ok(Some(tuple));
        }
        Ok(Some(match name {
            "errno" => f.errno.unwrap_or(NONE),
            "strerror" => f.strerror.unwrap_or(NONE),
            "filename" => f.filename.unwrap_or(NONE),
            "filename2" => f.filename2.unwrap_or(NONE),
            _ => self.int(
                f.written
                    .ok_or_else(|| Error::new("AttributeError", "characters_written"))?,
            ),
        }))
    }

    pub(super) fn update_os_field(
        &mut self,
        object: Id,
        name: &str,
        value: Option<Id>,
    ) -> Result<bool> {
        if !matches!(
            name,
            "errno" | "strerror" | "filename" | "filename2" | "characters_written"
        ) {
            return Ok(false);
        }
        let Value::Raised(error) = self.heap.get(object) else {
            return Ok(false);
        };
        if !self.is_os_error(error) {
            return Ok(false);
        }
        // __index__ may change this same exception and collect its previous fields.
        let written = if name == "characters_written"
            && let Some(id) = value
        {
            self.heap.keep(object);
            self.written_count(id)?
        } else {
            None
        };
        let Value::Raised(error) = self.heap.get(object) else {
            unreachable!()
        };
        let needs_args = error.args.is_none();
        let data = error.os_args.clone();
        let mut fields = match data.as_deref() {
            Some(Args::Native { source, fields }) => self.native_os_fields(source, fields)?,
            Some(Args::Python(fields)) => *fields,
            None => Fields::default(),
        };
        let native_args = if needs_args && matches!(data.as_deref(), Some(Args::Native { .. })) {
            Some(self.alloc(Value::Tuple(copy_ids(&[
                fields.errno.unwrap(),
                fields.strerror.unwrap(),
            ])?)))
        } else {
            None
        };
        match name {
            "errno" => fields.errno = value,
            "strerror" => fields.strerror = value,
            "filename" => fields.filename = value,
            "filename2" => fields.filename2 = value,
            _ => {
                fields.written = match value {
                    Some(_) => written,
                    None if fields.written.is_some() => None,
                    None => return Err(Error::new("AttributeError", "characters_written")),
                }
            }
        }
        let state = Args::new(Args::Python(fields))?;
        let Value::Raised(error) = self.heap.get_mut(object) else {
            unreachable!()
        };
        if error.args.is_none() && native_args.is_some() {
            error.args = native_args;
        }
        error.os_args = Some(state);
        Ok(true)
    }

    fn os_fragments(&self, object: Id) -> Option<[Option<Fragment>; 8]> {
        let Value::Raised(error) = self.heap.get(object) else {
            return None;
        };
        if !self.is_os_error(error)
            || error
                .class
                .is_some_and(|class| self.class_attr(class, "__str__").is_some())
        {
            return None;
        }
        match error.os_args.as_deref() {
            Some(Args::Python(fields)) => {
                if let Some(parts) = fields.fragments() {
                    return Some(parts);
                }
            }
            Some(Args::Native { .. }) => return None,
            None => {}
        }
        let tuple = error.args?;
        let Value::Tuple(args) = self.heap.get(tuple) else {
            unreachable!()
        };
        let mut parts = [None; 8];
        parts[0] = match args.as_slice() {
            [] => None,
            [value] => Some(Fragment::Value(*value, false)),
            _ => Some(Fragment::Value(tuple, false)),
        };
        Some(parts)
    }

    fn keep_os_fragments(&mut self, object: Id, parts: &[Option<Fragment>; 8]) -> Result<()> {
        let values = parts.iter().filter_map(|part| match part {
            Some(Fragment::Value(id, _)) => Some(*id),
            _ => None,
        });
        self.heap.reserve_temporaries(1 + values.clone().count())?;
        self.heap.keep(object);
        for value in values {
            self.heap.keep(value);
        }
        Ok(())
    }

    pub(super) fn checked_os_text(&mut self, object: Id) -> Result<Option<Id>> {
        let Some(parts) = self.os_fragments(object) else {
            return Ok(None);
        };
        // The pinned x86-64 Linux interpreter uses 528 bytes for a nested OSError str
        // and keeps two 2,048-word native-stack margins before raising RecursionError.
        let budget = stacker::remaining_stack().map_or(1000, |bytes| {
            (bytes / size_of::<usize>()).saturating_sub(2 * 2048) / 66
        });
        let mark = self.heap.temporary_mark();
        let result = stacker::maybe_grow(1024 * 1024, 4 * 1024 * 1024, || {
            self.keep_os_fragments(object, &parts)?;
            // BaseException.__str__ returns the single argument's str result itself.
            let mut forward = matches!(parts[0], Some(Fragment::Value(_, false)));
            let mut parts = parts.into_iter().flatten();
            let mut parents = Vec::new();
            let mut text = String::new();
            loop {
                match parts.next() {
                    Some(Fragment::Text(literal)) => append(&mut text, literal)?,
                    Some(Fragment::Value(id, false)) if let Some(child) = self.os_fragments(id) => {
                        if parents.len() + 1 >= budget {
                            return Err(Error::new(
                                "RecursionError",
                                "maximum recursion depth exceeded while getting the str of an object",
                            ));
                        }
                        parents
                            .try_reserve(1)
                            .map_err(|_| Error::new("MemoryError", ""))?;
                        let mark = self.heap.temporary_mark();
                        self.keep_os_fragments(id, &child)?;
                        parents.push((parts, mark));
                        forward &= matches!(child[0], Some(Fragment::Value(_, false)));
                        parts = child.into_iter().flatten();
                    }
                    Some(Fragment::Value(id, repr)) => {
                        let mark = self.heap.temporary_mark();
                        let value =
                            builtins::call(self, if repr { "repr" } else { "str" }, &[id], &[])?;
                        if forward {
                            return Ok(value);
                        }
                        append(&mut text, self.str_value(value)?)?;
                        self.heap.release_temporaries_since(mark);
                    }
                    None => match parents.pop() {
                        Some((parent, mark)) => {
                            self.heap.release_temporaries_since(mark);
                            parts = parent;
                        }
                        None => return Ok(self.text(text)),
                    },
                }
            }
        });
        self.heap.release_temporaries_since(mark);
        let value = result?;
        self.heap.keep(value);
        Ok(Some(value))
    }
}
