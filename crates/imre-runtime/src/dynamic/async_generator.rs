use super::value::MarkQueue;
use super::{Context, Error, Id, NONE, Result, Value};
use crate::objects::exceptions::Kind;

pub(super) const METHODS: &[&str] = &["__aiter__", "__anext__", "asend", "athrow", "aclose"];

#[derive(Clone, Copy)]
enum Request {
    Send(Id),
    Throw(Id),
    Close,
}

#[derive(Clone, Copy, PartialEq)]
enum State {
    New,
    Running,
    Done,
}

#[derive(Clone, Copy)]
pub(super) struct Operation {
    generator: Id,
    request: Request,
    state: State,
}

impl Operation {
    pub(super) fn class(self) -> &'static str {
        match self.request {
            Request::Send(_) => "async_generator_asend",
            Request::Throw(_) | Request::Close => "async_generator_athrow",
        }
    }

    pub(super) fn trace(self, visit: &mut MarkQueue<'_>) {
        visit.push(self.generator);
        if let Request::Send(arg) | Request::Throw(arg) = self.request {
            visit.push(arg);
        }
    }
}

impl Context {
    pub(super) fn call_async_generator_method(
        &mut self,
        owner: &str,
        method: &str,
        args: &[Id],
    ) -> Result<Id> {
        let receiver = args[0];
        let valid = match self.heap.get(receiver) {
            Value::AsyncGenerator { .. } => owner == "async_generator",
            Value::AsyncGeneratorOp(op) => owner == op.class(),
            _ => false,
        };
        if !valid {
            return Err(Error::new(
                "TypeError",
                format!("descriptor '{method}' requires a '{owner}' object"),
            ));
        }
        if owner == "async_generator" {
            if method == "__aiter__" {
                return Ok(receiver);
            }
            if method == "athrow" && args.len() > 2 {
                self.emit_builtin_warning(Kind::DeprecationWarning, "the (type, exc, tb) signature of athrow() is deprecated, use the single-arg signature instead.", 1)?;
            }
            if let Value::AsyncGenerator {
                finalizer: stored @ None,
                ..
            } = self.heap.get_mut(receiver)
            {
                let [firstiter, finalizer] = self.threads.asyncgen_hooks;
                *stored = Some(finalizer);
                self.heap.register_finalizer(receiver);
                if firstiter != NONE {
                    self.call(firstiter, vec![receiver], Vec::new())?;
                }
            }
            let request = match method {
                "__anext__" => Request::Send(NONE),
                "asend" => Request::Send(args[1]),
                "athrow" => Request::Throw(self.alloc(Value::Tuple(super::copy_ids(&args[1..])?))),
                "aclose" => Request::Close,
                _ => unreachable!("async generator method"),
            };
            let operation = self.alloc(Value::AsyncGeneratorOp(Operation {
                generator: receiver,
                request,
                state: State::New,
            }));
            self.heap.register_finalizer(operation);
            return Ok(operation);
        }
        match method {
            "__iter__" | "__await__" => Ok(receiver),
            "__next__" => self.drive_async_generator_op(receiver, NONE, None),
            "send" => self.drive_async_generator_op(receiver, args[1], None),
            "throw" => self.drive_async_generator_op(receiver, NONE, Some(&args[1..])),
            "close" => {
                if matches!(self.heap.get(receiver), Value::AsyncGeneratorOp(op) if op.state == State::Done)
                {
                    return Ok(NONE);
                }
                let exit = self.builtin("GeneratorExit");
                match self.drive_async_generator_op(receiver, NONE, Some(&[exit])) {
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
                }
            }
            _ => unreachable!("async generator awaitable method"),
        }
    }

