//! Registered context-manager protocol, roots and exceptional control flow.

use super::{Frame, FrameEmitter, RegisteredFrame};
use crate::ir::{ContextSite, Expr, Statement};
use ruff_text_size::TextRange;

#[derive(Clone)]
pub(crate) struct ContextExit {
    pub(crate) site: ContextSite,
    pub(crate) is_async: bool,
    pub(crate) callable: String,
    pub(crate) caught: String,
}

impl RegisteredFrame<'_, '_> {
    /// Publish an implicit operation's actual full range without releasing the frame's operands.
    pub(crate) fn set_source_range(&mut self, range: Option<TextRange>) {
        if !self.output.declarations.declared_definition_position {
            self.output
                .functions
                .push_str("declare void @imre_dyn_definition_position(ptr, ptr)\n");
            self.output.declarations.declared_definition_position = true;
        }
        let position = range.map_or_else(
            || "null".into(),
            |range| {
                let (line, start) = self.positions.line_and_column(range.start());
                let (end_line, end) = self.positions.line_and_column(range.end());
                self.expression_line = line;
                self.output.position([line, end_line, start, end])
            },
        );
        self.line(&format!(
            "call void @imre_dyn_definition_position(ptr %ctx, ptr {position})"
        ));
    }

    pub(crate) fn context_span(&self, site: crate::ir::ContextSite) -> Option<[usize; 4]> {
        if let Some((prefix, delimiter)) = site.delimiter
            && let Some(span) = self.expression_range_span(site.range, prefix, delimiter)
        {
            return Some(span);
        }
        let (line, start) = self.positions.line_and_column(site.range.start());
        let (end_line, end) = self.positions.line_and_column(site.range.end());
        (end_line == line).then_some([line, start, start, end])
    }

    pub(crate) fn exit_sync(&mut self, exit: &ContextExit, exception: Option<&str>) -> String {
        self.set_source_range(Some(exit.site.range));
        let result = self.op_at(
            "exit_context",
            &[&exit.callable, exception.unwrap_or("0"), &exit.caught],
            self.context_span(exit.site),
        );
        self.set_source_range(None);
        result
    }

    pub(crate) fn enter_sync(&mut self, manager: &str, site: ContextSite) -> (String, String) {
        self.set_source_range(Some(site.range));
        let span = self.context_span(site);
        let exit = self.op_at("lookup_exit", &[manager, "0"], span);
        let enter = self.op_at("lookup_enter", &[manager, "0"], span);
        let value = self.invoke_at(&enter, &[], span);
        self.set_source_range(None);
        (exit, value)
    }
}

/// Enter protection after acquisition succeeds, including while binding the as-target.
pub(crate) fn entered<'a, 'p, F: FrameEmitter<'a, 'p>>(
    frame: &mut F,
    site: ContextSite,
    callable: String,
    is_async: bool,
    body: impl FnOnce(&mut F, &ContextExit, &str),
    mut exit: impl FnMut(&mut F, &ContextExit, Option<&str>) -> String,
) {
    let handler = frame.name("b");
    let done = frame.name("b");
    let outer = std::mem::replace(&mut frame.body.error_target, handler.clone());
    let context = ContextExit {
        site,
        is_async,
        callable,
        caught: frame.caught.last().cloned().unwrap_or_else(|| "0".into()),
    };
    frame.live.push(context.callable.clone());
    body(frame, &context, &outer);
    frame.live.pop();
    frame.body.error_target = outer;
    if !frame.body.terminated {
        exit(frame, &context, None);
        frame.jump(&done);
    }
    frame.label(&handler);
    let exception = frame.op("take_error", &[]);
    let result = exit(frame, &context, Some(&exception));
    let suppress = frame.truth(&result);
    let propagate_label = frame.name("b");
    frame.branch(&suppress, &done, &propagate_label);
    frame.label(&propagate_label);
    frame.raise(&exception, None, true);
    frame.label(&done);
}

#[derive(Clone)]
pub(super) enum Region {
    Slots(Vec<usize>),
    Context {
        exit: ContextExit,
        outer: String,
    },
    Finally {
        body: std::rc::Rc<[Statement]>,
        outer: String,
        loops: usize,
        caught: usize,
    },
    ExceptionName {
        name: String,
        outer: String,
    },
    /// Replacing an exit releases its rooted owner under the enclosing handler.
    Pending {
        value: String,
        enclosing: Option<String>,
        outer: String,
    },
}

impl Frame<'_, '_, '_> {
    pub(in crate::llvm) fn with_manager(
        &mut self,
        manager: &Expr,
        site: ContextSite,
        target: Option<usize>,
        body: &[Statement],
    ) {
        let manager = self.evaluate_handle(manager);
        let (exit, value) = self.enter_sync(&manager, site);
        entered(
            self,
            site,
            exit,
            false,
            |frame, context, outer| {
                frame.code.iteration.scopes.push(Region::Context {
                    exit: context.clone(),
                    outer: outer.to_owned(),
                });
                if let Some(slot) = target {
                    frame.store_private(slot, &value);
                }
                frame.emit_suite(body);
                frame.leave_scope();
            },
            |frame, context, error| frame.exit_sync(context, error),
        );
    }

    pub(super) fn exit_regions(&mut self, depth: usize, pending_return: Option<&str>) {
        if self.code.iteration.scopes.len() == depth {
            return;
        }
        let target = self.code.body.error_target.clone();
        let mut left = Vec::new();
        let mut discarded = Vec::new();
        while self.code.iteration.scopes.len() > depth && !self.code.body.terminated {
            // An exit inside a final suite can override the original transfer. It
            // must see only the regions outside the cleanup currently being run.
            let region = self.code.iteration.scopes.pop().unwrap();
            match &region {
                Region::Context { exit, outer } => {
                    self.code.body.error_target.clone_from(outer);
                    self.code.exit_sync(exit, None);
                    self.code.flush_check();
                }
                Region::Slots(slots) if pending_return.is_none() => self.clear_slots(slots),
                Region::Slots(_) => {}
                Region::Finally {
                    body,
                    outer,
                    loops,
                    caught,
                } => {
                    self.code.body.error_target.clone_from(outer);
                    let inner_loops = self.code.iteration.loops.split_off(*loops);
                    let inner_handlers = self.code.caught.split_off(*caught);
                    let value = pending_return.filter(|value| *value != "0");
                    if let Some(value) = value {
                        self.code.iteration.scopes.push(Region::Pending {
                            value: value.into(),
                            enclosing: None,
                            outer: outer.clone(),
                        });
                    }
                    self.emit_suite(body);
                    if value.is_some() {
                        self.leave_scope();
                    }
                    self.code.caught.extend(inner_handlers);
                    self.code.iteration.loops.extend(inner_loops);
                }
                Region::ExceptionName { name, outer } => {
                    self.code.body.error_target.clone_from(outer);
                    self.code.clear_exception_name(name);
                    self.code.flush_check();
                }
                Region::Pending {
                    value,
                    enclosing,
                    outer,
                } => {
                    self.code.body.error_target.clone_from(outer);
                    if let Some(caught) = enclosing {
                        self.code.op("restore_caught", &[caught]);
                    }
                    let index = self.code.live.iter().rposition(|v| v == value).unwrap();
                    discarded.push((index, self.code.live.remove(index)));
                    self.code.release_target(None);
                }
            }
            left.push(region);
        }
        for (slot, owner) in discarded.into_iter().rev() {
            self.code.live.insert(slot, owner);
        }
        self.code.iteration.scopes.extend(left.into_iter().rev());
        self.code.body.error_target = target;
    }
}
