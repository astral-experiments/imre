//! Registered context-manager protocol, roots and exceptional control flow.

use super::frame::SourcePosition;
use super::{Frame, FrameEmitter, RegisteredFrame};
use crate::ir::{ContextBinding, ContextSite, Expr, Statement};
use ruff_text_size::TextRange;

/// An acquired manager owns both the normal and exceptional exit edges.
pub(crate) struct ContextRegion {
    site: ContextSite,
    callable: String,
    caught: String,
    error: String,
    done: String,
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

    fn exit_sync(&mut self, exit: &ContextRegion, exception: Option<&str>) -> String {
        self.set_source_range(Some(exit.site.range));
        let result = self.op_at(
            "exit_context",
            &[&exit.callable, exception.unwrap_or("0"), &exit.caught],
            self.context_span(exit.site),
        );
        self.set_source_range(None);
        result
    }

    fn enter_sync(&mut self, manager: &str, site: ContextSite) -> (String, String) {
        self.set_source_range(Some(site.range));
        let span = self.context_span(site);
        let exit = self.op_at("lookup_exit", &[manager, "0"], span);
        let enter = self.op_at("lookup_enter", &[manager, "0"], span);
        let value = self.invoke_at(&enter, &[], span);
        self.set_source_range(None);
        (exit, value)
    }
}

