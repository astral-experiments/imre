//! Emit native functions and control flow for closed modules using boxed Python values.

use std::collections::{BTreeSet, HashMap};
use std::fmt::Write as _;
use std::path::PathBuf;

use ruff_python_ast as ast;
use ruff_text_size::Ranged;

mod expressions;
mod statements;

const ABI: &str = "\
declare ptr @imre_dyn_new(i32, ptr, ptr)
declare void @imre_dyn_register(ptr, ptr, ptr, ptr)
declare i32 @imre_dyn_finish(ptr)
declare i64 @imre_dyn_lit(ptr, i32, ptr, i64)
declare i64 @imre_dyn_op(ptr, ptr, i64, i64, i64)
declare i64 @imre_dyn_def_from_site(ptr, ptr, i64, i64, i64, i64, i64, i64, i64, i1)
declare void @imre_dyn_generator_body(ptr, i64, ptr, i32)
declare i1 @imre_dyn_failed(ptr)
declare i64 @imre_dyn_op_at(ptr, ptr, i64, i64, i64, ptr)
declare void @imre_dyn_statement_with_exception(ptr, ptr, i64, ptr, i32, i64)
";

/// All module source has already been resolved. Import execution remains conditional.
pub(crate) fn emit(
    modules: &[(String, PathBuf, String)],
    config: &str,
    keep_docstrings: bool,
) -> Result<String, String> {
    let mut output = Output {
        keep_docstrings,
        ..Output::default()
    };
    let config = output.data(config.as_bytes());
    let mut main = format!(
        "define i32 @main(i32 %argc, ptr %argv) {{\nentry:\n  %ctx = call ptr @imre_dyn_new(i32 %argc, ptr %argv, ptr {config})\n"
    );
    for (name, path, source) in modules {
        let parsed = ruff_python_parser::parse_module(source)
            .map_err(|err| format!("{}: {err}", path.display()))?;
        crate::frontend::check_string_tokens(source, parsed.tokens())
            .map_err(|error| format!("{}:{error}", path.display()))?;
        let package = if path.file_name().is_some_and(|file| file == "__init__.py") {
            name.as_str()
        } else {
            name.rsplit_once('.').map_or("", |(parent, _)| parent)
        };
        let name = output.data(name.as_bytes());
        let file = output.data(path.to_str().ok_or("module path must be UTF-8")?.as_bytes());
        let location = SourceMap {
            package,
            text: source,
            string_annotations: parsed.syntax().body.iter().any(|s| {
                matches!(s, ast::Stmt::ImportFrom(i) if i.level == 0
                && i.module.as_ref().is_some_and(|m| m.as_str() == "__future__")
                && i.names.iter().any(|n| n.name.as_str() == "annotations"))
            }),
            file: &file,
            newlines: source
                .match_indices('\n')
                .map(|(offset, _)| offset)
                .collect(),
        };
        let code = output.code(
            &location,
            String::new(),
            Body::Namespace(&parsed.syntax().body),
        );
        writeln!(
            main,
            "  call void @imre_dyn_register(ptr %ctx, ptr {name}, ptr {file}, ptr {code})"
        )
        .unwrap();
    }
    main.push_str("  %status = call i32 @imre_dyn_finish(ptr %ctx)\n  ret i32 %status\n}\n");
    Ok(format!(
        "{ABI}\n{}\n{}\n{main}",
        output.constants, output.functions
    ))
}

