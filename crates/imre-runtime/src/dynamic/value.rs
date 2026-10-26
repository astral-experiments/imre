//! Values shared by native functions loading Python modules through `sys.path`.

use std::collections::HashMap;
use std::fmt;

use super::Context;
use crate::objects::exceptions::ExitStatus;

pub(super) type Id = u64;
pub(super) type Code = unsafe extern "C" fn(*mut Context, Id) -> Id;
pub(super) const NONE: Id = 0;
pub(super) const FALSE: Id = 1;
pub(super) const TRUE: Id = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Location {
    pub file: &'static str,
    pub line: u32,
}

#[derive(Clone, Debug)]
pub(super) struct Error {
    pub kind: String,
    pub message: String,
    pub args: Option<Vec<Id>>,
    pub trace: Vec<Location>,
    pub exit_status: Option<ExitStatus>,
}

impl Error {
    pub fn new(kind: &str, message: impl Into<String>) -> Self {
        Self {
            kind: kind.to_owned(),
            message: message.into(),
            args: None,
            trace: Vec::new(),
            exit_status: None,
        }
    }
}

impl Context {
    pub(super) fn exception(&mut self, kind: &str, args: &[Id]) -> Error {
        let message = match args {
            [] => String::new(),
            [one] => self.display(*one),
            many => {
                let tuple = self.alloc(Value::Tuple(many.to_vec()));
                self.display(tuple)
            }
        };
        let mut error = Error::new(kind, message);
        error.args = Some(args.to_vec());
        if kind == "SystemExit" {
            error.exit_status = match args {
                [] | [NONE] => Some(ExitStatus::Code(0)),
                [code] => match self.heap.get(*code) {
                    Value::Int(i) => Some(ExitStatus::from_i64(i.to_i64())),
                    Value::Bool(b) => Some(ExitStatus::Code(i32::from(*b))),
                    _ => None,
                },
                _ => None,
            };
        }
        error
    }
}

impl From<crate::Error> for Error {
    fn from(error: crate::Error) -> Self {
        let (kind, message) = error.into_parts();
        match message {
            Ok(message) => Self::new(kind, message.to_string()),
            Err(_) => Self::new("MemoryError", String::new()),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if !self.trace.is_empty() {
            writeln!(f, "Traceback (most recent call last):")?;
            for loc in self.trace.iter().rev() {
                writeln!(f, "  File {:?}, line {}", loc.file, loc.line)?;
            }
        }
        write!(f, "{}: {}", self.kind, self.message)
    }
}

pub(super) type Result<T> = std::result::Result<T, Error>;

#[derive(Clone)]
pub(super) struct Scope {
    pub class_body: Option<Vec<String>>,
    pub names: HashMap<String, Id>,
    pub parent: Option<Id>,
    pub module: String,
}

#[derive(Clone)]
pub(super) enum Value {
    None,
    Bool(bool),
    Int(crate::Int),
    Range {
        start: i64,
        stop: i64,
        step: i64,
    },
    Str(String),
    Bytes(Vec<u8>),
    ByteArray(Vec<u8>),
    Hash(std::rc::Rc<std::cell::RefCell<crate::objects::HashState>>),
    List(Vec<Id>),
    Tuple(Vec<Id>),
    NamedTuple {
        names: Vec<String>,
        values: Vec<Id>,
    },
    Set(Vec<Id>),
    Dict(Vec<(Id, Id)>),
    DictView {
        dict: Id,
        kind: DictViewKind,
    },
    DefaultDict {
        factory: Id,
        entries: Vec<(Id, Id)>,
    },
    Scope(Scope),
    Module {
        name: String,
        scope: Id,
    },
    Function {
        name: String,
        code: Code,
        scope: Id,
        parameters: Id,
        defaults: Id,
    },
    Class {
        name: String,
        scope: Id,
        bases: Vec<Id>,
    },
    Instance {
        class: Id,
        fields: HashMap<String, Id>,
    },
    Native(String),
    IoMethod {
        owner: Id,
        name: &'static str,
    },
    Raised(Error),
    Method {
        callable: Id,
        receiver: Id,
    },
    Property(Id),
    Cached {
        function: Id,
        entries: Vec<CachedCall>,
    },
    Iterator {
        values: Vec<Id>,
        offset: usize,
    },
    File(std::rc::Rc<std::cell::RefCell<Option<std::io::BufReader<std::fs::File>>>>),
    RegexPattern(std::rc::Rc<regex_automata::meta::Regex>),
    RegexMatch {
        groups: Vec<Option<String>>,
        names: HashMap<String, usize>,
    },
    Context(std::rc::Rc<HashMap<Id, Id>>),
    ContextVar {
        name: Id,
        default: Option<Id>,
        repr_active: std::cell::Cell<bool>,
    },
    ContextToken {
        context: Id,
        variable: Id,
        old: Option<Id>,
        used: bool,
    },
    TokenMissing,
}

#[derive(Clone)]
pub(super) struct CachedCall {
    pub args: Vec<Id>,
    pub kwargs: Vec<(String, Id)>,
    pub result: Id,
}

impl Value {
    fn children(&self, visit: &mut Vec<Id>) {
        match self {
            Self::List(v)
            | Self::Tuple(v)
            | Self::Set(v)
            | Self::NamedTuple { values: v, .. }
            | Self::Iterator { values: v, .. } => visit.extend(v),
            Self::Dict(entries) => {
                for (key, value) in entries {
                    visit.extend([*key, *value]);
                }
            }
            Self::DictView { dict, .. } => visit.push(*dict),
            Self::Context(bindings) => {
                for (key, value) in bindings.iter() {
                    visit.extend([*key, *value]);
                }
            }
            Self::ContextVar { name, default, .. } => {
                visit.push(*name);
                visit.extend(default);
            }
            Self::ContextToken {
                context,
                variable,
                old,
                ..
            } => {
                visit.extend([*context, *variable]);
                visit.extend(old);
            }
            Self::Range { .. } => {}
            Self::DefaultDict { factory, entries } => {
                visit.push(*factory);
                for (key, value) in entries {
                    visit.extend([*key, *value]);
                }
            }
            Self::Scope(scope) => {
                visit.extend(scope.parent);
                visit.extend(scope.names.values());
            }
            Self::Module { scope, .. } => visit.push(*scope),
            Self::Function {
                scope,
                parameters,
                defaults,
                ..
            } => visit.extend([*scope, *parameters, *defaults]),
            Self::Class { scope, bases, .. } => {
                visit.push(*scope);
                visit.extend(bases);
            }
            Self::Instance { class, fields } => {
                visit.push(*class);
                visit.extend(fields.values());
            }
            Self::Method { callable, receiver } => visit.extend([*callable, *receiver]),
            Self::Property(val) => visit.push(*val),
            Self::Cached { function, entries } => {
                visit.push(*function);
                for call in entries {
                    visit.extend(&call.args);
                    visit.extend(call.kwargs.iter().map(|(_, id)| *id));
                    visit.push(call.result);
                }
            }
            Self::Raised(error) => visit.extend(error.args.iter().flatten()),
            Self::IoMethod { owner, .. } => visit.push(*owner),
            Self::None
            | Self::Bool(_)
            | Self::Int(_)
            | Self::Str(_)
            | Self::Bytes(_)
            | Self::ByteArray(_)
            | Self::Hash(_)
            | Self::Native(_)
            | Self::TokenMissing
            | Self::File(_)
            | Self::RegexPattern(_)
            | Self::RegexMatch { .. } => {}
        }
    }
}

#[derive(Clone, Copy)]
pub(super) enum DictViewKind {
    Keys,
    Values,
    Items,
}

/// Native frames hold temporaries until the next Python statement. A caller's
/// partial expression remains rooted while evaluating a nested native function.
pub(super) struct Heap {
    slots: Vec<Option<Value>>,
    free: Vec<usize>,
    frames: Vec<Frame>,
    next_collection: usize,
}

struct Frame {
    scope: Id,
    live: Vec<Id>,
    location: Option<Location>,
}

impl Frame {
    fn new(scope: Id) -> Self {
        Self {
            scope,
            live: Vec::new(),
            location: None,
        }
    }
}

impl Heap {
    pub fn new() -> Self {
        Self {
            slots: vec![
                Some(Value::None),
                Some(Value::Bool(false)),
                Some(Value::Bool(true)),
            ],
            free: Vec::new(),
            frames: vec![Frame::new(NONE)],
            next_collection: 4096,
        }
    }

