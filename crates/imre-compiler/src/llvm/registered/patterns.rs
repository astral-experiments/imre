//! Native structural matching on semantic operands in the live registered namespace.
use super::{Frame, RegisteredFrame};
use crate::ir::{Expr, MatchCase, MatchTest};
use crate::llvm::operations::{CheckedBody, CheckedValue};
use imre_ast::{SequencePatternItems, SequencePatternPlan};

impl RegisteredFrame<'_, '_> {
    fn pattern_check(&mut self, condition: &str, failed: &str) {
        let matched = self.name("b");
        self.branch(condition, &matched, failed);
        self.label(&matched);
    }

    fn capture(&mut self, name: &str, value: &str, captures: &str) {
        let key = self.code_name(name);
        self.op("put", &[captures, &key, value]);
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
                let captures = if pattern.names.is_empty() {
                    "0".to_owned()
                } else {
                    frame.op("dict", &[])
                };
                frame.test_pattern(
                    &pattern.test,
                    &subject,
                    &next,
                    &captures,
                    (!again).then_some(mark.as_str()),
                );
                for (i, name) in pattern.names.iter().enumerate() {
                    let key = frame.code_name(name);
                    let value = frame.op("get", &[&captures, &key]);
                    frame.assign(case.captures[i], CheckedValue::handle(value));
                }
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
            frame.finish_operand_scope(&mark, scope);
            frame.flush_check();
            self.emit_block(&case.body);
            self.python().jump(&done);
            self.python().label(&next);
            self.python().discard_truth(&case_mark);
        }
        self.python().jump(&done);
        self.python().label(&done);
    }
}

impl Frame<'_, '_, '_> {
    fn pattern_value(&mut self, expr: &Expr) -> String {
        let outer = self.expression_integer_start;
        self.expression_integer_start = Some(outer.unwrap_or(self.integer_owners));
        let result = self.evaluate_eager_handle(expr);
        self.expression_integer_start = outer;
        result
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

    fn test_pattern(
        &mut self,
        test: &MatchTest,
        value: &str,
        failed: &str,
        captures: &str,
        last_use: Option<&str>,
    ) {
        match test {
            MatchTest::Always => {}
            MatchTest::Capture(inner, name) => {
                self.test_pattern(inner, value, failed, captures, None);
                self.capture(name, value, captures);
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
                    f.jump(&done);
                    if !last {
                        f.label(&next);
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
                let values = f.op("match_mapping_keys", &[value, &keys]);
                let ok = f.value(&format!("icmp ne i64 {values}, 0"));
                f.pattern_check(&ok, failed);
                for (i, (_, child)) in items.iter().enumerate() {
                    let f = &mut *self.code;
                    let i = f.integer(i);
                    let item = f.op("get", &[&values, &i]);
                    self.test_pattern(child, &item, failed, captures, None);
                }
                if let Some(name) = rest {
                    let f = &mut *self.code;
                    let rest = f.op("dict", &[]);
                    f.op("update", &[&rest, value]);
                    for key in matched {
                        f.op("delitem", &[&rest, &key]);
                    }
                    f.capture(name, &rest, captures);
                }
            }
            MatchTest::Class {
                range,
                class,
                positional,
                keywords,
            } => {
                let class = self.pattern_value(class);
                let f = &mut *self.code;
                let keys = f.op("list", &[]);
                for _ in positional {
                    f.op("append", &[&keys, &super::permanent::NONE.to_string()]);
                }
                for (key, _) in keywords {
                    let key = f.code_name(key);
                    f.op("append", &[&keys, &key]);
                }
                let fields = self.with_source_range(*range, |frame| {
                    frame.op("match_class", &[value, &class, &keys])
                });
                let f = &mut *self.code;
                let ok = f.value(&format!("icmp ne i64 {fields}, 0"));
                f.pattern_check(&ok, failed);
                for (i, child) in positional
                    .iter()
                    .chain(keywords.iter().map(|(_, child)| child))
                    .enumerate()
                {
                    let f = &mut *self.code;
                    let i = f.integer(i);
                    let val = f.op("get", &[&fields, &i]);
                    self.test_pattern(child, &val, failed, captures, None);
                }
            }
        }
    }

    fn sequence_test(
        &mut self,
        plan: &SequencePatternPlan,
        items: &[MatchTest],
        value: &str,
        failed: &str,
        captures: &str,
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
                let values = f.op("unpack", &[value, &count, &star]);
                for (i, child) in items.iter().enumerate() {
                    let f = &mut *self.code;
                    let i = f.integer(i);
                    let item = f.op("get", &[&values, &i]);
                    self.test_pattern(child, &item, failed, captures, None);
                }
            }
        }
    }
}
