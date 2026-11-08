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

use crate::{CompileOptions, SourceInputs, SourceKind, UnitCompilation};

/// Finds literal directories made available to imports by a startup script.
/// The generated program still evaluates the assignment and consults its actual
/// `sys.path` in order when each import executes.
#[derive(Default)]
struct Imports {
    names: Vec<(u32, String)>,
    explicit_roots: Vec<PathBuf>,
    runtime_imports: bool,
    private_names: imre_ast::Names,
    misplaced_future: bool,
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
    boxed_function_type_params(function, string_annotations, nested, private_names)
}

/// A synchronous boxed function with one body scope can capture its unbounded type parameters.
fn boxed_function_type_params<'a>(
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
    let input = imre_ast::indentation::prepare(&normalized);
    crate::analysis::check_source_complexity(&input.text).map_err(str::to_owned)?;
    let parsed = input.parse_module()?;
    crate::analysis::check_form_feeds(&normalized, parsed.tokens())?;
    let mut body = parsed.into_syntax().body;
    if imre_ast::future_import_error(&body).is_some() {
        // This module fails compilation before any of its imports or path edits execute.
        return Ok(Imports {
            misplaced_future: true,
            ..Imports::default()
        });
    }
    let mut imports = Imports::default();
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

pub(crate) const BUNDLED_SOURCE_ROOT: &str = "<imre stdlib>";

pub(crate) enum ObjectOperation {
    CheckedEntry,
    Builtin,
    OrderedCommandImport,
    ClassLayout,
}

#[derive(Clone, Copy)]
pub(crate) struct RegisteredSource<'a> {
    pub(crate) name: &'a str,
    pub(crate) path: &'a Path,
    pub(crate) source: &'a str,
    pub(crate) is_program: bool,
    pub(crate) checked_module: Option<crate::modules::ModuleId>,
}

impl<'a> RegisteredSource<'a> {
    pub(crate) fn new(
        (name, path, source): &'a (String, PathBuf, String),
        is_program: bool,
        checked_module: Option<crate::modules::ModuleId>,
    ) -> Self {
        Self {
            name,
            path,
            source,
            is_program,
            checked_module,
        }
    }
}

pub(crate) struct RegisteredSources<'a> {
    pub(crate) root: &'a Path,
    pub(crate) roots: &'a [PathBuf],
    pub(crate) winners: &'a [RegisteredSource<'a>],
}

