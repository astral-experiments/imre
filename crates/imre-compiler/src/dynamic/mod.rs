//! Collects Python source modules from explicit startup search paths for native lowering.

mod emit;
mod import_module;
pub(super) use import_module::{RegisteredImports, registered_imports};

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
    uses_import_builtin: bool,
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
                    | "atexit"
                    | "faulthandler"
                    | "code"
                    | "codeop"
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
                    | "traceback"
                    | "unittest.case"
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

fn boxed_unittest_cleanup(name: &str) -> bool {
    matches!(
        name,
        "case" | "addModuleCleanup" | "doModuleCleanups" | "enterModuleContext"
    )
}

fn boxed_operator(name: &str) -> bool {
    matches!(
        name,
        "attrgetter"
            | "__lt__"
            | "__le__"
            | "__eq__"
            | "__ne__"
            | "__ge__"
            | "__gt__"
            | "__and__"
            | "__or__"
            | "__sub__"
            | "__xor__"
            | "itemgetter"
            | "lt"
            | "le"
            | "eq"
            | "ne"
            | "gt"
            | "ge"
            | "add"
            | "sub"
            | "mul"
            | "matmul"
            | "truediv"
            | "floordiv"
            | "mod"
            | "lshift"
            | "rshift"
            | "and_"
            | "xor"
            | "or_"
            | "iadd"
            | "isub"
            | "imul"
            | "imatmul"
            | "itruediv"
            | "ifloordiv"
            | "imod"
            | "ipow"
            | "ilshift"
            | "irshift"
            | "iand"
            | "ixor"
            | "ior"
            | "neg"
            | "pos"
            | "invert"
            | "contains"
            | "getitem"
            | "setitem"
            | "delitem"
            | "index"
            | "length_hint"
    )
}

