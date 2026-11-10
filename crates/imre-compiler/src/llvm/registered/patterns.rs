//! Native structural matching on semantic operands in the live registered namespace.
use super::{Frame, RegisteredFrame, sites::CheckedSites};
use crate::frontend::{MatchGuard, SourceMatch};
use crate::ir::{Expr, MatchCase, MatchPattern, MatchTest};
use crate::llvm::operations::{CheckedBody, CheckedValue};
use imre_ast::{SequencePatternItems, SequencePatternPlan};

/// Tests, captures and successor edges for one evaluation of a match subject.
pub(crate) struct MatchRegions<'a> {
    done: String,
    cases: Vec<CaseBlock<'a>>,
    index: usize,
    guard_unwind: Option<String>,
}

struct CaseBlock<'a> {
    body: String,
    guard: Option<GuardBlock<'a>>,
}

struct GuardBlock<'a> {
    test: MatchGuard<&'a Expr>,
    entry: String,
    matched: String,
    failed: String,
    unwind: String,
    subject: Option<String>,
}

pub(crate) enum MatchRegion<'a> {
    ResidualGuard {
        case: usize,
        matched: &'a str,
        failed: &'a str,
    },
    Body(usize),
}

pub(crate) enum MatchSubject<'a> {
    Checked(&'a Expr),
    Evaluated(String),
}

/// Both representations borrow their operands directly from their declaring case.
enum MatchCases<'a> {
    Checked(&'a [MatchCase]),
    Physical(&'a SourceMatch),
}

struct CaseInput<'a> {
    pattern: &'a MatchPattern,
    slots: Option<&'a [usize]>,
    guard: Option<MatchGuard<&'a Expr>>,
}

impl<'a> MatchCases<'a> {
    fn len(&self) -> usize {
        match self {
            Self::Checked(cases) => cases.len(),
            Self::Physical(site) => site.cases.len(),
        }
    }

    fn get(&self, index: usize) -> CaseInput<'a> {
        match self {
            Self::Checked(cases) => {
                let case = &cases[index];
                CaseInput {
                    pattern: &case.pattern,
                    slots: Some(&case.captures),
                    guard: case.guard.as_ref().map(MatchGuard::Checked),
                }
            }
            Self::Physical(site) => {
                let case = &site.cases[index];
                CaseInput {
                    pattern: &case.pattern,
                    slots: None,
                    guard: case.guard.as_ref().map(MatchGuard::as_ref),
                }
            }
        }
    }
}

impl MatchRegions<'_> {
    /// Give the source owner a chance to register definitions in this guard.
    pub(crate) fn pending_checked_guard(&self) -> Option<&Expr> {
        if self.guard_unwind.is_some() {
            return None;
        }
        match self.cases.get(self.index)?.guard.as_ref()?.test {
            MatchGuard::Checked(test) => Some(test),
            MatchGuard::Residual => None,
        }
    }

    /// Resume on the same live frame after the owner filled the previous operand.
    fn next(&mut self, code: &mut CheckedBody<'_, '_, '_, '_>) -> Option<MatchRegion<'_>> {
        let index = self.index;
        if let Some(outer) = self.guard_unwind.take() {
            self.cases[index]
                .guard
                .as_ref()
                .unwrap()
                .leave(code.python().code, outer);
        } else {
            if index != 0 {
                code.python().jump(&self.done);
            }
            let Some(case) = self.cases.get(index) else {
                code.python().label(&self.done);
                return None;
            };
            if let Some(guard) = &case.guard {
                let outer = guard.enter(code.python().code);
                match guard.test {
                    MatchGuard::Checked(test) => {
                        code.branch_condition(test, &guard.matched, &guard.failed);
                        guard.leave(code.python().code, outer);
                    }
                    MatchGuard::Residual => {
                        self.guard_unwind = Some(outer);
                        return Some(MatchRegion::ResidualGuard {
                            case: index,
                            matched: &guard.matched,
                            failed: &guard.failed,
                        });
                    }
                }
            }
        }
        code.python().label(&self.cases[index].body);
        self.index += 1;
        Some(MatchRegion::Body(index))
    }
}

impl GuardBlock<'_> {
    fn enter(&self, frame: &mut RegisteredFrame<'_, '_>) -> String {
        frame.label(&self.entry);
        if let Some(subject) = &self.subject {
            frame.live.push(subject.clone());
        }
        std::mem::replace(&mut frame.body.error_target, self.unwind.clone())
    }

    fn leave(&self, frame: &mut RegisteredFrame<'_, '_>, outer: String) {
        if self.subject.is_some() {
            frame.live.pop();
        }
        frame.body.error_target = outer;
    }
}