pub(crate) fn compile(
    (path, module_root, mut filesystem): (&Path, Option<&Path>, crate::watch::FileSystem<'_>),
    options: &CompileOptions,
    runtime_imports: bool,
    operation: Option<ObjectOperation>,
    typed_graph: Option<&crate::modules::SourceGraph>,
    layout: crate::BoxedLayout<'_>,
    check: impl FnOnce(bool, Option<RegisteredSources<'_>>) -> Result<(), String>,
) -> Result<Option<UnitCompilation>, String> {
    let invocation = options.command.as_ref();
    let startup = options.python_startup();
    let frozen_modules = startup.is_none_or(|(flags, _)| flags.frozen_modules);
    let command = invocation.map(|i| &i.source);
    let module_entry = options.module_entry.as_deref();
    let in_memory = command.is_some() || module_entry.is_some();
    // A retained native candidate may already own the graph's disk inputs. Only
    // graphs with their original per-module snapshots can supply registrations.
    let ordinary_graph = typed_graph.filter(|graph| {
        !options.runtime_imports && graph.inputs.files.len() == graph.modules.len()
    });
    let file_entry = ordinary_graph.filter(|_| !in_memory);
    let src = if module_entry.is_some() {
        String::new()
    } else if let Some(command) = command {
        command.to_owned()
    } else if let Some(graph) = file_entry {
        graph.modules[graph.entry].original_source().to_owned()
    } else {
        // A direct .pyc operand already received its bytecode diagnostic from the script loader.
        if options.script_startup.is_some() && crate::named_bytecode(path) {
            return Ok(None);
        }
        let Ok(source) = filesystem.read_to_string(path) else {
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
            "pyexpat",
            "xml.parsers",
            "keyword",
            "site",
            "importlib",
            "runpy",
            "json",
            "tomllib",
            "zoneinfo",
            "xml",
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
    let startup_fallback = startup.is_some() && !options.runtime_imports;
    let ordered_command = matches!(operation, Some(ObjectOperation::OrderedCommandImport));
    if !object_hint && !startup_fallback && !ordered_command {
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
    if first.misplaced_future && startup.is_none() {
        // The checked graph can reach object fallback for an unrelated representation
        // error (e.g. a generic class). Report its owned diagnostics before declining.
        check(false, None)?;
        return Ok(None);
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
        || (first.uses_import_builtin && (startup.is_some() || options.runtime_imports))
        || provider_script;
    let object_entry = first.misplaced_future
        || object_hint
            && (entry_requests_objects
                || (operation.is_some() && !ordered_command)
                || (runtime_imports && !first.names.is_empty()));
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
            .any(|(level, name)| *level == 0 && crate::zip_entry::installed_member(name).is_some());
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
    if let Some(name) = module_entry {
        first.add(0, "runpy");
        // Register source candidates without interpreting a module operand as a filesystem path.
        // runpy and the normal finder decide which one can actually execute.
        if crate::modules::valid_module_operand(name) {
            first.add(0, name);
            first.add(0, &format!("{name}.__main__"));
        }
    }
    let registered_imports = typed_graph
        .filter(|_| runtime_imports && !first.misplaced_future)
        .map(import_module::registered_imports)
        .unwrap_or_default();
    first
        .names
        .extend(registered_imports.targets.into_iter().map(|name| (0, name)));
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    let isolated = startup.is_some_and(|(flags, _)| flags.isolated);
    let entry = cwd.join(path);
    let entry_identity = if let Some(graph) = file_entry {
        Some(graph.inputs.files[graph.entry].1.clone())
    } else {
        (!in_memory)
            .then(|| filesystem.canonicalize(&entry))
            .transpose()
            .map_err(|e| e.to_string())?
    };
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
    let mut initial = Vec::new();
    let safe_path = startup.is_some_and(|(flags, _)| flags.safe_path);
    if options.path_entry {
        // CPython preserves the directory spelling, including symlinks, and searches
        // an explicitly requested entry even when isolated startup omits the cwd.
        initial.push(entry.clone());
    } else if !safe_path {
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
    roots.extend(first.explicit_roots.iter().map(|p| cwd.join(p)));
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
        let digest = file_entry.map_or_else(
            || blake3::hash(src.as_bytes()).to_hex().to_string(),
            |graph| graph.inputs.files[graph.entry].2.clone(),
        );
        inputs.files.push((path.to_owned(), identity, digest));
    }
    let mut selected_sources = Vec::new();
    if module_entry.is_none() && !options.runtime_imports {
        selected_sources.push((0, true, ordinary_graph.map(|graph| graph.entry)));
    }
    let mut source_initializers = HashSet::new();
    if first.misplaced_future {
        source_initializers.insert(sources[0].1.clone());
    }
    let mut seen = HashSet::new();
    // Keep source hints on the winning regular package's path. The runtime finder
    // can still load a different child if a parent changes its __path__.
    let mut entry_sources: HashMap<String, (PathBuf, bool)> = HashMap::new();
    let mut before_site = HashSet::new();
    let mut frozen_imports = Imports::default();
    let mut work = VecDeque::from([("__main__".to_owned(), false, first)]);
    let mut uses_import_builtin = false;
    let mut uses_exec = false;
    let mut computed_import = registered_imports.computed;
    let mut bundled_only = false;
    let mut getters = VecDeque::new();
    let mut bundled_imports = VecDeque::new();
    let mut through_getter = false;
    loop {
        let (owner, package, imports) = if let Some(next) = work.pop_front() {
            next
        } else if let Some(next) = getters.pop_front() {
            // Keep project shadows for a bundled module's lazy attributes. The
            // ordinary graph has already checked any modules the user imported.
            through_getter = true;
            next
        } else if let Some((next, lazy)) = bundled_imports.pop_front() {
            // The program can reach these providers through a ZIP or an extended package
            // path. Their hints must not force unrequested application files to compile.
            through_getter = lazy;
            bundled_only = true;
            next
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
                // Carry its source for relocation, but leave compilation until the name is chosen.
                if !seen.contains("_pydecimal") {
                    source_initializers.insert(Path::new(bundled_root).join("_pydecimal.py"));
                }
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
                break;
            }
            bundled_only = true;
            (String::new(), false, imports)
        };
        if imports
            .names
            .iter()
            .any(|(level, name)| *level == 0 && name == "importlib.abc.SourceLoader")
            && let Some(graph) = typed_graph
            && (owner == "__main__" || graph.modules.iter().any(|m| !m.bundled && m.name == owner))
        {
            graph.require_fallback_calls();
        }
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
            && imports.uses_machinery_export("ModuleSpec")
            && !matches!(operation, Some(ObjectOperation::CheckedEntry))
            && (owner == "__main__"
                || machinery_graph
                    .is_none_or(|graph| graph.modules.iter().any(|m| m.name == owner)))
        {
            return Ok(None);
        }
        uses_exec |= imports.uses_exec;
        computed_import |= imports.computed_import;
        // A checked caller can request the loader before its other annotated calls are lowered.
        if !options.runtime_imports
            && options.python_startup().is_none()
            && (imports.uses_machinery_export("BuiltinImporter")
                || imports.uses_machinery_export("FrozenImporter"))
            && let Some(graph) = typed_graph
            && (owner == "__main__" || graph.modules.iter().any(|m| !m.bundled && m.name == owner))
        {
            graph.require_fallback_calls();
        }
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
        for (level, requested) in names {
            let Some(name) = crate::modules::resolve_import(package_name, level, &requested) else {
                continue;
            };
            // A frozen import never visits sys.path unless the program can disable
            // the frozen importer. Defer collecting its filesystem alternatives.
            if frozen_modules
                && !seen.contains("_imp")
                && let Some(top @ ("stat" | "__hello__")) = name.split('.').next()
                && crate::modules::bundled_runtime_source(top).is_some()
            {
                frozen_imports.names.push((0, name));
                continue;
            }
            let mut prefix = String::new();
            let mut project_parent = false;
            for part in name.split('.').filter(|s| !s.is_empty()) {
                if !prefix.is_empty() {
                    project_parent |= entry_sources
                        .get(&prefix)
                        .is_some_and(|(root, _)| initial.contains(root));
                    prefix.push('.');
                }
                prefix.push_str(part);
                if !seen.insert(prefix.clone()) {
                    continue;
                }
                if prefix == "_imp" && !frozen_imports.names.is_empty() {
                    work.push_back((String::new(), false, std::mem::take(&mut frozen_imports)));
                }
                let mut bundled = bundled_module_source(&prefix, startup.is_none());
                // Standalone programs can run without the launcher's stdlib ZIP.
                if bundled.is_none()
                    && startup.is_none()
                    && let Some(member) = crate::zip_entry::standalone_member(&prefix)
                {
                    bundled = Some((member, crate::zip_entry::installed_source(member)?.into()));
                }
                let mut installed = if bundled.is_none() {
                    crate::zip_entry::installed_member(&prefix)
                } else {
                    None
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
                    // An earlier regular project ancestor controls the live search path.
                    // Keep the registered alternative without following it into the project.
                    let mut enqueue = |hints, lazy| {
                        let next = (prefix.clone(), package, hints);
                        if project_parent {
                            bundled_imports.push_back((next, lazy || through_getter));
                        } else if lazy {
                            getters.push_back(next);
                        } else {
                            work.push_back(next);
                        }
                    };
                    enqueue(imports, false);
                    if !getter.names.is_empty() || getter.uses_exec {
                        enqueue(getter, true);
                    }
                    sources.push((prefix.clone(), path, source.into_owned()));
                }
                // When frozen imports are disabled, -m can load its runpy from sys.path.
                if frozen_modules && prefix == "runpy" {
                    continue;
                }
                let graph_file = ordinary_graph.and_then(|graph| {
                    let id = graph.python_file(&prefix)?;
                    Some((id, &graph.modules[id], &graph.inputs.files[id]))
                });
                for root in roots.iter().filter(|_| !bundled_only) {
                    if implicit_dirs.contains(root) && before_site.contains(&prefix) {
                        continue;
                    }
                    if let Some((parent, _)) = prefix.rsplit_once('.')
                        && let Some((selected, package)) = entry_sources.get(parent)
                        && (module_entry.is_some()
                            || archives.contains_key(selected)
                            || archives.contains_key(root))
                        && (!package || selected != root)
                    {
                        continue;
                    }
                    if let Some(archive) = archives.get_mut(root) {
                        // A regular source already won. Ordinary files still register later
                        // directory alternatives for programs that change sys.path at runtime.
                        if entry_sources.contains_key(&prefix) {
                            continue;
                        }
                        if let Some(package) = archive.package(&prefix) {
                            installed = None;
                            if let Some((member, source, _)) = archive.module(&prefix)
                                && let Ok(imports) = imports_for_module(
                                    &member,
                                    source.trim_start_matches('\u{feff}'),
                                )
                            {
                                work.push_back((prefix.clone(), package, imports));
                            }
                            if !implicit_dirs.is_empty() {
                                before_site.insert(prefix.clone());
                            }
                            entry_sources.insert(prefix.clone(), (root.clone(), package));
                            break;
                        }
                        continue;
                    }
                    let stem = root.join(prefix.replace('.', "/"));
                    let file = stem.with_extension("py");
                    let init = stem.join("__init__.py");
                    // The checked resolver already selected a concrete package or module at
                    // this root. Other roots still supply alternatives for a live sys.path.
                    let checked = graph_file
                        .filter(|(_, module, _)| module.path == init || module.path == file);
                    let package = checked.map(|(_, module, _)| module.path == init);
                    let (path, package) = if package.unwrap_or_else(|| filesystem.is_file(&init)) {
                        (init, true)
                    } else if checked.is_some() || filesystem.is_file(&file) {
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
                            sources.push((prefix.clone(), stem, String::new()));
                        } else {
                            inputs.missing.push(stem);
                        }
                        continue;
                    };
                    if !implicit_dirs.is_empty() && !implicit_dirs.contains(root) {
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
                            || (fallback_script
                                && ((logging_script && prefix == "logging")
                                    || (tokenize_script && prefix == "tokenize")
                                    || (configparser_script && prefix == "configparser")
                                    || (pyio_script && prefix == "_pyio"))))
                    {
                        return Ok(None);
                    }
                    installed = None;
                    let src = if let Some((_, module, input)) = checked {
                        inputs.files.push(input.clone());
                        module.original_source().to_owned()
                    } else {
                        let identity = filesystem.canonicalize(&path);
                        let src = filesystem
                            .read_to_string(&path)
                            .map_err(|e| format!("{}: {e}", path.display()))?;
                        inputs.files.push((
                            path.clone(),
                            identity.map_err(|e| e.to_string())?,
                            blake3::hash(src.as_bytes()).to_hex().to_string(),
                        ));
                        src
                    };
                    let imports = imports_for_module(&path, &src)?;
                    // Later alternatives remain available to the runtime finder. Only the
                    // winning provider supplies the initial module binding.
                    let on_package_path = prefix
                        .rsplit_once('.')
                        .and_then(|(parent, _)| entry_sources.get(parent))
                        .is_none_or(|(selected, is_package)| *is_package && selected == root);
                    let winner = on_package_path && !entry_sources.contains_key(&prefix);
                    if winner {
                        entry_sources.insert(prefix.clone(), (root.clone(), package));
                    }
                    if !imports.misplaced_future || startup.is_none() {
                        if imports.misplaced_future {
                            source_initializers.insert(path.clone());
                        } else {
                            work.push_back((prefix.clone(), package, imports));
                        }
                        let registration = (prefix.clone(), path, src);
                        if winner && !options.runtime_imports {
                            // Only path zero belongs to the program. With safe path
                            // the first remaining search entry may be an installation.
                            let program = initial[..path0_len].contains(root);
                            selected_sources.push((
                                sources.len(),
                                program,
                                checked.map(|(id, _, _)| id),
                            ));
                        }
                        sources.push(registration);
                    }
                    if module_entry.is_some() {
                        break;
                    }
                }
                if let Some(member) = installed {
                    let source = crate::zip_entry::installed_source(member)?;
                    let imports = imports_for_module(Path::new(member), &source)?;
                    bundled_imports.push_back((
                        (prefix.clone(), member.ends_with("/__init__.py"), imports),
                        false,
                    ));
                }
            }
        }
    }
    let threading = source_threading(|required| {
        sources
            .iter()
            .any(|(name, path, _)| name == required && path.starts_with(bundled_root))
    });
    for (name, file, body) in runtime_sources(|name| seen.contains(name), threading) {
        sources.push((name.into(), Path::new(bundled_root).join(file), body.into()));
    }
    // These bodies extend native module scopes. Keep them out of the source finder and
    // out of the checked project's winners, even when no explicit import names them.
    let builtins: Vec<_> = builtin_sources(options.target_os.unwrap_or(std::env::consts::OS))
        .into_iter()
        .map(|(name, file, body)| (name.into(), Path::new(bundled_root).join(file), body.into()))
        .collect();
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
        if in_memory && !options.path_entry {
            first.clear();
        } else if first.to_str().is_none() {
            path0_bytes = Some(std::mem::take(first).into_os_string().into_encoded_bytes());
        }
    }
    let paths = serde_json::to_value(initial)
        .map_err(|e| format!("cannot represent startup search path: {e}"))?;
    let keep_docstrings = startup.is_none_or(|(flags, _)| flags.optimize < 2);
    let stdlib_fallback =
        serde_json::to_value(&options.stdlib_fallback).map_err(|e| e.to_string())?;
    let config = serde_json::json!({
        "bundled_root":bundled_root,
        "stdlib_fallback":stdlib_fallback,
        "stdlib_members":include_str!(concat!(env!("OUT_DIR"), "/imre-stdlib-members.txt")),
        "sitecustomize":options.sitecustomize || options.stdlib_sitecustomize,"user_site_pth":options.user_site_pth,"after_user_site":after_user_site,"path0":options.path_entry || !safe_path,
        "bundle_entry":bundle_entry,
        "module_docs":{"json": keep_docstrings.then_some(include_str!("../../stdlib/docstrings/json.txt"))},
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
        "optimize":startup.map_or(0, |(flags, _)| flags.optimize),
        "dont_write_bytecode":startup.is_some_and(|(flags, _)| flags.dont_write_bytecode),
        "executable":startup.map(|(_, executable)| executable),
        "command_source":command,"path0_bytes":path0_bytes,
        "command":in_memory,"module_entry":module_entry,"path_entry":options.path_entry,"paths":paths,
    });
    // Borrow the winners once discovery has finished growing the emitter's source table.
    let selected_sources: Vec<_> = selected_sources
        .into_iter()
        .map(|(id, program, module)| RegisteredSource::new(&sources[id], program, module))
        .collect();
    check(
        entry_requests_objects || startup_provider,
        Some(RegisteredSources {
            root: &root,
            roots: &roots,
            winners: &selected_sources,
        }),
    )?;
    let modules = (sources.as_slice(), builtins.as_slice());
    let (units, used_archive) = if matches!(layout, crate::BoxedLayout::Check) {
        emit::check(modules, keep_docstrings, &source_initializers)?;
        (Vec::new(), false)
    } else {
        let providers = options
            .bundled_provider_archive
            .as_deref()
            .filter(|_| keep_docstrings)
            .and_then(provider_archive::symbols)
            .unwrap_or_default();
        let config = config.to_string();
        let emit = |split| {
            emit::emit(
                modules,
                &config,
                keep_docstrings,
                Path::new(bundled_root),
                &providers,
                split,
                &source_initializers,
            )
        };
        let (mut units, mut used_archive) = emit(!matches!(layout, crate::BoxedLayout::Whole))?;
        if let crate::BoxedLayout::WholeWithUnits(native) = layout
            && units.len() > 1
        {
            // Both representations use the same resolved source and registration order.
            let (whole, archived) = emit(false)?;
            *native = std::mem::replace(&mut units, whole);
            used_archive |= archived;
        }
        (units, used_archive)
    };
    inputs.boxed_imports = true;
    Ok(Some(UnitCompilation {
        units,
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
        NAMEDTUPLE_COLLECTIONS,
    ));
    collections
        .into_iter()
        .chain([("io", "io.py", SEALED_IO)])
        .chain(
            RUNTIME_PROVIDERS
                .iter()
                .copied()
                .chain(FIXED_UTF_PROVIDERS.iter().map(|(name, _)| *name))
                .chain(threading)
                .filter(move |name| !seen(name))
                .filter_map(|name| {
                    let (file, source) = import_module::boxed_source(name)?;
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
    names.sort_unstable();
    names.dedup();
    names.join("\0")
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
    [("selectors", "selectors.py", selectors)]
}

const NAMEDTUPLE_COLLECTIONS: &str = include_str!("../../stdlib/collections/namedtuple_repr.py");
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
    "encodings.koi8_u",
    "encodings.idna",
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

/// Return the effective source used for a discovered bundled module on this target.
fn bundled_module_source(
    name: &str,
    standalone: bool,
) -> Option<(&'static str, std::borrow::Cow<'static, str>)> {
    let (file, source) = if standalone && name == "_pydecimal" {
        // Standalone executables can run without the Python launcher's installed stdlib ZIP.
        ("_pydecimal.py", include_str!("../../stdlib/_pydecimal.py"))
    } else if name == "dataclasses" {
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
    Some((file, source.into()))
}

fn imports_for_module(path: &Path, src: &str) -> Result<Imports, String> {
    scan_imports(src, SourceKind::File, None).map_err(|e| format!("{}: {e}", path.display()))
}
