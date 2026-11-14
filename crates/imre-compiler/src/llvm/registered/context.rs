//! Registered context-manager protocol, roots and exceptional control flow.

use super::frame::SourcePosition;
use super::{Frame, RegisteredFrame};
use crate::ir::{ContextBinding, ContextSite, Expr, Statement};
use crate::llvm::{Completion, finally::Finalbody};
use ruff_text_size::TextRange;

/// The exit callable and enclosing exception captured on successful entry.
struct ContextRegion {
    site: ContextSite,
    callable: String,
    caught: String,
}

impl RegisteredFrame<'_, '_> {
    /// Publish an implicit operation's actual full range without releasing the frame's operands.
    pub(crate) fn set_source_range(&mut self, range: Option<TextRange>) {
        self.output.declare(
            |d| &mut d.declared_definition_position,
            "declare void @imre_dyn_definition_position(ptr, ptr)\n",
        );
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

    fn exit_sync(&mut self, exit: &ContextRegion, exception: &str) -> String {
        self.set_source_range(Some(exit.site.range));
        let result = self.op_at(
            "exit_context",
            &[&exit.callable, exception, &exit.caught],
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

    fn exit_async(&mut self, context: &ContextRegion, error: &str, outer_roots: usize) -> String {
        let range = context.site.range;
        let failed = self.value(&format!("icmp ne i64 {error}, 0"));
        let active = if context.caught == "0" {
            error.to_owned()
        } else {
            self.value(&format!(
                "select i1 {failed}, i64 {error}, i64 {}",
                context.caught
            ))
        };
        self.caught.push(active);
        self.current_range = range;
        self.checkpoint_with_roots(range, &[error, &context.callable]);
        self.caught.pop();
        self.call_position(range);
        let awaitable = self.op(
            "async_exit_context",
            &[&context.callable, error, &context.caught],
        );
        // Only exceptional exits retain the exception and callable across the await and truth test.
        let mark = self.value(&format!(
            "select i1 {failed}, i64 {}, i64 {outer_roots}",
            outer_roots + 2
        ));
        self.keep_operands(&mark, &awaitable, "0");
        self.await_at(&awaitable, "2", range, &mark)
    }
}

impl<'p> Frame<'_, '_, 'p> {
    pub(super) fn return_from_regions(&mut self, value: String) {
        self.live.push(value.clone());
        self.exit_regions(0, &Completion::Return(value.clone()));
        self.live.pop();
        if !self.body.terminated {
            self.return_value(&value);
        }
    }

    pub(in crate::llvm) fn jump_from_regions(&mut self, label: String, depth: usize) {
        self.exit_regions(
            depth,
            &Completion::Jump {
                label: label.clone(),
                depth,
            },
        );
        self.jump(&label);
    }

    fn exit_regions(&mut self, depth: usize, completion: &Completion<String>) {
        if self.regions.len() == depth {
            return;
        }
        let target = self.body.error_target.clone();
        let mut left = Vec::new();
        let mut discarded = Vec::new();
        while self.regions.len() > depth && !self.body.terminated {
            // An exit inside a final suite can override the original transfer. It
            // must see only the regions outside the cleanup currently being run.
            let mut region = self.regions.pop().unwrap();
            match &mut region {
                Region::Slots(slots) => self.clear_slots(slots),
                Region::Finally(finalbody) => {
                    self.flush_check();
                    finalbody.enter(&mut self.body, completion.clone(), None);
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
                    let pending = self.value(&format!("icmp ne i64 {value}, 0"));
                    let release = self.name("b");
                    let done = self.name("b");
                    self.branch(&pending, &release, &done);
                    self.label(&release);
                    if let Some(caught) = enclosing {
                        self.op("restore_caught", &[caught]);
                    }
                    let index = self.live.iter().rposition(|v| v == value).unwrap();
                    discarded.push((index, self.live.remove(index)));
                    self.release_target();
                    self.jump(&done);
                    self.label(&done);
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

pub(super) enum Region {
    Slots(Vec<usize>),
    Finally(Finalbody<String>),
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
                let manager = frame.evaluate_scoped_handle(manager);
                frame.enter_async(&manager, site, &mark)
            });
            (exit, value, Some(above_exit))
        } else {
            let manager = self.evaluate_scoped_handle(manager);
            let (exit, value) = self.enter_sync(&manager, site);
            (exit, value, None)
        };
        let context = ContextRegion {
            site,
            callable: exit,
            caught: self.caught.last().cloned().unwrap_or_else(|| "0".into()),
        };
        let failed = self.name("b");
        let done = self.name("b");
        self.live.push(context.callable.clone());
        let (mut region, _) = self.protected_exits(&failed, &done, |frame| {
            if let Some(binding) = binding {
                frame.store_private(binding.entered, &value);
                frame.emit_suite(&binding.body);
            }
            if let Some(mark) = above_exit {
                frame.discard_truth(&mark);
            }
            frame.emit_suite(body);
        });
        self.live.pop();

        let raised = self.take_context_error(&failed, &context.callable);
        region.enter(
            &mut self.body,
            Completion::Error,
            Some((raised, context.caught.clone())),
        );
        self.finish_exits(
            region,
            |frame, _, error| {
                let outer_roots = frame.live.len() + frame.live_slots.len();
                let result = if site.is_async {
                    frame.exit_async(&context, &error, outer_roots)
                } else {
                    frame.exit_sync(&context, &error)
                };
                (error, result, outer_roots)
            },
            |frame, target, (error, result, outer_roots)| {
                if matches!(target, Completion::Error) {
                    let suppress = if site.is_async {
                        frame.call_position(site.range);
                        let suppress = frame.truth(result);
                        frame.keep_operands(&outer_roots.to_string(), error, "0");
                        frame.op("restore_caught", &[&context.caught]);
                        suppress
                    } else {
                        frame.value(&format!("icmp eq i64 {result}, {}", super::permanent::TRUE))
                    };
                    let propagate = frame.name("b");
                    frame.branch(&suppress, &done, &propagate);
                    frame.label(&propagate);
                    frame.raise(error, None, true);
                } else if site.is_async {
                    frame.discard_truth(&outer_roots.to_string());
                }
            },
        );
        self.label(&done);
    }
}
