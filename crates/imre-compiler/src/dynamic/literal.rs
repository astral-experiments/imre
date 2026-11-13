//! Source-local exec occurrences selected by registered import resolution.
use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use ruff_db::{files::File, parsed::ParsedModuleRef};
use ruff_python_ast::visitor::Visitor;
use ruff_text_size::{TextRange, TextSize};

use super::Imports;
use super::emit::{ModuleOrigin, PreparedBody};
use crate::analysis::Session;
use crate::frontend::module_source::ParsedModule;

/// The existing IR call range identifies the operation. Python reports its source
/// argument's position when a selected child cannot be compiled.
pub(crate) struct LiteralSite {
    pub(crate) call: TextRange,
    pub(crate) operand: TextSize,
}

pub(super) struct PreparedLiteral {
    pub(super) body: PreparedBody<'static>,
    /// Present only when the decoded operand differs from the body's normalized source.
    pub(super) original_text: Option<Box<str>>,
}

/// One source slot's checked operands. Different modules named "<string>" have
/// independent tables, and a live rebinding still receives the original string.
#[derive(Default)]
pub(crate) struct LiteralSites {
    sites: HashMap<TextRange, Result<Box<PreparedLiteral>, (TextSize, String)>>,
}

impl LiteralSites {
    fn insert(&mut self, site: LiteralSite, body: Result<PreparedLiteral, String>) {
        let entry = body.map(Box::new).map_err(|reason| (site.operand, reason));
        self.sites.insert(site.call, entry);
    }

    pub(crate) fn contains(&self, site: TextRange) -> bool {
        self.sites.contains_key(&site)
    }

    pub(crate) fn refused(&self, site: TextRange) -> bool {
        self.sites.get(&site).is_some_and(Result::is_err)
    }

    pub(crate) fn has_refusals(&self) -> bool {
        self.sites.values().any(Result::is_err)
    }

    /// Absence means the ordinary callable owns this string. A selected builtin
    /// with a refused child must retain its diagnostic through placement.
    pub(super) fn body(
        &self,
        site: TextRange,
    ) -> Option<Result<&PreparedLiteral, (TextSize, &str)>> {
        Some(
            self.sites
                .get(&site)?
                .as_deref()
                .map_err(|(operand, reason)| (*operand, reason.as_str())),
        )
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
    inherited_flags: u32,
    imports: Vec<String>,
    candidates: Vec<TextRange>,
}

impl ParsedLiteral {
    pub(crate) fn new(
        file: File,
        text: Cow<'_, str>,
        original: &str,
        parsed: ParsedModuleRef,
        inherited_flags: u32,
    ) -> Self {
        let body = &parsed.syntax().body;
        let flags = inherited_flags | imre_ast::future_flags(body);
        let mut imports = Imports::default();
        imports.private_names.string_annotations =
            flags & imre_ast::code_flags::FUTURE_ANNOTATIONS != 0;
        imports.visit_body(body);
        Self {
            file,
            original_text: matches!(text, Cow::Owned(_)).then(|| original.into()),
            text: text.into_owned(),
            parsed,
            flags,
            inherited_flags,
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
                inherited_flags: self.inherited_flags,
                python_annotations,
            },
            children,
            Default::default(),
        )?;
        Ok(PreparedLiteral {
            body,
            original_text: self.original_text,
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
    pub(super) fn record_root(
        &mut self,
        unit: usize,
        sites: Vec<(LiteralSite, Result<ParsedLiteral, String>)>,
    ) -> Vec<String> {
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

    fn record(
        &mut self,
        parent: Parent,
        unit: usize,
        sites: Vec<(LiteralSite, Result<ParsedLiteral, String>)>,
    ) -> Vec<String> {
        let mut names = Vec::new();
        for (site, result) in sites {
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
