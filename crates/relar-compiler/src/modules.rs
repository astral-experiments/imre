//! Loads a closed graph of first-party modules without executing Python initialization.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use ruff_db::parsed::ParsedModuleRef;
use ruff_python_ast as ast;
use ruff_text_size::Ranged;

use crate::analysis::Session;

pub(crate) type ModuleId = usize;

// CPython's built-in/frozen lookup precedes first-party files. These internal and
// frozen test modules have no corresponding stubs in the pinned typeshed inventory.
const INTRINSIC_MODULES: &[&str] = &[
    "__main__",
    "__hello__",
    "__hello_alias__",
    "__hello_only__",
    "__phello__",
    "__phello_alias__",
    "_hmac",
    "_suggestions",
    "_sysconfig",
    "_types",
];

/// A source definition's identity, independent of its eventual runtime representation.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct DefinitionId {
    pub(crate) module: ModuleId,
    pub(crate) statement: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Binding {
    Module(ModuleId),
    Definition(DefinitionId),
    IntrinsicModule(IntrinsicModule),
    Intrinsic(Intrinsic),
}

/// Sealed namespaces expose only compiler-known symbols, never module values.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum IntrinsicModule {
    Sys,
    Typing,
    Io,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Intrinsic {
    Argv,
    TextIO,
    UnsupportedOperation,
}

impl IntrinsicModule {
    fn named(name: &str) -> Option<Self> {
        match name {
            "sys" => Some(Self::Sys),
            "typing" => Some(Self::Typing),
            "io" => Some(Self::Io),
            _ => None,
        }
    }

    fn symbol(self, name: &str) -> Option<Binding> {
        let symbol = match (self, name) {
            (Self::Sys, "argv") => Intrinsic::Argv,
            (Self::Typing, "TextIO") => Intrinsic::TextIO,
            (Self::Io, "UnsupportedOperation") => Intrinsic::UnsupportedOperation,
            _ => return None,
        };
        Some(Binding::Intrinsic(symbol))
    }
}

pub(crate) struct SourceModule {
    pub(crate) path: PathBuf,
    pub(crate) name: String,
    pub(crate) source: String,
    pub(crate) parsed: ParsedModuleRef,
    pub(crate) bindings: HashMap<String, Binding>,
    /// Executable entry statements, or inactive main guards that still require checking.
    pub(crate) executable: Vec<usize>,
    is_package: bool,
}

impl SourceModule {
    pub(crate) fn error(&self, node: &impl Ranged, message: &str) -> String {
        let prefix = &self.source[..usize::from(node.start())];
        let line = prefix.bytes().filter(|&byte| byte == b'\n').count() + 1;
        let column = prefix.rsplit('\n').next().unwrap_or("").chars().count() + 1;
        format!("{}:{line}:{column}: Relar: {message}", self.path.display())
    }
}

pub(crate) struct SourceGraph {
    pub(crate) modules: Vec<SourceModule>,
    pub(crate) entry: ModuleId,
}

impl SourceGraph {
    /// An isolated source API supports sealed imports only; first-party imports
    /// require the path API and its filesystem search root.
    pub(crate) fn single(source: &str) -> Result<Self, String> {
        let mut loader = Loader::new(PathBuf::from("/"))?;
        loader.allow_imports = false;
        let entry = loader.add(
            PathBuf::from("/input.py"),
            PathBuf::from("/input.py"),
            "__main__".into(),
            false,
            source,
        )?;
        loader.scan(entry)?;
        loader.finish(entry)
    }

    pub(crate) fn load(entry: &Path, module_root: Option<&Path>) -> Result<Self, String> {
        let entry = entry
            .canonicalize()
            .map_err(|error| format!("{}: {error}", entry.display()))?;
        if entry.extension().and_then(|value| value.to_str()) != Some("py") {
            return Err("entry source must be a .py file".into());
        }
        if entry.file_name().and_then(|value| value.to_str()) == Some("__init__.py") {
            return Err("use a program module or package __main__.py as the entry source".into());
        }
        let root = if let Some(root) = module_root {
            root.canonicalize()
                .map_err(|error| format!("{}: {error}", root.display()))?
        } else {
            let mut root = entry
                .parent()
                .ok_or("entry source has no parent")?
                .to_path_buf();
            while root.join("__init__.py").is_file() {
                root = root
                    .parent()
                    .ok_or("package has no module root")?
                    .to_path_buf();
            }
            root
        };
        if !root.is_dir() {
            return Err("module root must be a directory".into());
        }
        let relative = entry
            .strip_prefix(&root)
            .map_err(|_| "entry source must be inside the module root")?;
        let name = relative
            .with_extension("")
            .components()
            .map(|part| {
                part.as_os_str()
                    .to_str()
                    .ok_or("module paths must be valid UTF-8")
            })
            .collect::<Result<Vec<_>, _>>()?
            .join(".");
        let source = std::fs::read_to_string(&entry)
            .map_err(|error| format!("{}: {error}", entry.display()))?;
        let mut loader = Loader::new(root)?;
        let id = loader.add(entry.clone(), entry, name.clone(), false, &source)?;
        // Like `python -m package.program`, package initialization precedes the entry.
        // The entry itself is __main__, and is deliberately not attached as a package child.
        if let Some((package, _)) = name.rsplit_once('.') {
            let package = loader.load_module(package)?;
            if !loader.modules[package].is_package {
                return Err(
                    "entry source is inside a directory without an __init__.py package".into(),
                );
            }
        }
        loader.scan(id)?;
        loader.finish(id)
    }

