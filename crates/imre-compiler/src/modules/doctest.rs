//! Static documentation and ahead-of-time compilation of interactive examples.

use super::*;
use ruff_python_ast::visitor::{Visitor, walk_expr, walk_stmt};
use std::borrow::Cow;
use std::fmt::Write;

const CASE_PREFIX: &str = "_imre_compiled_doctest_";
const RUNNER_MODULE: &str = "_imre_compiled_doctests";

pub(crate) use crate::dynamic::Prompt;
pub(super) type Docstring = (String, String, Option<TextSize>);

/// The runner and the DocTest metadata use the same physical declarations in source order.
pub(crate) fn documents<'a>(
    modules: &'a [SourceModule],
    target: Binding,
    session: &Session,
) -> Option<(&'a str, Cow<'a, [Docstring]>)> {
    match target {
        Binding::Module(id) => {
            let source = &modules[id];
            let docs = source.doctest_documents.get_or_init(|| {
                let mut docs = Vec::new();
                source_docstrings(&source.name, &source.parsed.syntax().body, true, &mut docs);
                docs
            });
            Some((&source.name, Cow::Borrowed(docs)))
        }
        Binding::IntrinsicModule(module) => {
            let sealed = sealed(module)?;
            let mut docs = Vec::new();
            if sealed.entries.is_empty() {
                docs.push((
                    sealed.module.into(),
                    session.shipped_module_docstring(sealed.module)?,
                    None,
                ));
            } else {
                sealed.append_to(&mut docs);
            }
            Some((sealed.module, Cow::Owned(docs)))
        }
        _ => None,
    }
}

impl SourceModule {
    /// Only loader-created example modules may use interactive variable binding.
    pub(crate) fn is_doctest_case(&self) -> bool {
        self.bundled && self.name.starts_with(CASE_PREFIX)
    }
}

impl SourceGraph {
    /// Resolve the generated dispatcher from the same module used by native calls.
    pub(crate) fn doctest_runner(&self) -> Option<DefinitionId> {
        let module = self
            .modules
            .iter()
            .find(|module| module.is_bundled(RUNNER_MODULE))?;
        match module.bindings.get("_execute") {
            Some(Binding::Definition(id)) => Some(*id),
            _ => None,
        }
    }
}

pub(crate) fn source_docstrings(
    name: &str,
    body: &[ast::Stmt],
    classes: bool,
    out: &mut Vec<(String, String, Option<TextSize>)>,
) {
    if let Some(ast::Stmt::Expr(stmt)) = body.first()
        && let ast::Expr::StringLiteral(doc) = stmt.value.as_ref()
    {
        out.push((
            name.to_owned(),
            doc.value.to_str().to_owned(),
            Some(doc.start()),
        ));
    }
    if classes {
        for stmt in body {
            match stmt {
                ast::Stmt::FunctionDef(function) => {
                    source_docstrings(
                        &format!("{name}.{}", function.name),
                        &function.body,
                        false,
                        out,
                    );
                }
                ast::Stmt::ClassDef(class) => {
                    source_docstrings(&format!("{name}.{}", class.name), &class.body, true, out);
                }
                _ => {}
            }
        }
    }
}

/// Original documentation for a compiler-provided module.
pub(crate) struct SealedDocs {
    pub(crate) module: &'static str,
    entries: &'static [(&'static str, &'static str)],
}

impl SealedDocs {
    /// The original Python docstring of a definition within this module.
    pub(crate) fn object_docstring(&self, qualname: &str) -> Option<&'static str> {
        self.entries.iter().find_map(|&(name, text)| {
            let relative = name.strip_prefix(self.module)?.strip_prefix('.')?;
            (relative == qualname).then_some(text)
        })
    }

    pub(crate) fn append_to(&self, docs: &mut Vec<(String, String, Option<TextSize>)>) {
        docs.extend(
            self.entries
                .iter()
                .map(|&(name, text)| (name.into(), text.into(), None)),
        );
    }
}