    /// Advance one operation while retaining exclusive ownership through intermediate awaits.
    pub(super) fn drive_async_generator_op(
        &mut self,
        id: Id,
        sent: Id,
        thrown: Option<&[Id]>,
    ) -> Result<Id> {
        let Value::AsyncGeneratorOp(op) = *self.heap.get(id) else {
            unreachable!()
        };
        let Value::AsyncGenerator {
            frame,
            running,
            closed,
            ..
        } = *self.heap.get(op.generator)
        else {
            unreachable!()
        };
        if op.state == State::Done {
            let label = if matches!(op.request, Request::Send(_)) {
                "__anext__()/asend()"
            } else {
                "aclose()/athrow()"
            };
            return Err(Error::new(
                "RuntimeError",
                format!("cannot reuse already awaited {label}"),
            ));
        }
        if op.state == State::New {
            let finished = matches!(self.heap.get(frame), Value::Generator { offset: None, .. });
            if thrown.is_none() && finished && !matches!(op.request, Request::Send(_)) {
                self.set_async_op_state(id, State::Done);
                return Err(self.exception("StopIteration", &[]));
            }
            if running {
                self.set_async_op_state(id, State::Done);
                let name = match op.request {
                    Request::Send(_) => "anext",
                    Request::Close => "aclose",
                    Request::Throw(_) => "athrow",
                };
                return Err(Error::new(
                    "RuntimeError",
                    format!("{name}(): asynchronous generator is already running"),
                ));
            }
            if thrown.is_none() && !matches!(op.request, Request::Send(_)) {
                if closed {
                    self.set_async_op_state(id, State::Done);
                    return Err(self.exception("StopAsyncIteration", &[]));
                }
                if sent != NONE {
                    return Err(Error::new(
                        "RuntimeError",
                        "can't send non-None value to a just-started coroutine",
                    ));
                }
            }
            self.set_async_op_state(id, State::Running);
            let Value::AsyncGenerator {
                running, closed, ..
            } = self.heap.get_mut(op.generator)
            else {
                unreachable!()
            };
            *running = true;
            if thrown.is_none() && matches!(op.request, Request::Close) {
                *closed = true;
            }
        }

        if let Request::Throw(args) = op.request
            && op.state == State::New
            && thrown.is_none()
        {
            let Value::Tuple(args) = self.heap.get(args) else {
                unreachable!()
            };
            let count = args.len();
            if count == 0 || count > 3 {
                let limit = if count == 0 {
                    "at least 1 argument"
                } else {
                    "at most 3 arguments"
                };
                return Err(Error::new(
                    "TypeError",
                    format!("athrow expected {limit}, got {count}"),
                ));
            }
        }
        let result = self.with_slot_depth("maximum recursion depth exceeded", |ctx| {
            if let Some(args) = thrown {
                return ctx.throw_into_frame(frame, args, true);
            }
            if op.state == State::New {
                match op.request {
                    Request::Send(initial) if sent == NONE => {
                        return ctx.send_to_frame(frame, initial);
                    }
                    Request::Throw(args) => {
                        let Value::Tuple(args) = ctx.heap.get(args) else {
                            unreachable!()
                        };
                        let args = super::copy_ids(args)?;
                        return ctx.throw_into_frame(frame, &args, false);
                    }
                    Request::Close => {
                        let exit = ctx.builtin("GeneratorExit");
                        return ctx.throw_into_frame(frame, &[exit], false);
                    }
                    Request::Send(_) => {}
                }
            }
            ctx.send_to_frame(frame, sent)
        });

        let closing = matches!(op.request, Request::Close);
        let result = match result {
            Ok(Some(yielded)) => match *self.heap.get(yielded) {
                Value::AsyncGeneratorYield(_) if closing => Err(Error::new(
                    "RuntimeError",
                    "async generator ignored GeneratorExit",
                )),
                Value::AsyncGeneratorYield(value) => Err(self.exception("StopIteration", &[value])),
                _ => return Ok(yielded),
            },
            Ok(None) => Err(self.exception(
                if closing {
                    "StopIteration"
                } else {
                    "StopAsyncIteration"
                },
                &[],
            )),
            Err(error)
                if closing
                    && (self.is_builtin_error(&error, "GeneratorExit")
                        || self.is_builtin_error(&error, "StopAsyncIteration")) =>
            {
                Err(self.exception("StopIteration", &[]))
            }
            Err(error) => Err(error),
        };
        let Value::AsyncGenerator { running, .. } = self.heap.get_mut(op.generator) else {
            unreachable!()
        };
        *running = false;
        if matches!(
            self.heap.get(frame),
            Value::Generator {
                offset: None,
                running: false,
                ..
            }
        ) {
            self.heap.disarm_generator_finalizer(op.generator);
        }
        self.set_async_op_state(id, State::Done);
        result
    }

    pub(super) fn warn_unawaited_async_generator_op(&mut self, op: Operation) {
        let Value::AsyncGenerator { frame, .. } = *self.heap.get(op.generator) else {
            unreachable!()
        };
        let Value::Generator { qualname, .. } = *self.heap.get(frame) else {
            unreachable!()
        };
        let method = match op.request {
            Request::Send(_) => "asend",
            Request::Throw(_) => "athrow",
            Request::Close => "aclose",
        };
        let message = format!(
            "coroutine method '{method}' of {} was never awaited",
            self.repr(qualname)
        );
        if let Err(error) =
            self.emit_builtin_warning_with_source(Kind::RuntimeWarning, &message, 1, op.generator)
        {
            super::unraisable::report(
                self,
                error,
                false,
                Some(op.generator),
                "Exception ignored while finalizing async generator",
            );
        }
    }

    fn set_async_op_state(&mut self, id: Id, state: State) {
        self.heap.disarm_generator_finalizer(id);
        let Value::AsyncGeneratorOp(op) = self.heap.get_mut(id) else {
            unreachable!()
        };
        op.state = state;
    }
}
