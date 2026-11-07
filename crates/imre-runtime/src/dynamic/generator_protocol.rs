use super::source_code::{DelegateOutcome, GeneratorInput};
use super::value::{Body, FrameOwner, Location};
use super::{Context, Error, Id, NONE, Result, Value};

pub(super) const ANEXT_METHODS: &[&str] = &[
    "__await__",
    "__iter__",
    "__next__",
    "send",
    "throw",
    "close",
];

pub(super) fn generator_method(name: &str) -> bool {
    matches!(name, "__iter__" | "__next__" | "send" | "throw" | "close")
}

pub(super) fn coroutine_method(owner: &str, name: &str) -> bool {
    match owner {
        "coroutine" => matches!(name, "send" | "throw" | "close" | "__await__"),
        "coroutine_wrapper" => matches!(name, "send" | "throw" | "close" | "__iter__" | "__next__"),
        "anext_awaitable" => ANEXT_METHODS.contains(&name),
        _ => false,
    }
}

impl Context {
    pub(super) fn new_generator(&mut self, code: Id, scope: Id, owner: FrameOwner) -> Id {
        let definition = self.code_object(code);
        let (name, qualname) = match owner {
            FrameOwner::Function(function) => {
                let Value::Function { fields, .. } = self.heap.get(function) else {
                    unreachable!("generator function owner")
                };
                (fields.name, fields.qualname)
            }
            _ => (definition.name, definition.qualname),
        };
        let generator = self.alloc(Value::Generator {
            owner,
            frame: None,
            name,
            qualname,
            lineno: definition.first_line,
            code,
            scope,
            iterators: Vec::new(),
            protected: Vec::new(),
            offset: Some(0),
            running: false,
        });
        self.heap.record_generator_birth(generator);
        generator
    }

    pub(super) fn generator_name_frame(&self, obj: Id) -> Option<Id> {
        match *self.heap.get(obj) {
            Value::Generator { .. } => Some(obj),
            Value::Coroutine { frame, .. } => Some(frame),
            _ => None,
        }
    }

    pub(super) fn set_generator_name(&mut self, obj: Id, field: &str, value: Id) -> Result<()> {
        if self.string_storage(value).is_none() {
            return Err(Error::new(
                "TypeError",
                format!("{field} must be set to a string object"),
            ));
        }
        let Value::Generator { name, qualname, .. } = self.heap.get_mut(obj) else {
            unreachable!("generator name receiver")
        };
        if field == "__name__" {
            *name = value;
        } else {
            *qualname = value;
        }
        Ok(())
    }

    /// Acquire an asynchronous iterator once, without binding its next descriptor yet.
    pub(super) fn async_for_iter(&mut self, iterable: Id) -> Result<Id> {
        let Some(method) = self.bound_slot(iterable, "__aiter__")? else {
            return Err(Error::new(
                "TypeError",
                format!(
                    "'async for' requires an object with __aiter__ method, got {:.100}",
                    self.type_name(iterable),
                ),
            ));
        };
        let iter = self.call(method, Vec::new(), Vec::new())?;
        if !matches!(self.heap.get(iter), Value::AsyncGenerator { .. })
            && self.slot(iter, "__anext__").is_none()
        {
            return Err(Error::new(
                "TypeError",
                format!(
                    "'async for' received an object from __aiter__ that does not implement __anext__: {:.100}",
                    self.type_name(iter),
                ),
            ));
        }
        Ok(iter)
    }

    /// Invoke the next slot and distinguish its errors from an invalid awaitable result.
    pub(super) fn async_for_next(&mut self, iter: Id) -> Result<Id> {
        let Some(method) = self.bound_slot(iter, "__anext__")? else {
            return Err(Error::new(
                "TypeError",
                format!(
                    "'async for' requires an iterator with __anext__ method, got {:.100}",
                    self.type_name(iter),
                ),
            ));
        };
        let value = self.call(method, Vec::new(), Vec::new())?;
        self.get_await_iterator(value, None).map_err(|error| {
            let mut invalid = Error::new(
                "TypeError",
                format!(
                    "'async for' received an invalid object from __anext__: {:.100}",
                    self.type_name(value)
                ),
            );
            let cause = self.raised_instance(error);
            invalid.tracing().cause = Some(cause);
            invalid.context = cause;
            invalid
        })
    }

