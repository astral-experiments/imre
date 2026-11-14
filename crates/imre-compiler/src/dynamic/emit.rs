//! Emit native functions and control flow for closed modules using boxed Python values.

use std::borrow::Cow;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use ruff_python_ast as ast;
use ruff_text_size::{Ranged, TextRange, TextSize};

use super::prepare::{Initializer, PreparedUnit};
use crate::dynamic::literal::LiteralSites;
use crate::frontend::function_scope::{ClassBindings, NamedFunction};
use crate::frontend::module_source::{self, ParsedModule};
use crate::frontend::{
    ParameterEvaluator, RegisteredBody, RegisteredGenerator, RegisteredLambda, RequiredSite,
};
use crate::ir::RegisteredTypeParameter;
use crate::llvm::registered::sites::{
    CheckedSites, CompiledAlias, CompiledDefinition, CompiledTypeParameters, Declaration,
};
use crate::llvm::registered::{AnnotationScope, Output, RegisteredFrame, SourcePositions, sites};
use crate::llvm::{ModuleLauncher, ProviderBody};
use serde_json::json;

#[derive(Clone, Copy, Default)]
pub(super) struct PhysicalFile {
    pub(super) source_kind: Option<crate::SourceKind>,
    pub(super) digest: Option<[u8; 32]>,
}

/// Frontend inputs for one resolved source slot. Syntax can be shared even if
/// admission does not attach any checked function bodies.
#[derive(Default)]
pub(super) struct CheckedUnit {
    pub(super) module: Option<crate::modules::ModuleId>,
    pub(super) source: Option<module_source::CheckedFile>,
    // A declaration belongs to the selected name and origin, even when another
    // loader registers the same physical file under a different name.
    pub(super) run_path: Option<PathBuf>,
}

/// Snapshots and admission facts for the bodies selected by import resolution.
#[derive(Default)]
pub(super) struct RegisteredBodies<'a> {
    pub(super) checked_units: HashMap<usize, CheckedUnit>,
    pub(super) functions: HashMap<crate::modules::ModuleId, super::CheckedFunctions>,
    pub(super) modules: &'a [crate::modules::SourceModule],
    // Decoded installed files retain the fingerprint of their original loader bytes.
    pub(super) physical_files: HashMap<PathBuf, PhysicalFile>,
    pub(super) archive_members: HashMap<PathBuf, [u8; 32]>,
    // Each source slot has its own ownership, even if two names resolve to one file.
    // Providers contribute declarations to the checked program. Their Python bodies
    // retain Python argument binding, including annotations available to reflection.
    pub(super) python_providers: HashSet<usize>,
}

impl RegisteredBodies<'_> {
    pub(super) fn origin(&self, unit: usize, bundled: bool) -> ModuleOrigin<'_> {
        if bundled {
            ModuleOrigin::Bundled
        } else if self.python_providers.contains(&unit) {
            ModuleOrigin::Provider
        } else {
            ModuleOrigin::Program(
                self.checked_units
                    .get(&unit)
                    .and_then(|unit| self.functions.get(&unit.module?)),
            )
        }
    }

    pub(super) fn parsed(&self, unit: usize) -> Option<ParsedModule<'_, '_>> {
        self.checked_units
            .get(&unit)?
            .source
            .map(|file| file.parsed(self.modules))
    }
}

#[derive(Clone, Copy)]
pub(super) enum ModuleOrigin<'a> {
    Program(Option<&'a super::CheckedFunctions>),
    ExecLiteral {
        inherited_flags: u32,
        python_annotations: bool,
    },
    Interactive(u32),
    Provider,
    Bundled,
}

impl ModuleOrigin<'_> {
    pub(super) fn python_annotations(self, name: &str) -> bool {
        match self {
            Self::Provider => true,
            Self::ExecLiteral {
                python_annotations, ..
            } => python_annotations,
            Self::Bundled => matches!(name, "_py_warnings" | "tomllib._parser" | "typing"),
            _ => false,
        }
    }
}

impl crate::ProgramOutput {
    /// Write the initializers in registration order, releasing each body after emission.
    pub(super) fn emit_registered<'a>(
        &mut self,
        units_to_emit: impl Iterator<Item = Result<PreparedUnit<'a>, String>>,
        application_entry: Option<usize>,
        config: &str,
        python_optimization: i32,
        providers: &HashSet<String>,
    ) -> Result<bool, String> {
        let split = !matches!(self.layout, crate::CodegenLayout::Whole);
        let whole_requested = matches!(self.layout, crate::CodegenLayout::SplitWithWhole);
        let mut used_archive = false;
        let root = ModuleLauncher::new(config, python_optimization < 2);
        // The requested whole IR and split native root share their exact prefix.
        // Fork only when the split layout acquires its first independent local unit.
        self.emit_sources(
            units_to_emit,
            (0, "__main__"),
            split,
            (root, None::<ModuleLauncher>),
            |(root, whole), unit, prepared, separate| -> Result<_, String> {
                // The selected project entry shares its launcher's native object.
                let separate = separate && (whole_requested || application_entry != Some(unit));
                let (name, path, source) = prepared.module;
                let initializer = match &prepared.initializer {
                    Ok(initializer) => initializer,
                    Err(error) => {
                        for state in std::iter::once(root).chain(whole.as_mut()) {
                            state.register_error(
                                (name, path, source),
                                &error.message,
                                error.position,
                                error.in_args,
                                error.parse_error,
                                prepared.zip,
                            );
                        }
                        return Ok((name.as_str(), None));
                    }
                };
                let (bundled, future_flags, prompts) = match initializer {
                    Initializer::Checked(body) => {
                        (body.source.bundled, body.source.future_flags, body.prompts)
                    }
                    Initializer::Archived(_) => (true, 0, &[][..]),
                };
                let registration = prepared.registration(bundled, future_flags);
                let check_archive = bundled && !providers.is_empty();
                let mut placed = None;
                let mut whole_entry = None;
                let mut external = None;
                let mut archived = matches!(initializer, Initializer::Archived(_));
                if let Initializer::Checked(initializer) = initializer
                    && initializer.has_body_statements
                    && (check_archive || separate)
                {
                    // Commit the fork only if the provider gets an independent native unit.
                    let mut fork = None;
                    let target = if separate && !check_archive && whole_requested {
                        let launcher = whole.as_mut().unwrap_or_else(|| fork.insert(root.clone()));
                        let module = launcher.module_registration(&registration);
                        let (_, entry) = whole_entry.insert((module, String::new()));
                        Some((&mut launcher.output, unit, entry))
                    } else {
                        None
                    };
                    let (symbol, body) = initializer.provider_body(target)?;
                    archived = check_archive && providers.contains(&symbol);
                    if !archived && separate && !root.declares(&symbol) {
                        if whole.is_none() && whole_requested {
                            *whole = Some(fork.unwrap_or_else(|| root.clone()));
                        }
                        placed = Some(body.llvm_ir(&symbol, ""));
                    }
                    if archived || separate {
                        external = Some(symbol);
                    }
                }
                used_archive |= archived;
                let register = |launcher: &mut ModuleLauncher,
                                symbol: Option<&str>,
                                entry|
                 -> Result<String, String> {
                    let (module, code) = if let Some(entry) = entry {
                        entry
                    } else {
                        let module = launcher.module_registration(&registration);
                        let code = match (symbol, initializer) {
                            (Some(symbol), _) | (None, &Initializer::Archived(symbol)) => {
                                launcher.external(symbol)
                            }
                            (None, Initializer::Checked(body)) => {
                                body.emit(&mut launcher.output, unit, &module.file)?
                            }
                        };
                        (module, code)
                    };
                    module.write_call(launcher, &code);
                    Ok(module.name)
                };
                let owner = register(root, external.as_deref(), None)?;
                let mut other = if let Some(whole) = whole {
                    let owner =
                        register(whole, external.as_deref().filter(|_| archived), whole_entry)?;
                    Some((whole, owner))
                } else {
                    None
                };
                for Prompt(prompt) in prompts {
                    if let Some((whole, _)) = &mut other {
                        root.output.begin_placement(&mut whole.output, unit);
                    }
                    let mut file = root
                        .output
                        .data(prompt.source.path.as_os_str().as_encoded_bytes());
                    let mut code = prompt.emit(&mut root.output, unit, &file)?;
                    let text = &prompt.source.text;
                    let flags = prompt.source.future_flags;
                    if let Some((whole, owner)) = &mut other {
                        let whole_file = Output::whole_operand(&mut file);
                        let whole_code = Output::whole_operand(&mut code);
                        root.output.finish_placement(&mut whole.output);
                        whole.register_single(owner, &whole_file, text, flags, &whole_code);
                    }
                    root.register_single(&owner, &file, text, flags, &code);
                }
                Ok((name.as_str(), placed))
            },
            |(root, whole), units, entry| {
                units[entry].llvm_ir = root.finish();
                Ok(whole.map(ModuleLauncher::finish))
            },
        )?;
        Ok(used_archive)
    }
}

/// A selected single input owns its exact checked body. An unsupported prompt
/// leaves its fixed suite eligible for the existing native runner.
pub(crate) struct Prompt(PreparedBody<'static>);

impl Prompt {
    pub(crate) fn new(filename: PathBuf, source: String, python_optimization: i32) -> Option<Self> {
        if python_optimization >= 2 {
            return None;
        }
        PreparedBody::interactive(filename, source, python_optimization, 0).map(Self)
    }
}

/// Normalized source and lowered names shared by archive, split, and inline placement.
pub(super) struct PreparedBody<'a> {
    pub(super) prompts: &'a [Prompt],
    pub(super) has_body_statements: bool,
    name: &'a str,
    #[cfg(test)]
    syntax: Option<Cow<'static, ast::Suite>>,
    docstring: Option<String>,
    source: SourceInfo<'a>,
    initializer: crate::frontend::RegisteredSuite,
}

impl<'a> PreparedUnit<'a> {
    fn registration(&self, bundled: bool, future_flags: u32) -> crate::ir::ModuleRegistration<'a> {
        use crate::ir::{ModuleLoader, ModuleRegistration, PhysicalModuleKind};
        use sha2::{Digest, Sha256};

        let (name, path, original_source) = self.module;
        let loader = if let Some(&digest) = self.zip {
            ModuleLoader::Physical {
                kind: PhysicalModuleKind::Zip,
                digest,
                future_flags,
            }
        } else if let Some((file, kind)) = self.physical {
            // A file loader fingerprints the original bytes even when the body was normalized.
            ModuleLoader::Physical {
                kind,
                digest: file
                    .digest
                    .unwrap_or_else(|| Sha256::digest(original_source.as_bytes()).into()),
                future_flags,
            }
        } else {
            ModuleLoader::Source {
                builtin: self.builtin,
                text: bundled.then_some(original_source.as_bytes()),
            }
        };
        ModuleRegistration {
            name,
            file: path,
            loader,
        }
    }
}

pub(super) type ParsedInput<'a, 'p> = (ParsedModule<'a, 'p>, Option<super::LoweredRoot>);

impl<'a, 'p> From<ParsedModule<'a, 'p>> for ParsedInput<'a, 'p> {
    fn from(module: ParsedModule<'a, 'p>) -> Self {
        (module, None)
    }
}

impl<'a> PreparedBody<'a> {
    #[cfg(test)]
    fn module(
        name: &'a str,
        path: &'a Path,
        source: &'a str,
        python_optimization: i32,
        origin: ModuleOrigin<'a>,
    ) -> Result<PreparedBody<'a>, String> {
        Self::with_module(name, path, source, python_optimization, origin, Ok)
    }

    #[cfg(test)]
    fn module_with_syntax(
        name: &'a str,
        path: &'a Path,
        text: &'a str,
        python_optimization: i32,
        origin: ModuleOrigin<'a>,
    ) -> Result<PreparedBody<'a>, String> {
        let mut module = Self::module(name, path, text, python_optimization, origin)?;
        // These fixtures edit the original suite or force a physical operand after checking.
        module.syntax = Some(Cow::Owned(module.original_fixture_suite()?));
        Ok(module)
    }

    #[cfg(test)]
    fn original_fixture_suite(&self) -> Result<ast::Suite, String> {
        let path = self.source.path.as_ref();
        let text = self.source.text.as_ref();
        let mut body = module_source::into_body(ParsedModule::parse(path, text.into())?.parsed);
        let names = imre_ast::lower(&mut body);
        module_source::validate_names(path, text, &body, &names)?;
        Ok(body)
    }

    #[cfg(test)]
    pub(super) fn with_module<R>(
        name: &'a str,
        path: &'a Path,
        source: &'a str,
        python_optimization: i32,
        origin: ModuleOrigin<'a>,
        use_module: impl FnOnce(PreparedBody<'a>) -> Result<R, String>,
    ) -> Result<R, String> {
        let module = Self::prepare(
            name,
            path.into(),
            ParsedModule::parse(path, source.into())?.into(),
            python_optimization,
            origin,
            LiteralSites::default(),
            (None, None),
        )?;
        use_module(module)
    }

    /// Prepare a finite `single` input in the same lexical scope as module code.
    fn interactive(
        path: impl Into<Cow<'a, Path>>,
        text: impl Into<Cow<'a, str>>,
        python_optimization: i32,
        inherited_flags: u32,
    ) -> Option<Self> {
        let path = path.into();
        let module = ParsedModule::parse(&path, text.into()).ok()?;
        imre_ast::check_single_input(&module.parsed.syntax().body, module.parsed.tokens()).ok()?;
        Self::from_parsed(
            "<module>",
            path,
            module,
            python_optimization,
            ModuleOrigin::Interactive(inherited_flags),
            LiteralSites::default(),
            Default::default(),
        )
        .ok()
    }

    pub(super) fn from_parsed<'p>(
        name: &'a str,
        path: impl Into<Cow<'a, Path>>,
        module: impl Into<ParsedInput<'a, 'p>>,
        python_optimization: i32,
        origin: ModuleOrigin<'a>,
        literal_sites: LiteralSites,
        (operand_types, checked_source): (
            Option<crate::analysis::OperandTypes<'_>>,
            Option<&'a crate::modules::SourceModule>,
        ),
    ) -> Result<Self, String> {
        let body = Self::prepare(
            name,
            path.into(),
            module.into(),
            python_optimization,
            origin,
            literal_sites,
            (operand_types, checked_source),
        )?;
        if let Some(refusal) = body.initializer.refusal {
            use crate::frontend::PreparedRefusal;
            return Err(match refusal {
                PreparedRefusal::Operand((range, reason)) => module_source::operand_diagnostic(
                    &body.source.path,
                    &body.source.text,
                    range,
                    reason,
                ),
                PreparedRefusal::ExecLiteral(site) => {
                    let (operand, reason) = body
                        .source
                        .exec_literals
                        .body(site)
                        .and_then(Result::err)
                        .expect("selected literal refusal belongs to this source");
                    body.source.literal_diagnostic(operand, reason)
                }
            });
        }
        Ok(body)
    }

    fn prepare(
        name: &'a str,
        path: Cow<'a, Path>,
        (module, normalized): ParsedInput<'a, '_>,
        python_optimization: i32,
        origin: ModuleOrigin<'a>,
        exec_literals: LiteralSites,
        (operand_types, checked_source): (
            Option<crate::analysis::OperandTypes<'_>>,
            Option<&'a crate::modules::SourceModule>,
        ),
    ) -> Result<Self, String> {
        let ParsedModule { text, parsed } = module;
        #[cfg(test)]
        crate::single_emission_tests::record_preparation(&path, matches!(parsed, Cow::Borrowed(_)));
        // Layouts can parse the same physical bytes in another session.
        let checked_source = checked_source.filter(|source| {
            source.is_program_source() && std::ptr::eq(source.source.as_str(), text.as_ref())
        });
        let interactive = matches!(origin, ModuleOrigin::Interactive(_));
        let keep_docstrings = python_optimization < 2;
        let source = text.as_ref();
        let bundled = matches!(origin, ModuleOrigin::Bundled);
        let inherited_flags = match origin {
            ModuleOrigin::ExecLiteral {
                inherited_flags, ..
            }
            | ModuleOrigin::Interactive(inherited_flags) => inherited_flags,
            _ => 0,
        };
        let program_bodies = match origin {
            ModuleOrigin::Program(bodies) => bodies,
            _ => None,
        };
        let syntax = parsed.syntax();
        let tokens = parsed.tokens();
        let (replacements, has_templates) = module_source::validate(
            &path,
            source,
            &parsed,
            checked_source,
            keep_docstrings
                && !(interactive && syntax.body.first().is_some_and(ast::Stmt::is_expr_stmt)),
        )?;
        let docs = if keep_docstrings && name == "collections" && bundled {
            crate::modules::doctest::sealed(crate::modules::IntrinsicModule::Collections)
        } else {
            None
        };
        let registered_values = checked_source
            .filter(|source| std::ptr::eq(source.parsed.syntax(), syntax))
            .and_then(|source| source.cached_registered_values());
        let template_fields = if has_templates {
            imre_ast::strings::template_fields(source, tokens, syntax)
        } else {
            HashMap::new()
        };
        // The launcher already emits the module prologue, including its docstring,
        // for units without a separate executable body.
        let has_body_statements =
            !ast::helpers::body_without_leading_docstring(&syntax.body).is_empty();
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
        let method_imports = registered_values.map_or_else(
            || Cow::Owned(imre_ast::module_imports(&syntax.body)),
            |values| Cow::Borrowed(&values.method_imports),
        );
        let future_flags = inherited_flags | imre_ast::future_flags(&syntax.body);
        let (body, private_names) = match normalized {
            Some(super::LoweredRoot::Complete(body, names)) => (body, names),
            Some(super::LoweredRoot::Unchanged(names)) => (module_source::into_body(parsed), names),
            None => {
                let mut body = module_source::into_body(parsed);
                let names = imre_ast::lower_in(
                    &mut body,
                    "",
                    future_flags & imre_ast::code_flags::FUTURE_ANNOTATIONS != 0,
                );
                (body, names)
            }
        };
        module_source::validate_names(&path, source, &body, &private_names)?;
        let values = crate::frontend::RegisteredSource {
            python_optimization,
            operand_types,
            text: source,
            private_names: Some(&private_names),
            replacements: &replacements,
            method_imports: &method_imports,
            template_fields: Some(&template_fields),
            generators: None,
            lambdas: None,
        };
        let mut aliases = Vec::new();
        let mut lexical = crate::frontend::LexicalOperands {
            expressions: Some(Default::default()),
            ..Default::default()
        };
        let (mut scopes, children) = crate::frontend::function_scope::ScopeFacts::prepare(
            &body,
            &private_names,
            |alias, class| aliases.push((alias, class)),
            |site| lexical.prepare(site, values, private_names.string_annotations),
        );
        let crate::frontend::LexicalOperands {
            generators,
            lambdas,
            expressions,
        } = lexical;
        let values = values.with_children(&lambdas, &generators);
        // Annotation providers use the completed lambda sites for this source unit.
        let (sites, annotation_children) = crate::frontend::registered_sites(
            (&body, children),
            &private_names,
            values,
            bundled,
            |text, parsed| Self::prepare_annotation(text, parsed, python_optimization),
        );
        let operations = sites.operations;
        let mut type_aliases = HashMap::new();
        for (alias, class) in aliases {
            let Some(name) = alias.name.as_name_expr() else {
                continue;
            };
            type_aliases.insert(
                alias.start(),
                TypeAliasDefinition {
                    scope: scopes.type_alias_scope(
                        &alias.value,
                        class,
                        private_names.string_annotations,
                    ),
                    name: private_names.source_name(name.start(), &name.id).to_owned(),
                    body: crate::frontend::type_alias_body(&alias.value, values)
                        .ok_or(alias.value.range()),
                    parameters: alias.type_params.as_deref().map(|params| {
                        crate::frontend::TypeParameterValues::prepare(
                            params,
                            &private_names,
                            values,
                            class,
                        )
                    }),
                },
            );
        }
        let mut prepare =
            crate::frontend::SuitePreparation::new(&operations, values, python_optimization == 0);
        prepare.literal_refusals = exec_literals.has_refusals().then_some(&exec_literals);
        prepare.sites.expressions = expressions.unwrap_or_default();
        // Preparing inner declarations first lets their binding sites consume
        // the selected body's decision in the enclosing suite.
        use crate::frontend::DeclarationSite;
        let mut class_bodies = HashMap::new();
        let python_annotations = origin.python_annotations(name);
        for declaration in sites.declarations.into_iter().rev() {
            let site = match declaration {
                DeclarationSite::Function(site) => site,
                DeclarationSite::Class(site) => {
                    let (start, class) = prepare.class_suite(site, &private_names, &scopes);
                    class_bodies.insert(start, class);
                    continue;
                }
            };
            let start = site.def.start();
            let header = scopes.functions.get_mut(&start).expect("named function");
            prepare.prepare_function(
                header,
                site,
                program_bodies.and_then(|bodies| bodies.get(&start)),
                python_annotations,
            );
        }
        let initializer = if interactive {
            prepare
                .interactive(&body)
                .ok_or("unsupported interactive statement")?
        } else {
            // begin_module binds the leading docstring at its own statement checkpoint.
            let statements = if keep_docstrings {
                ast::helpers::body_without_leading_docstring(&body)
            } else {
                &body
            };
            prepare.suite(statements, crate::frontend::RegisteredScope::Module)
        };
        #[cfg(test)]
        let statement_sites = prepare.sites;
        let source = SourceInfo {
            exec_literals,
            annotation_children,
            scopes,
            #[cfg(test)]
            statement_sites,
            python_optimization,
            lambdas,
            generators,
            #[cfg(test)]
            namespace_operations: operations,
            type_aliases,
            module_annotations: sites.module_annotations.into_body(),
            class_bodies,
            bundled,
            path,
            python_annotations,
            module_start,
            docs,
            #[cfg(test)]
            template_fields,
            #[cfg(test)]
            replacements: replacements.into_owned(),
            #[cfg(test)]
            method_imports,
            future_flags,
            private_names,
            text,
        };
        let docstring = (!interactive && keep_docstrings)
            .then(|| crate::frontend::docstring(&body).or_else(|| source.bundled_docstring("")))
            .flatten();
        drop(body);
        Ok(PreparedBody {
            prompts: &[],
            has_body_statements,
            name,
            #[cfg(test)]
            syntax: None,
            docstring,
            source,
            initializer,
        })
    }

    /// Prepare the decoded expression and its lexical definitions in their own source unit.
    fn prepare_annotation(
        text: &str,
        parsed: &ruff_python_parser::Parsed<ast::ModExpression>,
        python_optimization: i32,
    ) -> Option<crate::frontend::AnnotationBody<Box<SourceInfo<'static>>>> {
        let expr = &parsed.syntax().body;
        let literals = crate::frontend::RegisteredModuleSource::from_tokens(
            text,
            parsed.tokens(),
            Default::default(),
        );
        let fields = literals
            .has_templates
            .then(|| imre_ast::strings::template_fields(text, parsed.tokens(), parsed.syntax()));
        let names = imre_ast::Names::default();
        let values = crate::frontend::RegisteredSource {
            template_fields: fields.as_ref(),
            private_names: Some(&names),
            ..literals.operands(text, python_optimization)
        };
        let mut lexical = crate::frontend::LexicalOperands::default();
        crate::frontend::function_scope::ScopeFacts::collect(
            |visitor| visitor.visit_expr(expr),
            &names,
            |_, _| {},
            |site| lexical.prepare(site, values, false),
        );
        let code = crate::frontend::AnnotationCode::prepare(
            expr,
            values.with_children(&lexical.lambdas, &lexical.generators),
        )?;
        Some(crate::frontend::AnnotationBody {
            code,
            scope: Box::new(SourceInfo {
                path: Cow::Borrowed(Path::new("<string>")),
                text: Cow::Owned(text.to_owned()),
                module_start: expr.start(),
                python_optimization,
                lambdas: lexical.lambdas,
                generators: lexical.generators,
                #[cfg(test)]
                replacements: literals.replacements,
                #[cfg(test)]
                template_fields: fields.unwrap_or_default(),
                ..Default::default()
            }),
        })
    }

    #[cfg(test)]
    fn initializer(&self) -> &crate::frontend::RegisteredSuite {
        &self.initializer
    }

    fn emit(&self, output: &mut Output, unit: usize, file: &str) -> Result<String, String> {
        #[cfg(test)]
        crate::single_emission_tests::record_llvm_start(Some(&self.source.path));
        // Frozen code exposes its origin independently of the module's registered source path.
        let code_file = match (self.source.bundled, self.name) {
            (true, "_frozen_importlib") => output.data(b"<frozen importlib._bootstrap>"),
            (true, "_frozen_importlib_external") => {
                output.data(b"<frozen importlib._bootstrap_external>")
            }
            _ => file.to_owned(),
        };
        let positions = SourcePositions::new(&self.source.path, &self.source.text, &code_file);
        let location = SourceMap {
            info: &self.source,
            callable_code: Default::default(),
            exec_code: Default::default(),
            unit,
            positions: &positions,
        };
        let code = location.code(output, "", Body::Module(self), false, None);
        match output.diagnostic.take() {
            Some(diagnostic) => Err(diagnostic),
            None => Ok(code),
        }
    }

    #[cfg(test)]
    pub(super) fn standalone(&self) -> Result<(String, String), String> {
        let (symbol, body) = self.provider_body(None)?;
        let ir = body.llvm_ir(&symbol, "");
        Ok((symbol, ir))
    }

    /// Hash the emitted module before deciding whether its native object is already available.
    pub(super) fn provider_body(
        &self,
        mut placement: Option<(&mut Output, usize, &mut String)>,
    ) -> Result<(String, ProviderBody), String> {
        let mut output = Output {
            keep_docstrings: self.source.python_optimization < 2,
            ..Output::default()
        };
        if let Some((target, unit, _)) = &mut placement {
            output.begin_placement(target, *unit);
        }
        let filename = self.source.path.as_os_str().as_encoded_bytes();
        let file = output.data(filename);
        let mut entry = self.emit(&mut output, 0, &file)?;
        if let Some((target, _, whole_entry)) = placement {
            *whole_entry = Output::whole_operand(&mut entry);
            output.finish_placement(target);
        }
        let body = ProviderBody::new(output, entry);
        let mut hash = blake3::Hasher::new();
        for field in [
            std::env::consts::ARCH.as_bytes(),
            self.name.as_bytes(),
            filename,
            self.source.text.as_bytes(),
        ] {
            hash.update(&(field.len() as u64).to_le_bytes());
            hash.update(field);
        }
        let parts = body.parts("");
        let length: usize = parts.iter().map(|part| part.len()).sum();
        hash.update(&(length as u64).to_le_bytes());
        for part in parts {
            hash.update(part.as_bytes());
        }
        hash.update(&[u8::from(self.source.python_optimization < 2)]);
        let symbol = format!("imre_provider_{}", hash.finalize().to_hex());
        Ok((symbol, body))
    }
}

enum DefinitionName<'a> {
    Binding(&'a ast::Identifier),
    Evaluator {
        name: &'a str,
        scope: &'a crate::frontend::function_scope::FunctionScope,
        type_parameter_scope: bool,
    },
    ClassHeader(&'a str, &'a crate::frontend::RegisteredClass),
}

enum Body<'a> {
    Module(&'a PreparedBody<'a>),
    Namespace(
        &'a crate::frontend::RegisteredClass,
        &'a crate::frontend::function_scope::classes::ClassScope,
    ),
    CheckedProvider(&'a crate::frontend::CheckedProvider, AnnotationScope<'a>),
    /// The checked body borrows its declaring callable's bindings during emission.
    Registered(RegisteredBody<'a>, &'a Callable<'a>),
    Operand(
        TextRange,
        &'a crate::frontend::RegisteredOperand,
        Option<&'a ClassBindings>,
    ),
}

struct SourceMap<'a> {
    info: &'a SourceInfo<'a>,
    /// A duplicated cleanup path still evaluates the same lexical definition.
    callable_code: std::cell::RefCell<HashMap<ruff_text_size::TextSize, String>>,
    exec_code: std::cell::RefCell<HashMap<TextRange, sites::CompiledExec>>,
    unit: usize,
    positions: &'a SourcePositions<'a>,
}

impl<'a> std::ops::Deref for SourceMap<'a> {
    type Target = SourceInfo<'a>;

    fn deref(&self) -> &Self::Target {
        self.info
    }
}

struct TypeAliasDefinition {
    scope: crate::frontend::function_scope::FunctionScope,
    name: String,
    body: Result<crate::frontend::AnnotationCode, TextRange>,
    parameters: Option<crate::frontend::TypeParameterValues>,
}

#[cfg(test)]
impl<'a> RegisteredBody<'a> {
    fn fully_checked(&self) -> bool {
        match self {
            Self::Function(_) => true,
            Self::Suite(suite) => suite.fully_checked(),
        }
    }

    fn checked(&self) -> &'a crate::frontend::RegisteredFunction {
        match self {
            Self::Function(checked) => checked,
            Self::Suite(suite) => &suite.checked,
        }
    }
}

