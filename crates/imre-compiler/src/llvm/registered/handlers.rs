//! Exception handlers entered through a registered Python frame.

use super::{Frame, RegisteredFrame, context::Region};
use crate::ir::{ExceptHandler, ExceptionMatcher, Statement};
use crate::llvm::operations::CheckedBody;
use crate::llvm::{Completion, finally::Finalbody};

impl Finalbody<String> {
    /// The actual exception on the failing predecessor, or None on an ordinary transfer.
    pub(super) fn pending_error(&self, frame: &mut RegisteredFrame<'_, '_>) -> String {
        self.phi(&mut frame.body, "i64", |edge| {
            edge.exception
                .as_ref()
                .map_or_else(|| "0".into(), |(error, _)| error.clone())
        })
    }
}

impl RegisteredFrame<'_, '_> {
    /// The normal else edge skips the handlers. On the error edge, keep the
    /// pending exception live and active until handler dispatch is complete.
    pub(in crate::llvm) fn enter_handlers(&mut self, handler: &str) -> (String, String, usize) {
        let done = self.name("b");
        self.jump(&done);
        self.label(handler);
        let exception = self.op("take_error", &[]);
        self.live.push(exception.clone());
        self.caught.push(exception.clone());
        (exception, done, self.regions.len())
    }

    pub(in crate::llvm) fn leave_handlers(&mut self, done: &str) {
        self.caught.pop();
        self.live.pop();
        self.label(done);
    }
}