pub(crate) fn sealed(name: IntrinsicModule) -> Option<SealedDocs> {
    macro_rules! docs {
        ($module:literal; $($name:literal),* $(,)?) => {
            SealedDocs {
                module: $module,
                entries: &[$(($name, include_str!(concat!("../../stdlib/docstrings/", $name, ".txt")))),*],
            }
        };
    }
    Some(match name {
        IntrinsicModule::Itertools => docs!("itertools"; "itertools.batched"),
        IntrinsicModule::Json => docs!("json";),
        IntrinsicModule::JsonEncoder => docs!("json.encoder"; "json.encoder.JSONEncoder.encode"),
        IntrinsicModule::JsonDecoder => docs!("json.decoder"; "json.decoder"),
        IntrinsicModule::Collections => docs!("collections";
            "collections.Counter",
            "collections.Counter.__add__",
            "collections.Counter.__and__",
            "collections.Counter.__iadd__",
            "collections.Counter.__iand__",
            "collections.Counter.__init__",
            "collections.Counter.__ior__",
            "collections.Counter.__isub__",
            "collections.Counter.__or__",
            "collections.Counter.__sub__",
            "collections.Counter.elements",
            "collections.Counter.most_common",
            "collections.Counter.subtract",
            "collections.Counter.update",
            "collections.namedtuple",
        ),
        _ => return None,
    })
}

struct Uses<'a> {
    session: &'a Session,
    modules: &'a [SourceModule],
    origin: ModuleId,
    suite: DefinitionId,
    targets: Vec<Binding>,
    /// Track only the variables actually passed to DocTestSuite. An unrelated
    /// tuple of modules must never add doctests to a different suite.
    variables: Vec<(ModuleId, String)>,
    assignments: HashMap<(ModuleId, String), Vec<Binding>>,
    executes: bool,
}

impl Uses<'_> {
    fn is_suite(&self, callee: &ast::Expr) -> bool {
        let suite = Some(Binding::Definition(self.suite));
        if let Some(binding) = resolve_binding(self.modules, self.origin, callee) {
            return Some(binding) == suite;
        }
        // A function-local import is not a module slot. Resolve its receiver at this use.
        let owner = &self.modules[self.suite.module];
        callee.as_attribute_expr().is_some_and(|attr| {
            owner.export(attr.attr.as_str()) == suite
                && self.session.module_name_at(self.origin, &attr.value)
                    == Some(owner.name.as_str())
        })
    }

    fn remember(&mut self, target: &ast::Expr, values: &[ast::Expr]) {
        let Some(name) = target.as_name_expr() else {
            return;
        };
        let bindings = self
            .assignments
            .entry((self.origin, name.id.to_string()))
            .or_default();
        bindings.extend(
            values
                .iter()
                .filter_map(|value| resolve_binding(self.modules, self.origin, value)),
        );
    }
}

impl<'a> Visitor<'a> for Uses<'_> {
    fn visit_stmt(&mut self, stmt: &'a ast::Stmt) {
        match stmt {
            ast::Stmt::Assign(s) => {
                for target in &s.targets {
                    self.remember(target, std::slice::from_ref(s.value.as_ref()));
                }
            }
            ast::Stmt::For(s) => match s.iter.as_ref() {
                ast::Expr::Tuple(tuple) => self.remember(&s.target, &tuple.elts),
                ast::Expr::List(list) => self.remember(&s.target, &list.elts),
                _ => {}
            },
            _ => {}
        }
        walk_stmt(self, stmt);
    }

    fn visit_expr(&mut self, expr: &'a ast::Expr) {
        if let ast::Expr::Attribute(a) = expr
            && matches!(a.attr.as_str(), "runTest" | "debug" | "run")
        {
            self.executes = true;
        }
        if let ast::Expr::Call(call) = expr
            && self.is_suite(&call.func)
        {
            let arg = call.arguments.args.first().or_else(|| {
                call.arguments
                    .keywords
                    .iter()
                    .find(|kw| kw.arg.as_ref().is_some_and(|arg| arg == "module"))
                    .map(|kw| &kw.value)
            });
            if let Some(arg) = arg.filter(|arg| !arg.is_none_literal_expr()) {
                if let Some(binding) = resolve_binding(self.modules, self.origin, arg) {
                    self.targets.push(binding);
                } else if let Some(name) = arg.as_name_expr() {
                    self.variables.push((self.origin, name.id.to_string()));
                }
            } else {
                self.targets.push(Binding::Module(self.origin));
            }
        }
        walk_expr(self, expr);
    }
}

