use super::{Context, Error, Id, NONE, Result, Value};

/// The original argument of an asynchronous operation survives awaits inside its generator.
#[derive(Clone, Copy)]
pub(super) enum Operation {
    Send(Id),
    Throw(Id),
    Close,
}

impl Operation {
    pub(super) fn type_name(self) -> &'static str {
        match self {
            Self::Send(_) => "async_generator_asend",
            _ => "async_generator_athrow",
        }
    }
    pub(super) fn argument(self) -> Option<Id> {
        match self {
            Self::Send(v) | Self::Throw(v) => Some(v),
            Self::Close => None,
        }
    }
    fn method(self) -> &'static str {
        match self {
            Self::Send(_) => "anext",
            Self::Throw(_) => "athrow",
            Self::Close => "aclose",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum State {
    Initial,
    Active,
    Done,
}

pub(super) fn method(owner: &str, name: &str) -> bool {
    match owner {
        "async_generator" => matches!(
            name,
            "__aiter__" | "__anext__" | "asend" | "athrow" | "aclose"
        ),
        "async_generator_asend" | "async_generator_athrow" => {
            super::generator_protocol::ANEXT_METHODS.contains(&name)
        }
        _ => false,
    }
}

impl Context {
    /// The generator owns the source frame and is responsible for its finalization.
    pub(super) fn async_generator(&mut self, frame: Id) -> Id {
        self.alloc(Value::AsyncGenerator {
            frame,
            running: false,
            closed: false,
            finalizer: None,
        })
    }
    /// Tag a direct item yield while leaving delegated await suspensions untouched.
    pub(super) fn wrap_async_generator_yield(&mut self, item: Id) -> Id {
        self.alloc(Value::AsyncGeneratorYield(item))
    }
    pub(super) fn call_async_generator(
        &mut self,
        object: Id,
        method: &str,
        args: &[Id],
    ) -> Result<Id> {
        if method == "__aiter__" {
            return Ok(object);
        }
        if method == "athrow" && args.len() > 1 {
            self.emit_builtin_warning(
                crate::objects::exceptions::Kind::DeprecationWarning,
                "the (type, exc, tb) signature of athrow() is deprecated, use the single-arg signature instead.",
                1,
            )?;
        }
        let Value::AsyncGenerator { finalizer, .. } = self.heap.get_mut(object) else {
            unreachable!()
        };
        if finalizer.is_none() {
            let [first, last] = self.threads.asyncgen_hooks;
            *finalizer = Some(last);
            self.heap.register_finalizer(object);
            if first != NONE {
                self.call(first, vec![object], Vec::new())?;
            }
        }
        let kind = match method {
            "__anext__" => Operation::Send(NONE),
            "asend" => Operation::Send(args[0]),
            "athrow" => Operation::Throw(self.alloc(Value::Tuple(super::copy_ids(args)?))),
            "aclose" => Operation::Close,
            _ => unreachable!(),
        };
        Ok(self.alloc(Value::AsyncGeneratorOperation {
            generator: object,
            kind,
            state: State::Initial,
        }))
    }
    pub(super) fn finalize_async_generator(&mut self, owner: Id) {
        let Value::AsyncGenerator {
            frame,
            closed,
            finalizer,
            ..
        } = *self.heap.get(owner)
        else {
            unreachable!()
        };
        if matches!(self.heap.get(frame), Value::Generator { offset: None, .. }) {
            return;
        }
        let (result, message) =
            if let Some(hook) = finalizer.filter(|&hook| hook != NONE && !closed) {
                (
                    self.call(hook, vec![owner], Vec::new()),
                    "Exception ignored while finalizing generator",
                )
            } else {
                (
                    self.generator_close(frame),
                    "Exception ignored while closing generator",
                )
            };
        if let Err(error) = result {
            super::unraisable::report(self, error, true, Some(owner), message);
        }
    }
    fn set_async_operation_state(&mut self, operation: Id, next: State) {
        let Value::AsyncGeneratorOperation { state, .. } = self.heap.get_mut(operation) else {
            unreachable!()
        };
        *state = next;
    }
    pub(super) fn call_async_generator_op(
        &mut self,
        operation: Id,
        method: &str,
        args: &[Id],
    ) -> Result<Id> {
        if matches!(method, "__await__" | "__iter__") {
            return Ok(operation);
        }
        let Value::AsyncGeneratorOperation {
            generator,
            kind,
            state,
        } = *self.heap.get(operation)
        else {
            unreachable!()
        };
        if state == State::Done {
            if method == "close" {
                return Ok(NONE);
            }
            let methods = if matches!(kind, Operation::Send(_)) {
                "__anext__()/asend()"
            } else {
                "aclose()/athrow()"
            };
            return Err(Error::new(
                "RuntimeError",
                format!("cannot reuse already awaited {methods}"),
            ));
        }
        // close() on the operation drives a GeneratorExit into the pending await.
        // aclose() on the generator creates a distinct awaitable.
        if method == "close" {
            let exit = self.builtin("GeneratorExit");
            return match self.call_async_generator_op(operation, "throw", &[exit]) {
                Err(e)
                    if self.is_stop_iteration(&e)
                        || self.is_builtin_error(&e, "StopAsyncIteration")
                        || self.is_builtin_error(&e, "GeneratorExit") =>
                {
                    Ok(NONE)
                }
                Err(e) => Err(e),
                Ok(_) => Err(Error::new(
                    "RuntimeError",
                    "coroutine ignored GeneratorExit",
                )),
            };
        }
        let Value::AsyncGenerator {
            frame,
            running,
            closed,
            ..
        } = *self.heap.get(generator)
        else {
            unreachable!()
        };
        let Value::Generator {
            offset,
            running: executing,
            ..
        } = *self.heap.get(frame)
        else {
            unreachable!()
        };
        let sent = args.first().copied().unwrap_or(NONE);
        let first = state == State::Initial;
        let closing = matches!(kind, Operation::Close);
        if method != "throw"
            && !matches!(kind, Operation::Send(_))
            && offset.is_none()
            && !executing
        {
            self.set_async_operation_state(operation, State::Done);
            return Err(self.exception("StopIteration", &[]));
        }
        if first {
            if running {
                self.set_async_operation_state(operation, State::Done);
                return Err(Error::new(
                    "RuntimeError",
                    format!(
                        "{}(): asynchronous generator is already running",
                        kind.method()
                    ),
                ));
            }
            if method != "throw" && !matches!(kind, Operation::Send(_)) {
                if closed {
                    self.set_async_operation_state(operation, State::Done);
                    return Err(self.exception("StopAsyncIteration", &[]));
                }
                if sent != NONE {
                    return Err(Error::new(
                        "RuntimeError",
                        "can't send non-None value to a just-started coroutine",
                    ));
                }
            }
            self.set_async_operation_state(operation, State::Active);
            let Value::AsyncGenerator {
                running, closed, ..
            } = self.heap.get_mut(generator)
            else {
                unreachable!()
            };
            *running = true;
            if closing && method != "throw" {
                *closed = true;
            }
        }
        // athrow takes ownership before unpacking its arguments on the first send.
        // An arity error leaves that operation able to resume the generator.
        if first
            && method != "throw"
            && let Operation::Throw(arguments) = kind
        {
            let Value::Tuple(args) = self.heap.get(arguments) else {
                unreachable!()
            };
            match args.len() {
                0 => {
                    return Err(Error::new(
                        "TypeError",
                        "athrow expected at least 1 argument, got 0",
                    ));
                }
                n if n > 3 => {
                    return Err(Error::new(
                        "TypeError",
                        format!("athrow expected at most 3 arguments, got {n}"),
                    ));
                }
                _ => {}
            }
        }
        let result = self.with_slot_depth("maximum recursion depth exceeded", |ctx| {
            if method == "throw" {
                return ctx.generator_throw(frame, args);
            }
            if first {
                match kind {
                    Operation::Close => {
                        let exit = ctx.builtin("GeneratorExit");
                        return ctx.generator_throw(frame, &[exit]);
                    }
                    Operation::Throw(arguments) => {
                        let args = ctx.tuple_items(arguments)?.expect("athrow argument tuple");
                        return ctx.generator_throw(frame, &args);
                    }
                    Operation::Send(initial) => {
                        let value = if sent == NONE { initial } else { sent };
                        if offset == Some(0) && value != NONE {
                            return Err(Error::new(
                                "TypeError",
                                "can't send non-None value to a just-started async generator",
                            ));
                        }
                        return ctx.generator_send(frame, value);
                    }
                }
            }
            ctx.generator_send(frame, sent)
        });
        let terminal = match result {
            Ok(id) => match *self.heap.get(id) {
                Value::AsyncGeneratorYield(_) if closing => Err(Error::new(
                    "RuntimeError",
                    "async generator ignored GeneratorExit",
                )),
                Value::AsyncGeneratorYield(value) if value == NONE => {
                    Err(self.exception("StopIteration", &[]))
                }
                Value::AsyncGeneratorYield(value) => Err(self.exception("StopIteration", &[value])),
                _ => return Ok(id), // An await inside the generator suspended this operation.
            },
            // A throw into a fresh or finished frame raises the injected exception itself.
            Err(e)
                if matches!(offset, None | Some(0))
                    && (method == "throw" || first && matches!(kind, Operation::Throw(_)))
                    && self.is_stop_iteration(&e) =>
            {
                Err(e)
            }
            Err(e) if self.is_stop_iteration(&e) => Err(self.exception(
                if closing {
                    "StopIteration"
                } else {
                    "StopAsyncIteration"
                },
                &[],
            )),
            Err(e)
                if closing
                    && (self.is_builtin_error(&e, "GeneratorExit")
                        || self.is_builtin_error(&e, "StopAsyncIteration")) =>
            {
                Err(self.exception("StopIteration", &[]))
            }
            Err(e) => Err(e),
        };
        // After a suspended athrow, send() unwraps the next item but leaves the
        // operation usable until a subsequent send observes a finished frame.
        if first || method == "throw" || !matches!(kind, Operation::Throw(_)) {
            self.set_async_operation_state(operation, State::Done);
        }
        let Value::AsyncGenerator { running, .. } = self.heap.get_mut(generator) else {
            unreachable!()
        };
        *running = false;
        terminal
    }
}
