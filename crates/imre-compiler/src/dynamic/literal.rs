//! Source-local exec occurrences selected by registered import resolution.
use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use ruff_db::{files::File, parsed::ParsedModuleRef};
use ruff_python_ast::visitor::Visitor;
use ruff_text_size::{TextRange, TextSize};

use super::Imports;
use super::emit::{ModuleOrigin, PreparedBody, PreparedEval};
use crate::analysis::Session;
use crate::frontend::module_source::ParsedModule;
use crate::llvm::registered::sites::CompiledSourceMode;

/// The existing IR call range identifies the operation. Python reports its source
/// argument's position when a selected child cannot be compiled.
pub(crate) struct LiteralSite {
    pub(crate) call: TextRange,
    pub(crate) operand: TextSize,
    pub(crate) optional: bool,
    pub(crate) file: Option<PathBuf>,
}

/// A call with a fixed source operand; a write candidate also names its prospective file.
pub(crate) struct LiteralCandidate {
    pub(crate) call: TextRange,
    pub(crate) file: Option<PathBuf>,
}

/// The selected source is either a prepared suite or a checked literal expression.
pub(super) enum LiteralBody<'a> {
    Suite(&'a PreparedLiteral),
    Expression(&'a str, Option<&'a PreparedEval>),
}

/// Checked static bodies and narrowly guarded runtime execution sites at one source level.
#[derive(Default)]
pub(crate) struct SelectedLiteralLevel {
    pub(crate) literals: Vec<(LiteralSite, Result<ParsedLiteral, String>)>,
    pub(crate) bounded_file_exec: Vec<TextRange>,
}

pub(super) struct PreparedLiteral {
    pub(super) body: PreparedBody<'static>,
    /// Original operand bytes when source decoding or normalization changed the checked text.
    pub(super) original_text: Option<Box<str>>,
    pub(super) mode: CompiledSourceMode,
    pub(super) file: Option<PathBuf>,
}

/// One source slot's checked operands. Different modules named "<string>" have
/// independent tables, and a live rebinding still receives the original string.
enum LiteralEntry {
    Suites(Vec<PreparedLiteral>),
    Expression(PreparedEval),
}

#[derive(Default)]
pub(crate) struct LiteralSites {
    sites: HashMap<TextRange, Result<LiteralEntry, (TextSize, String)>>,
    bounded_file_exec: HashSet<TextRange>,
}

/// The source unit's literal records and actual Python optimization during checking.
#[derive(Clone, Copy)]
pub(crate) struct LiteralUnit<'a> {
    pub(crate) sites: &'a std::cell::RefCell<BTreeMap<usize, LiteralSites>>,
    pub(crate) unit: usize,
    pub(crate) optimization: i32,
}

impl LiteralUnit<'_> {
    /// Prepare a selected eval expression once without displacing a suite or refusal.
    pub(crate) fn prepare_eval(self, site: TextRange, text: &str) -> Result<(), String> {
        let mut units = self.sites.borrow_mut();
        let sites = &mut units.entry(self.unit).or_default().sites;
        if let Some(existing) = sites.get(&site) {
            return match existing {
                Ok(LiteralEntry::Expression(entry)) if entry.text() == text => Ok(()),
                Ok(LiteralEntry::Expression(_)) => {
                    Err("literal source changed during checking".into())
                }
                Ok(LiteralEntry::Suites(_)) => {
                    Err("this site already owns a compiled source suite".into())
                }
                Err((_, reason)) => Err(reason.clone()),
            };
        }
        let entry = PreparedEval::new(text, self.optimization)?;
        sites.insert(site, Ok(LiteralEntry::Expression(entry)));
        Ok(())
    }
}

