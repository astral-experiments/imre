//! The boxed emitter lowers each suspended body to an LLVM coroutine. Its heap frame
//! contains machine spills; the Python activation tracks live handles at checkpoints.
use crate::Shared;
use std::alloc::Layout;
use std::cell::{Cell, RefCell, UnsafeCell};
use std::ffi::c_void;
use std::ptr::NonNull;

use super::generator_protocol::{DelegateOutcome, GeneratorInput};
use super::value::{
    Body, Frame, FrameOwner, ResumeCode, TracebackAnchor, TracebackSpan, VisitReferences,
};
use super::{Context, Error, Id, NONE, Result, Value};

enum Step {
    Yield(Id, usize),
    YieldFrom(Id, usize),
    AwaitIterator(Id, usize),
    Return(Id),
}

type Drive = unsafe extern "C" fn(*mut c_void, bool);

/// The generated entry reads the current Context from the first word on every
/// resume. It never owns a Rust Context borrow while the activation is suspended.
#[repr(C)]
pub(super) struct Activation {
    context: *mut Context,
    input: Option<Result<Id>>,
    step: Option<Step>,
    frame: *mut c_void,
    drive: Option<Drive>,
    allocation: Option<(NonNull<u8>, Layout)>,
}

struct Continuation {
    entry: ResumeCode,
    scope: Id,
    delegate_root: Cell<Option<usize>>,
    // LLVM may keep this address until destruction. No borrow spans a generated call.
    activation: UnsafeCell<Activation>,
    parked: RefCell<Option<Frame>>,
}

#[derive(Clone)]
pub(super) struct Saved(Shared<Continuation>);

impl Saved {
    fn new(entry: ResumeCode, scope: Id) -> Result<Self> {
        let state = Continuation {
            entry,
            scope,
            delegate_root: Cell::new(None),
            activation: UnsafeCell::new(Activation {
                context: std::ptr::null_mut(),
                input: None,
                step: None,
                frame: std::ptr::null_mut(),
                drive: None,
                allocation: None,
            }),
            parked: RefCell::new(None),
        };
        Ok(Self(
            Shared::try_new(state).map_err(|_| crate::Error::allocation())?,
        ))
    }

    fn resume(&self, ctx: &mut Context, input: Result<Id>) -> Result<Step> {
        let activation = self.0.activation.get();
        // SAFETY: the running guard allows only this resume, and the owner keeps the
        // embedded activation at a stable address until the native frame is destroyed.
        unsafe {
            (*activation).context = ctx;
            (*activation).input = Some(input);
            if let Some(drive) = (*activation).drive {
                drive((*activation).frame, false);
            } else {
                (self.0.entry)(activation, self.0.scope);
            }
            (*activation).context = std::ptr::null_mut();
            (*activation).input = None;
            let step = (*activation).step.take();
            if let Some(error) = ctx.heap.execution.pending.take() {
                return Err(error);
            }
            step.ok_or_else(|| Error::new("SystemError", "compiled coroutine returned no result"))
        }
    }
}

impl Drop for Continuation {
    fn drop(&mut self) {
        // SAFETY: the running callback has returned. The compiler-generated destroy
        // edge only tears down its machine frame. Python finally suites are driven
        // by close/throw or the ordinary finalizer before disposal.
        unsafe {
            let data = self.activation.get();
            if let Some(drive) = (*data).drive.take() {
                drive((*data).frame, true);
            }
            if let Some((memory, layout)) = (*data).allocation.take() {
                std::alloc::dealloc(memory.as_ptr(), layout);
            }
        }
    }
}

impl Saved {
    /// The current delegated iterator is a strong operand only while its frame is parked.
    pub(super) fn parked_delegate(&self) -> Option<Id> {
        let parked = self.0.parked.borrow();
        parked.as_ref()?.operand(self.0.delegate_root.get()?)
    }

    /// Recheck at most the caller's remaining budget of strong roots while parked.
    pub(super) fn any_root(&self, limit: usize, predicate: impl FnMut(Id) -> bool) -> bool {
        self.0
            .parked
            .borrow()
            .as_ref()
            .is_some_and(|frame| frame.roots().take(limit).any(predicate))
    }

    /// Number of strong edges owned by the parked native frame.
    pub(super) fn root_count(&self) -> usize {
        self.0
            .parked
            .borrow()
            .as_ref()
            .map_or(0, |frame| frame.roots().count())
    }

    /// Recheck one parked root while discovering or validating a bounded ownership path.
    pub(super) fn root_at(&self, index: usize) -> Option<Id> {
        self.0.parked.borrow().as_ref()?.roots().nth(index)
    }

