//! Emit native functions and control flow for closed modules using boxed Python values.

use std::collections::HashMap;
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
declare i64 @imre_dyn_def_with_metadata(ptr, ptr, i64, i64, i64, i64, i64, i64, i64, i1)
declare i1 @imre_dyn_failed(ptr)
declare void @imre_dyn_statement(ptr, ptr, i64, ptr, i32)
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
    Annotations(Vec<(&'a str, &'a ast::Expr)>),
    Expression(&'a ast::Expr),
    Generator(&'a ast::Comprehension, &'a ast::Expr),
    Unsupported(&'a ast::Stmt, &'static str),
}

struct SourceMap<'a> {
    package: &'a str,
    string_annotations: bool,
    file: &'a str,
    newlines: Vec<usize>,
}

#[derive(Default)]
struct Output {
    keep_docstrings: bool,
    strings: HashMap<Vec<u8>, u32>,
    constants: String,
    functions: String,
    count: u32,
}

impl Output {
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

    fn code(&mut self, source: &SourceMap<'_>, prefix: String, body: Body<'_>) -> String {
        let name = format!("@code{}", self.count);
        self.count += 1;
        let mut f = Function {
            output: self,
            source,
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
        let op = self.output.data(name.as_bytes());
        let arg = |index| *args.get(index).unwrap_or(&"0");
        self.checked(&format!(
            "call i64 @imre_dyn_op(ptr %ctx, ptr {op}, i64 {}, i64 {}, i64 {})",
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
        self.lit(1, value.as_bytes())
    }
    fn integer(&mut self, n: impl ToString) -> String {
        self.lit(0, n.to_string().as_bytes())
    }

    fn lookup(&mut self, name: &str) -> String {
        let name = self.text(name);
        self.op("lookup", &[&self.scope.clone(), &name])
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

    fn checkpoint(&mut self, node: &impl Ranged) {
        let count = self.live.len();
        self.max_live = self.max_live.max(count);
        for (i, value) in self.live.clone().iter().enumerate() {
            let ptr = self.value(&format!("getelementptr i64, ptr %live, i64 {i}"));
            self.line(&format!("store i64 {value}, ptr {ptr}"));
        }
        let ptr = if count == 0 { "null" } else { "%live" };
        let offset = usize::from(node.start());
        let line = self
            .source
            .newlines
            .partition_point(|&newline| newline < offset)
            + 1;
        self.line(&format!(
            "call void @imre_dyn_statement(ptr %ctx, ptr {ptr}, i64 {count}, ptr {}, i32 {line})",
            self.source.file
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
