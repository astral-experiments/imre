//! Emit native functions and control flow for closed modules using boxed Python values.

use std::borrow::Cow;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use ruff_python_ast as ast;
use ruff_text_size::Ranged;

use crate::CodegenUnit;
use crate::frontend::RegisteredLambda;
use crate::frontend::module_source::{self, ParsedModule};
use crate::llvm::registered::{
    ABI, FrameEmitter, Output, RegisteredFrame, SourcePositions, permanent, sites, suspension,
};

mod comprehensions;
mod expressions;
mod statements;

type ModuleSource = (String, PathBuf, String);

#[derive(Clone, Copy, Default)]
pub(super) struct PhysicalFile {
    pub(super) digest: Option<[u8; 32]>,
    pub(super) future_flags: u32,
}

/// Snapshots and admission facts for the bodies selected by import resolution.
#[derive(Default)]
pub(super) struct RegisteredBodies<'a> {
    pub(super) program_bodies: HashMap<usize, &'a super::CheckedFunctions>,
    // Decoded installed files retain the fingerprint of their original loader bytes.
    pub(super) physical_files: HashMap<PathBuf, PhysicalFile>,
    pub(super) archive_members: HashMap<PathBuf, ([u8; 32], u32)>,
    // Each source slot has its own ownership, even if two names resolve to one file.
    // Providers contribute declarations to the checked program. Their Python bodies
    // retain Python argument binding, including annotations available to reflection.
    pub(super) python_providers: HashSet<usize>,
}

#[derive(Clone, Copy)]
pub(super) enum ModuleOrigin<'a> {
    Program(Option<&'a super::CheckedFunctions>),
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
        self.output.integer_prelude();
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
    (files, builtins, mut entry_source): (
        &[ModuleSource],
        &[ModuleSource],
        Option<ParsedModule<'_>>,
    ),
    config: &str,
    python_optimization: i32,
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
        python_optimization < 2,
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
        let discovered = entry_source.take();
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
            ModuleOrigin::Program(program_bodies.get(&unit).copied())
        };
        // Preserve the loader's digest while compiling its normalized source.
        let physical = physical_files.get(path).filter(|_| !builtin);
        let file_digest = physical_files.get(path).and_then(|file| file.digest);
        let module = if let Some(entry) = discovered {
            entry
        } else {
            let text = if physical.is_some() && file_digest.is_none() {
                crate::normalize_source(source, crate::SourceKind::File)?
            } else if file_digest.is_some() || zip.is_some() {
                crate::normalize_source(source, crate::SourceKind::DecodedFile)?
            } else {
                std::borrow::Cow::Borrowed(source.as_str())
            };
            ParsedModule::parse(path, text)?
        };
        let initializer = PreparedBody::prepare(
            name,
            path,
            module.text,
            python_optimization,
            origin,
            Input::Module(module.parsed),
        )?;
        let registration = initializer.registration(source, builtin, physical, zip);
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
            let module = output.module_registration(&registration);
            let code = if let Some(symbol) = symbol {
                if declarations.insert(symbol.to_owned()) {
                    writeln!(output.functions, "declare i64 @{symbol}(ptr, i64)").unwrap();
                }
                format!("@{symbol}")
            } else {
                initializer.emit(output, unit, &module.file)?
            };
            module.write_call(output, main, &code);
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

pub(super) fn check_parsed_source(
    path: &Path,
    source: &str,
    parsed: &ruff_python_parser::Parsed<ast::ModModule>,
    keep_docstrings: bool,
) -> Result<HashMap<ruff_text_size::TextRange, Vec<u32>>, String> {
    module_source::validate_syntax(path, source, parsed)?;
    crate::frontend::module_literal_replacements(source, parsed, keep_docstrings)
        .map_err(|error| format!("{}:{error}", path.display()))
}

/// Validate the executable bodies. Discovery already recorded the malformed modules.
pub(super) fn check(
    (files, builtins, mut entry_source): (
        &[ModuleSource],
        &[ModuleSource],
        Option<ParsedModule<'_>>,
    ),
    keep_docstrings: bool,
    module_errors: &HashMap<PathBuf, Box<super::ModuleSyntaxError>>,
) -> Result<(), String> {
    for (_, path, source) in files.iter().chain(builtins) {
        let discovered = entry_source.take();
        if module_errors.contains_key(path) {
            continue;
        }
        // This check reports offsets in the registration's original text. A CRLF file
        // has different physical offsets from its normalized executable body.
        let module = match discovered.filter(|entry| entry.text == source.as_str()) {
            Some(entry) => entry,
            None => ParsedModule::parse(path, source.as_str().into())?,
        };
        check_parsed_source(path, &module.text, &module.parsed, keep_docstrings)?;
        module_source::lower(path, &module.text, &mut module.parsed.into_syntax().body)?;
    }
    Ok(())
}

/// Normalized source and lowered names shared by archive, split, and inline placement.
pub(super) struct PreparedBody<'a> {
    name: &'a str,
    body: ast::Suite,
    source: SourceInfo<'a>,
}

impl<'a> PreparedBody<'a> {
    fn registration(
        &self,
        original_source: &'a str,
        builtin: bool,
        physical: Option<&PhysicalFile>,
        zip: Option<&([u8; 32], u32)>,
    ) -> crate::ir::ModuleRegistration<'a> {
        use crate::ir::{ModuleLoader, ModuleRegistration, PhysicalModuleKind};
        use sha2::{Digest, Sha256};

