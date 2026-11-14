//! Collects Python source modules from explicit startup search paths for native lowering.

mod emit;
pub(crate) use emit::Prompt;
mod import_module;

pub(crate) use import_module::bundled_module_source;
pub(crate) mod literal;
mod prepare;
pub(crate) use prepare::ModuleSource;
mod provider_archive;
pub(crate) mod provider_pack;

use std::borrow::Cow;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};

use ruff_python_ast::name::UnqualifiedName;
use ruff_python_ast::visitor::{self, Visitor};
use ruff_python_ast::{self as ast, Expr, Stmt};
use ruff_text_size::{Ranged, TextRange};

use crate::frontend::module_source::{self, CheckedFile, ParsedModule};
use crate::{CompileOptions, SourceInputs, SourceKind};

/// runpy opens the lexical abspath, which can cross a symlink and select a
/// different file from the loader's spelling. Only register an equivalent origin.
fn declared_open_path(
    path: &Path,
    identity: &Path,
    inputs: &mut SourceInputs,
    filesystem: &mut crate::watch::FileSystem<'_>,
) -> Result<Option<PathBuf>, String> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::{OsStrExt, OsStringExt};
        let bytes = path.as_os_str().as_bytes();
        let normalized = imre_ast::posixpath::normalize(bytes, None).map_err(|e| e.to_string())?;
        if normalized != bytes {
            let path = PathBuf::from(std::ffi::OsString::from_vec(normalized));
            let resolved = filesystem.canonicalize(&path).ok();
            let same = resolved.as_deref() == Some(identity);
            inputs
                .declared_file_resolutions
                .push((path.clone(), resolved));
            return Ok(same.then_some(path));
        }
    }
    #[cfg(not(unix))]
    let _ = (identity, inputs, filesystem);
    Ok(Some(path.to_owned()))
}

/// Only a winning file can add a first-party frozen override while walking SDK dependencies.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ImportOrigin {
    Startup,
    File,
    Provider,
    Alternative,
}

impl ImportOrigin {
    fn file(self, winner: bool) -> Self {
        match (winner, self) {
            (true, Self::Startup) => Self::Startup,
            (true, _) => Self::File,
            (false, _) => Self::Alternative,
        }
    }

    fn enqueue<T>(self, work: &mut VecDeque<(T, Self)>, item: T) {
        if self == Self::Startup {
            work.push_front((item, self));
        } else {
            work.push_back((item, self));
        }
    }
}

/// A module rejected during discovery can still be found by a lazy import.
/// Keep its Python error at the import point without compiling the text again.
pub(crate) struct ModuleSyntaxError {
    pub(crate) range: ruff_text_size::TextRange,
    pub(crate) message: String,
    position: Option<[usize; 4]>,
    pub(crate) validate_at_origin: bool,
    in_args: bool,
    pub(crate) parse_error: bool,
}

impl ModuleSyntaxError {
    pub(crate) fn new(
        source: &str,
        range: ruff_text_size::TextRange,
        message: String,
        in_args: bool,
    ) -> Self {
        let location = |offset: ruff_text_size::TextSize| {
            let prefix = &source[..offset.to_usize()];
            let line = prefix.bytes().filter(|&b| b == b'\n').count() + 1;
            let column = prefix.rsplit('\n').next().unwrap().len() + 1;
            (line, column)
        };
        let (line, column) = location(range.start());
        let (end_line, end_column) = location(range.end());
        Self {
            range,
            message,
            position: Some([line, end_line, column, end_column]),
            validate_at_origin: false,
            in_args,
            parse_error: false,
        }
    }

    /// Binding and future errors have a shared precedence for files and literal modules.
    pub(crate) fn in_body(source: &str, body: &[ast::Stmt]) -> Option<Self> {
        let bindings = imre_ast::executable::BindingErrors::check(body, source);
        let future = imre_ast::future_import_error(body);
        let validate_at_origin =
            future.is_none() && bindings.symbol.is_none() && bindings.pattern.is_none();
        let mut error = bindings
            .first_symbol(future)
            .map(|(range, reason)| Self::new(source, range, reason.to_owned(), false))
            .or_else(|| {
                let issue = bindings.handler?;
                let mut error = Self::new(
                    source,
                    issue.location.unwrap_or(issue.range),
                    issue.message.to_owned(),
                    true,
                );
                if issue.location.is_none() {
                    error.position = None;
                }
                error.range = issue.range;
                Some(error)
            })
            .or_else(|| {
                bindings.pattern.map(|(range, reason)| {
                    Self::new(source, range, reason.checked().into_owned(), true)
                })
            });
        if let Some(diagnostic) = &mut error {
            diagnostic.validate_at_origin = validate_at_origin;
        }
        error
    }

    /// Parser diagnostics use character columns and include the offending line in SyntaxError.args.
    fn from_parser(source: &str, error: ruff_python_parser::ParseError) -> Self {
        let range = error.location;
        let mut diagnostic = Self::new(source, range, error.to_string(), true);
        for (position, offset) in [(2, range.start()), (3, range.end())] {
            let before = &source[..offset.to_usize()];
            diagnostic.position.as_mut().unwrap()[position] =
                before.rsplit('\n').next().unwrap().chars().count() + 1;
        }
        diagnostic.parse_error = true;
        diagnostic
    }
}

/// Finds literal directories made available to imports by a startup script.
/// The generated program still evaluates the assignment and consults its actual
/// `sys.path` in order when each import executes.
#[derive(Default)]
struct Imports {
    names: Vec<(u32, String)>,
    module_flags: u32,
    explicit_roots: Vec<PathBuf>,
    runtime_imports: bool,
    private_names: imre_ast::Names,
    invalid_executable: Option<Box<ModuleSyntaxError>>,
    uses_import_builtin: bool,
    uses_exec: bool,
    literal_calls: Vec<TextRange>,
    has_open_exec: bool,
    computed_import: bool,
    imports_logging: bool,
    sys_names: HashSet<String>,
    operator_names: HashSet<String>,
    opcode_names: HashSet<String>,
    hashlib_names: HashSet<String>,
    signal_names: HashMap<String, String>,
    unittest_names: HashSet<String>,
    boxed_mock_names: HashSet<String>,
    pkgutil_names: HashSet<String>,
    get_data_names: HashSet<String>,
    machinery_names: HashSet<String>,
    machinery_attributes: HashSet<(String, String)>,
}

fn literal_path(expr: &Expr) -> Option<PathBuf> {
    match expr {
        Expr::StringLiteral(s) => Some(PathBuf::from(s.value.to_string())),
        Expr::Call(c) if c.arguments.keywords.is_empty() => {
            let Expr::Attribute(attr) = &*c.func else {
                return None;
            };
            if attr.attr.as_str() != "fsdecode" {
                return None;
            }
            let [Expr::BytesLiteral(b)] = c.arguments.args.as_ref() else {
                return None;
            };
            let bytes: Vec<_> = b.value.bytes().collect();
            #[cfg(unix)]
            {
                use std::os::unix::ffi::OsStringExt;
                Some(std::ffi::OsString::from_vec(bytes).into())
            }
            #[cfg(not(unix))]
            {
                Some(String::from_utf8(bytes).ok()?.into())
            }
        }
        _ => None,
    }
}

/// A root's child imports follow its physical source's original registration queue.
#[derive(Default)]
struct LiteralImports {
    forest: literal::LiteralForest,
    pending_roots: Vec<(usize, u32, Vec<TextRange>)>,
    origins: HashMap<usize, (bool, bool, ImportOrigin)>,
    seeded_winners: usize,
    open_exec: HashSet<usize>,
}

impl LiteralImports {
    fn register(
        &mut self,
        unit: usize,
        imports: &mut Imports,
        package: bool,
        selected: bool,
        origin: ImportOrigin,
    ) {
        if imports.has_open_exec {
            self.open_exec.insert(unit);
        }
        if !imports.literal_calls.is_empty() {
            self.pending_roots.push((
                unit,
                imports.module_flags,
                std::mem::take(&mut imports.literal_calls),
            ));
            self.origins.insert(unit, (package, selected, origin));
        }
    }
}

impl Imports {
    fn uses_machinery_export(&self, export: &str) -> bool {
        self.machinery_attributes
            .iter()
            .any(|(receiver, attr)| attr == export && self.machinery_names.contains(receiver))
            || self.names.iter().any(|(level, name)| {
                *level == 0 && name.strip_prefix("importlib.machinery.") == Some(export)
            })
    }

    fn sys_path(&self, e: &Expr) -> bool {
        matches!(e,Expr::Attribute(a) if a.attr.as_str()=="path" && matches!(&*a.value,Expr::Name(n) if self.sys_names.contains(n.id.as_str())))
    }
    fn paths(&mut self, e: &Expr) {
        match e {
            Expr::BinOp(x) if x.op == ast::Operator::Add => {
                self.paths(&x.left);
                self.paths(&x.right);
            }
            Expr::List(x) => self
                .explicit_roots
                .extend(x.elts.iter().filter_map(literal_path)),
            Expr::Tuple(x) => self
                .explicit_roots
                .extend(x.elts.iter().filter_map(literal_path)),
            _ => {}
        }
    }
    fn add(&mut self, level: u32, name: &str) {
        self.names.push((level, name.to_owned()));
        if level == 0
            && matches!(
                name,
                "_imp"
                    | "_hashlib"
                    | "ssl"
                    | "_ssl"
                    | "atexit"
                    | "faulthandler"
                    | "code"
                    | "codeop"
                    | "copyreg"
                    | "json"
                    | "json.encoder"
                    | "opcode"
                    | "_opcode_metadata"
                    | "sysconfig"
                    | "pathlib._local"
                    | "pathlib.types"
                    | "platform"
                    | "pyexpat"
                    | "pyexpat.errors"
                    | "pyexpat.model"
                    | "xml.parsers"
                    | "xml.parsers.expat"
                    | "xml.parsers.expat.errors"
                    | "xml.parsers.expat.model"
                    | "keyword"
                    | "site"
                    | "importlib.abc"
                    | "importlib.machinery"
                    | "tomllib"
                    | "tomllib._parser"
                    | "tomllib._re"
                    | "tomllib._types"
                    | "xml"
                    | "xml.etree"
                    | "xml.etree.ElementPath"
                    | "xml.etree.ElementTree"
                    | "zoneinfo"
                    | "zoneinfo._common"
                    | "zoneinfo._tzpath"
                    | "zoneinfo._zoneinfo"
                    | "traceback"
                    | "unittest.case"
                    | "unittest.result"
                    | "unittest.suite"
                    | "string.templatelib"
                    | "runpy"
            )
        {
            self.runtime_imports = true;
        }
    }
}

// Keep the two typed specialization constants on their existing route.
fn boxed_opcode(name: &str) -> bool {
    use crate::modules::{Binding, Intrinsic, IntrinsicModule};
    matches!(
        IntrinsicModule::Opcode.symbol(name),
        Some(Binding::Intrinsic(Intrinsic::ObjectRuntimeExport(_)))
    )
}

fn boxed_sys_export(name: &str) -> bool {
    use crate::modules::{Binding, Intrinsic, IntrinsicModule};
    matches!(
        IntrinsicModule::Sys.symbol(name),
        Some(Binding::Intrinsic(Intrinsic::SYS_IMPORT_REGISTRY))
    ) || matches!(
        name,
        "path"
            | "modules"
            | "_xoptions"
            | "version_info"
            | "implementation"
            | "exit"
            | "exc_info"
            | "excepthook"
            | "__excepthook__"
            | "exception"
    )
}

fn boxed_signal_export(module: &str, name: &str) -> bool {
    use crate::modules::{Binding, Intrinsic, IntrinsicModule};
    let provider = match module {
        "signal" => IntrinsicModule::Signal,
        "_signal" => IntrinsicModule::SignalNative,
        _ => return false,
    };
    matches!(
        provider.symbol(name),
        Some(Binding::Intrinsic(Intrinsic::ObjectRuntimeExport(_)))
    )
}

fn boxed_hashlib_kdf(name: &str) -> bool {
    matches!(name, "pbkdf2_hmac" | "scrypt")
}

fn boxed_unittest_export(name: &str) -> bool {
    matches!(
        name,
        "BaseTestSuite"
            | "case"
            | "result"
            | "suite"
            | "addModuleCleanup"
            | "doModuleCleanups"
            | "enterModuleContext"
    )
}

fn boxed_operator(name: &str) -> bool {
    macro_rules! operator_functions {
        ($($function:literal => $operation:expr,)* checked { $($plain:literal => $checked:expr),* $(,)? }) => { &[$($function),*] };
    }
    const FUNCTIONS: &[&str] = include!("../../../imre-runtime/native/operator-functions.rs");
    matches!(name, "attrgetter" | "itemgetter")
        || FUNCTIONS.contains(&name)
        || name
            .strip_prefix("__")
            .and_then(|name| name.strip_suffix("__"))
            .is_some_and(|alias| {
                FUNCTIONS
                    .iter()
                    .any(|&name| name != "length_hint" && name.trim_end_matches('_') == alias)
            })
}