impl LiteralSites {
    /// Borrow the selected suites or the one checked expression at this call.
    pub(super) fn plan<'a>(
        &'a self,
        site: TextRange,
        expression: Option<&'a str>,
    ) -> Option<Result<impl Iterator<Item = LiteralBody<'a>>, (TextSize, &'a str)>> {
        let (suites, expression, checked) = match self.sites.get(&site) {
            Some(Ok(LiteralEntry::Suites(suites))) => (suites.as_slice(), None, None),
            Some(Ok(LiteralEntry::Expression(entry))) => {
                let text = expression.filter(|text| *text == entry.text())?;
                (&[][..], Some(text), Some(entry))
            }
            Some(Err((operand, reason))) => return Some(Err((*operand, reason.as_str()))),
            None => (&[][..], Some(expression?), None),
        };
        Some(Ok(suites.iter().map(LiteralBody::Suite).chain(
            expression.map(|text| LiteralBody::Expression(text, checked)),
        )))
    }

    fn insert(&mut self, site: LiteralSite, body: Result<PreparedLiteral, String>) {
        match body {
            Ok(mut body) => {
                body.file = site.file;
                if let Ok(LiteralEntry::Suites(bodies)) = self
                    .sites
                    .entry(site.call)
                    .or_insert_with(|| Ok(LiteralEntry::Suites(Vec::new())))
                {
                    bodies.push(body);
                }
            }
            Err(reason) if !site.optional => {
                self.sites.insert(site.call, Err((site.operand, reason)));
            }
            Err(_) => {}
        }
    }

    pub(crate) fn contains(&self, site: TextRange) -> bool {
        self.sites.contains_key(&site) || self.bounded_file_exec.contains(&site)
    }

    pub(super) fn bounded_file_exec(&self, site: TextRange) -> bool {
        self.bounded_file_exec.contains(&site)
    }

    pub(crate) fn refused(&self, site: TextRange) -> bool {
        self.sites.get(&site).is_some_and(Result::is_err)
    }

    pub(crate) fn has_refusals(&self) -> bool {
        self.sites.values().any(Result::is_err)
    }
}

/// The text and the syntax of one real operand. Its virtual file belongs to the
/// parent application's database. Only absolute imports are known independently
/// of the globals and locals supplied to the call.
pub(crate) struct ParsedLiteral {
    pub(crate) file: File,
    pub(crate) text: String,
    original_text: Option<Box<str>>,
    pub(crate) parsed: ParsedModuleRef,
    pub(crate) flags: u32,
    mode: CompiledSourceMode,
    imports: Vec<String>,
    candidates: Vec<LiteralCandidate>,
}

impl ParsedLiteral {
    pub(crate) fn new(
        file: File,
        text: Cow<'_, str>,
        original: &str,
        parsed: ParsedModuleRef,
        inherited_flags: u32,
        mode: CompiledSourceMode,
    ) -> Self {
        let body = &parsed.syntax().body;
        let flags = inherited_flags | imre_ast::future_flags(body);
        let mut imports = Imports::default();
        imports.private_names.string_annotations =
            flags & imre_ast::code_flags::FUTURE_ANNOTATIONS != 0;
        imports.visit_body(body);
        Self {
            file,
            original_text: (text.as_ref() != original).then(|| original.into()),
            text: text.into_owned(),
            parsed,
            flags,
            mode,
            candidates: imports.literal_calls,
            imports: imports
                .names
                .into_iter()
                .filter_map(|(level, name)| (level == 0).then_some(name))
                .collect(),
        }
    }
    fn prepare(
        self,
        python_optimization: i32,
        python_annotations: bool,
        children: LiteralSites,
    ) -> Result<PreparedLiteral, String> {
        let body = PreparedBody::from_parsed(
            "<module>",
            Path::new("<string>"),
            ParsedModule {
                text: Cow::Owned(self.text),
                parsed: Cow::Borrowed(&self.parsed),
            },
            python_optimization,
            ModuleOrigin::ExecLiteral {
                checked: &self.parsed,
                flags: self.flags,
                python_annotations,
            },
            children,
            Default::default(),
        )?;
        Ok(PreparedLiteral {
            body,
            original_text: self.original_text,
            mode: self.mode,
            file: None,
        })
    }
}

#[derive(Clone, Copy)]
enum Parent {
    Unit(usize),
    Literal(usize),
}

struct PendingLiteral {
    unit: usize,
    parent: Parent,
    site: LiteralSite,
    source: ParsedLiteral,
    children: LiteralSites,
}