    /// A coroutine is awaitable but does not implement the iterator protocol itself.
    pub(super) fn await_iterator(
        &mut self,
        awaitable: Id,
        context_method: Option<&str>,
    ) -> Result<Id> {
        if let Value::Coroutine { frame, .. } = *self.heap.get(awaitable)
            && matches!(self.heap.get(frame), Value::Generator { running: false, offset: Some(ip), .. } if *ip != 0)
        {
            return Err(Error::new(
                "RuntimeError",
                "coroutine is being awaited already",
            ));
        }
        self.get_await_iterator(awaitable, context_method)
    }

    /// Obtain the iterator for one awaitable, including a coroutine already driven by its owner.
    fn get_await_iterator(&mut self, awaitable: Id, context_method: Option<&str>) -> Result<Id> {
        if matches!(self.heap.get(awaitable), Value::Coroutine { .. }) {
            return Ok(self.alloc(Value::CoroutineWrapper(awaitable)));
        }
        if matches!(
            self.heap.get(awaitable),
            Value::ANextAwaitable { .. } | Value::AsyncGeneratorOperation { .. }
        ) || self.is_generator_coroutine(awaitable)
        {
            return Ok(awaitable);
        }
        let Some(method) = self.bound_slot(awaitable, "__await__")? else {
            let kind = self.type_name(awaitable);
            let message = match context_method {
                Some(method) => format!(
                    "'async with' received an object from {method} that does not implement __await__: {kind:.100}"
                ),
                None => format!("'{kind}' object can't be awaited"),
            };
            return Err(Error::new("TypeError", message));
        };
        let iter = self.call(method, Vec::new(), Vec::new())?;
        if matches!(self.heap.get(iter), Value::Coroutine { .. })
            || self.is_generator_coroutine(iter)
        {
            return Err(Error::new("TypeError", "__await__() returned a coroutine"));
        }
        if !self.is_native_iterator(iter) && self.slot(iter, "__next__").is_none() {
            return Err(Error::new(
                "TypeError",
                format!(
                    "__await__() returned non-iterator of type '{}'",
                    self.type_name(iter)
                ),
            ));
        }
        Ok(iter)
    }

    /// Advance the wrapped awaitable and replace only delegated asynchronous exhaustion.
    pub(super) fn call_anext_method(&mut self, method: &str, args: &[Id]) -> Result<Id> {
        let receiver = args[0];
        let Value::ANextAwaitable { wrapped, default } = *self.heap.get(receiver) else {
            return Err(Error::new(
                "TypeError",
                format!(
                    "descriptor '{method}' requires a 'anext_awaitable' object but received a '{}'",
                    self.type_name(receiver)
                ),
            ));
        };
        if matches!(method, "__await__" | "__iter__") {
            return Ok(receiver);
        }
        self.with_slot_depth("maximum recursion depth exceeded", |ctx| {
            let iter = ctx.get_await_iterator(wrapped, None)?;
            let result = if method == "__next__" {
                ctx.next_result(iter)
                    .and_then(|next| next.ok_or_else(|| ctx.exception("StopIteration", &[])))
            } else {
                // CPython's proxy forwards tuple arguments as a positional argument list.
                let forwarded = match method {
                    "send" => ctx.tuple_items(args[1])?.unwrap_or_else(|| vec![args[1]]),
                    "throw" | "close" => args[1..].to_vec(),
                    _ => unreachable!("known anext_awaitable method"),
                };
                ctx.lookup_attr(iter, method)
                    .and_then(|fun| ctx.call(fun, forwarded, Vec::new()))
            };
            match result {
                Err(error) if ctx.is_builtin_error(&error, "StopAsyncIteration") => {
                    Err(ctx.exception("StopIteration", &[default]))
                }
                other => other,
            }
        })
    }

    fn is_generator_coroutine(&self, id: Id) -> bool {
        matches!(self.heap.get(id), Value::Generator { code, .. }
            if self.code_object(*code).flags & imre_ast::code_flags::ITERABLE_COROUTINE != 0)
    }