enum Body<'a> {
    Namespace(&'a [ast::Stmt]),
    Statements(&'a [ast::Stmt]),
    GeneratorFunction(&'a [ast::Stmt]),
    Annotations(Vec<(&'a str, &'a ast::Expr)>),
    Expression(&'a ast::Expr),
    Generator(&'a ast::Comprehension, &'a ast::Expr),
    Unsupported(&'a ast::Stmt, &'static str),
}

struct SourceMap<'a> {
    text: &'a str,
    package: &'a str,
    string_annotations: bool,
    file: &'a str,
    newlines: Vec<usize>,
}

impl SourceMap<'_> {
    fn line(&self, offset: ruff_text_size::TextSize) -> usize {
        self.newlines.partition_point(|&n| n < usize::from(offset)) + 1
    }
}

#[derive(Default)]
struct Output {
    keep_docstrings: bool,
    strings: HashMap<Vec<u8>, u32>,
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
    ) -> String {
        let site = format!("@code_site{}", self.count);
        let code = self.code(source, prefix, body);
        writeln!(
            self.constants,
            "{site} = private constant {{ ptr, ptr, i32 }} {{ ptr {code}, ptr {}, i32 {} }}",
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

    fn code(&mut self, source: &SourceMap<'_>, prefix: String, body: Body<'_>) -> String {
        let name = format!("@code{}", self.count);
        self.count += 1;
        let mut f = Function {
            output: self,
            source,
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
        };
        match body {
            Body::Namespace(stmts) => {
                if f.output.keep_docstrings
                    && let Some(doc) = crate::frontend::docstring(stmts)
                {
                    let value = f.text(&doc);
                    f.bind("__doc__", &value);
                }
                f.declare_globals(stmts);
                f.statements(stmts);
            }
            Body::Statements(stmts) => {
                f.declare_globals(stmts);
                f.statements(stmts);
            }
            Body::GeneratorFunction(_) => {}
            Body::Annotations(entries) => f.annotations(&entries),
            Body::Unsupported(stmt, reason) => {
                f.checkpoint(stmt);
                f.unsupported(reason);
            }
            Body::Generator(generator, element) => f.generator_step(generator, element),
            Body::Expression(expr) => {
                f.checkpoint(expr);
                let value = f.expr(expr);
                f.end(&format!("ret i64 {value}"));
            }
        }
        if !f.terminated {
            f.end("ret i64 0");
        }
        let entry = if f.max_live == 0 {
            String::new()
        } else {
            format!("  %live = alloca [{} x i64]\n", f.max_live)
        };
        writeln!(f.output.functions,
            "define internal i64 {name}(ptr %ctx, i64 %scope) {{\nentry:\n{entry}{}failed:\n  ret i64 0\n}}", f.ir).unwrap();
        name
    }
}

struct Function<'a, 'p> {
    output: &'a mut Output,
    source: &'p SourceMap<'p>,
    prefix: String,
    class_locals: Option<BTreeSet<String>>,
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
}

#[derive(Clone)]
enum Cleanup {
    With(String),
    Finally(std::rc::Rc<[ast::Stmt]>),
}

struct LoopTarget {
    step: String,
    done: String,
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
        let failed = self.value("call i1 @imre_dyn_failed(ptr %ctx)");
        let next = self.name("b");
        self.branch(&failed, &self.error.clone(), &next);
        self.label(&next);
        result
    }

    fn op(&mut self, name: &str, args: &[&str]) -> String {
        self.op_at(name, args, None)
    }

    fn op_at(&mut self, name: &str, args: &[&str], span: Option<[usize; 4]>) -> String {
        let op = self.output.data(name.as_bytes());
        let arg = |index| *args.get(index).unwrap_or(&"0");
        let fixed = format!(
            "ptr %ctx, ptr {op}, i64 {}, i64 {}, i64 {}",
            arg(0),
            arg(1),
            arg(2)
        );
        let invocation = if let Some(span) = span {
            let position = self.output.position(span);
            format!("call i64 @imre_dyn_op_at({fixed}, ptr {position})")
        } else {
            format!("call i64 @imre_dyn_op({fixed})")
        };
        self.checked(&invocation)
    }

    fn lit(&mut self, kind: i32, bytes: &[u8]) -> String {
        let literal = self.output.data(bytes);
        self.checked(&format!(
            "call i64 @imre_dyn_lit(ptr %ctx, i32 {kind}, ptr {literal}, i64 {})",
            bytes.len()
        ))
    }

    fn text(&mut self, value: &str) -> String {
        self.lit(1, value.as_bytes())
    }
    fn integer(&mut self, n: impl ToString) -> String {
        self.lit(0, n.to_string().as_bytes())
    }

    fn lookup(&mut self, name: &str) -> String {
        let op = if self
            .class_locals
            .as_ref()
            .is_some_and(|locals| locals.contains(name))
        {
            "lookup_class_local"
        } else {
            "lookup"
        };
        let name = self.text(name);
        self.op(op, &[&self.scope.clone(), &name])
    }

    fn bind(&mut self, name: &str, value: &str) {
        let name = self.text(name);
        self.op("assign", &[&self.scope.clone(), &name, value]);
    }

    fn attr(&mut self, object: &str, name: &str) -> String {
        let name = self.text(name);
        self.op("attr", &[object, &name])
    }

    fn truth(&mut self, value: &str) -> String {
        let boolean = self.op("truth", &[value]);
        self.value(&format!("icmp eq i64 {boolean}, 2"))
    }

    fn select(
        &mut self,
        condition: &str,
        on_true: impl FnOnce(&mut Self) -> String,
        on_false: impl FnOnce(&mut Self) -> String,
    ) -> String {
        let yes = self.name("b");
        let no = self.name("b");
        let done = self.name("b");
        self.branch(condition, &yes, &no);
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

    fn checkpoint(&mut self, node: &impl Ranged) {
        let count = self.live.len();
        self.max_live = self.max_live.max(count);
        for (i, value) in self.live.clone().iter().enumerate() {
            let ptr = self.value(&format!("getelementptr i64, ptr %live, i64 {i}"));
            self.line(&format!("store i64 {value}, ptr {ptr}"));
        }
        let ptr = if count == 0 { "null" } else { "%live" };
        let line = self.source.line(node.start());
        self.line(&format!(
            "call void @imre_dyn_statement_with_exception(ptr %ctx, ptr {ptr}, i64 {count}, ptr {}, i32 {line}, i64 {})",
            self.source.file, self.caught.last().map_or("0", String::as_str)
        ));
    }

    fn invoke(&mut self, callable: &str, values: &[&str]) -> String {
        let args = self.op("list", &[]);
        for value in values {
            self.op("append", &[&args, value]);
        }
        let kwargs = self.op("dict", &[]);
        self.op("call", &[callable, &args, &kwargs])
    }
}