impl<'a> Visitor<'a> for Imports {
    fn visit_stmt(&mut self, s: &'a Stmt) {
        match s {
            Stmt::Import(imp) => {
                for alias in &imp.names {
                    let name = alias.name.as_str();
                    let bound = alias.asname.as_ref().unwrap_or(&alias.name);
                    self.imports_logging |= name == "logging";
                    if matches!(name, "__future__" | "_operator") {
                        self.runtime_imports = true;
                    }
                    match name {
                        "sys" => self.sys_names.insert(bound.to_string()),
                        "operator" => self.operator_names.insert(bound.to_string()),
                        "_opcode" => self.opcode_names.insert(bound.to_string()),
                        "hashlib" => self.hashlib_names.insert(bound.to_string()),
                        "signal" | "_signal" if cfg!(unix) => self
                            .signal_names
                            .insert(bound.to_string(), name.to_owned())
                            .is_none(),
                        "unittest" => self.unittest_names.insert(bound.to_string()),
                        "pkgutil" => self.pkgutil_names.insert(bound.to_string()),
                        "importlib" => self.machinery_names.insert(format!("{bound}.machinery")),
                        "importlib.machinery" => self.machinery_names.insert(bound.to_string()),
                        _ => false,
                    };
                    self.add(0, name);
                }
            }
            Stmt::ImportFrom(imp) => {
                let name = imp.module.as_ref().map_or("", |m| m.as_str());
                self.imports_logging |= imp.level == 0 && name == "logging";
                self.add(imp.level, name);
                for a in &imp.names {
                    let suffix = a.name.as_str();
                    if imp.level == 0
                        && (name == "_operator"
                            || name == "unittest" && boxed_unittest_export(suffix)
                            || name == "hashlib" && (boxed_hashlib_kdf(suffix) || suffix == "*")
                            || name == "weakref" && suffix == "finalize"
                            || boxed_signal_export(name, suffix))
                    {
                        self.runtime_imports = true;
                    }
                    if imp.level == 0
                        && name == "unittest.mock"
                        && crate::modules::boxed_mock_export(suffix)
                    {
                        self.boxed_mock_names
                            .insert(a.asname.as_ref().unwrap_or(&a.name).to_string());
                    }
                    if imp.level == 0 && name == "pkgutil" && suffix == "get_data" {
                        self.get_data_names
                            .insert(a.asname.as_ref().unwrap_or(&a.name).to_string());
                    }
                    if imp.level == 0 && name == "importlib" && suffix == "machinery" {
                        self.runtime_imports = true;
                        self.machinery_names
                            .insert(a.asname.as_ref().unwrap_or(&a.name).to_string());
                    }
                    if imp.level == 0
                        && ((name == "operator" && boxed_operator(suffix))
                            || (name == "_opcode" && (boxed_opcode(suffix) || suffix == "*"))
                            || (name == "sys" && boxed_sys_export(suffix))
                            || (name == "string" && suffix == "templatelib")
                            || (name == "pathlib" && matches!(suffix, "_local" | "types")))
                    {
                        self.runtime_imports = true;
                    }
                    if suffix != "*" {
                        self.names.push((
                            imp.level,
                            if name.is_empty() {
                                suffix.to_owned()
                            } else {
                                format!("{name}.{suffix}")
                            },
                        ));
                    }
                }
            }
            Stmt::FunctionDef(function)
                if self.private_names.string_annotations
                    && function_type_params(function, true, true, Some(&self.private_names))
                        .is_some() =>
            {
                self.runtime_imports = true;
            }
            Stmt::ClassDef(class)
                if class.type_params.is_some()
                    || crate::classes::defines_pickle_reducer(class)
                    || class.arguments.as_ref().is_some_and(|arguments| {
                        arguments.keywords.iter().any(|keyword| {
                            keyword
                                .arg
                                .as_deref()
                                .is_some_and(|name| name != "metaclass")
                        })
                    }) =>
            {
                if class.type_params.is_some() {
                    // The implicit Generic base calls typing._generic_init_subclass.
                    self.add(0, "typing");
                }
                self.runtime_imports = true;
            }
            Stmt::Assign(a) if a.targets.iter().any(|t| self.sys_path(t)) => {
                self.runtime_imports = true;
                self.paths(&a.value);
            }
            _ => {}
        }
        visitor::walk_stmt(self, s);
    }
    fn visit_expr(&mut self, e: &'a Expr) {
        if let Expr::Call(call) = e
            && call
                .func
                .as_name_expr()
                .is_some_and(|name| name.id == "exec")
        {
            let source = crate::frontend::exec_source_operand(call);
            if source.is_some() {
                self.literal_calls.push(call.range());
            }
            // Until binding resolution, a name can still hold arbitrary source.
            self.has_open_exec |= !source.is_some_and(Expr::is_string_literal_expr);
        }
        // A literal __import__ call cannot select a different provider at execution.
        if let Expr::Call(call) = e
            && matches!(&*call.func, Expr::Name(name) if name.id == "__import__")
            && let Some(Expr::StringLiteral(name)) = call.arguments.find_argument_value("name", 0)
        {
            self.uses_import_builtin = true;
            self.add(0, "importlib");
            if matches!(
                name.value.to_str().split('.').next(),
                Some("decimal" | "_pydecimal" | "pyexpat" | "time" | "xml" | "zoneinfo")
            ) {
                self.add(0, name.value.to_str());
            }
            self.visit_arguments(&call.arguments);
            return;
        }
        if let Expr::Name(name) = e
            && name.ctx == ast::ExprContext::Load
        {
            if name.id == "__import__" {
                self.uses_import_builtin = true;
                self.computed_import = true;
                self.add(0, "importlib");
            }
            if name.id == "exec" {
                self.uses_exec = true;
            }
            if self.boxed_mock_names.contains(name.id.as_str()) {
                self.runtime_imports = true;
            }
        }
        // A resource lookup can be the first operation to load its package.
        if let Expr::Call(call) = e
            && match &*call.func {
                Expr::Name(name) => self.get_data_names.contains(name.id.as_str()),
                Expr::Attribute(attr) => {
                    attr.attr.as_str() == "get_data"
                        && matches!(&*attr.value, Expr::Name(name) if self.pkgutil_names.contains(name.id.as_str()))
                }
                _ => false,
            }
            && let Some(Expr::StringLiteral(package)) = call
                .arguments
                .args
                .first()
                .or_else(|| call.arguments.find_keyword("package").map(|kw| &kw.value))
        {
            let name = package.value.to_string();
            if name.split('.').all(|part| {
                let path = Path::new(part);
                path.file_name() == Some(path.as_os_str()) && !part.contains('\0')
            }) {
                self.add(0, &name);
            }
        }
        // Changes to sys.path can select a ZIP loader at runtime. Register the same
        // spec factory used for explicit import_module and find_spec calls.
        if cfg!(unix) && self.sys_path(e) {
            self.add(0, "importlib");
        }
        if let Expr::Attribute(attr) = e
            && matches!(
                attr.attr.as_str(),
                "ModuleSpec" | "BuiltinImporter" | "FrozenImporter"
            )
            && let Some(receiver) = UnqualifiedName::from_expr(&attr.value)
        {
            self.machinery_attributes
                .insert((receiver.to_string(), attr.attr.to_string()));
        }
        if let Expr::Attribute(attr) = e
            && let Expr::Name(module) = &*attr.value
            && ((boxed_operator(attr.attr.as_str())
                && self.operator_names.contains(module.id.as_str()))
                || (boxed_hashlib_kdf(attr.attr.as_str())
                    && self.hashlib_names.contains(module.id.as_str()))
                || (boxed_opcode(attr.attr.as_str())
                    && self.opcode_names.contains(module.id.as_str()))
                || self
                    .signal_names
                    .get(module.id.as_str())
                    .is_some_and(|name| boxed_signal_export(name, attr.attr.as_str()))
                || (boxed_unittest_export(attr.attr.as_str())
                    && self.unittest_names.contains(module.id.as_str())))
        {
            self.runtime_imports = true;
        }
        if matches!(e, Expr::Attribute(a) if matches!(a.attr.as_str(), "__reduce__" | "__reduce_ex__" | "__getstate__" | "__getinitargs__"))
            || matches!(e, Expr::Name(n) if n.id.as_str() == "SystemExit")
            || matches!(e, Expr::Attribute(a) if boxed_sys_export(a.attr.as_str()) && matches!(&*a.value, Expr::Name(n) if self.sys_names.contains(n.id.as_str())))
        {
            self.runtime_imports = true;
        }
        if let Expr::Call(c) = e
            && let Expr::Attribute(a) = &*c.func
            && self.sys_path(&a.value)
        {
            if a.attr.as_str() == "append"
                && let [item] = c.arguments.args.as_ref()
            {
                self.explicit_roots.extend(literal_path(item));
            } else if a.attr.as_str() == "insert"
                && let [_, item] = c.arguments.args.as_ref()
            {
                self.explicit_roots.extend(literal_path(item));
            }
        }
        visitor::walk_expr(self, e);
    }
}

/// Keep eligibility for object lowering independent of what an already-boxed body can emit.
pub(crate) fn function_type_params<'a>(
    function: &'a ast::StmtFunctionDef,
    string_annotations: bool,
    nested: bool,
    private_names: Option<&imre_ast::Names>,
) -> Option<&'a ast::TypeParams> {
    if nested && !string_annotations {
        return None;
    }
    crate::frontend::function_scope::boxed_function_type_params(
        function,
        string_annotations,
        nested,
        private_names,
    )
}

enum ImportSource<'s, 'p> {
    Module(ParsedModule<'s, 'p>),
    Invalid(Box<ModuleSyntaxError>),
}

fn read_import_source<'s, 'p>(
    source: &'s str,
    kind: SourceKind,
    existing: Option<&'p ruff_python_parser::Parsed<ast::ModModule>>,
) -> Result<ImportSource<'s, 'p>, String> {
    let normalized = crate::normalize_source(source, kind)?;
    let input = imre_ast::indentation::prepare(&normalized);
    crate::analysis::check_source_complexity(&input.text).map_err(str::to_owned)?;
    let parsed = match existing {
        Some(parsed) => Ok(Cow::Borrowed(parsed)),
        None => ruff_python_parser::parse_module(&input.text).map(Cow::Owned),
    };
    if let Some(issue) = input.first_issue(parsed.as_ref().err()) {
        return Err(issue.to_string());
    }
    let parsed = match parsed {
        Ok(parsed) => parsed,
        Err(error) if kind == SourceKind::DecodedFile => {
            return Ok(ImportSource::Invalid(Box::new(
                ModuleSyntaxError::from_parser(&normalized, error),
            )));
        }
        Err(error) => return Err(error.to_string()),
    };
    crate::analysis::check_form_feeds(&normalized, parsed.tokens())?;
    Ok(ImportSource::Module(ParsedModule {
        text: normalized,
        parsed,
    }))
}

fn scan_imports(
    source: &str,
    kind: SourceKind,
    getter: Option<&mut Imports>,
    file_alternative: Option<(&Path, bool)>,
    shipped: bool,
) -> Result<(Imports, Option<imre_ast::Names>), String> {
    let module = match read_import_source(source, kind, None)? {
        ImportSource::Module(module) => module,
        ImportSource::Invalid(error) => {
            return Ok((
                Imports {
                    invalid_executable: Some(error),
                    ..Imports::default()
                },
                None,
            ));
        }
    };
    if let Some((path, keep_docstrings)) = file_alternative {
        module_source::validate_file(path, &module.text, &module.parsed, keep_docstrings)?;
    }
    let mut imports = imports_from_body(
        &module.text,
        &mut module_source::into_body(module.parsed),
        getter,
        file_alternative.map(|(path, _)| path),
    )?;
    // The standalone parser reads the registered bytes in their original coordinates.
    let names = (shipped
        && std::ptr::eq(module.text.as_ref(), source)
        && imports.invalid_executable.is_none()
        && !imports.private_names.rewritten)
        .then(|| std::mem::take(&mut imports.private_names));
    Ok((imports, names))
}

pub(super) enum LoweredRoot {
    Complete(ast::Suite, imre_ast::Names),
    /// The registered parser input already has the names found at discovery.
    Unchanged(imre_ast::Names),
}

enum ScannedParser {
    Owned(ruff_python_parser::Parsed<ast::ModModule>),
    Declaration(ruff_db::parsed::ParsedModuleRef),
    Indexed(crate::analysis::IndexedFileInput),
}

/// The saved name lowering belongs to this parser's node IDs.
pub(super) struct ScannedModule {
    parsed: Option<ScannedParser>,
    lowered: LoweredRoot,
}

impl ScannedModule {
    fn from_input((module, lowered): emit::ParsedInput<'_, '_>) -> Option<Self> {
        let lowered = lowered?;
        Some(Self {
            parsed: match module.parsed {
                Cow::Owned(parser) => Some(ScannedParser::Owned(parser)),
                Cow::Borrowed(_) => None,
            },
            lowered,
        })
    }
}

fn retained_imports<'a, 'p>(
    module: ParsedModule<'a, 'p>,
    getter: Option<&mut Imports>,
    prepared_text: Option<&str>,
) -> Result<(Imports, Option<emit::ParsedInput<'a, 'p>>), String> {
    // File discovery normalizes newlines. A caller preparing the original bytes
    // needs its own parser so diagnostics keep the executable source offsets.
    if prepared_text.is_some_and(|text| {
        !std::ptr::eq(text, module.text.as_ref()) && text != module.text.as_ref()
    }) {
        return imports_from_body(
            &module.text,
            &mut module_source::into_body(module.parsed),
            getter,
            None,
        )
        .map(|imports| (imports, None));
    }
    let mut body = module.parsed.syntax().body.clone();
    let mut imports = imports_from_body(&module.text, &mut body, getter, None)?;
    let lowered = imports
        .invalid_executable
        .is_none()
        .then(|| LoweredRoot::Complete(body, std::mem::take(&mut imports.private_names)));
    Ok((imports, Some((module, lowered))))
}

fn scan_imports_for_preparation<'s, 'p>(
    source: &'s str,
    kind: SourceKind,
    existing: Option<&'p ruff_python_parser::Parsed<ast::ModModule>>,
    getter: Option<&mut Imports>,
    prepared_text: Option<&str>,
) -> Result<(Imports, Option<emit::ParsedInput<'s, 'p>>), String> {
    match read_import_source(source, kind, existing)? {
        ImportSource::Module(module) => retained_imports(module, getter, prepared_text),
        ImportSource::Invalid(error) => Ok((
            Imports {
                invalid_executable: Some(error),
                ..Imports::default()
            },
            None,
        )),
    }
}