    /// Resolves namespace paths while leaving local-value attributes to the expression checker.
    pub(crate) fn resolve(&self, module: ModuleId, expression: &ast::Expr) -> Option<Binding> {
        match expression {
            ast::Expr::Name(name) => self.modules[module].bindings.get(name.id.as_str()).copied(),
            ast::Expr::Attribute(attribute) => match self.resolve(module, &attribute.value)? {
                Binding::Module(module) => self.modules[module]
                    .bindings
                    .get(attribute.attr.as_str())
                    .copied(),
                Binding::IntrinsicModule(module) => module.symbol(attribute.attr.as_str()),
                Binding::Definition(_) | Binding::Intrinsic(_) => None,
            },
            _ => None,
        }
    }
}

struct Loader {
    root: PathBuf,
    session: Session,
    modules: Vec<SourceModule>,
    by_name: HashMap<String, ModuleId>,
    by_path: HashMap<PathBuf, ModuleId>,
    loading: HashSet<ModuleId>,
    allow_imports: bool,
}

impl Loader {
    fn new(root: PathBuf) -> Result<Self, String> {
        Ok(Self {
            session: Session::new(&root)?,
            root,
            modules: Vec::new(),
            by_name: HashMap::new(),
            by_path: HashMap::new(),
            loading: HashSet::new(),
            allow_imports: true,
        })
    }

    fn add(
        &mut self,
        path: PathBuf,
        identity: PathBuf,
        name: String,
        is_package: bool,
        source: &str,
    ) -> Result<ModuleId, String> {
        let source = crate::normalize_source(source)
            .map_err(|error| format!("{}: {error}", path.display()))?
            .into_owned();
        let parsed = self.session.add(&path, &source)?;
        let id = self.modules.len();
        self.by_name.insert(name.clone(), id);
        self.by_path.insert(identity, id);
        self.loading.insert(id);
        self.modules.push(SourceModule {
            path,
            name,
            source,
            parsed,
            is_package,
            bindings: HashMap::new(),
            executable: Vec::new(),
        });
        Ok(id)
    }

    fn finish(self, entry: ModuleId) -> Result<SourceGraph, String> {
        self.session.check()?;
        Ok(SourceGraph {
            modules: self.modules,
            entry,
        })
    }

    fn scan(&mut self, id: ModuleId) -> Result<(), String> {
        let parsed = self.modules[id].parsed.clone();
        let mut executable = false;
        for (index, statement) in parsed.syntax().body.iter().enumerate() {
            match statement {
                ast::Stmt::FunctionDef(function) => {
                    if executable {
                        return Err(self.modules[id].error(
                            statement,
                            "definitions must precede executable module statements",
                        ));
                    }
                    self.bind(
                        id,
                        function.name.as_str(),
                        Binding::Definition(DefinitionId {
                            module: id,
                            statement: index,
                        }),
                        statement,
                    )?;
                }
                ast::Stmt::ClassDef(class) => {
                    if executable {
                        return Err(self.modules[id].error(
                            statement,
                            "definitions must precede executable module statements",
                        ));
                    }
                    self.bind(
                        id,
                        class.name.as_str(),
                        Binding::Definition(DefinitionId {
                            module: id,
                            statement: index,
                        }),
                        statement,
                    )?;
                }
                ast::Stmt::Import(_) | ast::Stmt::ImportFrom(_) => {
                    if executable {
                        return Err(self.modules[id].error(
                            statement,
                            "imports must precede executable module statements",
                        ));
                    }
                    self.import(id, statement)
                        .map_err(|error| self.modules[id].error(statement, &error))?;
                }
                ast::Stmt::Expr(value)
                    if matches!(&*value.value, ast::Expr::StringLiteral(_)) && !executable => {}
                _ => {
                    if id != 0 && !main_guard(statement) {
                        return Err(self.modules[id].error(statement, "library modules may contain only definitions, imports, docstrings, and an inactive __main__ guard"));
                    }
                    executable = true;
                    self.modules[id].executable.push(index);
                }
            }
        }
        self.loading.remove(&id);
        Ok(())
    }