/// Collect only the Python statements to compile. The native DocTestParser
/// supplies expected output, flags and line numbers at run time; each compiled
/// entry checks its exact source against that parser before executing.
fn prompts(doc: &str) -> Vec<String> {
    let expanded = ruff_python_trivia::expand_tabs(doc);
    let mut lines = expanded
        .split('\n')
        .map(|line| line.trim_start_matches(' '))
        .peekable();
    let mut result = Vec::new();
    while let Some(line) = lines.next() {
        let Some(first) = line.strip_prefix(">>>") else {
            continue;
        };
        let mut source = String::from(first.strip_prefix(' ').unwrap_or(first));
        while let Some(next) = lines.peek().and_then(|line| line.strip_prefix("...")) {
            source.push('\n');
            source.push_str(next.strip_prefix(' ').unwrap_or(next));
            lines.next();
        }
        if !source.ends_with('\n') {
            source.push('\n');
        }
        let trimmed = source.trim();
        if !trimmed.is_empty() && !(trimmed.starts_with('#') && !trimmed.contains('\n')) {
            result.push(source);
        }
    }
    result
}

/// CPython compiles a doctest in interactive mode: expression statements
/// display repr (except None), including expressions in the body of a loop.
/// Function and class bodies still use ordinary Python statement semantics.
struct Interactive<'s> {
    text: &'s str,
    edits: Vec<(usize, usize, String)>,
    imports: Vec<String>,
    names: HashSet<String>,
}

impl<'a> Visitor<'a> for Interactive<'_> {
    fn visit_expr(&mut self, expr: &'a ast::Expr) {
        if let ast::Expr::Name(name) = expr {
            self.names.insert(name.id.to_string());
        }
        walk_expr(self, expr);
    }
    fn visit_stmt(&mut self, stmt: &'a ast::Stmt) {
        match stmt {
            ast::Stmt::Expr(stmt) => {
                let start = usize::from(stmt.value.start());
                let end = usize::from(stmt.value.end());
                self.edits.push((
                    start,
                    end,
                    format!("_imre_display({})", &self.text[start..end]),
                ));
                self.visit_expr(&stmt.value);
            }
            ast::Stmt::Import(_) | ast::Stmt::ImportFrom(_) => {
                let start = usize::from(stmt.start());
                let end = usize::from(stmt.end());
                self.imports.push(self.text[start..end].to_owned());
                self.edits.push((start, end, "pass".to_owned()));
            }
            ast::Stmt::FunctionDef(_) | ast::Stmt::ClassDef(_) => {}
            _ => walk_stmt(self, stmt),
        }
    }
}

/// Register sources mentioned only in prompts without executing their initializers.
#[derive(Default)]
struct Dependencies {
    targets: std::collections::BTreeSet<String>,
    needs_runtime: bool,
}

impl Dependencies {
    fn extend(&mut self, other: Self) {
        self.targets.extend(other.targets);
        self.needs_runtime |= other.needs_runtime;
    }
}

struct PromptImports<'a, 'fs> {
    loader: &'a Loader<'fs>,
    origin: Option<ModuleId>,
    targets: Vec<(TextRange, String)>,
    needs_runtime: bool,
}

impl PromptImports<'_, '_> {
    fn module(&mut self, site: TextRange, name: &str) {
        self.needs_runtime |= IntrinsicModule::named(name).is_none();
        self.targets.push((site, name.to_owned()));
    }
}

