//! Emit native functions and control flow for closed modules using boxed Python values.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use ruff_python_ast as ast;
use ruff_text_size::Ranged;

use crate::CodegenUnit;
use crate::llvm::operations::{CheckedValue, ValueJoin};

mod boolean;
pub(crate) mod checked;
mod comprehensions;
mod expressions;
mod frame;
mod retained;
use frame::{RegisteredFrame, SourcePositions};
mod patterns;
mod sequences;
mod statements;
mod strings;
mod suspension;

pub(super) mod permanent {
    include!("../../../imre-runtime/native/boxed-handles.rs");
}

const ABI: &str = "\
%code_site = type { ptr, ptr, i32, i32 }
declare ptr @imre_dyn_new(i32, ptr, ptr)
declare void @imre_dyn_register(ptr, ptr, ptr, ptr)
declare i32 @imre_dyn_finish(ptr)
declare i64 @imre_dyn_lit(ptr, i32, ptr, i64)
declare i64 @imre_dyn_build_sequence(ptr, ptr, i64, i1, i1, ptr)
declare i64 @imre_dyn_build_set(ptr, ptr, i64, i64)
declare i64 @imre_dyn_display_item(ptr, i64, i64, i64, i1)
declare void @imre_dyn_unwind_display(ptr, i64)
declare i64 @imre_dyn_cached_handle(ptr, ptr)
declare i64 @imre_dyn_linked_value(ptr, ptr, i64)
declare i64 @imre_dyn_linked_string(ptr, ptr, i32, ptr, i64)
declare i64 @imre_dyn_lookup(ptr, i64, ptr, i64)
declare i64 @imre_dyn_import_name(ptr, i64, i64, i64, i32)
declare i64 @imre_dyn_mark_class_annotation(ptr, i64, i64, i32)
declare i64 @imre_dyn_define(ptr, ptr, i64, i64, i64, i64, i64, i64, i64, i1)
declare i1 @imre_dyn_failed(ptr)
declare i64 @imre_dyn_truth_mark(ptr)
declare void @imre_dyn_comprehension_step(ptr, i64)
declare void @imre_dyn_discard_truth(ptr, i64)
declare i64 @imre_dyn_keep_operands(ptr, i64, i64, i64)
declare void @imre_dyn_discard_expression(ptr, i64)
declare void @imre_dyn_named_assignment(ptr, i64, i64, i64)
declare void @imre_dyn_assignment_target(ptr, ptr, i64, i64)
declare i64 @imre_dyn_op_at(ptr, ptr, i64, i64, i64, ptr)
declare i64 @imre_dyn_get_slice_at(ptr, i64, i64, i64, ptr)
declare i64 @imre_dyn_get_linked_slice_at(ptr, i64, i64, i64, ptr, ptr)
declare void @imre_dyn_call_position(ptr, ptr)
declare i64 @imre_dyn_contains_literal_tuple(ptr, i64, ptr, i64, i32)
declare i64 @imre_dyn_scoped_call_at(ptr, i64, i64, i64, i64, ptr, i64)
declare i64 @imre_dyn_positional_method_at(ptr, i64, ptr, i64, i64, ptr, i64)
declare i64 @imre_dyn_partition_field_at(ptr, i64, ptr, i32, ptr, i64, ptr, i64)
declare i64 @imre_dyn_string_method_target(ptr, i64, ptr, ptr)
declare i64 @imre_dyn_unpack_pair(ptr, i64, ptr)
declare i64 @imre_dyn_next_pair(ptr, i64, ptr, i1)
declare i64 @imre_dyn_builtin_method_target(ptr, i64, ptr, ptr)
declare i64 @imre_dyn_dict_get_target(ptr, i64, ptr)
declare i64 @imre_dyn_positional_call_at(ptr, i64, ptr, i64, i64, ptr, i64)
declare i64 @imre_dyn_keyword_call_at(ptr, i64, ptr, i64, ptr, i64, i64, ptr, i64)
declare void @imre_dyn_expression_line(ptr, i32)
declare void @imre_dyn_statement_with_exception(ptr, ptr, i64, ptr, i32, i64)
";

// Keep repeated code-name loads and their error edge out of each caller.
const NAMED_OP: &str = "\
define internal i64 @imre_boxed_named_op(ptr %ctx, ptr %op, i64 %receiver, ptr %name, i64 %len, i64 %value) noinline {
entry:
  %key = call i64 @imre_dyn_lit(ptr %ctx, i32 6, ptr %name, i64 %len)
  %failed = call i1 @imre_dyn_failed(ptr %ctx)
  br i1 %failed, label %error, label %apply
error:
  ret i64 0
apply:
  %result = tail call i64 @imre_dyn_op_at(ptr %ctx, ptr %op, i64 %receiver, i64 %key, i64 %value, ptr null)
  ret i64 %result
}
";

const GUARDED_NAMED_OP: &str = "\
define internal i64 @imre_boxed_guarded_named_op(ptr %ctx, ptr %op, i64 %receiver, ptr %name, i64 %len, i64 %value) noinline {
entry:
  %pending = call i1 @imre_dyn_failed(ptr %ctx)
  br i1 %pending, label %error, label %apply
error:
  ret i64 0
apply:
  %result = tail call i64 @imre_boxed_named_op(ptr %ctx, ptr %op, i64 %receiver, ptr %name, i64 %len, i64 %value)
  ret i64 %result
}
";

const GUARDED_LIT: &str = "\
define internal i64 @imre_boxed_guarded_lit(ptr %ctx, i32 %kind, ptr %data, i64 %len) noinline {
entry:
  %pending = call i1 @imre_dyn_failed(ptr %ctx)
  br i1 %pending, label %error, label %load
error:
  ret i64 0
load:
  %value = tail call i64 @imre_dyn_lit(ptr %ctx, i32 %kind, ptr %data, i64 %len)
  ret i64 %value
}
";

type ModuleSource = (String, PathBuf, String);

#[derive(Clone, Copy, Default)]
pub(super) struct PhysicalFile {
    pub(super) digest: Option<[u8; 32]>,
    pub(super) future_flags: u32,
}

/// Snapshots and admission facts for the bodies selected by import resolution.
#[derive(Default)]
pub(super) struct RegisteredBodies<'a> {
    pub(super) program_bodies: HashMap<usize, ProgramBodies<'a>>,
    // Decoded installed files retain the fingerprint of their original loader bytes.
    pub(super) physical_files: HashMap<PathBuf, PhysicalFile>,
    pub(super) archive_members: HashMap<PathBuf, ([u8; 32], u32)>,
    // Each source slot has its own ownership, even if two names resolve to one file.
    // Providers contribute declarations to the checked program. Their Python bodies
    // retain Python argument binding, including annotations available to reflection.
    pub(super) python_providers: HashSet<usize>,
}

/// Each source slot can use the checked bodies owned by its selected physical file.
#[derive(Clone, Copy, Default)]
pub(super) struct ProgramBodies<'a> {
    pub(super) checked: Option<&'a super::CheckedFunctions>,
    pub(super) native:
        Option<&'a HashMap<ruff_text_size::TextSize, crate::frontend::RegisteredNative>>,
    pub(super) sanitize: bool,
}

#[derive(Clone, Copy)]
pub(super) enum ModuleOrigin<'a> {
    Program(ProgramBodies<'a>),
    Provider,
    Bundled,
}

/// Root constants, registration calls and declarations for one output layout.
#[derive(Clone)]
struct RootCode {
    output: Output,
    main: String,
    declarations: HashSet<String>,
}

