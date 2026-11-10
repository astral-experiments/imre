//! Emit generator expressions whose child still needs a physical source frame.

use super::FrameEmitter;
use imre_ast::comprehension::{Output, Plan, Step};
use ruff_python_ast as ast;
use ruff_text_size::Ranged;

use super::{Body, DefinitionName, Function};

impl Function<'_, '_> {
    /// The child may be checked even when the enclosing iterator expression is residual.
    pub(super) fn prepared_generator(&mut self, generator: &ast::ExprGenerator) -> String {
        self.iteration_result(|f| {
            let outer = imre_ast::comprehension::outer_clause(&generator.generators);
            let site = generator.range();
            let acquire = f.source.generators[&site].outer_operation(outer);
            let cursor = f.iteration_input(&outer.iter, acquire.runtime_op());
            f.prepare_generator(site);
            let scope = f.scope.clone();
            f.sites.bind_generator(&mut f.frame, site, &scope, &cursor)
        })
    }

    pub(super) fn residual_generator(&mut self, generator: &ast::ExprGenerator) -> String {
        let plan = Plan::generator(generator);
        self.iteration_result(|code| {
            let iterator = if plan.body_suspends() && !plan.outer_async {
                code.iteration_input(plan.outer, "get_iter")
            } else {
                code.iteration_cursor(plan.outer, plan.outer_async)
            };
            let targets = code
                .frame
                .text(&code.source.scopes.comprehension_targets[&generator.range()]);
            let function = code.define(
                generator.start(),
                DefinitionName::Plain("<genexpr>"),
                None,
                None,
                None,
                Body::Generator(generator.range(), &plan),
            );
            code.op("generator", &[&function, &iterator, &targets])
        })
    }

    pub(super) fn iteration_cursor(&mut self, iterable: &ast::Expr, is_async: bool) -> String {
        self.iteration_input(iterable, if is_async { "async_iter" } else { "iter" })
    }

    fn iteration_input(&mut self, iterable: &ast::Expr, operation: &str) -> String {
        self.iteration_result(|f| {
            let value = f.expr(iterable);
            if f.native_suspension {
                f.call_position(iterable.range());
            }
            f.op(operation, &[&value])
        })
    }

    pub(super) fn generator_step(&mut self, plan: &Plan<'_>) {
        // An LLVM suspension resumes inside this same activation. Its first
        // entry reads the already-acquired iterator from the implicit argument.
        if self.native_suspension {
            self.checkpoint(plan.output.value());
        }
        let outer = self.frame.lookup(if self.frame.native_suspension {
            ".0"
        } else {
            ".iterator"
        });
        let outer = if self.native_suspension && !plan.outer_async {
            self.op("loop_cursor", &[&outer])
        } else {
            outer
        };
        let labels: Vec<_> = plan.steps.iter().map(|_| self.name("b")).collect();
        let mut iterators = vec![String::new(); plan.depths];
        let mut marks = vec![String::new(); plan.depths];
        let slot = |depth| format!(".iterator.{depth}");
        if self.native_suspension {
            marks[0] = self.temporary_mark();
        } else if plan.depths > 1 {
            // A yield preserves every active cursor in the frame. Only the
            // first call has the outer iterator in the incoming slot.
            let resume = plan
                .steps
                .iter()
                .find_map(|step| match step {
                    Step::Emit { resume, .. } => Some(*resume),
                    _ => None,
                })
                .expect("generator yield");
            let first = self.name("b");
            let resumed = self.value(&format!("icmp eq i64 {outer}, 0"));
            self.branch(&resumed, &labels[resume], &first);
            self.label(&first);
            self.bind(&slot(0), &outer);
            self.bind(".iterator", "0");
        }
        iterators[0] = outer;
        self.jump(&labels[0]);
        for (pc, &step) in plan.steps.iter().enumerate() {
            self.label(&labels[pc]);
            match step {
                Step::Open {
                    depth,
                    iterable,
                    is_async,
                } => {
                    let iterator = self.iteration_cursor(iterable, is_async);
                    if self.native_suspension {
                        marks[depth] = self.temporary_mark();
                        iterators[depth] = iterator;
                    } else {
                        self.bind(&slot(depth), &iterator);
                    }
                    self.jump(&labels[pc + 1]);
                }
                Step::Next {
                    depth,
                    target,
                    exhausted,
                    is_async,
                } => {
                    let body = self.name("b");
                    if self.native_suspension {
                        self.line(&format!(
                            "call void @imre_dyn_comprehension_step(ptr %ctx, i64 {})",
                            marks[depth]
                        ));
                        let iter = &iterators[depth];
                        self.enter_iteration(
                            iter,
                            target,
                            is_async.then(|| target.range()),
                            false,
                            &body,
                            &labels[exhausted],
                        );
                    } else {
                        let iter = if plan.depths == 1 {
                            iterators[0].clone()
                        } else {
                            self.lookup(&slot(depth))
                        };
                        self.frame.enter_sync_iteration(
                            &iter,
                            false,
                            false,
                            &body,
                            &labels[exhausted],
                        );
                        // The generator's statement boundary precedes target
                        // assignment: release the previous pull's temporaries.
                        self.checkpoint(plan.output.value());
                        let item = self.op("next", &[&iter]);
                        self.assign(target, &item);
                    }
                    self.jump(&labels[pc + 1]);
                }
                Step::Filter { test, reject } => {
                    self.branch_expr(test, &labels[pc + 1], &labels[reject]);
                }
                Step::Emit { output, resume } => {
                    debug_assert!(matches!(output, Output::Generator(_)));
                    let value = self.expr(output.value());
                    if self.native_suspension {
                        self.suspend_at(
                            &value,
                            super::suspension::Suspension::AsyncYield,
                            output.value().range(),
                            &marks[plan.depths - 1],
                        );
                        self.jump(&labels[resume]);
                    } else {
                        let item = self.op("yielded", &[&value]);
                        self.end(&format!("ret i64 {item}"));
                    }
                }
                Step::Close { depth, next } => {
                    if !self.native_suspension && depth > 0 {
                        self.bind(&slot(depth), "0");
                    }
                    self.jump(&labels[next]);
                }
                Step::End => {}
            }
        }
    }
}