impl<'a> ruff_python_ast::statement_visitor::StatementVisitor<'a> for PromptImports<'_, '_> {
    fn visit_stmt(&mut self, stmt: &'a ast::Stmt) {
        match stmt {
            ast::Stmt::Import(import) => {
                for alias in &import.names {
                    self.module(stmt.range(), alias.name.as_str());
                }
            }
            ast::Stmt::ImportFrom(import) => {
                let module = if import.level == 0 {
                    import.module.as_ref().map(ToString::to_string)
                } else {
                    self.needs_runtime = true;
                    self.origin
                        .and_then(|id| self.loader.import_name(id, import).ok())
                };
                if let Some(module) = module {
                    self.module(stmt.range(), &module);
                    // A from-list may bind an existing attribute or load a child module.
                    self.targets.extend(
                        import
                            .names
                            .iter()
                            .filter(|a| a.name.as_str() != "*")
                            .map(|a| (stmt.range(), format!("{module}.{}", a.name))),
                    );
                }
            }
            _ => ruff_python_ast::statement_visitor::walk_stmt(self, stmt),
        }
    }
}

/// A sealed import has no initializer. It can be resolved at link time once
/// the prompt is known to execute unconditionally and its binding is fresh.
fn prompt_import(stmt: &ast::Stmt) -> Result<Vec<String>, String> {
    Ok(match stmt {
        ast::Stmt::Import(i) if !i.is_lazy => {
            let mut names = Vec::new();
            for alias in &i.names {
                let path = alias.name.as_str();
                if IntrinsicModule::named(path).is_none() {
                    return Err(format!(
                        "import {path} requires a runtime module initializer"
                    ));
                }
                names.push(alias.asname.as_ref().map_or_else(
                    || path.split('.').next().unwrap().to_owned(),
                    |a| a.to_string(),
                ));
            }
            names
        }
        ast::Stmt::ImportFrom(i) if i.level == 0 && !i.is_lazy => {
            let path = i.module.as_ref().map_or("", |m| m.as_str());
            let module = IntrinsicModule::named(path)
                .ok_or_else(|| format!("import {path} requires a runtime module initializer"))?;
            let mut names = Vec::new();
            for alias in &i.names {
                if module.symbol(alias.name.as_str()).is_none() {
                    return Err(format!(
                        "unsupported sealed standard-library export {path}.{}",
                        alias.name
                    ));
                }
                names.push(alias.asname.as_ref().unwrap_or(&alias.name).to_string());
            }
            names
        }
        _ => {
            return Err(
                "conditional, lazy, or relative doctest import requires a runtime namespace".into(),
            );
        }
    })
}

/// Recognize option comments using the bundled doctest parser's quote rule.
/// A candidate followed by a quote on the same line is ignored.
fn has_option_directive(source: &str) -> bool {
    source.lines().any(|line| {
        let suffix = line.rsplit_once(['\'', '"']).map_or(line, |(_, tail)| tail);
        suffix
            .split('#')
            .skip(1)
            .any(|comment| comment.trim_start().starts_with("doctest:"))
    })
}