    fn bind(
        &mut self,
        module: ModuleId,
        name: &str,
        binding: Binding,
        node: &impl Ranged,
    ) -> Result<(), String> {
        if let Some(previous) = self.modules[module].bindings.get(name) {
            if *previous != binding {
                return Err(self.modules[module]
                    .error(node, &format!("static binding {name:?} cannot be rebound")));
            }
        } else {
            self.modules[module].bindings.insert(name.into(), binding);
        }
        Ok(())
    }

    fn import(&mut self, id: ModuleId, statement: &ast::Stmt) -> Result<(), String> {
        match statement {
            ast::Stmt::Import(import) => {
                if import.is_lazy {
                    return Err("lazy imports are unsupported".into());
                }
                for alias in &import.names {
                    let name = alias.name.as_str();
                    if let Some(module) = self.intrinsic_module(name)? {
                        let local = alias.asname.as_ref().unwrap_or(&alias.name);
                        self.bind(id, local.as_str(), Binding::IntrinsicModule(module), alias)?;
                        continue;
                    }
                    let leaf = self.load_module(name)?;
                    let (name, target) = if let Some(alias) = &alias.asname {
                        (alias.as_str(), leaf)
                    } else {
                        let first = name.split('.').next().unwrap();
                        (first, self.by_name[first])
                    };
                    self.bind(id, name, Binding::Module(target), alias)?;
                }
            }
            ast::Stmt::ImportFrom(import) => {
                if import.is_lazy {
                    return Err("lazy imports are unsupported".into());
                }
                if import.names.iter().any(|alias| alias.name.as_str() == "*") {
                    return Err("wildcard imports are unsupported".into());
                }
                let name = self.import_name(id, import)?;
                if import.level == 0
                    && let Some(module) = self.intrinsic_module(&name)?
                {
                    for alias in &import.names {
                        let binding = module.symbol(alias.name.as_str()).ok_or_else(|| {
                            format!(
                                "unsupported sealed standard-library export {name}.{}",
                                alias.name
                            )
                        })?;
                        let local = alias.asname.as_ref().unwrap_or(&alias.name);
                        self.bind(id, local.as_str(), binding, alias)?;
                    }
                    return Ok(());
                }
                // A package currently importing its children is a namespace container,
                // not an import cycle, unless an import reads its incomplete definitions.
                let target = match self.by_name.get(&name).copied() {
                    Some(target)
                        if self.loading.contains(&target) && self.modules[target].is_package =>
                    {
                        target
                    }
                    _ => self.load_module(&name)?,
                };
                for alias in &import.names {
                    let binding = if let Some(binding) = self.modules[target]
                        .bindings
                        .get(alias.name.as_str())
                        .copied()
                    {
                        if self.loading.contains(&target)
                            && matches!(binding, Binding::Definition(_))
                        {
                            return Err(format!("cyclic import through {name:?}"));
                        }
                        if let Binding::Module(child) = binding
                            && self.loading.contains(&child)
                        {
                            return Err(format!(
                                "cyclic import through {:?}",
                                self.modules[child].name
                            ));
                        }
                        binding
                    } else if self.modules[target].is_package {
                        Binding::Module(self.load_module(&format!("{name}.{}", alias.name))?)
                    } else {
                        return Err(format!(
                            "module {name:?} has no static binding {:?}",
                            alias.name.as_str()
                        ));
                    };
                    let local = alias.asname.as_ref().unwrap_or(&alias.name);
                    self.bind(id, local.as_str(), binding, alias)?;
                }
            }
            _ => unreachable!(),
        }
        Ok(())
    }

    /// Intrinsic imports use fixed identities, but never silently accept a
    /// first-party file that attempts to shadow the standard-library namespace.
    fn intrinsic_module(&self, name: &str) -> Result<Option<IntrinsicModule>, String> {
        let module = IntrinsicModule::named(name);
        if module.is_some()
            && self.allow_imports
            && (self.root.join(format!("{name}.py")).exists()
                || self.root.join(name).join("__init__.py").exists())
        {
            return Err(format!(
                "standard-library shadowing is unsupported: {name:?}"
            ));
        }
        Ok(module)
    }