impl<'f, 'p> Frame<'f, '_, 'p> {
    /// Early transfers and fallthrough in a protected body enter one lexical cleanup.
    pub(super) fn protected_exits(
        &mut self,
        failed: &str,
        done: &str,
        protected: impl FnOnce(&mut Self),
    ) -> (Finalbody<String>, bool) {
        let entry = self.name("b");
        let depth = self.regions.len();
        let enclosing = std::mem::replace(&mut self.body.error_target, failed.to_owned());
        self.regions.push(Region::Finally(Finalbody::new(entry)));
        protected(self);
        let Some(Region::Finally(mut region)) = self.regions.pop() else {
            unreachable!("active finally region")
        };
        self.body.error_target = enclosing;
        let normal = !self.body.terminated;
        if normal {
            self.flush_check();
            region.enter(
                &mut self.body,
                Completion::Jump {
                    label: done.to_owned(),
                    depth,
                },
                None,
            );
        }
        (region, normal)
    }

    /// Keep the pending return across cleanup, then resume the original transfer.
    /// A Python context handles its exceptional exit separately because it can suppress it.
    pub(super) fn finish_exits<T>(
        &mut self,
        region: Finalbody<String>,
        cleanup: impl FnOnce(&mut Self, &Finalbody<String>, String) -> T,
        mut finish: impl FnMut(&mut RegisteredFrame<'_, '_>, &Completion<()>, &T),
    ) -> bool {
        if region.is_empty() {
            return false;
        }
        self.label(&region.entry);
        let error = region.pending_error(self);
        let (targets, tag) = region.completions(&mut self.body, Some(&error), |_| ());
        let returned = targets.contains(&Completion::Return(())).then(|| {
            region.phi(&mut self.body, "i64", |edge| match &edge.completion {
                Completion::Return(value) => value.clone(),
                _ => "0".into(),
            })
        });
        let live = self.live.len();
        let depth = self.regions.len();
        if let Some(value) = &returned {
            self.keep_pending_exit(value, None);
        }
        let result = cleanup(self, &region, error);
        self.live.truncate(live);
        self.regions.truncate(depth);
        if self.body.terminated {
            return false;
        }
        CheckedBody::registered(self.reborrow()).resume_completions(
            &targets,
            &tag,
            |body, target| {
                let frame = body.python();
                finish(frame, target, &result);
                match target {
                    Completion::Return(()) => {
                        frame.return_from_regions(returned.as_ref().unwrap().clone())
                    }
                    Completion::Jump { label, depth } => {
                        frame.jump_from_regions(label.clone(), *depth)
                    }
                    Completion::Error => {}
                }
            },
        );
        true
    }

    pub(in crate::llvm) fn finalized(
        &mut self,
        body: &[Statement],
        protected: impl FnOnce(&mut Self),
    ) {
        let failed = self.name("b");
        let done = self.name("b");
        let (mut region, normal) = self.protected_exits(&failed, &done, protected);
        self.label(&failed);
        let raised = self.op("take_error", &[]);
        let outer = self.caught.last().cloned().unwrap_or_else(|| "0".into());
        self.flush_check();
        region.enter(
            &mut self.body,
            Completion::Error,
            Some((raised, outer.clone())),
        );
        let resumes = self.finish_exits(
            region,
            |frame, region, error| {
                let active = if outer == "0" {
                    error.clone()
                } else {
                    region.phi(&mut frame.body, "i64", |edge| {
                        edge.exception
                            .as_ref()
                            .map_or_else(|| outer.clone(), |(error, _)| error.clone())
                    })
                };
                frame.keep_pending_exit(&error, Some(outer));
                frame.caught.push(active);
                frame.emit_suite(body);
                frame.caught.pop();
                error
            },
            |frame, target, error| {
                if matches!(target, Completion::Error) {
                    frame.raise(error, None, true);
                }
            },
        );
        if normal && resumes {
            self.label(&done);
        }
    }

    fn keep_pending_exit(&mut self, value: &str, enclosing: Option<String>) {
        self.live.push(value.to_owned());
        let outer = self.body.error_target.clone();
        self.regions.push(Region::Pending {
            value: value.to_owned(),
            enclosing,
            outer,
        });
    }

    pub(in crate::llvm) fn bind_handler_name(
        &mut self,
        handler: &ExceptHandler,
        exception: &str,
    ) -> Option<String> {
        let name = self.handler_name(handler)?;
        self.code.bind(name, exception);
        let failed = self.name("b");
        let outer = std::mem::replace(&mut self.body.error_target, failed.clone());
        self.regions.push(Region::ExceptionName {
            name: name.into(),
            outer,
        });
        Some(failed)
    }

    /// Errors raised by the handler clear its binding under the enclosing error target.
    pub(in crate::llvm) fn leave_handler_name(&mut self, failed: &str) {
        let Some(Region::ExceptionName { name, outer }) = self.regions.pop() else {
            unreachable!("named exception handler")
        };
        self.body.error_target = outer;
        self.label(failed);
        let error = self.op("take_error", &[]);
        self.clear_exception_name(&name);
        self.raise(&error, None, true);
    }

    /// Split a pending group in source order and retain both halves until the handlers finish.
    pub(in crate::llvm) fn groups(&mut self, handlers: &[ExceptHandler], exc: &str, done: &str) {
        let raised = self.op("list", &[]);
        self.live.push(raised.clone());
        let roots = self.live.len();
        let mut remainder = exc.to_owned();
        let mut active = exc.to_owned();
        for handler in handlers {
            self.live.extend([remainder.clone(), active.clone()]);
            let matcher = self
                .handler_class(&handler.matcher)
                .expect("except* requires a type");
            let name = self.handler_name(handler);
            let pair = self.op("except_star_split", &[&remainder, &matcher]);
            let zero = self.code.integer(0);
            let matched = self.op("get", &[&pair, &zero]);
            let one = self.code.integer(1);
            remainder = self.op("get", &[&pair, &one]);

            let has_match = self.value(&format!("icmp ne i64 {matched}, 0"));
            // Later match expressions see the last subgroup handled, even across an empty match.
            active = self.value(&format!(
                "select i1 {has_match}, i64 {matched}, i64 {active}"
            ));
            *self.caught.last_mut().unwrap() = active.clone();
            self.live.truncate(roots);
            self.live.extend([remainder.clone(), active.clone()]);

            let body = self.name("b");
            let failed = self.name("b");
            let next = self.name("b");
            self.branch(&has_match, &body, &next);
            self.label(&body);
            self.op("restore_caught", &[&matched]);
            if let Some(name) = name {
                self.code.bind(name, &matched);
            }
            let outer = std::mem::replace(&mut self.body.error_target, failed.clone());
            self.emit_suite(&handler.body);
            self.body.error_target = outer;
            if !self.body.terminated
                && let Some(name) = name
            {
                self.clear_star_target(name);
            }
            self.jump(&next);
            self.label(&failed);
            let error = self.op("take_error", &[]);
            if let Some(name) = name {
                self.clear_star_target(name);
            }
            self.op("append", &[&raised, &error]);
            self.jump(&next);
            self.label(&next);
            self.live.truncate(roots);
        }

        self.op("append", &[&raised, &remainder]);
        let merged = self.op("except_star_merge", &[exc, &raised]);
        self.live.pop();
        let enclosing = self
            .caught
            .iter()
            .rev()
            .nth(1)
            .cloned()
            .unwrap_or_else(|| "0".into());
        self.op("restore_caught", &[&enclosing]);
        let propagate = self.name("b");
        let handled = self.value(&format!("icmp eq i64 {merged}, 0"));
        self.branch(&handled, done, &propagate);
        self.label(&propagate);
        self.raise(&merged, None, true);
    }

    pub(in crate::llvm) fn handler_class(&mut self, matcher: &ExceptionMatcher) -> Option<String> {
        match matcher {
            ExceptionMatcher::Any | ExceptionMatcher::Categories(_) => None,
            ExceptionMatcher::Object(site, class) | ExceptionMatcher::Subgroup(site, class) => {
                self.checkpoint(*site);
                Some(self.evaluate_handle(class))
            }
        }
    }

    fn handler_name(&self, handler: &ExceptHandler) -> Option<&'f str> {
        handler
            .name
            .map(|slot| self.names.expect("registered scope")[&slot].name.as_str())
    }
}

impl RegisteredFrame<'_, '_> {
    /// A custom namespace observes both the None store and the deletion, outside the arm body.
    fn clear_star_target(&mut self, name: &str) {
        self.bind(name, "0");
        let key = self.code_name(name);
        self.op("delname", &[&self.scope.clone(), &key]);
    }

    pub(crate) fn clear_exception_name(&mut self, name: &str) {
        let key = self.code_name(name);
        self.op("clearname", &[&self.scope.clone(), &key]);
    }
}

/// Numeric categories belong to fixed-layout exceptions. Registered catches
/// use live class objects. Both frames handle bare and empty-tuple matchers.
pub(super) fn supported(handlers: &[ExceptHandler], finalbody: &[Statement]) -> bool {
    (!handlers.is_empty() || !finalbody.is_empty())
        && handlers.iter().all(|h| match &h.matcher {
            ExceptionMatcher::Any => true,
            ExceptionMatcher::Categories(kinds) => kinds.is_empty(),
            ExceptionMatcher::Object(..) | ExceptionMatcher::Subgroup(..) => true,
        })
}

impl Frame<'_, '_, '_> {
    pub(crate) fn emit_suite(&mut self, suite: &[Statement]) {
        CheckedBody::registered(self.reborrow()).emit_block(suite);
    }
}
