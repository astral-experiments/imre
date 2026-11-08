//! Lower the comprehension plan's branches to LLVM. Eager displays keep their
//! iterators in SSA values. A generator keeps them in its frame across yields.

use imre_ast::comprehension::{Output, Plan, Step};
use ruff_python_ast as ast;
use ruff_text_size::Ranged;

use super::{Body, DefinitionName, Function};

impl Function<'_, '_> {
    pub(super) fn comprehension(&mut self, expr: &ast::Expr, plan: Plan<'_>) -> String {
        self.iteration_result(|f| f.emit_comprehension(expr, plan))
    }

    fn emit_comprehension(&mut self, expr: &ast::Expr, plan: Plan<'_>) -> String {
        // Eager awaits require a containing coroutine. This emitter handles
        // ordinary frames and creates separately resumable generator expressions.
        if !self.native_suspension
            && !matches!(plan.output, Output::Generator(_))
            && plan.body_suspends()
        {
            return self.unsupported("async comprehension");
        }
        let iterator = self.iteration_cursor(plan.outer, plan.outer_async);
        let names: indexmap::IndexSet<String> = plan.collect_target_names();
        let declaration: Vec<_> = names.iter().map(String::as_str).collect();
        let targets = self.text(&declaration.join(" "));
        if let ast::Expr::Generator(generator) = expr {
            let function = self.define(
                generator.start(),
                DefinitionName::Plain("<genexpr>"),
                None,
                None,
                None,
                Body::Generator(generator, plan, names),
            );
            return self.op("generator", &[&function, &iterator, &targets]);
        }
        let scope = self.op("comprehension_scope", &[&self.scope.clone(), &targets]);
        let enclosing = std::mem::replace(&mut self.scope, scope);
        let class_locals = self.class_locals.take();
        let output = self.op(plan.output.kind(), &[]);
        self.comprehension_steps(&plan, iterator, Some(&output));
        self.class_locals = class_locals;
        self.scope = enclosing;
        output
    }

    pub(super) fn iteration_cursor(&mut self, iterable: &ast::Expr, is_async: bool) -> String {
        self.iteration_result(|f| {
            let value = f.expr(iterable);
            if f.native_suspension {
                f.suspension_position(iterable.range());
            }
            f.op(if is_async { "async_iter" } else { "iter" }, &[&value])
        })
    }

    /// The activation keeps its live handles until the next Python statement.
    /// Release the iterable when its cursor exists, and the cursor and local scope
    /// when the comprehension delivers its result to the enclosing expression.
    fn iteration_result(&mut self, emit: impl FnOnce(&mut Self) -> String) -> String {
        if !self.native_suspension {
            return emit(self);
        }
        let mark = self.temporary_mark();
        let result = self.unwind_operands(&mark, emit);
        self.keep_operands(&mark, &result, "0");
        result
    }

    pub(super) fn generator_step(&mut self, plan: &Plan<'_>) {
        let outer = self.lookup(".iterator");
        self.comprehension_steps(plan, outer, None);
    }

    fn comprehension_steps(&mut self, plan: &Plan<'_>, outer: String, collection: Option<&str>) {
        let labels: Vec<_> = plan.steps.iter().map(|_| self.name("b")).collect();
        let mut iterators = vec![String::new(); plan.depths];
        let mut marks = vec![String::new(); plan.depths];
        let slot = |depth| format!(".iterator.{depth}");
        if collection.is_some() {
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
                    if collection.is_some() {
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
                    if collection.is_some() {
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
                        let more = self.op("more", &[&iter]);
                        let ready = self.value(&format!("icmp eq i64 {more}, 2"));
                        self.branch(&ready, &body, &labels[exhausted]);
                        self.label(&body);
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
                    let key = output.key().map(|expr| self.expr(expr));
                    let value = self.expr(output.value());
                    if let Some(out) = collection {
                        if let Some(key) = key {
                            self.op("put", &[out, &key, &value]);
                        } else {
                            self.op("append", &[out, &value]);
                        }
                        self.jump(&labels[resume]);
                    } else {
                        debug_assert!(matches!(output, Output::Generator(_)));
                        let item = self.op("yielded", &[&value]);
                        self.end(&format!("ret i64 {item}"));
                    }
                }
                Step::Close { depth, next } => {
                    if collection.is_none() && depth > 0 {
                        self.bind(&slot(depth), "0");
                    }
                    self.jump(&labels[next]);
                }
                Step::End => {}
            }
        }
    }
}
