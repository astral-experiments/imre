//! Context-local bindings hold boxed heap handles, including arbitrary wheel-defined objects.

use std::borrow::Cow;
use std::rc::Rc;

use crate::dynamic::calls::{RegisteredCall, ScopeExit, ScopedCall};

use super::{Context, Error, Id, NONE, Result, Value};
use crate::objects::{consume_context_token, replace_context_binding};

impl Context {
    pub(in crate::dynamic) fn prepare_context_run(
        &mut self,
        args: Cow<'_, [Id]>,
        kwargs: Vec<(String, Id)>,
    ) -> Result<RegisteredCall> {
        // Keywords belong to the callback, including those matching Context.run's receiver.
        let (context, written) =
            super::builtins::method_receiver_args(self, "_contextvars.Context", "run", &args, &[])?;
        let (callable, rest) = imre_ast::context_run_arguments(written)
            .map_err(|message| Error::new("TypeError", message))?;
        let callable = *callable;
        let skip = args.len() - rest.len();
        if self.heap.context_is_entered(context) {
            return Err(Error::new(
                "RuntimeError",
                format!(
                    "cannot enter context: {} is already entered",
                    self.repr(context)
                ),
            ));
        }
        let mut callback_args = crate::dynamic::owned_ids(args)?;
        drop(callback_args.drain(..skip));
        Ok(ScopedCall::register(
            ScopeExit::Context(context),
            callable,
            callback_args,
            kwargs,
        ))
    }

    /// Use the entered context, creating the implicit one on its first copy or set.
    fn current_context(&mut self) -> Id {
        if let Some(&current) = self.heap.execution.contexts.last() {
            return current;
        }
        let current = self.alloc(Value::Context(Default::default()));
        self.heap.execution.contexts.push(current);
        current
    }

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
        replace_context_binding(bindings, variable, value, Id::eq)
    }

    fn reset_binding(&mut self, variable: Option<Id>, token: Id) -> Result<Id> {
        let Value::ContextToken {
            context,
            variable: key,
            old,
            used,
        } = self.heap.get(token)
        else {
            return Err(Error::new(
                "TypeError",
                format!(
                    "expected an instance of Token, got {}",
                    self.checked_format(token, true)?
                ),
            ));
        };
        let same_variable = variable.is_none_or(|var| var == *key);
        if let Err((kind, reason)) = consume_context_token(used, same_variable, || {
            Some(context) == self.heap.execution.contexts.last()
        }) {
            let token_repr = self.checked_format(token, true)?;
            return Err(Error::new(kind.name(), format!("{token_repr} {reason}")));
        }
        self.replace_binding(*context, *key, *old);
        Ok(NONE)
    }
}

