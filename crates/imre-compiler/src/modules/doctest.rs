//! Static documentation and ahead-of-time compilation of interactive examples.

use super::*;
use ruff_python_ast::visitor::{Visitor, walk_expr, walk_stmt};
use std::fmt::Write;

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

/// Sealed namespaces carry the original documentation of their example-bearing
/// definitions. The audit compares these strings with CPython's actual finder.
pub(crate) fn sealed(name: IntrinsicModule) -> Option<(&'static str, &'static str)> {
    Some(match name {
        IntrinsicModule::Itertools => (
            "itertools.batched",
            include_str!("../../stdlib/docstrings/itertools.batched.txt"),
        ),
        IntrinsicModule::Json => ("json", include_str!("../../stdlib/docstrings/json.txt")),
        IntrinsicModule::JsonEncoder => (
            "json.encoder.JSONEncoder.encode",
            include_str!("../../stdlib/docstrings/json.encoder.JSONEncoder.encode.txt"),
        ),
        IntrinsicModule::JsonDecoder => (
            "json.decoder",
            include_str!("../../stdlib/docstrings/json.decoder.txt"),
        ),
        _ => return None,
    })
}

struct Uses<'a> {
    modules: &'a [SourceModule],
    origin: ModuleId,
    suite: DefinitionId,
    targets: Vec<Binding>,
    tuple_targets: Vec<Binding>,
    variable: bool,
    executes: bool,
}

impl<'a> Visitor<'a> for Uses<'_> {
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
                } else {
                    self.variable = true;
                }
            } else {
                self.targets.push(Binding::Module(self.origin));
            }
        }
        if let ast::Expr::Tuple(tuple) = expr {
            self.tuple_targets.extend(
                tuple
                    .elts
                    .iter()
                    .filter_map(|item| resolve_binding(self.modules, self.origin, item)),
            );
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
        source.push('\n');
        while i < lines.len() {
            let Some(next) = lines[i].trim_start_matches(' ').strip_prefix("...") else {
                break;
            };
            source.push_str(next.strip_prefix(' ').unwrap_or(next));
            source.push('\n');
            i += 1;
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

/// Write one native function for a docstring: its examples share locals, and
/// the next invocation starts with a fresh frame. No expected output is used
/// to choose how to compile an expression.
fn case_program(
    name: &str,
    sources: &[String],
    context: Option<&SourceModule>,
) -> Result<String, String> {
    let mut header = String::from(
        "from doctest import DocTest, _checked_example, _check_example, _optionflags, _exception_line\nfrom _imre_doctest import display as _imre_display, capture_start as _imre_begin, capture_take as _imre_take\n",
    );
    let mut body = String::from(
        "\ndef _run_case(_imre_test: DocTest, _imre_debug: bool) -> None:\n    _imre_failures: list[str] = []\n",
    );
    let mut names = HashSet::new();
    let mut imports = Vec::new();
    for (i, source) in sources.iter().enumerate() {
        let parsed = ruff_python_parser::parse_module(source)
            .map_err(|err| format!("doctest {name}, example {}: {err}", i + 1))?;
        let mut interp = Interactive {
            text: source,
            edits: Vec::new(),
            imports: Vec::new(),
            names: HashSet::new(),
        };
        for stmt in &parsed.syntax().body {
            interp.visit_stmt(stmt);
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
        let pad = if source.contains("doctest:") {
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
    if let Some(context) = context {
        let mut referenced = names
            .into_iter()
            .filter(|n| context.bindings.contains_key(n))
            .collect::<Vec<_>>();
        referenced.sort_unstable();
        if !referenced.is_empty() {
            writeln!(
                header,
                "from {} import {}",
                context.name,
                referenced.join(", ")
            )
            .unwrap();
        }
    } else if name == "itertools.batched" {
        header.push_str("from itertools import batched\n");
    }
    for import in imports {
        writeln!(header, "{import}").unwrap();
    }
    body.push_str(
        "    if _imre_failures:\n        raise AssertionError(\"\\n\".join(_imre_failures))\n",
    );
    header.push_str(&body);
    Ok(header)
}

impl Loader<'_> {
    fn add_doctest_module(&mut self, name: String, source: &str) -> Result<ModuleId, String> {
        let path = self.root.join(format!("{name}.py"));
        if self.by_name.contains_key(&name) || self.fs.exists(&path) {
            return Err(format!("private doctest module {name} is shadowed"));
        }
        let id = self.add(path.clone(), path, name, false, source)?;
        self.modules[id].bundled = true;
        self.scan(id)?;
        Ok(id)
    }

    pub(super) fn doctest_program(&mut self) -> Result<Option<DefinitionId>, String> {
        let Some(suite) = self
            .by_name
            .get("doctest")
            .and_then(|id| self.modules[*id].bindings.get("DocTestSuite"))
            .copied()
        else {
            return Ok(None);
        };
        let Binding::Definition(suite) = suite else {
            unreachable!()
        };
        let mut uses = Uses {
            modules: &self.modules,
            origin: 0,
            suite,
            targets: Vec::new(),
            tuple_targets: Vec::new(),
            variable: false,
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
            if uses.variable {
                uses.targets.append(&mut uses.tuple_targets);
            }
            for target in uses.targets {
                let (mut docs, context) = (
                    Vec::new(),
                    match target {
                        Binding::Module(id) => Some(id),
                        _ => None,
                    },
                );
                match target {
                    Binding::Module(id) => {
                        let m = &self.modules[id];
                        source_docstrings(&m.name, &m.parsed.syntax().body, true, &mut docs);
                    }
                    Binding::IntrinsicModule(module) if let Some((n, s)) = sealed(module) => {
                        docs.push((n.into(), s.into()))
                    }
                    _ => continue,
                }
                for (name, doc) in docs {
                    let sources = prompts(&doc);
                    if !sources.is_empty() {
                        cases.insert(name, (sources, context));
                    }
                }
            }
        }
        let mut source = String::from("from doctest import DocTest\n");
        let mut body = String::from("\ndef _execute(test: DocTest, debug: bool) -> None:\n");
        for (i, (name, (prompts, context))) in cases.into_iter().enumerate() {
            let module = format!("_imre_compiled_doctest_{i}");
            let program = case_program(&name, &prompts, context.map(|id| &self.modules[id]))?;
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
        let id = self.add_doctest_module("_imre_compiled_doctests".into(), &source)?;
        let Binding::Definition(def) = self.modules[id].bindings["_execute"] else {
            unreachable!()
        };
        Ok(Some(def))
    }
}