    pub(super) fn call_coroutine_method(
        &mut self,
        owner: &str,
        method: &str,
        args: &[Id],
    ) -> Result<Id> {
        let receiver = args[0];
        let coroutine = match (owner, self.heap.get(receiver)) {
            ("coroutine", Value::Coroutine { .. }) => receiver,
            ("coroutine_wrapper", Value::CoroutineWrapper(coroutine)) => *coroutine,
            _ => {
                return Err(Error::new(
                    "TypeError",
                    format!(
                        "descriptor '{method}' requires a '{owner}' object but received a '{}'",
                        self.type_name(receiver)
                    ),
                ));
            }
        };
        let Value::Coroutine { frame, .. } = *self.heap.get(coroutine) else {
            unreachable!("coroutine receiver")
        };
        match method {
            "__await__" => return Ok(self.alloc(Value::CoroutineWrapper(coroutine))),
            "__iter__" => return Ok(receiver),
            _ => {}
        }
        // Calling an async function creates a cold frame. Count it while it is executing.
        self.check_call_depth(0)?;
        self.call_depth += 1;
        let result = stacker::maybe_grow(1024 * 1024, 4 * 1024 * 1024, || match method {
            "__next__" => self.send_generator_frame(frame, coroutine, NONE),
            "send" => self.send_generator_frame(frame, coroutine, args[1]),
            "throw" => self.throw_into_generator_frame(frame, coroutine, &args[1..]),
            "close" => self.close_generator_frame(frame, coroutine),
            _ => unreachable!("known coroutine method"),
        });
        self.call_depth -= 1;
        result
    }

    /// Normalize throw's class, instance and optional traceback before mutating the frame.
    fn generator_exception(&mut self, args: &[Id]) -> Result<Error> {
        let typ = args[0];
        let value = args.get(1).copied().unwrap_or(NONE);
        let traceback = args.get(2).copied().unwrap_or(NONE);
        if traceback != NONE && !matches!(self.heap.get(traceback), Value::Traceback { .. }) {
            return Err(Error::new(
                "TypeError",
                "throw() third argument must be a traceback object",
            ));
        }
        let base = self.builtin("BaseException");
        let instance = if self.subclass(typ, base) {
            if self.instance_of(value, typ) {
                value
            } else {
                let arguments = if value == NONE {
                    Vec::new()
                } else if let Value::Tuple(items) = self.heap.get(value) {
                    super::copy_ids(items)?
                } else {
                    vec![value]
                };
                self.call(typ, arguments, Vec::new())?
            }
        } else if matches!(self.heap.get(typ), Value::Raised(_)) {
            if value != NONE {
                return Err(Error::new(
                    "TypeError",
                    "instance exception may not have a separate value",
                ));
            }
            typ
        } else {
            return Err(Error::new(
                "TypeError",
                format!(
                    "exceptions must be classes or instances deriving from BaseException, not {}",
                    self.type_name(typ)
                ),
            ));
        };
        if traceback != NONE {
            self.set_traceback(instance, traceback)?;
        }
        let Value::Raised(error) = self.heap.get(instance) else {
            return Err(Error::new(
                "TypeError",
                "calling exception class did not return a BaseException",
            ));
        };
        let mut error = error.clone();
        error.tracing().instance = Some(instance);
        Ok(error)
    }

    /// Advance a suspended yield-from expression and retain its delegate's return value.
    pub(super) fn advance_delegate(
        &mut self,
        iter: Id,
        input: GeneratorInput,
    ) -> Result<DelegateOutcome> {
        let result = match input {
            GeneratorInput::Send(NONE) => self.next_result(iter),
            GeneratorInput::Send(value) => self
                .lookup_attr(iter, "send")
                .and_then(|method| self.call(method, vec![value], Vec::new()))
                .map(Some),
            GeneratorInput::Throw { error, .. }
                if self.is_builtin_error(&error, "GeneratorExit") =>
            {
                match self.lookup_attr(iter, "close") {
                    Ok(method) => {
                        self.call(method, Vec::new(), Vec::new())?;
                    }
                    Err(missing) if self.is_attribute_error(&missing) => {}
                    Err(other) => super::unraisable::report(
                        self,
                        other,
                        true,
                        Some(iter),
                        "Exception ignored while closing generator",
                    ),
                }
                return Err(error);
            }
            GeneratorInput::Throw { error, args } => {
                let method = match self.lookup_attr(iter, "throw") {
                    Ok(method) => method,
                    Err(missing) if self.is_attribute_error(&missing) => return Err(error),
                    Err(other) => return Ok(DelegateOutcome::LookupFailed(other)),
                };
                self.call(method, args, Vec::new()).map(Some)
            }
        };
        match result {
            Ok(Some(value)) => Ok(DelegateOutcome::Yield(value)),
            Ok(None) => Ok(DelegateOutcome::Done(NONE)),
            Err(error) if self.is_stop_iteration(&error) => {
                Ok(DelegateOutcome::Done(error.stop_value()))
            }
            Err(error) => Err(error),
        }
    }