/// Discovery records the immediate imports of a parsed operand before resolving
/// names inside it. An import may supply or shadow exec in that very suite.
#[derive(Default)]
pub(super) struct LiteralForest {
    roots: BTreeMap<usize, LiteralSites>,
    nodes: Vec<PendingLiteral>,
    next: usize,
}

impl LiteralForest {
    pub(super) fn record_root(&mut self, unit: usize, sites: SelectedLiteralLevel) -> Vec<String> {
        self.record(Parent::Unit(unit), unit, sites)
    }

    pub(super) fn has_pending(&self) -> bool {
        self.next < self.nodes.len()
    }

    /// The caller has finished loading the previous level's imports. Leave
    /// newly parsed children for the next pass through the same import queue.
    pub(super) fn advance(&mut self, session: &Session) -> Vec<(usize, Vec<String>)> {
        let start = self.next;
        self.next = self.nodes.len();
        let mut imports = Vec::new();
        for index in start..self.next {
            let parent = &self.nodes[index];
            let unit = parent.unit;
            let source = &parent.source;
            let sites = session.select_literal_level(
                source.file,
                &source.parsed,
                &source.text,
                source.flags,
                &source.candidates,
                (&[], &[]),
            );
            let names = self.record(Parent::Literal(index), unit, sites);
            if !names.is_empty() {
                imports.push((unit, names));
            }
        }
        imports
    }

    fn table(&mut self, parent: Parent) -> &mut LiteralSites {
        match parent {
            Parent::Unit(unit) => self.roots.entry(unit).or_default(),
            Parent::Literal(index) => &mut self.nodes[index].children,
        }
    }

    fn record(&mut self, parent: Parent, unit: usize, sites: SelectedLiteralLevel) -> Vec<String> {
        self.table(parent)
            .bounded_file_exec
            .extend(sites.bounded_file_exec);
        let mut names = Vec::new();
        for (site, result) in sites.literals {
            match result {
                Ok(mut source) => {
                    names.append(&mut source.imports);
                    self.nodes.push(PendingLiteral {
                        unit,
                        parent,
                        site,
                        source,
                        children: LiteralSites::default(),
                    });
                }
                Err(error) => {
                    self.table(parent).insert(site, Err(error));
                }
            }
        }
        names
    }

    /// Once the named import closure is stable, consume each owned parse once.
    /// Children have larger indices than their parents, so their checked bodies
    /// are available when the ordinary module compiler prepares the parent.
    pub(super) fn finish(
        mut self,
        session: &Session,
        python_optimization: i32,
        python_annotations: impl Fn(usize) -> bool,
    ) -> BTreeMap<usize, LiteralSites> {
        debug_assert!(!self.has_pending());
        while let Some(node) = self.nodes.pop() {
            let module = session.check_literal(&node.source).and_then(|()| {
                node.source.prepare(
                    python_optimization,
                    python_annotations(node.unit),
                    node.children,
                )
            });
            self.table(node.parent).insert(node.site, module);
        }
        self.roots
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eval_preserves_an_existing_suite_or_refusal() {
        let units = std::cell::RefCell::new(BTreeMap::new());
        let site = TextRange::new(0.into(), 1.into());
        let unit = LiteralUnit {
            sites: &units,
            unit: 0,
            optimization: 0,
        };
        units
            .borrow_mut()
            .entry(0)
            .or_default()
            .sites
            .insert(site, Ok(LiteralEntry::Suites(Vec::new())));
        assert!(unit.prepare_eval(site, "1").is_err());
        assert!(matches!(
            units.borrow()[&0].sites.get(&site),
            Some(Ok(LiteralEntry::Suites(_)))
        ));
        units
            .borrow_mut()
            .get_mut(&0)
            .unwrap()
            .sites
            .insert(site, Err((0.into(), "original refusal".into())));
        assert_eq!(
            unit.prepare_eval(site, "1").unwrap_err(),
            "original refusal"
        );
        assert!(
            matches!(units.borrow()[&0].sites.get(&site), Some(Err((_, reason))) if reason == "original refusal")
        );
    }
}
