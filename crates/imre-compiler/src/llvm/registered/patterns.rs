//! Native structural matching on semantic operands in the live registered namespace.
use std::ops::Range;

use super::{Frame, RegisteredFrame};
use crate::ir::{Expr, MatchCase, MatchTest};
use crate::llvm::operations::{CheckedBody, CheckedValue};
use imre_ast::{SequencePatternItems, SequencePatternPlan};

/// Temporary Python captures in the order the current alternative obtained them.
struct PatternCaptures {
    slots: Range<usize>,
    names: Vec<usize>,
}

impl PatternCaptures {
    fn slot(&self, ordinal: usize) -> usize {
        self.slots.end - 1 - ordinal
    }

    fn bind(&mut self, frame: &mut RegisteredFrame<'_, '_>, name: usize, value: &str) {
        frame.store_frame_slot(self.slot(self.names.len()), value);
        self.names.push(name);
    }

    /// Successful alternatives share their names even if they read them in different orders.
    fn reorder(&mut self, frame: &mut RegisteredFrame<'_, '_>, start: usize, target: &[usize]) {
        let current = &self.names[start..];
        if current == target {
            return;
        }
        let moves: Vec<_> = target
            .iter()
            .enumerate()
            .filter_map(|(to, name)| {
                let from = current
                    .iter()
                    .position(|other| other == name)
                    .expect("checked alternatives bind the same names");
                (from != to).then(|| {
                    (
                        self.slot(start + to),
                        frame.load_frame_slot(self.slot(start + from)),
                    )
                })
            })
            .collect();
        for (slot, value) in moves {
            frame.store_frame_slot(slot, &value);
        }
        self.names[start..].copy_from_slice(target);
    }
}

impl RegisteredFrame<'_, '_> {
    /// Publish the native operand or capture array before any released owner can reenter.
    fn replace_temporaries(&mut self, mark: &str, values: &str, count: usize) {
        self.line(&format!(
            "call void @imre_dyn_replace_temporaries(ptr %ctx, i64 {mark}, ptr {values}, i64 {count})"
        ));
    }

    fn pattern_check(&mut self, condition: &str, failed: &str) {
        let matched = self.name("b");
        self.branch(condition, &matched, failed);
        self.label(&matched);
    }
}

impl CheckedBody<'_, '_, '_, '_> {
    /// Test each pattern, then emit its guard and body while the subject and
    /// pattern error target still belong to that case.
    pub(crate) fn emit_prepared_match(&mut self, expr: &Expr, cases: &[MatchCase]) {
        let frame = self.python();
        let mark = frame.temporary_mark();
        let subject = frame.pattern_value(expr);
        let done = frame.name("b");
        // Cases are tested one at a time. Their temporary captures share these
        // frame slots until the pattern succeeds and installs its lexical bindings.
        let captures = frame.private_slot_count;
        frame.private_slot_count += cases
            .iter()
            .map(|case| case.captures.len())
            .max()
            .unwrap_or(0);
        let count = cases.len();
        let default = count > 1 && matches!(cases[count - 1].pattern.test, MatchTest::Always);
        let tested = count - usize::from(default);
        for (index, case) in cases.iter().enumerate() {
            let frame = self.python();
            let pattern = &case.pattern;
            let again = index + 1 < tested;
            let next = frame.name("b");
            let case_mark = if again {
                frame.temporary_mark()
            } else {
                mark.clone()
            };
            let scope = frame.begin_operand_scope();
            if index < tested {
                frame.live.push(subject.clone());
                let slots = captures..captures + case.captures.len();
                let enclosing = frame.live_slots.len();
                for slot in slots.clone() {
                    frame.store_frame_slot(slot, "0");
                }
                frame.live_slots.extend(slots.clone());
                let mut bindings = PatternCaptures {
                    slots: slots.clone(),
                    names: Vec::with_capacity(slots.len()),
                };
                frame.test_pattern(
                    &pattern.test,
                    &subject,
                    &next,
                    &mut bindings,
                    (!again).then_some(mark.as_str()),
                );
                // Captures are stacked in reverse binding order. A failed alternative
                // drops its first captured value first. A successful one binds in source order.
                for (capture, name) in slots.rev().zip(bindings.names) {
                    let value = frame.load_frame_slot(capture);
                    frame.assign(case.captures[name], CheckedValue::handle(value));
                }
                frame.live_slots.truncate(enclosing);
                if !again {
                    frame.live.pop();
                }
                frame.discard_truth(&case_mark);
            }
            if let Some(guard) = &case.guard {
                frame.flush_check();
                let matched = frame.name("b");
                self.branch_condition(guard, &matched, &next);
                self.python().label(&matched);
            }
            let frame = self.python();
            if again {
                frame.live.pop();
                frame.discard_truth(&mark);
            }
            frame.finish_unwind(scope, |frame| frame.release_owned_operands(&mark));
            frame.flush_check();
            self.emit_block(&case.body);
            self.python().jump(&done);
            self.python().label(&next);
            self.python().release_owned_operands(&case_mark);
        }
        self.python().jump(&done);
        self.python().label(&done);
    }
}

