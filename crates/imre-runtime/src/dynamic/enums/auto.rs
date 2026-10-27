//! Resolve automatic values while an Enum class body is executing.
use super::*;

#[derive(Clone, Copy, PartialEq)]
pub(in crate::dynamic) enum EnumKind {
    Plain,
    Integer,
    String,
    IntFlag,
}

impl EnumKind {
    pub(in crate::dynamic) fn scalar_base(self) -> Option<&'static str> {
        match self {
            Self::Plain => None,
            Self::Integer => Some("enum.IntEnum"),
            Self::String => Some("enum.StrEnum"),
            Self::IntFlag => Some("enum.IntFlag"),
        }
    }
}

#[derive(Clone)]
pub(in crate::dynamic) struct EnumNamespace {
    pub kind: EnumKind,
    pub members: indexmap::IndexSet<String>,
    pub generator: Option<Id>,
    pub history: Vec<Id>,
    generated: bool,
}

impl Context {
    pub(in crate::dynamic) fn enum_namespace(&self, bases: &[Id]) -> Option<Box<EnumNamespace>> {
        let (base, kind) = bases.iter().rev().find_map(|&base| {
            [
                ("enum.IntFlag", EnumKind::IntFlag),
                ("enum.IntEnum", EnumKind::Integer),
                ("enum.StrEnum", EnumKind::String),
                ("enum.Enum", EnumKind::Plain),
            ]
            .into_iter()
            .find(|(name, _)| self.has_base(base, name))
            .map(|(_, kind)| (base, kind))
        })?;
        Some(Box::new(EnumNamespace {
            kind,
            members: indexmap::IndexSet::new(),
            generator: self.class_attr(base, "_generate_next_value_"),
            history: Vec::new(),
            generated: false,
        }))
    }

    pub(in crate::dynamic) fn enum_auto_class(&mut self) -> Result<Id> {
        if let Some(&class) = self.classes.get("enum.auto") {
            return Ok(class);
        }
        let scope = self.scope("enum", None);
        let module = self.text("enum");
        self.bind(scope, "__module__", module);
        for method in ["__init__", "__repr__"] {
            let callable = self.builtin(&format!("enum.auto.{method}"));
            self.bind(scope, method, callable);
        }
        let class = self.allocate_class("auto".into(), scope, Vec::new())?;
        self.classes.insert("enum.auto".into(), class);
        Ok(class)
    }

    pub(in crate::dynamic) fn auto_method(
        &mut self,
        name: &str,
        args: &[Id],
        kwargs: &[(String, Id)],
    ) -> Result<Id> {
        let Some((&marker, rest)) = args.split_first() else {
            return Err(Error::new("TypeError", "auto method requires an instance"));
        };
        let sentinel = self.builtin("enum._auto_null");
        if name == "enum.auto.__init__" {
            let [value] = builtins::bind_method_arguments("auto", rest, kwargs, ["value"], 1)?;
            self.default_set_attr(marker, "value", value.unwrap_or(sentinel))?;
            Ok(NONE)
        } else {
            builtins::bind_method_arguments("__repr__", rest, kwargs, [], 0)?;
            let value = self.lookup_attr(marker, "value")?;
            let repr = if value == sentinel {
                "_auto_null".into()
            } else {
                self.repr(value)
            };
            Ok(self.text(format!("auto({repr})")))
        }
    }

    fn enum_body_mut(&mut self, scope: Id) -> &mut EnumNamespace {
        let Value::Scope(scope) = self.heap.get_mut(scope) else {
            unreachable!()
        };
        scope.enum_namespace.as_deref_mut().unwrap()
    }