/// Write one native function for a docstring: its examples share locals, and
/// the next invocation starts with a fresh frame. No expected output is used
/// to choose how to compile an expression.
fn case_program(
    name: &str,
    sources: &mut [String],
    target: Binding,
    modules: &[SourceModule],
) -> Result<String, String> {
    let mut header = String::from(
        "from doctest import DocTest, OutputChecker as _imre_OutputChecker, SKIP as _imre_SKIP, REPORT_ONLY_FIRST_FAILURE as _imre_FIRST, FAIL_FAST as _imre_FAIL_FAST, _checked_example, _check_output as _imre_check, _optionflags, _exception_line\nfrom _imre_doctest import display as _imre_display, capture_start as _imre_begin, capture_take as _imre_take\n",
    );
    let mut body = String::from(
        "\ndef _run_case(_imre_test: DocTest, _imre_debug: bool, _imre_optionflags: int, _imre_checker: _imre_OutputChecker) -> int:\n    _imre_failures: list[str] = []\n    _imre_attempted = 0\n    _imre_flags = _imre_optionflags\n",
    );
    let mut names = HashSet::new();
    let mut imports = Vec::new();
    let mut import_bindings = HashMap::new();
    for (i, source) in sources.iter_mut().enumerate() {
        let parsed = ruff_python_parser::parse_module(source)
            .map_err(|err| format!("doctest {name}, example {}: {err}", i + 1))?;
        let has_directive = has_option_directive(source);
        let mut interp = Interactive {
            text: source,
            edits: Vec::new(),
            imports: Vec::new(),
            names: HashSet::new(),
        };
        for stmt in &parsed.syntax().body {
            interp.visit_stmt(stmt);
        }
        if !interp.imports.is_empty() {
            let issue = |msg: String| format!("doctest {name}, example {}: {msg}", i + 1);
            let [stmt] = parsed.syntax().body.as_slice() else {
                return Err(issue(
                    "compound doctest import requires a runtime namespace".into(),
                ));
            };
            if has_directive {
                return Err(issue(
                    "option-dependent doctest import requires a runtime namespace".into(),
                ));
            }
            for bound in prompt_import(stmt).map_err(issue)? {
                let spelling = &interp.imports[0];
                if let Some(previous) = import_bindings.get(&bound) {
                    if previous != spelling {
                        return Err(issue(format!(
                            "rebinding import {bound} requires a runtime namespace"
                        )));
                    }
                } else {
                    if names.contains(&bound) {
                        return Err(issue(format!(
                            "import {bound} after prior use requires a runtime namespace"
                        )));
                    }
                    import_bindings.insert(bound, spelling.clone());
                }
            }
        }
        if interp.names.iter().any(|n| {
            n.starts_with("_imre_")
                || matches!(
                    n.as_str(),
                    "_checked_example" | "_exception_line" | "_optionflags"
                )
        }) {
            return Err(format!(
                "doctest {name}, example {} uses a reserved compiler name",
                i + 1
            ));
        }
        names.extend(interp.names);
        imports.extend(interp.imports);
        writeln!(
            body,
            "    _imre_ex = _checked_example(_imre_test, {i}, {})",
            serde_json::to_string(source).unwrap()
        )
        .unwrap();
        for (start, end, replacement) in interp.edits.into_iter().rev() {
            source.replace_range(start..end, &replacement);
        }
        body.push_str("    _imre_quiet = (_imre_flags & _imre_FIRST) != 0 if _imre_failures else False\n    _imre_flags = _optionflags(_imre_ex, _imre_optionflags)\n    if not _imre_flags & _imre_SKIP:\n        _imre_attempted += 1\n        _imre_error = \"\"\n        _imre_begin()\n        try:\n");
        for line in source.lines() {
            writeln!(body, "            {line}").unwrap();
        }
        body.push_str("        except Exception as _imre_exc:\n            _imre_error = _exception_line(_imre_exc)\n        finally:\n            _imre_got = _imre_take()\n        _imre_check(_imre_test, _imre_ex, _imre_got, _imre_error, _imre_failures, _imre_debug, _imre_checker, _imre_flags, _imre_quiet)\n        if _imre_failures and _imre_flags & _imre_FAIL_FAST:\n            raise AssertionError(\"\\n\".join(_imre_failures))\n");
    }
    // DocTestFinder starts a case with a shallow copy of its module namespace.
    // Resolve used globals there, including the public owner of a documented
    // definition even when that export has not yet been implemented natively.
    let (module, mut referenced): (&str, Vec<String>) = match target {
        Binding::Module(id) => {
            let module = &modules[id];
            (
                &module.name,
                names
                    .into_iter()
                    .filter(|n| module.bindings.contains_key(n))
                    .collect(),
            )
        }
        Binding::IntrinsicModule(m) => {
            let module = sealed(m).unwrap().module;
            let owner = name
                .strip_prefix(module)
                .and_then(|n| n.strip_prefix('.'))
                .and_then(|n| n.split('.').next());
            (
                module,
                names
                    .into_iter()
                    .filter(|n| m.symbol(n).is_some() || owner == Some(n.as_str()))
                    .collect(),
            )
        }
        _ => unreachable!(),
    };
    referenced.retain(|n| !import_bindings.contains_key(n));
    referenced.sort_unstable();
    if !referenced.is_empty() {
        writeln!(header, "from {module} import {}", referenced.join(", ")).unwrap();
    }
    let mut declared = HashSet::new();
    for import in imports {
        if declared.insert(import.clone()) {
            writeln!(header, "{import}").unwrap();
        }
    }
    body.push_str(
        "    if _imre_failures:\n        raise AssertionError(\"\\n\".join(_imre_failures))\n    return _imre_attempted\n",
    );
    header.push_str(&body);
    Ok(header)
}

