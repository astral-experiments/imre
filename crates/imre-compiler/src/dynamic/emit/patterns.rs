//! Structural patterns and their case guards for runtime-loaded Python.
use ruff_python_ast as ast;

use super::Function;

impl Function<'_, '_> {
    /// Keep the subject across failed cases and publish captures before evaluating a guard.
    pub(super) fn match_statement(&mut self, stmt: &ast::StmtMatch) {
        let subject = self.expr(&stmt.subject);
        let done = self.name("b");
        for case in &stmt.cases {
            let next = self.name("b");
            self.live.push(subject.clone());
            let captures = self.op("dict", &[]);
            let mut names = Vec::new();
            self.pattern(&case.pattern, &subject, &next, &captures, &mut names);
            for name in names {
                let key = self.code_name(&name);
                let value = self.op("get", &[&captures, &key]);
                self.bind(&name, &value);
            }
            if let Some(test) = &case.guard {
                let value = self.expr(test);
                let condition = self.truth(&value);
                self.pattern_check(&condition, &next);
            }
            self.live.pop();
            self.statements(&case.body);
            self.jump(&done);
            self.label(&next);
        }
        self.jump(&done);
        self.label(&done);
    }

    fn pattern_check(&mut self, condition: &str, failed: &str) {
        let matched = self.name("b");
        self.branch(condition, &matched, failed);
        self.label(&matched);
    }

    fn pattern_capture(
        &mut self,
        name: &ast::Identifier,
        value: &str,
        captures: &str,
        names: &mut Vec<String>,
    ) {
        let key = self.code_name(name.as_str());
        self.op("put", &[captures, &key, value]);
        if !names.iter().any(|seen| seen == name.as_str()) {
            names.push(name.to_string());
        }
    }

    fn pattern(
        &mut self,
        pattern: &ast::Pattern,
        value: &str,
        failed: &str,
        captures: &str,
        names: &mut Vec<String>,
    ) {
        match pattern {
            ast::Pattern::MatchAs(p) => {
                if let Some(inner) = &p.pattern {
                    self.pattern(inner, value, failed, captures, names);
                }
                if let Some(name) = &p.name {
                    self.pattern_capture(name, value, captures, names);
                }
            }
            ast::Pattern::MatchValue(p) => {
                let literal = self.expr(&p.value);
                let equal = self.op("==", &[value, &literal]);
                let condition = self.truth(&equal);
                self.pattern_check(&condition, failed);
            }
            ast::Pattern::MatchSingleton(p) => {
                let identity = match p.value {
                    ast::Singleton::None => 0,
                    ast::Singleton::False => 1,
                    ast::Singleton::True => 2,
                };
                let condition = self.value(&format!("icmp eq i64 {value}, {identity}"));
                self.pattern_check(&condition, failed);
            }
            ast::Pattern::MatchMapping(p) => {
                let minimum = if p.keys.is_empty() {
                    "0".to_owned()
                } else {
                    self.integer(p.keys.len())
                };
                let matched = self.op("match_mapping", &[value, &minimum]);
                let condition = self.truth(&matched);
                self.pattern_check(&condition, failed);
                let keys = self.op("list", &[]);
                let mut matched_keys = Vec::new();
                for key in &p.keys {
                    let key = self.expr(key);
                    self.op("append", &[&keys, &key]);
                    matched_keys.push(key);
                }
                let items = self.op("match_mapping_keys", &[value, &keys]);
                let condition = self.value(&format!("icmp ne i64 {items}, 0"));
                self.pattern_check(&condition, failed);
                for (i, child) in p.patterns.iter().enumerate() {
                    let index = self.integer(i);
                    let item = self.op("get", &[&items, &index]);
                    self.pattern(child, &item, failed, captures, names);
                }
                if let Some(name) = &p.rest {
                    let rest = self.op("dict", &[]);
                    self.op("update", &[&rest, value]);
                    for key in matched_keys {
                        self.op("delitem", &[&rest, &key]);
                    }
                    self.pattern_capture(name, &rest, captures, names);
                }
            }
            ast::Pattern::MatchSequence(p) => {
                let len = self.op("match_sequence_length", &[value]);
                let condition = self.value(&format!("icmp ne i64 {len}, 0"));
                self.pattern_check(&condition, failed);
                let star = p
                    .patterns
                    .iter()
                    .position(|p| matches!(p, ast::Pattern::MatchStar(_)));
                let required = self.integer(p.patterns.len() - usize::from(star.is_some()));
                let valid = self.op(if star.is_some() { ">=" } else { "==" }, &[&len, &required]);
                let condition = self.truth(&valid);
                self.pattern_check(&condition, failed);
                let count = self.integer(p.patterns.len());
                let star = star.map_or_else(|| "0".to_owned(), |i| self.integer(i));
                let items = self.op("unpack", &[value, &count, &star]);
                for (i, child) in p.patterns.iter().enumerate() {
                    let index = self.integer(i);
                    let item = self.op("get", &[&items, &index]);
                    self.pattern(child, &item, failed, captures, names);
                }
            }
            ast::Pattern::MatchStar(p) => {
                if let Some(name) = &p.name {
                    self.pattern_capture(name, value, captures, names);
                }
            }
            ast::Pattern::MatchOr(p) => {
                let done = self.name("b");
                for (i, alternative) in p.patterns.iter().enumerate() {
                    let last = i + 1 == p.patterns.len();
                    let next = if last {
                        failed.to_owned()
                    } else {
                        self.name("b")
                    };
                    self.pattern(alternative, value, &next, captures, names);
                    self.jump(&done);
                    if !last {
                        self.label(&next);
                    }
                }
                self.label(&done);
            }
            ast::Pattern::MatchClass(_) => {
                self.unsupported("class pattern");
            }
        }
    }
}
