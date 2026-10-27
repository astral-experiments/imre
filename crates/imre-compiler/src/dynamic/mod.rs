//! Collects Python source modules from explicit startup search paths for native lowering.

mod emit;
mod import_module;
pub(super) use import_module::literal_targets;

use std::collections::{HashSet, VecDeque};
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
    sys_names: HashSet<String>,
    operator_names: HashSet<String>,
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

// The object runtime provides installation metadata (EXTENSION_SUFFIXES);
// ModuleSpec and its operations belong to the typed frontend.
fn typed_machinery_export(name: &str) -> bool {
    crate::modules::IntrinsicModule::ImportlibMachinery
        .symbol(name)
        .is_some()
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
                "sysconfig"
                    | "platform"
                    | "keyword"
                    | "site"
                    | "importlib.machinery"
                    | "struct"
                    | "traceback"
            )
        {
            self.runtime_imports = true;
        }
    }
}

fn boxed_sys_export(name: &str) -> bool {
    matches!(
        name,
        "path" | "version_info" | "implementation" | "exit" | "exc_info" | "exception"
    )
}

fn boxed_operator(name: &str) -> bool {
    matches!(
        name,
        "attrgetter"
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
                    match name {
                        "sys" => self.sys_names.insert(bound.to_string()),
                        "operator" => self.operator_names.insert(bound.to_string()),
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
                            || (name == "sys" && boxed_sys_export(suffix)))
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
            Stmt::Assign(a) if a.targets.iter().any(|t| self.sys_path(t)) => {
                self.runtime_imports = true;
                self.paths(&a.value);
            }
            _ => {}
        }
        visitor::walk_stmt(self, s);
    }
    fn visit_expr(&mut self, e: &'a Expr) {
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
        if let Expr::Attribute(a) = e
            && typed_machinery_export(a.attr.as_str())
            && let Some(receiver) = UnqualifiedName::from_expr(&a.value)
        {
            self.typed_machinery_receivers.insert(receiver.to_string());
        }
        if let Expr::Attribute(attr) = e
            && boxed_operator(attr.attr.as_str())
            && let Expr::Name(module) = &*attr.value
            && self.operator_names.contains(module.id.as_str())
        {
            self.runtime_imports = true;
        }
        if matches!(e, Expr::Name(n) if n.id.as_str() == "SystemExit")
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
    imports.visit_body(&parsed.syntax().body);
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

pub(crate) fn compile(
    path: &Path,
    module_root: Option<&Path>,
    options: &CompileOptions,
    runtime_imports: bool,
    literal_modules: Vec<String>,
) -> Result<Option<Compilation>, String> {
    let invocation = options.command.as_ref();
    let startup = options.python_startup();
    let command = invocation.map(|i| &i.source);
    let src = if let Some(command) = command {
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
        && ![
            "sys",
            "platform",
            "keyword",
            "site",
            "importlib",
            "operator",
            "struct",
            "traceback",
            "SystemExit",
        ]
        .iter()
        .any(|name| src.contains(name))
    {
        return Ok(None);
    }
    let mut first = match imports(
        &src,
        if command.is_some() {
            SourceKind::Command
        } else {
            SourceKind::File
        },
    ) {
        Ok(i) if i.runtime_imports || (runtime_imports && !i.names.is_empty()) => i,
        _ => return Ok(None),
    };
    first
        .names
        .extend(literal_modules.into_iter().map(|name| (0, name)));
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    let isolated = startup.is_some_and(|(flags, _)| flags.isolated);
    let entry = cwd.join(path);
    let entry_identity = command
        .is_none()
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
        if command.is_some() {
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
    let mut work = VecDeque::from([("__main__".to_owned(), false, first)]);
    while let Some((owner, package, imports)) = work.pop_front() {
        if imports.needs_typed_machinery() {
            return Ok(None);
        }
        for (level, requested) in imports.names {
            let Some(name) = resolve(&owner, package, level, &requested) else {
                continue;
            };
            let mut prefix = String::new();
            for part in name.split('.').filter(|s| !s.is_empty()) {
                if !prefix.is_empty() {
                    prefix.push('.');
                }
                prefix.push_str(part);
                if !seen.insert(prefix.clone()) {
                    continue;
                }
                let bundled = if prefix == "dataclasses" {
                    Some((
                        "dataclasses.py",
                        include_str!("../../stdlib/dataclasses.py"),
                    ))
                } else {
                    crate::modules::bundled_runtime_source(&prefix)
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
                for root in &roots {
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
                            sources.push((prefix.clone(), stem, String::new()));
                        } else {
                            inputs.missing.push(stem);
                        }
                        continue;
                    };
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
                }
            }
        }
    }
    // These bundled initializers depend only on native modules and may be selected by name at runtime.
    for name in ["platform", "zipimport"] {
        if !seen.contains(name)
            && let Some((file, source)) = crate::modules::bundled_runtime_source(name)
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
    let config = serde_json::json!({
        "bundled_root":bundled_root,
        "python_version":crate::PYTHON_VERSION.to_string(),
        "implementation_version":env!("CARGO_PKG_VERSION"),
        "archive_suffix":format!(".{}",crate::NATIVE_ARCHIVE_EXTENSION),
        "isolated":isolated,"safe_path":safe_path,"no_user_site":startup.is_some_and(|(flags, _)| flags.no_user_site),
        "dont_write_bytecode":startup.is_some_and(|(flags, _)| flags.dont_write_bytecode),
        "executable":startup.map(|(_, executable)| executable),
        "command":command.is_some(),"paths":paths,
    });
    let keep_docstrings = startup.is_none_or(|(flags, _)| flags.optimize < 2);
    let llvm_ir = emit::emit(&sources, &config.to_string(), keep_docstrings)?;
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