impl RegisteredFrame<'_, '_> {
    pub(crate) fn registered_match<'a>(
        &mut self,
        subject: MatchSubject<'_>,
        mark: &str,
        site: &'a SourceMatch,
        sites: &CheckedSites,
    ) -> MatchRegions<'a> {
        self.with_checked_frame(sites, None, |mut frame| {
            frame.start_match(subject, mark, MatchCases::Physical(site))
        })
    }

    pub(crate) fn next_match_region<'r>(
        &mut self,
        regions: &'r mut MatchRegions<'_>,
        sites: &CheckedSites,
    ) -> Option<MatchRegion<'r>> {
        self.with_checked_frame(sites, None, |frame| {
            regions.next(&mut CheckedBody::registered(frame))
        })
    }

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
    pub(crate) fn emit_prepared_match(&mut self, expr: &Expr, cases: &[MatchCase]) {
        let mark = self.python().temporary_mark();
        let mut regions = self.python().start_match(
            MatchSubject::Checked(expr),
            &mark,
            MatchCases::Checked(cases),
        );
        while let Some(region) = regions.next(self) {
            match region {
                MatchRegion::Body(case) => self.emit_block(&cases[case].body),
                MatchRegion::ResidualGuard { .. } => unreachable!("checked match guard"),
            }
        }
    }
}

impl Frame<'_, '_, '_> {
    fn pattern_value(&mut self, expr: &Expr) -> String {
        let outer = self.expression_integer_start;
        self.expression_integer_start = Some(outer.unwrap_or(self.integer_owners));
        let result = self.evaluate_handle(expr);
        self.expression_integer_start = outer;
        result
    }

    fn pattern_equal(&mut self, subject: &str, expr: &Expr, failed: &str, last_use: Option<&str>) {
        let mark = last_use.map_or_else(|| self.temporary_mark(), str::to_owned);
        let target = self.begin_operand_scope();
        let expected = self.pattern_value(expr);
        let f = &mut *self.code;
        let equal = f.comparison_result(&mark, "==", subject, &expected);
        let bit = f.truth(&equal);
        f.finish_operand_scope(&mark, target);
        f.discard_truth(&mark);
        f.pattern_check(&bit, failed);
    }

    fn start_match<'a>(
        &mut self,
        subject: MatchSubject<'_>,
        mark: &str,
        cases: MatchCases<'a>,
    ) -> MatchRegions<'a> {
        let handle = match subject {
            MatchSubject::Checked(expr) => self.pattern_value(expr),
            MatchSubject::Evaluated(handle) => handle,
        };
        let subject = handle.as_str();
        let count = cases.len();
        let default = count > 1 && matches!(cases.get(count - 1).pattern.test, MatchTest::Always);
        let tested = count - usize::from(default);
        let done = self.name("b");
        let mut blocks = Vec::with_capacity(count);
        for index in 0..count {
            let CaseInput {
                pattern,
                slots,
                guard,
            } = cases.get(index);
            let again = index + 1 < tested;
            let next = self.name("b");
            let case_mark = if again {
                self.temporary_mark()
            } else {
                mark.to_owned()
            };
            let scope = self.begin_operand_scope();
            if index < tested {
                self.live.push(subject.to_owned());
                let captures = if pattern.names.is_empty() {
                    "0".to_owned()
                } else {
                    self.op("dict", &[])
                };
                self.test_pattern(
                    &pattern.test,
                    subject,
                    &next,
                    &captures,
                    (!again).then_some(mark),
                );
                for (i, name) in pattern.names.iter().enumerate() {
                    let key = self.code_name(name);
                    let val = self.op("get", &[&captures, &key]);
                    if let Some(slots) = slots {
                        self.assign(slots[i], CheckedValue::handle(val));
                    } else {
                        self.code.bind(name, &val);
                    }
                }
                if !again {
                    self.live.pop();
                }
                self.discard_truth(&case_mark);
            }
            let guard = guard.map(|test| {
                let entry = self.name("b");
                let matched = self.name("b");
                self.jump(&entry);
                self.label(&matched);
                GuardBlock {
                    test,
                    entry,
                    matched,
                    failed: next.clone(),
                    unwind: self.body.error_target.clone(),
                    subject: again.then(|| subject.to_owned()),
                }
            });
            if again {
                self.live.pop();
                self.discard_truth(mark);
            }
            self.finish_operand_scope(mark, scope);
            let body = self.name("b");
            self.jump(&body);
            self.label(&next);
            self.discard_truth(&case_mark);
            blocks.push(CaseBlock { body, guard });
        }
        self.jump(&done);
        MatchRegions {
            done,
            cases: blocks,
            index: 0,
            guard_unwind: None,
        }
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
                let fields = f.op("match_class", &[value, &class, &keys]);
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