/// Keep direct string entries as candidates. Lift entries out of an unpacked display
/// only while the subsequent writes to the dictionary are known.
fn dictionary_strings(
    dict: &ast::ExprDict,
    direct: bool,
    visit: &mut impl FnMut(&str, &ast::Expr),
) -> bool {
    let mut known = true;
    for item in dict.items.iter().rev() {
        match (&item.key, &item.value) {
            (Some(ast::Expr::StringLiteral(key)), value) if direct || known => {
                visit(key.value.to_str(), value);
            }
            (None, ast::Expr::Dict(nested)) if known => {
                known = dictionary_strings(nested, false, visit);
            }
            _ => known = false,
        }
    }
    known
}

impl Loader<'_> {
    fn add_doctest_module(&mut self, name: String, source: &str) -> Result<(), String> {
        let path = self.root.join(format!("{name}.py"));
        if self.by_name.contains_key(&name) || self.fs.exists(&path) {
            return Err(format!("private doctest module {name} is shadowed"));
        }
        let id = self.add(path.clone(), path, name, false, source)?;
        self.modules[id].bundled = true;
        self.scan(id)
    }

    pub(super) fn doctest_program(
        &mut self,
        bundled: &mut HashMap<String, Vec<Prompt>>,
    ) -> Result<Vec<String>, String> {
        use ruff_python_ast::statement_visitor::StatementVisitor;

        let Some(suite) = self
            .by_name
            .get("doctest")
            .and_then(|id| self.modules[*id].bindings.get("DocTestSuite"))
            .copied()
        else {
            return Ok(Vec::new());
        };
        let Binding::Definition(suite) = suite else {
            unreachable!()
        };
        let mut uses = Uses {
            session: &self.session,
            modules: &self.modules,
            origin: 0,
            suite,
            targets: Vec::new(),
            variables: Vec::new(),
            assignments: HashMap::new(),
            executes: false,
        };
        for (id, source) in self.modules.iter().enumerate().filter(|(_, m)| !m.bundled) {
            uses.origin = id;
            for stmt in &source.parsed.syntax().body {
                uses.visit_stmt(stmt);
            }
        }
        let optimization = self
            .options
            .and_then(crate::CompileOptions::python_startup)
            .map_or(0, |(flags, _)| flags.optimize);
        let mut cases = std::collections::BTreeMap::new();
        let mut dependencies = Dependencies::default();
        for variable in uses.variables {
            if let Some(bindings) = uses.assignments.get(&variable) {
                uses.targets.extend(bindings);
            }
        }
        // The last suite for a qualified case name supplies its fixed runner.
        // Keep that order while preparing each resolved module only once.
        let mut sources = HashSet::new();
        let mut intrinsics = HashSet::new();
        uses.targets.reverse();
        uses.targets.retain(|target| match target {
            Binding::Module(id) => sources.insert(*id),
            Binding::IntrinsicModule(module) => intrinsics.insert(*module),
            _ => false,
        });
        let executes = uses.executes;
        for target in uses.targets.into_iter().rev() {
            if let Binding::IntrinsicModule(module) = target
                && let Some(catalog) = sealed(module)
                && catalog.entries.is_empty()
            {
                self.session.prepare_shipped_source(catalog.module)?;
            }
            if executes {
                let Some((owner, docs)) = documents(&self.modules, target, &self.session) else {
                    continue;
                };
                let module = match target {
                    Binding::Module(id) => Some(id),
                    Binding::IntrinsicModule(_) => None,
                    _ => continue,
                };
                let mut recorded = HashSet::new();
                let mut prepared = Vec::new();
                let mut record = |name: &str, sources: &[String], emit: bool| {
                    let mut dependencies = Dependencies::default();
                    let mut imports = PromptImports {
                        loader: self,
                        origin: module,
                        targets: Vec::new(),
                        needs_runtime: false,
                    };
                    for (index, source) in sources.iter().enumerate() {
                        let inspect = source.contains("import");
                        if !emit && !inspect {
                            continue;
                        }
                        // prompts() has already expanded tabs, as DocTestParser does.
                        let Ok(parsed) = ruff_python_parser::parse_module(source) else {
                            continue;
                        };
                        if inspect {
                            imports.visit_body(&parsed.syntax().body);
                        }
                        if emit {
                            let path = format!("<doctest {name}[{index}]>").into();
                            let module = crate::frontend::module_source::ParsedModule {
                                text: Cow::Owned(source.clone()),
                                parsed: Cow::Owned(parsed),
                            };
                            if let Some((prompt, unreachable)) =
                                Prompt::from_parsed(path, module, optimization)
                            {
                                imports
                                    .targets
                                    .retain(|(site, _)| !unreachable.contains(site));
                                prepared.push(prompt);
                            }
                        }
                        dependencies
                            .targets
                            .extend(imports.targets.drain(..).map(|(_, name)| name));
                    }
                    // Runner selection also accounts for imports in the written fixed case.
                    dependencies.needs_runtime = imports.needs_runtime;
                    dependencies
                };
                for (name, doc, _) in docs.iter() {
                    let sources = prompts(doc);
                    if !sources.is_empty() {
                        let emit = optimization < 2 && recorded.insert(name.clone());
                        let imports = record(name, &sources, emit);
                        cases.insert(name.clone(), (sources, target, imports));
                    }
                }
                // These strings survive -OO. Only the live discovery can select them.
                if let Some(id) = module
                    && let Some(Binding::Value(binding)) = self.modules[id].bindings.get("__test__")
                    && binding.module == id
                    && let Some(ast::Expr::Dict(dict)) =
                        module_value(&self.modules[id].parsed.syntax().body[binding.statement])
                {
                    dictionary_strings(dict, true, &mut |key, value| {
                        let name = format!("{owner}.__test__.{key}");
                        if recorded.insert(name.clone())
                            && let ast::Expr::StringLiteral(doc) = value
                        {
                            let sources = prompts(doc.value.to_str());
                            dependencies.extend(record(&name, &sources, true));
                        }
                    });
                }
                if let Some(id) = module {
                    self.modules[id].doctest_prompts.extend(prepared);
                } else if !prepared.is_empty() {
                    bundled
                        .entry(owner.to_owned())
                        .or_default()
                        .extend(prepared);
                }
            }
        }
        for (_, _, imports) in cases.values_mut() {
            dependencies.extend(std::mem::take(imports));
        }
        if dependencies.needs_runtime {
            // Run the original prompt with its globals, output capture and option flags.
            let targets = dependencies.targets.into_iter().collect();
            self.needs_object_imports = true;
            return Ok(targets);
        }
        // Module startup executes the enrolled prompts in test.globs. Checked files
        // still need the fixed calls to diagnose typed arguments in a prompt.
        if self
            .options
            .is_some_and(crate::CompileOptions::registered_initializer)
        {
            return Ok(Vec::new());
        }
        let mut source = String::from(
            "from doctest import DocTest, OutputChecker, SKIP, _optionflags\nfrom unittest import SkipTest\n",
        );
        let mut body = String::from(
            "\ndef _execute(test: DocTest, debug: bool, optionflags: int = 0, checker: OutputChecker | None = None) -> None:\n    if checker is None:\n        checker = OutputChecker()\n    if not test.examples:\n        attempted = 0\n",
        );
        for (i, (name, (mut prompts, context, _))) in cases.into_iter().enumerate() {
            let module = format!("{CASE_PREFIX}{i}");
            let program = case_program(&name, &mut prompts, context, &self.modules)?;
            self.add_doctest_module(module.clone(), &program)
                .map_err(|err| format!("doctest {name}: {err}"))?;
            writeln!(source, "from {module} import _run_case as _case_{i}").unwrap();
            writeln!(
                body,
                "    elif test.name == {}:\n        attempted = _case_{i}(test, debug, optionflags, checker)",
                serde_json::to_string(&name).unwrap()
            )
            .unwrap();
        }
        body.push_str("    else:\n        for example in test.examples:\n            if not _optionflags(example, optionflags) & SKIP:\n                raise ValueError(\"no native doctest compiled for \" + test.name)\n        attempted = 0\n    if not attempted and not debug:\n        raise SkipTest(\"all examples were skipped\")\n");
        source.push_str(&body);
        self.add_doctest_module(RUNNER_MODULE.into(), &source)?;
        Ok(Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_winning_dictionary_prompts_enroll_imports() {
        let dir = tempfile::tempdir().unwrap();
        let entry = dir.path().join("app.py");
        std::fs::write(
            &entry,
            "import doctest, subject\ndoctest.DocTestSuite(subject).debug()\n",
        )
        .unwrap();
        std::fs::write(dir.path().join("discarded.py"), "broken = )\n").unwrap();
        std::fs::write(dir.path().join("kept.py"), "value = 42\n").unwrap();
        std::fs::write(
            dir.path().join("subject.py"),
            "__test__ = {'x': '>>> import discarded', 'y': '>>> import discarded', **{'x': '>>> import kept', 'y': 17}}\n",
        )
        .unwrap();
        let graph = SourceGraph::load(&entry, Some(dir.path()), &[]).unwrap();
        let subject = graph.modules.iter().find(|m| m.name == "subject").unwrap();
        assert_eq!(subject.doctest_prompts.len(), 1);
        assert_eq!(graph.doctest_imports, ["kept"]);
    }

    #[test]
    fn explicit_none_prepares_the_calling_modules_prompts() {
        let dir = tempfile::tempdir().unwrap();
        let entry = dir.path().join("app.py");
        std::fs::write(&entry, "import owner\nowner.run()\n").unwrap();
        for module in ["None", "module=None"] {
            let source = format!(
                "'''>>> 1 + 1\n2\n'''\nimport doctest\ndef run():\n    doctest.DocTestSuite({module}, checker=None, optionflags=0).debug()\n"
            );
            std::fs::write(dir.path().join("owner.py"), source).unwrap();
            let graph = SourceGraph::load(&entry, Some(dir.path()), &[]).unwrap();
            let owner = graph.modules.iter().find(|m| m.name == "owner").unwrap();
            assert_eq!(owner.doctest_prompts.len(), 1);
        }
    }

    #[test]
    fn a_prepared_prompt_enrolls_imports_on_its_returning_path() {
        let dir = tempfile::tempdir().unwrap();
        let entry = dir.path().join("app.py");
        std::fs::write(
            &entry,
            "import doctest, subject\ndoctest.DocTestSuite(subject).debug()\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("subject.py"),
            r#"'''>>> def f(stop):
...     if stop: return 42
...     import live
...     return live.VALUE
...     from discarded import VALUE
>>> f(False)
37
'''
"#,
        )
        .unwrap();
        std::fs::write(dir.path().join("live.py"), "VALUE = 37\n").unwrap();
        let graph = SourceGraph::load(&entry, Some(dir.path()), &[]).unwrap();
        assert_eq!(graph.doctest_imports, ["live"]);
    }

    #[test]
    fn physical_doctest_declarations_survive_syntax_withdrawal() {
        let source = "'module doc'\nclass C:\n    'class doc'\n    def method(self):\n        'method doc'\n";
        let mut graph = SourceGraph::single(source).unwrap();
        let entry = graph.entry;
        let target = Binding::Module(entry);
        assert!(graph.modules[entry].doctest_documents.get().is_none());
        let storage = documents(&graph.modules, target, &graph.session)
            .unwrap()
            .1
            .as_ptr();

        let erased = SourceGraph::single("pass\n").unwrap();
        graph.modules[entry].parsed = erased.modules[erased.entry].parsed.clone();
        let (owner, docs) = documents(&graph.modules, target, &graph.session).unwrap();
        assert_eq!(owner, "__main__");
        assert_eq!(docs.as_ptr(), storage);
        let actual: Vec<_> = docs
            .iter()
            .map(|(name, doc, offset)| (name.as_str(), doc.as_str(), usize::from(offset.unwrap())))
            .collect();
        assert_eq!(
            actual,
            [
                ("__main__", "module doc", 0),
                (
                    "__main__.C",
                    "class doc",
                    source.find("'class doc'").unwrap()
                ),
                (
                    "__main__.C.method",
                    "method doc",
                    source.find("'method doc'").unwrap()
                ),
            ]
        );
    }
}