    /// Like EnumDict.__setitem__, make the resolved value visible to the next assignment.
    pub(in crate::dynamic) fn enum_assignment(
        &mut self,
        scope: Id,
        name: &str,
        value: Id,
    ) -> Result<Id> {
        let Some(state) = self.scope_data(scope).enum_namespace.as_deref() else {
            return Ok(value);
        };
        if self.binding_scope(scope, name) != scope {
            return Ok(value);
        }
        if name == "_generate_next_value_" {
            if state.generated {
                return Err(Error::new(
                    "TypeError",
                    "_generate_next_value_ must be defined before members",
                ));
            }
            self.enum_body_mut(scope).generator = Some(value);
            return Ok(value);
        }
        if name.starts_with('_') && name.ends_with('_') {
            return Ok(value);
        }
        if state.members.contains(name) {
            let old = self.scope_data(scope).names[name];
            return Err(Error::new(
                "TypeError",
                format!("'{name}' already defined as {}", self.repr(old)),
            ));
        }
        if self.is_enum_descriptor(value)? {
            return Ok(value);
        }
        let marker = self.classes.get("enum.auto").copied();
        let is_auto = |ctx: &Self, id| marker.is_some_and(|class| ctx.instance_of(id, class));
        let parts = match self.heap.get(value) {
            Value::Tuple(items) if items.iter().any(|&id| is_auto(self, id)) => Some(items.clone()),
            _ => None,
        };
        let resolved = if is_auto(self, value) {
            self.resolve_auto(scope, name, value)?
        } else if let Some(mut parts) = parts {
            for part in parts.iter_mut() {
                if is_auto(self, *part) {
                    *part = self.resolve_auto(scope, name, *part)?;
                }
            }
            self.alloc(Value::Tuple(parts))
        } else {
            self.enum_body_mut(scope).history.push(value);
            value
        };
        self.enum_body_mut(scope).members.insert(name.to_owned());
        Ok(resolved)
    }

    fn resolve_auto(&mut self, scope: Id, name: &str, marker: Id) -> Result<Id> {
        let sentinel = self.builtin("enum._auto_null");
        let mut value = self.lookup_attr(marker, "value")?;
        if value == sentinel {
            let state = self.scope_data(scope).enum_namespace.as_deref().unwrap();
            let count = state.members.len();
            let kind = state.kind;
            let generator = state.generator;
            let empty = state.history.is_empty();
            let history = self.alloc(Value::List(state.history.clone()));
            let start = self.int(1);
            value = if let Some(generator) = generator {
                let name = self.text(name);
                let count = self.int(count as i64);
                self.call(generator, vec![name, start, count, history], Vec::new())?
            } else if kind == EnumKind::String {
                self.text(name.to_lowercase())
            } else if empty || (kind == EnumKind::IntFlag && count == 0) {
                start
            } else if kind == EnumKind::IntFlag {
                let mut values = self.items(history)?.into_iter();
                let mut highest = values.next().unwrap();
                for value in values {
                    let greater = self.binary(">", value, highest)?;
                    if self.truth(greater)? {
                        highest = value;
                    }
                }
                let bits = match self.heap.get(highest) {
                    Value::Int(value) => self.int(value.big().bits() as i64),
                    Value::Bool(value) => self.int(i64::from(*value)),
                    _ => self
                        .lookup_attr(highest, "bit_length")
                        .and_then(|method| self.call(method, Vec::new(), Vec::new()))
                        .map_err(|_| {
                            Error::new(
                                "TypeError",
                                format!("invalid flag value {}", self.repr(highest)),
                            )
                        })?,
                };
                self.binary("<<", start, bits)?
            } else {
                let sorted = builtins::call(self, "sorted", &[history], &[]).map_err(|error| {
                    if error.kind == "TypeError" {
                        Error::new("TypeError", "unable to sort non-numeric values")
                    } else {
                        error
                    }
                })?;
                let Value::List(values) = self.heap.get(sorted) else {
                    unreachable!()
                };
                let highest = *values.last().unwrap();
                self.binary("+", highest, start).map_err(|error| {
                    if error.kind == "TypeError" {
                        Error::new(
                            "TypeError",
                            format!("unable to increment {}", self.repr(highest)),
                        )
                    } else {
                        error
                    }
                })?
            };
            self.default_set_attr(marker, "value", value)?;
            self.enum_body_mut(scope).generated = true;
        }
        self.enum_body_mut(scope).history.push(value);
        Ok(value)
    }
}
