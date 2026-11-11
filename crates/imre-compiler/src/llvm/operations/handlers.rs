//! Checked exception dispatch and protected control transfers.

use super::{CheckedBody, Entry};
use crate::ir::{ExceptHandler, ExceptionMatcher, Statement};
use crate::llvm::Completion;

impl CheckedBody<'_, '_, '_, '_> {
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
            let test = format!("icmp eq i32 {tag}, {index}");
            let selected = match &mut self.0 {
                Entry::Native(code) => code.instruction(test),
                Entry::Registered(frame) => frame.value(&test),
            };
            let arm = self.block_name();
            let next = self.block_name();
            self.body().branch(&selected, &arm, &next);
            self.body().start(&arm);
            resume(self, target);
            self.body().start(&next);
        }
        resume(self, last);
    }

    fn handler_target(&mut self) -> &mut String {
        match &mut self.0 {
            Entry::Native(code) => &mut code.body.error_target,
            Entry::Registered(frame) => &mut frame.body.error_target,
        }
    }

    fn handler_suite(&mut self, suite: &[Statement]) {
        match &mut self.0 {
            Entry::Native(code) => code.block(suite),
            Entry::Registered(frame) => frame.emit_handler_suite(suite),
        }
    }

    /// Errors in the try suite enter its handler. The else suite runs with the
    /// enclosing target restored, including its fixed-frame cleanup depth.
    pub(in crate::llvm) fn protected_suites(
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
        self.handler_suite(body);
        if let Some((previous, depth)) = enclosing {
            *self.handler_target() = previous;
            if let Some(depth) = depth {
                self.fixed().handler_depth = depth;
            }
        }
        self.handler_suite(otherwise);
    }

    fn matches_exception(&mut self, exception: &str, matcher: &ExceptionMatcher) -> Option<String> {
        match &mut self.0 {
            Entry::Native(code) => {
                let ExceptionMatcher::Categories(kinds) = matcher else {
                    unreachable!("a live Python matcher requires the registered frame")
                };
                let mut matched = None;
                for kind in kinds {
                    let byte = code.instruction(format!(
                        "call i32 @imre_exception_matches(ptr {exception}, i32 {kind})"
                    ));
                    let test = code.instruction(format!("icmp ne i32 {byte}, 0"));
                    matched = Some(match matched {
                        None => test,
                        Some(previous) => code.instruction(format!("or i1 {previous}, {test}")),
                    });
                }
                matched
            }
            Entry::Registered(frame) => {
                let class = frame.handler_class(matcher)?;
                let matches = frame.op("matches", &[exception, &class]);
                Some(frame.value(&format!("icmp eq i64 {matches}, 2")))
            }
        }
    }

    /// Emit each match and its handler once. A bare handler consumes the exception
    /// unconditionally, so only a chain of conditional handlers can propagate it.
    pub(in crate::llvm) fn ordinary_handlers(
        &mut self,
        handlers: &[ExceptHandler],
        exception: &str,
        mut caught: impl FnMut(&mut Self, &ExceptHandler),
    ) -> bool {
        let pending = match &self.0 {
            Entry::Native(code) => code.temporaries.clone(),
            Entry::Registered(_) => Vec::new(),
        };
        for handler in handlers {
            // A Python exception matcher can allocate blocks of its own.
            let next = self.block_name();
            let Some(condition) = self.matches_exception(exception, &handler.matcher) else {
                caught(self, handler);
                return false;
            };
            self.when(&condition, Some(next), |code| caught(code, handler));
            if let Entry::Native(code) = &mut self.0 {
                code.temporaries.clone_from(&pending);
            }
        }
        true
    }
}