#[derive(Default)]
struct SourceInfo<'a> {
    exec_literals: LiteralSites,
    annotation_children:
        HashMap<TextSize, crate::frontend::AnnotationBody<Box<SourceInfo<'static>>>>,
    scopes: crate::frontend::function_scope::ScopeFacts<'a>,
    lambdas: HashMap<ruff_text_size::TextSize, RegisteredLambda>,
    generators: HashMap<TextRange, RegisteredGenerator>,
    type_aliases: HashMap<ruff_text_size::TextSize, TypeAliasDefinition>,
    python_optimization: i32,
    #[cfg(test)]
    statement_sites: crate::frontend::StatementSites,
    #[cfg(test)]
    namespace_operations: HashMap<ruff_text_size::TextRange, crate::ir::Expr>,
    module_annotations: Option<crate::frontend::AnnotationProvider>,
    class_bodies: HashMap<ruff_text_size::TextSize, crate::frontend::RegisteredClass>,
    bundled: bool,
    path: Cow<'a, Path>,
    python_annotations: bool,
    module_start: ruff_text_size::TextSize,
    docs: Option<crate::modules::doctest::SealedDocs>,
    #[cfg(test)]
    template_fields: HashMap<TextSize, imre_ast::strings::TemplateFieldText>,
    #[cfg(test)]
    replacements: HashMap<ruff_text_size::TextRange, Vec<u32>>,
    private_names: imre_ast::Names,
    #[cfg(test)]
    method_imports: Cow<'a, std::collections::BTreeSet<String>>,
    future_flags: u32,
    text: Cow<'a, str>,
}

impl SourceInfo<'_> {
    fn literal_diagnostic(&self, site: TextSize, reason: &str) -> String {
        crate::frontend::phase::Diagnostic::at(&self.text, &TextRange::empty(site), reason)
            .with_path(&self.path)
            .to_string()
    }

    /// Keep documentation attached to the bundled definition, even when callers rebind its name.
    fn bundled_docstring(&self, qualname: &str) -> Option<String> {
        self.docs
            .as_ref()?
            .object_docstring(qualname)
            .map(str::to_owned)
    }

    #[cfg(test)]
    fn registered_source(&self) -> crate::frontend::RegisteredSource<'_> {
        crate::frontend::RegisteredSource {
            python_optimization: self.python_optimization,
            operand_types: None,
            text: &self.text,
            replacements: &self.replacements,
            template_fields: Some(&self.template_fields),
            lambdas: Some(&self.lambdas),
            generators: Some(&self.generators),
            method_imports: &self.method_imports,
            private_names: Some(&self.private_names),
        }
    }
}

impl SourceInfo<'_> {
    fn function_flags(&self, nested: bool, method: bool) -> u32 {
        use imre_ast::code_flags;
        code_flags::function_flags(None, nested, method) | self.future_flags
    }
}

impl SourceMap<'_> {
    fn function<'a, 'p, 'scope>(
        &'p self,
        frame: RegisteredFrame<'a, 'p>,
        declared_globals: &'scope BTreeSet<String>,
        prefix: &'scope str,
        in_function: bool,
    ) -> Function<'a, 'p, 'scope> {
        Function {
            sites: CheckedSites::default(),
            frame,
            source: self,
            in_function,
            declared_globals,
            prefix,
        }
    }

    fn code(
        &self,
        output: &mut Output,
        prefix: &str,
        body: Body<'_>,
        in_function: bool,
        definition: Option<(TextSize, u32)>,
    ) -> String {
        let suspension = definition.map_or(0, |(_, flags)| flags);
        let start = definition.map(|(start, _)| start);
        let mut frame = RegisteredFrame::new(
            output,
            self.positions,
            suspension != 0,
            self.unit,
            self.module_start,
        );
        frame.async_generator = suspension == imre_ast::code_flags::ASYNC_GENERATOR;
        let empty = BTreeSet::new();
        let declared_globals = match &body {
            Body::Registered(_, Callable::Named(header)) => {
                frame.function_locals = header.metadata.local_names();
                &header.globals
            }
            Body::Registered(_, Callable::Generator(generator)) => {
                frame.function_locals = generator.metadata.local_names();
                &empty
            }
            Body::Module(_) => &self.scopes.module_globals,
            Body::Namespace(_, class) => &class.bindings.globals,
            _ => &empty,
        };
        let mut function = self.function(frame, declared_globals, prefix, in_function);
        match body {
            Body::Module(module) => {
                let frame = &mut function.frame;
                if let Some(annotations) = &self.module_annotations {
                    self.define_module_annotations(frame, annotations);
                }
                frame.begin_module(
                    self.scopes.module_annotations,
                    self.private_names.string_annotations,
                    module.docstring.as_deref(),
                );
                if !module.prompts.is_empty() {
                    let name = frame.code_name(module.name);
                    frame.op("bind_doctest_module", &[&frame.scope.clone(), &name]);
                }
                frame.declare_globals(declared_globals);
                RegisteredBody::Suite(&module.initializer).emit(&mut function);
                return function.frame.finish_module();
            }
            Body::Registered(body, _) => {
                function.frame.declare_globals(function.declared_globals);
                body.emit(&mut function);
            }
            Body::Namespace(prepared, class) => {
                let frame = &mut function.frame;
                frame.class_locals = Some(&class.bindings);
                if frame.output.keep_docstrings
                    && let Some(doc) = prepared.doc.as_deref().or_else(|| {
                        self.docs
                            .as_ref()?
                            .object_docstring(prefix.trim_end_matches('.'))
                    })
                {
                    let value = frame.text(doc);
                    frame.bind("__doc__", &value);
                }
                let globals = &class.bindings.globals;
                frame.declare_globals(globals);
                RegisteredBody::Suite(&prepared.body).emit(&mut function);
                self.finish_class_annotations(
                    &mut function.frame,
                    prefix,
                    in_function,
                    &prepared.annotations,
                    &class.bindings,
                    start.expect("class definition site"),
                );
            }
            Body::CheckedProvider(provider, scope) => {
                function.frame.annotation_scope = scope;
                match provider {
                    Ok(checked) => RegisteredBody::Function(checked).emit(&mut function),
                    Err((range, reason)) => {
                        function.frame.refuse_at(*range, reason);
                    }
                }
            }
            Body::Operand(range, operand, class) => {
                let frame = &mut function.frame;
                frame.current_range = range;
                frame.checkpoint(range);
                if let Some(class) = class {
                    frame.use_class_annotation_names(class);
                }
                let value = crate::llvm::registered::Frame::evaluate_operand(
                    &mut function,
                    &operand.value,
                    Some(operand),
                    false,
                );
                function.frame.return_value(&value);
            }
        }
        function.frame.finish_callable(start)
    }
}

/// Lexical inputs for compiling the children of one registered frame.
struct LexicalDefinitions<'a, 'p> {
    source: &'a SourceMap<'p>,
    output: &'a mut Output,
    prefix: &'a str,
    declared_globals: &'a std::collections::BTreeSet<String>,
    in_function: bool,
    method_scope: bool,
    class_locals: Option<&'a crate::frontend::function_scope::ClassBindings>,
}

enum Callable<'a> {
    Lambda(&'a RegisteredLambda, bool),
    Named(&'a NamedFunction<'a>),
    Generator(&'a RegisteredGenerator),
}

impl<'p> SourceMap<'p> {
    fn definitions<'a>(
        &'a self,
        frame: &'a mut RegisteredFrame<'_, '_>,
        prefix: &'a str,
        declared_globals: &'a std::collections::BTreeSet<String>,
        in_function: bool,
    ) -> LexicalDefinitions<'a, 'p> {
        LexicalDefinitions {
            source: self,
            output: frame.output,
            prefix,
            declared_globals,
            in_function,
            class_locals: frame.class_locals,
            method_scope: frame.class_locals.is_some() && frame.annotation_values.is_none(),
        }
    }
}

impl Function<'_, '_, '_> {
    fn definitions(&mut self) -> (&mut CheckedSites, LexicalDefinitions<'_, '_>) {
        let definitions = self.source.definitions(
            &mut self.frame,
            self.prefix,
            self.declared_globals,
            self.in_function,
        );
        (&mut self.sites, definitions)
    }
}

impl<'a, 'p> crate::llvm::registered::PreparedSuiteEmitter<'a, 'p> for Function<'a, 'p, '_> {
    fn frame_and_sites(&mut self) -> (&mut RegisteredFrame<'a, 'p>, &CheckedSites) {
        (&mut self.frame, &self.sites)
    }

    fn place_site(&mut self, site: RequiredSite) {
        let (sites, mut definitions) = self.definitions();
        sites.place(site, &mut definitions);
    }

    fn lambda_at(&mut self, site: TextSize, comprehension: bool) -> CompiledDefinition {
        let lambda = &self.source.lambdas[&site];
        let (_, mut definitions) = self.definitions();
        definitions.compile_callable(site, Callable::Lambda(lambda, comprehension))
    }

    fn exec_literal_at(&mut self, site: TextRange) -> Option<sites::CompiledExec> {
        let child = self.source.exec_literals.body(site)?;
        let output = &mut self.frame.output;
        if output.diagnostic.is_some() {
            return None;
        }
        let mut entries = self.source.exec_code.borrow_mut();
        let entry = match entries.entry(site) {
            std::collections::hash_map::Entry::Occupied(entry) => return Some(entry.get().clone()),
            std::collections::hash_map::Entry::Vacant(entry) => entry,
        };
        let child = match child {
            Ok(child) => child,
            Err((operand, reason)) => {
                output.diagnostic = Some(self.source.literal_diagnostic(operand, reason));
                return None;
            }
        };
        let file = output.data(b"<string>");
        // A decoded module interns its own constants when its prologue runs.
        let parent_names = std::mem::take(&mut output.interned_identifiers);
        let compiled = child.body.emit(output, self.source.unit, &file);
        output.interned_identifiers = parent_names;
        let code = match compiled {
            Ok(code) => code,
            Err(error) => {
                output.diagnostic = Some(error);
                return None;
            }
        };
        let text = child
            .original_text
            .as_deref()
            .unwrap_or(&child.body.source.text);
        let compiled = sites::CompiledExec {
            entry: code,
            flags: child.body.source.future_flags,
            source: output.data(text.as_bytes()),
        };
        Some(entry.insert(compiled).clone())
    }
}

impl sites::SiteDefinitions for LexicalDefinitions<'_, '_> {
    fn declaration(&mut self, site: TextSize) -> Declaration {
        let source = self.source;
        let (definition, decorator_offsets, type_params) =
            if let Some(class) = source.class_bodies.get(&site) {
                let definition = if let Some(header) = &class.header {
                    let name = format!("<generic parameters of {}>", class.name);
                    let body = Body::CheckedProvider(&header.body, AnnotationScope::Plain);
                    self.compile_definition(site, DefinitionName::ClassHeader(&name, class), body)
                } else {
                    self.compile_class_namespace(site, class)
                };
                (definition, &class.decorator_offsets, None)
            } else {
                let header = &source.scopes.functions[&site];
                let definition = self.compile_callable(site, Callable::Named(header));
                let parameters = (!header.type_params.is_empty())
                    .then(|| self.compile_type_parameters(&header.type_params, self.class_locals));
                (definition, &header.decorator_offsets, parameters)
            };
        Declaration {
            definition,
            decorator_offsets: decorator_offsets.clone(),
            type_params,
        }
    }

    fn class_namespace(
        &mut self,
        site: TextSize,
    ) -> (CompiledDefinition, CompiledTypeParameters, TextRange) {
        let class = &self.source.class_bodies[&site];
        let namespace = self.compile_class_namespace(site, class);
        let header = class.header.as_ref().expect("generic class");
        let parameters = self.compile_scoped_type_parameters(&header.parameters);
        (namespace, parameters, class.range)
    }

    fn generator(&mut self, site: TextRange) -> CompiledDefinition {
        let body = &self.source.generators[&site];
        self.compile_callable(site.start(), Callable::Generator(body))
    }

    fn type_alias(&mut self, site: TextSize) -> Result<CompiledAlias, String> {
        let alias = &self.source.type_aliases[&site];
        // Placement can visit children of a suite that exits before the alias binds.
        let outer_diagnostic = self.output.diagnostic.take();
        let compiled = match &alias.body {
            Ok(code) => Some(CompiledAlias {
                definition: self.compile_evaluator(
                    site,
                    &alias.name,
                    &alias.scope,
                    alias.parameters.is_some(),
                    code,
                    self.class_locals,
                ),
                parameters: alias
                    .parameters
                    .as_ref()
                    .map(|parameters| self.compile_scoped_type_parameters(parameters)),
            }),
            Err(range) => {
                self.output.diagnostic.get_or_insert_with(|| {
                    self.source
                        .positions
                        .diagnostic_at(*range, "type alias value has no checked expression operand")
                });
                None
            }
        };
        match std::mem::replace(&mut self.output.diagnostic, outer_diagnostic) {
            Some(error) => Err(error),
            None => Ok(compiled.expect("prepared alias evaluator")),
        }
    }

    fn annotation_string(&mut self, site: TextSize) -> String {
        let body = &self.source.annotation_children[&site];
        let text = body.scope.text.as_ref();
        let file = self.output.data(b"<string>");
        let location = SourceMap {
            info: &body.scope,
            callable_code: Default::default(),
            exec_code: Default::default(),
            unit: self.source.unit,
            positions: &SourcePositions::new(&body.scope.path, text, &file),
        };
        location.code(
            self.output,
            "",
            Body::Operand(body.code.range, &body.code.operand, None),
            false,
            None,
        )
    }
}

impl LexicalDefinitions<'_, '_> {
    fn compile_callable(&mut self, site: TextSize, callable: Callable<'_>) -> CompiledDefinition {
        let source = self.source;
        let (name, metadata, body, header) = match callable {
            Callable::Named(header) => (
                header.name.as_str(),
                &header.metadata,
                header.body(),
                Some(header),
            ),
            Callable::Lambda(lambda, _) => (
                "<lambda>",
                &lambda.metadata,
                RegisteredBody::Function(lambda.body.as_ref().expect("checked lambda body")),
                None,
            ),
            Callable::Generator(generator) => (
                "<genexpr>",
                &generator.metadata,
                RegisteredBody::Function(&generator.body),
                None,
            ),
        };
        let qualname = if header.is_some_and(|h| self.declared_globals.contains(&h.binding)) {
            name.to_owned()
        } else {
            format!("{}{name}", self.prefix)
        };
        let doc = header
            .filter(|_| self.output.keep_docstrings)
            .and_then(|h| {
                h.doc
                    .clone()
                    .or_else(|| source.bundled_docstring(&qualname))
            });
        let provider = header.and_then(|h| self.parameter_provider(h, &qualname));
        let generic = header.is_some_and(|h| !h.type_params.is_empty());
        let suspension = metadata.suspension_flags();
        // Several expression regions can place the same lexical lambda.
        let cache = match callable {
            Callable::Lambda(..) => Some(&source.callable_code),
            Callable::Named(_) | Callable::Generator(_) => None,
        };
        let existing = cache.and_then(|cache| cache.borrow().get(&site).cloned());
        let code = existing.unwrap_or_else(|| {
            let prefix = if matches!(callable, Callable::Generator(_)) {
                format!("{qualname}.")
            } else {
                format!("{qualname}.<locals>.")
            };
            let code = source.code(
                self.output,
                &prefix,
                Body::Registered(body, &callable),
                true,
                Some((site, suspension)),
            );
            if let Some(cache) = cache {
                cache.borrow_mut().insert(site, code.clone());
            }
            code
        });
        // A lambda inside a display captures the iteration namespace even in a class body.
        let comprehension = matches!(callable, Callable::Lambda(_, true));
        let direct_method = self.method_scope && !comprehension && !generic;
        let mut flags =
            source.function_flags(self.in_function || comprehension || generic, direct_method);
        if doc.is_some() {
            flags |= imre_ast::code_flags::HAS_DOCSTRING;
        }
        let signature = metadata.signature(flags, direct_method);
        if !matches!(callable, Callable::Generator(_)) {
            self.output
                .interned_identifiers
                .extend(metadata.parameter_names().map(str::to_owned));
        }
        CompiledDefinition {
            doc,
            provider,
            default_names: metadata.defaults.clone(),
            ..CompiledDefinition::new(code, signature, name, qualname, false)
        }
    }

    fn compile_class_namespace(
        &mut self,
        site: TextSize,
        class: &crate::frontend::RegisteredClass,
    ) -> CompiledDefinition {
        self.compile_definition(
            site,
            DefinitionName::Binding(&class.name),
            Body::Namespace(class, &self.source.scopes.classes[&site]),
        )
    }

    fn compile_definition(
        &mut self,
        start: TextSize,
        name: DefinitionName<'_>,
        body: Body<'_>,
    ) -> CompiledDefinition {
        let (evaluator_scope, type_parameter_scope) = match name {
            DefinitionName::Evaluator {
                scope,
                type_parameter_scope,
                ..
            } => (Some(scope), type_parameter_scope),
            _ => (None, false),
        };
        let class = match name {
            DefinitionName::ClassHeader(_, class) => Some(class),
            _ => None,
        };
        let (name, global) = match name {
            DefinitionName::Binding(identifier) => (
                self.source.private_names.definition(identifier),
                self.declared_globals.contains(identifier.as_str()),
            ),
            DefinitionName::ClassHeader(name, _) | DefinitionName::Evaluator { name, .. } => {
                (name, false)
            }
        };
        let qualname = if global {
            name.to_owned()
        } else {
            format!("{}{name}", self.prefix)
        };
        let class_header = class.map(|class| class.range);
        let prefix = if let Some(class) = class {
            if self.declared_globals.contains(class.name.as_str()) {
                String::new()
            } else {
                self.prefix.to_owned()
            }
        } else if matches!(body, Body::Namespace(..)) {
            format!("{qualname}.")
        } else {
            format!("{qualname}.<locals>.")
        };
        let mut signature = json!({});
        if !matches!(body, Body::Namespace(..)) {
            signature["code_flags"] = json!(
                self.source
                    .function_flags(self.in_function || type_parameter_scope, false)
            );
        }
        if let Body::Namespace(class, scope) = &body {
            signature["postponed_annotations"] = json!(class.annotations.postponed);
            if class.annotations.conditional_count != 0 || scope.cells.conditional {
                signature["conditional_annotations"] = json!(true);
            }
            scope.write_signature(&mut signature);
            if self.method_scope {
                // A nested class body closes over its defining class's cells just like a method.
                signature["method_class_cell"] = json!(scope.frame_reads.contains("__class__"));
            }
        } else if let Some(facts) = evaluator_scope {
            facts.write_signature(&mut signature);
        } else {
            signature["scope_cellvars"] = serde_json::Value::Null;
            signature["scope_complete"] = json!(false);
            signature["scope_locals_complete"] = json!(true);
        }
        if evaluator_scope.is_some() {
            signature["posonly"] = json!([".format"]);
            if matches!(body, Body::Operand(_, _, Some(_))) {
                signature["annotation_class"] = json!(true);
            }
        }
        let in_function = self.in_function || !matches!(body, Body::Namespace(..));
        let code = self
            .source
            .code(self.output, &prefix, body, in_function, Some((start, 0)));
        CompiledDefinition {
            class_header,
            format_evaluator: evaluator_scope.is_some(),
            ..CompiledDefinition::new(
                code,
                signature,
                name,
                qualname,
                class_header.is_some() || evaluator_scope.is_some(),
            )
        }
    }

    fn compile_evaluator(
        &mut self,
        site: TextSize,
        name: &str,
        scope: &crate::frontend::function_scope::FunctionScope,
        type_parameter_scope: bool,
        code: &crate::frontend::AnnotationCode,
        class: Option<&ClassBindings>,
    ) -> CompiledDefinition {
        self.compile_definition(
            site,
            DefinitionName::Evaluator {
                name,
                scope,
                type_parameter_scope,
            },
            Body::Operand(code.range, &code.operand, class),
        )
    }

    fn compile_scoped_type_parameters(
        &mut self,
        parameters: &crate::frontend::TypeParameterValues,
    ) -> CompiledTypeParameters {
        let class = parameters
            .enclosing_class
            .map(|site| &self.source.scopes.classes[&site].bindings);
        self.compile_type_parameters(&parameters.declarations, class)
    }

    fn compile_type_parameters(
        &mut self,
        parameters: &[RegisteredTypeParameter<ParameterEvaluator>],
        class: Option<&ClassBindings>,
    ) -> CompiledTypeParameters {
        parameters
            .iter()
            .map(|parameter| {
                parameter.try_map_fields(|provider| {
                    let body = provider.body.as_ref().ok_or_else(|| {
                        self.source.positions.diagnostic_at(
                            provider.range,
                            "type parameter evaluator has no checked expression operand",
                        )
                    })?;
                    Ok(self.compile_evaluator(
                        body.range.start(),
                        &parameter.name,
                        &provider.scope(class),
                        true,
                        body,
                        class,
                    ))
                })
            })
            .collect()
    }

    fn parameter_provider(
        &mut self,
        header: &NamedFunction<'_>,
        qualname: &str,
    ) -> Option<Box<CompiledDefinition>> {
        let source = self.source;
        let annotations = header.annotations.as_ref()?;
        let start = annotations.start;
        let class = self
            .class_locals
            .filter(|_| !source.private_names.string_annotations);
        let reads = annotations.reads(class);
        let mut metadata = json!({
            "posonly": ["format"],
            "code_flags": source.function_flags(self.in_function || !header.type_params.is_empty(), false),
            "scope_reads": reads,
            "scope_complete": true,
            "scope_locals_complete": true,
            "annotation_class": class.is_some(),
        });
        if !source.python_annotations && header.type_params.is_empty() {
            for (slot, reference) in &annotations.references {
                let root = reference.split('.').next().unwrap();
                if !class.is_some_and(|class| class.globals.contains(root)) {
                    metadata["annotation_references"][slot] = json!(reference);
                }
            }
        }
        let scope = class.map_or(AnnotationScope::Plain, AnnotationScope::Method);
        Some(Box::new(source.annotation_provider(
            self.output,
            &annotations.body,
            scope,
            start,
            metadata,
            (self.prefix, format!("{qualname}.__annotate__")),
        )))
    }
}

impl SourceMap<'_> {
    fn finish_class_annotations(
        &self,
        frame: &mut RegisteredFrame<'_, '_>,
        prefix: &str,
        in_function: bool,
        plan: &crate::frontend::ClassAnnotations,
        bindings: &ClassBindings,
        class_start: TextSize,
    ) {
        let Some(body) = &plan.body else {
            return;
        };
        if frame.body.terminated {
            return;
        }
        let signature = json!({
            "posonly": ["format"], "scope_complete": !plan.incomplete, "scope_reads": plan.reads,
            "code_flags": self.function_flags(in_function, false),
        });
        let scope = AnnotationScope::Class {
            bindings,
            conditional: plan.conditional_count != 0,
        };
        let compiled = self.annotation_provider(
            frame.output,
            body,
            scope,
            class_start,
            signature,
            (prefix, format!("{prefix}__annotate__")),
        );
        let environment = frame.op("class_annotation_environment", &[&frame.scope.clone()]);
        let function = compiled.bind_defaults(frame, &environment, &[]);
        frame.bind("__annotate_func__", &function);
    }

    fn annotation_provider(
        &self,
        output: &mut Output,
        body: &crate::frontend::AnnotationProvider,
        scope: AnnotationScope<'_>,
        start: TextSize,
        signature: serde_json::Value,
        (lexical_prefix, qualname): (&str, String),
    ) -> CompiledDefinition {
        // Class annotations already have an explicit class environment. A method's provider
        // instead captures the class namespace of the function it annotates.
        let annotation_scope = !matches!(&scope, AnnotationScope::Class { .. });
        // Annotation expressions keep the enclosing name and run inside the provider function.
        let code = self.code(
            output,
            lexical_prefix,
            Body::CheckedProvider(body, scope),
            true,
            Some((start, 0)),
        );
        CompiledDefinition::new(code, signature, "__annotate__", qualname, annotation_scope)
    }

    fn define_module_annotations(
        &self,
        frame: &mut RegisteredFrame<'_, '_>,
        body: &crate::frontend::AnnotationProvider,
    ) {
        let compiled = self.annotation_provider(
            frame.output,
            body,
            AnnotationScope::Plain,
            self.module_start,
            json!({
                "posonly": ["format"], "scope_complete": true, "scope_locals_complete": true,
                "code_flags": self.function_flags(false, false),
            }),
            ("", "__annotate__".into()),
        );
        let scope = frame.scope.clone();
        let provider = compiled.bind_defaults(frame, &scope, &[]);
        frame.bind("__annotate__", &provider);
    }
}

struct Function<'a, 'p, 'scope> {
    sites: sites::CheckedSites,
    frame: RegisteredFrame<'a, 'p>,
    source: &'p SourceMap<'p>,
    prefix: &'scope str,
    declared_globals: &'scope BTreeSet<String>,
    in_function: bool,
}

/// A prepared suite uses the activation's roots, exception state and lexical exits.
impl<'a, 'p> std::ops::Deref for Function<'a, 'p, '_> {
    type Target = RegisteredFrame<'a, 'p>;
    fn deref(&self) -> &Self::Target {
        &self.frame
    }
}

impl std::ops::DerefMut for Function<'_, '_, '_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.frame
    }
}

#[cfg(test)]
#[path = "tests"]
mod tests {
    mod declaration_sites;
    use super::super::prepare::{self, ModuleSource};
    use super::*;
    use crate::frontend::{FrameArguments, NamedBody};
    use crate::llvm::registered::permanent;

    #[test]
    fn ordinary_doctest_suites_place_owned_inputs_and_keep_the_fixed_runner() {
        let directory = tempfile::tempdir().unwrap();
        let entry = directory.path().join("app.py");
        let run = "doctest.DocTestSuite(subject).debug()";
        for (caller, second, prompt) in [
            (run.to_owned(), "record()", "record()\n"),
            (run.to_owned(), "if True:\n...     3", "if True:\n    3\n"),
            (format!("def later():\n    {run}"), "record()", "record()\n"),
            (format!("if False:\n    {run}"), "record()", "record()\n"),
        ] {
            std::fs::write(
                &entry,
                format!("import doctest\nimport subject\n{caller}\n"),
            )
            .unwrap();
            std::fs::write(
                directory.path().join("subject.py"),
                format!("\"\"\"\n>>> 3\n3\n>>> {second}\n\"\"\"\n"),
            )
            .unwrap();
            let graph =
                crate::modules::SourceGraph::load(&entry, Some(directory.path()), &[]).unwrap();
            let subject = graph.modules.iter().find(|m| m.name == "subject").unwrap();
            let inputs: Vec<_> = subject
                .doctest_prompts
                .iter()
                .map(|Prompt(body)| (body.source.path.as_ref(), body.source.text.as_ref()))
                .collect();
            assert_eq!(
                inputs,
                [
                    (Path::new("<doctest subject[0]>"), "3\n"),
                    (Path::new("<doctest subject[1]>"), prompt)
                ]
            );
            assert!(graph.doctest_runner().is_some());
            assert!(graph.modules.iter().any(|m| m.is_doctest_case()));
        }
    }

    #[test]
    fn interactive_prompts_use_checked_operands_and_display_only_expressions() {
        use crate::ir::{Builtin, ExprKind, Statement};
        let path = Path::new("<doctest example[0]>");
        for (text, display) in [
            ("3\n", true),
            ("record()\n", true),
            ("None\n", true),
            ("'prompt string'\n", true),
            ("saved = record()\n", false),
            ("raise problem\n", false),
            ("def f():\n    return 3\n", false),
            (
                "def outer(value):\n    def read():\n        return value\n    return read\n",
                false,
            ),
            ("class C:\n    x = 3\n", false),
            ("type T = int\n", false),
        ] {
            let prepared = PreparedBody::interactive(path, text, 0, 0).unwrap();
            assert!(prepared.syntax.is_none());
            assert!(prepared.docstring.is_none());
            let suite = prepared.initializer();
            assert!(suite.fully_checked());
            crate::verify::registered(suite.checked.region()).unwrap();
            let displays = suite.checked.function.body.iter().any(|stmt| {
                let Statement::Expr(value) = stmt else {
                    return false;
                };
                let ExprKind::Builtin(Builtin::RegisteredDisplayHook, args) = &value.kind else {
                    return false;
                };
                let [operand] = &args[..] else {
                    panic!("displayhook needs its expression")
                };
                let ExprKind::Builtin(Builtin::RegisteredLocation(range, ..), _) = &operand.kind
                else {
                    panic!("displayed operand lost its source position")
                };
                assert_eq!(&text[*range], text.trim_end());
                true
            });
            assert_eq!(displays, display);
            prepared.standalone().unwrap();
        }
        assert!(PreparedBody::interactive(path, "1\n2\n", 0, 0).is_none());
        assert!(PreparedBody::interactive(path, "return 1\n", 0, 0).is_none());
        assert!(PreparedBody::interactive(path, "def f():\n    '\\ud800'\n", 0, 0).is_none());
    }

    #[test]
    fn interactive_compound_suites_display_only_their_own_expressions() {
        use crate::ir::{Builtin, Expr, ExprKind, Node, Statement};

        let path = Path::new("<doctest sample[4]>");
        let text = "try:\n    for item in (\n        source\n    ):\n        saved = item; print(saved); None\n        def quiet():\n            record(item)\n        class C:\n            record(item)\n    else:\n        exhausted()\nexcept Error:\n    'handled'\nelse:\n    del item\nfinally:\n    done()\n";
        let prepared = PreparedBody::interactive(path, text, 0, 0).unwrap();
        let suite = prepared.initializer();
        assert!(suite.fully_checked());
        crate::verify::registered(suite.checked.region()).unwrap();
        let mut displayed = Vec::new();
        let mut checkpoints = Vec::new();
        crate::ir::walk_nodes(&suite.checked.function.body, &mut |node, _| match node {
            Node::Expression(Expr {
                kind: ExprKind::Builtin(Builtin::RegisteredDisplayHook, args),
                ..
            }) => {
                let ExprKind::Builtin(Builtin::RegisteredLocation(range, ..), _) = &args[0].kind
                else {
                    panic!("displayed operand lost its source position")
                };
                displayed.push(&text[*range]);
            }
            Node::Statement(Statement::Position(range, _)) => {
                checkpoints.push(text[*range].lines().next().unwrap());
            }
            _ => {}
        });
        assert_eq!(
            displayed,
            ["print(saved)", "None", "exhausted()", "'handled'", "done()"]
        );
        assert_eq!(
            checkpoints,
            [
                "try:",
                "source",
                "saved = item",
                "print(saved)",
                "None",
                "def quiet():",
                "class C:",
                "exhausted()",
                "'handled'",
                "del item",
                "done()",
            ]
        );
        prepared.standalone().unwrap();
    }

