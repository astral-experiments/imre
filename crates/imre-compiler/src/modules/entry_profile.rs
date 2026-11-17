//! A bounded entry profile keeps the original Python tests and traps excluded branches.

use super::{ModuleId, SourceModule};
use ruff_python_ast as ast;
use ruff_python_ast::visitor::{Visitor, walk_stmt};
use ruff_text_size::{Ranged, TextRange, TextSize};
use std::collections::{BTreeMap, HashMap};
use std::path::Path;

type ExcludedSuites = HashMap<(ModuleId, TextSize), (TextRange, String)>;

#[derive(Default)]
pub(crate) struct Profile {
    // Even a profile with no excluded suites requires a statically compiled entry.
    suites: Option<ExcludedSuites>,
}

fn name(expr: &ast::Expr) -> Option<String> {
    match expr {
        ast::Expr::Name(n) => Some(n.id.to_string()),
        ast::Expr::Attribute(a) => Some(format!("{}.{}", name(&a.value)?, a.attr)),
        _ => None,
    }
}

impl Profile {
    pub(super) fn load(
        path: &Path,
        modules: &[SourceModule],
        fs: &mut crate::watch::FileSystem<'_>,
    ) -> Result<Self, String> {
        let definition = fs
            .read(path)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        let mut rules: BTreeMap<String, BTreeMap<String, String>> =
            serde_json::from_slice(&definition).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut suites = ExcludedSuites::new();
        for (module, source) in modules.iter().enumerate() {
            for statement in &source.parsed.syntax().body {
                let ast::Stmt::FunctionDef(function) = statement else {
                    continue;
                };
                let qualified = format!("{}.{}", source.name, function.name);
                let Some(constraints) = rules.remove(&qualified) else {
                    continue;
                };
                for (subject, expected) in constraints {
                    let mut scan = Selector {
                        suites: &mut suites,
                        module,
                        subject: &subject,
                        expected: &expected,
                        changed: false,
                    };
                    for stmt in &function.body {
                        scan.visit_stmt(stmt);
                    }
                    if !scan.changed {
                        return Err(format!(
                            "entry profile: {qualified} has no supported branch for {subject} = {expected}"
                        ));
                    }
                }
            }
        }
        if let Some(name) = rules.keys().next() {
            return Err(format!("entry profile: unknown function {name}"));
        }
        Ok(Self {
            suites: Some(suites),
        })
    }

    pub(crate) fn requires_fixed_entry(&self) -> bool {
        self.suites.is_some()
    }

    pub(crate) fn has_exclusions(&self) -> bool {
        self.suites
            .as_ref()
            .is_some_and(|suites| !suites.is_empty())
    }

    pub(crate) fn excluded(&self, module: ModuleId, start: TextSize) -> Option<&str> {
        self.suites
            .as_ref()?
            .get(&(module, start))
            .map(|(_, explanation)| explanation.as_str())
    }

    pub(crate) fn ranges(&self, module: ModuleId) -> impl Iterator<Item = TextRange> + '_ {
        self.suites
            .iter()
            .flatten()
            .filter(move |((owner, _), _)| *owner == module)
            .map(|(_, (range, _))| *range)
    }
}

struct Selector<'a> {
    suites: &'a mut ExcludedSuites,
    module: ModuleId,
    subject: &'a str,
    expected: &'a str,
    changed: bool,
}

impl Selector<'_> {
    fn exclude(&mut self, body: &[ast::Stmt]) {
        if let (Some(first), Some(last)) = (body.first(), body.last()) {
            let range = TextRange::new(first.start(), last.end());
            self.suites.insert(
                (self.module, first.start()),
                (
                    range,
                    format!(
                        "this executable requires {} = {}",
                        self.subject, self.expected
                    ),
                ),
            );
            self.changed = true;
        }
    }
}

impl<'a> Visitor<'a> for Selector<'_> {
    fn visit_stmt(&mut self, stmt: &'a ast::Stmt) {
        match stmt {
            ast::Stmt::FunctionDef(_) | ast::Stmt::ClassDef(_) => return,
            ast::Stmt::If(branch)
                if name(&branch.test).as_deref() == Some(self.subject)
                    && self.expected == "False" =>
            {
                self.exclude(&branch.body)
            }
            ast::Stmt::Match(branch) if name(&branch.subject).as_deref() == Some(self.subject) => {
                let wanted = |case: &ast::MatchCase| matches!(&case.pattern, ast::Pattern::MatchValue(value) if name(&value.value).as_deref() == Some(self.expected));
                // Bindings, guards and user comparison protocols can change which arm wins.
                // Only accept an explicit member and a series of simple value/default patterns.
                if branch.cases.iter().any(wanted) && branch.cases.iter().all(|case| case.guard.is_none() && (matches!(&case.pattern, ast::Pattern::MatchValue(value) if name(&value.value).is_some()) || case.pattern.is_irrefutable())) {
                    for case in &branch.cases { if !wanted(case) { self.exclude(&case.body); } }
                }
            }
            _ => {}
        }
        walk_stmt(self, stmt);
    }
}

