//! Checked exception dispatch and protected control transfers.

use super::{CheckedBody, Entry};
use crate::ir::{ExceptHandler, ExceptionMatcher, Expr, ExprKind, Statement, Type};
use crate::llvm::{Completion, Value};

/// The native frame owns the exception pointer. The registered frame roots a
/// Python handle across the handler chain.
enum Caught {
    Native(Value),
    Registered(String),
}

impl CheckedBody<'_, '_, '_, '_> {
    /// Leave the active cleanups before branching to the target's lexical depth.
    pub(super) fn jump_from_regions(&mut self, label: String, depth: usize) {
        match &mut self.0 {
            Entry::Native(e) => e.route(Completion::Jump { label, depth }),
            Entry::Registered(f) => f.jump_from_regions(label, depth),
        }
    }

    /// Resume the saved action when cleanup falls through. The final destination
    /// is unconditional, including when all predecessors recorded the same action.
    pub(in crate::llvm) fn resume_completions<T>(
        &mut self,
        targets: &[Completion<T>],
        tag: &str,
        mut resume: impl FnMut(&mut Self, &Completion<T>),
    ) {
        let (last, tested) = targets.split_last().expect("reachable finalbody");
        for (index, target) in tested.iter().enumerate() {
            let test = format!("icmp eq {tag}, {index}");
            let selected = match &mut self.0 {
                Entry::Native(code) => code.instruction(test),
                Entry::Registered(frame) => frame.value(&test),
            };
            self.when(&selected, |code| resume(code, target));
        }
        resume(self, last);
    }

    pub(super) fn try_statement(
        &mut self,
        body: &[Statement],
        handlers: &[ExceptHandler],
        otherwise: &[Statement],
        finalbody: &[Statement],
    ) {
        let catches_error = !handlers.is_empty() || !finalbody.is_empty();
        let run = |code: &mut CheckedBody<'_, '_, '_, '_>, exit| {
            code.try_except(body, handlers, otherwise, catches_error, exit);
        };
        match &mut self.0 {
            Entry::Native(code) => {
                if handlers.is_empty()
                    && otherwise.is_empty()
                    && finalbody.iter().all(|statement| {
                        matches!(
                            statement,
                            Statement::Expr(Expr {
                                kind: ExprKind::None,
                                ..
                            })
                        )
                    })
                {
                    return code.block(body);
                }
                code.with_try_cleanups(finalbody, |code, done, depth| {
                    run(&mut CheckedBody::native(code), Some((done, depth)));
                });
            }
            Entry::Registered(frame) if !finalbody.is_empty() => {
                frame.finalized(finalbody, |frame| {
                    run(&mut CheckedBody::registered(frame.reborrow()), None);
                });
            }
            Entry::Registered(_) => run(self, None),
        }
    }

    /// Emit the protected and else suites, then consume the pending exception
    /// in source order. Handler and else failures use the enclosing error target.
    fn try_except(
        &mut self,
        body: &[Statement],
        handlers: &[ExceptHandler],
        otherwise: &[Statement],
        catches_error: bool,
        native_exit: Option<(String, usize)>,
    ) {
        if handlers.is_empty() && native_exit.is_none() {
            // The registered finally already installed its landing pad.
            self.emit_block(body);
            return;
        }
        let dispatch = self.block_name();
        self.protected_suites(body, otherwise, catches_error.then_some(&dispatch));
        let (caught, done, depth) = if let Some((done, depth)) = native_exit {
            if !self.body().terminated {
                self.jump_from_regions(done.clone(), depth);
            }
            let code = self.fixed();
            if !catches_error {
                return;
            }
            code.start(&dispatch);
            if handlers.is_empty() {
                code.route(Completion::Error);
                return;
            }
            let exception = code.call("imre_error_take", Type::Exception, &[]);
            (Caught::Native(exception), done, depth)
        } else {
            let (exception, done, depth) = self.python().enter_handlers(&dispatch);
            (Caught::Registered(exception), done, depth)
        };
        if let Caught::Registered(exception) = &caught
            && matches!(handlers[0].matcher, ExceptionMatcher::Subgroup(..))
        {
            self.python().groups(handlers, exception, &done);
        } else if self.ordinary_handlers(handlers, &caught, &done, depth) {
            match &caught {
                Caught::Native(exception) => {
                    let code = self.fixed();
                    code.void_call("imre_error_set", &[exception]);
                    code.release(exception);
                    code.route(Completion::Error);
                }
                Caught::Registered(exception) => self.python().raise(exception, None, true),
            }
        }
        if let Caught::Registered(_) = caught {
            self.python().leave_handlers(&done);
        }
    }