    fn emit_modules(
        (files, builtins, entry_source): (
            &[ModuleSource],
            &[ModuleSource],
            Option<ParsedModule<'_, '_>>,
        ),
        config: &str,
        python_optimization: i32,
        bundled_root: &Path,
        providers: &HashSet<String>,
        layout: crate::CodegenLayout,
        (module_errors, registrations): (
            &HashMap<PathBuf, Box<super::super::ModuleSyntaxError>>,
            &RegisteredBodies<'_>,
        ),
    ) -> Result<(Vec<crate::CodegenUnit>, bool, Option<String>), String> {
        let units = prepare::units(
            (files, builtins),
            prepare::SourceInputs {
                entry_source,
                ..Default::default()
            },
            python_optimization,
            bundled_root,
            module_errors,
            registrations,
            Vec::new(),
        );
        let mut output = crate::ProgramOutput::new(layout);
        let used_archive =
            output.emit_registered(units, None, config, python_optimization, providers)?;
        Ok((output.compilation.units, used_archive, output.whole))
    }

    #[test]
    fn literal_exec_sites_own_distinct_module_bodies_at_the_same_python_location() {
        let text = concat!(
            "def install(scope):\n",
            "    exec('class Entry: value = 1', scope)\n",
            "    code = 'class Entry: value = 2'\n",
            "    alias = code\n",
            "    exec(alias, scope)\n",
            "install({'__name__': 'app'})\n",
        );
        let ir = crate::compile(text, &crate::CompileOptions::default()).unwrap();
        let entries: Vec<_> = ir
            .lines()
            .filter(|line| line.contains("call i64 @imre_dyn_exec_operand("))
            .map(|line| {
                line.rsplit_once("ptr @code")
                    .unwrap()
                    .1
                    .split(',')
                    .next()
                    .unwrap()
            })
            .collect();
        assert_eq!(entries.len(), 2);
        assert_ne!(entries[0], entries[1]);
    }

    /// Plan the consuming suites without their lambda descriptors. The physical
    /// emitter still has the real, checked children when it reads a residual operand.
    fn inject_residual_child_module(prepared: &mut PreparedBody) {
        use crate::frontend::{RegisteredScope, RegisteredSource, SuitePreparation};

        let logicals = prepared.source.statement_sites.expressions.take_lexical();
        if prepared.syntax.is_none() {
            let parsed = ruff_python_parser::parse_module(&prepared.source.text).unwrap();
            prepared.syntax = Some(Cow::Owned(parsed.into_syntax().body));
        }
        let body = prepared.syntax.as_deref().unwrap();
        let missing = HashMap::new();
        let source = RegisteredSource {
            lambdas: Some(&missing),
            ..prepared.source.registered_source()
        };
        let mut lexical = crate::frontend::LexicalOperands::default();
        let (_, children) = crate::frontend::function_scope::ScopeFacts::prepare(
            body,
            &prepared.source.private_names,
            |_, _| {},
            |site| {
                if matches!(
                    site,
                    crate::frontend::function_scope::LexicalSite::Generator(..)
                ) {
                    lexical.prepare(
                        site,
                        source,
                        prepared.source.private_names.string_annotations,
                    );
                }
            },
        );
        let generators = lexical.generators;
        let source = RegisteredSource {
            generators: Some(&generators),
            ..source
        };
        let classes = crate::frontend::registered_sites(
            (body, children),
            &prepared.source.private_names,
            source,
            prepared.source.bundled,
            |text, parsed| {
                PreparedBody::prepare_annotation(text, parsed, prepared.source.python_optimization)
            },
        )
        .0
        .declarations
        .into_iter()
        .filter_map(|site| match site {
            crate::frontend::DeclarationSite::Class(class) => Some(class),
            _ => None,
        })
        .collect();
        let mut suites = SuitePreparation::new(&prepared.source.namespace_operations, source, true);
        suites.sites.expressions = logicals;
        let initializer = suites.suite(body, RegisteredScope::Module);
        let class_bodies = suites.class_suites(
            classes,
            &prepared.source.private_names,
            &prepared.source.scopes,
        );
        let sites = suites.sites;
        prepared.initializer = initializer;
        prepared.source.class_bodies = class_bodies;
        prepared.source.statement_sites = sites;
    }