fn imports_from_body(
    normalized: &str,
    body: &mut [Stmt],
    getter: Option<&mut Imports>,
    file_alternative: Option<&Path>,
) -> Result<Imports, String> {
    let error = ModuleSyntaxError::in_body(normalized, body);
    let module_flags = imre_ast::future_flags(body);
    let names = imre_ast::lower_in(
        body,
        "",
        module_flags & imre_ast::code_flags::FUTURE_ANNOTATIONS != 0,
    );
    // An optional file must pass the emitter's name checks before registration.
    if let Some(path) = file_alternative {
        module_source::validate_names(path, normalized, body, &names)?;
    }
    if let Some(diagnostic) = error {
        return Ok(Imports {
            invalid_executable: Some(Box::new(diagnostic)),
            ..Imports::default()
        });
    }
    let mut imports = Imports {
        module_flags,
        private_names: names,
        ..Imports::default()
    };
    let mut getter_bodies = Vec::new();
    if getter.is_some() {
        for (index, stmt) in body.iter_mut().enumerate() {
            if let Stmt::FunctionDef(function) = stmt
                && function.name.as_str() == "__getattr__"
                && !function.body.is_empty()
            {
                getter_bodies.push((index, std::mem::take(&mut function.body)));
            }
        }
    }
    imports.visit_body(body);
    if let Some(getter) = getter
        && !getter_bodies.is_empty()
    {
        // sys.path and pkgutil.get_data can add source hints through module aliases.
        getter.sys_names = std::mem::take(&mut imports.sys_names);
        getter.pkgutil_names = std::mem::take(&mut imports.pkgutil_names);
        getter.get_data_names = std::mem::take(&mut imports.get_data_names);
        getter.machinery_names.clone_from(&imports.machinery_names);
        for (index, statements) in getter_bodies {
            getter.visit_body(&statements);
            let Stmt::FunctionDef(function) = &mut body[index] else {
                unreachable!("saved a module-level __getattr__ declaration");
            };
            function.body = statements;
        }
        imports.literal_calls.append(&mut getter.literal_calls);
        imports.has_open_exec |= getter.has_open_exec;
    }
    Ok(imports)
}

pub(crate) fn is_permanent_integer(value: i64) -> bool {
    crate::llvm::registered::permanent::small_integer_id(value).is_some()
}

pub(crate) type CheckedFunctions =
    HashMap<ruff_text_size::TextSize, crate::frontend::RegisteredFunction>;

pub(crate) const BUNDLED_SOURCE_ROOT: &str = "<imre stdlib>";

pub(crate) enum ObjectOperation {
    CheckedEntry,
    Builtin,
    OrderedCommandImport,
    ClassLayout,
}

#[derive(Clone, Copy)]
pub(crate) enum EntryStart<'a> {
    Immediate,
    After(&'a Path),
}

#[derive(Clone, Copy)]
pub(crate) struct RegisteredSource<'a> {
    pub(crate) has_open_exec: bool,
    pub(crate) unit: Option<usize>,
    /// The first application initializer and its selected startup predecessor, if any.
    pub(crate) initial: Option<EntryStart<'a>>,
    /// The ordinary launcher selected this source to execute in the startup namespace.
    pub(crate) main_body: bool,
    /// This exact source supplies the annotations in the linked application.
    pub(crate) declares_signatures: bool,
    pub(crate) name: &'a str,
    pub(crate) path: &'a Path,
    pub(crate) source: &'a str,
    pub(crate) kind: SourceKind,
    pub(crate) is_program: bool,
    pub(crate) check_execution: bool,
    pub(crate) checked_module: Option<crate::modules::ModuleId>,
}

impl<'a> RegisteredSource<'a> {
    pub(crate) fn new(
        (name, path, source): &'a ModuleSource<'_>,
        is_program: bool,
        checked_module: Option<crate::modules::ModuleId>,
    ) -> Self {
        Self {
            has_open_exec: false,
            unit: None,
            name,
            path,
            source,
            kind: if path == Path::new("<string>") {
                SourceKind::Command
            } else {
                SourceKind::File
            },
            is_program,
            check_execution: is_program,
            checked_module,
            initial: None,
            main_body: false,
            declares_signatures: is_program,
        }
    }
}

pub(crate) type ProviderSources<'a> = HashMap<crate::modules::ModuleId, (&'a Path, &'a str)>;

pub(crate) struct RegisteredSources<'a> {
    pub(crate) layouts: &'a mut Vec<crate::analysis::RegisteredLayouts>,
    pub(crate) root: &'a Path,
    pub(crate) roots: &'a [PathBuf],
    pub(crate) winners: &'a [RegisteredSource<'a>],
    pub(crate) file_alternatives: &'a [RegisteredSource<'a>],
    /// Selected provider bodies that replace a fixed-layout declaration in the checked graph.
    pub(crate) provider_sources: ProviderSources<'a>,
    pub(crate) has_file_body: Option<&'a dyn Fn(&str) -> bool>,
}