        let loader = if let Some(&(digest, future_flags)) = zip {
            ModuleLoader::Physical {
                kind: PhysicalModuleKind::Zip,
                digest,
                future_flags,
            }
        } else if let Some(file) = physical {
            // A file loader fingerprints the original bytes even when the body was normalized.
            ModuleLoader::Physical {
                kind: PhysicalModuleKind::File,
                digest: file
                    .digest
                    .unwrap_or_else(|| Sha256::digest(original_source.as_bytes()).into()),
                future_flags: file.future_flags,
            }
        } else {
            ModuleLoader::Source {
                builtin,
                text: self.source.bundled.then_some(original_source.as_bytes()),
            }
        };
        ModuleRegistration {
            name: self.name,
            file: self.source.path,
            loader,
        }
    }

    pub(super) fn module(
        name: &'a str,
        path: &'a Path,
        source: &'a str,
        python_optimization: i32,
        origin: ModuleOrigin<'a>,
    ) -> Result<Self, String> {
        Self::prepare(
            name,
            path,
            Cow::Borrowed(source),
            python_optimization,
            origin,
            Input::Module(ParsedModule::parse(path, source.into())?.parsed),
        )
    }

    fn prepare(
        name: &'a str,
        path: &'a Path,
        text: Cow<'a, str>,
        python_optimization: i32,
        origin: ModuleOrigin<'a>,
        input: Input<'_>,
    ) -> Result<Self, String> {
        let keep_docstrings = python_optimization < 2;
        let source = text.as_ref();
        let bundled = matches!(origin, ModuleOrigin::Bundled);
        let provider = matches!(origin, ModuleOrigin::Provider);
        let expression = matches!(&input, Input::Expression(..));
        let program_bodies = match origin {
            ModuleOrigin::Program(bodies) => bodies,
            _ => None,
        };
        let (syntax, tokens, replacements) = match &input {
            Input::Module(parsed) => (
                parsed.syntax(),
                parsed.tokens(),
                check_parsed_source(path, source, parsed, keep_docstrings)?,
            ),
            Input::Expression(syntax, tokens) => (
                syntax,
                *tokens,
                crate::frontend::literal_replacements(source, tokens),
            ),
        };
        let docs = if keep_docstrings && name == "collections" && bundled {
            crate::modules::doctest::sealed(crate::modules::IntrinsicModule::Collections)
        } else {
            None
        };
        let has_templates = tokens
            .iter()
            .any(|t| t.kind() == ast::token::TokenKind::TStringStart);
        let has_interpolation = has_templates
            || tokens
                .iter()
                .any(|token| token.kind() == ast::token::TokenKind::FStringStart);
        let template_fields = imre_ast::strings::template_fields(source, tokens, syntax);
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
        let mut body = match input {
            Input::Module(parsed) => parsed.into_syntax().body,
            Input::Expression(syntax, _) => syntax.body,
        };
        let method_imports = imre_ast::module_imports(&body);
        let private_names = module_source::lower(path, source, &mut body)?;
        let values = crate::frontend::RegisteredSource {
            text: source,
            private_names: Some(&private_names),
            replacements: &replacements,
            has_templates,
            method_imports: &method_imports,
            templates: None,
        };
        let strings = if has_interpolation {
            crate::frontend::prepare_strings(&body, values, &template_fields)
        } else {
            Default::default()
        };
        let values = crate::frontend::RegisteredSource {
            templates: Some(&strings.templates),
            ..values
        };
        let sites = crate::frontend::registered_sites(&body, &private_names);
        let mut operations = sites.operations;
        let mut type_aliases = HashMap::new();
        let mut logicals = crate::frontend::LogicalSites::default();
        let mut scopes = crate::frontend::function_scope::ScopeFacts::prepare(
            &body,
            &private_names,
            |alias| {
                let Some(name) = alias.name.as_name_expr() else {
                    return;
                };
                let body = crate::frontend::type_alias_body(alias, values);
                let parameters = alias.type_params.as_deref().map(|params| {
                    crate::frontend::registered_type_parameters(params, &private_names)
                });
                if body.is_some() {
                    operations.insert(
                        alias.range(),
                        crate::frontend::registered_type_alias(alias.start()),
                    );
                }
                type_aliases.insert(
                    alias.start(),
                    TypeAliasDefinition {
                        range: alias.range(),
                        name: name.id.to_string(),
                        body,
                        parameters,
                    },
                );
            },
            |expr| logicals.visit(expr, values),
        );
        let lambdas = RegisteredLambda::collect(&body, values, std::mem::take(&mut scopes.lambdas));
        let mut namespace_annotations = HashMap::new();
        let annotation_operands = crate::frontend::annotation_operands(
            &body,
            private_names.string_annotations,
            values,
            bundled,
            |text, parsed| Self::prepare_annotation(text, parsed, python_optimization).ok(),
            &mut namespace_annotations,
        );
        let module_annotations = crate::frontend::module_annotations(
            &body,
            private_names.string_annotations,
            &mut operations,
            &annotation_operands,
        );
        let mut prepare = crate::frontend::SuitePreparation::new(
            &operations,
            values,
            &body,
            python_optimization == 0,
        );
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
                            && !program_bodies.is_some_and(|sites| sites.contains_key(&start)) =>
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
        // Physical frames without a prepared suite can still emit a complete deletion.
        // Its operands resolve names against the active Python frame.
        let checked_deletions = sites
            .deletions
            .into_iter()
            .filter_map(|stmt| {
                let suite = prepare.complete(
                    std::slice::from_ref(stmt),
                    crate::frontend::RegisteredScope::Module,
                )?;
                Some((stmt.range(), suite.checked))
            })
            .collect();
        let mut thread_entries = HashMap::new();
        for (def, entry) in sites.workers {
            let start = def.start();
            thread_entries.insert(start, entry);
            if !program_bodies.is_some_and(|bodies| bodies.contains_key(&start)) {
                // A method or a target nested inside a class may have no prepared body yet.
                function_bodies.entry(start).or_insert_with(|| {
                    prepare.suite(
                        &def.body,
                        crate::frontend::RegisteredScope::Function { is_async: false },
                    )
                });
            }
        }
        let source = SourceInfo {
            annotation_operands,
            namespace_annotations,
            scopes,
            lambdas,
            thread_entries,
            checked_deletions,
            logicals,
            statement_sites: prepare.sites,
            python_optimization,
            nested_bodies: prepare.nested,
            namespace_operations: operations,
            type_aliases,
            module_annotations,
            function_bodies,
            class_bodies,
            initializer,
            program_bodies,
            bundled,
            path,
            // These CPython providers implement their own call semantics in Python.
            python_annotations: provider
                || bundled && matches!(name, "_py_warnings" | "tomllib._parser" | "typing"),
            module_start,
            class_keywords,
            docs,
            has_templates,
            strings,
            template_fields,
            replacements,
            method_imports,
            barry_as_flufl: imre_ast::future_import(&body, "barry_as_FLUFL"),
            string_annotations: private_names.string_annotations,
            private_names,
            text,
        };
        Ok(Self { name, body, source })
    }

    /// Prepare the decoded expression and its lexical definitions in their own source unit.
    fn prepare_annotation(
        text: &str,
        parsed: &ruff_python_parser::Parsed<ast::ModExpression>,
        python_optimization: i32,
    ) -> Result<crate::frontend::AnnotationBody<Box<SourceInfo<'static>>>, String> {
        let expression = parsed.syntax();
        let stmt = ast::Stmt::Expr(ast::StmtExpr {
            node_index: Default::default(),
            range: expression.body.range(),
            value: expression.body.clone(),
        });
        let syntax = ast::ModModule {
            node_index: Default::default(),
            range: expression.range(),
            body: std::iter::once(stmt).collect(),
        };
        let mut prepared = PreparedBody::prepare(
            "",
            Path::new("<string>"),
            Cow::Owned(text.to_owned()),
            python_optimization,
            ModuleOrigin::Program(None),
            Input::Expression(syntax, parsed.tokens()),
        )?;
        let Some(ast::Stmt::Expr(statement)) = prepared.body.pop() else {
            unreachable!("single decoded expression")
        };
        let code = crate::frontend::AnnotationCode::prepare(
            statement.value,
            prepared.source.registered_source(),
        );
        Ok(crate::frontend::AnnotationBody {
            code,
            scope: Box::new(prepared.source),
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
            lambda_code: Default::default(),
            definitions: Some(definition_sites(&self.body)),
            unit,
            positions: SourcePositions::new(&self.source.text, &code_file),
        };
        let code = location.code(
            output,
            String::new(),
            Body::Namespace(&self.body),
            false,
            None,
        );
        match output.diagnostic.take() {
            Some(diagnostic) => Err(diagnostic),
            None => Ok(code),
        }
    }

    /// Emit independent IR with private, address-significant constant slots.
    pub(super) fn standalone(&self) -> Result<Option<(String, String)>, String> {
        let mut output = Output {
            keep_docstrings: self.source.python_optimization < 2,
            ..Output::default()
        };
        let filename = self.source.path.as_os_str().as_encoded_bytes();
        let file = output.data(filename);
        let entry = self.emit(&mut output, 0, &file)?;
        output.integer_prelude();
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
        hash.update(&[u8::from(self.source.python_optimization < 2)]);
        let symbol = format!("imre_provider_{}", hash.finalize().to_hex());
        writeln!(ir, "@{symbol} = alias i64 (ptr, i64), ptr {entry}").unwrap();
        Ok(Some((symbol, ir)))
    }
}