    pub fn alloc(&mut self, value: Value) -> Id {
        let index = if let Some(index) = self.free.pop() {
            self.slots[index] = Some(value);
            index
        } else {
            self.slots.push(Some(value));
            self.slots.len() - 1
        };
        let id = index as Id;
        self.keep(id);
        id
    }

    pub fn get(&self, id: Id) -> &Value {
        self.slots[id as usize]
            .as_ref()
            .expect("live native handle")
    }
    pub fn get_mut(&mut self, id: Id) -> &mut Value {
        self.slots[id as usize]
            .as_mut()
            .expect("live native handle")
    }
    pub fn enter(&mut self, scope: Id) {
        self.frames.push(Frame::new(scope));
    }
    pub fn leave(&mut self, value: Id) {
        self.frames.pop();
        self.keep(value);
    }
    pub fn keep(&mut self, value: Id) {
        self.frames.last_mut().unwrap().live.push(value);
    }

    pub fn location(&self) -> Option<Location> {
        self.frames.last()?.location
    }

    pub fn statement(
        &mut self,
        globals: impl Iterator<Item = Id>,
        live: &[Id],
        location: Location,
    ) {
        let frame = self.frames.last_mut().unwrap();
        frame.location = Some(location);
        frame.live.clear();
        frame.live.extend(live);
        if self.slots.len() < self.next_collection || !self.free.is_empty() {
            return;
        }
        let mut pending: Vec<_> = globals.collect();
        pending.extend([NONE, FALSE, TRUE]);
        for frame in &self.frames {
            pending.push(frame.scope);
            pending.extend(&frame.live);
        }
        let mut marked = vec![false; self.slots.len()];
        while let Some(id) = pending.pop() {
            if !std::mem::replace(&mut marked[id as usize], true) {
                self.get(id).children(&mut pending);
            }
        }
        for (id, alive) in marked.into_iter().enumerate() {
            if !alive && self.slots[id].take().is_some() {
                self.free.push(id);
            }
        }
        self.next_collection = (self.slots.len() - self.free.len())
            .saturating_mul(2)
            .max(4096);
    }
}
