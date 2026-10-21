//! Hosts ty's real semantic analysis and Ruff's cached parser over one source snapshot.

use std::sync::Arc;

use ruff_db::diagnostic::{Diagnostic, DisplayDiagnosticConfig, Severity};
use ruff_db::files::{File, Files, system_path_to_file};
use ruff_db::system::{InMemorySystem, System};
use ruff_db::vendored::VendoredFileSystem;
use ruff_python_ast::ModModule;
use ruff_python_ast::token::{TokenAt, TokenKind, Tokens};
use ruff_text_size::{Ranged, TextSize};
use ty_python_core::program::ProgramSettings;
use ty_python_core::{Program, ProgramFile};
use ty_python_semantic::dependency::DependencyMetadata;
use ty_python_semantic::lint::{LintRegistry, RuleSelection};
use ty_python_semantic::{AnalysisSettings, Db, check_file_unwrap, default_lint_registry};
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

/// Checks with ty, then exposes the same Ruff AST to Relar's stricter lowering.
///
/// ty diagnostics are useful general Python checks, not a code-generation proof:
/// Relar still checks closed bindings, exact representations, and every mutation.
pub(crate) fn with_module<T>(
    source: &str,
    lower: impl FnOnce(&ModModule) -> Result<T, String>,
) -> Result<T, String> {
    let vendored = ty_vendored::file_system().clone();
    let mut settings = ProgramSettings::empty(&vendored);
    settings.python_version.version = ruff_python_ast::PythonVersion::PY312;
    let db = Database {
        storage: salsa::Storage::default(),
        files: Files::default(),
        system: InMemorySystem::default(),
        vendored,
        settings,
        rules: Arc::new(RuleSelection::from_registry(default_lint_registry())),
        analysis: Arc::new(AnalysisSettings {
            respect_type_ignore_comments: false,
            ..AnalysisSettings::default()
        }),
    };
    db.settings.search_paths.try_register_static_roots(&db);
    db.system
        .fs()
        .write_file("/input.py", source)
        .map_err(|error| error.to_string())?;
    let file = system_path_to_file(&db, "/input.py").map_err(|error| error.to_string())?;
    let diagnostics = db.check_file(file);
    let errors = diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.severity() >= Severity::Error)
        .map(|diagnostic| {
            diagnostic
                .display(&db, &DisplayDiagnosticConfig::new("relar"))
                .to_string()
        })
        .collect::<Vec<_>>();
    if !errors.is_empty() {
        return Err(errors.join("\n"));
    }
    let parsed =
        ruff_db::parsed::parsed_module(&db, db.program_file(file).python_file(&db)).load(&db);
    if let Some(error) = parsed.errors().first() {
        return Err(error.to_string());
    }
    check_indentation(source, parsed.tokens())?;
    lower(parsed.syntax())
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
                let in_string = matches!(tokens.at_offset(position), TokenAt::Single(token)
                    if token.kind() == TokenKind::String && token.range().contains(position));
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
