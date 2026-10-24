//! Loads a closed graph of first-party modules and orders their initialization.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use ruff_db::parsed::ParsedModuleRef;
use ruff_python_ast as ast;
use ruff_text_size::Ranged;

use crate::analysis::Session;
use crate::ir::PosixPathOperation;
use crate::watch::FileSystem;

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
    Value(DefinitionId),
    IntrinsicModule(IntrinsicModule),
    Intrinsic(Intrinsic),
}

/// Sealed namespaces expose only compiler-known symbols, never module values.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum IntrinsicModule {
    Argparse,
    Future,
    Sys,
    Typing,
    Dataclasses,
    Pathlib,
    Tempfile,
    Subprocess,
    Os,
    PosixPath,
    Urllib,
    UrllibParse,
    Collections,
    CollectionsAbc,
    Itertools,
    Textwrap,
    Io,
    Gzip,
    Hashlib,
    Re,
    Enum,
    Zlib,
    Json,
    Relar,
    RelarJson,
    RelarToml,
    Tomllib,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Intrinsic {
    ArgumentParser,
    RawDescriptionHelpFormatter,
    FutureAnnotations,
    Stderr,
    Argv,
    TextIO,
    BinaryIO,
    Union,
    Literal,
    Callable,
    Sequence,
    UnsupportedOperation,
    GzipOpen,
    Sha256,
    BadGzipFile,
    RegexCompile,
    RegexSearch,
    RegexVerbose,
    RegexPattern,
    RegexMatch,
    StrEnum,
    EnumAuto,
    DefaultDict,
    Counter,
    GroupBy,
    Chain,
    AbstractSet,
    TomllibLoad,
    TomlDecodeError,
    Dedent,
    ZlibError,
    JsonDecode,
    JsonEncode,
    JsonLoads,
    JsonDecodeError,
    Any,
    TomlDecode,
    Dataclass,
    DataclassField,
    SelfType,
    AssertNever,
    Path,
    TemporaryDirectory,
    SubprocessRun,
    CompletedProcess,
    SubprocessError,
    CalledProcessError,
    TimeoutExpired,
    Environ,
    GetEnv,
    GetCwd,
    FsPath,
    PosixPath(PosixPathOperation),
    Unquote,
}

impl IntrinsicModule {
    fn named(name: &str) -> Option<Self> {
        match name {
            "__future__" => Some(Self::Future),
            "argparse" => Some(Self::Argparse),
            "sys" => Some(Self::Sys),
            "typing" => Some(Self::Typing),
            "dataclasses" => Some(Self::Dataclasses),
            "pathlib" => Some(Self::Pathlib),
            "tempfile" => Some(Self::Tempfile),
            "subprocess" => Some(Self::Subprocess),
            "os" => Some(Self::Os),
            "os.path" | "posixpath" => Some(Self::PosixPath),
            "urllib" => Some(Self::Urllib),
            "urllib.parse" => Some(Self::UrllibParse),
            "collections" => Some(Self::Collections),
            "collections.abc" => Some(Self::CollectionsAbc),
            "itertools" => Some(Self::Itertools),
            "textwrap" => Some(Self::Textwrap),
            "io" => Some(Self::Io),
            "gzip" => Some(Self::Gzip),
            "hashlib" => Some(Self::Hashlib),
            "re" => Some(Self::Re),
            "enum" => Some(Self::Enum),
            "zlib" => Some(Self::Zlib),
            "json" => Some(Self::Json),
            "relar" => Some(Self::Relar),
            "relar.json" => Some(Self::RelarJson),
            "relar.toml" => Some(Self::RelarToml),
            "tomllib" => Some(Self::Tomllib),
            _ => None,
        }
    }