pub(crate) fn compile(
    (operand, packaged_file_module, mut filesystem, wheel_records): (
        crate::ProgramEntry<'_>,
        Option<&str>,
        crate::watch::FileSystem<'_>,
        &mut crate::installed_wheels::WheelRecords,
    ),
    options: &CompileOptions,
    object_imports: bool,
    operation: Option<ObjectOperation>,
    project: &mut Option<crate::modules::SourceGraph>,
    output: Option<&mut crate::ProgramOutput>,
    check: impl FnOnce(
        Option<&mut crate::modules::SourceGraph>,
        bool,
        Option<RegisteredSources<'_>>,
    ) -> Result<(), String>,
) -> Result<Option<SourceInputs>, String> {
    let (path, module_root, source_entry) = match operand {
        crate::ProgramEntry::Source(_) => {
            (Path::new(crate::modules::MEMORY_SOURCE_PATH), None, true)
        }
        crate::ProgramEntry::File(path, root) => (path, root, false),
    };
    let invocation = options.command.as_ref();
    let startup = options.python_startup();
    let frozen_modules = startup.is_none_or(|(flags, _)| flags.frozen_modules);
    let command = invocation.map(|i| &i.source);
    let module_entry = options.module_entry.as_deref();
    let in_memory = command.is_some() || module_entry.is_some();
    let checked_graph = project.as_ref();
    let typed_graph = checked_graph.filter(|graph| !graph.has_checked_only_import);
    let file_entry = checked_graph.filter(|_| !in_memory);
    let (src, entry_snapshot) = if module_entry.is_some() {
        (String::new(), None)
    } else if let Some(command) = command {
        (command.to_owned(), None)
    } else if let Some(graph) = file_entry {
        (
            graph.modules[graph.entry].original_source().to_owned(),
            (!source_entry).then(|| {
                let (_, identity, hash) = &graph.inputs.files[graph.entry];
                (
                    identity.clone(),
                    hash.clone(),
                    graph.modules[graph.entry].raw_digest,
                )
            }),
        )
    } else {
        // A direct .pyc operand already received its bytecode diagnostic from the script loader.
        if options.script_startup.is_some() && crate::named_bytecode(path) {
            return Ok(None);
        }
        let Ok(identity) = filesystem.canonicalize(path) else {
            return Ok(None);
        };
        let bytes = match filesystem.read_source(&identity) {
            Ok(source) => source,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {
                return Err(format!("{}: {error}", path.display()));
            }
            Err(_) => return Ok(None),
        };
        if options.script_startup.is_some() && crate::bytecode_script_error(path, &bytes).is_some()
        {
            return Ok(None);
        }
        let hash = blake3::hash(&bytes).to_hex().to_string();
        let Ok(source) = crate::DiskSource::decode(path, bytes) else {
            return Ok(None);
        };
        (source.text, Some((identity, hash, source.raw_digest)))
    };
    let entry_identity = entry_snapshot
        .as_ref()
        .map(|(identity, _, _)| identity.as_path());
    let startup_fallback = startup.is_some();
    let ordered_command = matches!(operation, Some(ObjectOperation::OrderedCommandImport));
    let kind = if command.is_some() {
        SourceKind::Command
    } else if entry_snapshot.is_some() {
        SourceKind::DiskFile
    } else {
        SourceKind::File
    };
    let modules = checked_graph.map_or(&[][..], |graph| graph.modules.as_slice());
    let entry_file =
        file_entry.and_then(|graph| CheckedFile::entry(graph, entry_identity.unwrap_or(path)));
    let checked_entry = matches!(operation, Some(ObjectOperation::CheckedEntry));
    let mut scanned_modules = HashMap::new();
    let mut entry_imports = |text: &str, mut body: ast::Suite| -> Result<Imports, String> {
        let mut imports = imports_from_body(text, &mut body, None, None)?;
        // Generic functions can admit an unresolved entry. A runtime-class import
        // also needs the entry's own request when checking its first-party callers.
        if module_entry.is_none()
            && (!checked_entry || typed_graph.is_some_and(|graph| graph.imports_runtime_class))
            && imports.invalid_executable.is_none()
            && !imports.runtime_imports
        {
            imports.runtime_imports = body.iter().any(|stmt| {
                matches!(stmt, Stmt::FunctionDef(function) if function_type_params(function, imports.private_names.string_annotations, false, Some(&imports.private_names)).is_some())
            });
        }
        if let Some((graph, file)) = file_entry.zip(entry_file)
            && imports.invalid_executable.is_none()
        {
            let parsed = entry_identity.map(|_| {
                ScannedParser::Indexed(
                    graph
                        .session
                        .operand_types(graph.entry)
                        .indexed_input(&file.module(modules).parsed),
                )
            });
            scanned_modules.insert(
                0,
                ScannedModule {
                    parsed,
                    lowered: LoweredRoot::Complete(body, std::mem::take(&mut imports.private_names)),
                },
            );
        }
        Ok(imports)
    };
    let mut file_source = None;
    let mut first = if let Some(file) = entry_file {
        let module = file.parsed(modules);
        entry_imports(&module.text, module_source::into_body(module.parsed))?
    } else {
        let module = match read_import_source(&src, kind, None) {
            Ok(ImportSource::Module(module)) => module,
            _ => return Ok(None),
        };
        if in_memory {
            entry_imports(&module.text, module_source::into_body(module.parsed))?
        } else {
            let imports = entry_imports(&module.text, module.parsed.syntax().body.clone())?;
            file_source = Some(module);
            imports
        }
    };
    if startup.is_none()
        && let Some(error) = first.invalid_executable.take()
    {
        // Preserve diagnostics owned by the original first-party graph before
        // reporting why this entry cannot supply an executable Python body.
        check(project.as_mut(), false, None)?;
        let source = crate::normalize_source(&src, kind)?;
        let diagnostic = crate::frontend::phase::Diagnostic::at(
            &source,
            &error.range,
            format!("SyntaxError: {}", error.message),
        );
        return Err(diagnostic.with_path(path).to_string());
    }
    // Python entries can import these bundled sources when static lowering cannot handle the entry.
    let imports_provider = |provider: &str| {
        startup_fallback
            && first
                .names
                .iter()
                .any(|(level, name)| *level == 0 && name == provider)
    };
    let request_script = imports_provider("urllib.request");
    let ftp_script = imports_provider("ftplib");
    let argparse_script = imports_provider("argparse");
    let provider_script = request_script || ftp_script || argparse_script;
    let entry_requests_objects = module_entry.is_some()
        || first.runtime_imports
        || (first.uses_import_builtin && startup.is_some())
        || provider_script;
    let object_entry = match operation {
        Some(
            ObjectOperation::CheckedEntry | ObjectOperation::Builtin | ObjectOperation::ClassLayout,
        ) => true,
        None | Some(ObjectOperation::OrderedCommandImport) => {
            first.invalid_executable.is_some()
                || entry_requests_objects
                || (object_imports && !first.names.is_empty())
        }
    };
    // Existing object imports keep their normal project-module lookup. Only a new
    // startup fallback needs to verify that the imported provider is bundled.
    let consider_startup =
        !object_entry || typed_graph.is_some_and(|graph| graph.imports_runtime_class);
    let logging_script = consider_startup && startup_fallback && first.imports_logging;
    let tokenize_script = consider_startup && imports_provider("tokenize");
    let configparser_script = consider_startup && imports_provider("configparser");
    let pyio_script = consider_startup && imports_provider("_pyio");
    let installed_script = consider_startup
        && startup_fallback
        && first
            .names
            .iter()
            .any(|(level, name)| *level == 0 && crate::zip_entry::packaged_member(name).is_some());
    let startup_provider =
        logging_script || tokenize_script || configparser_script || pyio_script || installed_script;
    let fallback_script = !object_entry && startup_provider;
    let ordered_only = !object_entry && !fallback_script;
    if ordered_only
        && (!ordered_command
            || first.uses_import_builtin
            || first
                .names
                .iter()
                .any(|(level, name)| *level != 0 || name == "__future__"))
    {
        return Ok(None);
    }
    let provider_script = provider_script || fallback_script;
    if let Some((parent, _)) = packaged_file_module.and_then(|name| name.rsplit_once('.')) {
        first.add(0, parent);
    }
    if let Some(name) = module_entry {
        first.add(0, "runpy");
        // Register source candidates without interpreting a module operand as a filesystem path.
        // runpy and the normal finder decide which one can actually execute.
        if crate::modules::valid_module_operand(name) {
            first.add(0, name);
            first.add(0, &format!("{name}.__main__"));
        }
    }
    for name in &options.include_modules {
        first.add(0, name);
    }
    let registered_imports = typed_graph
        .filter(|_| object_imports && first.invalid_executable.is_none())
        .map(import_module::registered_imports)
        .unwrap_or_default();
    first
        .names
        .extend(registered_imports.targets.into_iter().map(|name| (0, name)));
    let cwd = if source_entry {
        PathBuf::from("/")
    } else {
        std::env::current_dir().map_err(|e| e.to_string())?
    };
    let isolated = startup.is_some_and(|(flags, _)| flags.isolated);
    let entry = cwd.join(path);
    let root = if let Some(root) = module_root {
        cwd.join(root)
    } else if entry_identity.is_some() {
        entry.parent().unwrap_or(&cwd).to_owned()
    } else {
        cwd.clone()
    };
    // Script entries follow the physical directory. A directory operand retains its spelling.
    let root = match (
        cfg!(unix) && !options.path_entry,
        &entry_identity,
        module_root,
    ) {
        (true, Some(_), Some(_)) => filesystem.canonicalize(&root).unwrap_or(root),
        (true, Some(identity), None) => identity.parent().unwrap_or(&root).to_owned(),
        _ => root,
    };
    // Keep the owning invocation's spelling for declared filenames without moving
    // imports away from the physical script directory (e.g. under a symlink).
    let invoked_root = entry_identity.as_ref().and_then(|identity| {
        let relative = identity.strip_prefix(&root).ok()?;
        let spelling = entry.ancestors().nth(relative.components().count())?;
        (spelling != root
            && !options.include_modules.is_empty()
            && filesystem.canonicalize(spelling).ok().as_ref() == Some(&root))
        .then(|| (root.clone(), spelling.to_owned()))
    });
    let mut initial = Vec::new();
    let safe_path = startup.is_some_and(|(flags, _)| flags.safe_path);
    if options.path_entry {
        // CPython preserves the directory spelling, including symlinks, and searches
        // an explicitly requested entry even when isolated startup omits the cwd.
        initial.push(entry.clone());
    } else if !source_entry && !safe_path {
        // The in-memory graph has a diagnostic filename, but no project import root.
        initial.push(root.clone());
    }
    // site removes duplicate startup paths before Python prepends path[0].
    let path0_len = initial.len();
    let dedup = startup.is_some_and(|(flags, _)| !flags.no_site);
    let mut seen = HashSet::new();
    initial.extend(
        options
            .extension_paths
            .iter()
            .map(|p| cwd.join(p))
            .filter(|p| !dedup || seen.insert(p.clone())),
    );
    let startup_paths = &initial[path0_len..];
    let installed_layout = options
        .stdlib_dir
        .iter()
        .chain(&options.site_dirs)
        .any(|p| !startup_paths.contains(p));
    let installed_dirs: Vec<_> = options
        .installed_dirs()
        .filter(|p| !startup_paths.contains(p))
        .cloned()
        .collect();
    let mut roots = initial.clone();
    let explicit_roots: Vec<_> = first
        .explicit_roots
        .iter()
        .filter(|_| !source_entry)
        .map(|path| cwd.join(path))
        .collect();
    roots.extend(explicit_roots.iter().cloned());
    let implicit_dirs: Vec<_> = installed_dirs
        .iter()
        .filter(|p| !roots.contains(p))
        .cloned()
        .collect();
    roots.extend(implicit_dirs.iter().cloned());
    // Discover each search directory once even when it appears twice in sys.path.
    let mut seen_roots = HashSet::new();
    roots.retain(|root| seen_roots.insert(root.clone()));
    let bundled_root = BUNDLED_SOURCE_ROOT;
    let mut inputs = SourceInputs::default();
    let mut archives = HashMap::new();
    let mut registrations = emit::RegisteredBodies::default();
    let mut syntax = None;
    for path in options
        .extension_paths
        .iter()
        .enumerate()
        .filter(|(index, _)| options.python_archive_indices.contains(index))
        .map(|(_, path)| cwd.join(path))
        .chain((options.path_entry && !filesystem.is_dir(&entry)).then(|| entry.clone()))
    {
        if let std::collections::hash_map::Entry::Vacant(entry) = archives.entry(path) {
            let archive =
                crate::zip_entry::ZipSources::open(entry.key(), &mut inputs, &mut filesystem)?;
            entry.insert(archive);
        }
    }
    for path in explicit_roots.iter().cloned() {
        if let std::collections::hash_map::Entry::Vacant(entry) = archives.entry(path)
            && let Some(archive) =
                crate::zip_entry::ZipSources::discover(entry.key(), &mut inputs, &mut filesystem)?
        {
            entry.insert(archive);
        }
    }
    let mut archive_modules = (!archives.is_empty()).then(|| {
        roots
            .iter()
            .filter_map(|root| archives.get(root))
            .flat_map(|archive| archive.modules())
            .collect::<std::collections::BTreeSet<_>>()
    });
    let mut installed_wheels = Vec::new();
    let mut wheel_files = HashMap::new();
    let mut wheel_roots: Vec<_> = roots
        .iter()
        .filter(|root| {
            !archives.contains_key(*root)
                && (startup_paths.contains(root)
                    || implicit_dirs.contains(root)
                    || explicit_roots.contains(root))
        })
        .collect();
    // The runtime executes the first FILE body in __main__. Its finder and
    // relative imports use the qualified name belonging to that same file.
    let entry_name = packaged_file_module.unwrap_or("__main__");
    let mut sources: Vec<ModuleSource<'_>> = vec![(
        entry_name.to_owned(),
        if in_memory {
            PathBuf::from("<string>")
        } else {
            entry
        },
        src.as_str().into(),
    )];
    if let Some((identity, hash, digest)) = entry_snapshot {
        let physical = registrations
            .physical_files
            .entry(sources[0].1.clone())
            .or_default();
        physical.source_kind = Some(SourceKind::DiskFile);
        physical.digest = digest;
        if module_entry.is_none() && options.include_modules.contains("__main__") {
            registrations.checked_units.entry(0).or_default().run_path =
                declared_open_path(&sources[0].1, &identity, &mut inputs, &mut filesystem)?;
        }
        inputs.files.push((path.to_owned(), identity, hash));
    }
    let mut selected_sources = Vec::new();
    let mut site_initializer = None;
    if module_entry.is_none() {
        selected_sources.push((0, true, checked_graph.map(|graph| graph.entry)));
    }
    let mut unavailable_sources = HashMap::new();
    let mut module_errors = HashMap::new();
    if let Some(error) = first.invalid_executable.take() {
        module_errors.insert(sources[0].1.clone(), error);
    }
    let mut seen = HashSet::new();
    let mut imp_in_file_graph = false;
    // Keep source hints on the winning regular package's path. The runtime finder
    // can still load a different child if a parent changes its __path__.
    let mut entry_sources: HashMap<String, (PathBuf, bool)> = HashMap::new();
    let mut checked_sdk_providers = Vec::new();
    let mut bundled_includes = HashMap::new();
    let mut before_site = HashSet::new();
    let mut named_registrations = None;
    let mut ordinary_registrations = None;
    let python_optimization = startup.map_or(0, |(flags, _)| flags.optimize);
    let mut literals = LiteralImports::default();
    literals.register(0, &mut first, false, true, ImportOrigin::File);
    let keep_docstrings = python_optimization < 2;
    // Standalone shipped module checks can release their syntax immediately. Application
    // types and physical loader registrations are selected later in registration order.
    let mut shipped_checks = HashMap::new();
    let mut shipped_imports = |scanned_modules: &mut HashMap<usize, ScannedModule>,
                               unit: usize,
                               name: &str,
                               path: &Path,
                               source: &str,
                               kind: SourceKind,
                               getter: Option<&mut Imports>| {
        if output.is_some() {
            let (imports, names) = scan_imports(source, kind, getter, None, true)?;
            if let Some(names) = names {
                scanned_modules.insert(
                    unit,
                    ScannedModule {
                        parsed: None,
                        lowered: LoweredRoot::Unchanged(names),
                    },
                );
            }
            return Ok(imports);
        }
        let (imports, scanned) =
            scan_imports_for_preparation(source, kind, None, getter, Some(source))?;
        if let Some(input) = scanned
            && imports.invalid_executable.is_none()
        {
            let checked = prepare::check_shipped(name, path, input, python_optimization);
            shipped_checks.insert(unit, prepare::Shipped::Checked(checked));
        }
        Ok(imports)
    };
    let mut shadowed_imports = VecDeque::new();
    let mut file_origins = Vec::new();
    let mut frozen_files: Vec<String> = Vec::new();
    let mut searched_frozen_files = HashSet::new();
    let mut selected_work =
        VecDeque::from([((entry_name.to_owned(), false, first), ImportOrigin::File)]);
    // Site imports its hook before the launcher inserts path[0]. Let the
    // ordinary source collector prepare its initializer and dependencies.
    let mut site_roots = Vec::new();
    let mut site_root_count = 0;
    if options.sitecustomize || options.stdlib_sitecustomize {
        site_roots.extend_from_slice(startup_paths);
        site_roots.extend_from_slice(&installed_dirs);
        site_root_count = site_roots.len();
        // These alternatives can be imported after Python installs the script path.
        site_roots.extend(
            roots
                .iter()
                .filter(|root| !startup_paths.contains(root) && !installed_dirs.contains(root))
                .cloned(),
        );
        let mut hook = Imports::default();
        hook.add(0, "sitecustomize");
        selected_work.push_front(((String::new(), false, hook), ImportOrigin::Startup));
    }
    let mut sdk_work = VecDeque::new();
    let mut following_sdk = false;
    let mut work = VecDeque::new();
    let mut uses_import_builtin = false;
    let mut uses_exec = false;
    let mut computed_import = registered_imports.computed;
    let mut bundled_only = false;
    let mut getters = VecDeque::new();
    let mut bundled_imports = VecDeque::new();
    let mut through_getter = false;
    loop {
        // Everything named by the entry, declarations, and ordinary file imports
        // is drained before lazy providers and bulk wheel/archive members.
        if selected_work.is_empty() && work.is_empty() && named_registrations.is_none() {
            if !literals.origins.is_empty() {
                // A wildcard or explicit import can bind exec in the caller. Populate
                // its type database with the actual selected providers before resolving
                // another level, including providers named only by a decoded module.
                let session =
                    crate::analysis::Session::registered_syntax(&mut syntax, &root, &roots)?;
                for &(unit, _, _) in &selected_sources[literals.seeded_winners..] {
                    let (_, path, source) = &sources[unit];
                    if !path.starts_with(bundled_root) && !module_errors.contains_key(path) {
                        session.seed(path, source)?;
                    }
                }
                literals.seeded_winners = selected_sources.len();
                let mut imports = literals.forest.advance(session);
                for (unit, flags, candidates) in literals.pending_roots.drain(..) {
                    let (file, parsed, source) = session.selected_file(&sources[unit].1)?;
                    let sites = session.select_literal_level(
                        file,
                        &parsed,
                        source.as_str(),
                        flags,
                        &candidates,
                    );
                    let names = literals.forest.record_root(unit, sites);
                    imports.push((unit, names));
                }
                for (unit, names) in imports {
                    if names.is_empty() {
                        continue;
                    }
                    let (package, selected, origin) = literals.origins[&unit];
                    let mut hints = Imports::default();
                    for name in names {
                        hints.add(0, &name);
                    }
                    let queue = if selected {
                        &mut selected_work
                    } else {
                        &mut work
                    };
                    queue.push_back(((sources[unit].0.clone(), package, hints), origin));
                }
                if !selected_work.is_empty() || !work.is_empty() || literals.forest.has_pending() {
                    continue;
                }
            }
            named_registrations = Some(selected_sources.len());
        }
        let (item, selected) = if let Some(next) = selected_work.pop_front() {
            (next, true)
        } else if let Some(next) = work.pop_front() {
            (next, false)
        } else if let Some(next) = getters.pop_front() {
            // Keep project shadows for a bundled module's lazy attributes. The
            // ordinary graph has already checked any modules the user imported.
            through_getter = true;
            ((next, ImportOrigin::Provider), false)
        } else if let Some(next) = sdk_work.pop_front() {
            // Finish the ordinary file graph first. Linking an SDK dependency does
            // not by itself make the application's optional frozen shadows reachable.
            following_sdk = true;
            through_getter = false;
            ((next, ImportOrigin::Provider), true)
        } else if let Some((next, lazy)) = bundled_imports.pop_front() {
            // The program can reach these providers through a ZIP or an extended package
            // path. Their hints must not force unrequested application files to compile.
            through_getter = lazy;
            bundled_only = true;
            ((next, ImportOrigin::Provider), false)
        } else if let Some(modules) = archive_modules.take().or_else(|| {
            wheel_roots.retain(|root| {
                let selected = sources.iter().any(|(_, path, _)| path.starts_with(root));
                if selected {
                    installed_wheels.extend(wheel_records.discover(
                        root,
                        &mut inputs,
                        &mut filesystem,
                    ));
                }
                !selected
            });
            crate::installed_wheels::InstalledWheel::take_active(
                &mut installed_wheels,
                &sources,
                &mut wheel_files,
            )
        }) {
            // A computed import can select another member of a supplied archive or an active installed wheel.
            // Register those origins without importing them or selecting additional project winners.
            let mut imports = Imports::default();
            imports
                .names
                .extend(modules.into_iter().map(|name| (0, name)));
            through_getter = false;
            following_sdk = false;
            bundled_only = false;
            (
                (
                    ("__main__".to_owned(), false, imports),
                    ImportOrigin::Alternative,
                ),
                false,
            )
        } else {
            through_getter = false;
            // Collect late imports and their bundled dependencies after the explicit
            // file graph, without parsing unrelated modules in project directories.
            let mut imports = Imports::default();
            if uses_import_builtin && !seen.contains("pprint") {
                imports.names.push((0, "pprint".to_owned()));
            }
            // A runtime-selected decimal module in a standalone binary needs its private provider.
            if startup.is_none() && computed_import && !seen.contains("decimal") {
                imports.add(0, "decimal");
            }
            if computed_import && !seen.contains("zoneinfo") {
                imports.names.push((0, "zoneinfo".to_owned()));
            }
            add_late_bundled_imports(
                &mut imports,
                &seen,
                uses_import_builtin,
                uses_exec,
                computed_import,
                options.user_site_pth.is_some(),
            );
            if imports.names.is_empty() {
                // A file loader or a runtime change of finder can still select physical
                // alternatives. Collect their closures after all ordinary import winners.
                if (frozen_files.is_empty() && shadowed_imports.is_empty())
                    || ordinary_registrations.is_some()
                {
                    break;
                }
                ordinary_registrations = Some(selected_sources.len());
                work.append(&mut shadowed_imports);
                bundled_only = false;
                for name in frozen_files.drain(..) {
                    let top = name.split('.').next().unwrap_or(&name);
                    if !imre_ast::providers::is_optional_frozen(top) || imp_in_file_graph {
                        imports.names.push((0, name));
                    }
                }
            } else {
                bundled_only = true;
            }
            (
                ((String::new(), false, imports), ImportOrigin::Provider),
                false,
            )
        };
        let ((owner, package, imports), origin) = item;
        let file_alternatives = ordinary_registrations.is_some();
        if imports
            .names
            .iter()
            .any(|(level, name)| *level == 0 && name == "importlib.abc.SourceLoader")
            && let Some(graph) = typed_graph
            && (owner == "__main__" || graph.modules.iter().any(|m| !m.bundled && m.name == owner))
        {
            graph.require_fallback_calls();
        }
        // Include the current module's computed imports when deciding its ownership.
        computed_import |= imports.computed_import;
        // A boxed provider can discover further site modules while collecting dependencies.
        // Keep checked machinery diagnostics on the modules in the original program.
        let machinery_graph = typed_graph.filter(|_| {
            matches!(operation, Some(ObjectOperation::ClassLayout))
                || seen.contains("zoneinfo")
                || computed_import
        });
        if module_entry.is_none()
            && !registered_imports.called
            && (!provider_script || owner == "__main__")
            && !through_getter
            && imports.uses_machinery_export("ModuleSpec")
            && !checked_entry
            && (owner == "__main__"
                || machinery_graph
                    .is_none_or(|graph| graph.modules.iter().any(|m| m.name == owner)))
        {
            return Ok(None);
        }
        uses_exec |= imports.uses_exec;
        // A checked caller can request the loader before its other annotated calls are lowered.
        if options.python_startup().is_none()
            && (imports.uses_machinery_export("BuiltinImporter")
                || imports.uses_machinery_export("FrozenImporter"))
            && let Some(graph) = typed_graph
            && (owner == "__main__" || graph.modules.iter().any(|m| !m.bundled && m.name == owner))
        {
            graph.require_fallback_calls();
        }
        let mut names = imports.names;
        // Provider hints appended below do not select additional standalone modules.
        let literal_imports = names.len();
        if owner == "importlib" {
            // Filesystem modules can import the loader protocols through either public module.
            names.push((0, "importlib.abc".to_owned()));
            names.push((0, "importlib.util".to_owned()));
        }
        // A late import of native pickle also needs its bundled reader and dependencies.
        if imports.uses_import_builtin {
            uses_import_builtin = true;
            names.push((0, "pickle".to_owned()));
        }
        // Importing array registers its type with MutableSequence. A runtime
        // __import__ can select array without naming it in a static import.
        if imports.uses_import_builtin
            || names.iter().any(|(level, name)| {
                *level == 0 && matches!(name.as_str(), "array" | "collections" | "collections.abc")
            })
        {
            names.push((0, "_collections_abc".to_owned()));
        }
        // Random.sample needs bisect when sampling with counts. A computed
        // import can select random without mentioning it in a module name.
        if imports.uses_import_builtin
            || imports.computed_import
            || imports.uses_exec
            || names
                .iter()
                .any(|(level, name)| *level == 0 && name == "random")
        {
            names.push((0, "bisect".to_owned()));
        }
        // The native string scanner imports the decoder's exception on a syntax error.
        if names
            .iter()
            .any(|(level, name)| *level == 0 && name == "_json")
        {
            names.push((0, "json.decoder".to_owned()));
        }
        let package_name = crate::modules::import_package(&owner, package);
        for (index, (level, requested)) in names.into_iter().enumerate() {
            let Some(name) = crate::modules::resolve_import(package_name, level, &requested) else {
                continue;
            };
            // Frozen modules are selected before path finders. Names from a from-import
            // can refer to their attributes without making a same-named package reachable.
            let top = name.split('.').next().unwrap_or(&name);
            let frozen = imre_ast::providers::is_frozen(top, || frozen_modules);
            let literal = index < literal_imports;
            let file_graph =
                !bundled_only && (!following_sdk || (origin == ImportOrigin::File && literal));
            let name = if frozen && !file_alternatives {
                // A switchable name can still need its physical file if a real file imports
                // _imp later. Ordinary SDK implementation imports do not select file loaders.
                if file_graph || (!bundled_only && imre_ast::providers::is_optional_frozen(top)) {
                    frozen_files.push(name.clone());
                }
                top
            } else {
                &name
            };
            let select_member = selected && startup.is_none() && !frozen && literal;
            let mut prefix = String::new();
            let mut project_parent = false;
            let mut on_sdk_path = true;
            for part in name.split('.').filter(|s| !s.is_empty()) {
                if !prefix.is_empty() {
                    if let Some((root, package)) = entry_sources.get(&prefix) {
                        project_parent |= initial.contains(root);
                        on_sdk_path &= *package && root == Path::new(bundled_root);
                    }
                    prefix.push('.');
                }
                prefix.push_str(part);
                if prefix == "_imp" && file_graph {
                    imp_in_file_graph = true;
                }
                let new_module = seen.insert(prefix.clone());
                let wheel_origins = wheel_files.remove(&prefix);
                let new_file =
                    file_alternatives && frozen && searched_frozen_files.insert(prefix.clone());
                let selected_member = if selected && !frozen && literal && on_sdk_path {
                    crate::zip_entry::packaged_member(&prefix)
                } else {
                    None
                };
                // A provider hint may have visited this name without registering a source.
                if !new_module
                    && wheel_origins.is_none()
                    && !new_file
                    && (selected_member.is_none()
                        || entry_sources.contains_key(&prefix)
                        || sources.iter().any(|(name, path, _)| {
                            name == &prefix && path.starts_with(bundled_root)
                        }))
                {
                    continue;
                }
                let mut bundled = bundled_module_source(&prefix, startup.is_none());
                // Standalone programs can run without the launcher's stdlib ZIP.
                if bundled.is_none()
                    && startup.is_none()
                    && let Some(member) = crate::zip_entry::standalone_member(&prefix)
                {
                    bundled = Some((member, crate::zip_entry::installed_source(member)?));
                }
                let installed = if bundled.is_some() {
                    None
                } else {
                    selected_member.or_else(|| crate::zip_entry::packaged_member(&prefix))
                };
                if !implicit_dirs.is_empty()
                    && (bundled.is_some()
                        || installed.is_some()
                        || imre_ast::providers::provider(&prefix).is_some()
                        || prefix
                            .rsplit_once('.')
                            .is_some_and(|(parent, _)| before_site.contains(parent)))
                {
                    before_site.insert(prefix.clone());
                }
                let inspect_file = select_member
                    || (following_sdk
                        && !bundled_only
                        && !frozen
                        && literal
                        && origin != ImportOrigin::Alternative);
                let builtin = matches!(
                    imre_ast::providers::builtin_finder_provider(
                        &prefix,
                        || frozen,
                        || bundled.is_some() || installed.is_some()
                    ),
                    Some(imre_ast::providers::Provider::Builtin)
                );
                let filesystem_import = inspect_file && !builtin;
                let mut bundled_winner = None;
                if let Some((filename, source)) = bundled
                    && new_module
                {
                    let path = Path::new(bundled_root).join(filename);
                    let package = path.ends_with("__init__.py");
                    let mut getter = Imports::default();
                    let mut imports = shipped_imports(
                        &mut scanned_modules,
                        sources.len(),
                        &prefix,
                        &path,
                        source,
                        SourceKind::File,
                        Some(&mut getter),
                    )
                    .map_err(|e| format!("{}: {e}", path.display()))?;
                    // unittest's loader imports application modules by name, including during discovery.
                    // Other bundled imports are implementation details of a known provider.
                    if prefix != "unittest.loader" {
                        imports.computed_import = false;
                        getter.computed_import = false;
                    }
                    // An earlier regular project ancestor controls the live search path.
                    // Keep the registered alternative without following it into the project.
                    let mut enqueue = |hints, lazy| {
                        let next = (prefix.clone(), package, hints);
                        if project_parent {
                            bundled_imports.push_back((next, lazy || through_getter));
                        } else if lazy {
                            getters.push_back(next);
                        } else {
                            let origin = if origin == ImportOrigin::Alternative {
                                origin
                            } else {
                                ImportOrigin::Provider
                            };
                            work.push_back((next, origin));
                        }
                    };
                    enqueue(imports, false);
                    if !getter.names.is_empty() || getter.uses_exec {
                        enqueue(getter, true);
                    }
                    if on_sdk_path && let Some(name) = options.include_modules.get(&prefix) {
                        bundled_includes.insert(name, frozen);
                    }
                    let mut checked = None;
                    if on_sdk_path
                        && let Some(graph) = checked_graph
                        && let Some(id) = graph.bundled_module(&prefix)
                    {
                        if graph.modules[id].original_source() == source {
                            checked = Some(id);
                        } else {
                            checked_sdk_providers.push((id, sources.len()));
                        }
                    }
                    if select_member && on_sdk_path && !project_parent && !builtin {
                        bundled_winner = Some((sources.len(), checked));
                    }
                    sources.push((prefix.clone(), path, source.into()));
                }
                // Frozen runpy drives -m before a project runpy on sys.path.
                if frozen && (!file_alternatives || prefix == "runpy") {
                    continue;
                }
                let graph_file =
                    checked_graph.and_then(|graph| Some((graph, graph.python_file(&prefix)?)));
                let search_roots = if origin == ImportOrigin::Startup {
                    &site_roots
                } else {
                    &roots
                };
                for (index, root) in search_roots.iter().enumerate().filter(|_| !bundled_only) {
                    if implicit_dirs.contains(root) && before_site.contains(&prefix) {
                        continue;
                    }
                    let parent = prefix.rsplit_once('.').map(|(parent, _)| parent);
                    let selected_parent = parent.and_then(|name| entry_sources.get(name));
                    let on_package_path = selected_parent
                        .is_none_or(|(selected, package)| *package && selected == root);
                    let winner = on_package_path
                        && !entry_sources.contains_key(&prefix)
                        && (origin != ImportOrigin::Startup || index < site_root_count);
                    let child_origin = origin
                        .file(winner && (filesystem_import || origin == ImportOrigin::Startup));
                    let selected_child = winner
                        && (select_member && filesystem_import || origin == ImportOrigin::Startup);
                    if !on_package_path && let Some(parent) = parent {
                        // A different known package may become the live parent after sys.path changes.
                        let available = if let Some(archive) = archives.get(root) {
                            archive.package(parent) == Some(true)
                                || archive.contains_namespace(parent)
                        } else {
                            filesystem.is_dir(&root.join(parent.replace('.', "/")))
                        };
                        if !available {
                            continue;
                        }
                    }
                    if let Some(archive) = archives.get_mut(root) {
                        if !new_module && wheel_origins.is_some() {
                            continue;
                        }
                        if let Some(package) = archive.package(&prefix) {
                            if let Some(member) = archive.module(&prefix)
                                && let Ok((mut imports, scanned)) = scan_imports_for_preparation(
                                    &member.source,
                                    SourceKind::DecodedFile,
                                    None,
                                    None,
                                    None,
                                )
                            {
                                if let Some(module) = scanned.and_then(ScannedModule::from_input) {
                                    scanned_modules.insert(sources.len(), module);
                                }
                                if !initial[..path0_len].contains(root) {
                                    registrations.python_providers.insert(sources.len());
                                }
                                registrations
                                    .archive_members
                                    .insert(member.path.clone(), member.digest);
                                if let Some(error) = imports.invalid_executable {
                                    module_errors.insert(member.path.clone(), error);
                                } else {
                                    if winner && named_registrations.is_none() {
                                        literals.register(
                                            sources.len(),
                                            &mut imports,
                                            package,
                                            select_member && filesystem_import,
                                            child_origin,
                                        );
                                    }
                                    let queue = if selected_child {
                                        &mut selected_work
                                    } else {
                                        &mut work
                                    };
                                    child_origin.enqueue(queue, (prefix.clone(), package, imports));
                                    if winner {
                                        let program = initial[..path0_len].contains(root);
                                        let checked = graph_file.and_then(|(graph, id)| {
                                            let checked = &graph.modules[id];
                                            (checked.path == member.path
                                                && checked.original_source() == member.source)
                                                .then_some(id)
                                        });
                                        if origin == ImportOrigin::Startup && owner.is_empty() {
                                            site_initializer = Some(sources.len());
                                        }
                                        selected_sources.push((sources.len(), program, checked));
                                    }
                                }
                                sources.push((prefix.clone(), member.path, member.source.into()));
                            }
                            if !implicit_dirs.is_empty() {
                                before_site.insert(prefix.clone());
                            }
                            if winner {
                                entry_sources.insert(prefix.clone(), (root.clone(), package));
                            }
                        }
                        continue;
                    }
                    let stem = root.join(prefix.replace('.', "/"));
                    let file = stem.with_extension("py");
                    let init = stem.join("__init__.py");
                    // The checked resolver already selected a concrete package or module at
                    // this root. Other roots still supply alternatives for a live sys.path.
                    let checked_slot = graph_file.and_then(|(graph, id)| {
                        let package = graph.registration_at(id, root, &file, &init)?;
                        Some((&graph.inputs.files[id].1, package))
                    });
                    let package = filesystem.is_file(&init);
                    if checked_slot.is_some_and(|(_, was_package)| was_package != package) {
                        return Err(format!(
                            "{}: package initializer {} during compilation",
                            init.display(),
                            if package { "appeared" } else { "disappeared" },
                        ));
                    }
                    let (path, package) = if package {
                        (init, true)
                    } else if checked_slot.is_some() || filesystem.is_file(&file) {
                        inputs.missing.push(init);
                        (file, false)
                    } else {
                        inputs.missing.extend([init, file]);
                        if filesystem.is_dir(&stem) {
                            if ordered_only {
                                return Ok(None);
                            }
                            inputs.directories.push((
                                stem.clone(),
                                filesystem.canonicalize(&stem).map_err(|e| e.to_string())?,
                            ));
                            sources.push((prefix.clone(), stem, "".into()));
                        } else {
                            inputs.missing.push(stem);
                        }
                        continue;
                    };
                    // A parent can import the entry by its qualified name.
                    // This physical body is already registered as unit zero.
                    if packaged_file_module == Some(&prefix) && path == sources[0].1 {
                        if winner {
                            entry_sources.insert(prefix.clone(), (root.clone(), package));
                        }
                        continue;
                    }
                    if !implicit_dirs.is_empty() && !implicit_dirs.contains(root) {
                        before_site.insert(prefix.clone());
                    }
                    // A late command import can use the runtime's builtins and bundled
                    // providers. Leave project-source imports on their original route.
                    if ordered_only {
                        return Ok(None);
                    }
                    // Only an unchecked winner can displace the requested bundled provider.
                    if winner
                        && checked_slot.is_none()
                        && initial.contains(root)
                        && ((request_script && prefix == "urllib")
                            || (ftp_script && prefix == "ftplib")
                            || (argparse_script && prefix == "argparse")
                            || (fallback_script
                                && ((logging_script && prefix == "logging")
                                    || (tokenize_script && prefix == "tokenize")
                                    || (configparser_script && prefix == "configparser")
                                    || (pyio_script && prefix == "_pyio"))))
                    {
                        return Ok(None);
                    }
                    if !new_module
                        && wheel_origins
                            .as_ref()
                            .is_some_and(|origins| !origins.contains(&path))
                    {
                        continue;
                    }
                    // A -m program can reorder its search paths before importing this name.
                    let shadowed =
                        (module_entry.is_some() || origin == ImportOrigin::Startup) && !winner;
                    let file_alternatives =
                        file_alternatives || wheel_origins.is_some() || shadowed;
                    let identity = if let Some((identity, _)) = checked_slot {
                        identity.clone()
                    } else {
                        match filesystem.canonicalize(&path) {
                            Ok(identity) => identity,
                            Err(_) if file_alternatives => {
                                inputs.missing.push(path);
                                continue;
                            }
                            Err(error) => return Err(error.to_string()),
                        }
                    };
                    let run_path = if winner
                        && ordinary_registrations.is_none()
                        && options.include_modules.contains(&prefix)
                    {
                        let open =
                            declared_open_path(&path, &identity, &mut inputs, &mut filesystem)?;
                        if open.as_deref() == Some(&path)
                            && let Some((physical, spelling)) = &invoked_root
                            && physical == root
                            && let Ok(relative) = path.strip_prefix(root)
                        {
                            let alias = spelling.join(relative);
                            let resolved = filesystem.canonicalize(&alias).ok();
                            let same = resolved.as_ref() == Some(&identity);
                            inputs
                                .declared_file_resolutions
                                .push((alias.clone(), resolved));
                            if same {
                                declared_open_path(&alias, &identity, &mut inputs, &mut filesystem)?
                                    .or(open)
                            } else {
                                open
                            }
                        } else {
                            open
                        }
                    } else {
                        None
                    };
                    let checked =
                        graph_file.filter(|(graph, id)| graph.inputs.files[*id].1 == identity);
                    // A registered file can still be selected after the path or finder order
                    // changes. Keep its syntax failure with the loader that would execute it.
                    let file_alternate = checked.is_none()
                        && (entry_sources.contains_key(&prefix)
                            || builtin && !options.include_modules.contains(&prefix));
                    let (mut src, digest) = if let Some((graph, id)) = checked {
                        inputs.files.push((
                            path.clone(),
                            identity,
                            graph.inputs.files[id].2.clone(),
                        ));
                        (
                            graph.modules[id].original_source().to_owned(),
                            graph.modules[id].raw_digest,
                        )
                    } else {
                        let bytes = match filesystem.read_source(&identity) {
                            Ok(bytes) => bytes,
                            Err(error)
                                if file_alternatives
                                    && error.kind() != std::io::ErrorKind::Interrupted =>
                            {
                                inputs.missing.push(path);
                                continue;
                            }
                            Err(error) => return Err(format!("{}: {error}", path.display())),
                        };
                        inputs.files.push((
                            path.clone(),
                            identity,
                            blake3::hash(&bytes).to_hex().to_string(),
                        ));
                        if wheel_origins.is_some() {
                            use sha2::{Digest, Sha256};
                            let digest = Sha256::digest(&bytes).into();
                            let Ok(text) = imre_ast::source_encoding::decode_compile_source(
                                bytes,
                                |_, error| error.to_string(),
                            ) else {
                                continue;
                            };
                            registrations
                                .physical_files
                                .entry(path.clone())
                                .or_default()
                                .source_kind = Some(SourceKind::DecodedFile);
                            (text, Some(digest))
                        } else {
                            let source = match crate::DiskSource::decode(&path, bytes) {
                                Ok(text) => text,
                                Err(_) if file_alternatives => continue,
                                Err(error) => return Err(error),
                            };
                            (source.text, source.raw_digest)
                        }
                    };
                    let alternative = (file_alternatives && checked.is_none())
                        .then_some((path.as_path(), keep_docstrings));
                    let kind = registrations
                        .physical_files
                        .get(&path)
                        .and_then(|file| file.source_kind)
                        .unwrap_or(SourceKind::DiskFile);
                    // The snapshot above came from the checked resolver's physical file.
                    // ZIP members, decoded alternatives and sources rewritten for inference
                    // keep their own parser input.
                    let checked_package =
                        checked.filter(|(graph, id)| graph.modules[*id].is_package == package);
                    let parsed = checked_package
                        .filter(|_| winner && kind == SourceKind::DiskFile)
                        .and_then(|(graph, id)| CheckedFile::select(graph, id));
                    let mut declaration = None;
                    let imports = if let Some(file) = parsed {
                        retained_imports(file.parsed(modules), None, None)
                    } else if winner && alternative.is_none() && !file_alternate {
                        if checked.is_none() && kind == SourceKind::DiskFile {
                            declaration = checked_graph
                                .and_then(|graph| graph.session.declaration_parser(&path, &src));
                        }
                        scan_imports_for_preparation(&src, kind, declaration.as_deref(), None, None)
                    } else {
                        let parser_kind = if file_alternate {
                            SourceKind::DecodedFile
                        } else {
                            kind
                        };
                        scan_imports(&src, parser_kind, None, alternative, false)
                            .map(|(imports, _)| (imports, None))
                    };
                    let (mut imports, scanned) = match imports {
                        Ok(imports) => imports,
                        Err(_) if file_alternatives => continue,
                        Err(error) => return Err(format!("{}: {error}", path.display())),
                    };
                    let physical = registrations
                        .physical_files
                        .entry(path.clone())
                        .or_default();
                    physical.source_kind = Some(kind);
                    physical.digest = digest.or(physical.digest);
                    // Later alternatives remain available to the runtime finder. Only the
                    // winning provider supplies the initial module binding.
                    if winner {
                        entry_sources.insert(prefix.clone(), (root.clone(), package));
                        if let Some(error) = &imports.invalid_executable {
                            let report = crate::frontend::phase::Diagnostic::at(
                                &src,
                                &error.range,
                                format!("SyntaxError: {}", error.message),
                            )
                            .with_path(&path);
                            if origin == ImportOrigin::Startup && owner.is_empty() {
                                return Err(report.to_string());
                            }
                            unavailable_sources.insert(prefix.clone(), report);
                        }
                    }
                    let loader_error = file_alternate && imports.invalid_executable.is_some();
                    if imports
                        .invalid_executable
                        .as_ref()
                        .is_none_or(|error| error.validate_at_origin)
                        || startup.is_none()
                        || loader_error
                    {
                        if let Some(error) = imports.invalid_executable {
                            if file_alternate && error.parse_error {
                                src = imre_ast::source_encoding::universal_newlines(src);
                            }
                            module_errors.insert(path.clone(), error);
                        } else {
                            if winner && named_registrations.is_none() {
                                literals.register(
                                    sources.len(),
                                    &mut imports,
                                    package,
                                    select_member && filesystem_import,
                                    child_origin,
                                );
                            }
                            let queue = if selected_child {
                                &mut selected_work
                            } else if shadowed && ordinary_registrations.is_none() {
                                // A dependency unique to a shadow must not veto the winning graph.
                                &mut shadowed_imports
                            } else {
                                &mut work
                            };
                            if let Some(mut module) = scanned.and_then(ScannedModule::from_input) {
                                if let Some(parser) = declaration {
                                    module.parsed = Some(ScannedParser::Declaration(parser));
                                }
                                scanned_modules.insert(sources.len(), module);
                            }
                            child_origin.enqueue(queue, (prefix.clone(), package, imports));
                        }
                        file_origins.push((sources.len(), root));
                        let registration = (prefix.clone(), path, src.into());
                        if winner && !loader_error {
                            // Only path zero belongs to the program. With safe path
                            // the first remaining search entry may be an installation.
                            let program = initial[..path0_len].contains(root);
                            if let Some(parsed) = parsed {
                                registrations
                                    .checked_units
                                    .entry(sources.len())
                                    .or_default()
                                    .source = Some(parsed);
                            }
                            if run_path.is_some() {
                                registrations
                                    .checked_units
                                    .entry(sources.len())
                                    .or_default()
                                    .run_path = run_path;
                            }
                            if origin == ImportOrigin::Startup && owner.is_empty() {
                                site_initializer = Some(sources.len());
                            }
                            selected_sources.push((
                                sources.len(),
                                program,
                                checked_package.map(|(_, id)| id),
                            ));
                        }
                        sources.push(registration);
                    }
                }
                // Physical search roots have now had their chance to supply this import.
                if let Some((unit, checked)) = bundled_winner
                    && !entry_sources.contains_key(&prefix)
                {
                    registrations.python_providers.insert(unit);
                    selected_sources.push((unit, false, checked));
                }
                if let Some(member) = installed {
                    // Removing an earlier path can expose this SDK member to its ordinary ZIP loader.
                    let winner = selected_member.is_some() && !entry_sources.contains_key(&prefix);
                    let (path, source, package, digest) =
                        if let Some(root) = &options.installed_stdlib_archive {
                            let archive = match archives.entry(root.clone()) {
                                std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
                                std::collections::hash_map::Entry::Vacant(e) => {
                                    e.insert(crate::zip_entry::ZipSources::open(
                                        root,
                                        &mut inputs,
                                        &mut filesystem,
                                    )?)
                                }
                            };
                            let Some(entry) = archive.module(&prefix) else {
                                continue;
                            };
                            (
                                entry.path,
                                Cow::Owned(entry.source),
                                entry.package,
                                Some(entry.digest),
                            )
                        } else {
                            use sha2::{Digest, Sha256};
                            let shipped = crate::zip_entry::installed_source(member)?;
                            let root = options.stdlib_fallback.as_deref();
                            let digest = root.map(|_| Sha256::digest(shipped.as_bytes()).into());
                            (
                                root.unwrap_or(Path::new(bundled_root)).join(member),
                                Cow::Borrowed(shipped),
                                member.ends_with("/__init__.py"),
                                digest,
                            )
                        };
                    let imports = if digest.is_some() {
                        scan_imports_for_preparation(
                            &source,
                            SourceKind::DecodedFile,
                            None,
                            None,
                            None,
                        )
                    } else {
                        shipped_imports(
                            &mut scanned_modules,
                            sources.len(),
                            &prefix,
                            &path,
                            &source,
                            SourceKind::DecodedFile,
                            None,
                        )
                        .map(|imports| (imports, None))
                    };
                    let Ok((imports, scanned)) = imports else {
                        continue;
                    };
                    if let Some(module) = scanned.and_then(ScannedModule::from_input) {
                        scanned_modules.insert(sources.len(), module);
                    }
                    registrations.python_providers.insert(sources.len());
                    if let Some(digest) = digest {
                        registrations.archive_members.insert(path.clone(), digest);
                    }
                    if winner {
                        entry_sources.insert(prefix.clone(), (bundled_root.into(), package));
                    }
                    if let Some(error) = imports.invalid_executable {
                        module_errors.insert(path.clone(), error);
                    } else if winner {
                        sdk_work.push_back((prefix.clone(), package, imports));
                        selected_sources.push((sources.len(), false, None));
                    } else {
                        bundled_imports.push_back(((prefix.clone(), package, imports), false));
                    }
                    sources.push((prefix.clone(), path, source));
                }
            }
        }
    }
    let threading = source_threading(|required| {
        sources
            .iter()
            .any(|(name, path, _)| name == required && path.starts_with(bundled_root))
    });
    let first_runtime_unit = sources.len();
    for (name, file, body) in runtime_sources(|name| seen.contains(name), threading) {
        sources.push((name.into(), Path::new(bundled_root).join(file), body.into()));
    }
    // These bodies extend native module scopes. Keep them out of the source finder and
    // out of the checked project's winners, even when no explicit import names them.
    let builtins: Vec<ModuleSource<'_>> =
        builtin_sources(options.target_os.unwrap_or(std::env::consts::OS))
            .into_iter()
            .map(|(name, file, body)| {
                (name.into(), Path::new(bundled_root).join(file), body.into())
            })
            .collect();
    // runpy selects __main__ for a package, including a namespace package.
    let module_body = module_entry.and_then(|name| {
        let main;
        let name = if entry_sources.get(name).is_none_or(|(_, package)| *package) {
            main = format!("{name}.__main__");
            main.as_str()
        } else {
            name
        };
        let (root, package) = entry_sources.get(name)?;
        let &(id, _, _) = selected_sources
            .iter()
            .find(|(id, _, _)| sources[*id].0 == name)?;
        Some((id, root, *package))
    });
    let entry_owner = module_body.and_then(|(id, root, _)| {
        crate::installed_wheels::ModuleEntryRoot::select(
            root,
            &sources[id].1,
            initial[..path0_len].contains(root),
            wheel_records,
            options,
            &mut inputs,
            &mut filesystem,
        )
    });
    // A runtime path change can select any linked file. Bind its functions using the
    // file's own search root and wheel membership, including for shadowed sources.
    for (id, root) in file_origins {
        let (_, path, _) = &sources[id];
        let application = entry_owner
            .as_ref()
            .and_then(|owner| owner.is_program(root, path))
            .unwrap_or_else(|| initial[..path0_len].contains(root));
        if !application {
            registrations.python_providers.insert(id);
        }
    }
    // A nested literal belongs to the same source slot as its outermost caller.
    let literal_sites = syntax.as_ref().map_or_else(Default::default, |session| {
        literals.forest.finish(session, python_optimization, |unit| {
            let (name, path, _) = &sources[unit];
            registrations
                .origin(unit, path.starts_with(bundled_root))
                .python_annotations(name)
        })
    });
    let sdk_runpy = sources
        .iter()
        .any(|(name, path, _)| name == "runpy" && path.starts_with(bundled_root))
        && (imre_ast::providers::is_frozen("runpy", || frozen_modules)
            || entry_sources
                .get("runpy")
                .is_none_or(|(root, _)| root == Path::new(bundled_root)));
    // A saved -m application can still find the file initializer selected from path[0].
    // ZIP and open module launchers keep their own finder and search path.
    let bound_path0 = sdk_runpy
        && !options.path_entry
        && !options.sitecustomize
        && !options.stdlib_sitecustomize
        && options.user_site_pth.is_none()
        && module_body.is_some_and(|(id, root, _)| {
            initial[..path0_len].contains(root)
                && registrations.physical_files.contains_key(&sources[id].1)
        });
    // Borrow the winners once discovery has finished growing the emitter's source table.
    let entry_start = if options.sitecustomize || options.stdlib_sitecustomize {
        // With unfrozen startup the physical site module can run before this hook.
        site_initializer
            .filter(|_| frozen_modules)
            .map(|unit| EntryStart::After(sources[unit].1.as_path()))
    } else {
        Some(EntryStart::Immediate)
    };
    let mut declared_open_paths = HashSet::new();
    let selected_sources: Vec<_> = selected_sources
        .into_iter()
        .enumerate()
        .map(|(index, (id, program, module))| {
            let application = !registrations.python_providers.contains(&id);
            if application
                && let (Some(graph), Some(module_id)) = (checked_graph, module)
                && graph.modules[module_id].original_source() == sources[id].2
            {
                registrations.checked_units.entry(id).or_default().module = Some(module_id);
            }
            // Emit the selected application's signature. Preserve the existing checker
            // scope: owning a declaration does not prove that a call will reach it.
            let mut registration =
                RegisteredSource::new(&sources[id], program && application, module);
            if !module_errors.contains_key(registration.path)
                && let Some(open) = registrations
                    .checked_units
                    .get(&id)
                    .and_then(|unit| unit.run_path.as_deref())
            {
                declared_open_paths.insert(open.as_os_str().to_owned());
            }
            registration.has_open_exec = literals.open_exec.contains(&id);
            registration.unit = output.is_some().then_some(id);
            registration.declares_signatures = application;
            registration.check_execution |= application;
            registration.main_body = match module_entry {
                None => id == 0,
                Some(_) => module_body.is_some_and(|(body, _, package)| id == body && !package),
            };
            registration.initial = entry_start.filter(|_| {
                index < ordinary_registrations.unwrap_or(usize::MAX)
                    && match module_entry {
                        Some(name) => registration.name == name,
                        // A packaged FILE enters its body after importing the parent.
                        None => registration.main_body && packaged_file_module.is_none(),
                    }
                    && !registration.name.contains('.')
                    && options.user_site_pth.is_none()
                    && module_entry.is_none_or(|name| {
                        sdk_runpy
                            && (options.path_entry && name == "__main__"
                                || crate::analysis::unambiguous_file_module(name))
                    })
                    && !startup.is_some_and(|(flags, _)| flags.has_warning_options)
            });
            // The selected source can execute from any user search root. Only the
            // SDK winner uses bundled_root, including when its bytes come from the SDK ZIP.
            registration.check_execution |= index < named_registrations.unwrap_or(0)
                && entry_sources
                    .get(registration.name)
                    .is_some_and(|(origin, _)| origin != Path::new(bundled_root));
            if registrations
                .archive_members
                .contains_key(registration.path)
            {
                registration.kind = SourceKind::DecodedFile;
            } else if let Some(kind) = registrations
                .physical_files
                .get(registration.path)
                .and_then(|file| file.source_kind)
            {
                registration.kind = kind;
            }
            registration
        })
        .collect();
    let (winners, file_alternatives) =
        selected_sources.split_at(ordinary_registrations.unwrap_or(selected_sources.len()));
    // A builtin initializer extends the native module and is not a file registration.
    // Type its checked callers against the native provider's public declaration.
    let mut builtin_declarations = Vec::new();
    if let Some(graph) = checked_graph {
        for (name, _, body) in &builtins {
            let Some(id) = graph.replaced_adapter(name, body) else {
                continue;
            };
            let provider = imre_ast::providers::builtin_finder_provider(
                name,
                || imre_ast::providers::is_frozen(name, || frozen_modules),
                || {
                    crate::zip_entry::packaged_member(name).is_some()
                        || sources
                            .iter()
                            .any(|(n, path, _)| n == name && path.starts_with(bundled_root))
                },
            );
            if !matches!(provider, Some(imre_ast::providers::Provider::Builtin))
                || winners
                    .iter()
                    .any(|s| s.name == name && s.declares_signatures)
            {
                continue;
            }
            let Some(source) = builtin_declaration(name).map_err(|error| error.to_string())? else {
                continue;
            };
            builtin_declarations.push((id, graph.modules[id].path.with_extension("pyi"), source));
        }
    }
    // Built-in SDK bodies have no file winner. Other origins, including ZIP
    // members, already have a selected source unit with its exact bytes.
    let mut provider_sources: HashMap<_, _> = checked_sdk_providers
        .into_iter()
        .filter_map(|(id, unit)| {
            let (name, path, body) = &sources[unit];
            (!entry_sources.contains_key(name)).then_some((id, (path.as_path(), body.as_ref())))
        })
        .collect();
    if let Some(graph) = checked_graph {
        for winner in winners {
            if let Some(id) = graph.replaced_adapter(winner.name, winner.source) {
                provider_sources.insert(id, (winner.path, winner.source));
            }
        }
    }
    for (id, path, source) in &builtin_declarations {
        provider_sources.insert(*id, (path.as_path(), source.as_str()));
    }
    // SDK members retain their loader-visible filenames while the ZIP is the build input.
    for path in inputs
        .files
        .iter()
        .flat_map(|(path, identity, _)| [cwd.join(path), identity.clone()])
    {
        registrations.physical_files.entry(path).or_default();
    }
    // file_code_from_bytes compares the original OS spelling and digest.
    let has_file_body = |filename: &str| {
        declared_open_paths.contains(std::ffi::OsStr::new(filename))
            || selected_sources.iter().any(|s| {
                s.path.as_os_str() == std::ffi::OsStr::new(filename)
                    && registrations.physical_files.contains_key(s.path)
                    && !module_errors.contains_key(s.path)
            })
    };
    let mut layouts = Vec::new();
    // Resolve the registered SDK files after the physical search roots in both checkers.
    roots.push(bundled_root.into());
    check(
        project.as_mut(),
        entry_requests_objects || startup_provider,
        Some(RegisteredSources {
            layouts: &mut layouts,
            root: &root,
            roots: &roots,
            winners,
            file_alternatives,
            provider_sources,
            has_file_body: sdk_runpy.then_some(&has_file_body),
        }),
    )?;
    // A namespace portion has no initializer. Its registered origin is a directory
    // that must remain available for the scratch session to add its child modules.
    let namespace_portions: HashSet<_> = inputs.directories.iter().map(|(path, _)| path).collect();
    let is_alternative = |path: &Path| {
        registrations.archive_members.contains_key(path)
            || registrations.physical_files.contains_key(path)
                && !winners.iter().any(|source| source.path == path)
    };
    let providers = options
        .bundled_provider_archive
        .as_deref()
        .filter(|_| keep_docstrings && output.is_some())
        .and_then(provider_archive::Providers::read)
        .unwrap_or_default();
    let matching_archive = (python_optimization == 0
        && options
            .target_os
            .is_none_or(|os| os == std::env::consts::OS))
    .then_some(&providers);
    for (unit, module) in sources.iter().chain(&builtins).enumerate() {
        let (name, path, source) = module;
        if namespace_portions.contains(path) {
            continue;
        }
        // A matching standalone provider has already passed the source-local syntax check.
        // A recorded loader failure still needs the diagnostic for its registration origin.
        if path.starts_with(bundled_root)
            && !module_errors.contains_key(path)
            && let Some(symbol) =
                matching_archive.and_then(|archive| archive.sealed(name, path, source))
        {
            shipped_checks.insert(unit, prepare::Shipped::Archived(symbol));
            continue;
        }
        let check_runtime = unit >= first_runtime_unit && output.is_none();
        let physical = registrations.physical_files.contains_key(path)
            || registrations.archive_members.contains_key(path);
        if let Some(parsed) = crate::analysis::check_native_syntax(
            &root,
            &mut syntax,
            (project.as_ref(), if physical { &layouts } else { &[] }),
            module,
            check_runtime,
            is_alternative,
            &mut module_errors,
        )? {
            let input = ParsedModule {
                text: Cow::Borrowed(source),
                parsed: Cow::Borrowed(&parsed),
            };
            let check = prepare::check_shipped(name, path, input.into(), python_optimization);
            shipped_checks.insert(unit, prepare::Shipped::Checked(check));
        }
    }
    drop(syntax);
    for name in &options.include_modules {
        // A finder can resolve a namespace or ZIP member without emitting a body.
        let file = winners
            .iter()
            .any(|source| source.name == name && !module_errors.contains_key(source.path));
        let bundled = bundled_includes
            .get(name)
            .is_some_and(|&frozen| frozen || !entry_sources.contains_key(name));
        if !file && !bundled {
            if let Some(error) = unavailable_sources.get(name) {
                return Err(error.to_string());
            }
            return Err(format!(
                "--include-module: no linkable Python source module found for {name:?}"
            ));
        }
    }
    if let Some(graph) = project.as_ref() {
        inputs
            .declaration_files
            .clone_from(&graph.inputs.declaration_files);
        inputs.file_checks.extend(graph.inputs.file_checks.clone());
        inputs
            .declared_file_resolutions
            .extend_from_slice(&graph.inputs.declared_file_resolutions);
    }
    // Nested search roots can register one identical input snapshot under multiple names.
    inputs.files.sort_unstable();
    inputs.files.dedup();
    inputs
        .declaration_files
        .retain(|file| inputs.files.binary_search(file).is_err());
    inputs.boxed_imports = true;
    // Registration has committed to the selected bodies. Leave the type database
    // and import graph behind before preparing their source initializers.
    if let Some(graph) = &project {
        registrations.functions = graph.take_registered_functions();
    }
    let modules = project
        .take()
        .map(|graph| graph.modules)
        .unwrap_or_default();
    registrations.modules = &modules;
    if let Some(file) = entry_file {
        registrations.checked_units.entry(0).or_default().source = Some(file);
    }
    let mut prepared = prepare::units(
        (&sources, &builtins),
        prepare::SourceInputs {
            entry_source: file_source,
            literal_sites,
            scanned_modules,
            shipped_checks,
        },
        python_optimization,
        Path::new(bundled_root),
        &module_errors,
        &registrations,
        layouts,
    );
    let Some(output) = output else {
        prepared.try_for_each(|unit| unit.map(drop))?;
        return Ok(Some(inputs));
    };
    // The bundle lives in the launcher. This absolute, non-directory path can neither
    // expose the working directory under -I nor collide with an explicit source root.
    let bundle_entry = startup
        .filter(|_| installed_layout)
        .map(|(_, executable)| Path::new(executable).join(bundled_root));
    initial.extend(bundle_entry.iter().cloned());
    let mut after_user_site = Vec::new();
    if let Some(user) = &options.user_site_pth
        && let Some(i) = options.site_dirs.iter().position(|p| p == user)
    {
        // A user's .pth files run before the following prefix site is visible.
        for site in installed_dirs {
            if options.site_dirs[i + 1..].contains(&site) {
                after_user_site.push(site);
            } else {
                initial.push(site);
            }
        }
    } else {
        initial.extend(installed_dirs);
    }
    // Command and module entries compute path[0] at startup, after site has run.
    // Preserve the native bytes when a script directory cannot be serialized as JSON text.
    let mut path0_bytes = None;
    if path0_len != 0 {
        let first = &mut initial[0];
        if in_memory && !options.path_entry && !bound_path0 {
            first.clear();
        } else if bound_path0 || first.to_str().is_none() {
            path0_bytes = Some(std::mem::take(first).into_os_string().into_encoded_bytes());
        }
    }
    // Report a source preparation error before a malformed startup path.
    let mut config_error = |error| prepared.find_map(Result::err).unwrap_or(error);
    let paths = serde_json::to_value(initial)
        .map_err(|e| config_error(format!("cannot represent startup search path: {e}")))?;
    let stdlib_fallback =
        serde_json::to_value(&options.stdlib_fallback).map_err(|e| config_error(e.to_string()))?;
    let config = serde_json::json!({
        "bundled_root":bundled_root,
        "stdlib_fallback":stdlib_fallback,
        "stdlib_members":crate::zip_entry::PACKAGED_MEMBERS,
        "sitecustomize":options.sitecustomize || options.stdlib_sitecustomize,"user_site_pth":options.user_site_pth,"after_user_site":after_user_site,"path0":path0_len != 0,
        "bundle_entry":bundle_entry,
        "python_version":crate::PYTHON_VERSION.to_string(),
        "python_hexversion":crate::version::hex(crate::version::components(false)),
        "implementation_version":env!("CARGO_PKG_VERSION"),
        "isolated":isolated,"safe_path":safe_path,"no_user_site":startup.is_some_and(|(flags, _)| flags.no_user_site),
        "no_site":startup.is_some_and(|(flags, _)| flags.no_site),
        "ignore_environment":startup.is_some_and(|(flags, _)| flags.ignore_environment),
        "context_aware_warnings":startup.map(|(flags, _)| flags.context_aware_warnings),
        "cpu_count":startup.map(|(flags, _)| flags.cpu_count),
        "utf8_mode":startup.and_then(|(flags, _)| flags.utf8_mode),
        "warnoptions":startup.map(|(flags, _)| &flags.warnoptions),
        "xoptions":startup.map(|(flags, _)| &flags.xoptions),
        "frozen_modules":frozen_modules,
        "faulthandler":startup.map(|(flags, _)| flags.faulthandler),
        "tracemalloc":startup.map_or(0, |(flags, _)| flags.tracemalloc),
        "unbuffered":startup.is_some_and(|(flags, _)| flags.unbuffered),
        "warn_default_encoding":startup.map(|(flags, _)| flags.warn_default_encoding),
        "optimize":python_optimization,
        "dont_write_bytecode":startup.is_some_and(|(flags, _)| flags.dont_write_bytecode),
        "executable":startup.map(|(_, executable)| executable),
        "command_source":command,"path0_bytes":path0_bytes,"packaged_file_module":packaged_file_module,
        "command":in_memory,"module_entry":module_entry,"path_entry":options.path_entry,"paths":paths,
    });
    let used_archive = output.emit_registered(
        prepared,
        module_body
            .filter(|(id, _, _)| bound_path0 && !registrations.python_providers.contains(id))
            .map(|(id, _, _)| id),
        &config.to_string(),
        python_optimization,
        &providers.symbols,
    )?;
    output.compilation.extension_archives.extend(
        options
            .bundled_provider_archive
            .iter()
            .filter(|_| used_archive)
            .cloned(),
    );
    output.compilation.input_paths = inputs.protected_paths(None);
    Ok(Some(inputs))
}