    pub(super) fn generator_throw(&mut self, id: Id, args: &[Id]) -> Result<Id> {
        self.throw_into_generator_frame(id, id, args)
    }

    /// Throw into a frame on behalf of its public generator or coroutine.
    fn throw_into_generator_frame(&mut self, id: Id, finalizer: Id, args: &[Id]) -> Result<Id> {
        let mut error = self.generator_exception(args)?;
        let Value::Generator {
            code,
            scope,
            offset,
            running,
            lineno,
            ..
        } = *self.heap.get(id)
        else {
            return Err(Error::new(
                "TypeError",
                "generator.throw requires a generator",
            ));
        };
        if running {
            let message = if self.code_object(code).is_coroutine() {
                "coroutine already executing"
            } else {
                "generator already executing"
            };
            return Err(Error::new("ValueError", message));
        }
        if offset.is_none() {
            return if self.code_object(code).is_coroutine() {
                Err(Error::new(
                    "RuntimeError",
                    "cannot reuse already awaited coroutine",
                ))
            } else {
                Err(error)
            };
        }
        let fresh = offset == Some(0);
        if fresh || matches!(self.code_object(code).body, Body::Native(_)) {
            // A fresh frame unwinds at its definition, a suspended native frame at its yield.
            let definition = self.code_object(code);
            let location = Location {
                file: self.str_value(definition.filename)?.to_owned().into(),
                line: lineno,
                span: None,
            };
            self.heap.enter(scope, code, FrameOwner::Generator(id));
            self.heap.publish_statement(&[], location, NONE);
            self.record_exception_frame(&mut error, None);
            self.heap.leave(NONE, false);
            if !fresh {
                // Unwind the compiled iterator stack. The traceback retains Python locals,
                // including the outer iterator in .0.
                let Value::Scope(frame) = self.heap.get_mut(scope) else {
                    unreachable!("generator scope")
                };
                frame
                    .names
                    .retain(|name, _| name != ".iterator" && !name.starts_with(".iterator."));
                error = self.frame_error(error, code);
            }
            self.heap.disarm_finalizer(finalizer);
            self.heap.discard_generator_frame(id);
            return Err(error);
        }
        self.resume_source_input(
            id,
            finalizer,
            GeneratorInput::Throw {
                error,
                args: super::copy_ids(args)?,
            },
        )?
        .ok_or_else(|| self.exception("StopIteration", &[]))
    }

    pub(super) fn generator_close(&mut self, id: Id) -> Result<Id> {
        self.close_generator_frame(id, id)
    }

    /// Close a suspended frame on behalf of its public generator or coroutine.
    pub(super) fn close_generator_frame(&mut self, id: Id, finalizer: Id) -> Result<Id> {
        let Value::Generator {
            code,
            offset,
            running,
            ..
        } = *self.heap.get(id)
        else {
            return Err(Error::new(
                "TypeError",
                "generator.close requires a generator",
            ));
        };
        if running {
            let message = if self.code_object(code).is_coroutine() {
                "coroutine already executing"
            } else {
                "generator already executing"
            };
            return Err(Error::new("ValueError", message));
        }
        if offset.is_none() {
            return Ok(NONE);
        }
        if offset == Some(0) || matches!(self.code_object(code).body, Body::Native(_)) {
            self.heap.disarm_finalizer(finalizer);
            self.heap.discard_generator_frame(id);
            return Ok(NONE);
        }
        let exit = self.exception("GeneratorExit", &[]);
        match self.resume_source_input(
            id,
            finalizer,
            GeneratorInput::Throw {
                error: exit,
                args: Vec::new(),
            },
        ) {
            Ok(None) => Ok(NONE),
            Ok(Some(_)) => Err(Error::new(
                "RuntimeError",
                if self.code_object(code).is_coroutine() {
                    "coroutine ignored GeneratorExit"
                } else {
                    "generator ignored GeneratorExit"
                },
            )),
            Err(error) if self.is_builtin_error(&error, "GeneratorExit") => Ok(NONE),
            Err(error) if self.is_stop_iteration(&error) => Ok(error.stop_value()),
            Err(error) => Err(error),
        }
    }
}