    fn import_name(&self, id: ModuleId, import: &ast::StmtImportFrom) -> Result<String, String> {
        let suffix = import.module.as_ref().map_or("", |name| name.as_str());
        if import.level == 0 {
            return Ok(suffix.to_owned());
        }
        let module = &self.modules[id];
        let package = if module.is_package {
            module.name.as_str()
        } else {
            module
                .name
                .rsplit_once('.')
                .map_or("", |(parent, _)| parent)
        };
        let mut parts: Vec<_> = package.split('.').filter(|part| !part.is_empty()).collect();
        if import.level as usize > parts.len() {
            return Err("relative import goes beyond the first-party package root".into());
        }
        parts.truncate(parts.len() + 1 - import.level as usize);
        if !suffix.is_empty() {
            parts.push(suffix);
        }
        Ok(parts.join("."))
    }

    fn load_module(&mut self, name: &str) -> Result<ModuleId, String> {
        if !self.allow_imports {
            return Err("imports require a source file and a first-party module root".into());
        }
        if let Some(&id) = self.by_name.get(name) {
            if self.loading.contains(&id) {
                return Err(format!(
                    "cyclic import through {name:?} (the entry module cannot import itself)"
                ));
            }
            return Ok(id);
        }
        let first = name.split('.').next().unwrap_or("");
        let vendored = ty_vendored::file_system();
        if INTRINSIC_MODULES.contains(&first)
            || vendored.is_file(format!("stdlib/{first}.pyi").as_str())
            || vendored.is_file(format!("stdlib/{first}/__init__.pyi").as_str())
        {
            return Err(format!(
                "standard-library imports and shadowing are unsupported: {first:?}"
            ));
        }
        let parent = if let Some((parent, child)) = name.rsplit_once('.') {
            let id = match self.by_name.get(parent).copied() {
                Some(id) if self.loading.contains(&id) && self.modules[id].is_package => id,
                _ => self.load_module(parent)?,
            };
            if !self.modules[id].is_package {
                return Err(format!("{parent:?} is not a package"));
            }
            Some((id, child))
        } else {
            None
        };
        // Parent initialization can have loaded this child while exposing re-exports.
        if let Some(&id) = self.by_name.get(name) {
            if self.loading.contains(&id) {
                return Err(format!("cyclic import through {name:?}"));
            }
            return Ok(id);
        }
        let base = self.root.join(name.replace('.', "/"));
        let package = base.join("__init__.py");
        let (path, is_package) = if package.is_file() {
            (package, true)
        } else {
            (base.with_extension("py"), false)
        };
        if !path.is_file() {
            return Err(format!(
                "cannot resolve first-party module {name:?} under {}; external and namespace-package imports are unsupported",
                self.root.display()
            ));
        }
        let identity = path.canonicalize().map_err(|error| error.to_string())?;
        if !identity.starts_with(&self.root) {
            return Err(format!(
                "module {name:?} resolves outside the first-party module root"
            ));
        }
        if let Some(&previous) = self.by_path.get(&identity) {
            return Err(format!(
                "{} is already loaded as {:?}; a source file cannot have multiple module identities",
                path.display(),
                self.modules[previous].name
            ));
        }
        let source = std::fs::read_to_string(&path)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        // ty resolves logical import paths; canonical identities enforce containment
        // and uniqueness without breaking a single internal symlink's import name.
        let id = self.add(path, identity, name.to_owned(), is_package, &source)?;
        self.scan(id)?;
        if let Some((parent, child)) = parent {
            let binding = Binding::Module(id);
            if let Some(previous) = self.modules[parent].bindings.get(child)
                && *previous != binding
            {
                return Err(format!(
                    "importing {name:?} would overwrite a fixed package binding"
                ));
            }
            self.modules[parent]
                .bindings
                .insert(child.to_owned(), binding);
        }
        Ok(id)
    }
}

/// Only this side-effect-free false guard is allowed in an imported library module.
fn main_guard(statement: &ast::Stmt) -> bool {
    let ast::Stmt::If(statement) = statement else {
        return false;
    };
    let ast::Expr::Compare(compare) = &*statement.test else {
        return false;
    };
    statement.elif_else_clauses.is_empty()
        && compare.ops.as_ref() == [ast::CmpOp::Eq]
        && matches!(compare.operands.as_ref(), [ast::Expr::Name(name), ast::Expr::StringLiteral(value)]
            if name.id == "__name__" && value.value.to_str() == "__main__")
}