impl RootCode {
    fn new(
        config: &str,
        keep_docstrings: bool,
        has_zip: bool,
        has_builtins: bool,
        has_errors: bool,
    ) -> Self {
        let mut output = Output {
            keep_docstrings,
            ..Output::default()
        };
        let config = output.data(config.as_bytes());
        // Entry-only registration must not change fingerprints of unrelated packed providers.
        let main = format!(
            "declare void @imre_dyn_register_source(ptr, ptr, ptr, ptr, i64, ptr)\n\ndefine i32 @main(i32 %argc, ptr %argv) {{\nentry:\n  %ctx = call ptr @imre_dyn_new(i32 %argc, ptr %argv, ptr {config})\n"
        );
        output
            .functions
            .push_str("declare void @imre_dyn_register_file_code(ptr, ptr, ptr, ptr, i32, ptr)\n");
        if has_zip {
            output.functions.push_str(
                "declare void @imre_dyn_register_zip_source(ptr, ptr, ptr, ptr, i32, ptr)\n",
            );
        }
        if has_builtins {
            output.functions.push_str(
                "declare void @imre_dyn_register_builtin_source(ptr, ptr, ptr, ptr, i64, ptr)\n",
            );
        }
        if has_errors {
            output.functions.push_str(
            "declare void @imre_dyn_register_module_error(ptr, ptr, ptr, ptr, i64, ptr, ptr, i8, ptr)\n",
        );
        }
        Self {
            output,
            main,
            declarations: HashSet::new(),
        }
    }

    fn register_error(
        &mut self,
        (name, path, source): &ModuleSource,
        error: &super::ModuleSyntaxError,
        zip: Option<&([u8; 32], u32)>,
    ) {
        let Self { output, main, .. } = self;
        let module_name = output.data(name.as_bytes());
        let file = output.data(path.as_os_str().as_encoded_bytes());
        let message = output.data(error.message.as_bytes());
        let position = error
            .position
            .map_or_else(|| "null".into(), |span| output.position(span));
        let text = output.data(source.as_bytes());
        let hash = zip.map_or_else(|| "null".into(), |(digest, _)| output.data(digest));
        writeln!(
                main,
                "  call void @imre_dyn_register_module_error(ptr %ctx, ptr {module_name}, ptr {file}, ptr {text}, i64 {}, ptr {message}, ptr {position}, i8 {}, ptr {hash})",
                source.len(), if error.parse_error { 2 } else { u8::from(error.in_args) }
            ).unwrap();
    }

    fn finish(mut self) -> CodegenUnit {
        self.output.native_prelude();
        self.main
            .push_str("  %status = call i32 @imre_dyn_finish(ptr %ctx)\n  ret i32 %status\n}\n");
        CodegenUnit {
            name: "__main__".into(),
            llvm_ir: format!(
                "{ABI}\n{}\n{}\n{}",
                self.output.constants, self.output.functions, self.main
            ),
        }
    }
}

/// All module source has already been resolved. Import execution remains conditional.
pub(super) fn emit(
    (files, builtins): (&[ModuleSource], &[ModuleSource]),
    config: &str,
    keep_docstrings: bool,
    bundled_root: &Path,
    providers: &HashSet<String>,
    layout: crate::BoxedLayout<'_>,
    (module_errors, registrations): (
        &HashMap<PathBuf, Box<super::ModuleSyntaxError>>,
        &RegisteredBodies<'_>,
    ),
) -> Result<(Vec<CodegenUnit>, bool), String> {
    let RegisteredBodies {
        program_bodies,
        physical_files,
        archive_members,
        python_providers,
    } = registrations;
    let split = matches!(
        layout,
        crate::BoxedLayout::Split | crate::BoxedLayout::WholeWithUnits(_)
    );
    let mut units = Vec::new();
    let mut used_archive = false;
    let mut root = RootCode::new(
        config,
        keep_docstrings,
        !archive_members.is_empty(),
        !builtins.is_empty(),
        !module_errors.is_empty(),
    );
    // The requested whole IR and split native root share their exact prefix.
    // Fork only when the split layout acquires its first independent local unit.
    let mut whole: Option<RootCode> = None;
    let files = files.iter().map(|module| (module, false));
    let builtins = builtins.iter().map(|module| (module, true));
    for (unit, (module, builtin)) in files.chain(builtins).enumerate() {
        let (name, path, source) = module;
        let zip = archive_members.get(path);
        if !builtin && let Some(error) = module_errors.get(path) {
            for state in std::iter::once(&mut root).chain(whole.as_mut()) {
                state.register_error(module, error, zip);
            }
            continue;
        }
        let bundled = path.starts_with(bundled_root);
        let origin = if bundled {
            ModuleOrigin::Bundled
        } else if python_providers.contains(&unit) {
            ModuleOrigin::Provider
        } else {
            ModuleOrigin::Program(program_bodies.get(&unit).copied().unwrap_or_default())
        };
        // Preserve the loader's digest while compiling its normalized source.
        let physical = physical_files.get(path).filter(|_| !builtin);
        let file_digest = physical_files.get(path).and_then(|file| file.digest);
        let code_source = if physical.is_some() && file_digest.is_none() {
            crate::normalize_source(source, crate::SourceKind::File)?
        } else if file_digest.is_some() || zip.is_some() {
            crate::normalize_source(source, crate::SourceKind::DecodedFile)?
        } else {
            std::borrow::Cow::Borrowed(source.as_str())
        };
        let initializer = PreparedBody::module(name, path, &code_source, keep_docstrings, origin)?;
        let check_archive = bundled && !providers.is_empty();
        let separate = split && unit != 0 && !source.is_empty();
        let mut external = None;
        let mut archived = false;
        if (check_archive || separate)
            && let Some((symbol, ir)) = initializer.standalone()?
        {
            archived = check_archive && providers.contains(&symbol);
            if !archived && separate && !root.declarations.contains(&symbol) {
                if whole.is_none() && matches!(layout, crate::BoxedLayout::WholeWithUnits(_)) {
                    whole = Some(root.clone());
                }
                units.push(CodegenUnit {
                    name: name.clone(),
                    llvm_ir: ir,
                });
            }
            used_archive |= archived;
            if archived || separate {
                external = Some(symbol);
            }
        }
        let register = |state: &mut RootCode, symbol: Option<&str>| -> Result<(), String> {
            let RootCode {
                output,
                main,
                declarations,
            } = state;
            let module_name = output.data(name.as_bytes());
            let file = output.data(path.as_os_str().as_encoded_bytes());
            let code = if let Some(symbol) = symbol {
                if declarations.insert(symbol.to_owned()) {
                    writeln!(output.functions, "declare i64 @{symbol}(ptr, i64)").unwrap();
                }
                format!("@{symbol}")
            } else {
                initializer.emit(output, unit, &file)?
            };
            if let Some((digest, flags)) = zip {
                let hash = output.data(digest);
                writeln!(main, "  call void @imre_dyn_register_zip_source(ptr %ctx, ptr {module_name}, ptr {file}, ptr {hash}, i32 {flags}, ptr {code})").unwrap();
                return Ok(());
            }
            // Physical code reads keep using the file. Identify its compiled snapshot without
            // embedding the source or exposing stale bytes through SourceFileLoader.get_data().
            if let Some(physical) = physical {
                use sha2::{Digest, Sha256};
                let digest = physical
                    .digest
                    .unwrap_or_else(|| Sha256::digest(source.as_bytes()).into());
                let hash = output.data(&digest);
                let flags = physical.future_flags;
                writeln!(main, "  call void @imre_dyn_register_file_code(ptr %ctx, ptr {module_name}, ptr {file}, ptr {hash}, i32 {flags}, ptr {code})").unwrap();
                return Ok(());
            }
            let (text, len) = if bundled {
                (output.data(source.as_bytes()), source.len())
            } else {
                ("null".to_owned(), 0)
            };
            let register = if builtin {
                "imre_dyn_register_builtin_source"
            } else {
                "imre_dyn_register_source"
            };
            writeln!(main, "  call void @{register}(ptr %ctx, ptr {module_name}, ptr {file}, ptr {text}, i64 {len}, ptr {code})").unwrap();
            Ok(())
        };
        register(&mut root, external.as_deref())?;
        if let Some(whole) = &mut whole {
            register(whole, external.as_deref().filter(|_| archived))?;
        }
    }
    units.insert(0, root.finish());
    if let (crate::BoxedLayout::WholeWithUnits(native), Some(whole)) = (layout, whole) {
        *native = std::mem::replace(&mut units, vec![whole.finish()]);
    }
    Ok((units, used_archive))
}