impl<'a> Visitor<'a> for Imports {
    fn visit_stmt(&mut self, s: &'a Stmt) {
        match s {
            Stmt::Import(imp) => {
                for alias in &imp.names {
                    let name = alias.name.as_str();
                    let bound = alias.asname.as_ref().unwrap_or(&alias.name);
                    if name == "__future__" {
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
                self.add(imp.level, name);
                for a in &imp.names {
                    let suffix = a.name.as_str();
                    if imp.level == 0
                        && (name == "unittest" && boxed_unittest_cleanup(suffix)
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
        if let Expr::Name(name) = e
            && name.ctx == ast::ExprContext::Load
        {
            if name.id == "__import__" {
                self.uses_import_builtin = true;
                self.add(0, "importlib");
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
                || (boxed_unittest_cleanup(attr.attr.as_str())
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

fn imports(source: &str, kind: SourceKind) -> Result<Imports, String> {
    let normalized = crate::normalize_source(source, kind)?;
    crate::analysis::check_source_complexity(&normalized).map_err(str::to_owned)?;
    let parsed = ruff_python_parser::parse_module(&normalized).map_err(|e| e.to_string())?;
    crate::analysis::check_indentation(&normalized, parsed.tokens())?;
    let mut imports = Imports::default();
    let mut body = parsed.into_syntax().body;
    imre_ast::lower(&mut body);
    imports.visit_body(&body);
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

pub(crate) enum ObjectOperation<'a> {
    Builtin,
    ClassBody(&'a crate::modules::SourceGraph),
}

pub(crate) fn compile(
    path: &Path,
    module_root: Option<&Path>,
    options: &CompileOptions,
    runtime_imports: bool,
    operation: Option<ObjectOperation<'_>>,
    registered_imports: RegisteredImports,
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
        let Ok(source) = std::fs::read_to_string(path) else {
            return Ok(None);
        };
        source
    };
    // An ordinary entry can import a package that uses the object runtime without
    // accessing sys.path or installation metadata in the entry itself.
    if !runtime_imports
        && !(startup.is_some() && (src.contains("urllib") || src.contains("ftplib")))
        && !(src.contains("class") && src.contains('='))
        && !(cfg!(unix) && src.contains("signal"))
        && ![
            "__future__",
            "hashlib",
            "_imp",
            "atexit",
            "faulthandler",
            "code",
            "weakref",
            "pathlib",
            "sys",
            "platform",
            "keyword",
            "site",
            "importlib",
            "runpy",
            "json",
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
        .any(|name| src.contains(name))
    {
        return Ok(None);
    }
    let Ok(mut first) = imports(
        &src,
        if command.is_some() {
            SourceKind::Command
        } else {
            SourceKind::File
        },
    ) else {
        return Ok(None);
    };
    // Python scripts load the URL opener and FTP client as boxed modules.
    let imports_transport = |transport: &str| {
        startup.is_some()
            && !options.runtime_imports
            && first
                .names
                .iter()
                .any(|(level, name)| *level == 0 && name == transport)
    };
    let request_script = imports_transport("urllib.request");
    let ftp_script = imports_transport("ftplib");
    let transport_script = request_script || ftp_script;
    if !(module_entry.is_some()
        || first.runtime_imports
        || operation.is_some()
        || (runtime_imports && !first.names.is_empty())
        || (first.uses_import_builtin && (startup.is_some() || options.runtime_imports))
        || transport_script)
    {
        return Ok(None);
    }
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
    if !safe_path {
        initial.push(root);
    }
    initial.extend(options.extension_paths.iter().map(|p| cwd.join(p)));
    let mut roots = initial.clone();
    roots.extend(first.explicit_roots.iter().map(|p| cwd.join(p)));
    // Discover each search directory once even when it appears twice in sys.path.
    let mut seen_roots = HashSet::new();
    roots.retain(|root| seen_roots.insert(root.clone()));
    let bundled_root = "<imre stdlib>";
    let mut sources = vec![(
        "__main__".to_owned(),
        if in_memory {
            PathBuf::from("<string>")
        } else {
            entry
        },
        src.clone(),
    )];
    let mut inputs = SourceInputs::default();
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
    let mut frozen_imports = Imports::default();
    let mut work = VecDeque::from([("__main__".to_owned(), false, first)]);
    let mut late_pprint = false;
    let mut bundled_only = false;
    loop {
        let (owner, package, imports) = if let Some(next) = work.pop_front() {
            next
        } else {
            // Collect late imports and their bundled dependencies after the explicit
            // file graph, without parsing unrelated modules in project directories.
            let mut imports = Imports::default();
            if late_pprint && !seen.contains("pprint") {
                imports.names.push((0, "pprint".to_owned()));
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
        if module_entry.is_none()
            && !options.runtime_imports
            && !registered_imports.called
            && (!transport_script || owner == "__main__")
            && imports.needs_typed_machinery()
            // Enforce the checked-file machinery contract on the original graph.
            // A module discovered only by boxed imports has no typed layout to preserve.
            && match operation {
                Some(ObjectOperation::ClassBody(graph)) => owner == "__main__"
                    || graph.modules.iter().any(|m| m.name == owner),
                _ => true,
            }
        {
            return Ok(None);
        }
        let mut names = imports.names;
        if owner == "importlib" {
            // Filesystem modules can import the loader protocols through either public module.
            names.push((0, "importlib.abc".to_owned()));
            names.push((0, "importlib.util".to_owned()));
        }
        // A late import of native pickle also needs its bundled reader and dependencies.
        if imports.uses_import_builtin {
            late_pprint = true;
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
                let bundled = if prefix == "dataclasses" {
                    Some((
                        "dataclasses.py",
                        concat!(
                            include_str!("../../stdlib/dataclasses_missing.py"),
                            "\n",
                            include_str!("../../stdlib/dataclasses.py"),
                        ),
                    ))
                } else {
                    import_module::boxed_source(&prefix)
                };
                if let Some((filename, source)) = bundled {
                    let path = Path::new(bundled_root).join(filename);
                    let package = path.ends_with("__init__.py");
                    let pkgutil_source;
                    let source = if prefix == "pkgutil" {
                        pkgutil_source = format!(
                            "{source}\n_module_suffixes = {}\n",
                            crate::modules::pkgutil_suffixes()
                        );
                        &pkgutil_source
                    } else {
                        source
                    };
                    let imports = imports_for_module(&path, source)?;
                    sources.push((prefix.clone(), path, source.to_owned()));
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
                    if let Some((parent, _)) = prefix.rsplit_once('.')
                        && let Some(selected) = entry_sources.get(parent)
                        && selected.as_ref() != Some(root)
                    {
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
                    // Bundled transports cannot stand in for earlier project modules.
                    if initial.contains(root)
                        && ((request_script && prefix == "urllib")
                            || (ftp_script && prefix == "ftplib"))
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
            include_str!("../../stdlib/collections/namedtuple_repr.py").into(),
        ));
    }
    // The sealed io module also needs its Python protocols when imported indirectly.
    sources.push((
        "io".into(),
        Path::new(bundled_root).join("io.py"),
        include_str!("../../stdlib/io.py").into(),
    ));
    // These modules and their dependencies must be available when selected by name at runtime.
    for name in [
        "__future__",
        "__hello__",
        "_frozen_importlib",
        "_frozen_importlib_external",
        "_opcode_metadata",
        "_tokenize",
        "abc",
        "annotationlib",
        "ast",
        "code",
        "codecs",
        "codeop",
        "copyreg",
        "datetime",
        "enum",
        "genericpath",
        "hashlib",
        "itertools",
        "json",
        "json.decoder",
        "json.encoder",
        "json.scanner",
        "opcode",
        "platform",
        "re",
        "signal",
        "stat",
        "string",
        "string.templatelib",
        "subprocess",
        "token",
        "types",
        "zipimport",
    ]
    .into_iter()
    .chain(threading)
    {
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
    let paths = serde_json::to_value(initial)
        .map_err(|e| format!("cannot represent startup search path: {e}"))?;
    let keep_docstrings = startup.is_none_or(|(flags, _)| flags.optimize < 2);
    let config = serde_json::json!({
        "bundled_root":bundled_root,
        "module_docs":{"json": keep_docstrings.then_some(include_str!("../../stdlib/docstrings/json.txt"))},
        "python_version":crate::PYTHON_VERSION.to_string(),
        "implementation_version":env!("CARGO_PKG_VERSION"),
        "archive_suffix":format!(".{}",crate::NATIVE_ARCHIVE_EXTENSION),
        "isolated":isolated,"safe_path":safe_path,"no_user_site":startup.is_some_and(|(flags, _)| flags.no_user_site),
        "ignore_environment":startup.is_some_and(|(flags, _)| flags.ignore_environment),
        "context_aware_warnings":startup.map(|(flags, _)| flags.context_aware_warnings),
        "utf8_mode":startup.and_then(|(flags, _)| flags.utf8_mode),
        "warnoptions":startup.map(|(flags, _)| &flags.warnoptions),
        "xoptions":startup.map(|(flags, _)| &flags.xoptions),
        "faulthandler":startup.map(|(flags, _)| flags.faulthandler),
        "warn_default_encoding":startup.map(|(flags, _)| flags.warn_default_encoding),
        "optimize":startup.map_or(0, |(flags, _)| flags.optimize),
        "dont_write_bytecode":startup.is_some_and(|(flags, _)| flags.dont_write_bytecode),
        "executable":startup.map(|(_, executable)| executable),
        "command":in_memory,"module_entry":module_entry,"paths":paths,
    });
    check_returns()?;
    let llvm_ir = emit::emit(
        &sources,
        &config.to_string(),
        keep_docstrings,
        Path::new(bundled_root),
    )?;
    Ok(Some(Compilation {
        llvm_ir,
        input_paths: inputs.files.iter().map(|(p, _, _)| p.clone()).collect(),
        source_inputs: inputs,
        extension_archives: Vec::new(),
    }))
}

fn imports_for_module(path: &Path, src: &str) -> Result<Imports, String> {
    imports(src, SourceKind::File).map_err(|e| format!("{}: {e}", path.display()))
}