impl RegisteredFrame<'_, '_> {
    fn enter_async(
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

    fn exit_context(&mut self, context: &ContextRegion, exception: Option<&str>) -> String {
        if !context.site.is_async {
            return self.exit_sync(context, exception);
        }
        let args = [
            context.callable.as_str(),
            exception.unwrap_or("0"),
            context.caught.as_str(),
        ];

        // A pending return, outer managers and the body exception survive this await.
        // The exceptional exit also retains its callable through the truth test.
        let outer_roots = self.live.len() + self.live_slots.len();
        self.caught
            .push(exception.unwrap_or(&context.caught).into());
        self.current_range = context.site.range;
        if let Some(exception) = exception {
            self.checkpoint_with_roots(context.site.range, &[exception, &context.callable]);
        } else {
            self.checkpoint_with_roots(context.site.range, &[&context.callable]);
        }
        self.caught.pop();
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

    /// Leave an enclosing manager on return, break or continue. An awaited exit drops
    /// its callable before resuming the transfer into the next enclosing cleanup.
    pub(crate) fn leave_context(
        &mut self,
        context: &ContextRegion,
        discarded: &mut Vec<(usize, String)>,
    ) {
        if context.site.is_async {
            let index = self
                .live
                .iter()
                .rposition(|value| value == &context.callable)
                .unwrap();
            discarded.push((index, self.live.remove(index)));
        }
        self.exit_context(context, None);
    }

    fn begin_context(&mut self, site: ContextSite, callable: String) -> (ContextRegion, String) {
        let error = self.name("b");
        let done = self.name("b");
        let outer = std::mem::replace(&mut self.body.error_target, error.clone());
        self.live.push(callable.clone());
        (
            ContextRegion {
                site,
                callable,
                caught: self.caught.last().cloned().unwrap_or_else(|| "0".into()),
                error,
                done,
            },
            outer,
        )
    }

    fn end_context(&mut self, region: ContextRegion, outer: String) {
        self.live.pop();
        self.body.error_target = outer;
        if !self.body.terminated {
            self.exit_context(&region, None);
            self.jump(&region.done);
        }
        let exception = self.take_context_error(&region.error, &region.callable);
        let result = self.exit_context(&region, Some(&exception));
        let suppress = self.truth(&result);
        let propagate = self.name("b");
        self.branch(&suppress, &region.done, &propagate);
        self.label(&propagate);
        self.raise(&exception, None, true);
        self.label(&region.done);
    }
}

/// The registered activation owns the exit stack across both statement emitters.
pub(crate) trait RegionEmitter<'a, 'p>: FrameEmitter<'a, 'p> {
    fn emit_finally(&mut self, suite: &Finally<'p>);

    fn return_from_regions(&mut self, value: String) {
        self.live.push(value.clone());
        self.exit_regions(0, Some(&value));
        self.live.pop();
        if !self.body.terminated {
            self.return_value(&value);
        }
    }

    fn unwind_loop(&mut self, stmt: &Statement) {
        let target = self.loops.last().expect("loop transfer");
        let (label, depth) = match stmt {
            Statement::Break => (&target.edges.done, target.exit_depth),
            Statement::Continue => (&target.edges.step, target.step_depth),
            Statement::Exhaust => (&target.edges.exhausted, target.step_depth),
            _ => unreachable!("loop transfer"),
        };
        let label = label.clone();
        self.exit_regions(depth, None);
        self.jump(&label);
    }

    fn exit_regions(&mut self, depth: usize, pending_return: Option<&str>) {
        if self.regions.len() == depth {
            return;
        }
        let target = self.body.error_target.clone();
        let mut left = Vec::new();
        let mut discarded = Vec::new();
        while self.regions.len() > depth && !self.body.terminated {
            // An exit inside a final suite can override the original transfer. It
            // must see only the regions outside the cleanup currently being run.
            let region = self.regions.pop().unwrap();
            match &region {
                Region::Context { exit, outer } => {
                    self.body.error_target.clone_from(outer);
                    self.leave_context(exit, &mut discarded);
                    self.flush_check();
                }
                Region::Slots(slots) => self.clear_slots(slots),
                Region::Finally {
                    body,
                    outer,
                    loops,
                    caught,
                } => {
                    self.body.error_target.clone_from(outer);
                    let inner_loops = self.loops.split_off(*loops);
                    let inner_handlers = self.caught.split_off(*caught);
                    let value = pending_return.filter(|value| *value != "0");
                    if let Some(value) = value {
                        self.regions.push(Region::Pending {
                            value: value.into(),
                            enclosing: None,
                            outer: outer.clone(),
                        });
                    }
                    self.emit_finally(body);
                    if value.is_some() {
                        self.regions.pop();
                    }
                    self.caught.extend(inner_handlers);
                    self.loops.extend(inner_loops);
                }
                Region::ExceptionName { name, outer } => {
                    self.body.error_target.clone_from(outer);
                    self.clear_exception_name(name);
                    self.flush_check();
                }
                Region::Pending {
                    value,
                    enclosing,
                    outer,
                } => {
                    self.body.error_target.clone_from(outer);
                    if let Some(caught) = enclosing {
                        self.op("restore_caught", &[caught]);
                    }
                    let index = self.live.iter().rposition(|v| v == value).unwrap();
                    discarded.push((index, self.live.remove(index)));
                    self.release_target(None);
                }
            }
            left.push(region);
        }
        for (slot, owner) in discarded.into_iter().rev() {
            self.live.insert(slot, owner);
        }
        self.regions.extend(left.into_iter().rev());
        self.body.error_target = target;
    }
}

pub(crate) enum Finally<'p> {
    Checked(std::rc::Rc<[Statement]>),
    Prepared(&'p crate::frontend::RegisteredSuite),
}

pub(crate) enum Region<'p> {
    Slots(Vec<usize>),
    Context {
        exit: ContextRegion,
        outer: String,
    },
    Finally {
        body: Finally<'p>,
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
        binding: Option<&ContextBinding>,
        body: &[Statement],
    ) {
        let (exit, value, above_exit) = if site.is_async {
            let mark = self.temporary_mark();
            let (exit, value, above_exit) = self.unwind_operands(&mark, |frame| {
                let manager = frame.evaluate_handle(manager);
                frame.enter_async(&manager, site, &mark)
            });
            (exit, value, Some(above_exit))
        } else {
            let manager = self.evaluate_handle(manager);
            let (exit, value) = self.enter_sync(&manager, site);
            (exit, value, None)
        };
        let (exit, outer) = self.begin_context(site, exit);
        self.regions.push(Region::Context { exit, outer });
        if let Some(binding) = binding {
            self.store_private(binding.entered, &value);
            self.emit_suite(&binding.body);
        }
        if let Some(mark) = above_exit {
            self.discard_truth(&mark);
        }
        self.emit_suite(body);
        let Some(Region::Context { exit, outer }) = self.regions.pop() else {
            unreachable!("active context region")
        };
        self.end_context(exit, outer);
    }
}

impl<'a, 'p> RegionEmitter<'a, 'p> for Frame<'_, 'a, 'p> {
    fn emit_finally(&mut self, suite: &Finally<'p>) {
        match suite {
            Finally::Checked(statements) => self.emit_suite(statements),
            Finally::Prepared(body) => super::emit_prepared_suite(self.code.prepared_owner(), body),
        }
    }
}