    fn symbol(self, name: &str) -> Option<Binding> {
        if self == Self::Os && name == "path" {
            return Some(Binding::IntrinsicModule(Self::PosixPath));
        }
        if self == Self::Urllib && name == "parse" {
            return Some(Binding::IntrinsicModule(Self::UrllibParse));
        }
        if self == Self::Collections && name == "abc" {
            return Some(Binding::IntrinsicModule(Self::CollectionsAbc));
        }
        if self == Self::Relar && name == "json" {
            return Some(Binding::IntrinsicModule(Self::RelarJson));
        }
        if self == Self::Relar && name == "toml" {
            return Some(Binding::IntrinsicModule(Self::RelarToml));
        }
        let symbol = match (self, name) {
            (Self::Future, "annotations") => Intrinsic::FutureAnnotations,
            (Self::Sys, "stderr") => Intrinsic::Stderr,
            (Self::Argparse, "ArgumentParser") => Intrinsic::ArgumentParser,
            (Self::Argparse, "RawDescriptionHelpFormatter") => {
                Intrinsic::RawDescriptionHelpFormatter
            }
            (Self::Sys, "argv") => Intrinsic::Argv,
            (Self::Typing, "TextIO") => Intrinsic::TextIO,
            (Self::Typing, "BinaryIO") => Intrinsic::BinaryIO,
            (Self::Typing, "Union") => Intrinsic::Union,
            (Self::Typing, "Literal") => Intrinsic::Literal,
            (Self::Typing, "Self") => Intrinsic::SelfType,
            (Self::Typing, "Any") => Intrinsic::Any,
            (Self::Typing, "assert_never") => Intrinsic::AssertNever,
            (Self::Typing | Self::CollectionsAbc, "Callable") => Intrinsic::Callable,
            (Self::Typing | Self::CollectionsAbc, "Sequence") => Intrinsic::Sequence,
            (Self::Io, "UnsupportedOperation") => Intrinsic::UnsupportedOperation,
            (Self::Gzip, "open") => Intrinsic::GzipOpen,
            (Self::Hashlib, "sha256") => Intrinsic::Sha256,
            (Self::Gzip, "BadGzipFile") => Intrinsic::BadGzipFile,
            (Self::Re, "compile") => Intrinsic::RegexCompile,
            (Self::Re, "search") => Intrinsic::RegexSearch,
            (Self::Re, "VERBOSE") => Intrinsic::RegexVerbose,
            (Self::Re, "Pattern") => Intrinsic::RegexPattern,
            (Self::Re, "Match") => Intrinsic::RegexMatch,
            (Self::Enum, "StrEnum") => Intrinsic::StrEnum,
            (Self::Enum, "auto") => Intrinsic::EnumAuto,
            (Self::Collections, "defaultdict") => Intrinsic::DefaultDict,
            (Self::Collections, "Counter") => Intrinsic::Counter,
            (Self::Itertools, "groupby") => Intrinsic::GroupBy,
            (Self::Itertools, "chain") => Intrinsic::Chain,
            (Self::CollectionsAbc, "Set") => Intrinsic::AbstractSet,
            (Self::Tomllib, "load") => Intrinsic::TomllibLoad,
            (Self::Tomllib, "TOMLDecodeError") => Intrinsic::TomlDecodeError,
            (Self::Textwrap, "dedent") => Intrinsic::Dedent,
            (Self::Zlib, "error") => Intrinsic::ZlibError,
            (Self::RelarJson, "decode") => Intrinsic::JsonDecode,
            (Self::Json, "loads") => Intrinsic::JsonLoads,
            (Self::Json, "JSONDecodeError") => Intrinsic::JsonDecodeError,
            (Self::RelarJson, "encode") => Intrinsic::JsonEncode,
            (Self::RelarToml, "decode") => Intrinsic::TomlDecode,
            (Self::Dataclasses, "dataclass") => Intrinsic::Dataclass,
            (Self::Dataclasses, "field") => Intrinsic::DataclassField,
            (Self::Pathlib, "Path") => Intrinsic::Path,
            (Self::Tempfile, "TemporaryDirectory") => Intrinsic::TemporaryDirectory,
            (Self::Subprocess, "run") => Intrinsic::SubprocessRun,
            (Self::Subprocess, "CompletedProcess") => Intrinsic::CompletedProcess,
            (Self::Subprocess, "SubprocessError") => Intrinsic::SubprocessError,
            (Self::Subprocess, "CalledProcessError") => Intrinsic::CalledProcessError,
            (Self::Subprocess, "TimeoutExpired") => Intrinsic::TimeoutExpired,
            (Self::Os, "environ") => Intrinsic::Environ,
            (Self::Os, "getenv") => Intrinsic::GetEnv,
            (Self::Os, "getcwd") => Intrinsic::GetCwd,
            (Self::Os, "fspath") => Intrinsic::FsPath,
            (Self::PosixPath, "join") => Intrinsic::PosixPath(PosixPathOperation::Join),
            (Self::PosixPath, "basename") => Intrinsic::PosixPath(PosixPathOperation::Basename),
            (Self::PosixPath, "dirname") => Intrinsic::PosixPath(PosixPathOperation::Dirname),
            (Self::PosixPath, "splitext") => Intrinsic::PosixPath(PosixPathOperation::Splitext),
            (Self::UrllibParse, "unquote") => Intrinsic::Unquote,
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
    /// Fixed pattern declarations in the entry module precede ordinary executable statements.
    pub(crate) entry_globals: Vec<usize>,
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
    pub(crate) inputs: crate::SourceInputs,
    pub(crate) session: Session,
    pub(crate) modules: Vec<SourceModule>,
    pub(crate) entry: ModuleId,
    /// Imports precede executable statements, so dependency completion order
    /// also gives Python's module initialization order.
    pub(crate) initialization: Vec<ModuleId>,
}

impl SourceGraph {
    /// The entry needs its own initializer when it declares globals for later definitions.
    pub(crate) fn initializer_modules(&self) -> impl Iterator<Item = ModuleId> + '_ {
        self.initialization
            .iter()
            .copied()
            .filter(|&id| id != self.entry || !self.modules[id].entry_globals.is_empty())
    }

    /// An isolated source API supports sealed imports only; first-party imports
    /// require the path API and its filesystem search root.
    pub(crate) fn single(source: &str) -> Result<Self, String> {
        let mut loader = Loader::new(PathBuf::from("/"), FileSystem::default())?;
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
        Self::load_with(entry, module_root, FileSystem::default())
    }

    pub(crate) fn load_with(
        entry: &Path,
        module_root: Option<&Path>,
        mut fs: FileSystem<'_>,
    ) -> Result<Self, String> {
        let entry = fs
            .canonicalize(entry)
            .map_err(|error| format!("{}: {error}", entry.display()))?;
        if entry.extension().and_then(|value| value.to_str()) != Some("py") {
            return Err("entry source must be a .py file".into());
        }
        if entry.file_name().and_then(|value| value.to_str()) == Some("__init__.py") {
            return Err("use a program module or package __main__.py as the entry source".into());
        }
        let inferred = module_root.is_none();
        let root = if let Some(root) = module_root {
            fs.canonicalize(root)
                .map_err(|error| format!("{}: {error}", root.display()))?
        } else {
            let mut root = entry
                .parent()
                .ok_or("entry source has no parent")?
                .to_path_buf();
            while fs.is_file(&root.join("__init__.py")) {
                root = root
                    .parent()
                    .ok_or("package has no module root")?
                    .to_path_buf();
            }
            root
        };
        if !fs.is_dir(&root) {
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
        let source = fs
            .read_to_string(&entry)
            .map_err(|error| format!("{}: {error}", entry.display()))?;
        let mut loader = Loader::new(root, fs)?;
        if inferred {
            loader.inputs.missing.push(loader.root.join("__init__.py"));
        }
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
        resolve_binding(&self.modules, module, expression)
    }
}

fn resolve_binding(
    modules: &[SourceModule],
    module: ModuleId,
    expression: &ast::Expr,
) -> Option<Binding> {
    match expression {
        ast::Expr::Name(name) => modules[module].bindings.get(name.id.as_str()).copied(),
        ast::Expr::Attribute(attribute) => {
            match resolve_binding(modules, module, &attribute.value)? {
                Binding::Module(module) => modules[module]
                    .bindings
                    .get(attribute.attr.as_str())
                    .copied(),
                Binding::IntrinsicModule(module) => module.symbol(attribute.attr.as_str()),
                Binding::Definition(_) | Binding::Value(_) | Binding::Intrinsic(_) => None,
            }
        }
        _ => None,
    }
}

struct Loader<'a> {
    fs: FileSystem<'a>,
    inputs: crate::SourceInputs,
    root: PathBuf,
    session: Session,
    modules: Vec<SourceModule>,
    by_name: HashMap<String, ModuleId>,
    by_path: HashMap<PathBuf, ModuleId>,
    loading: HashSet<ModuleId>,
    import_depth: usize,
    allow_imports: bool,
    initialization: Vec<ModuleId>,
}

impl<'a> Loader<'a> {
    fn new(root: PathBuf, fs: FileSystem<'a>) -> Result<Self, String> {
        Ok(Self {
            fs,
            session: Session::new(&root)?,
            inputs: crate::SourceInputs::default(),
            root,
            modules: Vec::new(),
            by_name: HashMap::new(),
            by_path: HashMap::new(),
            loading: HashSet::new(),
            import_depth: 0,
            allow_imports: true,
            initialization: Vec::new(),
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
        self.inputs.files.push((
            path.clone(),
            identity.clone(),
            blake3::hash(source.as_bytes()).to_hex().to_string(),
        ));
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
            entry_globals: Vec::new(),
            executable: Vec::new(),
        });
        Ok(id)
    }

