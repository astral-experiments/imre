//! Emit native functions and control flow for closed modules using boxed Python values.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use ruff_python_ast as ast;
use ruff_text_size::Ranged;

mod boolean;
mod expressions;
mod patterns;
mod statements;
mod strings;

const ABI: &str = "\
%code_site = type { ptr, ptr, i32, i32 }
declare ptr @imre_dyn_new(i32, ptr, ptr)
declare void @imre_dyn_register(ptr, ptr, ptr, ptr)
declare i32 @imre_dyn_finish(ptr)
declare i64 @imre_dyn_lit(ptr, i32, ptr, i64)
declare i64 @imre_dyn_mark_class_annotation(ptr, i64, i64, i32)
declare i64 @imre_dyn_define(ptr, ptr, i64, i64, i64, i64, i64, i64, i64, i1)
declare i1 @imre_dyn_failed(ptr)
declare i64 @imre_dyn_truth_mark(ptr)
declare void @imre_dyn_comprehension_step(ptr, i64)
declare void @imre_dyn_discard_truth(ptr, i64)
declare void @imre_dyn_named_assignment(ptr, i64, i64, i64)
declare void @imre_dyn_assignment_target(ptr, ptr, i64, i64)
declare i64 @imre_dyn_op_at(ptr, ptr, i64, i64, i64, ptr)
declare i64 @imre_dyn_scoped_call_at(ptr, i64, i64, i64, i64, ptr, i64)
declare void @imre_dyn_expression_line(ptr, i32)
declare void @imre_dyn_statement_with_exception(ptr, ptr, i64, ptr, i32, i64)
";

/// All module source has already been resolved. Import execution remains conditional.
pub(crate) fn emit(
    modules: &[(String, PathBuf, String)],
    config: &str,
    keep_docstrings: bool,
    bundled_root: &Path,
    providers: &HashSet<String>,
) -> Result<(String, bool), String> {
    let mut output = Output {
        keep_docstrings,
        ..Output::default()
    };
    let config = output.data(config.as_bytes());
    // Entry-only registration must not change fingerprints of unrelated packed providers.
    let mut main = format!(
        "declare void @imre_dyn_register_source(ptr, ptr, ptr, ptr, i64, ptr)\n\ndefine i32 @main(i32 %argc, ptr %argv) {{\nentry:\n  %ctx = call ptr @imre_dyn_new(i32 %argc, ptr %argv, ptr {config})\n"
    );
    let mut declarations = HashSet::new();
    for (unit, (name, path, source)) in modules.iter().enumerate() {
        let bundled = path.starts_with(bundled_root);
        let external = if bundled && !providers.is_empty() {
            provider(name, path, source, keep_docstrings)?
                .map(|(symbol, _)| symbol)
                .filter(|symbol| providers.contains(symbol))
        } else {
            None
        };
        let module_name = output.data(name.as_bytes());
        let file = output.data(path.to_str().ok_or("module path must be UTF-8")?.as_bytes());
        let code = if let Some(symbol) = external {
            if declarations.insert(symbol.clone()) {
                writeln!(output.functions, "declare i64 @{symbol}(ptr, i64)").unwrap();
            }
            format!("@{symbol}")
        } else {
            output.module(unit, name, path, source, &file, bundled)?
        };
        // Retain the text of virtual providers alongside the installed initializer.
        let (text, len) = if bundled {
            (output.data(source.as_bytes()), source.len())
        } else {
            ("null".to_owned(), 0)
        };
        writeln!(
            main,
            "  call void @imre_dyn_register_source(ptr %ctx, ptr {module_name}, ptr {file}, ptr {text}, i64 {len}, ptr {code})"
        )
        .unwrap();
    }
    main.push_str("  %status = call i32 @imre_dyn_finish(ptr %ctx)\n  ret i32 %status\n}\n");
    Ok((
        format!("{ABI}\n{}\n{}\n{main}", output.constants, output.functions),
        !declarations.is_empty(),
    ))
}