    pub(super) fn visit(&self, visitor: &mut impl VisitReferences) {
        if let Some(activation) = self.0.parked.borrow().as_ref() {
            activation.visit(visitor);
        }
    }
}

impl Context {
    pub(super) fn resume_native(
        &mut self,
        generator: Id,
        owner: Id,
        mut input: GeneratorInput,
    ) -> Result<Option<Id>> {
        input.keep(&mut self.heap, owner)?;
        let Value::Generator {
            code,
            scope,
            offset,
            running,
            ..
        } = self.heap.get(generator)
        else {
            unreachable!()
        };
        if *running {
            return Err(Error::new("ValueError", "generator already executing"));
        }
        if offset.is_none() {
            return Ok(None);
        }
        let (code, scope) = (*code, *scope);
        // Reserve the resumer's outgoing value slot before taking its parked frame.
        self.heap.reserve_temporaries(1)?;
        let Value::Generator { native, .. } = self.heap.get_mut(generator) else {
            unreachable!()
        };
        let saved = if let Some(saved) = native.take() {
            let frame = saved.0.parked.borrow_mut().take().expect("suspended frame");
            self.heap.restore_native_frame(frame);
            saved
        } else {
            let Body::Suspended(entry) = self.code_object(code).body else {
                unreachable!()
            };
            let saved = Saved::new(entry, scope)?;
            self.heap
                .enter(scope, code, FrameOwner::Generator(generator));
            saved
        };
        let Value::Generator { running, .. } = self.heap.get_mut(generator) else {
            unreachable!()
        };
        *running = true;
        self.heap.disarm_finalizer(owner);
        let mut external_throw = matches!(input, GeneratorInput::Throw { .. });
        let result = loop {
            let mut arg = if let Some(mark) = saved.0.delegate_root.get() {
                let iter = *self.heap.temporary_root(mark);
                // throw/close forwards to a suspended delegate before the
                // delegating handler becomes active again.
                let handled = external_throw.then(|| {
                    let inherited = self.heap.caught_exception(NONE);
                    let handled = self.heap.replace_active(inherited);
                    self.heap.keep(handled);
                    handled
                });
                let result = self.advance_delegate(iter, input);
                if let Some(handled) = handled {
                    self.heap.replace_active(handled);
                }
                let result = match result {
                    Ok(DelegateOutcome::Yield(value)) => {
                        return self.park_native(generator, owner, saved, mark + 1, Ok(value));
                    }
                    Ok(DelegateOutcome::LookupFailed(error)) => {
                        return self.park_native(generator, owner, saved, mark + 1, Err(error));
                    }
                    Ok(DelegateOutcome::Done(value)) => Ok(value),
                    Err(error) => Err(error),
                };
                saved.0.delegate_root.set(None);
                self.release_with_result(mark, result)
            } else {
                match input {
                    GeneratorInput::Send(value) => Ok(value),
                    GeneratorInput::Throw { error, .. } => Err(error),
                }
            };
            if external_throw && let Err(error) = &mut arg {
                self.chain_generator_input(error);
            }
            external_throw = false;
            let (iterator, mark) = match saved.resume(self, arg) {
                Ok(Step::Yield(value, mark)) => {
                    return self.park_native(generator, owner, saved, mark, Ok(value));
                }
                Ok(Step::YieldFrom(value, mark)) => (self.iterator(value), mark),
                Ok(Step::AwaitIterator(iter, mark)) => (Ok(iter), mark),
                Ok(Step::Return(value)) => break Ok(value),
                Err(error) => break Err(error),
            };
            input = match self.release_with_result(mark, iterator) {
                Ok(_) => {
                    saved.0.delegate_root.set(Some(mark));
                    GeneratorInput::Send(NONE)
                }
                Err(error) => GeneratorInput::Throw {
                    error,
                    args: Vec::new(),
                },
            };
        };
        let completed = result.map_err(|error| self.frame_error(error, code));
        self.heap.discard_generator_frame(generator);
        let value = match completed {
            Ok(value) => value,
            Err(error) => {
                self.heap.execution.pending = Some(error);
                NONE
            }
        };
        self.leave_frame(value, self.heap.execution.pending.is_none());
        if let Some(error) = self.heap.execution.pending.take() {
            return Err(error);
        }
        match value {
            NONE => Ok(None),
            value => Err(self.exception("StopIteration", &[value])),
        }
    }

