//! Context-local bindings hold boxed heap handles, including arbitrary wheel-defined objects.

use std::rc::Rc;

use super::{Context, Error, Id, NONE, Result, Value};

impl Context {
    pub(in super::super) fn context_key(&self, key: Id) -> Result<()> {
        if matches!(self.heap.get(key), Value::ContextVar { .. }) {
            Ok(())
        } else {
            Err(Error::new(
                "TypeError",
                format!("a ContextVar key was expected, got {}", self.repr(key)),
            ))
        }
    }

    pub(in super::super) fn context_iterator(&mut self, context: Id, name: &str) -> Id {
        let bindings = Rc::clone(self.bindings(context));
        let values = bindings
            .iter()
            .map(|(&key, &value)| match name {
                "Context.keys" => key,
                "Context.values" => value,
                _ => self.alloc(Value::Tuple(vec![key, value])),
            })
            .collect();
        let sequence = self.alloc(Value::Tuple(values));
        self.alloc(Value::Iterator {
            sequence: Some(sequence),
            offset: 0,
            reverse: false,
            kind: match name {
                "Context.keys" => "keys",
                "Context.values" => "values",
                _ => "items",
            },
            context: Some(bindings),
        })
    }

    fn bindings(&self, context: Id) -> &Rc<std::collections::HashMap<Id, Id>> {
        let Value::Context(bindings) = self.heap.get(context) else {
            unreachable!("contextvars receiver is an exact Context");
        };
        bindings
    }

    fn replace_binding(&mut self, context: Id, variable: Id, value: Option<Id>) -> Option<Id> {
        let Value::Context(bindings) = self.heap.get_mut(context) else {
            unreachable!("contextvars receiver is an exact Context");
        };
        let bindings = Rc::make_mut(bindings);
        match value {
            Some(value) => bindings.insert(variable, value),
            None => bindings.remove(&variable),
        }
    }

    fn reset_binding(&mut self, variable: Option<Id>, token: Id) -> Result<Id> {
        let Value::ContextToken {
            context,
            variable: key,
            old,
            used,
        } = *self.heap.get(token)
        else {
            return Err(Error::new(
                "TypeError",
                format!("expected an instance of Token, got {}", self.repr(token)),
            ));
        };
        let reason = if used {
            Some(("RuntimeError", "has already been used once"))
        } else if variable.is_some_and(|var| var != key) {
            Some(("ValueError", "was created by a different ContextVar"))
        } else if Some(&context) != self.contexts.last() {
            Some(("ValueError", "was created in a different Context"))
        } else {
            None
        };
        if let Some((kind, reason)) = reason {
            return Err(Error::new(kind, format!("{} {reason}", self.repr(token))));
        }
        self.replace_binding(context, key, old);
        if let Value::ContextToken { used, .. } = self.heap.get_mut(token) {
            *used = true;
        }
        Ok(NONE)
    }
}

pub(super) fn call(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let current = *ctx.contexts.last().expect("initial context");
    match (name, args, kwargs) {
        ("ContextVar", [var_name], [] | [(_, _)]) => {
            if !matches!(ctx.heap.get(*var_name), Value::Str(_)) {
                return Err(Error::new(
                    "TypeError",
                    "context variable name must be a str",
                ));
            }
            let default = match kwargs {
                [] => None,
                [(key, value)] if key == "default" => Some(*value),
                [(key, _)] => {
                    return Err(Error::new(
                        "TypeError",
                        format!("ContextVar() got an unexpected keyword argument '{key}'"),
                    ));
                }
                _ => unreachable!(),
            };
            Ok(ctx.alloc(Value::ContextVar {
                name: *var_name,
                default,
                repr_active: Default::default(),
            }))
        }
        ("ContextVar.get", [variable] | [variable, _], []) => {
            let Value::ContextVar { default, .. } = ctx.heap.get(*variable) else {
                unreachable!("bound ContextVar.get");
            };
            ctx.bindings(current)
                .get(variable)
                .copied()
                .or_else(|| args.get(1).copied())
                .or(*default)
                .ok_or_else(|| ctx.lookup_error("LookupError", *variable))
        }
        ("ContextVar.set", [variable, value], []) => {
            let old = ctx.replace_binding(current, *variable, Some(*value));
            Ok(ctx.alloc(Value::ContextToken {
                context: current,
                variable: *variable,
                old,
                used: false,
            }))
        }
        ("ContextVar.reset", [variable, token], []) => ctx.reset_binding(Some(*variable), *token),
        ("Token.__enter__", [token], []) => Ok(*token),
        ("Token", _, _) => Err(Error::new(
            "RuntimeError",
            "Tokens can only be created by ContextVars",
        )),
        ("Token.__exit__", [token, _, _, _], []) => ctx.reset_binding(None, *token),
        ("Context", [], []) => Ok(ctx.alloc(Value::Context(Default::default()))),
        ("copy_context", [], []) | ("Context.copy", [_], []) => {
            let source = args.first().copied().unwrap_or(current);
            Ok(ctx.alloc(Value::Context(Rc::clone(ctx.bindings(source)))))
        }
        ("Context.get", [context, key] | [context, key, _], []) => {
            ctx.context_key(*key)?;
            Ok(ctx
                .bindings(*context)
                .get(key)
                .copied()
                .or_else(|| args.get(2).copied())
                .unwrap_or(NONE))
        }
        ("Context.keys" | "Context.values" | "Context.items", [context], []) => {
            Ok(ctx.context_iterator(*context, name))
        }
        ("Context.run", [context, callable, rest @ ..], kwargs) => {
            if ctx.contexts.contains(context) {
                return Err(Error::new(
                    "RuntimeError",
                    format!(
                        "cannot enter context: {} is already entered",
                        ctx.repr(*context)
                    ),
                ));
            }
            ctx.contexts.push(*context);
            let result = ctx.call(*callable, rest.to_vec(), kwargs.to_vec());
            ctx.contexts.pop();
            result
        }
        _ => Err(Error::new(
            "TypeError",
            format!("invalid arguments to {name}"),
        )),
    }
}