/// The full threading factory needs these Python dependencies before the startup fallbacks.
fn source_threading(mut registered: impl FnMut(&str) -> bool) -> Option<&'static str> {
    ["_weakrefset", "collections", "warnings"]
        .into_iter()
        .all(&mut registered)
        .then_some("threading")
}

/// Add bundled imports that can be selected without a static import statement.
fn add_late_bundled_imports(
    imports: &mut Imports,
    seen: &HashSet<String>,
    uses_import_builtin: bool,
    uses_exec: bool,
    computed_import: bool,
    user_site_pth: bool,
) {
    // A runtime-selected HTTP child needs its server and transitive providers.
    if uses_import_builtin && seen.contains("http") && !seen.contains("http.server") {
        imports.names.push((0, "http.server".to_owned()));
    }
    // Late imports can select children of a bundled package without naming them in source.
    if uses_exec || uses_import_builtin || seen.contains("importlib") {
        for name in ["lzma", "compression.zstd"] {
            if !seen.contains(name) {
                imports.add(0, name);
            }
        }
        for (parent, child) in [
            ("email", "email._header_value_parser"),
            ("logging", "logging.config"),
            ("urllib", "urllib.robotparser"),
        ] {
            if seen.contains(parent) && !seen.contains(child) {
                imports.add(0, child);
            }
        }
    }
    if computed_import && !seen.contains("xml.parsers.expat") {
        imports.names.push((0, "xml.parsers.expat".to_owned()));
    }
    // Register the mock provider for imports from source-loaded modules.
    if uses_import_builtin && seen.contains("unittest") && !seen.contains("unittest.mock") {
        imports.add(0, "unittest.mock");
    }
    if uses_import_builtin || seen.contains("importlib") {
        for name in [
            "_pyio",
            "colorsys",
            "configparser",
            "optparse",
            "py_compile",
            "sysconfig",
            "uuid",
        ] {
            if !seen.contains(name) {
                imports.names.push((0, name.to_owned()));
            }
        }
    }
    // The runtime's site module calls os.path even when the entry never imports it.
    if (user_site_pth || seen.contains("site")) && !seen.contains("posixpath") {
        imports.add(0, "posixpath");
    }
    // Native time and datetime parsers import their Python helper on first use.
    if (seen.contains("time") || seen.contains("_datetime") || computed_import)
        && !seen.contains("_strptime")
    {
        imports.names.push((0, "_strptime".to_owned()));
    }
    // Python 3.13 path pickles name this module even when the reader never imports pathlib.
    if (seen.contains("pickle") || seen.contains("pathlib")) && !seen.contains("pathlib._local") {
        imports.names.push((0, "pathlib._local".to_owned()));
    }
    if seen.contains("pathlib") && !seen.contains("pathlib.types") {
        imports.names.push((0, "pathlib.types".to_owned()));
    }
}