pub(super) fn call(
    ctx: &mut Context,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    match (name, args, kwargs) {
        ("ContextVar", _, _) => {
            let count = args.len() + kwargs.len();
            if count > 2 {
                let kind = if args.is_empty() { "keyword " } else { "" };
                return Err(Error::new(
                    "TypeError",
                    format!("ContextVar() takes at most 2 {kind}arguments ({count} given)"),
                ));
            }
            let &[var_name] = args else {
                let limit = if args.is_empty() {
                    "exactly"
                } else {
                    "at most"
                };
                return Err(Error::new(
                    "TypeError",
                    format!(
                        "ContextVar() takes {limit} 1 positional argument ({} given)",
                        args.len()
                    ),
                ));
            };
            let default = match kwargs.first() {
                None => None,
                Some((key, value)) if key == "default" => Some(*value),
                Some((key, _)) => {
                    return Err(crate::dynamic::calls::unexpected_keyword_error(
                        "ContextVar",
                        key,
                        &["default"],
                    ));
                }
            };
            if ctx.string_storage(var_name).is_none() {
                return Err(Error::new(
                    "TypeError",
                    "context variable name must be a str",
                ));
            }
            let name_hash = ctx.dictionary_hash(var_name)?;
            Ok(ctx.alloc(Value::ContextVar {
                name: var_name,
                name_hash,
                default,
                repr_active: Default::default(),
            }))
        }
        ("ContextVar.get", [variable] | [variable, _], []) => {
            let Value::ContextVar { default, .. } = ctx.heap.get(*variable) else {
                unreachable!("bound ContextVar.get");
            };
            ctx.heap
                .execution
                .contexts
                .last()
                .and_then(|&current| ctx.bindings(current).get(variable))
                .copied()
                .or_else(|| args.get(1).copied())
                .or(*default)
                .ok_or_else(|| ctx.lookup_error("LookupError", *variable))
        }
        ("ContextVar.set", [variable, value], []) => {
            let current = ctx.current_context();
            let old = ctx.replace_binding(current, *variable, Some(*value));
            Ok(ctx.alloc(Value::ContextToken {
                context: current,
                variable: *variable,
                old,
                used: Default::default(),
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
        ("Context", _, _) => Err(Error::new(
            "TypeError",
            "Context() does not accept any arguments",
        )),
        ("copy_context", [], []) | ("Context.copy", [_], []) => {
            let source = args
                .first()
                .copied()
                .unwrap_or_else(|| ctx.current_context());
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
        _ => Err(Error::new(
            "TypeError",
            format!("invalid arguments to {name}"),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scoped_run_exits_after_immediate_return_and_preparation_error() {
        let mut ctx = crate::dynamic::ffi::new_context(serde_json::json!({"paths": []}), ["imre"]);
        let explicit = call(&mut ctx, "Context", &[], &[]).unwrap();
        let run = ctx.lookup_attr(explicit, "run").unwrap();
        let len = ctx.builtin("len");
        let data = ctx.alloc(Value::Bytes(b"abc".to_vec()));
        let value = ctx.call(run, vec![len, data], vec![]).unwrap();
        assert_eq!(value, ctx.int(3));
        assert!(ctx.heap.execution.contexts.is_empty());
        let scalar = ctx.int(1);
        assert_eq!(
            ctx.call(run, vec![len, scalar], vec![]).unwrap_err().kind,
            "TypeError"
        );
        assert!(ctx.heap.execution.contexts.is_empty());

        unsafe extern "C" fn entry(_: *mut Context, _: Id) -> Id {
            NONE
        }
        let globals = ctx.scope("scope_test", None);
        let name = ctx.text("requires_two");
        let signature = ctx.text(r#"{"positional":["first","second"]}"#);
        let file = ctx.text("scope_test.py");
        let code = ctx.prelinked_code(
            crate::dynamic::value::Body::Native(entry, NONE),
            globals,
            name,
            signature,
            (name, NONE),
            (file, 1),
        );
        let callback = ctx
            .define_function(code, globals, name, NONE, name, NONE, NONE, false)
            .unwrap();
        assert_eq!(
            ctx.call(run, vec![callback, data], vec![])
                .unwrap_err()
                .kind,
            "TypeError"
        );
        assert!(ctx.heap.execution.contexts.is_empty());
    }

    #[test]
    fn shared_snapshot_detaches_only_for_a_different_value() {
        let mut ctx = crate::dynamic::ffi::new_context(serde_json::json!({"paths": []}), ["imre"]);
        let name = ctx.text("key");
        let key = call(&mut ctx, "ContextVar", &[name], &[]).unwrap();
        let first = ctx.alloc(Value::List(Vec::new()));
        let second = ctx.alloc(Value::List(Vec::new()));
        let current = ctx.current_context();
        assert_eq!(ctx.replace_binding(current, key, Some(first)), None);
        let copy = call(&mut ctx, "copy_context", &[], &[]).unwrap();
        for value in [first, second] {
            assert_eq!(ctx.replace_binding(current, key, Some(value)), Some(first));
            assert_eq!(
                Rc::ptr_eq(ctx.bindings(copy), ctx.bindings(current)),
                value == first
            );
            assert_eq!(ctx.bindings(copy)[&key], first);
            assert_eq!(ctx.bindings(current)[&key], value);
        }
    }

    #[test]
    fn foreign_reset_leaves_the_ambient_context_for_the_first_copy() {
        let mut ctx = crate::dynamic::ffi::new_context(serde_json::json!({"paths": []}), ["imre"]);
        let name = ctx.text("key");
        let var = call(&mut ctx, "ContextVar", &[name], &[]).unwrap();
        let explicit = call(&mut ctx, "Context", &[], &[]).unwrap();
        let set = ctx.lookup_attr(var, "set").unwrap();
        let run = ctx.lookup_attr(explicit, "run").unwrap();
        let token = ctx.call(run, vec![set, name], vec![]).unwrap();
        let error = call(&mut ctx, "ContextVar.reset", &[var, token], &[]).unwrap_err();
        assert_eq!(error.kind, "ValueError");
        assert!(ctx.heap.execution.contexts.is_empty());

        let copy = call(&mut ctx, "copy_context", &[], &[]).unwrap();
        let current = ctx.heap.execution.contexts[0];
        assert_ne!(copy, current);
        assert!(Rc::ptr_eq(ctx.bindings(copy), ctx.bindings(current)));
        call(&mut ctx, "ContextVar.set", &[var, name], &[]).unwrap();
        assert!(ctx.bindings(copy).is_empty());
    }
}