enum Input<'a> {
    Module(ruff_python_parser::Parsed<ast::ModModule>),
    Expression(ast::ModModule, &'a ast::token::Tokens),
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
    AliasEvaluator(AliasValue<'a>),
    Generator(&'a imre_ast::comprehension::Plan<'a>),
}

impl Body<'_> {
    fn generator_flags(&self) -> u32 {
        use imre_ast::code_flags::{ASYNC_GENERATOR, GENERATOR};
        match self {
            Self::GeneratorFunction(_, flags) => *flags,
            Self::Generator(plan) if plan.body_suspends() => ASYNC_GENERATOR,
            Self::Generator(..) => GENERATOR,
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
                    self.visit_body(&class.body);
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
    /// A duplicated cleanup path still evaluates the same lexical definition.
    lambda_code: std::cell::RefCell<HashMap<ruff_text_size::TextSize, String>>,
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

enum AliasValue<'a> {
    Checked(&'a crate::frontend::RegisteredFunction),
    Residual(&'a ast::Expr),
}

struct TypeAliasDefinition {
    range: ruff_text_size::TextRange,
    name: String,
    body: Option<crate::frontend::RegisteredFunction>,
    parameters: Option<Vec<crate::ir::RegisteredTypeParameter>>,
}

struct SourceInfo<'a> {
    annotation_operands: HashMap<
        ruff_text_size::TextSize,
        crate::frontend::PreparedAnnotation<Box<SourceInfo<'static>>>,
    >,
    namespace_annotations: HashMap<Option<ruff_text_size::TextSize>, Vec<ruff_text_size::TextSize>>,
    logicals: crate::frontend::LogicalSites,
    scopes: crate::frontend::function_scope::ScopeFacts,
    thread_entries: HashMap<ruff_text_size::TextSize, imre_ast::ThreadEntry>,
    lambdas: HashMap<ruff_text_size::TextSize, RegisteredLambda>,
    type_aliases: HashMap<ruff_text_size::TextSize, TypeAliasDefinition>,
    python_optimization: i32,
    statement_sites: crate::frontend::StatementSites,
    checked_deletions: HashMap<ruff_text_size::TextRange, crate::frontend::RegisteredFunction>,
    namespace_operations: HashMap<ruff_text_size::TextRange, crate::ir::Expr>,
    module_annotations:
        HashMap<ruff_text_size::TextSize, crate::frontend::DeferredModuleAnnotation>,
    nested_bodies: HashMap<ruff_text_size::TextSize, crate::frontend::RegisteredSuite>,
    function_bodies: HashMap<ruff_text_size::TextSize, crate::frontend::RegisteredSuite>,
    class_bodies: HashMap<ruff_text_size::TextSize, crate::frontend::RegisteredSuite>,
    initializer: Option<crate::frontend::RegisteredSuite>,
    program_bodies: Option<&'a super::CheckedFunctions>,
    bundled: bool,
    path: &'a Path,
    python_annotations: bool,
    module_start: ruff_text_size::TextSize,
    class_keywords: Vec<ruff_text_size::TextSize>,
    docs: Option<crate::modules::doctest::SealedDocs>,
    has_templates: bool,
    strings: crate::frontend::PreparedStrings,
    template_fields: HashMap<ruff_text_size::TextSize, imre_ast::strings::TemplateFieldText>,
    replacements: HashMap<ruff_text_size::TextRange, Vec<u32>>,
    private_names: imre_ast::Names,
    method_imports: std::collections::BTreeSet<String>,
    string_annotations: bool,
    barry_as_flufl: bool,
    text: Cow<'a, str>,
}

impl SourceInfo<'_> {
    fn registered_source(&self) -> crate::frontend::RegisteredSource<'_> {
        crate::frontend::RegisteredSource {
            text: &self.text,
            replacements: &self.replacements,
            has_templates: self.has_templates,
            templates: Some(&self.strings.templates),
            method_imports: &self.method_imports,
            private_names: Some(&self.private_names),
        }
    }
}

impl SourceMap<'_> {
    fn function_flags(&self, params: Option<&ast::Parameters>, nested: bool, method: bool) -> u32 {
        use imre_ast::code_flags;
        code_flags::function_flags(params, nested, method) | self.future_flags()
    }

    fn future_flags(&self) -> u32 {
        use imre_ast::code_flags;
        let mut flags = 0;
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

impl SourceMap<'_> {
    /// Distinct Python definitions keep distinct addresses even if LLVM merges their bodies.
    fn definition(
        &self,
        output: &mut Output,
        prefix: String,
        body: Body<'_>,
        start: ruff_text_size::TextSize,
        in_function: bool,
        entry: Entry,
    ) -> String {
        let site = format!("@code_site{}", output.count);
        let suspended = matches!(entry, Entry::Suspended);
        let tag = if suspended {
            if !output.declarations.declared_suspension {
                output.functions.push_str(suspension::ABI);
                output.declarations.declared_suspension = true;
            }
            permanent::NATIVE_SUSPENDED_BODY
        } else {
            0
        };
        let entry = self.code(output, prefix, body, in_function, Some((start, entry)));
        writeln!(
            output.constants,
            "{site} = private constant %code_site {{ ptr {entry}, ptr {}, i32 {}, i32 {tag} }}",
            self.positions.file,
            self.positions.line(start)
        )
        .unwrap();
        site
    }

    fn code(
        &self,
        output: &mut Output,
        prefix: String,
        body: Body<'_>,
        in_function: bool,
        definition: Option<(ruff_text_size::TextSize, Entry)>,
    ) -> String {
        let native_suspension = matches!(definition, Some((_, Entry::Suspended)));
        let start = definition.map(|(start, _)| start);
        let checked = start
            .filter(|_| matches!(body, Body::Statements(_)))
            .and_then(|key| self.program_bodies?.get(&key));
        let mut frame = RegisteredFrame::new(
            output,
            &self.positions,
            native_suspension,
            self.unit,
            self.module_start,
        );
        frame.async_generator = matches!(
            body,
            Body::GeneratorFunction(_, imre_ast::code_flags::ASYNC_GENERATOR)
        );
        if let Body::Namespace(_) = body
            && !prefix.is_empty()
        {
            frame.class_locals = Some(
                self.scopes.classes[&start.expect("class definition")]
                    .bindings
                    .clone(),
            );
        }
        let module = matches!(body, Body::Namespace(..)) && prefix.is_empty();
        let prepared = match body {
            Body::Namespace(..) if module => self.initializer.as_ref(),
            Body::Namespace(stmts) => stmts
                .first()
                .and_then(|stmt| self.class_bodies.get(&stmt.start())),
            Body::Statements(_) | Body::GeneratorFunction(..) => {
                start.and_then(|key| self.function_bodies.get(&key))
            }
            _ => None,
        };
        let mut f = Function {
            sites: sites::CheckedSites::default(),
            prepare_nested_suites: prepared.is_some_and(|plan| !plan.fully_checked()),
            frame,
            source: self,
            class_sites: None,
            in_function,
            module,
            declared_globals: Default::default(),
            prefix,
            loops: Vec::new(),
            exits: Vec::new(),
        };
        match &body {
            Body::Annotations(entries, _) => {
                for (_, expression) in entries {
                    f.place_annotation_string(expression.start());
                }
            }
            Body::ClassAnnotations(entries, _) => {
                for entry in entries {
                    f.place_annotation_string(entry.expression.start());
                }
            }
            Body::ModuleAnnotations(_, entries) => {
                for entry in *entries {
                    f.place_annotation_string(entry.site);
                }
            }
            Body::Namespace(_) => {
                if let Some(entries) = self.namespace_annotations.get(&start) {
                    for site in entries {
                        f.place_annotation_string(*site);
                    }
                }
            }
            _ => {}
        }
        if let Some(checked) = checked {
            for name in self.scopes.functions[&start.expect("checked function")].globals() {
                f.frame.declare_global(name);
            }
            f.prepare_checked_sites(&checked.function.body);
            RegisteredFrame::checked_body(&mut f.frame, checked, &f.sites);
            return f.frame.finish(false);
        }
        match body {
            Body::Namespace(stmts)
            | Body::Statements(stmts)
            | Body::GeneratorFunction(stmts, _) => {
                if module && imre_ast::annotations::has_variable_annotation(stmts) {
                    if self.string_annotations {
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
                    && let Some(doc) = self.docstring(f.prefix.trim_end_matches('.'), stmts)
                {
                    let value = f.text(&doc);
                    f.bind("__doc__", &value);
                }
                let globals = if module {
                    &self.scopes.module_globals
                } else if matches!(body, Body::Namespace(..)) {
                    &self.scopes.classes[&start.expect("class definition")]
                        .bindings
                        .globals
                } else {
                    self.scopes.functions[&start.expect("function definition")].globals()
                };
                f.declare_globals(globals);
                if let Some(prepared) = prepared {
                    f.prepared_function(prepared, |code, run| code.source_statements(&stmts[run]));
                } else if f.class_locals.is_some() && !self.string_annotations {
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
                f.prepared_function(plan, |code, run| {
                    for index in run {
                        code.unprepared_annotation(index, entries[index]);
                    }
                });
            }
            Body::CheckedExpression(checked) | Body::AliasEvaluator(AliasValue::Checked(checked)) => {
                f.prepare_checked_sites(&checked.function.body);
                RegisteredFrame::checked_body(&mut f.frame, checked, &f.sites);
            }
            Body::Generator(plan) => f.generator_step(plan),
            Body::AliasEvaluator(AliasValue::Residual(expr)) => {
                f.checkpoint(expr);
                let format = f.lookup(".format");
                f.op("annotation_format", &[&format]);
                let value = f.expr(expr);
                f.return_value(&value);
            }
            Body::Expression(expr) => {
                f.checkpoint(expr);
                let value = f.expr(expr);
                f.return_value(&value);
            }
        }
        f.frame.finish(module)
    }
}

struct Function<'a, 'p> {
    sites: sites::CheckedSites,
    prepare_nested_suites: bool,
    frame: RegisteredFrame<'a, 'p>,
    source: &'p SourceMap<'p>,
    prefix: String,
    declared_globals: std::collections::BTreeSet<String>,
    in_function: bool,
    module: bool,
    class_sites: Option<statements::class_annotations::ClassSites>,
    loops: Vec<LoopTarget>,
    exits: Vec<(Cleanup, String)>,
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

use crate::llvm::registered::context::ContextExit;

#[derive(Clone)]
enum Cleanup {
    With(ContextExit),
    ExceptionName(String),
    RetainedValue {
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
    step: (String, usize),
    done: (String, usize),
}

impl Function<'_, '_> {
    fn checkpoint(&mut self, node: &impl Ranged) {
        self.current_range = node.range();
        self.frame.checkpoint(node.range());
    }

    fn annotation_string(&mut self, site: ruff_text_size::TextSize) -> String {
        self.sites.annotation_string(&mut self.frame, site)
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prepared_format_literals_emit_without_lexical_replacements() {
        let mut prepared = PreparedBody::module(
            "example",
            Path::new("example.py"),
            r#"out = f"\ud800{{{value = :{width}\ud801}}}" "\ud802""#,
            0,
            ModuleOrigin::Program(None),
        )
        .unwrap();
        assert!(!prepared.source.strings.literals.is_empty());
        let before = prepared.standalone().unwrap().unwrap();
        assert!(!prepared.source.replacements.is_empty());
        prepared.source.replacements.clear();
        assert_eq!(prepared.standalone().unwrap().unwrap(), before);
    }

    #[test]
    fn residual_named_assignment_consumes_its_completed_choice_after_erasing_the_arm_syntax() {
        let mut prepared = PreparedBody::module(
            "program", Path::new("program.py"),
            "try:\n    result = (seen := yes() if first() and second() else no())\nfinally:\n    cleanup()\n",
            0, ModuleOrigin::Program(None),
        ).unwrap();
        fn emitted(body: &PreparedBody) -> String {
            let mut out = Output::default();
            let file = out.data(b"program.py");
            body.emit(&mut out, 0, &file).unwrap();
            out.functions
        }
        let original = emitted(&prepared);
        let ast::Stmt::Try(block) = &mut prepared.body[0] else {
            panic!("expected try")
        };
        let ast::Stmt::Assign(assign) = &mut block.body[0] else {
            panic!("expected assignment")
        };
        assert!(
            crate::frontend::registered_value(&assign.value, prepared.source.registered_source())
                .is_none()
        );
        let ast::Expr::Named(named) = assign.value.as_mut() else {
            panic!("expected assignment expression")
        };
        assert!(matches!(
            prepared.source.logicals.get(&named.value),
            Some(crate::frontend::LogicalSelection::Checked(_))
        ));
        let ast::Expr::If(choice) = named.value.as_mut() else {
            panic!("expected choice")
        };
        for part in [&mut choice.test, &mut choice.body, &mut choice.orelse] {
            **part = ast::Expr::NoneLiteral(ast::ExprNoneLiteral {
                range: part.range(),
                node_index: Default::default(),
            });
        }
        assert_eq!(emitted(&prepared), original);
    }

    #[test]
    fn prepared_template_emits_after_source_body_is_dropped() {
        let mut prepared = PreparedBody::module(
            "query",
            Path::new("query.py"),
            r#"query = t"SELECT {client.lookup(*args, key=request.name, **options)!s:>{width}}""#,
            0,
            ModuleOrigin::Program(None),
        )
        .unwrap();
        assert!(
            prepared
                .source
                .initializer
                .as_ref()
                .unwrap()
                .fully_checked()
        );
        prepared.body.clear();
        assert!(prepared.source.strings.literals.is_empty());
        assert!(prepared.source.strings.residual_templates.is_empty());
        prepared.source.strings.templates.clear();
        prepared.source.template_fields.clear();

        let mut output = Output::default();
        let filename = output.data(b"query.py");
        prepared.emit(&mut output, 0, &filename).unwrap();
        assert_eq!(
            output
                .functions
                .matches("call i64 @imre_dyn_scoped_call_at(")
                .count(),
            1,
        );
        assert!(
            output
                .functions
                .contains("call i64 @imre_dyn_cached_handle(")
        );
        assert!(
            output
                .functions
                .contains("call i64 @imre_dyn_build_sequence(")
        );
        for operation in [
            "spread_arguments",
            "format",
            "template_interpolation",
            "build_template",
        ] {
            let symbol = output.strings[operation.as_bytes()];
            assert!(
                output.functions.contains(&format!(
                    "call i64 @imre_dyn_op_at(ptr %ctx, ptr @str{symbol},"
                )),
                "{operation}"
            );
        }
    }

    #[test]
    fn a_residual_operand_keeps_its_entire_template_at_the_source_boundary() {
        let prepared = PreparedBody::module(
            "query",
            Path::new("query.py"),
            "first = t'{client.lookup(*args)}{v * 2}'
second = t'{client.lookup(*args)}{(v for v in values)}'",
            0,
            ModuleOrigin::Program(None),
        )
        .unwrap();
        let checked: Vec<_> = prepared
            .source
            .strings
            .templates
            .keys()
            .map(|&range| &prepared.source.text[range])
            .collect();
        assert_eq!(checked, ["t'{client.lookup(*args)}{v * 2}'"]);
        assert!(
            !prepared
                .source
                .initializer
                .as_ref()
                .unwrap()
                .fully_checked()
        );
    }

    #[test]
    fn bundled_class_methods_have_their_own_checked_suite() {
        let prepared = PreparedBody::module(
            "reader",
            Path::new("reader.py"),
            "class Reader:\n    def read(self):\n        value = self.source()\n        return value\n",
            0,
            ModuleOrigin::Bundled,
        )
        .unwrap();
        let methods = &prepared.source.function_bodies;
        assert_eq!(methods.len(), 1);
        assert!(methods.values().all(crate::frontend::RegisteredSuite::fully_checked));
    }

    #[test]
    fn physical_worker_targets_use_checked_registered_calls() {
        use crate::ir::{self, Builtin, ExprKind, RegisteredCallTarget};
        use imre_ast::ThreadEntry;

        for (source, target, entry, calls, excluded) in [
            (
                include_str!("../../../../tests/fixtures/boxed_thread_bound_lock.py"),
                "def task(",
                ThreadEntry::Bound("gate".into()),
                1,
                "def replacement(",
            ),
            (
                include_str!("../../../../tests/fixtures/boxed_thread_method_closure.py"),
                "@staticmethod",
                ThreadEntry::Captured(vec!["gate".into()]),
                1,
                "def make_wait(",
            ),
            (
                include_str!("../../../../tests/fixtures/boxed_threading_tcp_server.py"),
                "def acquire_on_worker(",
                ThreadEntry::Parameters {
                    names: vec!["started".into(), "gate".into()],
                    lock_only: true,
                },
                2,
                "def handle(",
            ),
        ] {
            let prepared = PreparedBody::module(
                "worker",
                Path::new("worker.py"),
                source,
                0,
                ModuleOrigin::Program(None),
            )
            .unwrap();
            let start = |declaration| {
                ruff_text_size::TextSize::try_from(source.find(declaration).unwrap()).unwrap()
            };
            let entries = &prepared.source.thread_entries;
            assert_eq!(entries.get(&start(target)), Some(&entry));
            assert!(!entries.contains_key(&start(excluded)));
            let suite = &prepared.source.function_bodies[&start(target)];
            assert!(suite.fully_checked());
            let mut methods = Vec::new();
            ir::walk_expressions(&suite.checked.function.body, &mut |expr| {
                if let ExprKind::Builtin(Builtin::RegisteredCall(call), _) = &expr.kind
                    && let RegisteredCallTarget::Method { name, .. } = &call.target
                {
                    methods.push(name.as_str());
                }
            });
            assert_eq!(methods, vec!["acquire"; calls]);
        }
    }

    #[test]
    fn type_alias_uses_the_shared_operand_for_each_rhs() {
        let prepared = PreparedBody::module(
            "editor",
            Path::new("editor.py"),
            "type Callback = Callable[[], object]\n\
             type CharBuffer = list[str]\n\
             type Completer = Callable[[str, int], str | None]\n\
             type PlatformTripleKey = tuple[str, str, str]\n",
            0,
            ModuleOrigin::Program(None),
        )
        .unwrap();
        for alias in prepared
            .body
            .iter()
            .filter_map(ast::Stmt::as_type_alias_stmt)
        {
            let shared = crate::frontend::registered_value(
                &alias.value,
                prepared.source.registered_source(),
            );
            let checked = prepared.source.type_aliases[&alias.start()].body.is_some();
            let name = alias.name.as_name_expr().unwrap().id.as_str();
            assert_eq!(checked, shared.is_some(), "{name}");
            if name != "Completer" {
                assert!(checked, "expected checked operands for {name}");
            }
            assert_eq!(
                checked,
                prepared
                    .source
                    .namespace_operations
                    .contains_key(&alias.range())
            );
        }
        prepared.standalone().unwrap().expect("native initializer");
    }

    #[test]
    fn checked_alias_site_emits_after_dropping_the_ruff_suite() {
        let mut prepared = PreparedBody::module(
            "model",
            Path::new("model.py"),
            "type PublicRow[T] = payload[T]\n",
            0,
            ModuleOrigin::Program(None),
        )
        .unwrap();
        assert!(
            prepared
                .source
                .initializer
                .as_ref()
                .unwrap()
                .fully_checked()
        );
        drop(std::mem::take(&mut prepared.body));

        let mut output = Output::default();
        let file = output.data(b"model.py");
        let entry = prepared.emit(&mut output, 0, &file).unwrap();
        let alias_op = output.data(b"type_alias");
        let payload = output.data(b"payload");
        let format_op = output.data(b"annotation_format");
        let body = |name: &str| {
            output
                .functions
                .split_once(&format!("define internal i64 {name}("))
                .unwrap()
                .1
                .split_once("\n}")
                .unwrap()
                .0
        };
        let initializer = body(&entry);
        assert!(initializer.contains(&format!(
            "call i64 @imre_dyn_op_at(ptr %ctx, ptr {alias_op},"
        )));
        let (_, definition) = initializer
            .split_once("call i64 @imre_dyn_define(ptr %ctx, ptr ")
            .unwrap();
        let (site, _) = definition.split_once(',').unwrap();
        let (_, descriptor) = output
            .constants
            .split_once(&format!("{site} = private constant %code_site {{ ptr "))
            .unwrap();
        let (callback, _) = descriptor.split_once(',').unwrap();
        let evaluator = body(callback);
        let check = evaluator
            .find(&format!(
                "call i64 @imre_dyn_op_at(ptr %ctx, ptr {format_op},"
            ))
            .unwrap();
        let read = evaluator.find(&format!("ptr {payload}, i64 7)")).unwrap();
        assert!(check < read);
        assert!(!initializer.contains(&format!("ptr {payload}, i64 7)")));
    }

    #[test]
    fn type_alias_returns_a_registered_lambda_with_a_prepared_template_body() {
        use crate::ir::{Builtin, ExprKind};

        let prepared = PreparedBody::module(
            "editor",
            Path::new("editor.py"),
            "type Factory = lambda item: t'{item}'\n",
            0,
            ModuleOrigin::Program(None),
        )
        .unwrap();
        let alias = &prepared.source.type_aliases[&prepared.body[0].start()];
        let body = &alias.body.as_ref().expect("checked alias value").function.body;
        let mut sites = Vec::new();
        crate::ir::walk_expressions(body, &mut |expr| {
            if let ExprKind::Builtin(Builtin::RegisteredLambda(site, _), _) = &expr.kind {
                sites.push(*site);
            }
        });
        let [site] = sites.as_slice() else {
            panic!("expected one checked lambda binding in the alias evaluator");
        };
        assert!(prepared.source.lambdas[site].body.is_some());
    }

    #[test]
    fn type_alias_binds_a_checked_callback_with_lazy_name_lookups() {
        use crate::ir::{self, Builtin, ExprKind, Statement};

        let prepared = PreparedBody::module(
            "editor",
            Path::new("editor.py"),
            "type CharBuffer = list[str]\n",
            0,
            ModuleOrigin::Program(None),
        )
        .unwrap();
        let initializer = prepared.source.initializer.as_ref().unwrap();
        assert!(initializer.fully_checked());
        let [_, Statement::Expr(binding)] = initializer.checked.function.body.as_slice() else {
            panic!("expected one declaration after the statement marker");
        };
        let ExprKind::Builtin(Builtin::GlobalSet(slot), operands) = &binding.kind else {
            panic!("expected the alias binding");
        };
        assert_eq!(initializer.checked.local_names[slot].name, "CharBuffer");
        let [alias] = operands.as_slice() else {
            panic!("expected one lazy alias");
        };
        let ExprKind::Builtin(Builtin::RegisteredTypeAlias(site), args) = &alias.kind else {
            panic!("expected the prepared alias site");
        };
        assert!(args.is_empty());
        let callback = &prepared.source.type_aliases[site]
            .body
            .as_ref()
            .unwrap()
            .function
            .body;
        let [_, Statement::Expr(format), Statement::Return(value)] = callback.as_slice() else {
            panic!("expected the format check before the alias value");
        };
        assert!(
            matches!(&format.kind, ExprKind::Builtin(Builtin::RegisteredAnnotationFormat, args)
            if matches!(args.as_slice(), [arg] if matches!(&arg.kind,
                ExprKind::Builtin(Builtin::RegisteredName(name), _) if name == ".format")))
        );
        let mut names = Vec::new();
        ir::walk_operand_expressions(value, &mut |expr| {
            if let ExprKind::Builtin(Builtin::RegisteredName(name), _) = &expr.kind {
                names.push(name.as_str());
            }
        });
        assert_eq!(names, ["list", "str"]);
    }

    #[test]
    fn exception_group_handlers_in_nested_generators_have_native_entries() {
        let source = "def supported():\n    yield 1\ndef handled():\n    def nested():\n        yield 2\n    try:\n        yield 3\n    except* ValueError:\n        pass\ndef outer():\n    def inner():\n        try:\n            yield 4\n        except* ValueError:\n            pass\n    yield 5\n";
        let (_, ir) = PreparedBody::module(
            "sample",
            Path::new("sample.py"),
            source,
            0,
            ModuleOrigin::Program(None),
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
                0,
                if bundled {
                    ModuleOrigin::Bundled
                } else {
                    ModuleOrigin::Program(None)
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
            0,
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
            ("plain".into(), PathBuf::from("plain.py"), "pass".into()),
        ];
        let builtins = [(
            "native".into(),
            root.join("native.py"),
            "answer = 42".into(),
        )];
        let (name, path, source) = &modules[3];
        let (symbol, _) = PreparedBody::module(name, path, source, 0, ModuleOrigin::Bundled)
            .unwrap()
            .standalone()
            .unwrap()
            .unwrap();
        let providers = HashSet::from([symbol.clone()]);
        let registrations = RegisteredBodies {
            physical_files: HashMap::from([(
                modules[0].1.clone(),
                PhysicalFile {
                    digest: Some([b'F'; 32]),
                    future_flags: 8,
                },
            )]),
            archive_members: HashMap::from([(modules[1].1.clone(), ([b'Z'; 32], 16))]),
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
                (&modules, &builtins, None),
                "{}",
                0,
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
        for ir in [&whole[0].llvm_ir, &split[0].llvm_ir] {
            let main = ir.split_once("define i32 @main(").unwrap().1;
            let calls: Vec<_> = main
                .lines()
                .filter_map(|line| {
                    line.trim_start()
                        .strip_prefix("call void @imre_dyn_register_")
                })
                .map(|call| call.split_once('(').unwrap())
                .collect();
            assert_eq!(
                calls.iter().map(|(op, _)| *op).collect::<Vec<_>>(),
                [
                    "file_code",
                    "zip_source",
                    "module_error",
                    "source",
                    "source",
                    "builtin_source"
                ],
            );
            let constant = |encoded: &str| {
                let suffix = format!(r#"c"{encoded}\00""#);
                ir.lines()
                    .filter_map(|line| line.split_once(" = "))
                    .find(|(_, data)| data.ends_with(&suffix))
                    .unwrap()
                    .0
            };
            for (i, payload, flags) in [(0, "F".repeat(32), 8), (1, "Z".repeat(32), 16)] {
                assert!(
                    calls[i]
                        .1
                        .contains(&format!("ptr {}, i32 {flags}, ptr @", constant(&payload)))
                );
            }
            let archived = constant(r"value = 1\0A");
            assert!(
                calls[3]
                    .1
                    .ends_with(&format!("ptr {archived}, i64 10, ptr @{symbol})"))
            );
            assert!(calls[4].1.contains("ptr null, i64 0, ptr @"));
            let builtin = constant("answer = 42");
            assert!(
                calls[5]
                    .1
                    .contains(&format!("ptr {builtin}, i64 11, ptr @"))
            );
        }
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
    fn declare_context_position_only_for_compiled_with() {
        let path = Path::new("<imre stdlib>/example.py");
        let declare = "declare void @imre_dyn_definition_position";
        for (source, expected) in [("value = 1", 0), ("with a, b:\n    pass", 1)] {
            let (_, ir) = PreparedBody::module("example", path, source, 0, ModuleOrigin::Bundled)
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
        let compiled = |name, path: &Path, src, optimization| {
            PreparedBody::module(name, path, src, optimization, ModuleOrigin::Bundled)
                .unwrap()
                .standalone()
                .unwrap()
                .unwrap()
        };
        let (symbol, ir) = compiled("example", &path, source, 0);
        let slot = ir
            .lines()
            .find_map(|line| line.strip_suffix(" = private constant i8 0"))
            .expect("a source-local address for the template strings");
        assert!(ir.contains(&format!(
            "call i64 @imre_dyn_cached_handle(ptr %ctx, ptr {slot})"
        )));
        assert_ne!(symbol, compiled("other", &path, source, 0).0);
        assert_ne!(
            symbol,
            compiled("example", &root.join("other.py"), source, 0).0
        );
        assert_ne!(symbol, compiled("example", &path, "value = 2", 0).0);
        assert_ne!(symbol, compiled("example", &path, source, 2).0);

        assert_ne!(
            compiled("example", &path, "assert ready()", 0).0,
            compiled("example", &path, "assert ready()", 1).0,
        );

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
                (modules, &[], None),
                "{}",
                0,
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
                    (std::slice::from_ref(missing), &[], None),
                    "{}",
                    0,
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
            (&[bundled.clone(), project.clone()], &[], None),
            "{}",
            0,
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
            (&[bundled, project.clone(), shadow, project], &[], None),
            "{}",
            0,
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

    #[test]
    fn physical_deletion_uses_the_complete_checked_statement_after_targets_are_dropped() {
        let source = concat!(
            "del first, [(owner := obj).field, obj[lo:hi]], obj[[key for key in keys if key]], last\n",
            "del rejected, (orphan := obj)[(key for key in keys if key)]\n",
        );
        let mut prepared = PreparedBody::module(
            "remove",
            Path::new("remove.py"),
            source,
            0,
            ModuleOrigin::Program(None),
        )
        .unwrap();
        let admitted = prepared.body[0].range();
        let rejected = prepared.body[1].range();
        assert_eq!(&source[admitted], source.lines().next().unwrap());
        assert_eq!(&source[rejected], source.lines().nth(1).unwrap());
        let sites = &prepared.source.checked_deletions;
        assert_eq!(sites.len(), 1);
        assert!(!sites.contains_key(&rejected));
        let checked = &sites[&admitted];
        crate::verify::registered(checked.region()).unwrap();

        let initializer = prepared.source.initializer.take().unwrap();
        let [gap] = initializer.residual.as_slice() else {
            panic!("expected the second deletion to remain physical");
        };
        assert_eq!(gap.statements, 1..2);
        assert_eq!(gap.at, checked.function.body.len());
        assert_eq!(gap.at, initializer.checked.function.body.len());
        assert_eq!(initializer.checked.local_names, checked.local_names);

        // Leave just the physical statement selector; the saved plan owns all its effects.
        prepared.body.truncate(1);
        let emit = |prepared: &PreparedBody<'_>| {
            let mut output = Output::default();
            let file = output.data(b"remove.py");
            prepared.emit(&mut output, 0, &file).unwrap();
            for op in ["delname", "delattr", "delitem"] {
                let symbol = output.strings[op.as_bytes()];
                assert!(
                    output.functions.contains(&format!("ptr @str{symbol},")),
                    "{op}"
                );
            }
            (output.functions, output.constants)
        };
        let before = emit(&prepared);
        let stmt = prepared.body[0].as_delete_stmt_mut().unwrap();
        stmt.targets.clear();
        assert_eq!(stmt.range(), admitted);
        assert_eq!(emit(&prepared), before);
    }
}
