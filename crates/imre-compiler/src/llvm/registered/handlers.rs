//! Exception handlers entered through a registered Python frame.

use super::{
    Frame, RegisteredFrame,
    context::{Finally, Region},
};
use crate::ir::{ExceptHandler, ExceptionMatcher, Statement};
use crate::llvm::operations::CheckedBody;

impl RegisteredFrame<'_, '_> {
    /// The normal else edge skips the handlers. On the error edge, keep the
    /// pending exception live and active until handler dispatch is complete.
    fn enter_handlers(&mut self, handler: &str) -> (String, String) {
        let done = self.name("b");
        self.jump(&done);
        self.label(handler);
        let exception = self.op("take_error", &[]);
        self.live.push(exception.clone());
        self.caught.push(exception.clone());
        (exception, done)
    }

    fn leave_handlers(&mut self, done: &str) {
        self.caught.pop();
        self.live.pop();
        self.label(done);
    }
}

impl<'f, 'p> Frame<'f, '_, 'p> {
    /// Install the final suite before entering the body so an abrupt exit can find it.
    fn finalized(&mut self, body: Finally<'p>, protected: impl FnOnce(&mut Self)) {
        let failed = self.name("b");
        let done = self.name("b");
        let enclosing = std::mem::replace(&mut self.body.error_target, failed.clone());
        let region = Region::Finally {
            body,
            outer: enclosing.clone(),
            loops: self.loops.len(),
            caught: self.caught.len(),
        };
        self.regions.push(region);
        protected(self);
        let Some(Region::Finally { body, .. }) = self.regions.pop() else {
            unreachable!("active finally region")
        };
        self.body.error_target = enclosing;
        // Module providers also visit unreachable annotation sites in source order.
        let mut needs_join = self.module_annotation_sites.is_some();
        if !self.body.terminated || needs_join {
            self.emit_finally(&body);
            needs_join |= !self.body.terminated;
            self.jump(&done);
        }

        self.label(&failed);
        let exception = self.op("take_error", &[]);
        let outer = self.caught.last().cloned().unwrap_or_else(|| "0".into());
        self.live.push(exception.clone());
        self.caught.push(exception.clone());
        let pending = Region::Pending {
            value: exception.clone(),
            enclosing: Some(outer),
            outer: self.body.error_target.clone(),
        };
        self.regions.push(pending);
        self.emit_finally(&body);
        self.regions.pop();
        self.caught.pop();
        self.live.pop();
        if !self.body.terminated {
            self.raise(&exception, None, true);
        }
        if needs_join {
            self.label(&done);
        }
    }

    /// A named handler clears its binding on every path out of the body.
    fn arm(&mut self, handler: &ExceptHandler, exception: &str, done: &str) {
        let next = self.name("b");
        let (class, name) = self.handler_header(handler);
        if let Some(class) = class {
            let matches = self.op("matches", &[exception, &class]);
            let matches = self.value(&format!("icmp eq i64 {matches}, 2"));
            let matched = self.name("b");
            self.branch(&matches, &matched, &next);
            self.label(&matched);
        }
        if let Some(name) = name {
            self.code.bind(name, exception);
            let failed = self.name("b");
            let enclosing = std::mem::replace(&mut self.body.error_target, failed.clone());
            self.regions.push(Region::ExceptionName {
                name: name.into(),
                outer: enclosing.clone(),
            });
            self.emit_handler_suite(&handler.body);
            self.regions.pop();
            self.body.error_target = enclosing;
            if !self.body.terminated {
                self.clear_exception_name(name);
            }
            self.jump(done);
            self.label(&failed);
            let error = self.op("take_error", &[]);
            self.clear_exception_name(name);
            self.raise(&error, None, true);
        } else {
            self.emit_handler_suite(&handler.body);
            self.jump(done);
        }
        self.label(&next);
    }

    /// Run the else suite outside the protected body, then dispatch any pending exception.
    fn protected(
        &mut self,
        body: &[Statement],
        handlers: &[ExceptHandler],
        otherwise: &[Statement],
        is_star: bool,
    ) {
        if handlers.is_empty() {
            self.emit_handler_suite(body);
            return;
        }
        let (handler, enclosing) = self.begin_operand_scope();
        self.emit_handler_suite(body);
        self.body.error_target = enclosing;
        self.emit_handler_suite(otherwise);
        let (exception, done) = self.enter_handlers(&handler);
        if is_star {
            self.groups(handlers, &exception, &done);
        } else {
            for handler in handlers {
                self.arm(handler, &exception, &done);
            }
            self.raise(&exception, None, true);
        }
        self.leave_handlers(&done);
    }

    /// Split a pending group in source order and retain both halves until the handlers finish.
    fn groups(&mut self, handlers: &[ExceptHandler], exc: &str, done: &str) {
        let raised = self.op("list", &[]);
        self.live.push(raised.clone());
        let roots = self.live.len();
        let mut remainder = exc.to_owned();
        let mut active = exc.to_owned();
        for handler in handlers {
            self.live.extend([remainder.clone(), active.clone()]);
            let (matcher, name) = self.handler_header(handler);
            let matcher = matcher.expect("except* requires a type");
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
            self.emit_handler_suite(&handler.body);
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

    fn handler_header(&mut self, handler: &ExceptHandler) -> (Option<String>, Option<&'f str>) {
        let class = match &handler.matcher {
            ExceptionMatcher::Categories(_) => None,
            ExceptionMatcher::Object(site, class) | ExceptionMatcher::Subgroup(site, class) => {
                self.checkpoint(*site);
                Some(self.evaluate_handle(class))
            }
        };
        let name = handler
            .name
            .map(|slot| self.names.expect("registered scope")[&slot].name.as_str());
        (class, name)
    }

    fn emit_handler_suite(&mut self, body: &[Statement]) {
        // Module providers retain annotation sites even in an unentered else.
        if self.body.terminated
            && let Some(child) = self.prepared_child(body)
        {
            super::RegisteredBody::Suite(child).emit(self.code.prepared_owner());
        } else {
            self.emit_suite(body);
        }
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
/// use live class objects, or the unconditional empty category list.
pub(super) fn supported(handlers: &[ExceptHandler], finalbody: &[Statement]) -> bool {
    (!handlers.is_empty() || !finalbody.is_empty())
        && handlers.iter().all(|h| match &h.matcher {
            ExceptionMatcher::Categories(kinds) => kinds.is_empty(),
            ExceptionMatcher::Object(..) | ExceptionMatcher::Subgroup(..) => true,
        })
}

impl<'p> Frame<'_, '_, 'p> {
    fn prepared_child(&self, body: &[Statement]) -> Option<&'p crate::frontend::RegisteredSuite> {
        let super::FrameInputs::Region(region) = self.inputs else {
            return None;
        };
        region.child_suite(body)
    }

    pub(in crate::llvm) fn try_region(
        &mut self,
        body: &[Statement],
        handlers: &[ExceptHandler],
        otherwise: &[Statement],
        finalbody: &[Statement],
    ) {
        let stars = handlers
            .first()
            .is_some_and(|handler| matches!(handler.matcher, ExceptionMatcher::Subgroup(..)));
        let run = |frame: &mut Self| frame.protected(body, handlers, otherwise, stars);
        if finalbody.is_empty() {
            run(self);
            return;
        }
        // A child can transfer from another prepared scope. Resolve the owned
        // final suite here, before that child changes the active inputs.
        let cleanup = self
            .prepared_child(finalbody)
            .map_or_else(|| Finally::Checked(finalbody.into()), Finally::Prepared);
        self.finalized(cleanup, run);
    }

    pub(crate) fn emit_suite(&mut self, suite: &[Statement]) {
        CheckedBody::registered(self.reborrow()).emit_block(suite);
    }
}
