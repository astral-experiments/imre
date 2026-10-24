//! Hosts ty's real semantic analysis and Ruff's cached parser over one source snapshot.

use std::path::Path;
use std::sync::Arc;

use ruff_db::diagnostic::{Diagnostic, DisplayDiagnosticConfig, Severity};
use ruff_db::files::{File, Files, system_path_to_file};
use ruff_db::parsed::ParsedModuleRef;
use ruff_db::system::{InMemorySystem, System, SystemPathBuf};
use ruff_db::vendored::VendoredFileSystem;
use ruff_python_ast::token::{TokenKind, Tokens};
use ruff_python_parser::{Mode, lexer::lex};
use ruff_text_size::{Ranged, TextSize};
use ty_python_core::program::ProgramSettings;
use ty_python_core::{Program, ProgramFile};
use ty_python_semantic::dependency::DependencyMetadata;
use ty_python_semantic::lint::{LintRegistry, RuleSelection};
use ty_python_semantic::{
    AnalysisSettings, Db, HasType, SemanticModel, check_file_unwrap, default_lint_registry,
};
use ty_site_packages::PythonVersionWithSource;

#[salsa::db]
#[derive(Clone)]
struct Database {
    storage: salsa::Storage<Self>,
    files: Files,
    system: InMemorySystem,
    vendored: VendoredFileSystem,
    settings: ProgramSettings,
    rules: Arc<RuleSelection>,
    analysis: Arc<AnalysisSettings>,
}

/// One immutable project snapshot shared by ty's checks and Ruff's cached parser.
pub(crate) struct Session {
    db: Database,
    files: Vec<File>,
}

impl Session {
    pub(crate) fn new(root: &Path, extension_roots: &[std::path::PathBuf]) -> Result<Self, String> {
        let root = root.to_str().ok_or("module paths must be valid UTF-8")?;
        let system = InMemorySystem::default();
        system
            .fs()
            .create_directory_all(root)
            .map_err(|error| error.to_string())?;
        // The loader reserves this namespace for sealed compiler intrinsics.
        // Give ty its exact public signature without loading the dynamic CPython
        // compatibility implementation or weakening unresolved-import checks.
        let intrinsic = SystemPathBuf::from(root).join("relar");
        system
            .fs()
            .write_file_all(
                intrinsic.join("__init__.pyi"),
                "from . import json as json, toml as toml\n",
            )
            .map_err(|error| error.to_string())?;
        system
            .fs()
            .write_file_all(
                intrinsic.join("toml.pyi"),
                "from typing import TypeVar\n_T = TypeVar(\"_T\")\ndef decode(text: str, schema: type[_T], /) -> _T: ...\n",
            )
            .map_err(|error| error.to_string())?;
        system
            .fs()
            .write_file_all(
                intrinsic.join("json.pyi"),
                "from typing import TypeVar\n_T = TypeVar(\"_T\")\ndef decode(text: str, schema: type[_T], /) -> _T: ...\ndef encode(value: object, /, *, indent: int | None = None, ensure_ascii: bool = True) -> str: ...\n",
            )
            .map_err(|error| error.to_string())?;
        let vendored = ty_vendored::file_system().clone();
        let mut settings = ProgramSettings::empty(&vendored);
        settings.python_version.version = crate::PYTHON_VERSION;
        let mut roots = vec![SystemPathBuf::from(root)];
        for path in extension_roots {
            let path = path.to_str().ok_or("extension paths must be valid UTF-8")?;
            system
                .fs()
                .create_directory_all(path)
                .map_err(|error| error.to_string())?;
            roots.push(SystemPathBuf::from(path));
        }
        settings.search_paths = ty_module_resolver::SearchPathSettings::new(roots)
            .to_search_paths(&system, &vendored, &ty_module_resolver::FallibleStrategy)
            .map_err(|error| error.to_string())?;
        let db = Database {
            storage: salsa::Storage::default(),
            files: Files::default(),
            system,
            vendored,
            settings,
            rules: Arc::new(RuleSelection::from_registry(default_lint_registry())),
            analysis: Arc::new(AnalysisSettings {
                respect_type_ignore_comments: false,
                ..AnalysisSettings::default()
            }),
        };
        db.settings.search_paths.try_register_static_roots(&db);
        Ok(Self {
            db,
            files: Vec::new(),
        })
    }