/// Python registrations provided by the launcher even without a direct source import.
fn runtime_sources(
    seen: impl Fn(&str) -> bool,
    threading: Option<&'static str>,
) -> impl Iterator<Item = (&'static str, &'static str, &'static str)> {
    let collections = (!seen("collections")).then_some((
        "collections",
        "collections/__init__.py",
        COLLECTIONS_BOOTSTRAP,
    ));
    collections
        .into_iter()
        .chain([("io", "io.py", SEALED_IO)])
        // Lookup can select this compiled codec in a checked caller. Its Python
        // implementation is a runtime registration, with no checked class surface.
        .chain((!seen("encodings.big5")).then_some((
            "encodings.big5",
            "encodings/big5.py",
            include_str!("../../stdlib/encodings/multibyte.py"),
        )))
        .chain(
            RUNTIME_PROVIDERS
                .iter()
                .copied()
                .chain(FIXED_UTF_PROVIDERS.iter().map(|(name, _)| *name))
                .chain(threading)
                .filter(move |name| !seen(name))
                .filter_map(|name| {
                    let (file, source) = bundled_module_source(name, false)?;
                    Some((name, file, source))
                }),
        )
}

/// Carry the checked program's Python source registrations to the runtime finder classifier.
pub(crate) fn builtin_inventory_sources(graph: &crate::modules::SourceGraph) -> String {
    let threading =
        source_threading(|name| graph.modules.iter().any(|m| m.bundled && m.name == name));
    let defaults = runtime_sources(
        |name| graph.modules.iter().any(|m| m.name == name),
        threading,
    );
    let mut names: Vec<_> = graph
        .modules
        .iter()
        .filter(|m| m.bundled)
        .map(|m| m.name.as_str())
        .chain(defaults.map(|(name, _, _)| name))
        .filter(|name| imre_ast::providers::provider(name).is_some())
        .collect();
    for name in imre_ast::providers::builtin_module_candidates() {
        if crate::zip_entry::packaged_member(name).is_some() {
            names.push(name);
        }
    }
    names.sort_unstable();
    names.dedup();
    names.join("\0")
}