struct ParsedSource {
    parsed: ruff_python_parser::Parsed<ast::ModModule>,
    replacements: HashMap<ruff_text_size::TextRange, Vec<u32>>,
}

fn parse_source(path: &Path, source: &str, keep_docstrings: bool) -> Result<ParsedSource, String> {
    let parsed = imre_ast::indentation::prepare(source)
        .parse_module()
        .map_err(|err| format!("{}: {err}", path.display()))?;
    let replacements = check_parsed_source(path, source, &parsed, keep_docstrings)?;
    Ok(ParsedSource {
        parsed,
        replacements,
    })
}

pub(super) fn check_parsed_source(
    path: &Path,
    source: &str,
    parsed: &ruff_python_parser::Parsed<ast::ModModule>,
    keep_docstrings: bool,
) -> Result<HashMap<ruff_text_size::TextRange, Vec<u32>>, String> {
    if let Some((range, reason)) = imre_ast::comprehension::dict_unpacking_error(parsed) {
        let error =
            crate::frontend::diagnostic_message(source, &range, &format!("SyntaxError: {reason}"));
        return Err(format!("{}:{error}", path.display()));
    }
    crate::analysis::check_scope_syntax(source, parsed.syntax())
        .map_err(|error| format!("{}:{error}", path.display()))?;
    if let Some(keyword) = imre_ast::repeated_keyword(parsed.syntax().into()) {
        let name = keyword.arg.as_ref().expect("repeated named argument");
        let reason = format!("SyntaxError: keyword argument repeated: {name}");
        let error = crate::frontend::diagnostic_message(source, keyword, &reason);
        return Err(format!("{}:{error}", path.display()));
    }
    let replacements = strings::literal_replacements(source, parsed.tokens());
    if keep_docstrings && !replacements.is_empty() {
        strings::check_docs(source, &parsed.syntax().body, &replacements)
            .map_err(|error| format!("{}:{error}", path.display()))?;
    }
    Ok(replacements)
}

pub(super) fn lower_source(
    path: &Path,
    source: &str,
    body: &mut [ast::Stmt],
) -> Result<imre_ast::Names, String> {
    let private_names = imre_ast::lower(body);
    if source.contains("nonlocal") {
        crate::frontend::function_scope::check_nonlocals(source, body)
            .map_err(|error| format!("{}:{error}", path.display()))?;
    }
    if let Some((range, reason)) = imre_ast::annotation_scope_error(body) {
        let error =
            crate::frontend::diagnostic_message(source, &range, &format!("SyntaxError: {reason}"));
        return Err(format!("{}:{error}", path.display()));
    }
    if let Some(param) = &private_names.duplicate_type_parameter {
        let reason = format!("SyntaxError: duplicate type parameter '{}'", param.id);
        let error = crate::frontend::diagnostic_message(source, param, &reason);
        return Err(format!("{}:{error}", path.display()));
    }
    Ok(private_names)
}

/// Validate the executable bodies. Discovery already recorded the malformed modules.
pub(super) fn check(
    (files, builtins): (&[ModuleSource], &[ModuleSource]),
    keep_docstrings: bool,
    module_errors: &HashMap<PathBuf, Box<super::ModuleSyntaxError>>,
) -> Result<(), String> {
    for (_, path, source) in files.iter().chain(builtins) {
        if !module_errors.contains_key(path) {
            check_source(path, source, keep_docstrings)?;
        }
    }
    Ok(())
}

fn check_source(path: &Path, source: &str, keep_docstrings: bool) -> Result<(), String> {
    let source_tree = parse_source(path, source, keep_docstrings)?;
    lower_source(path, source, &mut source_tree.parsed.into_syntax().body)?;
    Ok(())
}

/// Normalized source and lowered names shared by archive, split, and inline placement.
pub(super) struct PreparedBody<'a> {
    name: &'a str,
    body: ast::Suite,
    source: SourceInfo<'a>,
    expression: bool,
    keep_docstrings: bool,
}

impl<'a> PreparedBody<'a> {
    pub(super) fn module(
        name: &'a str,
        path: &'a Path,
        source: &'a str,
        keep_docstrings: bool,
        origin: ModuleOrigin<'a>,
    ) -> Result<Self, String> {
        Self::new(
            name,
            path,
            source,
            keep_docstrings,
            SourceBody::Module(origin),
        )
    }