    /// Parses a normalized source once; semantic checks wait until every import is present.
    pub(crate) fn add(&mut self, path: &Path, source: &str) -> Result<ParsedModuleRef, String> {
        let path = path.to_str().ok_or("module paths must be valid UTF-8")?;
        check_source_complexity(source).map_err(|error| format!("{path}: {error}"))?;
        self.db
            .system
            .fs()
            .write_file_all(path, source)
            .map_err(|error| error.to_string())?;
        let file = system_path_to_file(&self.db, path).map_err(|error| error.to_string())?;
        let parsed = ruff_db::parsed::parsed_module(
            &self.db,
            self.db.program_file(file).python_file(&self.db),
        )
        .load(&self.db);
        if let Some(error) = parsed.errors().first() {
            return Err(format!("{path}: {error}"));
        }
        check_indentation(source, parsed.tokens()).map_err(|error| format!("{path}: {error}"))?;
        self.files.push(file);
        Ok(parsed)
    }

    /// Get a scalar layout hint for an append operand in its own lexical scope.
    pub(crate) fn scalar_type(
        &self,
        module: usize,
        value: &ruff_python_ast::Expr,
    ) -> Option<crate::ir::Type> {
        use crate::ir::Type;
        use ty_python_semantic::types::{KnownClass, ProgramEnvironment, Type as Ty};

        let file = self.db.program_file(self.files[module]);
        let ty = value.inferred_type(&SemanticModel::new(&self.db, file))?;
        if !matches!(
            ty,
            Ty::LiteralValue(_) | Ty::NominalInstance(_) | Ty::Union(_)
        ) {
            return None;
        }
        let env = ProgramEnvironment::from_file(file);
        // ty expands a float annotation to float | int. Match that annotation
        // after the narrower bool and int cases; Relar checks the exact operand.
        [
            (KnownClass::Bool, Type::Bool),
            (KnownClass::Int, Type::Int),
            (KnownClass::Float, Type::Float),
            (KnownClass::Str, Type::Str),
            (KnownClass::Bytes, Type::Bytes),
        ]
        .into_iter()
        .find(|(class, _)| {
            let mut instance = class.to_instance(&self.db, &env);
            if matches!(class, KnownClass::Float) {
                instance = ty_python_semantic::types::UnionType::from_two_elements(
                    &self.db,
                    &env,
                    instance,
                    KnownClass::Int.to_instance(&self.db, &env),
                );
            }
            ty.is_assignable_to(&self.db, &env, instance)
        })
        .map(|(_, ty)| ty)
    }

    pub(crate) fn is_never(&self, module: usize, value: &ruff_python_ast::Expr) -> bool {
        let model = SemanticModel::new(&self.db, self.db.program_file(self.files[module]));
        matches!(
            value.inferred_type(&model),
            Some(ty_python_semantic::types::Type::Never)
        )
    }

    /// Checks every loaded file, including unused functions and inactive main guards.
    pub(crate) fn check(&self) -> Result<(), String> {
        let mut errors = Vec::new();
        for &file in &self.files {
            errors.extend(
                self.db
                    .check_file(file)
                    .iter()
                    .filter(|diagnostic| {
                        // The closed checker proves operand, assignment, argument,
                        // and return compatibility for every statement, including
                        // dead and unused code. ty can conservatively retain None
                        // after a successful float pattern; its flow types must
                        // not override those exact checks.
                        // Invalid index values and zero slice steps are runtime
                        // errors. Exact index types and fixed tuple bounds are
                        // still enforced by the closed checker.
                        // Attribute reads also use closed layouts. In particular,
                        // sealed exception payload getters guard their category
                        // at runtime rather than depending on ty's narrowing.
                        diagnostic.severity() >= Severity::Error
                            && !matches!(
                                diagnostic.id().as_str(),
                                "unsupported-operator"
                                    | "invalid-assignment"
                                    | "invalid-argument-type"
                                    | "invalid-return-type"
                                    | "zero-stepsize-in-slice"
                                    | "index-out-of-bounds"
                                    | "unresolved-attribute"
                            )
                    })
                    .map(|diagnostic| {
                        diagnostic
                            .display(&self.db, &DisplayDiagnosticConfig::new("relar"))
                            .to_string()
                    }),
            );
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("\n"))
        }
    }
}