#[cfg(test)]
mod tests {
    use crate::{CompileOptions, check_file, compile_file};
    use std::fs;

    #[test]
    fn empty_profile_still_requires_a_fixed_class_base() {
        let dir = tempfile::tempdir().unwrap();
        let entry = dir.path().join("main.py");
        let profile = dir.path().join("profile.json");
        fs::write(&entry, "from typing import NamedTuple as Base\nclass Row(Base):\n    value: int\nprint(Row(1).value)\n").unwrap();
        fs::write(&profile, "{}").unwrap();
        check_file(&entry, None).unwrap();
        let options = CompileOptions {
            entry_profile: Some(profile),
            ..Default::default()
        };
        let error = compile_file(&entry, None, &options).err().unwrap();
        assert!(
            error.contains(":2:11: Imre: the base must be a known fixed-layout class"),
            "{error}"
        );
    }

    #[test]
    fn excluded_suite_traps_and_remains_an_input() {
        let dir = tempfile::tempdir().unwrap();
        let entry = dir.path().join("main.py");
        let profile = dir.path().join("profile.json");
        fs::write(&entry, "from service import choose\nprint(choose(False))\n").unwrap();
        fs::write(
            dir.path().join("service.py"),
            r#"
def choose(debug: bool) -> str:
    if debug:
        print(globals())
    return "report"
"#,
        )
        .unwrap();
        let options = CompileOptions {
            entry_profile: Some(profile.clone()),
            ..Default::default()
        };
        assert!(check_file(&entry, None).is_err());
        fs::write(&profile, r#"{"service.choose":{"debug":"False"}}"#).unwrap();
        let result = compile_file(&entry, None, &options).unwrap();
        assert!(result.input_paths.contains(&profile));
        let graph = super::super::SourceGraph::load_with(
            &entry,
            None,
            &options,
            Default::default(),
            &mut None,
            &mut Default::default(),
        )
        .unwrap();
        let (module, service) = graph
            .modules
            .iter()
            .enumerate()
            .find(|(_, m)| m.name == "service")
            .unwrap();
        let range = graph.entry_profile.ranges(module).next().unwrap();
        assert_eq!(
            graph.entry_profile.excluded(module, range.start()),
            Some("this executable requires debug = False")
        );
        assert_eq!(&service.source[range], "print(globals())");
        fs::write(&profile, r#"{"service.choose":{"stale":"False"}}"#).unwrap();
        assert!(
            compile_file(&entry, None, &options)
                .err()
                .unwrap()
                .contains("no supported branch")
        );
    }

    #[test]
    fn correlated_optional_argument_requires_the_callee_fallback_profile() {
        let dir = tempfile::tempdir().unwrap();
        let entry = dir.path().join("main.py");
        let profile = dir.path().join("profile.json");
        fs::write(
            &entry,
            "from service import Command, dispatch\nprint(dispatch(Command.check, None))\n",
        )
        .unwrap();
        fs::write(
            dir.path().join("service.py"),
            r#"
from enum import Enum
class Command(Enum):
    check = 1
    format = 2
class Comparison(Enum):
    selected = 1
    other = 2
def check(value: int) -> int:
    return value
def format(value: Comparison) -> int:
    match value:
        case Comparison.selected:
            return 1
        case Comparison.other:
            return 2
        case _:
            raise ValueError("unsupported comparison")
def dispatch(command: Command, comparison: Comparison | None) -> int:
    match command:
        case Command.check:
            selected, value = check, 1
        case Command.format:
            selected, value = format, comparison
        case _:
            raise ValueError("unsupported command")
    return selected(value)
"#,
        )
        .unwrap();
        let diagnostics = |options: &CompileOptions| {
            let mut graph = super::super::SourceGraph::load_with(
                &entry,
                None,
                options,
                Default::default(),
                &mut None,
                &mut Default::default(),
            )?;
            let (skipped, overloads) = graph.diagnostic_scopes();
            graph.session.set_closed_program_rules();
            graph.session.closed_program_errors(
                &graph.modules,
                skipped.as_deref(),
                &overloads,
                None,
            )
        };
        let error = diagnostics(&CompileOptions::default()).unwrap_err();
        assert!(
            error.contains("invalid-argument-type") && error.contains("None"),
            "{error}"
        );
        fs::write(
            &profile,
            r#"{"service.format":{"value":"Comparison.selected"}}"#,
        )
        .unwrap();
        diagnostics(&CompileOptions {
            entry_profile: Some(profile.clone()),
            ..Default::default()
        })
        .unwrap();
        fs::write(&profile, "{}").unwrap();
        let error = diagnostics(&CompileOptions {
            entry_profile: Some(profile),
            ..Default::default()
        })
        .unwrap_err();
        assert!(
            error.contains("invalid-argument-type") && error.contains("None"),
            "{error}"
        );
    }
}
