//! Exception handlers entered through a registered Python frame.

use super::{Frame, FrameEmitter, RegisteredFrame, context::Region};
use crate::ir::{ExceptHandler, ExceptionMatcher, Statement};
use crate::llvm::operations::CheckedBody;

/// Only the protected body transfers its failures to these handlers. The else
/// suite and the handlers themselves propagate to the enclosing target.
pub(crate) fn protected<'a, 'p, E: FrameEmitter<'a, 'p>>(
    code: &mut E,
    body: impl FnOnce(&mut E),
    otherwise: impl FnOnce(&mut E),
    dispatch: impl FnOnce(&mut E, &str, &str),
) {
    let handler = code.name("b");
    let done = code.name("b");
    let enclosing = std::mem::replace(&mut code.body.error_target, handler.clone());
    body(code);
    code.body.error_target = enclosing;
    otherwise(code);
    code.jump(&done);
    code.label(&handler);
    let exception = code.op("take_error", &[]);
    code.live.push(exception.clone());
    code.caught.push(exception.clone());
    dispatch(code, &exception, &done);
    code.caught.pop();
    code.live.pop();
    code.label(&done);
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
    if !code.body.terminated || code.module_annotation_sites.is_some() {
        cleanup(code, None);
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
    code.label(&done);
}

/// A named arm clears its binding on fallthrough, handler failure and abrupt
/// transfer. The caller installs the abrupt-transfer cleanup around its body.
pub(crate) fn arm<'a, 'p, E: FrameEmitter<'a, 'p>>(
    code: &mut E,
    exception: &str,
    done: &str,
    name: Option<&str>,
    matcher: impl FnOnce(&mut E) -> Option<String>,
    body: impl FnOnce(&mut E, Option<(&str, &str)>),
) {
    let next = code.name("b");
    if let Some(class) = matcher(code) {
        let matches = code.op("matches", &[exception, &class]);
        let matches = code.value(&format!("icmp eq i64 {matches}, 2"));
        let matched = code.name("b");
        code.branch(&matches, &matched, &next);
        code.label(&matched);
    }
    if let Some(name) = name {
        code.bind(name, exception);
        let failed = code.name("b");
        let enclosing = std::mem::replace(&mut code.body.error_target, failed.clone());
        body(code, Some((name, &enclosing)));
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
        body(code, None);
        code.jump(done);
    }
    code.label(&next);
}

/// Split a pending group in source order, retaining its matched and unhandled parts
/// across callbacks before merging the outcomes back into the enclosing exception.
pub(crate) fn groups<'a, 'p, 'h, F: FrameEmitter<'a, 'p>, H, S: 'h, N: AsRef<str>>(
    code: &mut F,
    handlers: &'h [H],
    exc: &str,
    done: &str,
    mut describe: impl FnMut(&mut F, &'h H) -> (String, Option<N>, &'h [S]),
    mut emit_suite: impl FnMut(&mut F, &[S]),
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
        emit_suite(code, suite);
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
        let run = |f: &mut Self| {
            if handlers.is_empty() {
                f.emit_suite(body);
                return;
            }
            protected(
                f,
                |f| f.emit_suite(body),
                |f| f.emit_suite(otherwise),
                |f, exc, done| {
                    if matches!(handlers[0].matcher, ExceptionMatcher::Subgroup(..)) {
                        groups(
                            f,
                            handlers,
                            exc,
                            done,
                            |f, handler| {
                                let ExceptionMatcher::Subgroup(site, class) = &handler.matcher
                                else {
                                    unreachable!("ordinary and subgroup handlers cannot be mixed")
                                };
                                f.checkpoint(*site);
                                let matcher = f.evaluate_handle(class);
                                let name = handler.name.map(|slot| {
                                    f.names.expect("registered scope")[&slot].name.clone()
                                });
                                (matcher, name, handler.body.as_slice())
                            },
                            Self::emit_suite,
                        );
                        return;
                    }
                    for handler in handlers {
                        let name = handler
                            .name
                            .map(|slot| f.names.expect("registered scope")[&slot].name.as_str());
                        arm(
                            f,
                            exc,
                            done,
                            name,
                            |f| match &handler.matcher {
                                ExceptionMatcher::Categories(_) => None,
                                ExceptionMatcher::Subgroup(..) => unreachable!("subgroup handler"),
                                ExceptionMatcher::Object(range, class) => {
                                    f.checkpoint(*range);
                                    Some(f.evaluate_handle(class))
                                }
                            },
                            |f, cleanup| {
                                if let Some((name, outer)) = cleanup {
                                    f.code.iteration.scopes.push(Region::ExceptionName {
                                        name: name.into(),
                                        outer: outer.into(),
                                    });
                                }
                                f.emit_suite(&handler.body);
                                if cleanup.is_some() {
                                    f.leave_scope();
                                }
                            },
                        );
                    }
                    f.raise(exc, None, true);
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
                    loops: f.code.iteration.loops.len(),
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
        CheckedBody::registered(Frame {
            code: self.code,
            sites: self.sites,
            names: self.names,
        })
        .emit_block(suite);
    }
}