    fn park_native(
        &mut self,
        generator: Id,
        owner: Id,
        saved: Saved,
        mark: usize,
        result: Result<Id>,
    ) -> Result<Option<Id>> {
        // Operand release can run Python. Publish to the resumer first and keep
        // the generator running until its remaining frame is ready to suspend.
        let result = self.heap.publish_to_resumer(result);
        if let (Some(delegate), Ok(value)) = (saved.0.delegate_root.get(), &result)
            && !self.workers.has_unreaped_workers()
            && self.heap.forwarded_roots_survive(delegate, mark, *value)
        {
            self.heap.release_temporaries_since(mark);
        } else {
            self.discard_temporaries(mark, None);
        }
        let frame = self.heap.suspend_native_frame();
        *saved.0.parked.borrow_mut() = Some(frame);
        let Value::Generator {
            code,
            running,
            offset,
            native,
            ..
        } = self.heap.get_mut(generator)
        else {
            unreachable!()
        };
        *native = Some(saved);
        *running = false;
        *offset = Some(1);
        let code = *code;
        // An async generator's public wrapper owns its first-iteration and finalization hooks.
        if self.code_object(code).definition.flags & imre_ast::code_flags::ASYNC_GENERATOR == 0 {
            self.heap.register_finalizer(owner);
        }
        result.map(Some)
    }
}

/// Reserve exactly the frame layout computed by LLVM for this definition.
#[unsafe(no_mangle)]
pub(super) unsafe extern "C" fn imre_dyn_coro_alloc(
    activation: *mut Activation,
    size: u64,
    alignment: u32,
) -> *mut c_void {
    // SAFETY: the compiler requests one allocation from the running activation.
    let data = unsafe { &mut *activation };
    let layout = usize::try_from(size)
        .ok()
        .and_then(|size| Layout::from_size_align(size.max(1), alignment as usize).ok());
    if let Some(layout) = layout
        && data.allocation.is_none()
        // SAFETY: from_size_align validated this nonempty LLVM frame layout.
        && let Some(memory) = NonNull::new(unsafe { std::alloc::alloc(layout) })
    {
        data.allocation = Some((memory, layout));
        return memory.as_ptr().cast();
    }
    // SAFETY: resume installed the active Context for the duration of this entry.
    unsafe { &mut *data.context }.heap.execution.pending = Some(crate::Error::allocation().into());
    std::ptr::null_mut()
}

#[unsafe(no_mangle)]
pub(super) unsafe extern "C" fn imre_dyn_coro_open(
    activation: *mut Activation,
    frame: *mut c_void,
    drive: Drive,
) {
    // SAFETY: the generated entry supplies its initialized LLVM frame and its
    // matching C calling-convention adapter for resume and destroy.
    let data = unsafe { &mut *activation };
    data.frame = frame;
    data.drive = Some(drive);
}

#[unsafe(no_mangle)]
pub(super) unsafe extern "C" fn imre_dyn_suspend_operands(
    activation: *mut Activation,
    value: Id,
    mark: usize,
    kind: u32,
) {
    // SAFETY: only the running generated entry publishes its next request.
    let data = unsafe { &mut *activation };
    data.step = Some(match kind {
        0 => Step::Yield(value, mark),
        1 => {
            // The async generator operation unwraps an item, while a delegated
            // await suspension returns its actual event to the event loop.
            // SAFETY: the running activation borrows this context.
            let item = unsafe { &mut *data.context }.wrap_async_generator_yield(value);
            Step::Yield(item, mark)
        }
        2 => Step::AwaitIterator(value, mark),
        3 => Step::YieldFrom(value, mark),
        _ => {
            // SAFETY: this callback runs within the activation that borrowed the Context.
            unsafe { &mut *data.context }.heap.execution.pending =
                Some(Error::new("SystemError", "invalid suspension operation"));
            return;
        }
    });
}

#[unsafe(no_mangle)]
pub(super) unsafe extern "C" fn imre_dyn_suspend_return(activation: *mut Activation, value: Id) {
    // SAFETY: the running generated entry publishes its Python return value.
    unsafe { (*activation).step = Some(Step::Return(value)) };
}

#[unsafe(no_mangle)]
pub(super) unsafe extern "C" fn imre_dyn_resume_input(activation: *mut Activation) -> Id {
    // SAFETY: the resumer has installed its context and input before running
    // this edge. An injected exception is attributed to the parked expression.
    let data = unsafe { &mut *activation };
    // SAFETY: the activation points to the exclusive Context of this resume call.
    let ctx = unsafe { &mut *data.context };
    let result = data
        .input
        .take()
        .expect("compiled frame has a resume input");
    let position = ctx.heap.take_call_position();
    let result = result.map_err(|mut error| {
        if let Some([line, end_line, start, end]) = position {
            error.tracing().source_position = Some((
                line,
                TracebackSpan {
                    end_line,
                    start,
                    end,
                    anchor: TracebackAnchor::Uniform,
                },
            ));
        }
        error
    });
    super::ffi::finish(ctx, result)
}