    impl SourceInfo<'_> {
        fn definition_body(&self, site: TextSize) -> RegisteredBody<'_> {
            self.scopes.functions[&site].body()
        }

        fn take_prepared_function(
            &mut self,
            site: TextSize,
        ) -> crate::frontend::RegisteredFunction {
            let body = self.scopes.functions.get_mut(&site).unwrap().body.take();
            let Some(NamedBody::Owned(checked)) = body else {
                panic!("expected an owned complete function");
            };
            *checked
        }

        fn prepared_function_mut(
            &mut self,
            site: TextSize,
        ) -> &mut crate::frontend::RegisteredFunction {
            let body = &mut self.scopes.functions.get_mut(&site).unwrap().body;
            let Some(NamedBody::Owned(checked)) = body else {
                panic!("expected an owned complete function");
            };
            checked
        }
    }

    fn install_test_function_body(
        source: &mut SourceInfo<'_>,
        def: &ast::StmtFunctionDef,
        body: crate::frontend::RegisteredSuite,
    ) {
        source.scopes.functions.get_mut(&def.start()).unwrap().body =
            Some(NamedBody::from_suite(body));
    }

    fn test_function_suite<'s>(
        source: &'s mut SourceInfo<'_>,
        start: TextSize,
    ) -> &'s mut crate::frontend::RegisteredSuite {
        match source
            .scopes
            .functions
            .get_mut(&start)
            .unwrap()
            .body
            .as_mut()
            .unwrap()
        {
            NamedBody::Suite(suite) => suite,
            _ => panic!("expected a prepared suite with residual operands or children"),
        }
    }

    fn assert_checked_call(body: &[crate::ir::Statement], source: &str, spelling: &str) {
        use crate::ir::Statement;

        let [Statement::Position(range, _), Statement::Expr(call)] = body else {
            panic!("expected a statement checkpoint and call for {spelling}");
        };
        assert_eq!(&source[*range], spelling);
        assert!(call.without_location().is_registered_invoke(), "{spelling}");
    }

    fn named_call(value: &crate::ir::Expr) -> (&str, &[crate::ir::Expr]) {
        use crate::ir::{Builtin, ExprKind};

        let ExprKind::Invoke { callable, args } = &value.without_location().kind else {
            panic!("expected a positioned Python call");
        };
        let ExprKind::Builtin(Builtin::RegisteredName(name), _) = &callable.without_location().kind
        else {
            panic!("expected a call to a loaded name");
        };
        (name, &args.values)
    }

    fn assert_input(value: &crate::ir::Expr, index: usize) {
        use crate::ir::{Builtin, ExprKind};

        assert!(
            matches!(value.kind, ExprKind::Builtin(Builtin::RegisteredInput(slot), _) if slot == index)
        );
    }

    fn input_statements(
        suite: &crate::frontend::RegisteredSuite,
    ) -> impl Iterator<Item = (usize, &crate::ir::Statement)> {
        suite
            .checked
            .function
            .body
            .iter()
            .enumerate()
            .filter(|(_, statement)| {
                let mut has_input = false;
                crate::ir::walk_expressions(std::slice::from_ref(*statement), &mut |expr| {
                    has_input |= matches!(
                        expr.kind,
                        crate::ir::ExprKind::Builtin(crate::ir::Builtin::RegisteredInput(_), _)
                    );
                });
                has_input
            })
    }

    fn residual_occurrence(
        suite: &mut crate::frontend::RegisteredSuite,
    ) -> &mut crate::frontend::OwnedExpression {
        let [crate::frontend::PhysicalOperand::Residual(value)] = suite.operands.as_mut_slice()
        else {
            panic!("expected one owned expression");
        };
        value
    }

    fn residual_selection(
        suite: &mut crate::frontend::RegisteredSuite,
    ) -> &mut crate::frontend::ExpressionSelection {
        std::rc::Rc::get_mut(&mut residual_occurrence(suite).selected).unwrap()
    }

    fn replace_residual_input(prepared: &mut PreparedBody<'_>, index: usize) {
        residual_selection(&mut prepared.initializer)
            .replace_residual_input(index, crate::ir::Expr::none());
    }

    #[test]
    fn imported_generator_binds_the_generic_default_at_its_definition() {
        use crate::ir::{Builtin, DefinitionSite, ExprKind, SuspendValue};

        let driver = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/suspended_generic_default/main.py");
        let mut graph = crate::modules::SourceGraph::load(&driver, None, &[]).unwrap();
        let _ = crate::checked_lower(&mut graph).unwrap();
        let app = graph.python_file("app").unwrap();
        let file = &graph.modules[app];
        let functions = graph.registered_functions();
        let module = PreparedBody::module(
            "app",
            &file.path,
            &file.source,
            0,
            ModuleOrigin::Program(functions.get(&app)),
        )
        .unwrap();
        let maker = module.source.definition_body(TextSize::new(0));
        assert!(maker.fully_checked());
        let mut defaults = Vec::new();
        crate::ir::walk_expressions(&maker.checked().function.body, &mut |expr| {
            if let ExprKind::Builtin(
                Builtin::RegisteredDefinition(DefinitionSite::Function(site)),
                inputs,
            ) = &expr.kind
            {
                let [default] = inputs.as_slice() else {
                    panic!("expected one default");
                };
                crate::ir::walk_operand_expressions(default, &mut |child| {
                    if let ExprKind::Builtin(
                        Builtin::RegisteredSuspend(SuspendValue::Yield, range),
                        _,
                    ) = child.kind
                    {
                        defaults.push((site.to_usize(), &file.source[range]));
                    }
                });
            }
        });
        assert_eq!(defaults, [(17, "yield 'default'")]);
    }

    #[test]
    fn starred_annotation_retains_the_scoped_input_and_its_unpack() {
        use crate::ir::{Builtin, Expr, ExprKind, Statement};

        let source = "def f(*items: *[row for row in rows()]): pass\n";
        let mut prepared = PreparedBody::module_with_syntax(
            "example",
            Path::new("example.py"),
            source,
            0,
            ModuleOrigin::Program(None),
        )
        .unwrap();
        let before = prepared.standalone().unwrap();
        prepared.syntax.as_mut().unwrap().to_mut().clear();
        assert_eq!(prepared.standalone().unwrap(), before);

        let header = prepared.source.scopes.functions.get_mut(&0.into()).unwrap();
        let provider = header.annotations.as_mut().unwrap();
        let checked = provider.body.as_mut().unwrap();
        assert!(!checked.region().private_slots().is_empty());
        crate::verify::registered(checked.region()).unwrap();
        let [
            Statement::Return(Expr {
                kind: ExprKind::Builtin(Builtin::RegisteredAnnotations { .. }, fields),
                ..
            }),
        ] = checked.function.body.as_mut_slice()
        else {
            panic!("expected one annotation provider")
        };
        let [value] = fields.as_mut_slice() else {
            panic!("expected one annotation value");
        };
        let ExprKind::Builtin(Builtin::ListGet(_), items) = &value.kind else {
            panic!("expected the value from the checked unpack");
        };
        assert!(matches!(
            items[0].kind,
            ExprKind::Builtin(Builtin::RegisteredUnpack { count: 1, .. }, _)
        ));
        *value = Expr::none();
        assert_ne!(prepared.standalone().unwrap(), before);
    }

    #[test]
    fn a_class_keeps_the_annotation_body_prepared_from_its_declarations() {
        use crate::ir::{Builtin, ExprKind, Statement};

        let mut prepared = PreparedBody::module_with_syntax(
            "example",
            Path::new("example.py"),
            "class Row:\n    first: [item for item in types]\n    second: str\n",
            0,
            ModuleOrigin::Program(None),
        )
        .unwrap();
        let original = prepared.standalone().unwrap();
        let owner = ruff_text_size::TextSize::new(0);
        prepared.syntax.as_mut().unwrap().to_mut().clear();
        assert_eq!(prepared.standalone().unwrap(), original);

        let provider = prepared
            .source
            .class_bodies
            .get_mut(&owner)
            .unwrap()
            .annotations
            .body
            .as_mut()
            .unwrap();
        let provider = provider.as_mut().as_mut().unwrap();
        assert!(!provider.region().private_slots().is_empty());
        crate::verify::registered(provider.region()).unwrap();
        let [
            Statement::Return(crate::ir::Expr {
                kind: ExprKind::Builtin(Builtin::RegisteredAnnotations { fields, .. }, values),
                ..
            }),
        ] = provider.function.body.as_mut_slice()
        else {
            panic!("expected checked class annotations")
        };
        assert_eq!(
            fields.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(),
            ["first", "second"]
        );
        let [value, replacement] = values.as_mut_slice() else {
            panic!("expected two annotation operands")
        };
        *value = replacement.clone();
        assert_ne!(prepared.standalone().unwrap(), original);
    }

    #[test]
    fn mixed_function_header_keeps_its_binding_after_erasing_the_declaration() {
        use crate::frontend::PhysicalOperand;
        let text = "@decorate\ndef choose[T: bound() = fallback()](first=checked(), *, last=(lambda: item)()):\n    return T, first, last\n";
        let mut module = PreparedBody::module_with_syntax(
            "case",
            Path::new("case.py"),
            text,
            0,
            ModuleOrigin::Provider,
        )
        .unwrap();
        inject_residual_child_module(&mut module);

        let plan = module.initializer();
        assert!(plan.refusal.is_none());
        let assignments: Vec<_> = input_statements(plan)
            .map(|(_, statement)| {
                let crate::ir::Statement::Assign(_, value) = statement else {
                    panic!("expected a mixed header assignment");
                };
                let crate::ir::ExprKind::Builtin(crate::ir::Builtin::RegisteredInput(slot), _) =
                    value.kind
                else {
                    panic!("expected a retained header input");
                };
                slot
            })
            .collect();
        assert_eq!(assignments, [0, 1, 2]);
        assert!(matches!(
            plan.operands.as_slice(),
            [
                PhysicalOperand::Checked(_),
                PhysicalOperand::Checked(_),
                PhysicalOperand::Residual(_)
            ]
        ));
        let original = module.standalone().unwrap();
        assert!(original.1.contains("type_parameter_evaluator"));
        module.syntax = None;
        assert_eq!(module.standalone().unwrap(), original);
    }

    #[test]
    fn nested_function_emits_without_parsed_declarations() {
        let source = concat!(
            "def factory(local):\n",
            "    @decorate('outer')\n",
            "    @decorate('inner')\n",
            "    def callback(value=local, /):\n",
            "        return local, value\n",
            "    return callback\n",
        );
        PreparedBody::with_module(
            "callbacks",
            Path::new("callbacks.py"),
            source,
            0,
            ModuleOrigin::Program(None),
            |mut module| {
                let initializer = module.initializer();
                let mut headers = Vec::new();
                for suite in std::iter::once(RegisteredBody::Suite(initializer)).chain(
                    module
                        .source
                        .scopes
                        .functions
                        .keys()
                        .map(|site| module.source.definition_body(*site)),
                ) {
                    assert!(suite.fully_checked());
                    crate::ir::walk_expressions(&suite.checked().function.body, &mut |value| {
                        if let crate::ir::ExprKind::Builtin(
                            crate::ir::Builtin::RegisteredDefinition(site),
                            operands,
                        ) = &value.kind
                        {
                            headers.push((site.start().to_usize(), operands.len()));
                        }
                    });
                }
                headers.sort_unstable();
                assert_eq!(headers, [(0, 0), (source.find("@decorate").unwrap(), 3)]);
                let expected = module.standalone()?;
                assert_eq!(
                    expected.1.matches(" = private constant %code_site").count(),
                    2
                );
                let retained = module.syntax.take().is_some();
                assert!(module.standalone()? == expected);
                assert!(!retained);
                Ok(())
            },
        )
        .unwrap();
    }

    #[test]
    fn paired_module_registers_its_checked_bodies_without_executable_syntax() {
        let fixture = include_str!("../../../../tests/fixtures/registered_reduction_callback.py");
        let mut graph = crate::modules::SourceGraph::single(fixture).unwrap();
        assert!(matches!(
            crate::checked_lower(&mut graph).unwrap(),
            crate::CheckedProgram::Registered(_)
        ));
        let bodies = graph.registered_functions();
        let functions = &bodies[&graph.entry];
        let source = &graph.modules[graph.entry].source;
        let prepared = PreparedBody::module(
            "example",
            Path::new("example.py"),
            source,
            0,
            ModuleOrigin::Program(Some(functions)),
        )
        .unwrap();
        assert!(prepared.syntax.is_none());
        let mut sites = 0;
        RequiredSite::visit(
            &prepared.initializer().checked.function.body,
            Some(&prepared.source.exec_literals),
            &mut |_| {
                sites += 1;
            },
        );
        assert_eq!(sites, 4);
        assert_eq!(prepared.source.scopes.functions.len(), functions.len());
        assert_eq!(functions.len(), 4);

        let (_, llvm) = prepared.standalone().unwrap();
        let mut ordinary = Vec::new();
        let mut annotated = Vec::new();
        for start in functions.keys() {
            let plan = prepared.source.definition_body(*start);
            let header = &prepared.source.scopes.functions[start];
            let name = header.name.as_str();
            if header.annotations.is_some() {
                annotated.push(name);
            } else {
                ordinary.push(name);
            }
            assert!(std::ptr::eq(plan.checked(), &functions[start]));
            let def_line = source[..usize::from(*start)].lines().count() + 1;
            let first_statement = format!(", i32 {}, i64 0)", def_line + 1);
            let (symbol, body) = llvm
                .split("\ndefine internal i64 ")
                .skip(1)
                .filter_map(|function| {
                    let (symbol, code) = function.split_once("(ptr %ctx, i64 %scope)")?;
                    Some((symbol, code.split_once("\n}").unwrap().0))
                })
                .find(|(_, body)| {
                    body.lines().any(|line| {
                        line.contains("call void @imre_dyn_statement_with_exception(")
                            && line.ends_with(&first_statement)
                    })
                })
                .unwrap();
            let site = llvm
                .lines()
                .filter_map(|line| line.split_once(" = private constant %code_site "))
                .find(|(_, addr)| {
                    addr.starts_with(&format!("{{ ptr {symbol},"))
                        && addr.ends_with(&format!(", i32 {def_line}, i32 0 }}"))
                })
                .unwrap()
                .0;
            assert!(llvm.contains(&format!("call i64 @imre_dyn_define(ptr %ctx, ptr {site},")));
            if name == "publish" {
                assert_eq!(
                    body.matches("call i64 @imre_dyn_exact_int_word(").count(),
                    2
                );
                assert_eq!(body.matches("call i64 @int_add(").count(), 1);
                assert_eq!(
                    body.matches("call i64 @imre_dyn_positional_call_at(")
                        .count(),
                    1
                );
            }
        }
        ordinary.sort_unstable();
        annotated.sort_unstable();
        assert_eq!(ordinary, ["first", "second"]);
        assert_eq!(annotated, ["publish", "successor_sum"]);
    }

    #[test]
    fn a_checked_generator_reference_requires_its_lexical_activation() {
        let source = "def render(words):\n    return ''.join(word.title() for word in words)\n";
        let path = Path::new("render.py");
        let site = ruff_text_size::TextSize::default();
        let mut original =
            PreparedBody::module("render", path, source, 0, ModuleOrigin::Program(None)).unwrap();
        let mut checked = original.source.take_prepared_function(site);
        let mut generators = Vec::new();
        RequiredSite::visit(&checked.function.body, None, &mut |required| {
            if let RequiredSite::Generator(range) = required {
                generators.push(&source[range]);
            }
        });
        assert_eq!(generators, ["word.title() for word in words"]);
        (checked.inputs, checked.frame_slots) =
            crate::llvm::registered::frame_inputs(&checked, FrameArguments::Python).unwrap();
        let bodies = HashMap::from([(site, checked)]);
        for (source, retained) in [
            (source, true),
            ("def render(words):\n    return ''.join(words)\n", false),
        ] {
            let module = PreparedBody::module(
                "render",
                path,
                source,
                0,
                ModuleOrigin::Program(Some(&bodies)),
            )
            .unwrap();
            assert_eq!(
                std::ptr::eq(
                    module.source.definition_body(site).checked(),
                    &bodies[&site]
                ),
                retained
            );
        }
    }

    #[test]
    fn a_checked_lambda_reference_requires_its_lexical_body() {
        let source = "async def make():\n    return lambda: 1\n";
        let refused = "async def make():\n    return [lambda v=await get(x): v for x in [1, 2]]\n";
        let path = Path::new("example.py");
        let site = TextSize::default();
        for arguments in [
            FrameArguments::Checked,
            FrameArguments::Pending,
            FrameArguments::Python,
        ] {
            let mut original =
                PreparedBody::module("example", path, source, 0, ModuleOrigin::Program(None))
                    .unwrap();
            let mut checked = original.source.take_prepared_function(site);
            (checked.inputs, checked.frame_slots) =
                crate::llvm::registered::frame_inputs(&checked, arguments).unwrap();
            let bodies = HashMap::from([(site, checked)]);
            for (text, retained) in [(source, true), (refused, false)] {
                let module = PreparedBody::module(
                    "example",
                    path,
                    text,
                    0,
                    ModuleOrigin::Program(Some(&bodies)),
                )
                .unwrap();
                assert_eq!(
                    std::ptr::eq(
                        module.source.definition_body(site).checked(),
                        &bodies[&site]
                    ),
                    retained
                );
                if !retained {
                    assert_eq!(
                        module.standalone().unwrap_err(),
                        "example.py:2:12: cannot compile this comprehension ahead of time (through 2:54)"
                    );
                }
            }
        }
    }

    #[test]
    fn detached_header_default_can_reuse_the_prepared_template_spelling() {
        let source =
            "def read(rows=[t'{ (v) = !r}' for v in (1, 2)]) -> object:\n    return rows\n";
        let path = Path::new("header.py");
        let site = ruff_text_size::TextSize::default();
        let mut initial =
            PreparedBody::module("header", path, source, 0, ModuleOrigin::Program(None)).unwrap();
        let checked = initial.source.take_prepared_function(site);
        let bodies = HashMap::from([(site, checked)]);
        let module = PreparedBody::module(
            "header",
            path,
            source,
            0,
            ModuleOrigin::Program(Some(&bodies)),
        )
        .unwrap();
        assert!(module.syntax.is_none());
        let initializer = module.initializer();
        assert!(initializer.fully_checked());
        assert_eq!(
            module.source.scopes.functions[&site]
                .metadata
                .defaults
                .as_ref(),
            ["rows"]
        );
        let context = module.source.registered_source();
        let spelling = context.template_fields.unwrap().values().next().unwrap();
        assert_eq!(spelling.expression, " (v)");
        assert_eq!(spelling.debug_prefix.as_deref(), Some(" (v) = "));
        assert!(
            initializer
                .checked
                .local_names
                .values()
                .any(|binding| binding.name == "v" && binding.namespace.is_some())
        );
        let mut captures = Vec::new();
        crate::ir::walk_expressions(&initializer.checked.function.body, &mut |expr| {
            if let crate::ir::ExprKind::Builtin(
                crate::ir::Builtin::RegisteredInterpolation {
                    expression,
                    conversion,
                },
                _,
            ) = &expr.kind
            {
                captures.push((expression.as_str(), *conversion));
            }
        });
        assert_eq!(captures, [(" (v)", Some('r'))]);
        module.standalone().unwrap();
    }

    #[test]
    fn named_checked_function_publishes_after_its_source_body_is_erased() {
        let source = concat!(
            "def publish(a: int, b: int, send):\n",
            "    \"Send the sum.\"\n",
            "    total = a + b\n",
            "    send(total)\n",
            "state: int = 0\n",
        );
        let graph = crate::modules::SourceGraph::single(source).unwrap();
        let _ = crate::frontend::lower(&graph);
        let bodies = graph.registered_functions();
        let mut prepared = PreparedBody::module(
            "example",
            Path::new("example.py"),
            source,
            0,
            ModuleOrigin::Program(Some(&bodies[&graph.entry])),
        )
        .unwrap();
        let entry = prepared.standalone().unwrap();
        assert!(entry.1.contains("call i64 @int_add("));
        assert!(entry.1.contains("call i64 @imre_dyn_positional_call_at("));
        assert!(entry.1.contains("Send the sum."));
        assert!(prepared.syntax.is_none());
        prepared.syntax = Some(Cow::Owned(
            graph.modules[graph.entry].parsed.syntax().body.clone(),
        ));
        let def = prepared.syntax.as_mut().unwrap().to_mut()[0]
            .as_function_def_stmt_mut()
            .unwrap();
        let plan = prepared.source.definition_body(def.start());
        assert_eq!(
            prepared.source.scopes.functions[&def.start()].name,
            "publish"
        );
        assert!(std::ptr::eq(
            plan.checked(),
            &bodies[&graph.entry][&def.start()]
        ));
        def.body.clear();
        assert_eq!(prepared.standalone().unwrap(), entry);
    }

    #[test]
    fn fixed_pair_loops_project_only_plain_dictionary_item_cursors() {
        let checked_raise = concat!(
            "        try:\n",
            "            raise (yield 'send-error')\n",
            "        except ValueError:\n",
            "            pass",
        );
        let deletion = "        del sink[(yield 1)]";
        let manager = "        with manager((yield 1)): pass";
        let checked_lambda = "        selected = (lambda value=(yield 1): value)()";
        let generic_default = "        def hold[T](value=(yield 1)):\n            return value";
        let annotated = "        sink[0]: int = 1";
        let checked_default = "        def resume(answer=(yield 1)):\n            return answer";
        let checked = "        publish(x, y)";
        let pair_header = "def f(rows):\n    for x, y in rows:";
        let suspended_target = "def f(rows):\n    for sink[(yield 'slot')], y in rows:";
        let yielded_rows = "def f():\n    for x, y in (yield 'rows'):";
        for (header, body) in [
            (pair_header, deletion),
            (pair_header, manager),
            (pair_header, checked_lambda),
            (pair_header, generic_default),
            (suspended_target, generic_default),
            (yielded_rows, checked),
            (pair_header, annotated),
            (pair_header, checked_raise),
            (pair_header, checked),
            (pair_header, checked_default),
            ("def f(table):\n    for x in table.items():", deletion),
            ("def f(table):\n    for x in table.items():", manager),
            (
                "def f(table):\n    for x in table.items():",
                generic_default,
            ),
            ("def f(rows):\n    for x, *ys in rows:", deletion),
            ("def f(rows):\n    for x, *ys in rows:", manager),
            ("def f(rows):\n    for x, *ys in rows:", generic_default),
            ("def f(manager):\n    with manager as (x, y):", deletion),
            ("def f(manager):\n    with manager as (x, y):", manager),
            (
                "def f(manager):\n    with manager as (x, y):",
                generic_default,
            ),
            ("async def f(rows):\n    async for x, y in rows:", checked),
        ] {
            let source = format!("{header}\n{body}\n");
            let module = PreparedBody::module(
                "example",
                Path::new("example.py"),
                &source,
                0,
                ModuleOrigin::Program(None),
            )
            .unwrap();
            let plan = module
                .source
                .definition_body(ruff_text_size::TextSize::new(0));
            if body == deletion
                || body == manager
                || body == checked_lambda
                || (header == pair_header && body == generic_default)
            {
                assert!(plan.fully_checked(), "{source}");
            }
            if body == annotated {
                assert!(plan.fully_checked() && module.syntax.is_none(), "{source}");
            }
            if header == suspended_target {
                assert!(plan.fully_checked(), "{source}");
                let owners = plan.checked().region().private_slots();
                let mut projected = 0;
                crate::ir::walk_statements(&plan.checked().function.body, &mut |stmt| {
                    if let crate::ir::Statement::IterNext {
                        iterator,
                        key,
                        ready,
                        value: Some(other),
                        ..
                    } = stmt
                    {
                        let slots = [*iterator, *key, *other, *ready];
                        for (index, slot) in slots.iter().enumerate() {
                            assert!(owners.contains(slot) && !slots[..index].contains(slot));
                        }
                        projected += 1;
                    }
                });
                assert_eq!(projected, 1, "{source}");
            }
            let (_, llvm) = module.standalone().unwrap();
            let test = "call i1 @imre_dyn_plain_items_cursor(";
            let pull = "call i64 @imre_dyn_next_pair(";
            let projects_pair = [pair_header, yielded_rows, suspended_target].contains(&header);
            assert_eq!(
                llvm.matches(test).count(),
                usize::from(projects_pair),
                "{source}"
            );
            assert_eq!(
                llvm.matches(pull).count(),
                usize::from(projects_pair),
                "{source}"
            );
            if projects_pair {
                assert!(llvm.find(test).unwrap() < llvm.find(pull).unwrap());
                let ordinary = "call i32 @imre_dyn_list_cursor_pull(";
                assert_eq!(llvm.matches(ordinary).count(), 1, "{source}");
            }
        }
    }

    #[test]
    fn registered_handlers_have_distinct_blocks_after_a_callback_raises() {
        let source = concat!(
            "def decode(value, callback):\n",
            "    try:\n",
            "        return callback(value)\n",
            "    except (TypeError, ValueError) as error:\n",
            "        return error\n",
            "    except:\n",
            "        return value\n",
        );
        let module = PreparedBody::module(
            "app",
            Path::new("app.py"),
            source,
            0,
            ModuleOrigin::Program(None),
        )
        .unwrap();
        let (_, llvm) = module.standalone().unwrap();
        assert!(llvm.contains("call i64 @imre_dyn_positional_call_at("));
        let mut blocks = HashSet::new();
        for line in llvm.lines() {
            if line.starts_with("define ") {
                blocks.clear();
            } else if let Some(block) = line.strip_suffix(':') {
                assert!(blocks.insert(block), "duplicate basic block {block}");
            }
        }
    }

    #[test]
    fn mixed_module_entry_uses_its_stored_inputs_after_source_erasure() {
        let mut module = PreparedBody::module(
            "entry",
            Path::new("entry.py"),
            "result = record((lambda: value)())
",
            0,
            ModuleOrigin::Program(None),
        )
        .unwrap();
        inject_residual_child_module(&mut module);
        let initializer = &mut module.initializer;
        assert!(!initializer.fully_checked());
        let before = module.standalone().unwrap();
        module.syntax = None;
        assert_eq!(module.standalone().unwrap(), before);
        replace_residual_input(&mut module, 0);
        assert_ne!(module.standalone().unwrap(), before);
    }

    #[test]
    fn named_entry_keeps_its_checked_body_after_erasing_the_declaration() {
        for update in [
            "del sink[(yield 1)]",
            "with manager((yield 1)): pass",
            "for entry in rows: selected = (lambda value=(yield 1): value)()",
            "for entry in rows:\n            def hold[T](value=(yield entry)):\n                return value",
            "for sink[(yield 'slot')] in rows: pass",
        ] {
            let source = format!(
                "def make():
    def define[T]():
        @retain
        def twice[U](value):
            return 2 * value
    try:
        {update}
    except ValueError:
        pass
    return define
"
            );
            let mut module = PreparedBody::module_with_syntax(
                "example",
                Path::new("example.py"),
                &source,
                0,
                ModuleOrigin::Program(None),
            )
            .unwrap();
            let before = module.standalone().unwrap();
            let def = module.syntax.as_mut().unwrap().to_mut()[0]
                .as_function_def_stmt_mut()
                .unwrap();
            let start = def.start();
            assert!(module.source.definition_body(start).fully_checked());
            def.body.clear();
            assert_eq!(module.standalone().unwrap(), before);

            let checked = module.source.prepared_function_mut(start);
            let Some(crate::ir::Statement::Return(result)) = checked.function.body.last_mut()
            else {
                panic!("expected the checked return after the nested declaration");
            };
            *result = crate::ir::Expr::none();
            assert_ne!(module.standalone().unwrap(), before);
        }
    }

    #[test]
    fn mixed_for_emits_after_its_source_statement_is_erased() {
        let source = concat!(
            "def capture(rows):\n",
            "    for item in rows:\n",
            "        rows[item]: object\n",
            "        return t'pre{(yield item)}'\n",
            "    else:\n",
            "        return 42\n",
        );
        let mut module = PreparedBody::module_with_syntax(
            "example",
            Path::new("example.py"),
            source,
            0,
            ModuleOrigin::Program(None),
        )
        .unwrap();
        let def = module.syntax.as_ref().unwrap()[0]
            .as_function_def_stmt()
            .unwrap();
        let start = def.start();
        // Withhold template spelling from the suite planner. The physical child can
        // still read the real prepared template, even after its enclosing loop is erased.
        let lexical = module.source.statement_sites.expressions.take_lexical();
        let missing = HashMap::new();
        let source = crate::frontend::RegisteredSource {
            template_fields: Some(&missing),
            ..module.source.registered_source()
        };
        let mut physical = crate::frontend::SuitePreparation::new(
            &module.source.namespace_operations,
            source,
            true,
        );
        physical.sites.expressions = lexical;
        let body = physical.physical_function_suite(
            &def.body,
            crate::frontend::RegisteredScope::Function { is_async: false },
            false,
        );
        module.source.statement_sites = physical.sites;
        install_test_function_body(&mut module.source, def, body);
        let plan = test_function_suite(&mut module.source, start);
        let [region] = plan.suites.as_slice() else {
            panic!("missing registered loop");
        };
        assert!(region.operands.is_empty());
        let mut pulls = 0;
        crate::ir::walk_statements(&region.checked.function.body, &mut |stmt| {
            if matches!(stmt, crate::ir::Statement::IterNext { .. }) {
                pulls += 1;
            }
        });
        assert_eq!(pulls, 1);
        assert!(!region.suites[0].fully_checked());
        let original = module.standalone().unwrap();
        let def = module.syntax.as_mut().unwrap().to_mut()[0]
            .as_function_def_stmt_mut()
            .unwrap();
        def.body[0] = ast::StmtPass {
            range: def.body[0].range(),
            node_index: Default::default(),
        }
        .into();
        assert_eq!(module.standalone().unwrap(), original);

        let plan = test_function_suite(&mut module.source, start);
        let [region] = plan.suites.as_mut_slice() else {
            panic!("missing registered loop");
        };
        crate::ir::walk_expressions_mut(&mut region.checked.function.body, &mut |expr| {
            if let crate::ir::ExprKind::Builtin(crate::ir::Builtin::RegisteredIter(_), args) =
                &mut expr.kind
            {
                args[0] = crate::ir::Expr::none();
            }
        });
        assert_ne!(module.standalone().unwrap(), original);
    }

    #[test]
    fn nested_generic_suites_keep_their_checked_namespace_operations() {
        let text = concat!(
            "def outer(value):\n",
            "    def scoped[T]():\n",
            "        global marker\n",
            "        nonlocal value\n",
            "        import math\n",
            "        from math import sqrt\n",
            "        type Label = tuple[str, int]\n",
            "        pass\n",
            "        class Local: pass\n",
            "        return Label\n",
        );
        let module = PreparedBody::module(
            "example",
            Path::new("example.py"),
            text,
            0,
            ModuleOrigin::Program(None),
        )
        .unwrap();
        let parsed = ruff_python_parser::parse_module(text).unwrap();
        let outer = parsed.syntax().body[0].as_function_def_stmt().unwrap();
        let scoped = outer.body[0].as_function_def_stmt().unwrap();
        assert!(
            module
                .source
                .definition_body(scoped.start())
                .fully_checked()
        );
        let parameters = &module.source.scopes.functions[&scoped.start()].type_params;
        assert_eq!(parameters.len(), 1);
        assert_eq!(parameters[0].name, "T");
    }

    #[test]
    fn function_parameter_owns_its_checked_fields() {
        use imre_ast::TypeParameterField::{Constraints, Default};
        let text = concat!(
            "def factory(select, manager):\n",
            "    def choose[U, T: (U, int) = select(), V = str]():\n",
            "        with manager:\n",
            "            return T\n",
            "    return choose\n",
        );
        let mut module = PreparedBody::module(
            "fields",
            Path::new("fields.py"),
            text,
            0,
            ModuleOrigin::Program(None),
        )
        .unwrap();
        let site = TextSize::try_from(text.find("def choose").unwrap()).unwrap();
        assert!(module.syntax.is_none() && module.source.definition_body(site).fully_checked());
        let before = module.standalone().unwrap();
        let header = module.source.scopes.functions.get_mut(&site).unwrap();
        let [u, t, v] = header.type_params.as_mut_slice() else {
            panic!("expected three parameters");
        };
        assert!(u.fields.is_empty() && matches!(v.fields.as_slice(), [(Default, _)]));
        assert_eq!(t.binding, "T");
        let [(Constraints, _), (Default, default)] = t.fields.as_mut_slice() else {
            panic!("expected constraints followed by a default");
        };
        let code = default.body.as_mut().unwrap();
        assert_eq!(&text[code.range], "select()");
        code.operand.value = crate::ir::Expr::none();
        assert_ne!(module.standalone().unwrap(), before);
    }

    #[test]
    fn mixed_loop_transfers_emit_after_erasing_their_keywords() {
        use crate::frontend::{RegisteredScope, RegisteredSource, SuitePreparation};
        let text = concat!(
            "def run(ready, decide, callback):\n",
            "    while ready():\n",
            "        if decide():\n",
            "            callback(lambda: 1)\n",
            "            continue\n",
            "        callback(lambda: 2)\n",
            "        break\n",
        );
        let mut module = PreparedBody::module(
            "worker",
            Path::new("worker.py"),
            text,
            0,
            ModuleOrigin::Bundled,
        )
        .unwrap();
        // The ordinary compilation can already discard this function's syntax.
        // Supply the original tree to the intentionally mixed preparation below.
        let parsed = ruff_python_parser::parse_module(text).unwrap();
        module.syntax = Some(Cow::Owned(parsed.into_syntax().body));
        let function = module.syntax.as_ref().unwrap()[0]
            .as_function_def_stmt()
            .unwrap();
        let function_start = function.start();
        // Keep both suites mixed by withholding only the callback descriptors.
        let lexical = module.source.statement_sites.expressions.take_lexical();
        let missing = HashMap::new();
        let source = RegisteredSource {
            lambdas: Some(&missing),
            ..module.source.registered_source()
        };
        let mut plan = SuitePreparation::new(&module.source.namespace_operations, source, true);
        plan.sites.expressions = lexical;
        let body = plan.physical_function_suite(
            &function.body,
            RegisteredScope::Function { is_async: false },
            false,
        );
        module.source.statement_sites = plan.sites;
        install_test_function_body(&mut module.source, function, body);

        let original = module.standalone().unwrap();
        let def = module.syntax.as_mut().unwrap().to_mut()[0]
            .as_function_def_stmt_mut()
            .unwrap();
        let body = &mut def.body[0].as_while_stmt_mut().unwrap().body;
        let (branch, tail) = body.split_at_mut(1);
        let branch_range = branch[0].range();
        let skip = &mut branch[0].as_if_stmt_mut().unwrap().body[1];
        let skip_range = skip.range();
        let stop_range = tail[1].range();
        for keyword in [skip, &mut tail[1]] {
            *keyword = ast::StmtPass {
                range: keyword.range(),
                node_index: Default::default(),
            }
            .into();
        }
        assert_eq!(module.standalone().unwrap(), original);

        let plan = test_function_suite(&mut module.source, function_start);
        assert!(plan.refusal.is_none());
        let [region] = plan.suites.as_mut_slice() else {
            panic!("expected the prepared while");
        };
        assert!(matches!(
            region.checked.function.body.as_slice(),
            [crate::ir::Statement::While(..)]
        ));
        let suite = &mut region.suites[0];
        fn transfer(
            suite: &mut crate::frontend::RegisteredSuite,
            range: TextRange,
        ) -> &mut crate::ir::Statement {
            use crate::ir::Statement;
            let (transfer, preceding) = suite.checked.function.body.split_last_mut().unwrap();
            assert!(
                !matches!(preceding.last(), Some(Statement::Position(site, _)) if *site == range)
            );
            let (Statement::Break(Some(site)) | Statement::Continue(site)) = transfer else {
                panic!("expected a source-positioned loop transfer");
            };
            assert_eq!(*site, range);
            transfer
        }
        assert!(matches!(
            transfer(suite, stop_range),
            crate::ir::Statement::Break(_)
        ));
        let [branch] = suite.suites.as_mut_slice() else {
            panic!("expected the if at the start of the while body");
        };
        use crate::ir::Statement;
        let [Statement::Position(site, None), Statement::If(..)] =
            branch.checked.function.body.as_slice()
        else {
            panic!("expected the checkpoint and checked conditional");
        };
        assert_eq!(*site, branch_range);
        let op = transfer(&mut branch.suites[0], skip_range);
        assert!(matches!(op, crate::ir::Statement::Continue(_)));
        *op = crate::ir::Statement::Break(Some(skip_range));
        assert_ne!(module.standalone().unwrap(), original);
    }

    #[test]
    fn mixed_coroutine_emits_sync_for_without_its_source() {
        let mut module = PreparedBody::module_with_syntax(
            "async_groups",
            Path::new("async_groups.py"),
            include_str!("../../../../tests/fixtures/processes/async_groups.py"),
            0,
            ModuleOrigin::Program(None),
        )
        .unwrap();
        let syntax = module.syntax.as_ref().unwrap();
        let definition = syntax
            .iter()
            .position(|stmt| {
                stmt.as_function_def_stmt()
                    .is_some_and(|def| def.name.as_str() == "main")
            })
            .unwrap();
        let main = syntax[definition].as_function_def_stmt().unwrap();
        let suite = module.source.definition_body(main.start());
        let index = main
            .body
            .iter()
            .position(|stmt| {
                stmt.as_for_stmt()
                    .and_then(|loop_| loop_.target.as_name_expr())
                    .is_some_and(|name| name.id.as_str() == "entry")
            })
            .unwrap();
        assert!(
            !matches!(&suite, RegisteredBody::Suite(plan) if plan.suites.iter().any(|region| matches!(region.checked.function.body.first(),
                        Some(crate::ir::Statement::Position(range, _)) if *range == main.body[index].range()
            )))
        );
        let last = main.body.last().unwrap().as_try_stmt().unwrap();
        assert!(last.body[0].as_with_stmt().unwrap().is_async);
        let task_group = main.body.len() - 1;
        assert!(suite.checked().function.body.windows(2).any(|ops| matches!(
            ops,
            [crate::ir::Statement::Position(range, _), crate::ir::Statement::Try { .. }]
                if *range == last.range()
        )));

        let before = module.standalone().unwrap();
        let body = &mut module.syntax.as_mut().unwrap().to_mut()[definition]
            .as_function_def_stmt_mut()
            .unwrap()
            .body;
        for i in [index, task_group] {
            body[i] = ast::StmtPass {
                node_index: Default::default(),
                range: body[i].range(),
            }
            .into();
        }
        assert!(module.standalone().unwrap() == before);
    }

    #[test]
    fn unresolved_generator_retains_the_typed_operands_after_yield() {
        let source = concat!(
            "def raised(error, cause, n: int):\n",
            "    yield None\n",
            "    yield n + n\n",
            "    raise error from cause\n",
            "cause = StopIteration('explicit')\n",
            "for error, explicit in ((ValueError('row'), cause), (cause, None)):\n",
            "    next(raised(error, explicit, 11))\n",
        );
        let mut graph = crate::modules::SourceGraph::single(source).unwrap();
        graph.registered_startup = true;
        let _ = crate::checked_lower(&mut graph).unwrap();
        let table = graph.registered_functions();
        let body = &table[&graph.entry][&TextSize::default()];
        crate::verify::registered(body.region()).unwrap();
        let mut adds = Vec::new();
        crate::ir::walk_expressions(&body.function.body, &mut |expr| {
            if let crate::ir::ExprKind::Binary(crate::ir::Binary::Add, left, right) = &expr.kind {
                adds.push((left.checked_operand_type(), right.checked_operand_type()));
            }
        });
        assert_eq!(adds, [(crate::ir::Type::Int, crate::ir::Type::Int)]);
    }

    #[test]
    fn retained_program_generator_emits_after_erasing_its_suspensions() {
        let source = concat!(
            "def values():\n",
            "    yield 0\n",
            "    def choose(value): return value\n",
            "    transform = lambda x: x + 1\n",
            "    for item in (1, 2): yield transform(choose(item))\n",
            "for item in values(): print(item)\n",
            "state: int\n",
        );
        let graph = crate::modules::SourceGraph::single(source).unwrap();
        let _ = crate::frontend::lower(&graph);
        let table = graph.registered_functions();
        let bodies = &table[&graph.entry];
        let retained = &bodies[&ruff_text_size::TextSize::default()];
        let mut required = Vec::new();
        RequiredSite::visit(&retained.function.body, None, &mut |site| match site {
            RequiredSite::Definition(_) => required.push("definition"),
            RequiredSite::Lambda(..) => required.push("lambda"),
            _ => {}
        });
        assert_eq!(required, ["definition", "lambda"]);
        let mut module = PreparedBody::module(
            "__main__",
            Path::new("example.py"),
            source,
            0,
            ModuleOrigin::Program(Some(bodies)),
        )
        .unwrap();
        let before = module.standalone().unwrap();
        assert!(module.syntax.is_none());
        module.syntax = Some(Cow::Owned(
            graph.modules[graph.entry].parsed.syntax().body.clone(),
        ));
        let def = module.syntax.as_mut().unwrap().to_mut()[0]
            .as_function_def_stmt_mut()
            .unwrap();
        // The nested declaration still supplies its signature and code site.
        def.body.retain(ast::Stmt::is_function_def_stmt);
        assert_eq!(def.body.len(), 1);
        assert_eq!(module.standalone().unwrap().1, before.1);
    }

    #[test]
    fn bundled_directory_and_datagram_loops_use_checked_suspensions() {
        for (name, text, definition) in [
            ("os", include_str!("../../stdlib/os.py"), "def _fwalk("),
            (
                "asyncio.base_events",
                include_str!("../../stdlib/asyncio/base_events.py"),
                "async def create_datagram_endpoint(",
            ),
        ] {
            let module =
                PreparedBody::module(name, Path::new("module.py"), text, 0, ModuleOrigin::Bundled)
                    .unwrap();
            let start = ruff_text_size::TextSize::try_from(text.find(definition).unwrap()).unwrap();
            let suite = module.source.definition_body(start);
            assert!(suite.fully_checked(), "{name}");
            crate::verify::registered(suite.checked().region()).unwrap();
            let mut suspends = false;
            crate::ir::walk_expressions(&suite.checked().function.body, &mut |expr| {
                suspends |= matches!(
                    expr.kind,
                    crate::ir::ExprKind::Builtin(crate::ir::Builtin::RegisteredSuspend(..), _)
                );
            });
            assert!(suspends);
        }
    }

    #[test]
    fn suspended_class_header_emits_after_erasing_its_class_statement() {
        let source = concat!(
            "def build():\n",
            "    @(yield 'decorator')\n",
            "    class Child((yield 'base')):\n",
            "        marker = 'child'\n",
            "    @(yield 'generic decorator')\n",
            "    class Packet[T](base(T)):\n",
            "        pass\n",
            "    yield Child, Packet\n",
        );
        let mut module = PreparedBody::module(
            "example",
            Path::new("example.py"),
            source,
            0,
            ModuleOrigin::Program(None),
        )
        .unwrap();
        let mut body = ruff_python_parser::parse_module(source)
            .unwrap()
            .into_syntax()
            .body;
        let def = body[0].as_function_def_stmt_mut().unwrap();
        let classes = [def.body[0].start(), def.body[1].start()];
        let plan = module.source.definition_body(def.start());
        assert!(plan.fully_checked());
        let mut definitions = Vec::new();
        RequiredSite::visit(&plan.checked().function.body, None, &mut |required| {
            if let RequiredSite::Definition(start) = required {
                definitions.push(start);
            }
        });
        assert_eq!(definitions, classes);
        let expected = module.standalone().unwrap();
        for declaration in &mut def.body[..2] {
            *declaration = ast::StmtPass {
                range: declaration.range(),
                node_index: Default::default(),
            }
            .into();
        }
        module.syntax = Some(Cow::Owned(body));
        assert_eq!(module.standalone().unwrap(), expected);
    }

    #[test]
    fn mixed_class_header_uses_its_retained_starred_operand_after_erasure() {
        use crate::frontend::{ExpressionSelection, PhysicalOperand};
        use crate::ir::{Builtin, DefinitionSite, Expr, ExprKind, Statement};

        let source = "def keep(cls): return cls\n@keep\nclass C(*(lambda: (object,))(), metaclass=type):\n    (lambda: None)()\n";
        let mut module = PreparedBody::module(
            "example",
            Path::new("example.py"),
            source,
            0,
            ModuleOrigin::Program(None),
        )
        .unwrap();
        assert!(module.initializer().fully_checked());
        inject_residual_child_module(&mut module);
        let before = module.standalone().unwrap();
        assert!(module.syntax.take().is_some());
        assert_eq!(module.standalone().unwrap(), before);

        let suite = &mut module.initializer;
        assert!(suite.refusal.is_none());
        let statements: Vec<_> = input_statements(suite).collect();
        let [(_, statement)] = statements.as_slice() else {
            panic!("expected one mixed class header");
        };
        let Statement::Assign(
            _,
            Expr {
                kind:
                    ExprKind::Builtin(
                        Builtin::RegisteredDefinition(DefinitionSite::Class {
                            start,
                            arguments: Some(layout),
                            ..
                        }),
                        slots,
                    ),
                ..
            },
        ) = statement
        else {
            panic!("expected the packed class definition result");
        };
        let mut deferred = Vec::new();
        RequiredSite::visit(&suite.checked.function.body, None, &mut |site| {
            if let RequiredSite::DefinitionAfterInputs(at) = site {
                deferred.push(at);
            }
        });
        assert_eq!(deferred, [*start]);
        assert_eq!(layout.starred, [true]);
        assert_eq!(slots.len(), 3);
        let [
            PhysicalOperand::Checked(_),
            PhysicalOperand::Residual(base),
            PhysicalOperand::Checked(_),
        ] = suite.operands.as_mut_slice()
        else {
            panic!("expected the starred base between the decorator and metaclass");
        };
        assert_eq!(&source[base.range], "(lambda: (object,))()");
        let mut refused_base = base.clone();
        refused_base.selected =
            std::rc::Rc::new(ExpressionSelection::Refused(base.range, "header refused"));
        base.selected = std::rc::Rc::new(ExpressionSelection::Checked(Expr::none()));
        assert_ne!(module.standalone().unwrap(), before);

        let class = module.source.class_bodies.values_mut().next().unwrap();
        let body = residual_occurrence(&mut class.body);
        body.selected = std::rc::Rc::new(ExpressionSelection::Refused(body.range, "body refused"));
        assert!(module.standalone().unwrap_err().contains("body refused"));
        module.initializer.operands[1] = PhysicalOperand::Residual(refused_base);
        assert!(module.standalone().unwrap_err().contains("header refused"));
    }

    #[test]
    fn generic_typing_protocols_emit_without_their_class_syntax() {
        use crate::ir::{Builtin, ExprKind};

        let source = include_str!("../../stdlib/typing.py");
        PreparedBody::with_module(
            "typing",
            Path::new("<imre stdlib>/typing.py"),
            source,
            0,
            ModuleOrigin::Bundled,
            |typing| {
                let expected = typing.standalone()?;
                let mut body = typing.original_fixture_suite()?;
                let mut protocols = Vec::new();
                for stmt in &mut body {
                    let Some(class) = stmt.as_class_def_stmt() else {
                        continue;
                    };
                    if matches!(class.name.as_str(), "SupportsAbs" | "SupportsRound") {
                        let start = class.start();
                        protocols.push((class.name.to_string(), start));
                        assert!(typing.source.class_bodies[&start].body.fully_checked());
                        *stmt = ast::StmtPass {
                            node_index: Default::default(),
                            range: class.range(),
                        }
                        .into();
                    }
                }
                assert_eq!(
                    protocols
                        .iter()
                        .map(|(name, _)| name.as_str())
                        .collect::<Vec<_>>(),
                    ["SupportsAbs", "SupportsRound"]
                );
                let mut bindings = Vec::new();
                let initializer = &typing.initializer;
                crate::ir::walk_expressions(&initializer.checked.function.body, &mut |expr| {
                    if let ExprKind::Builtin(
                        Builtin::RegisteredDefinition(crate::ir::DefinitionSite::Class {
                            start,
                            ..
                        }),
                        _,
                    ) = &expr.kind
                        && protocols.iter().any(|(_, site)| site == start)
                    {
                        bindings.push(*start);
                    }
                });
                assert_eq!(bindings, [protocols[0].1, protocols[1].1]);
                let withheld = PreparedBody {
                    syntax: Some(Cow::Owned(body)),
                    ..typing
                };
                assert!(withheld.standalone()? == expected);
                Ok(())
            },
        )
        .unwrap();
    }

    #[test]
    fn bundled_generic_method_uses_the_prepared_class_suite() {
        let source = include_str!("../../stdlib/typing.py");
        let module = PreparedBody::module(
            "typing",
            Path::new("<imre stdlib>/typing.py"),
            source,
            0,
            ModuleOrigin::Bundled,
        )
        .unwrap();
        let class = source.find("class _IdentityCallable(Protocol):").unwrap();
        let method = class + source[class..].find("def __call__[T]").unwrap();
        let start = |offset: usize| ruff_text_size::TextSize::try_from(offset).unwrap();
        let body = &module.source.class_bodies[&start(class)].body;
        assert!(body.fully_checked());
        let mut bindings = Vec::new();
        crate::ir::walk_expressions(&body.checked.function.body, &mut |value| {
            if let crate::ir::ExprKind::Builtin(crate::ir::Builtin::RegisteredDefinition(site), _) =
                &value.kind
            {
                bindings.push(site.start());
            }
        });
        assert_eq!(bindings, [start(method)]);
        assert!(matches!(
            module.source.scopes.functions[&start(method)].body.as_ref(),
            Some(NamedBody::Owned(_))
        ));
    }

    #[test]
    fn generic_class_sites_own_type_parameters_decorators_and_bases() {
        use crate::ir::{Builtin, DefinitionSite, ExprKind};

        let source = include_str!("../../stdlib/typing.py");
        let syntax = ruff_python_parser::parse_module(source)
            .unwrap()
            .into_syntax();
        let typing = PreparedBody::module(
            "typing",
            Path::new("<imre stdlib>/typing.py"),
            source,
            0,
            ModuleOrigin::Bundled,
        )
        .unwrap();
        let body = &typing.initializer.checked.function.body;
        for name in ["SupportsAbs", "SupportsRound"] {
            let class = syntax
                .body
                .iter()
                .filter_map(ast::Stmt::as_class_def_stmt)
                .find(|class| class.name.as_str() == name)
                .unwrap();
            let owner = &typing.source.class_bodies[&class.decorator_list[0].start()];
            let header = owner.header.as_ref().unwrap();
            assert_eq!(header.parameters.declarations[0].name, "T");
            let header = header.body.as_ref().unwrap();
            let mut arguments = Vec::new();
            crate::ir::walk_expressions(&header.function.body, &mut |expr| {
                if let ExprKind::Builtin(Builtin::RegisteredClassNamespace(site, _), children) =
                    &expr.kind
                {
                    assert_eq!(*site, class.start());
                    arguments.push(children.len());
                }
            });
            assert_eq!(arguments, [1], "{name}");
            let mut decorations = Vec::new();
            crate::ir::walk_expressions(body, &mut |expr| {
                if let ExprKind::Builtin(
                    Builtin::RegisteredDefinition(DefinitionSite::Class {
                        start,
                        arguments: None,
                    }),
                    operands,
                ) = &expr.kind
                    && *start == class.start()
                {
                    decorations.push(operands.len());
                }
            });
            assert_eq!(decorations, [1], "{name}");
        }

        let program = PreparedBody::module(
            "mixed",
            Path::new("mixed.py"),
            "@outer\n@next(d for d in decorators)\nclass Packet[T](base(T), *(c for c in bases), metaclass=meta(T)):\n    pass\n",
            0,
            ModuleOrigin::Program(None),
        )
        .unwrap();
        let header = &program.source.class_bodies[&ruff_text_size::TextSize::new(0)].header;
        assert!(program.initializer().fully_checked());
        let mut decorators = Vec::new();
        crate::ir::walk_expressions(&program.initializer().checked.function.body, &mut |expr| {
            if let ExprKind::Builtin(
                Builtin::RegisteredDefinition(DefinitionSite::Class { .. }),
                operands,
            ) = &expr.kind
            {
                decorators.push(operands.len());
            }
        });
        assert_eq!(decorators, [2]);
        let function = header.as_ref().unwrap().body.as_ref().unwrap();
        assert!(matches!(
            function.function.body.last(),
            Some(crate::ir::Statement::Return(crate::ir::Expr {
                kind: ExprKind::Builtin(Builtin::RegisteredClassNamespace(_, _), operands), ..
            })) if operands.len() == 3
        ));
    }

    #[test]
    fn generic_class_header_reports_its_missing_checked_child() {
        let mut module = PreparedBody::module(
            "example",
            Path::new("example.py"),
            "class Packet[T](\n    pick(lambda: T),\n    metaclass=meta(T),\n): pass\n",
            0,
            ModuleOrigin::Program(None),
        )
        .unwrap();
        module.standalone().unwrap();

        inject_residual_child_module(&mut module);
        let header = &module.source.class_bodies.values().next().unwrap().header;
        assert!(header.as_ref().unwrap().body.is_err());
        assert!(module.syntax.take().is_some());
        assert_eq!(
            module.standalone().unwrap_err(),
            "example.py:2:10: cannot compile this lambda body ahead of time (through 2:19)"
        );
    }

    #[test]
    fn async_method_prepares_its_streaming_display_before_llvm() {
        let source = r#"class Stream:
    async def aread(self) -> bytes:
        """Read and return the response content."""
        if not hasattr(self, "_content"):
            self._content = b"".join([part async for part in self.aiter_bytes()])
        return self._content
"#;
        let module = PreparedBody::module(
            "stream",
            Path::new("stream.py"),
            source,
            0,
            ModuleOrigin::Program(None),
        )
        .unwrap();
        let start = ruff_text_size::TextSize::try_from(source.find("async def").unwrap()).unwrap();
        let suite = module.source.definition_body(start);
        let mut pulls = 0;
        crate::ir::walk_statements(&suite.checked().function.body, &mut |stmt| {
            if matches!(
                stmt,
                crate::ir::Statement::IterNext {
                    kind: crate::ir::IterNextKind::Async(_),
                    ..
                }
            ) {
                pulls += 1;
            }
        });
        assert_eq!(pulls, 1);
        let (_, llvm) = module.standalone().unwrap();
        assert!(llvm.contains("%ir_slot0 = alloca ["));
    }

    #[test]
    fn async_stream_caller_emits_its_prepared_generator_and_display() {
        use crate::ir::{Builtin, ExprKind, IterNextKind, Statement, SuspendValue};

        let source = concat!(
            "async def read_response(response):\n",
            "    chunks = (piece async for piece in response.aiter_bytes(chunk_size=7))\n",
            "    print('chunks', [piece async for piece in chunks], flush=True)\n",
        );
        let mut module = PreparedBody::module_with_syntax(
            "stream",
            Path::new("stream.py"),
            source,
            0,
            ModuleOrigin::Program(None),
        )
        .unwrap();
        let caller = module.source.definition_body(0.into());
        assert!(caller.fully_checked());
        let generators = &module.source.generators;
        assert_eq!(generators.len(), 1);
        let (site, child) = generators.iter().next().unwrap();
        let caller = &caller.checked().function.body;
        let child = &child.body.function.body;
        for body in [caller, child] {
            let mut pulls = 0;
            crate::ir::walk_statements(body, &mut |stmt| {
                if matches!(
                    stmt,
                    Statement::IterNext {
                        kind: IterNextKind::Async(_),
                        ..
                    }
                ) {
                    pulls += 1;
                }
            });
            assert_eq!(pulls, 1);
        }
        let mut children = Vec::new();
        crate::ir::walk_expressions(caller, &mut |expr| {
            if let ExprKind::Builtin(Builtin::RegisteredGenerator(site, ..), _) = expr.kind {
                children.push(site);
            }
        });
        assert_eq!(children, [*site]);
        let mut yields = 0;
        crate::ir::walk_expressions(child, &mut |expr| {
            if matches!(
                expr.kind,
                ExprKind::Builtin(Builtin::RegisteredSuspend(SuspendValue::Yield, _), _)
            ) {
                yields += 1;
            }
        });
        assert_eq!(yields, 1);

        let before = module.standalone().unwrap();
        assert!(module.syntax.take().is_some());
        assert_eq!(module.standalone().unwrap(), before);
    }

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
        let before = prepared.standalone().unwrap();
        assert!(!prepared.source.replacements.is_empty());
        prepared.source.replacements.clear();
        assert_eq!(prepared.standalone().unwrap(), before);
    }

    #[test]
    fn a_suspended_loop_owns_its_fall_through_conditional() {
        let source = concat!(
            "def values(flag, consume):\n",
            "    for item in (1, 2):\n",
            "        yield item\n",
            "        if (selected := flag): consume(selected, item)\n",
            "        if (suspended := flag): yield suspended\n",
            "        if flag: return item\n",
            "async def asynchronous(flag, consume, stream):\n",
            "    for item in (1, 2):\n",
            "        yield item\n",
            "        if flag: consume(item)\n",
            "        if flag: await consume(item)\n",
            "        if flag:\n",
            "            async for child in stream: consume(child)\n",
        );
        let prepared = PreparedBody::module_with_syntax(
            "example",
            Path::new("example.py"),
            source,
            0,
            ModuleOrigin::Program(None),
        )
        .unwrap();
        let def = prepared.syntax.as_ref().unwrap()[0]
            .as_function_def_stmt()
            .unwrap();
        let suite = prepared.source.definition_body(def.start());
        crate::verify::registered(suite.checked().region()).unwrap();
        assert!(suite.fully_checked());
        assert!(matches!(
            prepared.source.scopes.functions[&def.start()].body.as_ref(),
            Some(NamedBody::Owned(_))
        ));
        for name in ["selected", "suspended"] {
            assert!(
                suite
                    .checked()
                    .local_names
                    .values()
                    .any(|binding| binding.name == name)
            );
        }
        let def = prepared.syntax.as_ref().unwrap()[1]
            .as_function_def_stmt()
            .unwrap();
        let suite = prepared.source.definition_body(def.start());
        assert!(suite.fully_checked());
        crate::verify::registered(suite.checked().region()).unwrap();
        let mut suspensions = Vec::new();
        crate::ir::walk_expressions(&suite.checked().function.body, &mut |expr| {
            if let crate::ir::ExprKind::Builtin(
                crate::ir::Builtin::RegisteredSuspend(_, range),
                _,
            ) = &expr.kind
            {
                suspensions.push(&source[*range]);
            }
        });
        assert!(suspensions.contains(&"yield item"));
        assert!(suspensions.contains(&"await consume(item)"));
    }

    #[test]
    fn checked_with_owns_a_yielded_manager_inside_an_active_context() {
        let text = "def entries(path, stream):\n    with guard(), scandir(path) as stream:\n        with (yield stream):\n            pass\n";
        let mut prepared = PreparedBody::module_with_syntax(
            "scan",
            Path::new("scan.py"),
            text,
            0,
            ModuleOrigin::Program(None),
        )
        .unwrap();
        let original = prepared.standalone().unwrap();
        let function = prepared.syntax.as_mut().unwrap().to_mut()[0]
            .as_function_def_stmt_mut()
            .unwrap();
        assert!(
            prepared
                .source
                .definition_body(function.start())
                .fully_checked()
        );
        function.body[0] = ast::StmtPass {
            range: function.body[0].range(),
            node_index: Default::default(),
        }
        .into();
        assert_eq!(prepared.standalone().unwrap(), original);
    }

    #[test]
    fn checked_with_retains_the_manager_factory_across_a_suspended_argument() {
        let source = concat!(
            "def named(manager):\n",
            "    with manager((yield 1)): pass\n",
            "def pairs(rows, op):\n",
            "    for name, value in rows:\n",
            "        with op((yield 'manager')) as entered: publish(name, value, entered)\n",
            "async def select(subject, manager, key):\n",
            "    match subject:\n",
            "        case item if accept(item):\n",
            "            with manager(await key(item)): pass\n",
        );
        let mut module = PreparedBody::module_with_syntax(
            "program",
            Path::new("program.py"),
            source,
            0,
            ModuleOrigin::Program(None),
        )
        .unwrap();
        let original = module.standalone().unwrap();
        for stmt in module.syntax.as_mut().unwrap().to_mut() {
            let def = stmt.as_function_def_stmt_mut().unwrap();
            assert!(module.source.definition_body(def.start()).fully_checked());
            def.body.clear();
        }
        assert_eq!(module.standalone().unwrap(), original);
    }

    #[test]
    fn checked_with_owns_single_and_grouped_yielding_targets() {
        let source = concat!(
            "def receive(cm, sink):\n",
            "    with cm as sink[(yield 'key')]:\n",
            "        return sink\n",
            "def unpack(cm, sink):\n",
            "    with cm as (sink.head, *middle, sink[(yield 'key')]):\n",
            "        return sink\n",
        );
        let mut prepared = PreparedBody::module_with_syntax(
            "program",
            Path::new("program.py"),
            source,
            0,
            ModuleOrigin::Program(None),
        )
        .unwrap();
        let original = prepared.standalone().unwrap();
        let syntax = prepared.syntax.as_mut().unwrap().to_mut();
        for statement in syntax {
            let function = statement.as_function_def_stmt_mut().unwrap();
            assert!(
                prepared
                    .source
                    .definition_body(function.start())
                    .fully_checked()
            );
            function.body.clear();
        }
        assert_eq!(prepared.standalone().unwrap(), original);
    }

    #[test]
    fn async_with_owns_its_checked_binding_and_return() {
        let mut prepared = PreparedBody::module_with_syntax(
            "program",
            Path::new("program.py"),
            "async def use(manager):\n    async with manager as value:\n        return value\n",
            0,
            ModuleOrigin::Program(None),
        )
        .unwrap();
        let original = prepared.standalone().unwrap();
        let function = prepared.syntax.as_mut().unwrap().to_mut()[0]
            .as_function_def_stmt_mut()
            .unwrap();
        let suite = prepared.source.definition_body(function.start());
        assert!(suite.fully_checked());
        assert!(
            suite
                .checked()
                .function
                .body
                .iter()
                .any(|statement| matches!(
                    statement,
                    crate::ir::Statement::With { binding: Some(_), site, .. } if site.is_async
                ))
        );
        let context = function.body[0].as_with_stmt_mut().unwrap();
        context.items.clear();
        context.body.clear();
        assert_eq!(prepared.standalone().unwrap(), original);
    }

    #[test]
    fn async_with_prepares_its_awaited_manager_and_target() {
        let mut prepared = PreparedBody::module_with_syntax(
            "program",
            Path::new("program.py"),
            "async def use(provider, sink):\n    async with await provider() as sink.item:\n        return sink.item\n",
            0,
            ModuleOrigin::Program(None),
        )
        .unwrap();
        let original = prepared.standalone().unwrap();
        let function = prepared.syntax.as_mut().unwrap().to_mut()[0]
            .as_function_def_stmt_mut()
            .unwrap();
        function.body[0] = ast::StmtPass {
            range: function.body[0].range(),
            node_index: Default::default(),
        }
        .into();
        assert_eq!(prepared.standalone().unwrap(), original);
    }

    #[test]
    fn scandir_generator_keeps_its_checked_with_body() {
        PreparedBody::with_module(
            "scan",
            Path::new("scan.py"),
            "def entries(path):\n    with scandir(path) as stream:\n        for entry in stream:\n            yield entry\n",
            0,
            ModuleOrigin::Program(None),
            |prepared| {
                let bodies = &prepared.source.scopes.functions;
                assert_eq!(bodies.len(), 1);
                let suite = prepared.source.definition_body(*bodies.keys().next().unwrap());
                assert!(suite.fully_checked());
                crate::verify::registered(suite.checked().region()).unwrap();
                assert!(suite.checked().function.body.iter().any(|stmt| {
                    matches!(stmt, crate::ir::Statement::With { .. })
                }));
                prepared.standalone()
            },
        )
        .unwrap();
    }

    #[test]
    fn nested_call_values_keep_their_checked_operands() {
        use crate::ir::{Builtin, ExprKind};

        let source = concat!(
            "async def collect(pause, sink, left, right, items):\n",
            "    item = sink(await pause(), second=left() + (lambda: right())())\n",
            "    return sink(await pause(), second=items[(lambda: item)()])\n",
            "state: int\n",
        );
        let mut prepared = PreparedBody::module_with_syntax(
            "worker",
            Path::new("worker.py"),
            source,
            0,
            ModuleOrigin::Program(None),
        )
        .unwrap();
        let function = prepared.syntax.as_ref().unwrap()[0]
            .as_function_def_stmt()
            .unwrap();
        let suite = prepared.source.definition_body(function.start());
        assert!(suite.fully_checked());
        crate::verify::registered(suite.checked().region()).unwrap();
        let mut operations = Vec::new();
        crate::ir::walk_expressions(
            &suite.checked().function.body,
            &mut |expr| match &expr.kind {
                ExprKind::Binary(..) => operations.push("binary"),
                ExprKind::Builtin(Builtin::RegisteredSubscript(..), _) => operations.push("index"),
                ExprKind::Builtin(Builtin::RegisteredLambda(..), _) => operations.push("lambda"),
                ExprKind::Builtin(Builtin::RegisteredSuspend(..), _) => operations.push("suspend"),
                _ => {}
            },
        );
        for (kind, expected) in [("binary", 1), ("index", 1), ("lambda", 2), ("suspend", 2)] {
            assert_eq!(
                operations.iter().filter(|op| **op == kind).count(),
                expected
            );
        }

        let original = prepared.standalone().unwrap();
        prepared.syntax.as_mut().unwrap().to_mut()[0]
            .as_function_def_stmt_mut()
            .unwrap()
            .body
            .clear();
        assert_eq!(prepared.standalone().unwrap(), original);
    }

    #[test]
    fn prepared_output_owns_binary_operands_after_erasure() {
        assert_prepared_binary_output(
            "async def difference():\n    return (await pause() - 1) - await right()\n",
        );
        assert_prepared_binary_output(
            "def difference():\n    return ((yield pause()) - 1) - (yield from right())\n",
        );
    }

    fn assert_prepared_binary_output(source: &str) {
        use crate::ir::{Builtin, ExprKind, Statement, SuspendValue};

        let mut prepared = PreparedBody::module_with_syntax(
            "worker",
            Path::new("worker.py"),
            source,
            0,
            ModuleOrigin::Program(None),
        )
        .unwrap();
        let original = prepared.standalone().unwrap();
        let function = prepared.syntax.as_mut().unwrap().to_mut()[0]
            .as_function_def_stmt_mut()
            .unwrap();
        let plan = prepared.source.definition_body(function.start());
        let ret = function.body[0].as_return_stmt_mut().unwrap();
        let value = ret.value.as_deref_mut().unwrap();
        assert!(plan.fully_checked());
        crate::verify::registered(plan.checked().region()).unwrap();
        let Some(Statement::Return(result)) = plan.checked().function.body.last() else {
            panic!("expected checked return");
        };
        let mut binary = 0;
        let mut suspensions = Vec::new();
        crate::ir::walk_operand_expressions(result, &mut |expr| match expr.kind {
            ExprKind::Binary(..) => binary += 1,
            ExprKind::Builtin(Builtin::RegisteredSuspend(kind, _), _) => {
                suspensions.push(kind);
            }
            _ => {}
        });
        assert_eq!(binary, 2);
        assert!(matches!(
            (function.is_async, suspensions.as_slice()),
            (true, [SuspendValue::Await, SuspendValue::Await])
                | (false, [SuspendValue::Yield, SuspendValue::Delegate])
        ));
        *value = ast::Expr::NoneLiteral(ast::ExprNoneLiteral {
            range: value.range(),
            node_index: Default::default(),
        });
        assert_eq!(prepared.standalone().unwrap(), original);
    }

    #[test]
    fn bare_annotation_targets_emit_after_erasing_their_syntax() {
        use crate::ir::{self, Builtin, ExprKind, Node};

        let path = Path::new("targets.py");
        let source = "owner.field: Unused\nvalues[(left, right):stop:step, (one, two)]: Unused\n";
        let mut module =
            PreparedBody::module("targets", path, source, 0, ModuleOrigin::Program(None)).unwrap();
        let suite = module.initializer();
        assert!(suite.fully_checked());
        let mut reads = Vec::new();
        let mut discarded = 0;
        let mut tuples = 0;
        ir::walk_nodes(&suite.checked.function.body, &mut |node, _| {
            if let Node::Expression(expr) = node {
                match &expr.kind {
                    ExprKind::Builtin(Builtin::RegisteredName(name), _) => {
                        reads.push(name.as_str())
                    }
                    ExprKind::Builtin(Builtin::RegisteredDiscard, operands) => {
                        discarded += operands.len();
                    }
                    ExprKind::Tuple(_) => tuples += 1,
                    _ => {}
                }
            }
        });
        assert_eq!(
            reads,
            [
                "owner", "values", "left", "right", "stop", "step", "one", "two"
            ]
        );
        // The tuple used as a slice bound is evaluated as one operand.
        assert_eq!(tuples, 1);
        assert_eq!(discarded, 7);
        RequiredSite::visit(
            &module.initializer().checked.function.body,
            Some(&module.source.exec_literals),
            &mut |_| panic!("unexpected declaration site"),
        );
        assert!(module.syntax.is_none());
        let withdrawn = module.standalone().unwrap();
        assert!(withdrawn.1.contains("setup_module_annotations"));
        let parsed = ParsedModule::parse(path, source.into()).unwrap();
        module.syntax = Some(Cow::Owned(module_source::into_body(parsed.parsed)));
        assert_eq!(module.standalone().unwrap(), withdrawn);
        module.syntax.as_mut().unwrap().to_mut().clear();
        assert_eq!(module.standalone().unwrap(), withdrawn);
    }

    #[test]
    fn a_deferred_module_annotation_publishes_its_provider_without_syntax() {
        let module = PreparedBody::module(
            "annotations",
            Path::new("annotations.py"),
            "value: Later\n",
            0,
            ModuleOrigin::Program(None),
        )
        .unwrap();
        assert!(module.syntax.is_none());
        RequiredSite::visit(
            &module.initializer().checked.function.body,
            Some(&module.source.exec_literals),
            &mut |_| panic!("unexpected declaration site"),
        );
        let (_, llvm) = module.standalone().unwrap();
        assert!(llvm.contains("mark_module_annotation"));
        assert!(llvm.contains("setup_module_annotations"));
        assert!(llvm.contains("__annotate__"));
        assert_eq!(llvm.matches(" = private constant %code_site").count(), 1);
    }

    #[test]
    fn an_unreached_module_declaration_keeps_its_provider_without_a_runtime_mark() {
        let module = PreparedBody::module(
            "annotations",
            Path::new("annotations.py"),
            "while False:\n    hidden: int\n",
            0,
            ModuleOrigin::Program(None),
        )
        .unwrap();
        let (_, llvm) = module.standalone().unwrap();
        assert!(llvm.contains("__annotate__"));
        assert!(!llvm.contains("mark_module_annotation"));
    }

    #[test]
    fn an_indexed_store_retains_its_code_constant_key() {
        let mut module = PreparedBody::module(
            "store",
            Path::new("store.py"),
            "target()[1000, -0.0, 'key']: int = rhs()\n",
            0,
            ModuleOrigin::Program(None),
        )
        .unwrap();
        let before = module.standalone().unwrap();
        assert!(module.syntax.is_none());
        let suite = &mut module.initializer;
        assert!(suite.fully_checked());
        let mut stored = 0;
        crate::ir::rewrite_statements(&mut suite.checked.function.body, &mut |statement| {
            if let crate::ir::Statement::SetItem(_, _, key, _) = statement {
                assert!(
                    matches!(crate::ir::Constant::from_expr(key), Some(crate::ir::Constant::Tuple(parts))
                    if parts.len() == 3 && parts[1] == crate::ir::Constant::Float((-0.0f64).to_bits()))
                );
                *key = crate::ir::Expr::integer(21);
                stored += 1;
            }
        });
        assert_eq!(stored, 1);
        assert_ne!(module.standalone().unwrap(), before);
    }

    #[test]
    fn checked_raise_places_only_reached_statements_declarations() {
        let source = concat!(
            "try:\n",
            "    raise (lambda: RuntimeError('raised child'))() from (lambda: KeyError('cause child'))()\n",
            "    def dead_in_try(): return 'dead handler body'\n",
            "except RuntimeError:\n",
            "    def caught(): return 'handled child'\n",
            "raise RuntimeError('stop')\n",
            "def dead_at_module(): return 'dead module body'\n",
        );
        let module = PreparedBody::module(
            "example",
            Path::new("example.py"),
            source,
            0,
            ModuleOrigin::Program(None),
        )
        .unwrap();
        let mut output = Output::default();
        let file = output.data(b"example.py");
        module.emit(&mut output, 0, &file).unwrap();
        for literal in ["raised child", "cause child", "handled child"] {
            assert!(output.strings.contains(literal.as_bytes()), "{literal}");
        }
        for literal in ["dead handler body", "dead module body"] {
            assert!(!output.strings.contains(literal.as_bytes()), "{literal}");
        }
    }

    #[test]
    fn physical_branches_check_the_declarations_on_their_normal_edge() {
        let path = Path::new("example.py");
        for (branch, otherwise, checks_declaration) in [
            ("if flag", "", true),
            ("if flag", "        else:\n            pass\n", true),
            (
                "if flag",
                "        else:\n            if True:\n                yield 1\n                return 2\n",
                true,
            ),
            (
                "if flag",
                "        else:\n            raise SystemExit(2)\n",
                false,
            ),
            (
                "while False",
                "        else:\n            raise SystemExit(2)\n",
                false,
            ),
            (
                "while False",
                "        else:\n            if True:\n                yield 1\n                return 2\n",
                false,
            ),
        ] {
            let source = format!(
                concat!(
                    "class Choice:\n",
                    "    def run[T: int](self, flag):\n",
                    "        {}:\n",
                    "            raise SystemExit(1)\n",
                    "{}",
                    "        async def refused(values):\n",
                    "            values += [lambda v=await get(x): v for x in [0]]\n",
                ),
                branch, otherwise,
            );
            let parsed = ParsedModule::parse(path, source.as_str().into()).unwrap();
            let module = PreparedBody::from_parsed(
                "example",
                path,
                parsed,
                0,
                ModuleOrigin::Program(None),
                LiteralSites::default(),
                Default::default(),
            );
            if checks_declaration {
                let error = module.err().expect("reachable declaration must be checked");
                assert!(
                    error.contains("cannot compile this comprehension"),
                    "{error}"
                );
            } else {
                module.unwrap().standalone().unwrap();
            }
        }
    }

    #[test]
    fn checked_raise_preserves_the_following_declarations_located_refusal() {
        let check = |source: &str, expected| {
            let path = Path::new("example.py");
            let parsed = ParsedModule::parse(path, source.into()).unwrap();
            let diagnostic = PreparedBody::from_parsed(
                "example",
                path,
                parsed,
                0,
                ModuleOrigin::Program(None),
                LiteralSites::default(),
                Default::default(),
            )
            .err()
            .expect("located declaration refusal");
            assert_eq!(diagnostic, expected);
        };
        check(
            concat!(
                "raise RuntimeError('stop')\n",
                "async def update(values):\n",
                "    values += [lambda v=await get(x): v for x in [0]]\n",
            ),
            "example.py:3:15: cannot compile this comprehension ahead of time (through 3:54)",
        );
        check(
            concat!(
                "class C:\n",
                "    def f[T](self):\n",
                "        raise RuntimeError('stop')\n",
                "        while False:\n",
                "            pass\n",
                "        async def update(values):\n",
                "            values += [lambda v=await get(x): v for x in [0]]\n",
            ),
            "example.py:7:23: cannot compile this comprehension ahead of time (through 7:62)",
        );
    }

    #[test]
    fn checked_raise_retains_its_two_suspending_operands() {
        use crate::ir::{self, Builtin, ExprKind, Statement, SuspendValue};

        let mut prepared = PreparedBody::module_with_syntax(
            "example",
            Path::new("example.py"),
            "async def throw_after(error, cause):\n    raise await error() from await cause()\nstate: int\n",
            0,
            ModuleOrigin::Program(None),
        )
        .unwrap();
        let def = prepared.syntax.as_ref().unwrap()[0]
            .as_function_def_stmt()
            .unwrap();
        let plan = prepared.source.definition_body(def.start());
        assert!(plan.fully_checked());
        let Some(Statement::Raise(Some(value))) = plan.checked().function.body.last() else {
            panic!("expected a terminal raise");
        };
        for operand in [&value.exception, value.cause.as_deref().unwrap()] {
            let mut awaits = 0;
            ir::walk_operand_expressions(operand, &mut |expr| {
                if matches!(
                    expr.kind,
                    ExprKind::Builtin(Builtin::RegisteredSuspend(SuspendValue::Await, _), _)
                ) {
                    awaits += 1;
                }
            });
            assert_eq!(awaits, 1);
        }
        let before = prepared.standalone().unwrap();
        let def = prepared.syntax.as_mut().unwrap().to_mut()[0]
            .as_function_def_stmt_mut()
            .unwrap();
        let raise = def.body[0].as_raise_stmt_mut().unwrap();
        raise.exc = None;
        raise.cause = None;
        assert_eq!(prepared.standalone().unwrap(), before);
    }

    #[test]
    fn a_physical_subscript_retains_its_receiver_and_individual_slice_bounds() {
        use crate::frontend::{
            ExpressionInput,
            SourceOperand::{Checked, Residual},
        };

        let path = Path::new("store.py");
        let text = concat!(
            "good = complete()[key()]\n",
            "result = target()[lo():receiver()[begin():(lambda v: v)(hi())]:step()]\n",
        );
        let mut prepared =
            PreparedBody::module("store", path, text, 0, ModuleOrigin::Program(None)).unwrap();
        inject_residual_child_module(&mut prepared);

        let original = prepared.standalone().unwrap();
        assert!(input_statements(prepared.initializer()).next().unwrap().0 > 0);
        let root = residual_selection(&mut prepared.initializer);
        let outer_inputs = root.physical().unwrap().operands().collect::<Vec<_>>();
        let [
            Checked(_),
            Checked(_),
            Residual(ExpressionInput {
                range,
                selected: inner,
                ..
            }),
            Checked(_),
        ] = outer_inputs.as_slice()
        else {
            panic!("expected the outer slice to own its upper-bound operation");
        };
        assert_eq!(&text[*range], "receiver()[begin():(lambda v: v)(hi())]");
        assert!(matches!(
            inner
                .physical()
                .unwrap()
                .operands()
                .collect::<Vec<_>>()
                .as_slice(),
            [Checked(_), Checked(_), Residual(_)]
        ));
        prepared.syntax = None;
        assert_eq!(prepared.standalone().unwrap(), original);
        replace_residual_input(&mut prepared, 2);
        assert_ne!(prepared.standalone().unwrap(), original);
    }

    #[test]
    fn a_mixed_comparison_keeps_its_operators_and_only_the_residual_child() {
        use crate::frontend::ExpressionInput;
        use crate::frontend::ExpressionSelection::Operand;
        use crate::frontend::SourceOperand::Residual;

        let text = concat!(
            "good = left() < right()\n",
            "result = first() < (lambda v: v)(middle()) <= last()\n",
        );
        let mut prepared = PreparedBody::module(
            "example",
            Path::new("example.py"),
            text,
            0,
            ModuleOrigin::Program(None),
        )
        .unwrap();
        inject_residual_child_module(&mut prepared);
        let original = prepared.standalone().unwrap();
        assert!(input_statements(prepared.initializer()).next().unwrap().0 > 0);
        let root = residual_occurrence(&mut prepared.initializer);
        assert_eq!(
            &text[root.range],
            "first() < (lambda v: v)(middle()) <= last()"
        );
        let inputs = root.selected.physical().unwrap();
        let crate::ir::ExprKind::Builtin(
            crate::ir::Builtin::RegisteredLocation(range, ..),
            located,
        ) = &inputs.value.kind
        else {
            panic!("expected the physical comparison location");
        };
        assert_eq!(*range, root.range);
        let crate::ir::ExprKind::Compare(values, ops) = &located[0].kind else {
            panic!("expected a registered comparison");
        };
        assert_eq!(ops.as_slice(), [ast::CmpOp::Lt, ast::CmpOp::LtE]);
        let [first, middle, last] = values.as_slice() else {
            panic!("expected three comparison operands");
        };
        assert_eq!(named_call(first).0, "first");
        assert_input(middle, 0);
        assert_eq!(named_call(last).0, "last");
        let parts = inputs.operands().collect::<Vec<_>>();
        let [
            Residual(ExpressionInput {
                range,
                selected: Operand(_),
                ..
            }),
        ] = parts.as_slice()
        else {
            panic!("expected the comparison to own its middle call");
        };
        assert_eq!(&text[*range], "(lambda v: v)(middle())");

        prepared.syntax = None;
        assert_eq!(prepared.standalone().unwrap(), original);
        replace_residual_input(&mut prepared, 0);
        assert_ne!(prepared.standalone().unwrap(), original);
    }

    #[test]
    fn a_physical_binary_keeps_its_operation_and_checked_input() {
        use crate::frontend::SourceOperand::Residual;

        let path = Path::new("store.py");
        let text = "good = before() + after()\nresult = (lambda v: v)(hi()) + more()\n";
        let mut prepared =
            PreparedBody::module("store", path, text, 0, ModuleOrigin::Program(None)).unwrap();
        inject_residual_child_module(&mut prepared);

        let original = prepared.standalone().unwrap();
        assert!(input_statements(prepared.initializer()).next().unwrap().0 > 0);
        let inputs = residual_selection(&mut prepared.initializer)
            .physical()
            .unwrap();
        let crate::ir::ExprKind::Binary(crate::ir::Binary::Add, left, right) =
            &inputs.value.without_location().kind
        else {
            panic!("expected the addition");
        };
        assert_input(left, 0);
        assert_eq!(named_call(right).0, "more");
        assert!(
            matches!(inputs.operands().collect::<Vec<_>>().as_slice(), [Residual(input)] if &text[input.range()] == "(lambda v: v)(hi())")
        );
        prepared.syntax = None;
        assert_eq!(prepared.standalone().unwrap(), original);
        replace_residual_input(&mut prepared, 0);
        assert_ne!(prepared.standalone().unwrap(), original);
    }

    #[test]
    fn a_physical_method_retains_lookup_keywords_and_checked_inputs() {
        use crate::frontend::SourceOperand::Residual;

        let mut prepared = PreparedBody::module(
            "store",
            Path::new("store.py"),
            "result = owner().apply(first(), label=(lambda v: v)(key()), tail=last())\n",
            0,
            ModuleOrigin::Program(None),
        )
        .unwrap();
        inject_residual_child_module(&mut prepared);
        let original = prepared.standalone().unwrap();
        let expected = prepared.syntax.as_ref().unwrap()[0]
            .as_assign_stmt()
            .unwrap()
            .value
            .as_call_expr()
            .unwrap()
            .arguments
            .keywords[0]
            .value
            .range();
        let inputs = residual_selection(&mut prepared.initializer)
            .physical()
            .unwrap();
        use crate::ir::{ExprKind, RegisteredArguments, RegisteredCallTarget};
        let ExprKind::Invoke {
            callable: owner,
            args,
        } = &inputs.value.without_location().kind
        else {
            panic!("expected the prepared method invocation");
        };
        let call = &args.site.python().unwrap().call;
        assert!(
            matches!(&call.target, RegisteredCallTarget::Method { name, .. } if name == "apply")
        );
        assert!(
            matches!(&call.arguments, RegisteredArguments::Keywords { positional: 1, names } if names == &["label", "tail"])
        );
        let [first, label, tail] = args.values.as_slice() else {
            panic!("expected three argument values");
        };
        assert_eq!(
            [named_call(owner).0, named_call(first).0, named_call(tail).0],
            ["owner", "first", "last"]
        );
        assert_input(label, 0);
        assert!(
            matches!(inputs.operands().collect::<Vec<_>>().as_slice(), [Residual(arg)] if arg.range() == expected)
        );
        prepared.syntax = None;
        assert_eq!(prepared.standalone().unwrap(), original);
        replace_residual_input(&mut prepared, 0);
        assert_ne!(prepared.standalone().unwrap(), original);
    }

    #[test]
    fn residual_with_reads_its_prepared_manager_after_erasing_the_call() {
        let text = "def read(open_stream, path, record):\n    with open_stream(path) as stream:\n        record((lambda: stream)())\n";
        let path = Path::new("scan.py");
        let mut prepared =
            PreparedBody::module("scan", path, text, 0, ModuleOrigin::Program(None)).unwrap();
        let parsed = ParsedModule::parse(path, text.into()).unwrap();
        prepared.syntax = Some(Cow::Owned(module_source::into_body(parsed.parsed)));
        let function = prepared.syntax.as_ref().unwrap()[0]
            .as_function_def_stmt()
            .unwrap();
        // Supply a physical function body explicitly so this test is independent of
        // which Python constructs still need that entry in an ordinary compilation.
        let lexical = prepared.source.statement_sites.expressions.take_lexical();
        let missing = HashMap::new();
        let source = crate::frontend::RegisteredSource {
            lambdas: Some(&missing),
            ..prepared.source.registered_source()
        };
        let mut physical = crate::frontend::SuitePreparation::new(
            &prepared.source.namespace_operations,
            source,
            true,
        );
        physical.sites.expressions = lexical;
        let body = physical.physical_function_suite(
            &function.body,
            crate::frontend::RegisteredScope::Function { is_async: false },
            false,
        );
        prepared.source.statement_sites = physical.sites;
        let position = function.start();
        install_test_function_body(&mut prepared.source, function, body);
        let suite = test_function_suite(&mut prepared.source, position);
        let [region] = suite.suites.as_slice() else {
            panic!("expected a prepared context")
        };
        assert!(region.operands.is_empty());
        let [
            _,
            crate::ir::Statement::With {
                binding: Some(_),
                body,
                ..
            },
        ] = &region.checked.function.body[..]
        else {
            panic!("expected the checked context and binding")
        };
        let [crate::ir::Statement::Expr(input)] = body.as_slice() else {
            panic!("expected a prepared suite input")
        };
        assert!(matches!(
            input.kind,
            crate::ir::ExprKind::Builtin(crate::ir::Builtin::RegisteredSuite(0), _)
        ));
        let original = prepared.standalone().unwrap();

        let function = prepared.syntax.as_mut().unwrap().to_mut()[0]
            .as_function_def_stmt_mut()
            .unwrap();
        function.body[0] = ast::StmtPass {
            range: function.body[0].range(),
            node_index: Default::default(),
        }
        .into();
        assert_eq!(prepared.standalone().unwrap(), original);

        // The expression statement owns the one call that still needs its lambda child.
        let suite = test_function_suite(&mut prepared.source, position);
        let [region] = suite.suites.as_mut_slice() else {
            panic!("expected the mixed context");
        };
        let body = &mut region.suites[0];
        assert!(body.refusal.is_none());
        assert!(matches!(
            input_statements(body).next().unwrap().1,
            crate::ir::Statement::Expr(_)
        ));
        let [crate::frontend::PhysicalOperand::Residual(call)] = body.operands.as_mut_slice()
        else {
            panic!("expected the residual call");
        };
        std::rc::Rc::get_mut(&mut call.selected)
            .expect("the expression statement owns the selected call")
            .replace_residual_input(0, crate::ir::Expr::none());
        assert_ne!(prepared.standalone().unwrap(), original);
    }

    #[test]
    fn residual_named_assignment_consumes_its_completed_choice_after_erasing_the_arm_syntax() {
        let mut prepared = PreparedBody::module_with_syntax(
            "program", Path::new("program.py"),
            "def yes(): return 1\ntry:\n    result = (seen := yes() if first() and second() else no())\nfinally:\n    cleanup()\n",
            0, ModuleOrigin::Program(None),
        ).unwrap();
        fn emitted(body: &PreparedBody) -> String {
            let mut out = Output::default();
            let file = out.data(b"program.py");
            body.emit(&mut out, 0, &file).unwrap();
            out.functions
        }
        let original = emitted(&prepared);
        let ast::Stmt::Try(block) = &mut prepared.syntax.as_mut().unwrap().to_mut()[1] else {
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
        let mut owns_choice = false;
        crate::ir::walk_expressions(&prepared.initializer.checked.function.body, &mut |value| {
            owns_choice |= matches!(value.kind, crate::ir::ExprKind::Conditional(..));
        });
        assert!(owns_choice);
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
    fn a_mixed_conditional_reads_only_its_residual_child() {
        use crate::frontend::{
            ExpressionInput, ExpressionSelection,
            SourceOperand::{Checked, Residual},
        };

        let path = Path::new("store.py");
        let text = "result = yes() if flag() else (lambda v: v)(make())\n";
        let mut prepared =
            PreparedBody::module("store", path, text, 0, ModuleOrigin::Program(None)).unwrap();
        inject_residual_child_module(&mut prepared);

        let original = prepared.standalone().unwrap();
        let ExpressionSelection::Operand(inputs) = residual_selection(&mut prepared.initializer)
        else {
            panic!("expected a mixed conditional");
        };
        let [
            Checked(_),
            Checked(_),
            Residual(ExpressionInput {
                range,
                selected: ExpressionSelection::Operand(_),
                ..
            }),
        ] = inputs.operands().collect::<Vec<_>>()[..]
        else {
            panic!("expected a checked test and true arm, and a residual false arm");
        };
        assert_eq!(&text[range], "(lambda v: v)(make())");
        prepared.syntax = None;
        assert_eq!(prepared.standalone().unwrap(), original);
        replace_residual_input(&mut prepared, 2);
        assert_ne!(prepared.standalone().unwrap(), original);
    }

    #[test]
    fn a_mixed_condition_keeps_its_prepared_edges_and_inputs() {
        let mut prepared = PreparedBody::module(
            "program",
            Path::new("program.py"),
            "result = yes() if first() and (lambda: second())() else no()\n",
            0,
            ModuleOrigin::Program(None),
        )
        .unwrap();
        inject_residual_child_module(&mut prepared);
        let original = prepared.standalone().unwrap();
        use crate::frontend::{ExpressionInput, ExpressionSelection, SourceOperand};
        let ExpressionSelection::Operand(inputs) = residual_selection(&mut prepared.initializer)
        else {
            panic!("expected a mixed choice");
        };
        assert!(matches!(
            inputs.operands().nth(1).unwrap(),
            SourceOperand::Residual(ExpressionInput {
                selected: ExpressionSelection::Operand(_),
                ..
            })
        ));
        prepared.syntax = None;
        assert_eq!(prepared.standalone().unwrap(), original);
    }

    #[test]
    fn awaited_branch_conditions_keep_the_enclosing_loop_checked() {
        let source = include_str!("../../../../tests/fixtures/boxed_source_coroutine.py");
        let module = PreparedBody::module(
            "boxed_source_coroutine",
            Path::new("boxed_source_coroutine.py"),
            source,
            0,
            ModuleOrigin::Provider,
        )
        .unwrap();
        let start = TextSize::try_from(source.find("async def choose(").unwrap()).unwrap();
        let function = module.source.definition_body(start);
        assert!(function.fully_checked());
        assert!(
            function
                .checked()
                .function
                .body
                .iter()
                .any(|statement| { matches!(statement, crate::ir::Statement::While(..)) })
        );
        crate::verify::registered(function.checked().region()).unwrap();
    }

    #[test]
    fn checked_async_if_owns_its_awaited_guards_and_raise() {
        use crate::ir::{self, Builtin, ExprKind, SuspendValue};

        let source = concat!(
            "async def choose(flag, predicate):\n",
            "    if flag and await predicate(1):\n        raise await failure()\n",
            "    elif not await predicate(2):\n        return 2\n",
            "state: int\n",
        );
        let mut prepared = PreparedBody::module_with_syntax(
            "example",
            Path::new("example.py"),
            source,
            0,
            ModuleOrigin::Program(None),
        )
        .unwrap();
        let original = prepared.standalone().unwrap();
        let def = prepared.syntax.as_mut().unwrap().to_mut()[0]
            .as_function_def_stmt_mut()
            .unwrap();
        let suite = prepared.source.definition_body(def.start());
        assert!(suite.fully_checked());
        let mut awaits = 0;
        ir::walk_nodes(&suite.checked().function.body, &mut |node, _| {
            if let ir::Node::Expression(expr) = node
                && matches!(
                    expr.kind,
                    ExprKind::Builtin(Builtin::RegisteredSuspend(SuspendValue::Await, _), _)
                )
            {
                awaits += 1;
            }
        });
        assert_eq!(awaits, 3);
        crate::verify::registered(suite.checked().region()).unwrap();
        def.body.clear();
        assert_eq!(prepared.standalone().unwrap(), original);
    }

    #[test]
    fn mixed_if_owns_its_guards_and_residual_arm_after_source_erasure() {
        use crate::frontend::{PhysicalOperand, RegisteredSuite};
        use crate::ir::Statement;

        fn region<'a>(prepared: &'a mut PreparedBody<'_>) -> &'a mut RegisteredSuite {
            &mut prepared.initializer.suites[0]
        }

        let source = concat!(
            "if ready():\n    accepted()\n",
            "elif probe((lambda v: v)(make())):\n",
            "    before()\n",
            "    consume((lambda v: v)(hi()))\n",
            "    after()\n",
            "else:\n    declined()\n",
        );
        let mut prepared = PreparedBody::module(
            "example",
            Path::new("example.py"),
            source,
            0,
            ModuleOrigin::Program(None),
        )
        .unwrap();

        inject_residual_child_module(&mut prepared);
        assert!(prepared.initializer().refusal.is_none());
        let [branch] = prepared.initializer().suites.as_slice() else {
            panic!("expected a prepared conditional");
        };
        let [_, Statement::If(_, first, no)] = branch.checked.function.body.as_slice() else {
            panic!("expected a conditional");
        };
        let [Statement::If(_, middle, last)] = no.as_slice() else {
            panic!("expected the elif and else arms");
        };
        assert_checked_call(first, source, "accepted()");
        assert_checked_call(last, source, "declined()");
        assert!(!branch.child_suite(middle).unwrap().fully_checked());
        assert!(matches!(
            branch.operands.as_slice(),
            [PhysicalOperand::Checked(_), PhysicalOperand::Residual(_)]
        ));
        let original = prepared.standalone().unwrap();
        prepared.syntax.as_mut().unwrap().to_mut().clear();
        assert_eq!(prepared.standalone().unwrap(), original);

        // Each retained operand must still affect the generated code on its own.
        let none = ast::Expr::NoneLiteral(ast::ExprNoneLiteral {
            range: TextRange::default(),
            node_index: Default::default(),
        });
        let checked_none = || {
            let value = crate::frontend::RegisteredOperand::prepare(
                &none,
                prepared.source.registered_source(),
                crate::frontend::RegisteredScope::Module,
            )
            .unwrap();
            PhysicalOperand::Checked(Box::new(value))
        };
        let (guard, child) = (checked_none(), checked_none());
        let saved = std::mem::replace(&mut region(&mut prepared).operands[1], guard);
        assert_ne!(prepared.standalone().unwrap(), original);
        region(&mut prepared).operands[1] = saved;
        assert_eq!(prepared.standalone().unwrap(), original);

        let [middle] = region(&mut prepared).suites.as_mut_slice() else {
            panic!("expected the arm with the residual operand");
        };
        assert!(middle.refusal.is_none());
        let [operand @ PhysicalOperand::Residual(_)] = middle.operands.as_mut_slice() else {
            panic!("expected only the call to need its source operand");
        };
        *operand = child;
        let changed_child = prepared.standalone().unwrap();
        assert_ne!(changed_child, original);

        // Input zero supplies both an object and the first guard, even with a live
        // private local zero in the same region.
        let checked = &mut region(&mut prepared).checked;
        assert!(checked.function.locals.is_empty());
        use crate::ir::{Expr, Type};
        checked.function.locals.push(Type::PythonObject);
        let branch = checked.function.body.pop().unwrap();
        checked.function.body.push(Statement::Scope {
            body: vec![
                Statement::Assign(0, Expr::registered_input(0, Type::PythonObject)),
                branch,
                Statement::Expr(Expr::local(0, Type::PythonObject)),
            ],
            clear: vec![0],
        });
        assert_eq!(
            crate::verify::registered(checked.region()).unwrap_err(),
            "registered frame storage disagrees with its scoped operations"
        );
        checked.frame_slots = crate::frontend::FrameSlots::new(vec![0].into(), HashMap::new());
        crate::verify::registered(checked.region()).unwrap();
        let scoped = prepared.standalone().unwrap();
        let load = "load i64, ptr %ir_slot0";
        assert!(scoped.1.matches(load).count() > changed_child.1.matches(load).count());

        // Removing the checked If disconnects all three arms.
        let Some(Statement::Scope { body, .. }) =
            region(&mut prepared).checked.function.body.last_mut()
        else {
            panic!("expected the private local scope")
        };
        body.remove(1);
        assert_ne!(prepared.standalone().unwrap(), scoped);
    }

    #[test]
    fn a_checked_generic_emits_its_match_after_source_erasure() {
        let source = concat!(
            "def choose[T: int](value):\n",
            "    match subject(value):\n",
            "        case 17 as captured if allow(captured): record(captured)\n",
            "        case other: record(other)\n",
        );
        let mut prepared = PreparedBody::module(
            "example",
            Path::new("example.py"),
            source,
            0,
            ModuleOrigin::Program(None),
        )
        .unwrap();
        assert!(prepared.syntax.is_none());
        let start = TextSize::new(0);
        let function = prepared.source.definition_body(start);
        assert!(function.fully_checked());
        crate::verify::registered(function.checked().region()).unwrap();

        let original = prepared.standalone().unwrap();
        let checked = prepared.source.prepared_function_mut(start);
        let cases = checked
            .function
            .body
            .iter_mut()
            .find_map(|statement| {
                if let crate::ir::Statement::Match(_, cases) = statement {
                    Some(cases)
                } else {
                    None
                }
            })
            .unwrap();
        assert_eq!(cases[0].pattern.names, ["captured"]);
        assert_eq!(cases[0].captures.len(), 1);
        assert_eq!(cases[1].pattern.names, ["other"]);
        assert_eq!(cases[1].captures.len(), 1);
        assert!(cases[1].guard.is_none());
        assert!(!cases[0].body.is_empty() && !cases[1].body.is_empty());
        assert!(cases[0].guard.take().is_some());
        assert_ne!(prepared.standalone().unwrap(), original);
    }

    #[test]
    fn a_checked_match_renders_its_suspended_guard_and_deletion_after_source_erasure() {
        let source = concat!(
            "async def evaluate(seed):\n",
            "    match sink(await gate(), second=left() + table()[(lambda: seed)()]):\n",
            "        case prior if decline(prior): return prior\n",
            "        case value if accepted(await pause(value)): return value\n",
            "        case item: del table[await key(item)]\n",
            "state: int\n",
        );
        let mut prepared = PreparedBody::module_with_syntax(
            "example",
            Path::new("example.py"),
            source,
            0,
            ModuleOrigin::Program(None),
        )
        .unwrap();
        let function = prepared.syntax.as_ref().unwrap()[0]
            .as_function_def_stmt()
            .unwrap();
        let suite = prepared.source.definition_body(function.start());
        assert!(suite.fully_checked());
        crate::verify::registered(suite.checked().region()).unwrap();
        let original = prepared.standalone().unwrap();
        prepared.syntax.as_mut().unwrap().to_mut()[0]
            .as_function_def_stmt_mut()
            .unwrap()
            .body
            .clear();
        assert_eq!(prepared.standalone().unwrap(), original);
    }

    #[test]
    fn checked_matches_own_their_suspended_subject() {
        let selector = "\n            try:\n                raise LookupError(item)\n            except (await Select(item)):\n                return item";
        let manager = "\n            with manager(await key(item)): pass";
        let loop_default = concat!(
            "\n            for entry in rows:\n",
            "                with manager(lambda value=(await key(entry)): value): pass",
        );
        let protected = concat!(
            "\n            with TemporaryDirectory() as directory:\n",
            "                path = Path(directory)\n",
            "                async with asyncio.TaskGroup() as group:\n",
            "                    left = group.create_task(partner(path, 'left', 'right'))\n",
            "                child = await asyncio.create_subprocess_exec('/bin/sh', '-c', 'exit 7', stdout=DEVNULL)\n",
            "                assert await child.wait() == 7",
        );
        for body in [
            "return item",
            "raise await failure(item)",
            "del table[await key(item)]",
            selector,
            manager,
            loop_default,
            protected,
        ] {
            let source = format!(
                "async def evaluate(seed):\n    match sink(await gate(), second=left() + table()[(lambda: seed)()]):\n        case prior if decline(prior): return prior\n        case item: {body}\nstate: int\n"
            );
            let mut prepared = PreparedBody::module_with_syntax(
                "example",
                Path::new("example.py"),
                &source,
                0,
                ModuleOrigin::Program(None),
            )
            .unwrap();
            let function = prepared.syntax.as_ref().unwrap()[0]
                .as_function_def_stmt()
                .unwrap();
            let start = function.start();
            let suite = prepared.source.definition_body(start);
            assert!(suite.fully_checked(), "{body}");
            crate::verify::registered(suite.checked().region()).unwrap();
            let original = prepared.standalone().unwrap();
            let function = prepared.syntax.as_mut().unwrap().to_mut()[0]
                .as_function_def_stmt_mut()
                .unwrap();
            function.body.clear();
            assert_eq!(prepared.standalone().unwrap(), original);
            let checked = prepared.source.prepared_function_mut(start);
            let [_, crate::ir::Statement::Match(subject, cases), ..] =
                &mut checked.function.body[..]
            else {
                panic!("expected a checked match")
            };
            if body == selector || body == manager {
                match &mut cases[1].body[..] {
                    [_, crate::ir::Statement::Try { handlers, .. }] if body == selector => {
                        let crate::ir::ExceptionMatcher::Object(range, operand) =
                            &mut handlers[0].matcher
                        else {
                            panic!("expected an exception-class operand")
                        };
                        assert_eq!(&source[*range], "await Select(item)");
                        **operand = crate::ir::Expr::none();
                    }
                    [_, crate::ir::Statement::With { manager, site, .. }] => {
                        assert_eq!(&source[site.range], "manager(await key(item))");
                        *manager = crate::ir::Expr::none();
                    }
                    _ => panic!("expected the checked capture-case operation"),
                }
            } else {
                *subject = crate::ir::Expr::none();
            }
            crate::verify::registered(checked.region()).unwrap();
            assert_ne!(prepared.standalone().unwrap(), original);
        }
    }

    #[test]
    fn a_registered_match_renders_after_its_checked_input_syntax_is_erased() {
        let source = concat!(
            "match subject():\n",
            "    case 1 as item if allow(item):\n        keep(item)\n",
            "    case other:\n        before(other)\n        consume((lambda v: v)(other))\n        after(other)\n",
        );
        let mut prepared = PreparedBody::module(
            "example",
            Path::new("example.py"),
            source,
            0,
            ModuleOrigin::Program(None),
        )
        .unwrap();
        fn emit(prepared: &PreparedBody) -> String {
            let mut out = Output::default();
            let file = out.data(b"example.py");
            prepared.emit(&mut out, 0, &file).unwrap();
            out.functions
        }
        inject_residual_child_module(&mut prepared);
        assert!(!prepared.initializer().fully_checked());
        let original = emit(&prepared);
        let ast::Stmt::Match(branch) = &mut prepared.syntax.as_mut().unwrap().to_mut()[0] else {
            panic!("expected match")
        };
        let suite = &prepared.initializer;
        let [region] = suite.suites.as_slice() else {
            panic!("expected a mixed match")
        };
        let [_, crate::ir::Statement::Match(subject, cases)] =
            region.checked.function.body.as_slice()
        else {
            panic!("expected a checked match")
        };
        assert!(matches!(
            subject.kind,
            crate::ir::ExprKind::Builtin(crate::ir::Builtin::RegisteredInput(0), _)
        ));
        assert_eq!(cases[0].pattern.names, ["item"]);
        let [item] = cases[0].captures.as_slice() else {
            panic!("expected one capture")
        };
        assert_eq!(region.checked.local_names[item].name, "item");
        assert!(matches!(
            cases[0].guard.as_ref().map(|g| &g.kind),
            Some(crate::ir::ExprKind::Builtin(
                crate::ir::Builtin::RegisteredInput(1),
                _
            ))
        ));
        assert!(matches!(
            region.operands.as_slice(),
            [
                crate::frontend::PhysicalOperand::Checked(_),
                crate::frontend::PhysicalOperand::Checked(_)
            ]
        ));
        assert_checked_call(&cases[0].body, source, "keep(item)");
        let mixed = region.child_suite(&cases[1].body).unwrap();
        assert!(mixed.refusal.is_none());
        let positions: Vec<_> = input_statements(mixed).map(|(at, _)| at).collect();
        assert!(
            matches!(positions.as_slice(), [at] if *at > 0 && *at + 1 < mixed.checked.function.body.len())
        );
        *branch.subject = ast::Expr::NoneLiteral(ast::ExprNoneLiteral {
            range: branch.subject.range(),
            node_index: Default::default(),
        });
        branch.cases[0].guard = None;
        branch.cases[0].pattern = branch.cases[1].pattern.clone();
        branch.cases[0].body.clear();
        let pass = ruff_python_parser::parse_module("pass")
            .unwrap()
            .into_syntax()
            .body
            .pop()
            .unwrap();
        branch.cases[1].body[0] = pass.clone();
        branch.cases[1].body[2] = pass;
        assert_eq!(emit(&prepared), original);
    }

    #[test]
    fn annotation_comprehensions_emit_after_their_clauses_are_erased() {
        let mut prepared = PreparedBody::module_with_syntax(
            "example",
            Path::new("example.py"),
            "class C:\n    def f(self, x: [outer for _ in (1,)], y: [item for item in [outer]]): pass\n",
            0,
            ModuleOrigin::Program(None),
        )
        .unwrap();
        let class = prepared.syntax.as_ref().unwrap()[0]
            .as_class_def_stmt()
            .unwrap();
        let method = class.body[0].as_function_def_stmt().unwrap();
        let header = prepared.source.scopes.function(method);
        let provider = header.annotations.as_ref().unwrap();
        assert_eq!(provider.start, method.parameters.start());
        let body = provider.body.as_ref().unwrap();
        use crate::ir::{Builtin, Expr, ExprKind, Statement};
        assert!(matches!(
            body.function.body.as_slice(),
            [Statement::Return(Expr { kind: ExprKind::Builtin(Builtin::RegisteredAnnotations { .. }, values), .. })]
                if values.len() == 2
        ));
        assert!(!body.local_names.is_empty());
        let original = prepared.standalone().unwrap();
        let class = prepared.syntax.as_mut().unwrap().to_mut()[0]
            .as_class_def_stmt_mut()
            .unwrap();
        let method = class.body[0].as_function_def_stmt_mut().unwrap();
        for parameter in &mut method.parameters.args[1..] {
            let annotation = parameter.parameter.annotation.as_mut().unwrap();
            *annotation = annotation.as_list_comp_expr().unwrap().elt.clone();
        }
        assert_eq!(prepared.standalone().unwrap(), original);
    }

    #[test]
    fn decoded_template_annotation_keeps_its_checked_interpolation() {
        use crate::frontend::{AnnotationBody, AnnotationCode};
        use crate::ir::{Builtin, ExprKind};

        let prepared = PreparedBody::module(
            "example",
            Path::new("example.py"),
            r#"def example(x: "t'{capture(x)!r:>{width}}'"): pass"#,
            0,
            ModuleOrigin::Program(None),
        )
        .unwrap();
        let mut sites = prepared.source.annotation_children.values();
        let Some(AnnotationBody {
            code: AnnotationCode { operand: child, .. },
            ..
        }) = sites.next()
        else {
            panic!("decoded template annotation needs a checked child");
        };
        assert!(sites.next().is_none());
        let mut captures = Vec::new();
        crate::ir::walk_operand_expressions(&child.value, &mut |value| {
            if let ExprKind::Builtin(
                Builtin::RegisteredInterpolation {
                    expression,
                    conversion,
                },
                _,
            ) = &value.kind
            {
                captures.push((expression.as_str(), *conversion));
            }
        });
        assert_eq!(captures, [("capture(x)", Some('r'))]);
    }

    #[test]
    fn a_mixed_suite_uses_the_entire_prepared_template() {
        let path = Path::new("field.py");
        let original = r#"q = t"left{(lambda v: v)(xs)!r:>4}right""#;
        let mut prepared =
            PreparedBody::module("field", path, original, 0, ModuleOrigin::Program(None)).unwrap();
        assert_eq!(
            prepared
                .source
                .statement_sites
                .expressions
                .templates()
                .count(),
            1
        );
        inject_residual_child_module(&mut prepared);
        assert!(!prepared.initializer().fully_checked());
        let emit = |prepared: &PreparedBody<'_>| {
            let mut output = Output::default();
            let file = output.data(b"field.py");
            prepared.emit(&mut output, 0, &file).unwrap();
            (output.constants, output.functions)
        };
        let expected = emit(&prepared);

        // Keep the site range while changing both its metadata and its operand.
        // The mixed suite must emit only the prepared operation.
        let replace_body = |text| {
            ruff_python_parser::parse_module(text)
                .unwrap()
                .into_syntax()
                .body
        };
        prepared.syntax = Some(Cow::Owned(replace_body(
            r#"q = t"xxxx{(lambda v: v)(xs)!s:^9}yyyyy""#,
        )));
        assert_eq!(emit(&prepared), expected);
        prepared.syntax = Some(Cow::Owned(replace_body(
            r#"q = t"xxxx{(lambda v: v)(ys)!s:^9}yyyyy""#,
        )));
        assert_eq!(emit(&prepared), expected);
    }

    #[test]
    fn checked_named_entries_keep_their_registration_after_their_bodies_are_dropped() {
        let mut module = PreparedBody::module_with_syntax(
            "entries",
            Path::new("entries.py"),
            concat!(
                "def sign(value: float, /):\n",
                "    'Record the sign.'\n",
                "    global positive\n",
                "    positive = value > 0\n",
                "    return positive\n",
                "def closure(value):\n",
                "    return lambda: value\n",
            ),
            0,
            ModuleOrigin::Program(None),
        )
        .unwrap();
        let before = module.standalone().unwrap();
        let defs = module.syntax.as_mut().unwrap().to_mut();
        for (i, stmt) in defs.iter_mut().enumerate() {
            let def = stmt.as_function_def_stmt_mut().unwrap();
            let plan = module.source.definition_body(def.start());
            assert!(plan.fully_checked());
            let body = plan.checked();
            let mut required = Vec::new();
            RequiredSite::visit(&body.function.body, None, &mut |site| required.push(site));
            if i == 0 {
                assert!(required.is_empty());
                let header = &module.source.scopes.functions[&def.start()];
                assert_eq!(header.doc.as_deref(), Some("Record the sign."));
                assert_eq!(
                    header.metadata.signature(0, false)["integer_sign_parameters"],
                    serde_json::json!({"value": true})
                );
            } else {
                assert!(matches!(required.as_slice(), [RequiredSite::Lambda(..)]));
            }
            def.body.clear();
        }
        assert!(module.standalone().unwrap() == before);
    }

    #[test]
    fn independent_module_keeps_its_registered_entry_after_releasing_syntax() {
        let path = Path::new("state.py");
        let source = concat!(
            "'Module state.'\n",
            "global offset\n",
            "inputs = [2, 3]\n",
            "offset = inputs[0] + inputs[1]\n",
            "while offset:\n",
            "    break\n",
            "    def skipped(): pass\n",
        );
        for optimization in [0, 2] {
            let mut module = PreparedBody::module(
                "state",
                path,
                source,
                optimization,
                ModuleOrigin::Program(None),
            )
            .unwrap();
            assert!(module.syntax.is_none());
            let entry = module.standalone().unwrap();

            // Exercise the same plan through the entry used by modules with residual code.
            let parsed = ParsedModule::parse(path, source.into()).unwrap();
            module.syntax = Some(Cow::Owned(module_source::into_body(parsed.parsed)));
            assert!(module.standalone().unwrap() == entry);
        }
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
        assert!(prepared.initializer().fully_checked());
        assert!(prepared.syntax.is_none());
        prepared.source.statement_sites.expressions = Default::default();

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
        assert!(
            output
                .functions
                .contains("call i64 @imre_dyn_interpolation(")
        );
        for operation in ["spread_arguments", "format", "build_template"] {
            let symbol = output.strings.get_index_of(operation.as_bytes()).unwrap();
            assert!(
                output.functions.contains(&format!(
                    "call i64 @imre_dyn_op_at(ptr %ctx, ptr @str{symbol},"
                )),
                "{operation}"
            );
        }
    }

    #[test]
    fn checked_generator_emits_after_source_body_is_dropped() {
        use crate::ir::{Builtin, ExprKind};

        let mut prepared = PreparedBody::module(
            "scan",
            Path::new("scan.py"),
            "found = any(t'{(matches(y) for xs in rows() if keep(xs) for y in xs)!r}'.values[0])\n",
            0,
            ModuleOrigin::Program(None),
        )
        .unwrap();
        let initializer = prepared.initializer();
        assert!(initializer.fully_checked());
        assert_eq!(prepared.source.generators.len(), 1);
        let (site, generator) = prepared.source.generators.iter().next().unwrap();
        let Some((_, crate::frontend::ExpressionSelection::Operand(template))) = prepared
            .source
            .statement_sites
            .expressions
            .templates()
            .next()
        else {
            panic!("expected a fully checked template");
        };
        let mut references = Vec::new();
        let mut caller_names = Vec::new();
        crate::ir::walk_operand_expressions(&template.value, &mut |expr| match &expr.kind {
            ExprKind::Builtin(Builtin::RegisteredGenerator(range, ..), _) => {
                references.push(*range)
            }
            ExprKind::Builtin(Builtin::RegisteredName(name), _) => caller_names.push(name.as_str()),
            _ => {}
        });
        assert_eq!(references, [*site]);
        assert_eq!(caller_names, ["rows"]);
        let mut pulls = 0;
        crate::ir::walk_statements(&generator.body.function.body, &mut |stmt| {
            pulls += usize::from(matches!(stmt, crate::ir::Statement::IterNext { .. }));
        });
        assert_eq!(pulls, 2);
        assert!(prepared.syntax.is_none());
        prepared.source.statement_sites.expressions = Default::default();

        let mut output = Output::default();
        let file = output.data(b"scan.py");
        prepared.emit(&mut output, 0, &file).unwrap();
        assert_eq!(
            output
                .functions
                .matches("call i64 @imre_dyn_define(")
                .count(),
            1
        );
        assert_eq!(
            output
                .functions
                .matches("call void @imre_dyn_suspend_operands(")
                .count(),
            1
        );
    }

    #[test]
    fn generator_creation_emits_the_prepared_child_after_its_clauses_are_erased() {
        let mut prepared = PreparedBody::module_with_syntax(
            "scan",
            Path::new("scan.py"),
            concat!(
                "def receive():\n",
                "    cursor = (emit(y) for x in (yield [key for key in keys]) if keep(x) for y in children(x))\n",
                "    yield cursor\n",
            ),
            0,
            ModuleOrigin::Program(None),
        )
        .unwrap();
        let enclosing = prepared.syntax.as_ref().unwrap()[0]
            .as_function_def_stmt()
            .unwrap();
        let site = enclosing.body[0].as_assign_stmt().unwrap().value.range();
        assert!(prepared.source.generators.contains_key(&site));
        let before = prepared.standalone().unwrap();
        assert!(before.1.contains("call void @imre_dyn_suspend_operands("));

        let enclosing = prepared.syntax.as_mut().unwrap().to_mut()[0]
            .as_function_def_stmt_mut()
            .unwrap();
        let assignment = enclosing.body[0].as_assign_stmt_mut().unwrap();
        let generator = assignment.value.as_generator_expr_mut().unwrap();
        // The caller still evaluates the outer iterable. Only the child owns
        // the target, filters, inner loop and yield operand.
        generator.generators.truncate(1);
        let outer = &mut generator.generators[0];
        outer.ifs.clear();
        for operand in [generator.elt.as_mut(), &mut outer.target] {
            *operand = ast::Expr::NoneLiteral(ast::ExprNoneLiteral {
                range: operand.range(),
                node_index: Default::default(),
            });
        }
        assert_eq!(prepared.standalone().unwrap(), before);
    }

    #[test]
    fn a_residual_operand_keeps_its_entire_template_at_the_source_boundary() {
        let mut prepared = PreparedBody::module(
            "query",
            Path::new("query.py"),
            "first = t'{client.lookup(*args)}{v * 2}'
second = t'{client.lookup(*args)}{(lambda v: v)(values)}'",
            0,
            ModuleOrigin::Program(None),
        )
        .unwrap();
        assert_eq!(
            prepared
                .source
                .statement_sites
                .expressions
                .templates()
                .count(),
            2
        );
        inject_residual_child_module(&mut prepared);
        let sites = &prepared.source.statement_sites.expressions;
        assert_eq!(sites.templates().count(), 2);
        assert!(!prepared.initializer().fully_checked());
    }

    #[test]
    fn bundled_class_methods_have_their_own_checked_suite() {
        PreparedBody::with_module(
            "reader",
            Path::new("reader.py"),
            "class Reader:\n    def read(self):\n        value = self.source()\n        return value\n",
            0,
            ModuleOrigin::Bundled,
            |prepared| {
                let methods = &prepared.source.scopes.functions;
                assert_eq!(methods.len(), 1);
                assert!(methods.keys().all(|site| prepared.source.definition_body(*site).fully_checked()));
                assert!(prepared.syntax.is_none());
                prepared.standalone()
            },
        ).unwrap();
    }

    #[test]
    fn physical_worker_targets_use_checked_method_invocations() {
        use crate::ir::{self, RegisteredCallTarget};
        use serde_json::json;

        for (source, target, key, expected, calls, excluded) in [
            (
                include_str!("../../../../tests/fixtures/boxed_thread_bound_lock.py"),
                "def task(",
                "thread_bound_lock_field",
                json!("gate"),
                1,
                "def replacement(",
            ),
            (
                include_str!("../../../../tests/fixtures/boxed_thread_method_closure.py"),
                "@staticmethod",
                "thread_captured_locks",
                json!(["gate"]),
                1,
                "def make_wait(",
            ),
            (
                include_str!("../../../../tests/fixtures/boxed_threading_tcp_server.py"),
                "def acquire_on_worker(",
                "thread_lock_only",
                json!(true),
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
            let functions = &prepared.source.scopes.functions;
            assert_eq!(
                functions[&start(target)].metadata.signature(0, false)[key],
                expected
            );
            let excluded = functions[&start(excluded)].metadata.signature(0, false);
            assert!(
                [
                    "thread_bound_lock_field",
                    "thread_captured_locks",
                    "thread_lock_only"
                ]
                .iter()
                .all(|key| excluded.get(key).is_none())
            );
            let suite = prepared.source.definition_body(start(target));
            assert!(suite.fully_checked());
            let mut methods = Vec::new();
            ir::walk_expressions(&suite.checked().function.body, &mut |expr| {
                if let Some(call) = expr.python_call()
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
            .original_fixture_suite()
            .unwrap()
            .iter()
            .filter_map(ast::Stmt::as_type_alias_stmt)
        {
            let shared = crate::frontend::AnnotationCode::prepare(
                &alias.value,
                prepared.source.registered_source(),
            );
            let checked = prepared.source.type_aliases[&alias.start()].body.is_ok();
            let name = alias.name.as_name_expr().unwrap().id.as_str();
            assert_eq!(checked, shared.is_some(), "{name}");
            assert!(checked, "expected checked operands for {name}");
        }
        prepared.standalone().unwrap();
    }

    #[test]
    fn boxed_type_alias_values_emit_after_withdrawing_the_alias_statements() {
        use ast::visitor::transformer::{self, Transformer};

        struct WithdrawAliases<'a> {
            definitions: &'a HashMap<ruff_text_size::TextSize, TypeAliasDefinition>,
            count: std::cell::Cell<usize>,
        }
        impl Transformer for WithdrawAliases<'_> {
            fn visit_stmt(&self, stmt: &mut ast::Stmt) {
                if let ast::Stmt::TypeAlias(alias) = stmt {
                    let definition = &self.definitions[&alias.start()];
                    assert_eq!(
                        definition.name,
                        alias.name.as_name_expr().unwrap().id.as_str()
                    );
                    assert!(definition.body.is_ok(), "{}", definition.name);
                    self.count.set(self.count.get() + 1);
                    *stmt = ast::StmtPass {
                        range: alias.range(),
                        node_index: Default::default(),
                    }
                    .into();
                } else {
                    transformer::walk_stmt(self, stmt);
                }
            }
        }

        let source = include_str!("../../../../tests/fixtures/boxed_type_alias_values.py");
        let path = Path::new("boxed_type_alias_values.py");
        let mut prepared = PreparedBody::module(
            "boxed_type_alias_values",
            path,
            source,
            0,
            ModuleOrigin::Program(None),
        )
        .unwrap();
        assert!(prepared.syntax.is_none());
        let original = prepared.standalone().unwrap();

        let parsed = ParsedModule::parse(path, source.into()).unwrap();
        let mut body = module_source::into_body(parsed.parsed);
        let withdraw = WithdrawAliases {
            definitions: &prepared.source.type_aliases,
            count: Default::default(),
        };
        withdraw.visit_body(&mut body);
        assert_eq!(withdraw.count.get(), 4);
        assert_eq!(withdraw.definitions.len(), 4);
        prepared.syntax = Some(Cow::Owned(body));
        assert_eq!(prepared.standalone().unwrap(), original);
    }

    #[test]
    fn checked_alias_site_emits_after_dropping_the_ruff_suite() {
        let prepared = PreparedBody::module(
            "model",
            Path::new("model.py"),
            "type PublicRow[T] = payload[T]\n",
            0,
            ModuleOrigin::Program(None),
        )
        .unwrap();
        assert!(prepared.initializer().fully_checked());
        assert!(prepared.syntax.is_none());

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

        let mut prepared = PreparedBody::module_with_syntax(
            "editor",
            Path::new("editor.py"),
            concat!(
                "class Owner:\n",
                "    __secret = int\n",
                "    type Factory = (\n",
                "        [__secret, lambda item: t'{item}']\n",
                "    )\n",
            ),
            0,
            ModuleOrigin::Program(None),
        )
        .unwrap();
        let alias = prepared.source.type_aliases.values().next().unwrap();
        let body = &alias
            .body
            .as_ref()
            .expect("checked alias value")
            .operand
            .value;
        let mut sites = Vec::new();
        crate::ir::walk_operand_expressions(body, &mut |expr| {
            if let ExprKind::Builtin(Builtin::RegisteredLambda(site, _), _) = &expr.kind {
                sites.push(*site);
            }
        });
        let [site] = sites.as_slice() else {
            panic!("expected one checked lambda binding in the alias evaluator");
        };
        assert!(prepared.source.lambdas[site].body.is_some());

        let emit = |prepared: &PreparedBody<'_>| {
            let mut output = Output::default();
            let file = output.data(b"editor.py");
            prepared.emit(&mut output, 0, &file).unwrap();
            (output, file)
        };
        let (original, _) = emit(&prepared);
        assert!(prepared.syntax.take().is_some());
        let (mut output, file) = emit(&prepared);
        assert_eq!(original.constants, output.constants);
        assert_eq!(original.functions, output.functions);

        let format_op = output.data(b"annotation_format");
        let class_lookup = output.data(b"lookup_class_annotation_global");
        let private_name = output.data(b"_Owner__secret");
        let evaluator = output
            .functions
            .split("\n}")
            .find(|body| {
                body.contains(&format!(
                    "call i64 @imre_dyn_op_at(ptr %ctx, ptr {format_op},"
                ))
            })
            .expect("alias evaluator");
        assert!(evaluator.lines().any(|line| {
            line.contains("call void @imre_dyn_statement_with_exception(")
                && line.contains(&format!(", ptr {file}, i32 4,"))
        }));
        assert!(evaluator.lines().any(|line| {
            line.contains(&format!(
                "call i64 @imre_boxed_named_op(ptr %ctx, ptr {class_lookup},"
            )) && line.contains(&format!("ptr {private_name},"))
        }));
        let (_, operands) = evaluator
            .split_once("call i64 @imre_dyn_define(ptr %ctx, ptr ")
            .expect("lambda binding in the alias evaluator");
        let (lambda_site, _) = operands.split_once(',').unwrap();
        assert!(output.constants.lines().any(|line| {
            line.starts_with(&format!(
                "{lambda_site} = private constant %code_site {{ ptr "
            )) && line.ends_with(&format!(", ptr {file}, i32 4, i32 0 }}"))
        }));
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
        let initializer = prepared.initializer();
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
            .operand
            .value;
        let ExprKind::Scope { body, value, .. } = &callback.kind else {
            panic!("expected a guarded alias value");
        };
        let [Statement::Expr(format)] = body.as_slice() else {
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
    fn both_finally_paths_bind_the_same_function_after_withdrawing_its_declaration() {
        let source = concat!(
            "def closing(seed):\n",
            "    try:\n",
            "        seed.marker: int\n",
            "        yield 'open'\n",
            "    finally:\n",
            "        @decorate('outer')\n",
            "        @decorate('inner')\n",
            "        def cleanup(first=mark(seed), *, last=mark(seed + 1)):\n",
            "            return first, last, seed\n",
            "        yield cleanup\n",
        );
        let mut prepared = PreparedBody::module_with_syntax(
            "closing",
            Path::new("closing.py"),
            source,
            0,
            ModuleOrigin::Provider,
        )
        .unwrap();
        let original = prepared.standalone().unwrap();
        // Both exits bind cleanup at the same site inside closing.
        assert_eq!(
            original.1.matches(" = private constant %code_site").count(),
            2
        );
        assert_eq!(original.1.matches("call i64 @imre_dyn_define(").count(), 2);
        let closing = prepared.syntax.as_mut().unwrap().to_mut()[0]
            .as_function_def_stmt_mut()
            .unwrap();
        let cleanup = &mut closing.body[0].as_try_stmt_mut().unwrap().finalbody[0];
        let site = cleanup.start();
        *cleanup = ast::StmtPass {
            range: cleanup.range(),
            node_index: Default::default(),
        }
        .into();
        assert_eq!(prepared.source.scopes.functions[&site].name, "cleanup");
        assert!(prepared.standalone().unwrap() == original);
    }

    #[test]
    fn mixed_try_retains_its_matcher_and_suites_after_erasing_the_statement() {
        use crate::frontend::{
            PhysicalOperand, RegisteredScope, RegisteredSource, SuitePreparation,
        };

        let source = concat!(
            "async def run(matcher, record, finish):\n",
            "    try:\n",
            "        await record((lambda: 1)())\n",
            "    except (lambda: matcher)() as caught:\n",
            "        record(caught)\n",
            "    else:\n",
            "        record('clear')\n",
            "    finally:\n",
            "        await finish((lambda: 2)())\n",
        );
        let mut module = PreparedBody::module(
            "example",
            Path::new("example.py"),
            source,
            0,
            ModuleOrigin::Program(None),
        )
        .unwrap();
        let parsed = ruff_python_parser::parse_module(source).unwrap();
        module.syntax = Some(Cow::Owned(parsed.into_syntax().body));
        let function = module.syntax.as_ref().unwrap()[0]
            .as_function_def_stmt()
            .unwrap();
        let function_start = function.start();
        // Withhold the descriptors from suite preparation alone so these operands
        // still use their real compiled lambda children during emission.
        let lexical = module.source.statement_sites.expressions.take_lexical();
        let missing = HashMap::new();
        let values = RegisteredSource {
            lambdas: Some(&missing),
            ..module.source.registered_source()
        };
        let mut plan = SuitePreparation::new(&module.source.namespace_operations, values, true);
        plan.sites.expressions = lexical;
        let body = plan.physical_function_suite(
            &function.body,
            RegisteredScope::Function { is_async: true },
            true,
        );
        module.source.statement_sites = plan.sites;
        install_test_function_body(&mut module.source, function, body);

        let [region] = test_function_suite(&mut module.source, function_start)
            .suites
            .as_slice()
        else {
            panic!("expected a registered try with prepared children")
        };
        let [
            _,
            crate::ir::Statement::Try {
                body,
                handlers,
                orelse,
                finalbody,
            },
        ] = &region.checked.function.body[..]
        else {
            panic!("expected a checked try")
        };
        let child = |body: &[crate::ir::Statement]| region.child_suite(body).unwrap();
        let crate::ir::ExceptionMatcher::Object(site, class) = &handlers[0].matcher else {
            panic!("expected a prepared exception selector")
        };
        let crate::ir::ExprKind::Builtin(crate::ir::Builtin::RegisteredLocation(range, ..), args) =
            &class.kind
        else {
            panic!("expected a located selector")
        };
        assert_eq!(site, range);
        assert!(matches!(
            args.as_slice(),
            [crate::ir::Expr {
                kind: crate::ir::ExprKind::Builtin(crate::ir::Builtin::RegisteredInput(0), _),
                ..
            }]
        ));
        assert!(matches!(
            &region.operands[..],
            [PhysicalOperand::Residual(_)]
        ));
        assert!(!child(body).fully_checked());
        assert_checked_call(orelse, source, "record('clear')");
        assert_eq!(
            region.checked.local_names[&handlers[0].name.unwrap()].name,
            "caught"
        );
        assert!(!child(finalbody).fully_checked());
        let original = module.standalone().unwrap();
        let function = module.syntax.as_mut().unwrap().to_mut()[0]
            .as_function_def_stmt_mut()
            .unwrap();
        let stmt = &mut function.body[0];
        *stmt = ast::StmtPass {
            range: stmt.range(),
            node_index: Default::default(),
        }
        .into();
        assert_eq!(module.standalone().unwrap(), original);

        let [region] = test_function_suite(&mut module.source, function_start)
            .suites
            .as_mut_slice()
        else {
            unreachable!()
        };
        let [_, crate::ir::Statement::Try { handlers, .. }] = &mut region.checked.function.body[..]
        else {
            unreachable!()
        };
        handlers[0].name = None;
        assert_ne!(module.standalone().unwrap(), original);
    }

    #[test]
    fn nested_finally_after_return_only_emits_reachable_cleanup() {
        let source = concat!(
            "def wait(work, release, reacquire):\n",
            "    try:\n",
            "        try:\n",
            "            return work()\n",
            "        finally:\n",
            "            release()\n",
            "    finally:\n",
            "        reacquire()\n",
        );
        let (_, ir) = PreparedBody::module(
            "locks",
            Path::new("locks.py"),
            source,
            0,
            ModuleOrigin::Program(None),
        )
        .unwrap()
        .standalone()
        .unwrap();
        for line in [6, 8] {
            let position = format!(", i32 {line}, i64 ");
            let calls = ir.lines().filter(|instruction| {
                instruction.contains("call void @imre_dyn_statement_with_exception(")
                    && instruction.contains(&position)
            });
            assert_eq!(calls.count(), 1, "cleanup on line {line}");
        }
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
    fn suspended_index_update_keeps_its_operands_after_source_erasure() {
        let mut prepared = PreparedBody::module_with_syntax(
            "example",
            Path::new("example.py"),
            "def update():\n    (yield 'receiver')[(yield 'key')] += (yield 'rhs')\n",
            0,
            ModuleOrigin::Program(None),
        )
        .unwrap();
        let before = prepared.standalone().unwrap();
        let def = prepared.syntax.as_mut().unwrap().to_mut()[0]
            .as_function_def_stmt_mut()
            .unwrap();
        let suite = prepared.source.definition_body(def.start());
        assert!(suite.fully_checked());
        assert!(
            suite
                .checked()
                .function
                .body
                .iter()
                .any(|statement| { matches!(statement, crate::ir::Statement::AugmentItem(..)) })
        );
        def.body[0] = ast::Stmt::Pass(ast::StmtPass {
            range: def.body[0].range(),
            node_index: Default::default(),
        });
        assert_eq!(prepared.standalone().unwrap(), before);
    }

    #[test]
    fn mixed_augmented_updates_preserve_the_rhs_diagnostic() {
        for (target, start, end) in [("values", 15, 54), ("values[0]", 18, 57)] {
            let source = format!(
                "async def update(values):\n    {target} += [lambda v=await get(x): v for x in [0]]\n"
            );
            let prepared = PreparedBody::module(
                "update",
                Path::new("update.py"),
                &source,
                0,
                ModuleOrigin::Program(None),
            )
            .unwrap();
            assert_eq!(
                prepared.standalone().unwrap_err(),
                format!(
                    "update.py:2:{start}: cannot compile this comprehension ahead of time (through 2:{end})"
                )
            );
        }
    }

    #[test]
    fn suspending_assertions_prepare_their_external_expression_children() {
        let source = concat!(
            "def verify(left, right):\n",
            "    assert left() and (yield right()), 'failed'\n",
            "def explain():\n",
            "    assert (yield 'first') and (yield 'second'), 1 + (yield 'message')\n",
        );
        let module = PreparedBody::module(
            "program",
            Path::new("program.py"),
            source,
            0,
            ModuleOrigin::Program(None),
        )
        .unwrap();
        module.standalone().unwrap();
    }

    #[test]
    fn checked_assertion_emits_after_erasing_its_entire_suite() {
        let text = "assert probe(), message()\n";
        let mut prepared = PreparedBody::module(
            "proof",
            Path::new("proof.py"),
            text,
            0,
            ModuleOrigin::Program(None),
        )
        .unwrap();
        let plan = prepared.initializer();
        assert!(plan.fully_checked());
        assert!(matches!(
            plan.checked.function.body.as_slice(),
            [_, crate::ir::Statement::If(_, _, _)]
        ));
        prepared.syntax = Some(Cow::Owned(
            ruff_python_parser::parse_module(text)
                .unwrap()
                .into_syntax()
                .body,
        ));
        let original = prepared.standalone().unwrap();
        prepared.syntax = None;
        assert_eq!(prepared.standalone().unwrap(), original);
    }

    #[test]
    fn physical_assertion_retains_its_test_before_a_suspending_message() {
        use crate::frontend::{
            ExpressionSelection, OwnedExpression, PhysicalOperand, RegisteredScope,
            RegisteredSource, RegisteredSuite, SuitePreparation,
        };
        use crate::ir::{Builtin, Expr, ExprKind, Statement, SuspendValue, Type};

        fn held_input(suite: &mut RegisteredSuite, external_test: bool) -> &mut OwnedExpression {
            let (checked, held) = match (external_test, suite.operands.as_mut_slice()) {
                (
                    true,
                    [
                        PhysicalOperand::Residual(held),
                        PhysicalOperand::Checked(checked),
                    ],
                )
                | (
                    false,
                    [
                        PhysicalOperand::Checked(checked),
                        PhysicalOperand::Residual(held),
                    ],
                ) => (checked, held),
                _ => panic!("expected the test followed by the message"),
            };
            assert_eq!(checked.local_names[&0].name, "value");
            held
        }

        fn suspend_modes(ir: &str) -> Vec<&str> {
            ir.lines()
                .filter(|line| {
                    line.trim_start()
                        .starts_with("call void @imre_dyn_suspend_operands(")
                })
                .map(|line| line.rsplit_once(", i32 ").unwrap().1)
                .collect()
        }

        for (text, external_test) in [
            (
                "def verify(check, message):\n    assert (value := check()), (yield (lambda: message())())\n",
                false,
            ),
            (
                "def verify(check, message):\n    assert (yield (lambda: check())()), (value := message())\n",
                true,
            ),
        ] {
            let mut module = PreparedBody::module(
                "proof",
                Path::new("proof.py"),
                text,
                0,
                ModuleOrigin::Program(None),
            )
            .unwrap();
            let parsed = ruff_python_parser::parse_module(text).unwrap();
            module.syntax = Some(Cow::Owned(parsed.into_syntax().body));
            let def = module.syntax.as_ref().unwrap()[0]
                .as_function_def_stmt()
                .unwrap();
            let function = def.start();
            let assertion = def.body[0].as_assert_stmt().unwrap();
            let start = assertion.start();
            let test_range = assertion.test.range();
            let input = if external_test {
                &*assertion.test
            } else {
                assertion.msg.as_deref().unwrap()
            };
            let replacement = ast::Expr::NoneLiteral(ast::ExprNoneLiteral {
                range: input.range(),
                node_index: Default::default(),
            });
            // Withhold the lambda from suite lowering alone. Its real compiled
            // child is still available when the owned physical operand executes.
            let lexical = module.source.statement_sites.expressions.take_lexical();
            let missing = HashMap::new();
            let source = RegisteredSource {
                lambdas: Some(&missing),
                ..module.source.registered_source()
            };
            let kind = RegisteredScope::Function { is_async: false };
            let mut physical =
                SuitePreparation::new(&module.source.namespace_operations, source, true);
            physical.sites.expressions = lexical;
            let body = physical.physical_function_suite(&def.body, kind, true);
            module.source.statement_sites = physical.sites;
            install_test_function_body(&mut module.source, def, body);
            let suite = test_function_suite(&mut module.source, function);
            assert!(suite.refusal.is_none());
            let Some(Statement::If(test, success, failure)) = suite.checked.function.body.get(1)
            else {
                panic!("expected a conditional assertion")
            };
            assert!(matches!(
                test.kind,
                ExprKind::Builtin(crate::ir::Builtin::RegisteredInput(0), _)
            ));
            assert_eq!(test.ty, Type::Bool);
            assert!(success.is_empty());
            let [Statement::Expr(raise)] = failure.as_slice() else {
                panic!("expected only the checked assertion failure")
            };
            let ExprKind::Builtin(
                Builtin::RegisteredAssertionFailure {
                    statement,
                    test: position,
                },
                message,
            ) = &raise.kind
            else {
                panic!("expected the checked assertion failure")
            };
            assert_eq!((*statement, *position), (start, test_range));
            assert!(matches!(
                message.as_slice(),
                [Expr {
                    ty: Type::PythonObject,
                    kind: ExprKind::Builtin(crate::ir::Builtin::RegisteredInput(1), _),
                    ..
                }]
            ));
            let input = held_input(suite, external_test);
            assert_eq!(input.range, replacement.range());
            let inputs = input.selected.physical().unwrap();
            let ExprKind::Builtin(Builtin::RegisteredLocation(range, ..), located) =
                &inputs.value.kind
            else {
                panic!("expected the physical yield location");
            };
            assert_eq!(*range, input.range);
            assert!(matches!(
                &located[0].kind,
                ExprKind::Builtin(Builtin::RegisteredSuspend(SuspendValue::Yield, range), _)
                    if *range == input.range
            ));
            assert_eq!(inputs.operands().count(), 1);
            let original = module.standalone().unwrap();
            assert_eq!(suspend_modes(&original.1), ["0)"]);
            let def = module.syntax.as_mut().unwrap().to_mut()[0]
                .as_function_def_stmt_mut()
                .unwrap();
            let assertion = def.body[0].as_assert_stmt_mut().unwrap();
            let original_input = if external_test {
                &mut *assertion.test
            } else {
                assertion.msg.as_deref_mut().unwrap()
            };
            *original_input = replacement.clone();
            assert_eq!(module.standalone().unwrap(), original);

            let def = module.syntax.as_mut().unwrap().to_mut()[0]
                .as_function_def_stmt_mut()
                .unwrap();
            def.body[0] = ast::StmtPass {
                range: def.body[0].range(),
                node_index: Default::default(),
            }
            .into();
            assert_eq!(module.standalone().unwrap(), original);

            let suite = test_function_suite(&mut module.source, function);
            held_input(suite, external_test).selected =
                std::rc::Rc::new(ExpressionSelection::Checked(crate::ir::Expr::none()));
            assert!(suspend_modes(&module.standalone().unwrap().1).is_empty());
        }
    }

    #[test]
    fn mixed_while_owns_its_guard_body_and_else_after_source_erasure() {
        use crate::frontend::{PhysicalOperand, RegisteredSuite};
        use crate::ir::Statement;

        fn region<'a>(module: &'a mut PreparedBody) -> &'a mut RegisteredSuite {
            let suite = &mut module.initializer;
            assert!(suite.refusal.is_none());
            let [region] = suite.suites.as_mut_slice() else {
                panic!("expected the prepared while");
            };
            region
        }

        for (guard, checked) in [("ready", true), ("(lambda: ready)()", false)] {
            let source =
                format!("while {guard}:\n    consume((lambda v: v)(item))\nelse:\n    finish()\n");
            let mut module = PreparedBody::module(
                "example",
                Path::new("example.py"),
                &source,
                0,
                ModuleOrigin::Program(None),
            )
            .unwrap();
            // Leave the body's callable and, when present, the guard's callable at placement.
            inject_residual_child_module(&mut module);
            let original = module.standalone().unwrap();
            module.syntax.as_mut().unwrap().to_mut().clear();
            assert!(module.standalone().unwrap() == original);

            fn else_block(region: &mut RegisteredSuite) -> &mut Vec<Statement> {
                let [Statement::While(_, _, otherwise)] =
                    region.checked.function.body.as_mut_slice()
                else {
                    panic!("expected checked While with its ordered suites");
                };
                otherwise
            }
            let otherwise = std::mem::take(else_block(region(&mut module)));
            assert!(module.standalone().unwrap() != original);
            *else_block(region(&mut module)) = otherwise;
            let child = &mut region(&mut module).suites[0];
            assert!(child.refusal.is_none());
            let body = std::mem::take(&mut child.checked.function.body);
            assert!(!body.is_empty());
            assert!(module.standalone().unwrap() != original);
            region(&mut module).suites[0].checked.function.body = body;
            match &mut region(&mut module).operands[0] {
                PhysicalOperand::Checked(operand) if checked => {
                    operand.value = crate::ir::Expr::integer(0);
                }
                PhysicalOperand::Residual(test) if !checked => {
                    test.selected = std::rc::Rc::new(
                        crate::frontend::ExpressionSelection::Checked(crate::ir::Expr::integer(0)),
                    );
                }
                _ => panic!("unexpected guard for {guard}"),
            }
            assert!(module.standalone().unwrap() != original);
        }
    }

    #[test]
    fn rejected_assignment_target_is_read_at_its_original_statement() {
        let source = concat!(
            "async def get(x):\n    return x\n\n",
            "async def update(values):\n",
            "    values[[lambda v=await get(x): v for x in [0]][0]()] = 'stored'\n",
            "    return values[0]\n\n",
            "try:\n    update({}).send(None)\n",
            "except StopIteration as exc:\n    print(exc.value)\n",
        );
        let mut module = PreparedBody::module_with_syntax(
            "target",
            Path::new("target.py"),
            source,
            0,
            ModuleOrigin::Program(None),
        )
        .unwrap();
        let expected =
            "target.py:5:12: cannot compile this comprehension ahead of time (through 5:51)";
        assert_eq!(module.standalone().unwrap_err(), expected);

        let function = module.syntax.as_mut().unwrap().to_mut()[1]
            .as_function_def_stmt_mut()
            .unwrap();
        let site = function.start();
        function.body.clear();
        assert_eq!(module.standalone().unwrap_err(), expected);

        let suite = test_function_suite(&mut module.source, site);
        let [crate::frontend::PhysicalOperand::Residual(input)] = suite.operands.as_mut_slice()
        else {
            panic!("expected the rejected assignment target");
        };
        assert_eq!(
            &source[input.range],
            "[lambda v=await get(x): v for x in [0]]"
        );
        input.selected = std::rc::Rc::new(crate::frontend::ExpressionSelection::Checked(
            crate::ir::Expr::none(),
        ));
        assert!(module.standalone().is_ok());
    }

    #[test]
    fn mixed_while_preserves_its_residual_guard_diagnostic_after_erasure() {
        use crate::frontend::PhysicalOperand;

        let source = "async def probe():\n    while [lambda v=await get(x): v for x in [0]]:\n        yield 'body'\n        break\n";
        let mut module = PreparedBody::module_with_syntax(
            "guard",
            Path::new("guard.py"),
            source,
            0,
            ModuleOrigin::Program(None),
        )
        .unwrap();
        let expected =
            "guard.py:2:11: cannot compile this comprehension ahead of time (through 2:50)";
        assert_eq!(module.standalone().unwrap_err(), expected);
        let def = module.syntax.as_mut().unwrap().to_mut()[0]
            .as_function_def_stmt_mut()
            .unwrap();
        let start = def.start();
        let guard_range = def.body[0].as_while_stmt().unwrap().test.range();
        def.body.clear();
        assert_eq!(module.standalone().unwrap_err(), expected);

        let plan = test_function_suite(&mut module.source, start);
        let [region] = plan.suites.as_mut_slice() else {
            panic!("expected the prepared while");
        };
        assert!(
            matches!(region.operands.as_slice(), [PhysicalOperand::Residual(test)] if test.range == guard_range)
        );
        let [crate::ir::Statement::While(condition, _, _)] =
            region.checked.function.body.as_mut_slice()
        else {
            panic!("expected the checked loop");
        };
        *condition = crate::ir::Expr::boolean(false);
        assert!(module.standalone().is_ok());
    }

    #[test]
    fn yield_in_a_false_while_still_declares_a_generator() {
        let module = PreparedBody::module(
            "idle",
            Path::new("idle.py"),
            "def idle():\n    while False:\n        yield 'unreached yield'\n    else:\n        return 'finished'\n",
            0,
            ModuleOrigin::Program(None),
        )
        .unwrap();
        let (_, llvm) = module.standalone().unwrap();
        let suspended = format!(", i32 {} }}", permanent::NATIVE_SUSPENDED_BODY);
        assert!(
            llvm.lines()
                .any(|line| line.contains(" = private constant %code_site")
                    && line.ends_with(&suspended))
        );
        assert!(llvm.contains("finished"));
        assert!(!llvm.contains("unreached yield"));
    }

    #[test]
    fn suspended_while_keeps_its_condition_and_suites_after_erasure() {
        let source = "def pump():\n    while (yield 'test'):\n        yield 'body'\n    else:\n        yield 'else'\n";
        let mut prepared = PreparedBody::module_with_syntax(
            "example",
            Path::new("example.py"),
            source,
            0,
            ModuleOrigin::Program(None),
        )
        .unwrap();
        let before = prepared.standalone().unwrap();
        let def = prepared.syntax.as_mut().unwrap().to_mut()[0]
            .as_function_def_stmt_mut()
            .unwrap();
        assert!(prepared.source.definition_body(def.start()).fully_checked());
        let loop_ = def.body[0].as_while_stmt_mut().unwrap();
        *loop_.test = ast::Expr::NoneLiteral(ast::ExprNoneLiteral {
            range: loop_.test.range(),
            node_index: Default::default(),
        });
        loop_.body.clear();
        loop_.orelse.clear();
        assert_eq!(prepared.standalone().unwrap(), before);
    }

    #[test]
    fn suspended_definitions_use_native_entries_in_each_module_origin() {
        let source = "def items(xs):\n    yield 1\n    yield from xs\npark = lambda: (yield 2)\nresume = lambda: 1 + (yield 3)\nasync def more(xs):\n    async for x in xs:\n        yield x\nasync def consume(f):\n    return await f()\ndef defaults():\n    yield 0\n    @(yield 'decorator')\n    def nested(first=1, second=(yield 4)):\n        return second\n    return nested\n";
        for bundled in [false, true] {
            let mut prepared = PreparedBody::module_with_syntax(
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
            let original = prepared.standalone().unwrap();
            let syntax = prepared.syntax.as_mut().unwrap().to_mut();
            for name in ["items", "more", "consume", "defaults"] {
                let def = syntax
                    .iter_mut()
                    .filter_map(ast::Stmt::as_function_def_stmt_mut)
                    .find(|def| def.name.as_str() == name)
                    .unwrap();
                let suite = prepared.source.definition_body(def.start());
                assert!(suite.fully_checked(), "{name}");
                crate::verify::registered(suite.checked().region()).unwrap();
                if name == "defaults" {
                    let site = def.body[1].start();
                    assert!(prepared.source.definition_body(site).fully_checked());
                    assert_eq!(prepared.source.scopes.functions[&site].name, "nested");
                }
                def.body.clear();
            }
            let (_, ir) = prepared.standalone().unwrap();
            assert_eq!(ir, original.1);
            let tag = format!(", i32 {} }}", permanent::NATIVE_SUSPENDED_BODY);
            assert_eq!(ir.lines().filter(|line| line.ends_with(&tag)).count(), 6);
            let operations: Vec<_> = ir
                .lines()
                .filter(|line| line.contains("call void @imre_dyn_suspend_operands("))
                .map(|line| line.rsplit_once(", i32 ").unwrap().1)
                .collect();
            assert_eq!(
                operations,
                ["0)", "3)", "0)", "0)", "2)", "1)", "2)", "0)", "0)", "0)"]
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
        .unwrap();
        assert_eq!(ir.matches("call i64 @imre_dyn_keyword_call_at(").count(), 1);
        assert_eq!(ir.matches("call i64 @imre_dyn_scoped_call_at(").count(), 1);
    }

    #[test]
    fn whole_ir_and_split_link_share_the_same_module_selection() {
        use crate::CodegenLayout;
        let root = Path::new("<imre stdlib>");
        let modules: Vec<ModuleSource> = vec![
            (
                "driver".into(),
                PathBuf::from("driver.py"),
                "from __future__ import annotations\nimport helper\n".into(),
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
            .unwrap();
        let providers = HashSet::from([symbol.clone()]);
        let registrations = RegisteredBodies {
            physical_files: HashMap::from([(
                modules[0].1.clone(),
                PhysicalFile {
                    digest: Some([b'F'; 32]),
                    ..Default::default()
                },
            )]),
            archive_members: HashMap::from([(modules[1].1.clone(), [b'Z'; 32])]),
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
            emit_modules(
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
        let (whole, whole_packed, _) = build(CodegenLayout::Whole);
        let (split, split_packed, _) = build(CodegenLayout::Split);
        let (native, packed, dump) = build(CodegenLayout::SplitWithWhole);
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
            let mut declared: Vec<_> = ir
                .lines()
                .filter_map(|line| line.strip_prefix("declare void @imre_dyn_register_"))
                .map(|line| line.split_once('(').unwrap().0)
                .collect();
            let mut used: Vec<_> = calls.iter().map(|(name, _)| *name).collect();
            declared.sort_unstable();
            used.sort_unstable();
            used.dedup();
            assert_eq!(declared, used);
            let constant = |encoded: &str| {
                let suffix = format!(r#"c"{encoded}\00""#);
                ir.lines()
                    .filter_map(|line| line.split_once(" = "))
                    .find(|(_, data)| data.ends_with(&suffix))
                    .unwrap()
                    .0
            };
            for (i, payload, flags) in [
                (0, "F".repeat(32), imre_ast::code_flags::FUTURE_ANNOTATIONS),
                (1, "Z".repeat(32), 0),
            ] {
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
        assert_eq!(dump.as_deref(), Some(whole[0].llvm_ir.as_str()));
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
            let (units, used, _) = emit_modules(
                (modules, &[], None),
                "{}",
                0,
                root,
                &symbols,
                crate::CodegenLayout::Whole,
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
                !emit_modules(
                    (std::slice::from_ref(missing), &[], None),
                    "{}",
                    0,
                    root,
                    &symbols,
                    crate::CodegenLayout::Whole,
                    initializers
                )
                .unwrap()
                .1
            );
        }
        assert_eq!(refs(&[project.clone(), bundled.clone()]), [false, true]);

        let (units, used, _) = emit_modules(
            (&[bundled.clone(), project.clone()], &[], None),
            "{}",
            0,
            root,
            &symbols,
            crate::CodegenLayout::Whole,
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
        let (units, used, _) = emit_modules(
            (&[bundled, project.clone(), shadow, project], &[], None),
            "{}",
            0,
            root,
            &symbols,
            crate::CodegenLayout::Split,
            (&HashMap::new(), &registrations),
        )
        .unwrap();
        assert!(used);
        assert_eq!(units.len(), 3);
        assert_eq!(units[1].name, units[2].name);
        assert_ne!(units[1].llvm_ir, units[2].llvm_ir);
    }

    #[test]
    fn a_residual_statement_uses_its_prepared_call_and_inline_operand() {
        use crate::frontend::{ExpressionInput, ExpressionSelection, SourceOperand::Residual};

        let mut prepared = PreparedBody::module(
            "store",
            Path::new("store.py"),
            "record((lambda v: v)(hi()), target(), {'rows': [f(x) for x in xs]})\n",
            0,
            ModuleOrigin::Program(None),
        )
        .unwrap();
        inject_residual_child_module(&mut prepared);
        let original = prepared.standalone().unwrap();
        let call = prepared.syntax.as_ref().unwrap()[0]
            .as_expr_stmt()
            .unwrap()
            .value
            .as_call_expr()
            .unwrap();
        let inputs = residual_selection(&mut prepared.initializer)
            .physical()
            .unwrap();
        let (callee, arguments) = named_call(&inputs.value);
        assert_eq!(callee, "record");
        let [first, target, dictionary] = arguments else {
            panic!("expected three arguments to record");
        };
        assert_eq!(named_call(target).0, "target");
        assert_input(first, 0);
        assert_input(dictionary, 1);
        let parts = inputs.operands().collect::<Vec<_>>();
        let [
            Residual(ExpressionInput {
                range: first,
                selected: ExpressionSelection::Operand(_),
                ..
            }),
            Residual(ExpressionInput {
                range: dictionary,
                selected: ExpressionSelection::Operand(operand),
                ..
            }),
        ] = parts.as_slice()
        else {
            panic!("expected the nested call and dictionary to use their own operand plans");
        };
        assert_eq!(*first, call.arguments.args[0].range());
        assert_eq!(*dictionary, call.arguments.args[2].range());
        assert!(!operand.region().private_slots().is_empty());
        prepared.syntax = None;
        assert_eq!(prepared.standalone().unwrap(), original);
        replace_residual_input(&mut prepared, 1);
        let changed = prepared.standalone().unwrap();
        assert_ne!(changed, original);
        replace_residual_input(&mut prepared, 0);
        assert_ne!(prepared.standalone().unwrap(), changed);
    }

    #[test]
    fn physical_comprehension_keeps_its_prepared_diagnostic() {
        let source = "async def make():\n    return [lambda v=await get(x): v for x in [1, 2]]\n";
        let mut prepared = PreparedBody::module_with_syntax(
            "example",
            Path::new("example.py"),
            source,
            0,
            ModuleOrigin::Program(None),
        )
        .unwrap();
        let def = prepared.syntax.as_mut().unwrap().to_mut()[0]
            .as_function_def_stmt_mut()
            .unwrap();
        let start = def.start();
        let value = def.body[0]
            .as_return_stmt_mut()
            .unwrap()
            .value
            .as_deref_mut()
            .unwrap();
        assert!(matches!(
            residual_occurrence(test_function_suite(&mut prepared.source, start)).selected.as_ref(),
            crate::frontend::ExpressionSelection::Refused(range, _) if *range == value.range()
        ));
        let display = value.as_list_comp_expr_mut().unwrap();
        *display.elt = ast::Expr::NoneLiteral(ast::ExprNoneLiteral {
            range: display.elt.range(),
            node_index: Default::default(),
        });
        assert_eq!(
            prepared.standalone().unwrap_err(),
            "example.py:2:12: cannot compile this comprehension ahead of time (through 2:54)",
        );
    }

    #[test]
    fn dictionary_reports_its_rejected_comprehension() {
        let source = concat!(
            "async def make():\n",
            "    return {\"items\": [lambda v=await get(x): v for x in [1, 2]]}\n",
        );
        let mut prepared = PreparedBody::module_with_syntax(
            "example",
            Path::new("example.py"),
            source,
            0,
            ModuleOrigin::Program(None),
        )
        .unwrap();
        let def = prepared.syntax.as_mut().unwrap().to_mut()[0]
            .as_function_def_stmt_mut()
            .unwrap();
        let start = def.start();
        let value = def.body[0]
            .as_return_stmt_mut()
            .unwrap()
            .value
            .as_deref_mut()
            .unwrap();
        let crate::frontend::ExpressionSelection::Refused(range, _) =
            residual_occurrence(test_function_suite(&mut prepared.source, start))
                .selected
                .as_ref()
        else {
            panic!("expected the checked child diagnostic");
        };
        assert_eq!(
            &source[*range],
            "[lambda v=await get(x): v for x in [1, 2]]"
        );
        value.as_dict_expr_mut().unwrap().items.clear();
        assert_eq!(
            prepared.standalone().unwrap_err(),
            "example.py:2:22: cannot compile this comprehension ahead of time (through 2:64)",
        );
    }

    #[test]
    fn rejected_lambda_default_keeps_its_span_after_parameter_erasure() {
        use crate::frontend::ExpressionSelection;

        let source = concat!(
            "async def make(rows):\n",
            "    return lambda a=take(await first(), [x for x in rows]), ",
            "b={'items': [lambda v=await get(x): v for x in rows]}: a\n",
        );
        let mut prepared = PreparedBody::module_with_syntax(
            "example",
            Path::new("example.py"),
            source,
            0,
            ModuleOrigin::Program(None),
        )
        .unwrap();
        let function = prepared.syntax.as_mut().unwrap().to_mut()[0]
            .as_function_def_stmt_mut()
            .unwrap();
        let start = function.start();
        let expr = function.body[0]
            .as_return_stmt_mut()
            .unwrap()
            .value
            .as_deref_mut()
            .unwrap();
        let binding = prepared.source.lambdas[&expr.start()].binding.clone();
        let held = residual_occurrence(test_function_suite(&mut prepared.source, start));
        assert_eq!(held.range, expr.range());
        assert!(std::rc::Rc::ptr_eq(&held.selected, &binding));
        let ExpressionSelection::Refused(range, reason) = binding.as_ref() else {
            panic!("the prepared lambda owns the rejected display's range");
        };
        assert_eq!(&source[*range], "[lambda v=await get(x): v for x in rows]");
        assert_eq!(*reason, "cannot compile this comprehension ahead of time");
        expr.as_lambda_expr_mut().unwrap().parameters = None;
        assert_eq!(
            prepared.standalone().unwrap_err(),
            "example.py:2:73: cannot compile this comprehension ahead of time (through 2:113)",
        );
    }

    #[test]
    fn dictionary_method_default_prepares_its_template_in_class_scope() {
        let source = concat!(
            "class Outer[T]:\n",
            "    item = 7\n",
            "    @staticmethod\n",
            "    def run[U](value={'spelling': t'{item}'}): return value\n",
        );
        let mut prepared = PreparedBody::module(
            "example",
            Path::new("example.py"),
            source,
            0,
            ModuleOrigin::Program(None),
        )
        .unwrap();
        let before = prepared.standalone().unwrap();
        assert!(prepared.source.lambdas.is_empty());
        prepared.source.statement_sites.expressions.take_lexical();
        assert_eq!(prepared.standalone().unwrap(), before);
    }

    #[test]
    fn suspended_dictionary_emits_after_its_entries_are_erased() {
        let mut prepared = PreparedBody::module_with_syntax(
            "example",
            Path::new("example.py"),
            concat!(
                "def make():\n",
                "    result = {(yield 'key'): (kept := (yield 'value')), **(yield 'spread')}\n",
                "    return result, kept\n",
            ),
            0,
            ModuleOrigin::Program(None),
        )
        .unwrap();
        let before = prepared.standalone().unwrap();
        let def = prepared.syntax.as_mut().unwrap().to_mut()[0]
            .as_function_def_stmt_mut()
            .unwrap();
        assert!(prepared.source.definition_body(def.start()).fully_checked());
        let assignment = def.body[0].as_assign_stmt_mut().unwrap();
        assignment.value.as_dict_expr_mut().unwrap().items.clear();
        assert_eq!(prepared.standalone().unwrap(), before);
    }

    #[test]
    fn suspended_assignment_emits_after_its_targets_are_erased() {
        let mut prepared = PreparedBody::module_with_syntax(
            "example",
            Path::new("example.py"),
            concat!(
                "def assign(sink):\n",
                "    whole = [(yield 'owner').field, (part, sink[(yield 'key')])] = rhs()\n",
                "    yield whole\n",
            ),
            0,
            ModuleOrigin::Program(None),
        )
        .unwrap();
        let before = prepared.standalone().unwrap();
        let def = prepared.syntax.as_mut().unwrap().to_mut()[0]
            .as_function_def_stmt_mut()
            .unwrap();
        assert!(prepared.source.definition_body(def.start()).fully_checked());
        def.body[0].as_assign_stmt_mut().unwrap().targets.clear();
        assert_eq!(prepared.standalone().unwrap(), before);
    }

    #[test]
    fn grouped_assignment_keeps_its_starred_comprehension_key_after_target_erasure() {
        let source = concat!(
            "def run(ok, xs, table):\n",
            r#"    earlier = first, [receiver()[key((saved := (17 if ok else t"{[*x for x in xs]=}")))], *rest] = rhs()"#,
            "\n",
            r#"    a = table[(saved := 17 if ok else t"{(*x for x in xs)=}")] = 41"#,
            "\n    return earlier, first, saved, rest, a\n",
            "state: int = 0\n",
        );
        let mut prepared = PreparedBody::module(
            "example",
            Path::new("example.py"),
            source,
            0,
            ModuleOrigin::Program(None),
        )
        .unwrap();
        let before = prepared.standalone().unwrap();
        assert!(prepared.syntax.is_none());
        let mut syntax = ruff_python_parser::parse_module(source)
            .unwrap()
            .into_syntax()
            .body;
        let def = syntax[0].as_function_def_stmt_mut().unwrap();
        assert!(prepared.source.definition_body(def.start()).fully_checked());
        for assignment in &mut def.body[..2] {
            assignment.as_assign_stmt_mut().unwrap().targets.clear();
        }
        prepared.syntax = Some(Cow::Owned(syntax));
        assert_eq!(prepared.standalone().unwrap(), before);
    }

    #[test]
    fn annotated_class_retains_checked_targets_and_their_physical_inputs() {
        let source = concat!(
            "def outer[T]():\n",
            "    class Example:\n",
            "        member: int\n",
            "        receiver().field = [item.copy() for item in items]\n",
            "        saved = [receiver().field, (table()[key()], *tail)] = (lambda: items)()\n",
            "        select(lambda: rows).field: int = fallback()\n",
            "        del receiver().field\n",
            "        saved = (first, [receiver().field, table()[key()], *rest]) = rows()\n",
            "        def read(self): return self.member\n",
            "    return Example\n",
        );
        let mut prepared = PreparedBody::module(
            "example",
            Path::new("example.py"),
            source,
            0,
            ModuleOrigin::Program(None),
        )
        .unwrap();
        assert!(prepared.syntax.is_none());
        assert!(
            prepared
                .source
                .definition_body(TextSize::new(0))
                .fully_checked()
        );
        let class = prepared.source.class_bodies.values().next().unwrap();
        assert!(class.body.fully_checked());
        prepared.standalone().unwrap();

        inject_residual_child_module(&mut prepared);
        let outer = prepared.syntax.as_ref().unwrap()[0]
            .as_function_def_stmt()
            .unwrap();
        let class = outer.body[0].as_class_def_stmt().unwrap();
        let class_start = class.start();
        let suite = &prepared.source.class_bodies[&class_start].body;
        crate::verify::registered(suite.checked.region()).unwrap();
        assert!(!suite.checked.region().private_slots().is_empty());
        assert!(suite.refusal.is_none());
        let statements: Vec<_> = input_statements(suite)
            .map(|(_, statement)| statement)
            .collect();
        let [
            crate::ir::Statement::Assign(..),
            crate::ir::Statement::Expr(attribute),
        ] = statements.as_slice()
        else {
            panic!("expected a residual RHS followed by a mixed attribute store");
        };
        assert!(matches!(&attribute.kind, crate::ir::ExprKind::Builtin(
            crate::ir::Builtin::RegisteredAttribute { action: crate::ir::AttributeAction::Store, .. },
            args
        ) if args.len() == 2));
        assert_eq!(suite.operands.len(), 3);

        let emit = |prepared: &PreparedBody<'_>| {
            let mut output = Output::default();
            let file = output.data(b"example.py");
            prepared.emit(&mut output, 0, &file).unwrap();
            (output.functions, output.constants)
        };
        let before = emit(&prepared);
        let outer = prepared.syntax.as_mut().unwrap().to_mut()[0]
            .as_function_def_stmt_mut()
            .unwrap();
        let class = outer.body[0].as_class_def_stmt_mut().unwrap();
        let store = class.body[1].as_assign_stmt_mut().unwrap();
        let target = store.targets[0].as_attribute_expr_mut().unwrap();
        std::mem::swap(&mut target.value, &mut store.value);
        store.targets.clear();
        // Neither the checked targets nor their residual input need the assignment syntax.
        class.body[2] = class.body[4].clone();
        class.body[3] = class.body[4].clone();
        class.body[5].as_assign_stmt_mut().unwrap().targets.clear();
        assert_eq!(emit(&prepared), before);

        let class = prepared.source.class_bodies.get_mut(&class_start).unwrap();
        let crate::frontend::PhysicalOperand::Residual(value) = &mut class.body.operands[0] else {
            panic!("expected the residual lambda call");
        };
        value.selected = std::rc::Rc::new(crate::frontend::ExpressionSelection::Checked(
            crate::ir::Expr::none(),
        ));
        assert_ne!(emit(&prepared), before);
    }

    #[test]
    fn finally_match_keeps_its_prepared_delete_targets() {
        let source = concat!(
            "try:\n    work()\n",
            "finally:\n    match subject():\n",
            "        case target:\n            del first, target[(lambda: key)()], target[last]\n",
        );
        let mut prepared = PreparedBody::module(
            "example",
            Path::new("example.py"),
            source,
            0,
            ModuleOrigin::Program(None),
        )
        .unwrap();
        inject_residual_child_module(&mut prepared);
        let before = prepared.standalone().unwrap();
        let protected = prepared.syntax.as_mut().unwrap().to_mut()[0]
            .as_try_stmt_mut()
            .unwrap();
        let branch = protected.finalbody[0].as_match_stmt_mut().unwrap();
        let suite = &prepared.initializer;
        let [region] = suite.suites.as_slice() else {
            panic!("expected a registered try with prepared children")
        };
        let [_, crate::ir::Statement::Try { finalbody, .. }] = &region.checked.function.body[..]
        else {
            panic!("expected a checked try")
        };
        let finalbody = region.child_suite(finalbody).unwrap();
        let [region] = finalbody.suites.as_slice() else {
            panic!("expected a mixed match in finally")
        };
        assert!(matches!(
            region.checked.function.body.as_slice(),
            [_, crate::ir::Statement::Match(..)]
        ));
        let suite = &region.suites[0];
        assert!(suite.refusal.is_none());
        let statements: Vec<_> = input_statements(suite).collect();
        let [(at, crate::ir::Statement::Expr(value))] = statements.as_slice() else {
            panic!("expected one external deletion operand");
        };
        assert!(matches!(
            value.kind,
            crate::ir::ExprKind::Builtin(crate::ir::Builtin::DeleteItem, _)
        ));
        assert!(*at > 0 && *at + 1 < suite.checked.function.body.len());
        branch.cases[0].body[0]
            .as_delete_stmt_mut()
            .unwrap()
            .targets
            .clear();
        assert_eq!(prepared.standalone().unwrap(), before);
    }

    #[test]
    fn physical_deletion_uses_the_complete_checked_statement_after_targets_are_dropped() {
        let source = concat!(
            "del first, [(owner := obj).field, obj[lo:hi]], obj[[key for key in keys if key]], last\n",
            "del rejected, (orphan := obj)[(lambda: key)()]\n",
        );
        let mut prepared = PreparedBody::module(
            "remove",
            Path::new("remove.py"),
            source,
            0,
            ModuleOrigin::Program(None),
        )
        .unwrap();
        assert!(prepared.initializer().fully_checked());
        inject_residual_child_module(&mut prepared);
        let targets = &prepared.syntax.as_ref().unwrap()[1]
            .as_delete_stmt()
            .unwrap()
            .targets;
        let key_range = targets[1].as_subscript_expr().unwrap().slice.range();
        let initializer = prepared.initializer();
        let suite = initializer;
        crate::verify::registered(suite.checked.region()).unwrap();
        assert!(suite.refusal.is_none());
        let statements: Vec<_> = input_statements(suite).collect();
        let [(at, crate::ir::Statement::Expr(value))] = statements.as_slice() else {
            panic!("only the last deletion needs external inputs");
        };
        assert_eq!(*at + 1, suite.checked.function.body.len());
        assert!(
            matches!(&value.kind, crate::ir::ExprKind::Builtin(crate::ir::Builtin::DeleteItem, slots)
            if slots.len() == 2)
        );
        let [
            crate::frontend::PhysicalOperand::Checked(_),
            crate::frontend::PhysicalOperand::Residual(key),
        ] = suite.operands.as_slice()
        else {
            panic!("expected a checked base and an opaque key");
        };
        assert_eq!(key.range, key_range);
        assert_eq!(&source[key_range], "(lambda: key)()");

        let emit = |prepared: &PreparedBody<'_>| {
            let mut output = Output::default();
            let file = output.data(b"remove.py");
            prepared.emit(&mut output, 0, &file).unwrap();
            for op in ["delname", "delattr", "delitem"] {
                let symbol = output.strings.get_index_of(op.as_bytes()).unwrap();
                assert!(
                    output.functions.contains(&format!("ptr @str{symbol},")),
                    "{op}"
                );
            }
            (output.functions, output.constants)
        };
        let before = emit(&prepared);
        for stmt in prepared.syntax.as_mut().unwrap().to_mut() {
            stmt.as_delete_stmt_mut().unwrap().targets.clear();
        }
        assert_eq!(emit(&prepared), before);
    }

    #[test]
    fn checked_raise_uses_the_prepared_tuple_after_its_elements_are_erased() {
        let source = concat!(
            "def factory(seed):\n",
            "    async def evaluate():\n",
            "        raise KeyError((await gate(), (lambda v: combine(seed, v))(right())))\n",
            "    return evaluate\n",
            "state: int\n",
        );
        let mut prepared = PreparedBody::module_with_syntax(
            "worker",
            Path::new("worker.py"),
            source,
            0,
            ModuleOrigin::Program(None),
        )
        .unwrap();
        let original = prepared.standalone().unwrap();
        let factory = prepared.syntax.as_mut().unwrap().to_mut()[0]
            .as_function_def_stmt_mut()
            .unwrap();
        let evaluate = factory.body[0].as_function_def_stmt_mut().unwrap();
        let suite = prepared.source.definition_body(evaluate.start());
        assert!(suite.fully_checked());
        assert!(matches!(
            suite.checked().function.body.last(),
            Some(crate::ir::Statement::Raise(Some(_)))
        ));
        let raise = evaluate.body[0].as_raise_stmt_mut().unwrap();
        let call = raise
            .exc
            .as_deref_mut()
            .unwrap()
            .as_call_expr_mut()
            .unwrap();
        let tuple = &mut call.arguments.args[0];
        tuple.as_tuple_expr_mut().unwrap().elts.clear();
        assert_eq!(prepared.standalone().unwrap(), original);
    }

    #[test]
    fn a_residual_assignment_uses_the_prepared_tuple_after_its_elements_are_erased() {
        use crate::frontend::{ExpressionInput, ExpressionSelection, SourceOperand::Residual};

        let mut prepared = PreparedBody::module(
            "worker",
            Path::new("worker.py"),
            "result = consume((seed(), [f(x) for x in xs]), (lambda: right())())\n",
            0,
            ModuleOrigin::Program(None),
        )
        .unwrap();
        inject_residual_child_module(&mut prepared);
        let original = prepared.standalone().unwrap();

        let call = prepared.syntax.as_ref().unwrap()[0]
            .as_assign_stmt()
            .unwrap()
            .value
            .as_call_expr()
            .unwrap();
        let inputs = residual_selection(&mut prepared.initializer)
            .physical()
            .unwrap();
        let (callee, arguments) = named_call(&inputs.value);
        assert_eq!(callee, "consume");
        let [tuple, lambda] = arguments else {
            panic!("expected the tuple and lambda arguments");
        };
        assert_input(tuple, 0);
        assert_input(lambda, 1);
        let parts = inputs.operands().collect::<Vec<_>>();
        let [
            Residual(ExpressionInput {
                range,
                selected: ExpressionSelection::Operand(_),
                ..
            }),
            Residual(_),
        ] = parts.as_slice()
        else {
            panic!("expected the prepared tuple argument");
        };
        assert_eq!(*range, call.arguments.args[0].range());
        prepared.syntax = None;
        assert_eq!(prepared.standalone().unwrap(), original);
        replace_residual_input(&mut prepared, 1);
        assert_ne!(prepared.standalone().unwrap(), original);
    }
}