    fn finish(mut self, entry: ModuleId) -> Result<SourceGraph, String> {
        self.inputs.missing.sort_unstable();
        self.inputs.missing.dedup();
        Ok(SourceGraph {
            inputs: self.inputs,
            session: self.session,
            modules: self.modules,
            entry,
            initialization: self.initialization,
        })
    }

    fn scan(&mut self, id: ModuleId) -> Result<(), String> {
        let parsed = self.modules[id].parsed.clone();
        let last_definition = parsed.syntax().body.iter().rposition(|statement| {
            matches!(
                statement,
                ast::Stmt::FunctionDef(_) | ast::Stmt::ClassDef(_)
            )
        });
        let mut executable = false;
        for (index, statement) in parsed.syntax().body.iter().enumerate() {
            match statement {
                ast::Stmt::FunctionDef(function) => {
                    if executable && id == 0 {
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
                    if executable && id == 0 {
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
                    if executable || !self.modules[id].entry_globals.is_empty() {
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
                    let entry_global = id == 0
                        && !executable
                        && (last_definition.is_some_and(|last| index < last)
                            || matches!(
                                module_value(statement),
                                Some(ast::Expr::Call(call)) if resolve_binding(&self.modules, id, &call.func) == Some(Binding::Intrinsic(Intrinsic::RegexCompile))
                            ));
                    if let Some(name) = value_name(statement)
                        && (id != 0 || entry_global)
                    {
                        self.bind(
                            id,
                            name,
                            Binding::Value(DefinitionId {
                                module: id,
                                statement: index,
                            }),
                            statement,
                        )?;
                        if entry_global {
                            self.modules[id].entry_globals.push(index);
                            continue;
                        }
                    }
                    executable = true;
                    self.modules[id].executable.push(index);
                }
            }
        }
        self.loading.remove(&id);
        self.initialization.push(id);
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
                        if alias.asname.is_none()
                            && let Some((local, _)) = name.split_once('.')
                            && let Some(namespace) = IntrinsicModule::named(local)
                        {
                            self.bind(id, local, Binding::IntrinsicModule(namespace), alias)?;
                            continue;
                        }
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
                            && matches!(binding, Binding::Definition(_) | Binding::Value(_))
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
    fn intrinsic_module(&mut self, name: &str) -> Result<Option<IntrinsicModule>, String> {
        let module = IntrinsicModule::named(name);
        if module.is_some() && self.allow_imports {
            for base in [
                self.root.join(name.split('.').next().unwrap()),
                self.root.join(name.replace('.', "/")),
            ] {
                for candidate in [base.with_extension("py"), base.join("__init__.py")] {
                    if self.fs.exists(&candidate) {
                        return Err(format!(
                            "standard-library shadowing is unsupported: {name:?}"
                        ));
                    }
                    self.inputs.missing.push(candidate);
                }
            }
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
        // Both scanning an import and resolving a dotted package parent can
        // re-enter the loader. Count active calls, including unresolved parents.
        if self.import_depth >= 96 {
            return Err("first-party import nesting exceeds 96 levels".into());
        }
        self.import_depth += 1;
        let result = self.load_new_module(name);
        self.import_depth -= 1;
        result
    }

    fn load_new_module(&mut self, name: &str) -> Result<ModuleId, String> {
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
        let (path, is_package) = if self.fs.is_file(&package) {
            (package, true)
        } else {
            self.inputs.missing.push(package);
            (base.with_extension("py"), false)
        };
        if !self.fs.is_file(&path) {
            return Err(format!(
                "cannot resolve first-party module {name:?} under {}; external and namespace-package imports are unsupported",
                self.root.display()
            ));
        }
        let identity = self
            .fs
            .canonicalize(&path)
            .map_err(|error| error.to_string())?;
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
        let source = self
            .fs
            .read_to_string(&path)
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

/// Module values use one unconditional, fixed binding; their contents may mutate.
pub(crate) fn value_name(statement: &ast::Stmt) -> Option<&str> {
    let target = match statement {
        ast::Stmt::Assign(assignment) if assignment.targets.len() == 1 => &assignment.targets[0],
        ast::Stmt::AnnAssign(assignment) => &assignment.target,
        _ => return None,
    };
    if let ast::Expr::Name(name) = target {
        Some(name.id.as_str())
    } else {
        None
    }
}

fn module_value(statement: &ast::Stmt) -> Option<&ast::Expr> {
    match statement {
        ast::Stmt::Assign(assignment) => Some(&assignment.value),
        ast::Stmt::AnnAssign(assignment) => assignment.value.as_deref(),
        _ => None,
    }
}
