//! Registered context-manager protocol, roots and exceptional control flow.

use super::frame::SourcePosition;
use super::{Frame, RegisteredFrame};
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
        let position = if let Some(range) = range {
            self.position = SourcePosition::Implicit(range);
            self.position_constant(range)
        } else {
            if let SourcePosition::Implicit(previous) | SourcePosition::Expression(previous) =
                self.position
            {
                self.position = SourcePosition::statement(self.positions.line(previous.start()));
            }
            "null".into()
        };
        self.line(&format!(
            "call void @imre_dyn_definition_position(ptr %ctx, ptr {position})"
        ));
    }

    pub(crate) fn context_span(&self, site: crate::ir::ContextSite) -> Option<[usize; 4]> {
        let (line, start) = self.positions.line_and_column(site.range.start());
        let (end_line, end) = self.positions.line_and_column(site.range.end());
        let anchor = if self.simple_call == Some(site.range) {
            start
        } else {
            start + usize::from(site.anchor - site.range.start())
        };
        (end_line == line).then_some([line, start, anchor, end])
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

/// An acquired manager owns both the normal and exceptional exit edges.
pub(crate) struct ContextRegion {
    pub(crate) exit: ContextExit,
    pub(crate) outer: String,
    error: String,
    done: String,
}

impl RegisteredFrame<'_, '_> {
    pub(crate) fn enter_async(
        &mut self,
        manager: &str,
        site: ContextSite,
        mark: &str,
    ) -> (String, String, String) {
        let range = site.range;
        self.call_position(range);
        let exit = self.op("lookup_exit", &[manager, "2"]);
        self.call_position(range);
        let enter = self.op("lookup_enter", &[manager, "2"]);
        let above_exit = self.keep_operands(mark, &exit, &enter);
        self.call_position(range);
        let value = self.invoke(&enter, &[]);
        self.keep_operands(&above_exit, &value, "0");
        let value = self.await_at(&value, "1", range, &above_exit);
        (exit, value, above_exit)
    }

    pub(crate) fn exit_context(
        &mut self,
        context: &ContextExit,
        exception: Option<&str>,
    ) -> String {
        if !context.is_async {
            return self.exit_sync(context, exception);
        }
        let args = [
            context.callable.as_str(),
            exception.unwrap_or("0"),
            context.caught.as_str(),
        ];

        // A pending return, outer managers and the body exception survive this await.
        // The exceptional exit also retains its callable through the truth test.
        let outer_roots = self.live.len();
        self.live.extend(exception.map(String::from));
        self.live.push(context.callable.clone());
        self.caught
            .push(exception.unwrap_or(&context.caught).into());
        self.current_range = context.site.range;
        self.checkpoint(context.site.range);
        self.caught.pop();
        self.live.truncate(outer_roots);
        self.call_position(context.site.range);
        let awaitable = self.op("async_exit_context", &args);
        let mark = if exception.is_some() {
            outer_roots + 2
        } else {
            outer_roots
        }
        .to_string();
        self.keep_operands(&mark, &awaitable, "0");
        let result = self.await_at(&awaitable, "2", context.site.range, &mark);
        if let Some(exc) = exception {
            self.call_position(context.site.range);
            let suppress = self.op("truth", &[&result]);
            self.keep_operands(&outer_roots.to_string(), exc, &suppress);
            self.op("restore_caught", &[&context.caught]);
            suppress
        } else {
            self.discard_truth(&mark);
            "0".into()
        }
    }

    pub(crate) fn begin_context(
        &mut self,
        site: ContextSite,
        callable: String,
        is_async: bool,
    ) -> ContextRegion {
        let error = self.name("b");
        let done = self.name("b");
        let outer = std::mem::replace(&mut self.body.error_target, error.clone());
        let exit = ContextExit {
            site,
            is_async,
            callable,
            caught: self.caught.last().cloned().unwrap_or_else(|| "0".into()),
        };
        self.live.push(exit.callable.clone());
        ContextRegion {
            exit,
            outer,
            error,
            done,
        }
    }

    pub(crate) fn end_context(&mut self, region: ContextRegion) {
        self.live.pop();
        self.body.error_target = region.outer;
        if !self.body.terminated {
            self.exit_context(&region.exit, None);
            self.jump(&region.done);
        }
        self.label(&region.error);
        let exception = self.op("take_error", &[]);
        let result = self.exit_context(&region.exit, Some(&exception));
        let suppress = self.truth(&result);
        let propagate = self.name("b");
        self.branch(&suppress, &region.done, &propagate);
        self.label(&propagate);
        self.raise(&exception, None, true);
        self.label(&region.done);
    }
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
        let region = self.code.begin_context(site, exit, false);
        self.code.iteration.scopes.push(Region::Context {
            exit: region.exit.clone(),
            outer: region.outer.clone(),
        });
        if let Some(slot) = target {
            self.store_private(slot, &value);
        }
        self.emit_suite(body);
        self.leave_scope();
        self.code.end_context(region);
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
                Region::Slots(slots) => self.clear_slots(slots),
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