/// Only a published declaration displaces the checked adapter's existing contracts.
fn builtin_declaration(name: &str) -> std::io::Result<Option<String>> {
    let stub = format!("stdlib/{}.pyi", name.replace('.', "/"));
    match ty_vendored::file_system().read_to_string(stub.as_str()) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        result => result.map(Some),
    }
}

// A builtin may create native bindings before executing a fixed Python body in that scope.
fn builtin_sources(
    target_os: &str,
) -> impl IntoIterator<Item = (&'static str, &'static str, &'static str)> {
    let selectors = if target_os == "linux" {
        concat!(
            include_str!("../../../imre-runtime/src/dynamic/stdlib/selectors_base.py"),
            "\n",
            include_str!("../../../imre-runtime/src/dynamic/stdlib/selectors_epoll.py")
        )
    } else {
        include_str!("../../../imre-runtime/src/dynamic/stdlib/selectors_base.py")
    };
    [
        ("selectors", "selectors.py", selectors),
        (
            "random",
            "random.py",
            include_str!("../../../imre-runtime/src/dynamic/stdlib/random/random.py"),
        ),
        (
            "site",
            "site.py",
            include_str!("../../../imre-runtime/src/dynamic/stdlib/site.py"),
        ),
    ]
}

const COLLECTIONS_BOOTSTRAP: &str = include_str!("../../stdlib/collections/bootstrap.py");
const SEALED_IO: &str = include_str!("../../stdlib/io.py");
const FIXED_UTF_PROVIDERS: &[(&str, &str)] = &[
    ("encodings.utf_16_le", "encodings/utf_16_le.py"),
    ("encodings.utf_16_be", "encodings/utf_16_be.py"),
    ("encodings.utf_32_le", "encodings/utf_32_le.py"),
    ("encodings.utf_32_be", "encodings/utf_32_be.py"),
];
const RUNTIME_PROVIDERS: &[&str] = &[
    "__future__",
    "__hello__",
    "_collections_abc",
    "_frozen_importlib",
    "_frozen_importlib_external",
    "_opcode_metadata",
    "_py_warnings",
    "_tokenize",
    "abc",
    "annotationlib",
    "ast",
    "code",
    "codecs",
    "codeop",
    "copyreg",
    "datetime",
    "decimal",
    "encodings",
    "encodings.ascii",
    "encodings.latin_1",
    "encodings.utf_8",
    "encodings.utf_8_sig",
    "encodings.utf_16",
    "encodings.utf_32",
    "encodings.cp1251",
    "encodings.cp1252",
    "encodings.cp424",
    "encodings.cp437",
    "encodings.iso8859_2",
    "encodings.iso8859_15",
    "encodings.koi8_r",
    "encodings.koi8_u",
    "encodings.idna",
    "encodings.euc_jp",
    "encodings.shift_jis",
    "enum",
    "fractions",
    "functools",
    "genericpath",
    "bz2",
    "gzip",
    "hashlib",
    "itertools",
    "json",
    "json.decoder",
    "json.encoder",
    "json.scanner",
    "linecache",
    "numbers",
    "opcode",
    "platform",
    "re",
    "re._constants",
    "signal",
    "ssl",
    "stat",
    "string",
    "string.templatelib",
    "subprocess",
    "token",
    "tomllib",
    "tomllib._parser",
    "tomllib._re",
    "tomllib._types",
    "tracemalloc",
    "types",
    "warnings",
    "zipimport",
];