/// Ruff indexes the AST recursively as soon as the database parses a module.
/// Bound the depth before that happens, including chains with no parentheses.
fn check_source_complexity(source: &str) -> Result<(), &'static str> {
    const LIMIT: usize = 96;
    if source.len() < LIMIT {
        return Ok(());
    }

    #[derive(Default)]
    struct Group {
        operators: usize,
        child_depth: usize,
        longest_item: usize,
        // A comma in lambda parameters or a comprehension target does not
        // separate independent expressions at this delimiter level.
        keep_commas: bool,
    }

    impl Group {
        fn depth(&self) -> usize {
            self.longest_item.max(self.operators + self.child_depth)
        }
    }

    use TokenKind::*;
    let mut lexer = lex(source, Mode::Module);
    let mut parents = Vec::new();
    let mut group = Group::default();
    let mut indent = 0_usize;
    loop {
        match lexer.next_token() {
            EndOfFile => return Ok(()),
            Indent => indent += 1,
            Dedent => indent = indent.saturating_sub(1),
            Newline | Semi if parents.is_empty() => group = Group::default(),
            Lpar | Lsqb | Lbrace | FStringStart | TStringStart => {
                group.operators += 1;
                parents.push(std::mem::take(&mut group));
            }
            Rpar | Rsqb | Rbrace | FStringEnd | TStringEnd => {
                if let Some(mut parent) = parents.pop() {
                    parent.child_depth = parent.child_depth.max(group.depth());
                    group = parent;
                }
            }
            Comma if !group.keep_commas => {
                group.longest_item = group.depth();
                group.operators = 0;
                group.child_depth = 0;
            }
            Lambda | For => {
                group.keep_commas = true;
                group.operators += 1;
            }
            Name | Int | Float | Complex | String | True | False | None | Ellipsis
            | FStringMiddle | TStringMiddle | Comment | NonLogicalNewline => continue,
            _ => group.operators += 1,
        }
        if indent + parents.len() + group.depth() > LIMIT {
            return Err("Relar: source exceeds the syntax complexity limit");
        }
    }
}

/// Rejects ambiguous indentation while preserving whitespace inside string tokens.
fn check_indentation(source: &str, tokens: &Tokens) -> Result<(), String> {
    let mut offset = 0;
    for (line_number, line) in source.split_inclusive('\n').enumerate() {
        for (index, character) in line.char_indices() {
            if !matches!(character, ' ' | '\t' | '\x0c') {
                break;
            }
            if character != ' ' {
                let position = TextSize::try_from(offset + index)
                    .map_err(|_| "source exceeds the supported size")?;
                let in_string = tokens.at_offset(position).any(|token| {
                    matches!(token.kind(), TokenKind::String | TokenKind::FStringMiddle)
                        && token.range().contains(position)
                });
                if !in_string {
                    return Err(format!(
                        "line {}: indentation must use spaces, not tabs or form feeds",
                        line_number + 1
                    ));
                }
            }
        }
        offset += line.len();
    }
    Ok(())
}

#[salsa::db]
impl salsa::Database for Database {}

#[salsa::db]
impl ruff_db::Db for Database {
    fn vendored(&self) -> &VendoredFileSystem {
        &self.vendored
    }

    fn system(&self) -> &dyn System {
        &self.system
    }

    fn files(&self) -> &Files {
        &self.files
    }
}

#[salsa::db]
impl ty_module_resolver::Db for Database {}

#[salsa::db]
impl ty_python_core::Db for Database {
    fn should_check_file(&self, file: File) -> bool {
        !file.path(self).is_vendored_path()
    }
}

#[salsa::db]
impl Db for Database {
    fn check_file(&self, file: File) -> Vec<Diagnostic> {
        check_file_unwrap(self, self.program_file(file))
    }

    fn program_file(&self, file: File) -> ProgramFile<'_> {
        Program::from_settings(self, &self.settings).program_file(self, file)
    }

    fn python_version_with_source(&self, _file: File) -> &PythonVersionWithSource {
        &self.settings.python_version
    }

    fn rule_selection(&self, _file: File) -> &RuleSelection {
        &self.rules
    }

    fn lint_registry(&self) -> &LintRegistry {
        default_lint_registry()
    }

    fn analysis_settings(&self, _file: File) -> &AnalysisSettings {
        &self.analysis
    }

    fn dependency_metadata(&self, _file: File) -> Option<&DependencyMetadata> {
        None
    }

    fn verbose(&self) -> bool {
        false
    }

    fn is_open_file(&self, _file: File) -> bool {
        false
    }

    fn dyn_clone(&self) -> Box<dyn Db> {
        Box::new(self.clone())
    }
}
