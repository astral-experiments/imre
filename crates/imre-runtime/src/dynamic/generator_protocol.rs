use super::source_code::{DelegateOutcome, GeneratorInput};
use super::value::{Body, FrameOwner, Location};
use super::{Context, Error, Id, NONE, Result, Value};

impl Context {
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
                    Err(other) => eprintln!("Exception ignored while closing generator: {other}"),
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

    /// Discard a never-entered frame or a compiled comprehension with no exception handlers.
    fn discard_generator_frame(&mut self, id: Id) {
        let Value::Generator {
            offset,
            owner,
            frame,
            scope,
            iterators,
            ..
        } = self.heap.get_mut(id)
        else {
            unreachable!("checked generator");
        };
        *offset = None;
        *owner = FrameOwner::Module(NONE);
        *frame = None;
        *scope = NONE;
        iterators.clear();
        self.heap.disarm_generator_finalizer(id);
    }

    pub(super) fn generator_throw(&mut self, id: Id, args: &[Id]) -> Result<Id> {
        let mut error = self.generator_exception(args)?;
        let Value::Generator {
            code,
            scope,
            offset,
            running,
            ..
        } = *self.heap.get(id)
        else {
            return Err(Error::new(
                "TypeError",
                "generator.throw requires a generator",
            ));
        };
        if running {
            return Err(Error::new("ValueError", "generator already executing"));
        }
        if offset.is_none() {
            return Err(error);
        }
        if offset == Some(0) {
            // Throwing into a fresh generator unwinds at its definition before any body instruction.
            let definition = self.code_object(code);
            let location = Location {
                file: self.str_value(definition.filename)?.to_owned().into(),
                line: definition.first_line,
                span: None,
            };
            self.heap.enter(scope, code, FrameOwner::Generator(id));
            self.heap.publish_statement(&[], location, NONE);
            self.record_exception_frame(&mut error, None);
            self.heap.leave(NONE, false);
            self.discard_generator_frame(id);
            return Err(error);
        }
        if matches!(self.code_object(code).body, Body::Native(_)) {
            self.discard_generator_frame(id);
            return Err(self.generator_error(error));
        }
        self.resume_source_input(
            id,
            GeneratorInput::Throw {
                error,
                args: super::copy_ids(args)?,
            },
        )?
        .ok_or_else(|| self.exception("StopIteration", &[]))
    }

    pub(super) fn generator_close(&mut self, id: Id) -> Result<Id> {
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
            return Err(Error::new("ValueError", "generator already executing"));
        }
        if offset.is_none() {
            return Ok(NONE);
        }
        if offset == Some(0) || matches!(self.code_object(code).body, Body::Native(_)) {
            self.discard_generator_frame(id);
            return Ok(NONE);
        }
        let exit = self.exception("GeneratorExit", &[]);
        match self.resume_source_input(
            id,
            GeneratorInput::Throw {
                error: exit,
                args: Vec::new(),
            },
        ) {
            Ok(None) => Ok(NONE),
            Ok(Some(_)) => Err(Error::new(
                "RuntimeError",
                "generator ignored GeneratorExit",
            )),
            Err(error) if self.is_builtin_error(&error, "GeneratorExit") => Ok(NONE),
            Err(error) if self.is_stop_iteration(&error) => Ok(error.stop_value()),
            Err(error) => Err(error),
        }
    }
}