#[test]
fn a_shipped_getter_keeps_its_body_and_executable_source_offsets() {
    let crlf = "def __getattr__(name):\r\n    import fractions\r\n    return fractions\r\n";
    let normalized = crlf.replace("\r\n", "\n");
    let mut getter = Imports::default();
    let (imports, module) = scan_imports_for_preparation(
        crlf,
        SourceKind::File,
        None,
        Some(&mut getter),
        Some(&normalized),
    )
    .unwrap();
    assert!(imports.names.is_empty());
    assert_eq!(getter.names, [(0, "fractions".to_owned())]);
    let (_, lower) = module.unwrap();
    let Some(LoweredRoot::Complete(body, _)) = lower else {
        panic!("expected checked syntax");
    };
    let [Stmt::FunctionDef(function)] = body.as_slice() else {
        panic!("expected the module's lazy getter");
    };
    let [Stmt::Import(_), Stmt::Return(ret)] = function.body.as_slice() else {
        panic!("expected the import and return inside the getter");
    };
    assert_eq!(&normalized[ret.range()], "return fractions");

    let (_, module) =
        scan_imports_for_preparation(crlf, SourceKind::File, None, None, Some(crlf)).unwrap();
    assert!(module.is_none());
}

#[test]
fn shipped_private_parameter_requires_fresh_lowering() {
    let names = |source| {
        scan_imports(source, SourceKind::File, None, None, true).unwrap().1
    };
    assert!(names("class C:\n    def f(self, arg): pass\n").is_some());
    assert!(names("class C:\n    def f(self, __arg): pass\n").is_none());
    assert!(names("class C:\r\n    pass\r\n").is_none());
}

#[test]
fn a_native_builtin_need_not_have_a_typeshed_declaration() {
    assert!(builtin_declaration("selectors").unwrap().is_some());
    assert!(builtin_declaration("_typing").unwrap().is_none());
}

#[cfg(all(test, unix))]
#[test]
fn declared_open_path_tracks_the_identity_after_lexical_normalization() {
    use std::fs;
    use std::os::unix::fs::symlink;

    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let child = root.join("package/child");
    fs::create_dir_all(&child).unwrap();
    let target = root.join("package/report.py");
    fs::write(&target, "answer = 1").unwrap();
    let identity = target.canonicalize().unwrap();
    let mut filesystem = crate::watch::FileSystem::default();
    let mut inputs = SourceInputs::default();
    assert_eq!(
        declared_open_path(
            &child.join("../report.py"),
            &identity,
            &mut inputs,
            &mut filesystem
        )
        .unwrap(),
        Some(target.clone())
    );

    symlink(&child, root.join("link")).unwrap();
    let origin = root.join("link/../report.py");
    let opened = root.join("report.py");
    symlink(&target, &opened).unwrap();
    assert_eq!(
        declared_open_path(&origin, &identity, &mut inputs, &mut filesystem).unwrap(),
        Some(opened.clone())
    );
    assert!(inputs.fresh());
    fs::remove_file(&opened).unwrap();
    fs::write(&opened, "answer = 2").unwrap();
    assert!(!inputs.fresh());
    assert_eq!(origin.canonicalize().unwrap(), identity);
    let mut changed = SourceInputs::default();
    assert_eq!(
        declared_open_path(&origin, &identity, &mut changed, &mut filesystem).unwrap(),
        None
    );
    assert!(changed.fresh());
    fs::remove_file(&opened).unwrap();
    symlink(&target, opened).unwrap();
    assert!(!changed.fresh());
}