    fn new(
        name: &'a str,
        path: &'a Path,
        source: &'a str,
        keep_docstrings: bool,
        kind: SourceBody<'a>,
    ) -> Result<Self, String> {
        let bundled = matches!(kind, SourceBody::Module(ModuleOrigin::Bundled));
        let provider = matches!(kind, SourceBody::Module(ModuleOrigin::Provider));
        let expression = matches!(&kind, SourceBody::Expression(_));
        let program_bodies = match kind {
            SourceBody::Module(ModuleOrigin::Program(bodies)) => bodies,
            _ => ProgramBodies::default(),
        };
        // The expression grammar supplies its own checked syntax and tokens.
        // Both inputs below then use the same name lowering and native code entry.
        enum Input<'a> {
            Module(ParsedSource),
            Expression(ast::ModModule, &'a ast::token::Tokens),
        }
        let input = match kind {
            SourceBody::Module(_) => Input::Module(parse_source(path, source, keep_docstrings)?),
            SourceBody::Expression(parsed) => {
                let expression = parsed.syntax();
                let stmt = ast::Stmt::Expr(ast::StmtExpr {
                    node_index: Default::default(),
                    range: expression.body.range(),
                    value: expression.body.clone(),
                });
                Input::Expression(
                    ast::ModModule {
                        node_index: Default::default(),
                        range: expression.range(),
                        body: std::iter::once(stmt).collect(),
                    },
                    parsed.tokens(),
                )
            }
        };
        let (syntax, tokens) = match &input {
            Input::Module(p) => (p.parsed.syntax(), p.parsed.tokens()),
            Input::Expression(syntax, tokens) => (syntax, *tokens),
        };
        let docs = if keep_docstrings && name == "collections" && bundled {
            crate::modules::doctest::sealed(crate::modules::IntrinsicModule::Collections)
        } else {
            None
        };
        let has_templates = tokens
            .iter()
            .any(|t| t.kind() == ast::token::TokenKind::TStringStart);
        let template_expressions = imre_ast::strings::template_expressions(source, tokens, syntax);
        // CPython starts the module provider at the first statement, excluding its decorators.
        let module_start = syntax
            .body
            .first()
            .map(|stmt| {
                let last = match stmt {
                    ast::Stmt::FunctionDef(stmt) => stmt.decorator_list.last(),
                    ast::Stmt::ClassDef(stmt) => stmt.decorator_list.last(),
                    _ => None,
                };
                last.and_then(|decorator| {
                    tokens.after(decorator.end()).iter().find(|token| {
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
        let class_keywords = tokens
            .iter()
            .filter(|token| token.kind() == ast::token::TokenKind::Class)
            .map(Ranged::start)
            .collect();
        let (mut body, replacements) = match input {
            Input::Module(p) => (p.parsed.into_syntax().body, p.replacements),
            Input::Expression(syntax, tokens) => {
                (syntax.body, strings::literal_replacements(source, tokens))
            }
        };
        let method_imports = imre_ast::module_imports(&body);
        let private_names = lower_source(path, source, &mut body)?;
        let values = crate::frontend::RegisteredSource {
            text: source,
            private_names: Some(&private_names),
            replacements: &replacements,
            has_templates,
            method_imports: &method_imports,
        };
        let mut operations = crate::frontend::namespace_operations(&body, &private_names);
        let module_annotations = crate::frontend::module_annotations(
            &body,
            private_names.string_annotations,
            &mut operations,
            values,
            bundled,
        );
        let mut prepare = crate::frontend::SuitePreparation::new(&operations, values);
        let initializer =
            (!expression).then(|| prepare.suite(&body, crate::frontend::RegisteredScope::Module));
        let mut function_bodies = HashMap::new();
        let mut class_bodies = HashMap::new();
        let prepare_classes = !bundled
            || initializer
                .as_ref()
                .is_some_and(|body| body.fully_checked());
        if !expression {
            for (start, site) in definition_sites(&body) {
                match site {
                    DefinitionSite::Function(def)
                        if def.type_params.is_none()
                            && !program_bodies
                                .checked
                                .is_some_and(|sites| sites.contains_key(&start)) =>
                    {
                        let kind = crate::frontend::RegisteredScope::Function {
                            is_async: def.is_async,
                        };
                        // Suspended activations keep their existing whole-body selection.
                        if def.is_async || imre_ast::code_flags::is_generator(&def.body) {
                            if let Some(suite) = prepare.complete(&def.body, kind) {
                                function_bodies.insert(start, suite);
                            }
                        } else {
                            function_bodies.insert(start, prepare.suite(&def.body, kind));
                        }
                    }
                    DefinitionSite::Class(class) if prepare_classes => {
                        if let Some(first) = class.body.first()
                            && let Some(suite) = prepare.class_body(class)
                        {
                            class_bodies.insert(first.start(), suite);
                        }
                    }
                    _ => {}
                }
            }
        }
        let source = SourceInfo {
            scopes: crate::frontend::function_scope::ScopeFacts::prepare(&body, &private_names),
            nested_bodies: prepare.nested,
            namespace_operations: operations,
            module_annotations,
            function_bodies,
            class_bodies,
            initializer,
            program_bodies,
            bundled,
            path: path.into(),
            // These CPython providers implement their own call semantics in Python.
            python_annotations: provider
                || bundled && matches!(name, "_py_warnings" | "tomllib._parser" | "typing"),
            module_start,
            class_keywords,
            docs,
            has_templates,
            template_expressions,
            replacements,
            method_imports,
            barry_as_flufl: imre_ast::future_import(&body, "barry_as_FLUFL"),
            string_annotations: private_names.string_annotations,
            private_names,
            text: source,
        };
        Ok(Self {
            name,
            body,
            source,
            expression,
            keep_docstrings,
        })
    }

    fn emit(&self, output: &mut Output, unit: usize, file: &str) -> Result<String, String> {
        // Frozen code exposes its origin independently of the module's registered source path.
        let code_file = match (self.source.bundled, self.name) {
            (true, "_frozen_importlib") => output.data(b"<frozen importlib._bootstrap>"),
            (true, "_frozen_importlib_external") => {
                output.data(b"<frozen importlib._bootstrap_external>")
            }
            _ => file.to_owned(),
        };
        let location = SourceMap {
            info: &self.source,
            definitions: (!self.expression).then(|| definition_sites(&self.body)),
            unit,
            positions: SourcePositions::new(self.source.text, &code_file),
        };
        let suite = if self.expression {
            let [ast::Stmt::Expr(statement)] = self.body.as_slice() else {
                return Err("expected an annotation expression".into());
            };
            Body::Expression(&statement.value)
        } else {
            Body::Namespace(&self.body)
        };
        let code = output.code(&location, String::new(), suite, false, None);
        match output.diagnostic.take() {
            Some(diagnostic) => Err(diagnostic),
            None => Ok(code),
        }
    }

    /// Emit independent IR with private, address-significant constant slots.
    pub(super) fn standalone(&self) -> Result<Option<(String, String)>, String> {
        let mut output = Output {
            keep_docstrings: self.keep_docstrings,
            ..Output::default()
        };
        let filename = self.source.path.as_os_str().as_encoded_bytes();
        let file = output.data(filename);
        let entry = self.emit(&mut output, 0, &file)?;
        output.native_prelude();
        let mut ir = format!("{ABI}\n{}\n{}", output.constants, output.functions);
        let mut hash = blake3::Hasher::new();
        for field in [
            std::env::consts::ARCH.as_bytes(),
            self.name.as_bytes(),
            filename,
            self.source.text.as_bytes(),
            ir.as_bytes(),
        ] {
            hash.update(&(field.len() as u64).to_le_bytes());
            hash.update(field);
        }
        hash.update(&[u8::from(self.keep_docstrings)]);
        let symbol = format!("imre_provider_{}", hash.finalize().to_hex());
        writeln!(ir, "@{symbol} = alias i64 (ptr, i64), ptr {entry}").unwrap();
        Ok(Some((symbol, ir)))
    }
}

enum SourceBody<'a> {
    Module(ModuleOrigin<'a>),
    Expression(&'a ruff_python_parser::Parsed<ast::ModExpression>),
}

enum DefinitionName<'a> {
    Binding(&'a ast::Identifier),
    Plain(&'a str),
}

enum Body<'a> {
    Namespace(&'a [ast::Stmt]),
    ClassParameters(&'a ast::StmtClassDef),
    Statements(&'a [ast::Stmt]),
    /// A suspended suite and its generator, coroutine, or async generator code flag.
    GeneratorFunction(&'a [ast::Stmt], u32),
    ModuleAnnotations(
        &'a crate::frontend::RegisteredSuite,
        &'a [&'a crate::frontend::DeferredModuleAnnotation],
    ),
    ClassAnnotations(Vec<statements::class_annotations::Site>, BTreeSet<String>),
    Annotations(
        Vec<(&'a str, &'a ast::Expr)>,
        Option<crate::frontend::function_scope::ClassBindings>,
    ),
    CheckedExpression(&'a crate::frontend::RegisteredFunction),
    Expression(&'a ast::Expr),
    TypeAlias(&'a ast::Expr),
    Generator(&'a imre_ast::comprehension::Plan<'a>),
}

impl Body<'_> {
    fn generator_flags(&self) -> u32 {
        use imre_ast::code_flags::{ASYNC_GENERATOR, GENERATOR, yields_in_expression};
        match self {
            Self::GeneratorFunction(_, flags) => *flags,
            Self::Generator(plan) if plan.body_suspends() => ASYNC_GENERATOR,
            Self::Generator(..) => GENERATOR,
            Self::Expression(expr) if yields_in_expression(expr) => GENERATOR,
            _ => 0,
        }
    }
}

enum Entry {
    Compiled,
    Suspended,
}

#[derive(Clone, Copy)]
enum DefinitionSite<'a> {
    Function(&'a ast::StmtFunctionDef),
    Class(&'a ast::StmtClassDef),
}

/// Collect declaration sites for the registered frame. Executing a definition still
/// evaluates its header and binds its name at the source statement.
fn definition_sites(body: &[ast::Stmt]) -> HashMap<ruff_text_size::TextSize, DefinitionSite<'_>> {
    use ast::statement_visitor::{StatementVisitor, walk_stmt};

    #[derive(Default)]
    struct Sites<'a>(HashMap<ruff_text_size::TextSize, DefinitionSite<'a>>);

    impl<'a> StatementVisitor<'a> for Sites<'a> {
        fn visit_stmt(&mut self, stmt: &'a ast::Stmt) {
            match stmt {
                ast::Stmt::FunctionDef(def) => {
                    self.0.insert(def.start(), DefinitionSite::Function(def));
                    if def.type_params.is_none() {
                        self.visit_body(&def.body);
                    }
                }
                ast::Stmt::ClassDef(class) => {
                    self.0.insert(class.start(), DefinitionSite::Class(class));
                }
                _ => walk_stmt(self, stmt),
            }
        }
    }
    let mut sites = Sites::default();
    sites.visit_body(body);
    sites.0
}

struct SourceMap<'a> {
    info: &'a SourceInfo<'a>,
    definitions: Option<HashMap<ruff_text_size::TextSize, DefinitionSite<'a>>>,
    unit: usize,
    positions: SourcePositions<'a>,
}

impl<'a> std::ops::Deref for SourceMap<'a> {
    type Target = SourceInfo<'a>;

    fn deref(&self) -> &Self::Target {
        self.info
    }
}

#[derive(Default)]
struct SourceInfo<'a> {
    scopes: crate::frontend::function_scope::ScopeFacts,
    namespace_operations: HashMap<ruff_text_size::TextRange, crate::ir::Expr>,
    module_annotations:
        HashMap<ruff_text_size::TextSize, crate::frontend::DeferredModuleAnnotation>,
    nested_bodies: HashMap<ruff_text_size::TextSize, crate::frontend::RegisteredSuite>,
    function_bodies: HashMap<ruff_text_size::TextSize, crate::frontend::RegisteredSuite>,
    class_bodies: HashMap<ruff_text_size::TextSize, crate::frontend::RegisteredSuite>,
    initializer: Option<crate::frontend::RegisteredSuite>,
    program_bodies: ProgramBodies<'a>,
    bundled: bool,
    path: std::borrow::Cow<'a, Path>,
    python_annotations: bool,
    module_start: ruff_text_size::TextSize,
    class_keywords: Vec<ruff_text_size::TextSize>,
    docs: Option<crate::modules::doctest::SealedDocs>,
    has_templates: bool,
    template_expressions: HashMap<ruff_text_size::TextSize, String>,
    replacements: HashMap<ruff_text_size::TextRange, Vec<u32>>,
    private_names: imre_ast::Names,
    method_imports: std::collections::BTreeSet<String>,
    string_annotations: bool,
    barry_as_flufl: bool,
    text: &'a str,
}

impl SourceInfo<'_> {
    fn registered_source(&self) -> crate::frontend::RegisteredSource<'_> {
        crate::frontend::RegisteredSource {
            text: self.text,
            replacements: &self.replacements,
            has_templates: self.has_templates,
            method_imports: &self.method_imports,
            private_names: Some(&self.private_names),
        }
    }
}

impl SourceMap<'_> {
    fn function_flags(&self, params: Option<&ast::Parameters>, nested: bool, method: bool) -> u32 {
        use imre_ast::code_flags;
        let mut flags = code_flags::function_flags(params, nested, method);
        if self.string_annotations {
            flags |= code_flags::FUTURE_ANNOTATIONS;
        }
        if self.barry_as_flufl {
            flags |= code_flags::FUTURE_BARRY_AS_BDFL;
        }
        flags
    }

    /// Keep documentation attached to the bundled definition, even when callers rebind its name.
    fn docstring(&self, qualname: &str, body: &[ast::Stmt]) -> Option<String> {
        crate::frontend::docstring(body).or_else(|| {
            self.docs
                .as_ref()?
                .object_docstring(qualname)
                .map(str::to_owned)
        })
    }
}

use crate::ir::Constant;

#[derive(Clone, Copy, Default)]
struct Declarations {
    declared_suspension: bool,
    declared_statement_line: bool,
    declared_definition_position: bool,
    emitted_named_op: bool,
    emitted_guarded_lit: bool,
    emitted_guarded_named_op: bool,
    registered_expression: bool,
    checked_integer_arithmetic: bool,
    retained_native: bool,
}

#[derive(Clone, Default)]
struct Output {
    declarations: Declarations,
    diagnostic: Option<String>,
    keep_docstrings: bool,
    constant_slots: HashMap<(usize, Constant), usize>,
    strings: HashMap<Vec<u8>, u32>,
    interned_identifiers: BTreeSet<String>,
    positions: HashMap<[usize; 4], usize>,
    constants: String,
    functions: String,
    count: u32,
}

impl Output {
    /// Roll back private code sites when an optional annotation expression cannot be compiled.
    fn try_native_body(&mut self, emit: impl FnOnce(&mut Self) -> String) -> Option<String> {
        let code = self.count;
        let functions = self.functions.len();
        let constants = self.constants.len();
        let strings = self.strings.len() as u32;
        let positions = self.positions.len();
        let slots = self.constant_slots.len();
        let declarations = self.declarations;
        let names = std::mem::take(&mut self.interned_identifiers);
        let outer_diagnostic = self.diagnostic.take();
        let site = emit(self);
        let added_names = std::mem::replace(&mut self.interned_identifiers, names);
        if std::mem::replace(&mut self.diagnostic, outer_diagnostic).is_none() {
            self.interned_identifiers.extend(added_names);
            return Some(site);
        }
        self.count = code;
        self.functions.truncate(functions);
        self.constants.truncate(constants);
        self.strings.retain(|_, id| *id < strings);
        self.positions.retain(|_, id| *id < positions);
        self.constant_slots.retain(|_, id| *id < slots);
        self.declarations = declarations;
        None
    }

    /// Distinct Python definitions keep distinct addresses even if LLVM merges their bodies.
    fn definition(
        &mut self,
        source: &SourceMap<'_>,
        prefix: String,
        body: Body<'_>,
        start: ruff_text_size::TextSize,
        in_function: bool,
        entry: Entry,
    ) -> String {
        let site = format!("@code_site{}", self.count);
        let suspended = matches!(entry, Entry::Suspended);
        let tag = if suspended {
            if !self.declarations.declared_suspension {
                self.functions.push_str(suspension::ABI);
                self.declarations.declared_suspension = true;
            }
            permanent::NATIVE_SUSPENDED_BODY
        } else {
            0
        };
        let entry = self.code(source, prefix, body, in_function, Some((start, entry)));
        writeln!(
            self.constants,
            "{site} = private constant %code_site {{ ptr {entry}, ptr {}, i32 {}, i32 {tag} }}",
            source.positions.file,
            source.positions.line(start)
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
        definition: Option<(ruff_text_size::TextSize, Entry)>,
    ) -> String {
        let native_suspension = matches!(definition, Some((_, Entry::Suspended)));
        let start = definition.map(|(start, _)| start);
        let checked = start
            .filter(|_| matches!(body, Body::Statements(_)))
            .and_then(|key| source.program_bodies.checked?.get(&key));
        let frame = RegisteredFrame::new(self, &source.positions, native_suspension);
        let module = matches!(body, Body::Namespace(..)) && prefix.is_empty();
        let prepared = match body {
            Body::Namespace(..) if module => source.initializer.as_ref(),
            Body::Namespace(stmts) => stmts
                .first()
                .and_then(|stmt| source.class_bodies.get(&stmt.start())),
            Body::Statements(_) | Body::GeneratorFunction(..) if checked.is_none() => {
                start.and_then(|key| source.function_bodies.get(&key))
            }
            _ => None,
        };
        let mut f = Function {
            prepare_nested_suites: prepared.is_some_and(|plan| !plan.fully_checked()),
            frame,
            source,
            current_range: ruff_text_size::TextRange::empty(source.module_start),
            async_generator: matches!(
                body,
                Body::GeneratorFunction(_, imre_ast::code_flags::ASYNC_GENERATOR)
            ),
            class_sites: None,
            module_annotation_sites: None,
            annotation_children: HashMap::new(),
            in_function,
            module,
            declared_globals: Default::default(),
            annotation_values: None,
            class_locals: match &body {
                Body::Namespace(_) if !prefix.is_empty() => Some(
                    source.scopes.classes[&start.expect("class definition")]
                        .bindings
                        .clone(),
                ),
                _ => None,
            },
            prefix,
            loops: Vec::new(),
            iteration: Default::default(),
            exits: Vec::new(),
        };
        if checked.is_none()
            && matches!(body, Body::Statements(_))
            && let Some(native) = start.and_then(|key| source.program_bodies.native?.get(&key))
        {
            f.select_native(native);
        }
        match body {
            Body::Namespace(stmts)
            | Body::Statements(stmts)
            | Body::GeneratorFunction(stmts, _) => {
                if module && imre_ast::annotations::has_variable_annotation(stmts) {
                    if source.string_annotations {
                        f.frame
                            .op("setup_module_string_annotations", &[&f.scope.clone()]);
                    } else {
                        // Lower the provider after collecting the body, but run its setup first.
                        f.module_annotation_sites = Some(Vec::new());
                        f.jump("annotations_init");
                        f.label("module_body");
                    }
                }
                if matches!(body, Body::Namespace(..))
                    && f.output.keep_docstrings
                    && let Some(doc) = f.source.docstring(f.prefix.trim_end_matches('.'), stmts)
                {
                    let value = f.text(&doc);
                    f.bind("__doc__", &value);
                }
                let globals = if module {
                    &source.scopes.module_globals
                } else if matches!(body, Body::Namespace(..)) {
                    &source.scopes.classes[&start.expect("class definition")]
                        .bindings
                        .globals
                } else {
                    source.scopes.functions[&start.expect("function definition")].globals()
                };
                f.declare_globals(globals);
                if let Some(checked) = checked {
                    f.checked_body(checked);
                } else if let Some(prepared) = prepared {
                    f.prepared_function(prepared, |code, run| code.source_statements(&stmts[run]));
                } else if f.class_locals.is_some() && !f.source.string_annotations {
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
            Body::ClassParameters(class) => f.parameterized_class(class),
            Body::ClassAnnotations(sites, globals) => f.class_annotation_values(&sites, &globals),
            Body::Annotations(entries, class) => f.annotations(&entries, class),
            Body::ModuleAnnotations(plan, entries) => {
                for entry in entries {
                    if let crate::frontend::AnnotationValue::Bound(child) = &entry.value
                        && !f.annotation_children.contains_key(&child.id)
                    {
                        let file = f.output.data(b"<string>");
                        let info = SourceInfo {
                            path: Path::new("<string>").into(),
                            text: &child.text,
                            has_templates: child.has_templates,
                            ..Default::default()
                        };
                        let source = SourceMap {
                            info: &info,
                            definitions: None,
                            unit: source.unit,
                            positions: SourcePositions::new(&child.text, &file),
                        };
                        let code = f.output.code(
                            &source,
                            String::new(),
                            Body::CheckedExpression(&child.body),
                            false,
                            None,
                        );
                        f.annotation_children.insert(child.id, code);
                    }
                }
                f.prepared_function(plan, |code, run| {
                    for index in run {
                        code.unprepared_annotation(index, entries[index]);
                    }
                });
            }
            Body::CheckedExpression(checked) => f.checked_body(checked),
            Body::Generator(plan) => f.generator_step(plan),
            Body::Expression(expr) | Body::TypeAlias(expr) => {
                f.checkpoint(expr);
                if matches!(body, Body::TypeAlias(_)) {
                    let format = f.lookup(".format");
                    f.op("annotation_format", &[&format]);
                }
                let value = f.expr(expr);
                f.return_value(&value);
            }
        }
        f.frame.finish(module)
    }
}

/// Pass fixed operands in native storage or pack expanded operands in Python containers.
enum CallArguments {
    FixedKeywords {
        array: &'static str,
        positional: usize,
        names: String,
        keywords: usize,
    },
    Packed {
        args: String,
        kwargs: String,
    },
    Positional {
        array: &'static str,
        count: usize,
        receiver_slot: bool,
    },
    PartitionField {
        array: &'static str,
        index: u32,
        get_position: String,
    },
}

enum IndexKey<'a, E> {
    Object(&'a E),
    Bounds(
        Option<&'a E>,
        Option<&'a E>,
        Option<&'a crate::ir::Constant>,
    ),
    Prepared(&'a crate::ir::Expr),
}

struct Function<'a, 'p> {
    prepare_nested_suites: bool,
    frame: RegisteredFrame<'a, 'p>,
    source: &'p SourceMap<'p>,
    current_range: ruff_text_size::TextRange,
    prefix: String,
    declared_globals: std::collections::BTreeSet<String>,
    in_function: bool,
    module: bool,
    module_annotation_sites: Option<Vec<ruff_text_size::TextSize>>,
    annotation_children: HashMap<crate::ir::AnnotationCodeId, String>,
    class_sites: Option<statements::class_annotations::ClassSites>,
    class_locals: Option<crate::frontend::function_scope::ClassBindings>,
    annotation_values: Option<String>,
    loops: Vec<LoopTarget>,
    iteration: checked::IterationState,
    exits: Vec<(Cleanup, String)>,
    async_generator: bool,
}

/// The syntax walker adds Python scopes and cleanup to the registered frame operations.
impl<'a, 'p> std::ops::Deref for Function<'a, 'p> {
    type Target = RegisteredFrame<'a, 'p>;
    fn deref(&self) -> &Self::Target {
        &self.frame
    }
}

impl std::ops::DerefMut for Function<'_, '_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.frame
    }
}

#[derive(Clone)]
struct ContextExit {
    range: ruff_text_size::TextRange,
    is_async: bool,
    callable: String,
    caught: String,
}

#[derive(Clone)]
enum Cleanup {
    With(ContextExit),
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
    fn checkpoint(&mut self, node: &impl Ranged) {
        self.current_range = node.range();
        self.frame.checkpoint(self.current_range);
    }

    /// An annotation's text is still a normal str. Retain its native expression
    /// separately so a later evaluation can supply a new globals/locals pair.
    fn annotation_text(&mut self, value: &str) -> String {
        if !self.source.bundled
            && let Some(parsed) = crate::frontend::annotation_expression(value)
        {
            return self.preparsed_annotation_text(value, &parsed);
        }
        self.text(value)
    }

    fn preparsed_annotation_text(
        &mut self,
        value: &str,
        parsed: &ruff_python_parser::Parsed<ast::ModExpression>,
    ) -> String {
        let unit = self.source.unit;
        let code = self.output.try_native_body(|output| {
            let file = output.data(b"<string>");
            match PreparedBody::new(
                "",
                Path::new("<string>"),
                value,
                output.keep_docstrings,
                SourceBody::Expression(parsed),
            )
            .and_then(|body| body.emit(output, unit, &file))
            {
                Ok(code) => code,
                Err(err) => {
                    output.diagnostic = Some(err);
                    String::new()
                }
            }
        });
        if let Some(code) = code {
            let text = self.text(value);
            self.bind_annotation_expression(&text, &code);
            return text;
        }
        self.text(value)
    }

    fn bind_annotation_expression(&mut self, text: &str, code: &str) {
        if !self.output.declarations.registered_expression {
            self.output
                .functions
                .push_str("declare void @imre_dyn_bind_expression(ptr, i64, ptr)\n");
            self.output.declarations.registered_expression = true;
        }
        self.line(&format!(
            "call void @imre_dyn_bind_expression(ptr %ctx, i64 {text}, ptr {code})"
        ));
    }

    fn text(&mut self, value: &str) -> String {
        if !value.is_empty() && strings::interned_spelling(value) {
            self.output.interned_identifiers.insert(value.to_owned());
        }
        self.lit(1, value.as_bytes())
    }

    fn lookup(&mut self, name: &str) -> String {
        let op = match &self.class_locals {
            Some(class) if self.annotation_values.is_some() && class.globals.contains(name) => {
                "lookup_annotation_global"
            }
            Some(class) if self.annotation_values.is_some() && class.locals.contains(name) => {
                "lookup_class_annotation_global"
            }
            Some(_) if self.annotation_values.is_some() => "lookup_class_annotation",
            Some(class) if class.locals.contains(name) => "lookup_class_local",
            _ => "lookup",
        };
        self.frame.named_op(op, &self.scope.clone(), name, "0")
    }

    fn bind(&mut self, name: &str, value: &str) {
        self.frame
            .named_op("assign", &self.scope.clone(), name, value);
    }

    fn attr(&mut self, object: &str, name: &str) -> String {
        self.named_op("attr", object, name, "0")
    }

    fn select(
        &mut self,
        branch: impl FnOnce(&mut Self, &str, &str),
        on_true: impl FnOnce(&mut Self) -> CheckedValue,
        on_false: impl FnOnce(&mut Self) -> CheckedValue,
    ) -> CheckedValue {
        ValueJoin::select(
            self,
            |code| {
                code.flush_check();
                &mut code.body
            },
            branch,
            on_true,
            on_false,
        )
        .expect("value-producing registered conditional")
    }

    fn unsupported(&mut self, what: &str) -> String {
        if self.native_suspension {
            let (line, col) = self
                .source
                .positions
                .line_and_column(self.current_range.start());
            let (end_line, end_col) = self
                .source
                .positions
                .line_and_column(self.current_range.end());
            self.frame.output.diagnostic.get_or_insert_with(|| format!(
                "{}:{line}:{}: unsupported {what} in a compiled suspended function (through {end_line}:{})",
                self.source.path.display(), col + 1, end_col + 1
            ));
            return "0".into();
        }
        let what = self.text(what);
        self.op("unsupported", &[&what])
    }

    fn expression_span(
        &self,
        expr: &ast::Expr,
        prefix: &ast::Expr,
        opener: u8,
    ) -> Option<[usize; 4]> {
        self.expression_range_span(expr.range(), prefix.end(), opener)
    }

    fn invoke(&mut self, callable: &str, values: &[&str]) -> String {
        let mark = self.temporary_mark();
        let (ptr, count) = self.handle_array(values);
        self.call_at(
            callable,
            CallArguments::Positional {
                array: ptr,
                count,
                receiver_slot: false,
            },
            &mark,
            None,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exception_group_handlers_in_nested_generators_have_native_entries() {
        let source = "def supported():\n    yield 1\ndef handled():\n    def nested():\n        yield 2\n    try:\n        yield 3\n    except* ValueError:\n        pass\ndef outer():\n    def inner():\n        try:\n            yield 4\n        except* ValueError:\n            pass\n    yield 5\n";
        let (_, ir) = PreparedBody::module(
            "sample",
            Path::new("sample.py"),
            source,
            true,
            ModuleOrigin::Program(ProgramBodies::default()),
        )
        .unwrap()
        .standalone()
        .unwrap()
        .unwrap();
        let tag = format!(", i32 {} }}", permanent::NATIVE_SUSPENDED_BODY);
        assert_eq!(ir.lines().filter(|line| line.ends_with(&tag)).count(), 5);
        assert_eq!(
            ir.matches("call void @imre_dyn_suspend_operands(").count(),
            5
        );
        assert!(!ir.contains("except* ValueError"));
    }

    #[test]
    fn suspended_definitions_use_native_entries_in_each_module_origin() {
        let source = "def items(xs):\n    yield 1\n    yield from xs\npark = lambda: (yield 2)\nresume = lambda: 1 + (yield 3)\nasync def more(xs):\n    async for x in xs:\n        yield x\nasync def consume(f):\n    return await f()\ndef defaults():\n    yield 0\n    def nested(first=1, second=(yield 4)):\n        return second\n    return nested\n";
        for bundled in [false, true] {
            let prepared = PreparedBody::module(
                "example",
                Path::new("example.py"),
                source,
                true,
                if bundled {
                    ModuleOrigin::Bundled
                } else {
                    ModuleOrigin::Program(ProgramBodies::default())
                },
            )
            .unwrap();
            let selected: Vec<_> = prepared
                .body
                .iter()
                .filter_map(|stmt| {
                    let def = stmt.as_function_def_stmt()?;
                    prepared
                        .source
                        .function_bodies
                        .contains_key(&def.start())
                        .then_some(def.name.as_str())
                })
                .collect();
            assert_eq!(selected, ["items", "consume"]);
            let (_, ir) = prepared.standalone().unwrap().unwrap();
            let tag = format!(", i32 {} }}", permanent::NATIVE_SUSPENDED_BODY);
            assert_eq!(ir.lines().filter(|line| line.ends_with(&tag)).count(), 6);
            let operations: Vec<_> = ir
                .lines()
                .filter(|line| line.contains("call void @imre_dyn_suspend_operands("))
                .map(|line| line.rsplit_once(", i32 ").unwrap().1)
                .collect();
            assert_eq!(
                operations,
                ["0)", "3)", "0)", "0)", "2)", "1)", "2)", "0)", "0)"]
            );
        }
    }

    #[test]
    fn pass_fixed_keywords_directly_and_keep_expanded_calls_packed() {
        let source = "target(1, name=2)\ntarget(1, **options)\n";
        let (_, ir) = PreparedBody::module(
            "example",
            Path::new("example.py"),
            source,
            true,
            ModuleOrigin::Bundled,
        )
        .unwrap()
        .standalone()
        .unwrap()
        .unwrap();
        assert_eq!(ir.matches("call i64 @imre_dyn_keyword_call_at(").count(), 1);
        assert_eq!(ir.matches("call i64 @imre_dyn_scoped_call_at(").count(), 1);
    }

    #[test]
    fn whole_ir_and_split_link_share_the_same_module_selection() {
        use crate::BoxedLayout;
        let root = Path::new("<imre stdlib>");
        let modules: Vec<ModuleSource> = vec![
            (
                "driver".into(),
                PathBuf::from("driver.py"),
                "import helper\n".into(),
            ),
            (
                "helper".into(),
                PathBuf::from("helper.py"),
                "from . import extra\n".into(),
            ),
            (
                "broken".into(),
                PathBuf::from("broken.py"),
                "invalid (".into(),
            ),
            (
                "archived".into(),
                root.join("archived.py"),
                "value = 1\n".into(),
            ),
        ];
        let (name, path, source) = &modules[3];
        let (symbol, _) = PreparedBody::module(name, path, source, true, ModuleOrigin::Bundled)
            .unwrap()
            .standalone()
            .unwrap()
            .unwrap();
        let providers = HashSet::from([symbol]);
        let registrations = RegisteredBodies {
            archive_members: HashMap::from([(modules[1].1.clone(), ([23; 32], 16))]),
            ..Default::default()
        };
        let errors = HashMap::from([(
            modules[2].1.clone(),
            Box::new(super::super::ModuleSyntaxError::new(
                &modules[2].2,
                ruff_text_size::TextRange::default(),
                "invalid".into(),
                false,
            )),
        )]);
        let build = |layout| {
            emit(
                (&modules, &[]),
                "{}",
                true,
                root,
                &providers,
                layout,
                (&errors, &registrations),
            )
            .unwrap()
        };
        let (whole, whole_packed) = build(BoxedLayout::Whole);
        let (split, split_packed) = build(BoxedLayout::Split);
        let mut native = Vec::new();
        let (combined, packed) = build(BoxedLayout::WholeWithUnits(&mut native));
        assert!(packed && whole_packed && split_packed);
        assert_eq!(combined.len(), 1);
        assert_eq!(whole[0].llvm_ir, combined[0].llvm_ir);
        assert_eq!(native.len(), split.len());
        assert!(native.len() > 1);
        for (actual, expected) in native.iter().zip(&split) {
            assert_eq!(actual.name, expected.name);
            assert_eq!(actual.llvm_ir, expected.llvm_ir);
        }
    }

    #[test]
    fn declare_exit_line_only_for_compiled_with() {
        let path = Path::new("<imre stdlib>/example.py");
        let declare = "declare void @imre_dyn_set_statement_line";
        for (source, expected) in [("value = 1", 0), ("with a, b:\n    pass", 1)] {
            let (_, ir) =
                PreparedBody::module("example", path, source, true, ModuleOrigin::Bundled)
                    .unwrap()
                    .standalone()
                    .unwrap()
                    .unwrap();
            assert_eq!(ir.matches(declare).count(), expected);
        }
    }

    #[test]
    fn provider_substitution() {
        let root = Path::new("<imre stdlib>");
        let path = root.join("example.py");
        let source = "value = t'{123}'";
        let compiled = |name, path: &Path, src, docs| {
            PreparedBody::module(name, path, src, docs, ModuleOrigin::Bundled)
                .unwrap()
                .standalone()
                .unwrap()
                .unwrap()
        };
        let (symbol, ir) = compiled("example", &path, source, true);
        let slot = ir
            .lines()
            .find_map(|line| line.strip_suffix(" = private constant i8 0"))
            .expect("a source-local address for the template strings");
        assert!(ir.contains(&format!(
            "call i64 @imre_dyn_cached_handle(ptr %ctx, ptr {slot})"
        )));
        assert_ne!(symbol, compiled("other", &path, source, true).0);
        assert_ne!(
            symbol,
            compiled("example", &root.join("other.py"), source, true).0
        );
        assert_ne!(symbol, compiled("example", &path, "value = 2", true).0);
        assert_ne!(symbol, compiled("example", &path, source, false).0);

        let bundled = ("example".into(), path, source.into());
        let project = (
            "example".into(),
            PathBuf::from("/project/example.py"),
            source.into(),
        );
        let symbols = HashSet::from([symbol.clone()]);
        let registrations = RegisteredBodies {
            physical_files: HashMap::from([(project.1.clone(), Default::default())]),
            ..Default::default()
        };
        let initializers = (&HashMap::new(), &registrations);
        let refs = |modules: &[_]| {
            let (units, used) = emit(
                (modules, &[]),
                "{}",
                true,
                root,
                &symbols,
                crate::BoxedLayout::Whole,
                initializers,
            )
            .unwrap();
            let ir = &units[0].llvm_ir;
            assert!(used);
            let declared = format!("declare i64 @{symbol}(ptr, i64)");
            assert_eq!(ir.lines().filter(|line| *line == declared).count(), 1);
            ir.lines()
                .filter(|line| {
                    line.contains("call void @imre_dyn_register_source(")
                        || line.contains("call void @imre_dyn_register_file_code(")
                })
                .map(|line| line.ends_with(&format!("@{symbol})")))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            refs(&[bundled.clone(), project.clone(), bundled.clone()]),
            [true, false, true]
        );
        let changed = (
            bundled.0.clone(),
            bundled.1.clone(),
            "value = t'{456}'".into(),
        );
        for missing in [&project, &changed] {
            assert!(
                !emit(
                    (std::slice::from_ref(missing), &[]),
                    "{}",
                    true,
                    root,
                    &symbols,
                    crate::BoxedLayout::Whole,
                    initializers
                )
                .unwrap()
                .1
            );
        }
        assert_eq!(refs(&[project.clone(), bundled.clone()]), [false, true]);

        let (units, used) = emit(
            (&[bundled.clone(), project.clone()], &[]),
            "{}",
            true,
            root,
            &symbols,
            crate::BoxedLayout::Whole,
            (
                &HashMap::from([(
                    bundled.1.clone(),
                    Box::new(super::super::ModuleSyntaxError::new(
                        source,
                        ruff_text_size::TextRange::default(),
                        "invalid".into(),
                        false,
                    )),
                )]),
                &registrations,
            ),
        )
        .unwrap();
        assert!(!used);
        let ir = &units[0].llvm_ir;
        assert_eq!(
            ir.matches("call void @imre_dyn_register_module_error(")
                .count(),
            1
        );
        assert_eq!(
            ir.matches("call void @imre_dyn_register_file_code(")
                .count(),
            1
        );
        assert!(!ir.contains(&symbol));

        let shadow = (
            project.0.clone(),
            PathBuf::from("/other/example.py"),
            project.2.clone(),
        );
        let registrations = RegisteredBodies {
            physical_files: HashMap::from([
                (project.1.clone(), Default::default()),
                (shadow.1.clone(), Default::default()),
            ]),
            ..Default::default()
        };
        let (units, used) = emit(
            (&[bundled, project.clone(), shadow, project], &[]),
            "{}",
            true,
            root,
            &symbols,
            crate::BoxedLayout::Split,
            (&HashMap::new(), &registrations),
        )
        .unwrap();
        assert!(used);
        assert_eq!(units.len(), 3);
        assert_eq!(units[1].name, units[2].name);
        assert_ne!(units[1].llvm_ir, units[2].llvm_ir);
    }
}
