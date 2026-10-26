//! Static documentation and ahead-of-time compilation of interactive examples.

use super::*;
use ruff_python_ast::visitor::{Visitor, walk_expr, walk_stmt};
use std::fmt::Write;

const CASE_PREFIX: &str = "_imre_compiled_doctest_";
const RUNNER_MODULE: &str = "_imre_compiled_doctests";

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
    out: &mut Vec<(String, String)>,
) {
    if let Some(ast::Stmt::Expr(stmt)) = body.first()
        && let ast::Expr::StringLiteral(doc) = stmt.value.as_ref()
    {
        out.push((name.to_owned(), doc.value.to_str().to_owned()));
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
    pub(crate) fn append_to(&self, docs: &mut Vec<(String, String)>) {
        docs.extend(
            self.entries
                .iter()
                .map(|&(name, text)| (name.into(), text.into())),
        );
    }
}

pub(crate) fn sealed(name: IntrinsicModule) -> Option<SealedDocs> {
    macro_rules! docs {
        ($module:literal; $($name:literal),+ $(,)?) => {
            SealedDocs {
                module: $module,
                entries: &[$(($name, include_str!(concat!("../../stdlib/docstrings/", $name, ".txt")))),+],
            }
        };
    }
    Some(match name {
        IntrinsicModule::Itertools => docs!("itertools"; "itertools.batched"),
        IntrinsicModule::Json => docs!("json"; "json"),
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
            && resolve_binding(self.modules, self.origin, &call.func)
                == Some(Binding::Definition(self.suite))
        {
            let arg = call.arguments.args.first().or_else(|| {
                call.arguments
                    .keywords
                    .iter()
                    .find(|kw| kw.arg.as_ref().is_some_and(|arg| arg == "module"))
                    .map(|kw| &kw.value)
            });
            if let Some(arg) = arg {
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
    let lines = doc
        .split('\n')
        .map(|line| {
            let mut expanded = String::new();
            let mut column = 0;
            for c in line.chars() {
                if c == '\t' {
                    let count = 8 - column % 8;
                    expanded.extend(std::iter::repeat_n(' ', count));
                    column += count;
                } else {
                    expanded.push(c);
                    column += 1;
                }
            }
            expanded
        })
        .collect::<Vec<_>>();
    let mut result = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i].trim_start_matches(' ');
        i += 1;
        let Some(first) = line.strip_prefix(">>>") else {
            continue;
        };
        let mut source = String::from(first.strip_prefix(' ').unwrap_or(first));
        while i < lines.len() {
            let Some(next) = lines[i].trim_start_matches(' ').strip_prefix("...") else {
                break;
            };
            source.push('\n');
            source.push_str(next.strip_prefix(' ').unwrap_or(next));
            i += 1;
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
    sources: &[String],
    target: Binding,
    modules: &[SourceModule],
) -> Result<String, String> {
    let mut header = String::from(
        "from doctest import DocTest, _checked_example, _check_example, _optionflags, _exception_line\nfrom _imre_doctest import display as _imre_display, capture_start as _imre_begin, capture_take as _imre_take\n",
    );
    let mut body = String::from(
        "\ndef _run_case(_imre_test: DocTest, _imre_debug: bool) -> None:\n    _imre_failures: list[str] = []\n",
    );
    let mut names = HashSet::new();
    let mut imports = Vec::new();
    let mut import_bindings = HashMap::new();
    for (i, source) in sources.iter().enumerate() {
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
                    "_check_example" | "_checked_example" | "_exception_line" | "_optionflags"
                )
        }) {
            return Err(format!(
                "doctest {name}, example {} uses a reserved compiler name",
                i + 1
            ));
        }
        names.extend(interp.names);
        imports.extend(interp.imports);
        let mut code = source.clone();
        for (start, end, replacement) in interp.edits.into_iter().rev() {
            code.replace_range(start..end, &replacement);
        }
        writeln!(
            body,
            "    _imre_ex = _checked_example(_imre_test, {i}, {})",
            serde_json::to_string(source).unwrap()
        )
        .unwrap();
        let pad = if has_directive {
            body.push_str("    if not _optionflags(_imre_ex) & 16:\n");
            "        "
        } else {
            "    "
        };
        writeln!(
            body,
            "{pad}_imre_error = \"\"\n{pad}_imre_begin()\n{pad}try:"
        )
        .unwrap();
        for line in code.lines() {
            writeln!(body, "{pad}    {line}").unwrap();
        }
        writeln!(body,"{pad}except Exception as _imre_exc:\n{pad}    _imre_error = _exception_line(_imre_exc)\n{pad}finally:\n{pad}    _imre_got = _imre_take()\n{pad}_check_example(_imre_test, _imre_ex, _imre_got, _imre_error, _imre_failures, _imre_debug)").unwrap();
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
        "    if _imre_failures:\n        raise AssertionError(\"\\n\".join(_imre_failures))\n",
    );
    header.push_str(&body);
    Ok(header)
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

    pub(super) fn doctest_program(&mut self) -> Result<(), String> {
        let Some(suite) = self
            .by_name
            .get("doctest")
            .and_then(|id| self.modules[*id].bindings.get("DocTestSuite"))
            .copied()
        else {
            return Ok(());
        };
        let Binding::Definition(suite) = suite else {
            unreachable!()
        };
        let mut uses = Uses {
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
        let mut cases = std::collections::BTreeMap::new();
        if uses.executes {
            for variable in uses.variables {
                if let Some(bindings) = uses.assignments.get(&variable) {
                    uses.targets.extend(bindings);
                }
            }
            for target in uses.targets {
                let mut docs = Vec::new();
                match target {
                    Binding::Module(id) => {
                        let m = &self.modules[id];
                        source_docstrings(&m.name, &m.parsed.syntax().body, true, &mut docs);
                    }
                    Binding::IntrinsicModule(module) if let Some(catalog) = sealed(module) => {
                        catalog.append_to(&mut docs);
                    }
                    _ => continue,
                }
                for (name, doc) in docs {
                    let sources = prompts(&doc);
                    if !sources.is_empty() {
                        cases.insert(name, (sources, target));
                    }
                }
            }
        }
        let mut source = String::from("from doctest import DocTest\n");
        let mut body = String::from("\ndef _execute(test: DocTest, debug: bool) -> None:\n");
        for (i, (name, (prompts, context))) in cases.into_iter().enumerate() {
            let module = format!("{CASE_PREFIX}{i}");
            let program = case_program(&name, &prompts, context, &self.modules)?;
            self.add_doctest_module(module.clone(), &program)
                .map_err(|err| format!("doctest {name}: {err}"))?;
            writeln!(source, "from {module} import _run_case as _case_{i}").unwrap();
            writeln!(
                body,
                "    if test.name == {}:\n        _case_{i}(test, debug)\n        return",
                serde_json::to_string(&name).unwrap()
            )
            .unwrap();
        }
        body.push_str("    raise ValueError(\"no native doctest compiled for \" + test.name)\n");
        source.push_str(&body);
        self.add_doctest_module(RUNNER_MODULE.into(), &source)
    }
}