/// Emit one bundled initializer with a symbol determined by its source and native body.
pub(super) fn provider(
    name: &str,
    path: &Path,
    source: &str,
    keep_docstrings: bool,
) -> Result<Option<(String, String)>, String> {
    let mut output = Output {
        keep_docstrings,
        ..Output::default()
    };
    let filename = path.to_str().ok_or("module path must be UTF-8")?;
    let file = output.data(filename.as_bytes());
    let entry = output.module(0, name, path, source, &file, true)?;
    // These slots are numbered across the whole program and must stay in its codegen unit.
    if !output.string_constant_slots.is_empty() {
        return Ok(None);
    }
    let mut ir = format!("{ABI}\n{}\n{}", output.constants, output.functions);
    let mut hash = blake3::Hasher::new();
    for field in [std::env::consts::ARCH, name, filename, source, &ir] {
        hash.update(&(field.len() as u64).to_le_bytes());
        hash.update(field.as_bytes());
    }
    hash.update(&[u8::from(keep_docstrings)]);
    let symbol = format!("imre_provider_{}", hash.finalize().to_hex());
    writeln!(ir, "@{symbol} = alias i64 (ptr, i64), ptr {entry}").unwrap();
    Ok(Some((symbol, ir)))
}

impl Output {
    fn module(
        &mut self,
        unit: usize,
        name: &str,
        path: &Path,
        source: &str,
        file: &str,
        bundled: bool,
    ) -> Result<String, String> {
        let parsed = ruff_python_parser::parse_module(source)
            .map_err(|err| format!("{}: {err}", path.display()))?;
        crate::analysis::check_scope_syntax(source, parsed.syntax())
            .map_err(|error| format!("{}:{error}", path.display()))?;
        if let Some(keyword) = imre_ast::repeated_keyword(parsed.syntax().into()) {
            let name = keyword.arg.as_ref().expect("repeated named argument");
            let reason = format!("SyntaxError: keyword argument repeated: {name}");
            let error = crate::frontend::error(source, keyword, &reason);
            return Err(format!("{}:{error}", path.display()));
        }
        let replacements = strings::literal_replacements(source, parsed.tokens());
        if self.keep_docstrings && !replacements.is_empty() {
            strings::check_docs(source, &parsed.syntax().body, &replacements)
                .map_err(|error| format!("{}:{error}", path.display()))?;
        }
        let package = if path.file_name().is_some_and(|file| file == "__init__.py") {
            name
        } else {
            name.rsplit_once('.').map_or("", |(parent, _)| parent)
        };
        let docs = if self.keep_docstrings && name == "collections" && bundled {
            crate::modules::doctest::sealed(crate::modules::IntrinsicModule::Collections)
        } else {
            None
        };
        let has_templates = parsed
            .tokens()
            .iter()
            .any(|t| t.kind() == ast::token::TokenKind::TStringStart);
        let template_expressions =
            imre_ast::strings::template_expressions(source, parsed.tokens(), parsed.syntax());
        // CPython starts the module provider at the first statement, excluding its decorators.
        let module_start = parsed
            .syntax()
            .body
            .first()
            .map(|stmt| {
                let last = match stmt {
                    ast::Stmt::FunctionDef(stmt) => stmt.decorator_list.last(),
                    ast::Stmt::ClassDef(stmt) => stmt.decorator_list.last(),
                    _ => None,
                };
                last.and_then(|decorator| {
                    parsed.tokens().after(decorator.end()).iter().find(|token| {
                        use ast::token::TokenKind;
                        matches!(
                            token.kind(),
                            TokenKind::Async | TokenKind::Def | TokenKind::Class
                        )
                    })
                })
                .map_or_else(|| stmt.start(), Ranged::start)
            })
            .unwrap_or_default();
        let mut body = parsed.into_syntax().body;
        let method_imports = imre_ast::module_imports(&body);
        let private_names = imre_ast::lower(&mut body);
        if source.contains("nonlocal") {
            statements::type_parameters::check_nonlocals(source, &body)
                .map_err(|error| format!("{}:{error}", path.display()))?;
        }
        if let Some((expr, reason)) = imre_ast::type_alias_error(&body) {
            let error = crate::frontend::error(source, expr, &format!("SyntaxError: {reason}"));
            return Err(format!("{}:{error}", path.display()));
        }
        if let Some(param) = &private_names.duplicate_type_parameter {
            let reason = format!("SyntaxError: duplicate type parameter '{}'", param.id);
            let error = crate::frontend::error(source, param, &reason);
            return Err(format!("{}:{error}", path.display()));
        }
        let location = SourceMap {
            module_start,
            docs,
            unit,
            has_templates,
            template_expressions,
            replacements,
            method_imports,
            barry_as_flufl: body.iter().any(|stmt| {
                matches!(stmt,
                    ast::Stmt::ImportFrom(import) if import.level == 0
                        && import.module.as_deref() == Some("__future__")
                        && import.names.iter().any(|name| name.name.as_str() == "barry_as_FLUFL")
                )
            }),
            string_annotations: private_names.string_annotations,
            private_names,
            package,
            text: source,
            file,
            newlines: source
                .match_indices('\n')
                .map(|(offset, _)| offset)
                .collect(),
        };
        Ok(self.code(&location, String::new(), Body::Namespace(&body), false))
    }
}