impl Frame<'_, '_, '_> {
    fn pattern_value(&mut self, expr: &Expr) -> String {
        self.with_operand_source(expr, |frame| frame.evaluate_scoped_handle(expr))
    }

    fn pattern_equal(&mut self, subject: &str, expr: &Expr, failed: &str, last_use: Option<&str>) {
        let mark = last_use.map_or_else(|| self.temporary_mark(), str::to_owned);
        let target = self.begin_operand_scope();
        let bit = self.with_operand_source(expr, |frame| {
            let expected = frame.pattern_value(expr);
            let equal = frame.comparison_result(&mark, "==", subject, &expected);
            frame.truth(&equal)
        });
        let f = &mut *self.code;
        f.finish_operand_scope(&mark, target);
        f.discard_truth(&mark);
        f.pattern_check(&bit, failed);
    }

    /// Publish the collected operands in stack order. Each completed child can die while
    /// later children and already-captured values remain visible to a Python finalizer.
    fn test_collected_items<'t>(
        &mut self,
        mark: &str,
        values: &str,
        children: impl Iterator<Item = &'t MatchTest> + Clone,
        failed: &str,
        captures: &mut PatternCaptures,
    ) {
        let mut operands = Vec::with_capacity(children.size_hint().0);
        for (i, _) in children.clone().enumerate() {
            let index = crate::llvm::small(i as i64);
            self.take_matching_check();
            operands.push(self.read_index_word(values, &index.operand, "0", None));
        }
        operands.reverse();
        let (ptr, count) = self.handle_array(&operands);
        self.replace_temporaries(mark, ptr, count);
        for (i, (child, item)) in children.zip(operands.iter().rev()).enumerate() {
            let remaining = count - i - 1;
            let boundary = if remaining == 0 {
                mark.to_owned()
            } else {
                self.value(&format!("add i64 {mark}, {remaining}"))
            };
            let mismatch = self.name("b");
            let release = self.name("b");
            self.test_pattern(child, item, &mismatch, captures, None);
            let accepted = self.current_block().to_owned();
            self.jump(&release);
            self.label(&mismatch);
            self.jump(&release);
            self.label(&release);
            let matched = self.value(&format!(
                "phi i1 [ true, %{accepted} ], [ false, %{mismatch} ]"
            ));
            if captures.slots.is_empty() {
                self.release_owned_operands(&boundary);
            } else {
                // Older operands precede the mark. Only the staged bindings can have
                // acquired an additional live reference while testing this child.
                self.replace_temporaries(
                    &boundary,
                    &format!("%ir_slot{}", captures.slots.start),
                    captures.slots.len(),
                );
            }
            self.pattern_check(&matched, failed);
        }
    }

    fn test_pattern(
        &mut self,
        test: &MatchTest,
        value: &str,
        failed: &str,
        captures: &mut PatternCaptures,
        last_use: Option<&str>,
    ) {
        match test {
            MatchTest::Always => {}
            MatchTest::Capture(inner, index) => {
                self.test_pattern(inner, value, failed, captures, None);
                captures.bind(self.code, *index, value);
            }
            MatchTest::Equal(expr) => self.pattern_equal(value, expr, failed, last_use),
            MatchTest::Singleton(s) => {
                let f = &mut *self.code;
                let id = match s {
                    None => super::permanent::NONE,
                    Some(false) => super::permanent::FALSE,
                    Some(true) => super::permanent::TRUE,
                };
                let ok = f.value(&format!("icmp eq i64 {value}, {id}"));
                f.pattern_check(&ok, failed);
            }
            MatchTest::Or(tests) => {
                let mark = self.temporary_mark();
                let first = captures.names.len();
                let mut matched_names = Vec::new();
                let done = self.name("b");
                for (i, child) in tests.iter().enumerate() {
                    let last = i + 1 == tests.len();
                    let next = if last {
                        failed.to_owned()
                    } else {
                        self.name("b")
                    };
                    self.test_pattern(child, value, &next, captures, None);
                    let f = &mut *self.code;
                    if i == 0 {
                        matched_names.extend_from_slice(&captures.names[first..]);
                    } else {
                        captures.reorder(f, first, &matched_names);
                    }
                    f.jump(&done);
                    if !last {
                        f.label(&next);
                        for ordinal in first..captures.names.len() {
                            f.store_frame_slot(captures.slot(ordinal), "0");
                        }
                        f.release_owned_operands(&mark);
                        captures.names.truncate(first);
                    }
                }
                self.label(&done);
            }
            MatchTest::Sequence(plan, items) => {
                self.sequence_test(plan, items, value, failed, captures)
            }
            MatchTest::Mapping(items, rest) => {
                let f = &mut *self.code;
                let minimum = if items.is_empty() {
                    "0".to_owned()
                } else {
                    f.integer(items.len())
                };
                let result = f.op("match_mapping", &[value, &minimum]);
                let ok = f.truth(&result);
                f.pattern_check(&ok, failed);
                let keys = f.op("list", &[]);
                let mut matched = Vec::with_capacity(items.len());
                for (key, _) in items {
                    let key = self.pattern_value(key);
                    let f = &mut *self.code;
                    f.op("append", &[&keys, &key]);
                    matched.push(key);
                }
                let f = &mut *self.code;
                let mark = f.temporary_mark();
                let values = f.op("match_mapping_keys", &[value, &keys]);
                let ok = f.value(&format!("icmp ne i64 {values}, 0"));
                f.pattern_check(&ok, failed);
                self.test_collected_items(
                    &mark,
                    &values,
                    items.iter().map(|(_, child)| child),
                    failed,
                    captures,
                );
                if let Some(index) = rest {
                    let f = &mut *self.code;
                    let rest = f.op("dict", &[]);
                    f.op("update", &[&rest, value]);
                    for key in matched {
                        f.op("delitem", &[&rest, &key]);
                    }
                    captures.bind(f, *index, &rest);
                }
            }
            MatchTest::Class {
                range,
                class,
                positional,
                keywords,
            } => {
                let class = self.pattern_value(class);
                let keys: Vec<_> = keywords
                    .iter()
                    .map(|(key, _)| self.code_name(key))
                    .collect();
                let positional_count = positional.len();
                let mark = self.temporary_mark();
                let fields = self.with_source_range(*range, |frame| {
                    frame.take_matching_check();
                    let (keys, count) = frame.handle_array(&keys);
                    frame.checked(&format!(
                        "call i64 @imre_dyn_match_class(ptr %ctx, i64 {value}, i64 {class}, i64 {positional_count}, ptr {keys}, i64 {count})"
                    ))
                });
                let f = &mut *self.code;
                let ok = f.value(&format!("icmp ne i64 {fields}, 0"));
                f.pattern_check(&ok, failed);
                let children = positional
                    .iter()
                    .chain(keywords.iter().map(|(_, child)| child));
                self.test_collected_items(&mark, &fields, children, failed, captures);
            }
        }
    }

    fn sequence_test(
        &mut self,
        plan: &SequencePatternPlan,
        items: &[MatchTest],
        value: &str,
        failed: &str,
        captures: &mut PatternCaptures,
    ) {
        let f = &mut *self.code;
        let sequence = f.op("match_sequence", &[value]);
        let condition = f.truth(&sequence);
        f.pattern_check(&condition, failed);
        if let Some((op, count)) = plan.length {
            let len = f.op("match_sequence_length", &[value]);
            let required = f.integer(count);
            let valid = f.op(op.as_str(), &[&len, &required]);
            let cond = f.truth(&valid);
            f.pattern_check(&cond, failed);
        }
        match plan.items {
            SequencePatternItems::Ignore => {}
            SequencePatternItems::IndexAround(star) => {
                for (i, child) in items
                    .iter()
                    .enumerate()
                    .filter(|(_, p)| !matches!(p, MatchTest::Always))
                {
                    let f = &mut *self.code;
                    let index = if i < star {
                        f.integer(i)
                    } else {
                        let size = f.op("match_sequence_length", &[value]);
                        let offset = f.integer(items.len() - i);
                        f.op("-", &[&size, &offset])
                    };
                    let item = f.op("get", &[value, &index]);
                    self.test_pattern(child, &item, failed, captures, None);
                }
            }
            SequencePatternItems::Unpack(star) => {
                let count = f.integer(items.len());
                let star = star.map_or_else(|| "0".into(), |i| f.integer(i));
                let mark = f.temporary_mark();
                let values = f.op("unpack", &[value, &count, &star]);
                self.test_collected_items(&mark, &values, items.iter(), failed, captures);
            }
        }
    }
}
