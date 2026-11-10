//! Exception handlers entered through a registered Python frame.

use super::{Frame, FrameEmitter, RegisteredFrame, context::Region};
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

/// Emit the final suite on the normal and exceptional edges. The protected
/// caller installs the same final suite for its lexical return and loop exits.
/// During exceptional cleanup the pending value and enclosing exception can
/// be replaced by a return, break or continue in the final suite itself.
pub(crate) fn finalized<'a, 'p, E: FrameEmitter<'a, 'p>>(
    code: &mut E,
    protected: impl FnOnce(&mut E, &str),
    mut cleanup: impl FnMut(&mut E, Option<(&str, &str)>),
) {
    let failed = code.name("b");
    let done = code.name("b");
    let enclosing = std::mem::replace(&mut code.body.error_target, failed.clone());
    protected(code, &enclosing);
    code.body.error_target = enclosing;
    // Module providers also visit unreachable annotation sites in source order.
    let mut needs_join = code.module_annotation_sites.is_some();
    if !code.body.terminated || needs_join {
        cleanup(code, None);
        needs_join |= !code.body.terminated;
        code.jump(&done);
    }

    code.label(&failed);
    let exception = code.op("take_error", &[]);
    let outer = code.caught.last().cloned().unwrap_or_else(|| "0".into());
    code.live.push(exception.clone());
    code.caught.push(exception.clone());
    cleanup(code, Some((&exception, &outer)));
    code.caught.pop();
    code.live.pop();
    if !code.body.terminated {
        code.raise(&exception, None, true);
    }
    if needs_join {
        code.label(&done);
    }
}

/// A named arm clears its binding on fallthrough, handler failure and abrupt
/// transfer. The caller installs the abrupt-transfer cleanup around its body.
fn arm<'a, 'p, E: FrameEmitter<'a, 'p>, S, N: AsRef<str>>(
    code: &mut E,
    exception: &str,
    done: &str,
    describe: impl FnOnce(&mut E) -> (Option<String>, Option<N>, S),
    emit_suite: impl FnOnce(&mut E, S, Option<(&str, &str)>),
) {
    let next = code.name("b");
    let (class, name, body) = describe(code);
    if let Some(class) = class {
        let matches = code.op("matches", &[exception, &class]);
        let matches = code.value(&format!("icmp eq i64 {matches}, 2"));
        let matched = code.name("b");
        code.branch(&matches, &matched, &next);
        code.label(&matched);
    }
    if let Some(name) = name.as_ref().map(AsRef::as_ref) {
        code.bind(name, exception);
        let failed = code.name("b");
        let enclosing = std::mem::replace(&mut code.body.error_target, failed.clone());
        emit_suite(code, body, Some((name, &enclosing)));
        code.body.error_target = enclosing;
        if !code.body.terminated {
            code.clear_exception_name(name);
        }
        code.jump(done);
        code.label(&failed);
        let error = code.op("take_error", &[]);
        code.clear_exception_name(name);
        code.raise(&error, None, true);
    } else {
        emit_suite(code, body, None);
        code.jump(done);
    }
    code.label(&next);
}

/// Run else outside the protected body, then dispatch the pending exception.
pub(crate) fn protected<'a, 'p, 'h, E, H, S, N>(
    code: &mut E,
    body: &'h [S],
    handlers: &'h [H],
    otherwise: &'h [S],
    is_star: bool,
    mut describe: impl FnMut(&mut E, &'h H) -> (Option<String>, Option<N>, &'h [S]),
    mut suite: impl FnMut(&mut E, &'h [S], Option<(&str, &str)>),
) where
    E: FrameEmitter<'a, 'p>,
    N: AsRef<str>,
{
    if handlers.is_empty() {
        suite(code, body, None);
        return;
    }
    let (handler, enclosing) = code.begin_operand_scope();
    suite(code, body, None);
    code.body.error_target = enclosing;
    suite(code, otherwise, None);
    let (exception, done) = code.enter_handlers(&handler);
    if is_star {
        groups(code, handlers, &exception, &done, describe, suite);
    } else {
        for handler in handlers {
            arm(
                code,
                &exception,
                &done,
                |code| describe(code, handler),
                &mut suite,
            );
        }
        code.raise(&exception, None, true);
    }
    code.leave_handlers(&done);
}