enum DefinitionName<'a> {
    Binding(&'a ast::Identifier),
    Plain(&'a str),
}

enum Body<'a> {
    Namespace(&'a [ast::Stmt]),
    Statements(&'a [ast::Stmt]),
    /// A suspended suite and whether it was declared with async def.
    GeneratorFunction((&'a [ast::Stmt], bool)),
    ModuleAnnotations(Vec<(&'a str, &'a ast::Expr)>),
    ClassAnnotations(Vec<statements::class_annotations::Site>, BTreeSet<String>),
    Annotations(
        Vec<(&'a str, &'a ast::Expr)>,
        Option<statements::ClassBindings>,
    ),
    Expression(&'a ast::Expr),
    YieldingLambda(&'a ast::Expr),
    TypeAlias(&'a ast::Expr),
    Generator(&'a [ast::Comprehension], &'a ast::Expr),
    UnsupportedAsyncGenerator(&'a ast::StmtFunctionDef),
}

struct SourceMap<'a> {
    module_start: ruff_text_size::TextSize,
    docs: Option<crate::modules::doctest::SealedDocs>,
    unit: usize,
    has_templates: bool,
    template_expressions: HashMap<ruff_text_size::TextSize, String>,
    replacements: HashMap<ruff_text_size::TextRange, Vec<u32>>,
    private_names: imre_ast::Names,
    method_imports: std::collections::BTreeSet<String>,
    text: &'a str,
    package: &'a str,
    string_annotations: bool,
    barry_as_flufl: bool,
    file: &'a str,
    newlines: Vec<usize>,
}

impl SourceMap<'_> {
    /// Keep documentation attached to the bundled definition, even when callers rebind its name.
    fn docstring(&self, qualname: &str, body: &[ast::Stmt]) -> Option<String> {
        crate::frontend::docstring(body).or_else(|| {
            self.docs
                .as_ref()?
                .object_docstring(qualname)
                .map(str::to_owned)
        })
    }

    fn line(&self, offset: ruff_text_size::TextSize) -> usize {
        self.newlines.partition_point(|&n| n < usize::from(offset)) + 1
    }
}

#[derive(Eq, Hash, PartialEq)]
enum StringConstant {
    Text(Vec<u8>),
    Tuple(Vec<Vec<u8>>),
}

#[derive(Default)]
struct Output {
    keep_docstrings: bool,
    string_constant_slots: HashMap<(usize, StringConstant), usize>,
    strings: HashMap<Vec<u8>, u32>,
    interned_identifiers: BTreeSet<String>,
    positions: HashMap<[usize; 4], usize>,
    constants: String,
    functions: String,
    count: u32,
}

impl Output {
    /// Distinct Python definitions keep distinct addresses even if LLVM merges their bodies.
    fn definition(
        &mut self,
        source: &SourceMap<'_>,
        prefix: String,
        body: Body<'_>,
        start: ruff_text_size::TextSize,
        in_function: bool,
        suspended: Option<(String, usize)>,
    ) -> String {
        let site = format!("@code_site{}", self.count);
        let (entry, body_line) = if let Some(source) = suspended {
            self.count += 1;
            source
        } else {
            (self.code(source, prefix, body, in_function), 0)
        };
        writeln!(
            self.constants,
            "{site} = private constant %code_site {{ ptr {entry}, ptr {}, i32 {}, i32 {body_line} }}",
            source.file,
            source.line(start)
        )
        .unwrap();
        site
    }

    fn data(&mut self, bytes: &[u8]) -> String {
        let count = self.strings.len() as u32;
        let index = *self.strings.entry(bytes.to_vec()).or_insert(count);
        if index == count {
            write!(
                self.constants,
                "@str{index} = private unnamed_addr constant [{} x i8] c\"",
                bytes.len() + 1
            )
            .unwrap();
            for &byte in bytes {
                if (b' '..=b'~').contains(&byte) && !matches!(byte, b'"' | b'\\') {
                    self.constants.push(char::from(byte));
                } else {
                    write!(self.constants, "\\{byte:02X}").unwrap();
                }
            }
            self.constants.push_str("\\00\"\n");
        }
        format!("@str{index}")
    }

    fn position(&mut self, span: [usize; 4]) -> String {
        let next = self.positions.len();
        let index = *self.positions.entry(span).or_insert(next);
        if index == next {
            let [line, start, split, end] = span;
            writeln!(self.constants, "@pos{index} = private unnamed_addr constant [4 x i32] [i32 {line}, i32 {start}, i32 {split}, i32 {end}]").unwrap();
        }
        format!("@pos{index}")
    }

    fn code(
        &mut self,
        source: &SourceMap<'_>,
        prefix: String,
        body: Body<'_>,
        in_function: bool,
    ) -> String {
        let name = format!("@code{}", self.count);
        self.count += 1;
        let module = matches!(body, Body::Namespace(_)) && prefix.is_empty();
        let mut f = Function {
            output: self,
            source,
            class_sites: None,
            module_annotation_sites: None,
            in_function,
            module,
            declared_globals: Default::default(),
            annotation_lookup: false,
            class_locals: match &body {
                Body::Namespace(stmts) if !prefix.is_empty() => {
                    Some(statements::class_locals(stmts))
                }
                _ => None,
            },
            prefix,
            ir: String::new(),
            block: "entry".into(),
            error: "failed".into(),
            scope: "%scope".into(),
            serial: 0,
            terminated: false,
            live: Vec::new(),
            max_live: 0,
            loops: Vec::new(),
            exits: Vec::new(),
            caught: Vec::new(),
            simple_call: None,
            expression_line: 0,
        };
        match body {
            Body::Namespace(stmts) => {
                if module && imre_ast::annotations::has_variable_annotation(stmts) {
                    if source.string_annotations {
                        f.op("setup_module_string_annotations", &[&f.scope.clone()]);
                    } else {
                        // Lower the provider after collecting the body, but run its setup first.
                        f.module_annotation_sites = Some(Vec::new());
                        f.jump("annotations_init");
                        f.label("module_body");
                    }
                }
                if f.output.keep_docstrings
                    && let Some(doc) = f.source.docstring(f.prefix.trim_end_matches('.'), stmts)
                {
                    let value = f.text(&doc);
                    f.bind("__doc__", &value);
                }
                f.declare_globals(stmts);
                if f.class_locals.is_some() && !f.source.string_annotations {
                    f.annotated_class_body(stmts);
                } else {
                    f.statements(stmts);
                }
                if let Some(sites) = f.module_annotation_sites.take() {
                    f.end("ret i64 0");
                    f.label("annotations_init");
                    f.setup_deferred_module_annotations(&sites);
                    f.jump("module_body");
                }
            }
            Body::Statements(stmts) => {
                f.declare_globals(stmts);
                f.statements(stmts);
            }
            Body::GeneratorFunction(_) | Body::YieldingLambda(_) => {
                unreachable!("suspended definitions store their source in the code site")
            }
            Body::ClassAnnotations(sites, globals) => f.class_annotation_values(&sites, &globals),
            Body::Annotations(entries, class) => f.annotations(&entries, class, false),
            Body::ModuleAnnotations(entries) => f.annotations(&entries, None, true),
            Body::UnsupportedAsyncGenerator(function) => {
                f.checkpoint(function);
                f.unsupported("async function");
            }
            Body::Generator(generators, element) => f.generator_step(generators, element),
            Body::Expression(expr) | Body::TypeAlias(expr) => {
                f.checkpoint(expr);
                if matches!(body, Body::TypeAlias(_)) {
                    let format = f.lookup(".format");
                    f.op("annotation_format", &[&format]);
                }
                let value = f.expr(expr);
                f.end(&format!("ret i64 {value}"));
            }
        }
        if !f.terminated {
            f.end("ret i64 0");
        }
        let mut entry = if f.max_live == 0 {
            String::new()
        } else {
            format!("  %live = alloca [{} x i64]\n", f.max_live)
        };
        // A loaded code object owns its identifier-like strings before any statement runs.
        if module && !f.output.interned_identifiers.is_empty() {
            let names = std::mem::take(&mut f.output.interned_identifiers)
                .into_iter()
                .collect::<Vec<_>>()
                .join("\0");
            let data = f.output.data(names.as_bytes());
            writeln!(
                entry,
                "  call i64 @imre_dyn_lit(ptr %ctx, i32 5, ptr {data}, i64 {})",
                names.len()
            )
            .unwrap();
        }
        writeln!(f.output.functions,
            "define internal i64 {name}(ptr %ctx, i64 %scope) {{\nentry:\n{entry}{}failed:\n  ret i64 0\n}}", f.ir).unwrap();
        name
    }
}

struct Function<'a, 'p> {
    output: &'a mut Output,
    source: &'p SourceMap<'p>,
    prefix: String,
    declared_globals: std::collections::BTreeSet<String>,
    in_function: bool,
    module: bool,
    module_annotation_sites: Option<Vec<(String, ast::Expr)>>,
    class_sites: Option<statements::class_annotations::ClassSites>,
    class_locals: Option<statements::ClassBindings>,
    annotation_lookup: bool,
    ir: String,
    block: String,
    error: String,
    scope: String,
    serial: usize,
    terminated: bool,
    // Handles held in native registers across statement boundaries must stay reachable.
    live: Vec<String>,
    max_live: usize,
    loops: Vec<LoopTarget>,
    exits: Vec<(Cleanup, String)>,
    caught: Vec<String>,
    simple_call: Option<ruff_text_size::TextRange>,
    expression_line: usize,
}

#[derive(Clone)]
enum Cleanup {
    With {
        exit: String,
        caught: String,
    },
    ExceptionName(String),
    PendingValue {
        value: String,
        enclosing_exception: Option<String>,
    },
    Finally {
        body: std::rc::Rc<[ast::Stmt]>,
        loop_depth: usize,
        caught_depth: usize,
    },
}

struct LoopTarget {
    step: String,
    done: String,
    iterator: Option<String>,
    cleanup_depth: usize,
}

impl Function<'_, '_> {
    fn name(&mut self, prefix: &str) -> String {
        let n = self.serial;
        self.serial += 1;
        format!("{prefix}{n}")
    }

    fn line(&mut self, line: &str) {
        writeln!(self.ir, "  {line}").unwrap();
    }

    fn value(&mut self, instruction: &str) -> String {
        let value = self.name("%v");
        self.line(&format!("{value} = {instruction}"));
        value
    }

    fn end(&mut self, instruction: &str) {
        if !self.terminated {
            self.line(instruction);
            self.terminated = true;
        }
    }

    fn jump(&mut self, label: &str) {
        self.end(&format!("br label %{label}"));
    }

    fn label(&mut self, label: &str) {
        writeln!(self.ir, "{label}:").unwrap();
        self.block = label.into();
        self.terminated = false;
    }

    fn branch(&mut self, condition: &str, yes: &str, no: &str) {
        self.end(&format!("br i1 {condition}, label %{yes}, label %{no}"));
    }

    fn checked(&mut self, invocation: &str) -> String {
        let result = self.value(invocation);
        self.check_error();
        result
    }

    fn check_error(&mut self) {
        let failed = self.value("call i1 @imre_dyn_failed(ptr %ctx)");
        let next = self.name("b");
        self.branch(&failed, &self.error.clone(), &next);
        self.label(&next);
    }

    fn op(&mut self, name: &str, args: &[&str]) -> String {
        self.op_at(name, args, None)
    }

    fn op_at(&mut self, name: &str, args: &[&str], span: Option<[usize; 4]>) -> String {
        let op = self.output.data(name.as_bytes());
        let arg = |index| *args.get(index).unwrap_or(&"0");
        let position = span.map_or_else(|| "null".into(), |span| self.output.position(span));
        self.checked(&format!(
            "call i64 @imre_dyn_op_at(ptr %ctx, ptr {op}, i64 {}, i64 {}, i64 {}, ptr {position})",
            arg(0),
            arg(1),
            arg(2)
        ))
    }

    fn lit(&mut self, kind: i32, bytes: &[u8]) -> String {
        let literal = self.output.data(bytes);
        self.checked(&format!(
            "call i64 @imre_dyn_lit(ptr %ctx, i32 {kind}, ptr {literal}, i64 {})",
            bytes.len()
        ))
    }

    fn text(&mut self, value: &str) -> String {
        if !value.is_empty()
            && value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_')
        {
            self.output.interned_identifiers.insert(value.to_owned());
        }
        self.lit(1, value.as_bytes())
    }
    /// Emit a code-object name, including a Unicode identifier that would not intern as a literal.
    fn code_name(&mut self, name: &str) -> String {
        self.output.interned_identifiers.insert(name.to_owned());
        self.lit(6, name.as_bytes())
    }

    fn integer(&mut self, n: impl ToString) -> String {
        self.lit(0, n.to_string().as_bytes())
    }

    fn lookup(&mut self, name: &str) -> String {
        let op = match &self.class_locals {
            Some(class) if self.annotation_lookup && class.globals.contains(name) => {
                "lookup_annotation_global"
            }
            Some(class) if self.annotation_lookup && class.locals.contains(name) => {
                "lookup_class_annotation_global"
            }
            Some(_) if self.annotation_lookup => "lookup_class_annotation",
            Some(class) if class.locals.contains(name) => "lookup_class_local",
            _ => "lookup",
        };
        let name = self.code_name(name);
        self.op(op, &[&self.scope.clone(), &name])
    }

    fn bind(&mut self, name: &str, value: &str) {
        let name = self.code_name(name);
        self.op("assign", &[&self.scope.clone(), &name, value]);
    }

    fn attr(&mut self, object: &str, name: &str) -> String {
        let name = self.code_name(name);
        self.op("attr", &[object, &name])
    }

    fn truth(&mut self, value: &str) -> String {
        let boolean = self.op("truth", &[value]);
        self.value(&format!("icmp eq i64 {boolean}, 2"))
    }

    /// Save the caller's earlier operands before an expression can release a writer.
    fn temporary_mark(&mut self) -> String {
        self.value("call i64 @imre_dyn_truth_mark(ptr %ctx)")
    }

    /// End the lifetime of the operands evaluated after the mark.
    fn discard_truth(&mut self, mark: &str) {
        self.line(&format!(
            "call void @imre_dyn_discard_truth(ptr %ctx, i64 {mark})"
        ));
    }

    fn select(
        &mut self,
        condition: &ast::Expr,
        on_true: impl FnOnce(&mut Self) -> String,
        on_false: impl FnOnce(&mut Self) -> String,
    ) -> String {
        let yes = self.name("b");
        let no = self.name("b");
        let done = self.name("b");
        self.branch_expr(condition, &yes, &no);
        self.label(&yes);
        let left = on_true(self);
        let left_block = self.block.clone();
        self.jump(&done);
        self.label(&no);
        let right = on_false(self);
        let right_block = self.block.clone();
        self.jump(&done);
        self.label(&done);
        self.value(&format!(
            "phi i64 [ {left}, %{left_block} ], [ {right}, %{right_block} ]"
        ))
    }

    fn unsupported(&mut self, what: &str) -> String {
        let what = self.text(what);
        self.op("unsupported", &[&what])
    }

    fn expression_span(
        &self,
        expr: &ast::Expr,
        prefix: &ast::Expr,
        opener: u8,
    ) -> Option<[usize; 4]> {
        let range = expr.range();
        if self.simple_call == Some(range) {
            return None;
        }
        let start = range.start().to_usize();
        let end = range.end().to_usize();
        let prefix_end = prefix.end().to_usize();
        let text = self.source.text.as_bytes();
        if text.get(start..end)?.contains(&b'\n') {
            return None;
        }
        let split = prefix_end
            + text
                .get(prefix_end..end)?
                .iter()
                .position(|&b| b == opener)?;
        let line = self.source.line(range.start());
        let beginning = if line == 1 {
            0
        } else {
            self.source.newlines[line - 2] + 1
        };
        Some([line, start - beginning, split - beginning, end - beginning])
    }

    /// Store the handles that must survive the next statement or assignment target.
    fn live_handles(&mut self) -> (&'static str, usize) {
        let count = self.live.len();
        self.max_live = self.max_live.max(count);
        for (i, value) in self.live.clone().iter().enumerate() {
            let ptr = self.value(&format!("getelementptr i64, ptr %live, i64 {i}"));
            self.line(&format!("store i64 {value}, ptr {ptr}"));
        }
        (if count == 0 { "null" } else { "%live" }, count)
    }

    fn checkpoint(&mut self, node: &impl Ranged) {
        let (ptr, count) = self.live_handles();
        let line = self.source.line(node.start());
        self.expression_line = line;
        self.line(&format!(
            "call void @imre_dyn_statement_with_exception(ptr %ctx, ptr {ptr}, i64 {count}, ptr {}, i32 {line}, i64 {})",
            self.source.file, self.caught.last().map_or("0", String::as_str)
        ));
        self.check_error();
    }

    fn invoke(&mut self, callable: &str, values: &[&str]) -> String {
        let mark = self.temporary_mark();
        let args = self.op("list", &[]);
        for value in values {
            self.op("append", &[&args, value]);
        }
        let kwargs = self.op("dict", &[]);
        self.call_at(callable, &args, &kwargs, &mark, None)
    }

    /// Pass the caller's temporary root boundary and optional traceback position.
    fn call_at(
        &mut self,
        callable: &str,
        args: &str,
        kwargs: &str,
        mark: &str,
        span: Option<[usize; 4]>,
    ) -> String {
        let position = span.map_or_else(|| "null".into(), |span| self.output.position(span));
        let scope = &self.scope;
        self.checked(&format!("call i64 @imre_dyn_scoped_call_at(ptr %ctx, i64 {callable}, i64 {args}, i64 {kwargs}, i64 {mark}, ptr {position}, i64 {scope})"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_substitution() {
        let root = Path::new("<imre stdlib>");
        let path = root.join("example.py");
        let source = "value = 1";
        let compiled =
            |name, path: &Path, src, docs| provider(name, path, src, docs).unwrap().unwrap().0;
        let symbol = compiled("example", &path, source, true);
        assert_ne!(symbol, compiled("other", &path, source, true));
        assert_ne!(
            symbol,
            compiled("example", &root.join("other.py"), source, true)
        );
        assert_ne!(symbol, compiled("example", &path, "value = 2", true));
        assert_ne!(symbol, compiled("example", &path, source, false));
        assert!(
            provider("example", &path, "value = t'{123}'", true)
                .unwrap()
                .is_none()
        );

        let bundled = ("example".into(), path, source.into());
        let project = (
            "example".into(),
            PathBuf::from("/project/example.py"),
            source.into(),
        );
        let symbols = HashSet::from([symbol.clone()]);
        let refs = |modules: &[_]| {
            let (ir, used) = emit(modules, "{}", true, root, &symbols).unwrap();
            assert!(used);
            let declared = format!("declare i64 @{symbol}(ptr, i64)");
            assert_eq!(ir.lines().filter(|line| *line == declared).count(), 1);
            ir.lines()
                .filter(|line| line.contains("call void @imre_dyn_register_source("))
                .map(|line| line.ends_with(&format!("@{symbol})")))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            refs(&[bundled.clone(), project.clone(), bundled.clone()]),
            [true, false, true]
        );
        assert!(
            !emit(std::slice::from_ref(&project), "{}", true, root, &symbols)
                .unwrap()
                .1
        );
        assert_eq!(refs(&[project, bundled]), [false, true]);
    }
}