    fn handler_target(&mut self) -> &mut String {
        match &mut self.0 {
            Entry::Native(code) => &mut code.body.error_target,
            Entry::Registered(frame) => &mut frame.body.error_target,
        }
    }

    /// Errors in the try suite enter its handler. The else suite runs with the
    /// enclosing target restored, including its fixed-frame cleanup depth.
    fn protected_suites(
        &mut self,
        body: &[Statement],
        otherwise: &[Statement],
        handler: Option<&str>,
    ) {
        let enclosing = handler.map(|handler| {
            let previous = std::mem::replace(self.handler_target(), handler.into());
            let depth = match &mut self.0 {
                Entry::Native(code) => Some(std::mem::replace(
                    &mut code.handler_depth,
                    code.cleanups.len(),
                )),
                Entry::Registered(_) => None,
            };
            (previous, depth)
        });
        self.emit_block(body);
        if let Some((previous, depth)) = enclosing {
            *self.handler_target() = previous;
            if let Some(depth) = depth {
                self.fixed().handler_depth = depth;
            }
        }
        self.emit_block(otherwise);
    }

    /// Enter the handler's binding, emit its checked suite, then unwind a normal
    /// exit through the same lexical cleanup as an explicit loop transfer.
    fn caught_arm(&mut self, handler: &ExceptHandler, caught: &Caught, done: &str, depth: usize) {
        let binding = match caught {
            Caught::Native(exception) => {
                self.fixed().catch_exception(handler.name, exception);
                None
            }
            Caught::Registered(exception) => self.python().bind_handler_name(handler, exception),
        };
        self.emit_block(&handler.body);
        if !self.body().terminated {
            self.jump_from_regions(done.to_owned(), depth);
        }
        if let Some(failed) = binding {
            self.python().leave_handler_name(&failed);
        } else if let Entry::Native(code) = &mut self.0 {
            code.cleanups.pop();
        }
    }

    /// Emit each match and its handler once. A bare handler consumes the exception
    /// unconditionally, so only a chain of conditional handlers can propagate it.
    fn ordinary_handlers(
        &mut self,
        handlers: &[ExceptHandler],
        caught: &Caught,
        done: &str,
        depth: usize,
    ) -> bool {
        let exception = match caught {
            Caught::Native(value) => &value.operand,
            Caught::Registered(handle) => handle,
        };
        let pending = match &self.0 {
            Entry::Native(code) => code.temporaries.clone(),
            Entry::Registered(_) => Vec::new(),
        };
        for handler in handlers {
            let condition = match &handler.matcher {
                ExceptionMatcher::Any => {
                    self.caught_arm(handler, caught, done, depth);
                    return false;
                }
                ExceptionMatcher::Categories(kinds) => {
                    let Some((&first, rest)) = kinds.split_first() else {
                        continue;
                    };
                    let code = self.fixed();
                    let mut matched = code.exception_matches(exception, first);
                    for &kind in rest {
                        let next = code.exception_matches(exception, kind);
                        matched = code.instruction(format!("or i1 {matched}, {next}"));
                    }
                    matched
                }
                ExceptionMatcher::Object(site, class) => {
                    let frame = self.python();
                    let class = frame.handler_class(*site, class);
                    let matched = frame.op("matches", &[exception, &class]);
                    frame.value(&format!("icmp eq i64 {matched}, 2"))
                }
                ExceptionMatcher::Subgroup(..) => unreachable!("subgroups use group dispatch"),
            };
            self.when(&condition, |code| {
                code.caught_arm(handler, caught, done, depth)
            });
            if let Entry::Native(code) = &mut self.0 {
                code.temporaries.clone_from(&pending);
            }
        }
        true
    }
}
