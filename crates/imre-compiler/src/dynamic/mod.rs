//! Collects Python source modules from explicit startup search paths for native lowering.

mod emit;
mod import_module;
mod provider_archive;
pub(crate) mod provider_pack;

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};

use ruff_python_ast::name::UnqualifiedName;
use ruff_python_ast::visitor::{self, Visitor};
use ruff_python_ast::{self as ast, Expr, Stmt};

use crate::{Compilation, CompileOptions, SourceInputs, SourceKind};

/// Finds literal directories made available to imports by a startup script.
/// The generated program still evaluates the assignment and consults its actual
/// `sys.path` in order when each import executes.
#[derive(Default)]
struct Imports {
    names: Vec<(u32, String)>,
    explicit_roots: Vec<PathBuf>,
    runtime_imports: bool,
    private_names: imre_ast::Names,
    uses_import_builtin: bool,
    uses_exec: bool,
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
    typed_machinery_receivers: HashSet<String>,
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

// Ordinary files keep their typed ModuleSpec diagnostics unless the graph
// requests the source importer. Explicit runtime imports can use the boxed class.
fn typed_machinery_export(name: &str) -> bool {
    name == "ModuleSpec"
}

impl Imports {
    fn needs_typed_machinery(&self) -> bool {
        !self
            .machinery_names
            .is_disjoint(&self.typed_machinery_receivers)
            || self.names.iter().any(|(level, name)| {
                *level == 0
                    && name
                        .strip_prefix("importlib.machinery.")
                        .is_some_and(typed_machinery_export)
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
                    | "keyword"
                    | "site"
                    | "importlib.abc"
                    | "importlib.machinery"
                    | "tomllib"
                    | "tomllib._parser"
                    | "tomllib._re"
                    | "tomllib._types"
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
    matches!(
        name,
        "path"
            | "meta_path"
            | "path_hooks"
            | "path_importer_cache"
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
        ($($function:literal => $operation:expr),* $(,)?) => { &[$($function),*] };
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
        // A literal __import__ call cannot select a different provider at execution.
        if let Expr::Call(call) = e
            && matches!(&*call.func, Expr::Name(name) if name.id == "__import__")
            && let Some(Expr::StringLiteral(name)) = call.arguments.find_argument_value("name", 0)
        {
            self.uses_import_builtin = true;
            self.add(0, "importlib");
            if name.value.to_str().split('.').next() == Some("zoneinfo") {
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
        if let Expr::Attribute(a) = e
            && typed_machinery_export(a.attr.as_str())
            && let Some(receiver) = UnqualifiedName::from_expr(&a.value)
        {
            self.typed_machinery_receivers.insert(receiver.to_string());
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

/// A synchronous function with one body scope can capture its unbounded type parameters.
pub(crate) fn function_type_params<'a>(
    function: &'a ast::StmtFunctionDef,
    string_annotations: bool,
    nested: bool,
    private_names: Option<&imre_ast::Names>,
) -> Option<&'a ast::TypeParams> {
    use ast::AnyNodeRef::*;
    use ast::visitor::source_order::{SourceOrderVisitor, TraversalSignal};

    let params = function.type_params.as_deref()?;
    let mut names = HashSet::new();
    if function.is_async
        || (nested && !string_annotations)
        || !params.iter().all(|p| {
            let name = private_names.map_or(p.name().as_str(), |names| names.definition(p.name()));
            p.default().is_none()
                && match p {
                    ast::TypeParam::TypeVar(v) => v.bound.is_none(),
                    ast::TypeParam::TypeVarTuple(_) | ast::TypeParam::ParamSpec(_) => {
                        string_annotations
                    }
                }
                && !matches!(name, "__classdict__" | "__debug__")
                && (!nested || !name.starts_with("__"))
                && names.insert(p.name().as_str())
        })
    {
        return None;
    }

    struct BodyScope {
        nested: bool,
        unsupported: bool,
    }
    impl<'a> SourceOrderVisitor<'a> for BodyScope {
        fn enter_node(&mut self, node: ast::AnyNodeRef<'a>) -> TraversalSignal {
            self.unsupported |= matches!(
                node,
                StmtFunctionDef(_)
                    | StmtClassDef(_)
                    | StmtTypeAlias(_)
                    | StmtNonlocal(_)
                    | ExprLambda(_)
                    | ExprGenerator(_)
                    | ExprListComp(_)
                    | ExprSetComp(_)
                    | ExprDictComp(_)
                    | ExprYield(_)
                    | ExprYieldFrom(_)
            );
            // Nested declarations with a class cell need a different capture path.
            self.unsupported |= self.nested
                && matches!(node, ExprName(n) if matches!(n.id.as_str(), "__class__" | "super"));
            if self.unsupported {
                TraversalSignal::Skip
            } else {
                TraversalSignal::Traverse
            }
        }
    }
    let mut body = BodyScope {
        nested,
        unsupported: false,
    };
    body.visit_body(&function.body);
    (!body.unsupported).then_some(params)
}

fn scan_imports(
    source: &str,
    kind: SourceKind,
    getter: Option<&mut Imports>,
) -> Result<Imports, String> {
    let normalized = crate::normalize_source(source, kind)?;
    crate::analysis::check_source_complexity(&normalized).map_err(str::to_owned)?;
    let parsed = ruff_python_parser::parse_module(&normalized).map_err(|e| e.to_string())?;
    crate::analysis::check_indentation(&normalized, parsed.tokens())?;
    let mut imports = Imports::default();
    let mut body = parsed.into_syntax().body;
    imports.private_names = imre_ast::lower(&mut body);
    imports.runtime_imports = body.iter().any(|stmt| {
        matches!(stmt, Stmt::FunctionDef(function) if function_type_params(function, imports.private_names.string_annotations, false, Some(&imports.private_names)).is_some())
    });
    let mut getter_body = Vec::new();
    if getter.is_some() {
        for stmt in &mut body {
            if let Stmt::FunctionDef(function) = stmt
                && function.name.as_str() == "__getattr__"
            {
                getter_body.extend(std::mem::take(&mut function.body));
            }
        }
    }
    imports.visit_body(&body);
    if let Some(getter) = getter
        && !getter_body.is_empty()
    {
        // sys.path and pkgutil.get_data can add source hints through module aliases.
        getter.sys_names = std::mem::take(&mut imports.sys_names);
        getter.pkgutil_names = std::mem::take(&mut imports.pkgutil_names);
        getter.get_data_names = std::mem::take(&mut imports.get_data_names);
        getter.machinery_names.clone_from(&imports.machinery_names);
        getter.visit_body(&getter_body);
    }
    Ok(imports)
}

fn resolve(owner: &str, package: bool, level: u32, name: &str) -> Option<String> {
    if level == 0 {
        return Some(name.to_owned());
    }
    let mut parts: Vec<_> = owner.split('.').collect();
    if !package {
        parts.pop();
    }
    if level as usize > parts.len() {
        return None;
    }
    parts.truncate(parts.len() + 1 - level as usize);
    if !name.is_empty() {
        parts.push(name);
    }
    Some(parts.join("."))
}

pub(crate) enum ObjectOperation {
    TracedEntry,
    Builtin,
    OrderedCommandImport,
    ClassLayout,
}

pub(crate) fn compile(
    path: &Path,
    module_root: Option<&Path>,
    options: &CompileOptions,
    runtime_imports: bool,
    operation: Option<ObjectOperation>,
    typed_graph: Option<&crate::modules::SourceGraph>,
    check_returns: impl FnOnce() -> Result<(), String>,
) -> Result<Option<Compilation>, String> {
    let invocation = options.command.as_ref();
    let startup = options.python_startup();
    let command = invocation.map(|i| &i.source);
    let module_entry = options.module_entry.as_deref();
    let in_memory = command.is_some() || module_entry.is_some();
    let src = if module_entry.is_some() {
        String::new()
    } else if let Some(command) = command {
        command.to_owned()
    } else {
        // A direct .pyc operand already received its bytecode diagnostic from the script loader.
        if options.script_startup.is_some() && crate::named_bytecode(path) {
            return Ok(None);
        }
        let Ok(source) = std::fs::read_to_string(path) else {
            return Ok(None);
        };
        if options.script_startup.is_some()
            && crate::bytecode_script_error(path, source.as_bytes()).is_some()
        {
            return Ok(None);
        }
        source
    };
    // An ordinary entry can import a package that uses the object runtime without
    // accessing sys.path or installation metadata in the entry itself.
    let object_hint = runtime_imports
        || startup.is_some()
            && (src.contains("urllib") || src.contains("ftplib") || src.contains("argparse"))
        || src.contains("class") && src.contains('=')
        || cfg!(unix) && src.contains("signal")
        || [
            "__future__",
            "hashlib",
            "ssl",
            "_imp",
            "atexit",
            "faulthandler",
            "code",
            "copyreg",
            "weakref",
            "pathlib",
            "sys",
            "platform",
            "keyword",
            "site",
            "importlib",
            "runpy",
            "json",
            "tomllib",
            "zoneinfo",
            "opcode",
            "operator",
            "struct",
            "traceback",
            "unittest",
            "SystemExit",
            "__import__",
            "__reduce",
            "__getstate__",
            "__getinitargs__",
            "templatelib",
        ]
        .iter()
        .any(|name| src.contains(name));
    // Python normalizes non-ASCII identifiers, including imported module names.
    let provider_hint = startup.is_some()
        && !options.runtime_imports
        && (src.contains("logging")
            || src.contains("tokenize")
            || src.contains("configparser")
            || src.contains("_pyio")
            || !src.is_ascii());
    let ordered_command = matches!(operation, Some(ObjectOperation::OrderedCommandImport));
    if !object_hint && !provider_hint && !ordered_command {
        return Ok(None);
    }
    let Ok(mut first) = scan_imports(
        &src,
        if command.is_some() {
            SourceKind::Command
        } else {
            SourceKind::File
        },
        None,
    ) else {
        return Ok(None);
    };
    // Python entries can import these bundled sources when static lowering cannot handle the entry.
    let imports_provider = |provider: &str| {
        startup.is_some()
            && !options.runtime_imports
            && first
                .names
                .iter()
                .any(|(level, name)| *level == 0 && name == provider)
    };
    let request_script = imports_provider("urllib.request");
    let ftp_script = imports_provider("ftplib");
    let argparse_script = imports_provider("argparse");
    let provider_script = request_script || ftp_script || argparse_script;
    let object_entry = object_hint
        && (module_entry.is_some()
            || first.runtime_imports
            || (operation.is_some() && !ordered_command)
            || (runtime_imports && !first.names.is_empty())
            || (first.uses_import_builtin && (startup.is_some() || options.runtime_imports))
            || provider_script);
    // Existing object imports keep their normal project-module lookup. Only a new
    // startup fallback needs to verify that the imported provider is bundled.
    let logging_script = !object_entry && provider_hint && first.imports_logging;
    let tokenize_script = !object_entry && imports_provider("tokenize");
    let configparser_script = !object_entry && imports_provider("configparser");
    let pyio_script = !object_entry && imports_provider("_pyio");
    let fallback_script = logging_script || tokenize_script || configparser_script || pyio_script;
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
    if let Some(name) = module_entry {
        first.add(0, "runpy");
        // Register source candidates without interpreting a module operand as a filesystem path.
        // runpy and the normal finder decide which one can actually execute.
        if name.split('.').all(|part| {
            !part.is_empty() && Path::new(part).file_name().is_some_and(|file| file == part)
        }) {
            first.add(0, name);
            first.add(0, &format!("{name}.__main__"));
        }
    }
    let registered_imports = typed_graph
        .filter(|_| runtime_imports)
        .map(import_module::registered_imports)
        .unwrap_or_default();
    first
        .names
        .extend(registered_imports.targets.into_iter().map(|name| (0, name)));
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    let isolated = startup.is_some_and(|(flags, _)| flags.isolated);
    let entry = cwd.join(path);
    let entry_identity = (!in_memory)
        .then(|| entry.canonicalize())
        .transpose()
        .map_err(|e| e.to_string())?;
    let root = if let Some(root) = module_root {
        cwd.join(root)
    } else if entry_identity.is_some() {
        entry.parent().unwrap_or(&cwd).to_owned()
    } else {
        cwd.clone()
    };
    // POSIX starts a script's search path at its physical directory. Keep a
    // usable UTF-8 alias when the physical name is outside our string domain.
    let physical_root = match (cfg!(unix), &entry_identity, module_root) {
        (true, Some(_), Some(_)) => root.canonicalize().ok(),
        (true, Some(identity), None) => identity.parent().map(Path::to_owned),
        _ => None,
    };
    let root = physical_root
        .filter(|path| path.to_str().is_some())
        .unwrap_or(root);
    let mut initial = Vec::new();
    let safe_path = startup.is_some_and(|(flags, _)| flags.safe_path);
    if options.path_entry {
        // CPython preserves the directory spelling, including symlinks, and searches
        // an explicitly requested entry even when isolated startup omits the cwd.
        initial.push(entry.clone());
    } else if !safe_path {
        initial.push(root);
    }
    initial.extend(options.extension_paths.iter().map(|p| cwd.join(p)));
    let site = options
        .system_site
        .as_ref()
        .filter(|p| !initial.contains(p));
    let mut roots = initial.clone();
    roots.extend(first.explicit_roots.iter().map(|p| cwd.join(p)));
    let implicit_site = site.filter(|p| !roots.contains(p));
    roots.extend(implicit_site.cloned());
    // Discover each search directory once even when it appears twice in sys.path.
    let mut seen_roots = HashSet::new();
    roots.retain(|root| seen_roots.insert(root.clone()));
    let bundled_root = "<imre stdlib>";
    let mut inputs = SourceInputs::default();
    let mut archive = if options.path_entry && !entry.is_dir() {
        Some(crate::zip_entry::ZipSources::open(&entry, &mut inputs)?)
    } else {
        None
    };
    let mut sources = vec![(
        "__main__".to_owned(),
        if in_memory {
            PathBuf::from("<string>")
        } else {
            entry
        },
        src.clone(),
    )];
    if let Some(identity) = entry_identity {
        inputs.files.push((
            path.to_owned(),
            identity,
            blake3::hash(src.as_bytes()).to_hex().to_string(),
        ));
    }
    let mut seen = HashSet::new();
    // Keep source hints on the winning regular package's path. The runtime finder
    // can still load a different child if a parent changes its __path__.
    let mut entry_sources: HashMap<String, Option<PathBuf>> = HashMap::new();
    let mut before_site = HashSet::new();
    let mut frozen_imports = Imports::default();
    let mut work = VecDeque::from([("__main__".to_owned(), false, first)]);
    let mut uses_import_builtin = false;
    let mut uses_exec = false;
    let mut computed_import = registered_imports.computed;
    let mut bundled_only = false;
    let mut getters = VecDeque::new();
    let mut through_getter = false;
    loop {
        let (owner, package, imports) = if let Some(next) = work.pop_front() {
            next
        } else if let Some(next) = getters.pop_front() {
            // Keep project shadows for a bundled module's lazy attributes. The
            // ordinary graph has already checked any modules the user imported.
            through_getter = true;
            next
        } else {
            through_getter = false;
            // Collect late imports and their bundled dependencies after the explicit
            // file graph, without parsing unrelated modules in project directories.
            let mut imports = Imports::default();
            if uses_import_builtin && !seen.contains("pprint") {
                imports.names.push((0, "pprint".to_owned()));
            }
            if computed_import && !seen.contains("zoneinfo") {
                imports.names.push((0, "zoneinfo".to_owned()));
            }
            // A runtime-selected HTTP child needs its server and transitive providers.
            if uses_import_builtin && seen.contains("http") && !seen.contains("http.server") {
                imports.names.push((0, "http.server".to_owned()));
            }
            // Computed imports can select a urllib child without naming it in source.
            if (uses_exec || uses_import_builtin || seen.contains("importlib"))
                && seen.contains("urllib")
                && !seen.contains("urllib.robotparser")
            {
                imports.add(0, "urllib.robotparser");
            }
            // Register the mock provider for imports from source-loaded modules.
            if uses_import_builtin && seen.contains("unittest") && !seen.contains("unittest.mock") {
                imports.add(0, "unittest.mock");
            }
            if uses_import_builtin || seen.contains("importlib") {
                for name in ["_pyio", "configparser", "py_compile", "sysconfig", "uuid"] {
                    if !seen.contains(name) {
                        imports.names.push((0, name.to_owned()));
                    }
                }
            }
            // The native time.strptime classmethod imports its Python parser on first use.
            if seen.contains("_datetime") && !seen.contains("_strptime") {
                imports.names.push((0, "_strptime".to_owned()));
            }
            // Python 3.13 path pickles name this module even when the reader never imports pathlib.
            if (seen.contains("pickle") || seen.contains("pathlib"))
                && !seen.contains("pathlib._local")
            {
                imports.names.push((0, "pathlib._local".to_owned()));
            }
            if seen.contains("pathlib") && !seen.contains("pathlib.types") {
                imports.names.push((0, "pathlib.types".to_owned()));
            }
            if imports.names.is_empty() {
                break;
            }
            bundled_only = true;
            (String::new(), false, imports)
        };
        // A boxed provider can discover further site modules while collecting dependencies.
        // Keep checked machinery diagnostics on the modules in the original program.
        let machinery_graph = typed_graph.filter(|_| {
            matches!(operation, Some(ObjectOperation::ClassLayout))
                || seen.contains("zoneinfo")
                || computed_import
        });
        if module_entry.is_none()
            && !options.runtime_imports
            && !registered_imports.called
            && (!provider_script || owner == "__main__")
            && !through_getter
            && imports.needs_typed_machinery()
            && !matches!(operation, Some(ObjectOperation::TracedEntry))
            && (owner == "__main__"
                || machinery_graph
                    .is_none_or(|graph| graph.modules.iter().any(|m| m.name == owner)))
        {
            return Ok(None);
        }
        uses_exec |= imports.uses_exec;
        computed_import |= imports.computed_import;
        let mut names = imports.names;
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
        // The native string scanner imports the decoder's exception on a syntax error.
        if names
            .iter()
            .any(|(level, name)| *level == 0 && name == "_json")
        {
            names.push((0, "json.decoder".to_owned()));
        }
        for (level, requested) in names {
            let Some(name) = resolve(&owner, package, level, &requested) else {
                continue;
            };
            // A frozen import never visits sys.path unless the program can disable
            // the frozen importer. Defer collecting its filesystem alternatives.
            if !seen.contains("_imp")
                && let Some(top @ ("stat" | "__hello__")) = name.split('.').next()
                && crate::modules::bundled_runtime_source(top).is_some()
            {
                frozen_imports.names.push((0, name));
                continue;
            }
            let mut prefix = String::new();
            for part in name.split('.').filter(|s| !s.is_empty()) {
                if !prefix.is_empty() {
                    prefix.push('.');
                }
                prefix.push_str(part);
                if !seen.insert(prefix.clone()) {
                    continue;
                }
                if prefix == "_imp" && !frozen_imports.names.is_empty() {
                    work.push_back((String::new(), false, std::mem::take(&mut frozen_imports)));
                }
                let bundled = bundled_module_source(&prefix);
                if implicit_site.is_some()
                    && (bundled.is_some()
                        || imre_ast::providers::provider(&prefix).is_some()
                        || prefix
                            .rsplit_once('.')
                            .is_some_and(|(parent, _)| before_site.contains(parent)))
                {
                    before_site.insert(prefix.clone());
                }
                if let Some((filename, source)) = bundled {
                    let path = Path::new(bundled_root).join(filename);
                    let package = path.ends_with("__init__.py");
                    let mut getter = Imports::default();
                    let mut imports = scan_imports(&source, SourceKind::File, Some(&mut getter))
                        .map_err(|e| format!("{}: {e}", path.display()))?;
                    // unittest's loader imports application modules by name, including during discovery.
                    // Other bundled imports are implementation details of a known provider.
                    if prefix != "unittest.loader" {
                        imports.computed_import = false;
                        getter.computed_import = false;
                    }
                    if !getter.names.is_empty() || getter.uses_exec {
                        getters.push_back((prefix.clone(), package, getter));
                    }
                    sources.push((prefix.clone(), path, source.into_owned()));
                    work.push_back((prefix.clone(), package, imports));
                }
                if bundled_only {
                    continue;
                }
                // The interpreter bootstraps runpy from its frozen copy before executing the target.
                if prefix == "runpy" {
                    continue;
                }
                for root in &roots {
                    if implicit_site == Some(root) && before_site.contains(&prefix) {
                        continue;
                    }
                    if let Some((parent, _)) = prefix.rsplit_once('.')
                        && let Some(selected) = entry_sources.get(parent)
                        && selected.as_ref() != Some(root)
                    {
                        continue;
                    }
                    if let Some(archive) = &mut archive
                        && initial.first() == Some(root)
                    {
                        if let Some((member, source, package)) = archive.module(&prefix) {
                            if let Ok(imports) =
                                imports_for_module(&member, source.trim_start_matches('\u{feff}'))
                            {
                                work.push_back((prefix.clone(), package, imports));
                            }
                            if implicit_site.is_some() {
                                before_site.insert(prefix.clone());
                            }
                            entry_sources.insert(prefix.clone(), package.then(|| root.clone()));
                            break;
                        }
                        continue;
                    }
                    let stem = root.join(prefix.replace('.', "/"));
                    let file = stem.with_extension("py");
                    let init = stem.join("__init__.py");
                    let (path, package) = if init.is_file() {
                        (init, true)
                    } else if file.is_file() {
                        inputs.missing.push(init);
                        (file, false)
                    } else {
                        inputs.missing.extend([init, file]);
                        if stem.is_dir() {
                            if ordered_only {
                                return Ok(None);
                            }
                            inputs.directories.push((
                                stem.clone(),
                                stem.canonicalize().map_err(|e| e.to_string())?,
                            ));
                            sources.push((prefix.clone(), stem, String::new()));
                        } else {
                            inputs.missing.push(stem);
                        }
                        continue;
                    };
                    if implicit_site.is_some() && implicit_site != Some(root) {
                        before_site.insert(prefix.clone());
                    }
                    // A late command import can use the runtime's builtins and bundled
                    // providers. Leave project-source imports on their original route.
                    if ordered_only {
                        return Ok(None);
                    }
                    // Bundled providers cannot stand in for earlier project modules.
                    if initial.contains(root)
                        && ((request_script && prefix == "urllib")
                            || (ftp_script && prefix == "ftplib")
                            || (argparse_script && prefix == "argparse")
                            || (logging_script && prefix == "logging")
                            || (tokenize_script && prefix == "tokenize")
                            || (configparser_script && prefix == "configparser")
                            || (pyio_script && prefix == "_pyio"))
                    {
                        return Ok(None);
                    }
                    let src = std::fs::read_to_string(&path)
                        .map_err(|e| format!("{}: {e}", path.display()))?;
                    let imports = imports_for_module(&path, &src)?;
                    inputs.files.push((
                        path.clone(),
                        path.canonicalize().map_err(|e| e.to_string())?,
                        blake3::hash(src.as_bytes()).to_hex().to_string(),
                    ));
                    sources.push((prefix.clone(), path, src));
                    work.push_back((prefix.clone(), package, imports));
                    if module_entry.is_some() {
                        entry_sources.insert(prefix.clone(), package.then(|| root.clone()));
                        break;
                    }
                }
            }
        }
    }
    // Cold imports can use the canonical threading factory when its Python dependencies
    // are already registered. Check before adding the minimal collections fallback below.
    let threading = ["_weakrefset", "collections", "warnings"]
        .into_iter()
        .all(|required| {
            sources
                .iter()
                .any(|(name, path, _)| name == required && path.starts_with(bundled_root))
        })
        .then_some("threading");
    // Native modules can construct named tuples even without a source-level collections import.
    if !seen.contains("collections") {
        sources.push((
            "collections".into(),
            Path::new(bundled_root).join("collections/__init__.py"),
            NAMEDTUPLE_COLLECTIONS.into(),
        ));
    }
    // The sealed io module also needs its Python protocols when imported indirectly.
    sources.push((
        "io".into(),
        Path::new(bundled_root).join("io.py"),
        SEALED_IO.into(),
    ));
    // These modules and their dependencies must be available when selected by name at runtime.
    for name in RUNTIME_PROVIDERS.iter().copied().chain(threading) {
        if !seen.contains(name)
            && let Some((file, source)) = import_module::boxed_source(name)
        {
            sources.push((
                name.into(),
                Path::new(bundled_root).join(file),
                source.into(),
            ));
        }
    }
    // The bundle lives in the launcher. This absolute, non-directory path can neither
    // expose the working directory under -I nor collide with an explicit source root.
    let bundle_entry = startup
        .filter(|_| site.is_some())
        .map(|(_, executable)| Path::new(executable).join(bundled_root));
    if let (Some(bundle), Some(site)) = (&bundle_entry, site) {
        initial.extend([bundle.clone(), site.clone()]);
    }
    let paths = serde_json::to_value(initial)
        .map_err(|e| format!("cannot represent startup search path: {e}"))?;
    let keep_docstrings = startup.is_none_or(|(flags, _)| flags.optimize < 2);
    let config = serde_json::json!({
        "bundled_root":bundled_root,
        "bundle_entry":bundle_entry,
        "module_docs":{"json": keep_docstrings.then_some(include_str!("../../stdlib/docstrings/json.txt"))},
        "python_version":crate::PYTHON_VERSION.to_string(),
        "implementation_version":env!("CARGO_PKG_VERSION"),
        "archive_suffix":format!(".{}",crate::NATIVE_ARCHIVE_EXTENSION),
        "isolated":isolated,"safe_path":safe_path,"no_user_site":startup.is_some_and(|(flags, _)| flags.no_user_site),
        "ignore_environment":startup.is_some_and(|(flags, _)| flags.ignore_environment),
        "context_aware_warnings":startup.map(|(flags, _)| flags.context_aware_warnings),
        "cpu_count":startup.map(|(flags, _)| flags.cpu_count),
        "utf8_mode":startup.and_then(|(flags, _)| flags.utf8_mode),
        "warnoptions":startup.map(|(flags, _)| &flags.warnoptions),
        "xoptions":startup.map(|(flags, _)| &flags.xoptions),
        "faulthandler":startup.map(|(flags, _)| flags.faulthandler),
        "tracemalloc":startup.map_or(0, |(flags, _)| flags.tracemalloc),
        "unbuffered":startup.is_some_and(|(flags, _)| flags.unbuffered),
        "warn_default_encoding":startup.map(|(flags, _)| flags.warn_default_encoding),
        "optimize":startup.map_or(0, |(flags, _)| flags.optimize),
        "dont_write_bytecode":startup.is_some_and(|(flags, _)| flags.dont_write_bytecode),
        "executable":startup.map(|(_, executable)| executable),
        "command_source":command,
        "command":in_memory,"module_entry":module_entry,"path_entry":options.path_entry,"paths":paths,
    });
    check_returns()?;
    let providers = options
        .bundled_provider_archive
        .as_deref()
        .filter(|_| keep_docstrings)
        .and_then(provider_archive::symbols)
        .unwrap_or_default();
    let (llvm_ir, used_archive) = emit::emit(
        &sources,
        &config.to_string(),
        keep_docstrings,
        Path::new(bundled_root),
        &providers,
    )?;
    Ok(Some(Compilation {
        llvm_ir,
        input_paths: inputs.files.iter().map(|(p, _, _)| p.clone()).collect(),
        source_inputs: inputs,
        extension_archives: options
            .bundled_provider_archive
            .iter()
            .filter(|_| used_archive)
            .cloned()
            .collect(),
    }))
}

const NAMEDTUPLE_COLLECTIONS: &str = include_str!("../../stdlib/collections/namedtuple_repr.py");
const SEALED_IO: &str = include_str!("../../stdlib/io.py");
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
    "encodings.cp1251",
    "encodings.cp424",
    "encodings.cp437",
    "enum",
    "fractions",
    "functools",
    "genericpath",
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

/// Return the effective source used for a discovered bundled module on this target.
fn bundled_module_source(name: &str) -> Option<(&'static str, std::borrow::Cow<'static, str>)> {
    let (file, source) = if name == "dataclasses" {
        (
            "dataclasses.py",
            concat!(
                include_str!("../../stdlib/dataclasses_missing.py"),
                "\n",
                include_str!("../../stdlib/dataclasses.py"),
            ),
        )
    } else {
        import_module::boxed_source(name)?
    };
    let source = if name == "pkgutil" {
        std::borrow::Cow::Owned(format!(
            "{source}\n_module_suffixes = {}\n",
            crate::modules::pkgutil_suffixes()
        ))
    } else {
        std::borrow::Cow::Borrowed(source)
    };
    Some((file, source))
}

fn imports_for_module(path: &Path, src: &str) -> Result<Imports, String> {
    scan_imports(src, SourceKind::File, None).map_err(|e| format!("{}: {e}", path.display()))
}