/// Split a pending group in source order, retaining its matched and unhandled parts
/// across callbacks before merging the outcomes back into the enclosing exception.
fn groups<'a, 'p, 'h, F: FrameEmitter<'a, 'p>, H, S: 'h, N: AsRef<str>>(
    code: &mut F,
    handlers: &'h [H],
    exc: &str,
    done: &str,
    mut describe: impl FnMut(&mut F, &'h H) -> (Option<String>, Option<N>, &'h [S]),
    mut emit_suite: impl FnMut(&mut F, &'h [S], Option<(&str, &str)>),
) {
    let raised = code.op("list", &[]);
    code.live.push(raised.clone());
    let roots = code.live.len();
    let mut remainder = exc.to_owned();
    let mut active = exc.to_owned();
    for handler in handlers {
        code.live.extend([remainder.clone(), active.clone()]);
        let (matcher, name, suite) = describe(code, handler);
        let name = name.as_ref().map(AsRef::as_ref);
        let matcher = matcher.expect("except* requires a type");
        let pair = code.op("except_star_split", &[&remainder, &matcher]);
        let zero = code.integer(0);
        let matched = code.op("get", &[&pair, &zero]);
        let one = code.integer(1);
        remainder = code.op("get", &[&pair, &one]);

        let has_match = code.value(&format!("icmp ne i64 {matched}, 0"));
        // Later match expressions see the last subgroup handled, even across an empty match.
        active = code.value(&format!(
            "select i1 {has_match}, i64 {matched}, i64 {active}"
        ));
        *code.caught.last_mut().unwrap() = active.clone();
        code.live.truncate(roots);
        code.live.extend([remainder.clone(), active.clone()]);

        let body = code.name("b");
        let failed = code.name("b");
        let next = code.name("b");
        code.branch(&has_match, &body, &next);
        code.label(&body);
        code.op("restore_caught", &[&matched]);
        if let Some(name) = name {
            code.bind(name, &matched);
        }
        let outer = std::mem::replace(&mut code.body.error_target, failed.clone());
        emit_suite(code, suite, None);
        code.body.error_target = outer;
        if !code.body.terminated
            && let Some(name) = name
        {
            code.clear_star_target(name);
        }
        code.jump(&next);
        code.label(&failed);
        let error = code.op("take_error", &[]);
        if let Some(name) = name {
            code.clear_star_target(name);
        }
        code.op("append", &[&raised, &error]);
        code.jump(&next);
        code.label(&next);
        code.live.truncate(roots);
    }

    code.op("append", &[&raised, &remainder]);
    let merged = code.op("except_star_merge", &[exc, &raised]);
    code.live.pop();
    let enclosing = code
        .caught
        .iter()
        .rev()
        .nth(1)
        .cloned()
        .unwrap_or_else(|| "0".into());
    code.op("restore_caught", &[&enclosing]);
    let propagate = code.name("b");
    let handled = code.value(&format!("icmp eq i64 {merged}, 0"));
    code.branch(&handled, done, &propagate);
    code.label(&propagate);
    code.raise(&merged, None, true);
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

impl Frame<'_, '_, '_> {
    pub(in crate::llvm) fn try_region(
        &mut self,
        body: &[Statement],
        handlers: &[ExceptHandler],
        otherwise: &[Statement],
        finalbody: &[Statement],
    ) {
        let names = self.names;
        let stars = handlers
            .first()
            .is_some_and(|handler| matches!(handler.matcher, ExceptionMatcher::Subgroup(..)));
        let run = |f: &mut Self| {
            protected(
                f,
                body,
                handlers,
                otherwise,
                stars,
                |f, handler| {
                    let class = match &handler.matcher {
                        ExceptionMatcher::Categories(_) => None,
                        ExceptionMatcher::Object(site, class)
                        | ExceptionMatcher::Subgroup(site, class) => {
                            f.checkpoint(*site);
                            Some(f.evaluate_handle(class))
                        }
                    };
                    let name = handler
                        .name
                        .map(|slot| names.expect("registered scope")[&slot].name.as_str());
                    (class, name, handler.body.as_slice())
                },
                |f, body, cleanup| {
                    if let Some((name, outer)) = cleanup {
                        f.code.iteration.scopes.push(Region::ExceptionName {
                            name: name.into(),
                            outer: outer.into(),
                        });
                    }
                    f.emit_suite(body);
                    if cleanup.is_some() {
                        f.leave_scope();
                    }
                },
            );
        };
        if finalbody.is_empty() {
            run(self);
            return;
        }
        finalized(
            self,
            |f, outer| {
                f.code.iteration.scopes.push(Region::Finally {
                    body: finalbody.into(),
                    outer: outer.into(),
                    loops: f.code.loops.len(),
                    caught: f.code.caught.len(),
                });
                run(f);
                f.leave_scope();
            },
            |f, pending| {
                if let Some((value, enclosing)) = pending {
                    f.code.iteration.scopes.push(Region::Pending {
                        value: value.into(),
                        enclosing: Some(enclosing.into()),
                        outer: f.code.body.error_target.clone(),
                    });
                }
                f.emit_suite(finalbody);
                if pending.is_some() {
                    f.leave_scope();
                }
            },
        );
    }

    pub(super) fn emit_suite(&mut self, suite: &[Statement]) {
        CheckedBody::registered(self.reborrow()).emit_block(suite);
    }
}
