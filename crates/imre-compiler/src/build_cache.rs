//! Private native executable and module object cache for supported Clang 18 toolchains.
use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::ffi::OsStr;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use imre_compiler::{CodegenUnit, SourceInputs};
use serde::{Deserialize, Serialize};
use tempfile::{NamedTempFile, TempDir};

#[derive(Serialize, Deserialize, PartialEq)]
struct Stamp {
    resolved: PathBuf,
    size: u64,
    modified: std::time::SystemTime,
    #[cfg(unix)]
    identity: (u64, u64, i64, i64),
}

impl Stamp {
    fn optional(path: &Path) -> Option<Option<Self>> {
        if path.try_exists().ok()? {
            Some(Some(Self::read(path)?))
        } else {
            Some(None)
        }
    }

    fn read(path: &Path) -> Option<Self> {
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;
        let m = path.metadata().ok()?;
        Some(Self {
            resolved: path.canonicalize().ok()?,
            size: m.len(),
            modified: m.modified().ok()?,
            #[cfg(unix)]
            identity: (m.dev(), m.ino(), m.ctime(), m.ctime_nsec()),
        })
    }
}

#[derive(Serialize, Deserialize)]
struct Manifest {
    sources: SourceInputs,
    native: Vec<(PathBuf, Option<Stamp>)>,
    search_dirs: Vec<PathBuf>,
    executable: String,
}

pub struct Cache {
    pub root: PathBuf,
    manifest: PathBuf,
    object_recipe: blake3::Hash,
    toolchain: NativeToolchain,
    runtime: PathBuf,
    previous: Option<Manifest>,
}

#[derive(Clone)]
pub struct NativeToolchain {
    pub clang: PathBuf,
    pub lld: Option<PathBuf>,
    bundle: Option<PathBuf>,
}

impl NativeToolchain {
    pub fn bundled() -> Option<Self> {
        if !cfg!(target_os = "linux") {
            return None;
        }
        let root = env::current_exe().ok()?.with_file_name("imre-toolchain");
        let clang = root.join("bin/clang");
        clang.is_file().then(|| Self {
            clang,
            lld: Some(root.join("bin/ld.lld")),
            bundle: Some(root),
        })
    }

    pub fn driver(&self) -> Command {
        let mut command = Command::new(&self.clang);
        if self.bundle.is_some() {
            command.arg("--no-default-config");
        }
        command
    }

    pub fn native_link_paths(&self, command: &mut Command) {
        if let Some(root) = &self.bundle {
            let link = root.join("link");
            command.arg("-B").arg(&link).arg("-L").arg(link);
        }
    }

    fn linker(&self) -> &Path {
        self.lld.as_deref().unwrap_or(Path::new("/usr/bin/ld"))
    }
}

/// On hosts without distro metadata, ask the ELF interpreter which libraries
/// the bundled tools actually load. The loader exits before entering the tool.
fn stamp_loaded_libraries(tool: &Path, stamps: &mut BTreeMap<PathBuf, Stamp>) -> Option<()> {
    let trace = Command::new(tool)
        .env("LD_TRACE_LOADED_OBJECTS", "1")
        .output()
        .ok()?;
    if !trace.status.success() {
        return None;
    }
    let mut saw_interpreter = false;
    for line in std::str::from_utf8(&trace.stdout).ok()?.lines() {
        let line = line.trim_start();
        if line.starts_with("linux-vdso.so.") {
            continue;
        }
        let (name, _) = line.rsplit_once(" (0x")?;
        let path = match name.split_once(" => ") {
            Some((_, path)) => Path::new(path),
            None => {
                saw_interpreter = true;
                Path::new(name)
            }
        };
        if !path.is_absolute() {
            return None;
        }
        let stamp = Stamp::read(path)?;
        stamps.insert(stamp.resolved.clone(), stamp);
    }
    saw_interpreter.then_some(())
}

pub fn trace_enabled() -> bool {
    env::var_os("IMRE_CACHE_TRACE").as_deref() == Some(OsStr::new("1"))
}

pub fn trace(event: &str) {
    if trace_enabled() {
        eprintln!("imre cache {event}");
    }
}

fn file_hash(path: &Path) -> io::Result<String> {
    let mut hasher = blake3::Hasher::new();
    hasher.update_reader(fs::File::open(path)?)?;
    Ok(hasher.finalize().to_hex().to_string())
}

fn ordinary_archive(path: &Path) -> bool {
    let mut header = [0; 8];
    fs::File::open(path)
        .and_then(|mut f| f.read_exact(&mut header))
        .is_ok()
        && &header == b"!<arch>\n"
}

/// LLD quotes dependency filenames for Make. BFD writes the filenames literally.
fn lld_dependency_name(name: &str) -> Option<String> {
    let mut path = String::with_capacity(name.len());
    let mut chars = name.chars();
    while let Some(c) = chars.next() {
        match c {
            ' ' => {
                // Each preceding literal backslash is doubled before the space's escape.
                let slashes = path.len() - path.trim_end_matches('\\').len();
                if slashes.is_multiple_of(2) {
                    return None;
                }
                path.truncate(path.len() - slashes.div_ceil(2));
            }
            '#' if path.pop() != Some('\\') => return None,
            '$' if chars.next() != Some('$') => return None,
            _ => {}
        }
        path.push(c);
    }
    Some(path)
}

fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = NamedTempFile::new_in(
        path.parent()
            .ok_or_else(|| io::Error::other("missing parent"))?,
    )?;
    file.write_all(bytes)?;
    file.persist(path)?;
    Ok(())
}

/// An action index points to a verified object. Publishing the bytes first
/// lets parallel builds safely finish the same action in either order.
struct ObjectEntry {
    directory: PathBuf,
    index: PathBuf,
}

impl ObjectEntry {
    fn restore(&self, staged: &Path) -> Option<()> {
        let expected = blake3::Hash::from_hex(fs::read_to_string(&self.index).ok()?).ok()?;
        let bytes = fs::read(self.directory.join(format!("{}.o", expected.to_hex()))).ok()?;
        if blake3::hash(&bytes) != expected {
            return None;
        }
        fs::write(staged, bytes).ok()
    }

    fn publish(&self, staged: &Path) -> io::Result<()> {
        let bytes = fs::read(staged)?;
        let digest = blake3::hash(&bytes).to_hex();
        atomic_write(&self.directory.join(format!("{digest}.o")), &bytes)?;
        atomic_write(&self.index, digest.as_bytes())
    }
}

pub fn compile_objects(
    units: &[CodegenUnit],
    toolchain: &NativeToolchain,
    opt: &OsStr,
    stage: &Path,
    cache: Option<&Cache>,
    mut progress: impl FnMut(&str, bool),
) -> Result<Vec<PathBuf>, String> {
    let store = cache.and_then(|c| {
        let dir = c.root.join("objects");
        fs::create_dir_all(&dir).ok()?;
        Some((dir, c.object_recipe))
    });
    let ir_path = stage.join("module.ll");
    let mut objects = Vec::with_capacity(units.len());
    for (i, unit) in units.iter().enumerate() {
        let filename = format!("unit-{i}.o");
        let staged = stage.join(&filename);
        let entry = store.as_ref().map(|(dir, recipe)| {
            // The IR includes the ABI, layouts, helpers and whole-graph analysis
            // decisions that affect this unit. Source bytes alone are insufficient.
            let mut action = blake3::Hasher::new_keyed(recipe.as_bytes());
            action.update(&(unit.name.len() as u64).to_le_bytes());
            action.update(unit.name.as_bytes());
            action.update(unit.llvm_ir.as_bytes());
            ObjectEntry {
                directory: dir.clone(),
                index: dir.join(format!("{}.key", action.finalize().to_hex())),
            }
        });
        let hit = entry.as_ref().is_some_and(|e| e.restore(&staged).is_some());
        progress(&unit.name, hit);
        if cache.is_some() && trace_enabled() {
            eprintln!(
                "imre object {} {}",
                if hit { "hit" } else { "miss" },
                unit.name
            );
        }
        if !hit {
            fs::write(&ir_path, &unit.llvm_ir).map_err(|e| e.to_string())?;
            let status = toolchain
                .driver()
                .current_dir(stage)
                .arg(format!("-O{}", opt.to_string_lossy()))
                .args([
                    "-Wno-override-module",
                    "-c",
                    "-x",
                    "ir",
                    "module.ll",
                    "-o",
                    &filename,
                ])
                .status()
                .map_err(|e| format!("Could not compile {}: {e}", unit.name))?;
            if !status.success() {
                return Err(format!(
                    "LLVM compilation of {} failed: {status}",
                    unit.name
                ));
            }
            if let Some(entry) = entry {
                let _ = entry.publish(&staged);
            }
        }
        objects.push(staged);
    }
    Ok(objects)
}

/// Preserve output symlinks, and keep writable user outputs independent of the cache.
pub fn materialize(from: &Path, output: &Path) -> io::Result<()> {
    let target = super::identity(output).map_err(io::Error::other)?;
    let temp = NamedTempFile::new_in(
        target
            .parent()
            .ok_or_else(|| io::Error::other("no output parent"))?,
    )?;
    fs::copy(from, temp.path())?;
    temp.persist(target)?;
    Ok(())
}

/// Decide native codegen independently of whether the user permits cache writes.
pub fn native_toolchain(
    runtime: &Path,
    bundled: Option<&NativeToolchain>,
) -> Option<NativeToolchain> {
    if !ordinary_archive(runtime)
        || (!cfg!(all(target_os = "linux", target_arch = "x86_64")) && bundled.is_none())
    {
        return None;
    }
    // Custom search paths, injected Clang flags, and user configs may introduce
    // arbitrary link inputs. Leave those invocations on the ordinary build path.
    if env::vars_os().any(|(k, _)| {
        let k = k.to_string_lossy();
        k.starts_with("LD_")
            || k.starts_with("CCC_")
            || k.starts_with("CLANG_")
            || matches!(
                k.as_ref(),
                "COMPILER_PATH"
                    | "GCC_EXEC_PREFIX"
                    | "LIBRARY_PATH"
                    | "SDKROOT"
                    | "LDEMULATION"
                    | "GNUTARGET"
                    | "CPATH"
                    | "SOURCE_DATE_EPOCH"
            )
    }) {
        return None;
    }
    if let Some(bundled) = bundled {
        return Some(bundled.clone());
    }
    let path = env::var_os("PATH")?;
    let clang = env::split_paths(&path)
        .map(|p| p.join("clang-18"))
        .find(|p| p.is_file())?;
    if clang.canonicalize().ok()? != Path::new("/usr/bin/clang-18").canonicalize().ok()? {
        return None;
    }
    let config = env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    if config.join("clang").exists() || Path::new("/etc/clang").exists() {
        return None;
    }
    let linker = Path::new("/usr/bin/ld").canonicalize().ok()?;
    if !linker.file_name()?.to_str()?.ends_with("ld.bfd")
        || !Path::new("/var/lib/dpkg/status").is_file()
    {
        return None;
    }
    let compiler = env::current_exe().ok()?;
    // Also support a standalone linker beside Imre, as in earlier releases.
    let lld = [
        compiler.with_file_name("imre-ld.lld"),
        PathBuf::from("/usr/bin/ld.lld-18"),
    ]
    .into_iter()
    .find(|path| path.is_file());
    Some(NativeToolchain {
        clang: PathBuf::from("/usr/bin/clang-18"),
        lld,
        bundle: None,
    })
}

impl Cache {
    pub fn open(
        entry: (&Path, &imre_compiler::CompileOptions),
        module_root: Option<&Path>,
        opt: &OsStr,
        runtime: &Path,
        toolchain: &NativeToolchain,
        module_objects: bool,
        link: &crate::cli::LinkOptions,
    ) -> Option<Self> {
        let path = env::var_os("PATH")?;
        let module_root = match module_root {
            Some(p) => Some(p.canonicalize().ok()?),
            None => None,
        };
        let runtime = Stamp::read(runtime)?;
        let dpkg = Stamp::optional(Path::new("/var/lib/dpkg/status"))?;
        let loader_cache = Stamp::optional(Path::new("/etc/ld.so.cache"))?;
        // Updating Clang's private shared libraries can change native code even
        // if the driver executable and the host package database stay unchanged.
        let mut compiler_libraries = BTreeMap::new();
        if let Some(root) = &toolchain.bundle {
            for library in fs::read_dir(root.join("lib")).ok()? {
                let library = library.ok()?.path();
                if library.is_file() {
                    let stamp = Stamp::read(&library)?;
                    compiler_libraries.insert(library, stamp);
                }
            }
            if dpkg.is_none() || loader_cache.is_none() {
                stamp_loaded_libraries(&toolchain.clang, &mut compiler_libraries)?;
                stamp_loaded_libraries(toolchain.linker(), &mut compiler_libraries)?;
            }
        } else if dpkg.is_none() || loader_cache.is_none() {
            return None;
        }
        let (entry, options) = entry;
        let command_hash = options
            .command
            .as_ref()
            .map(|command| blake3::hash(command.source.as_bytes()).to_hex().to_string());
        let file = if command_hash.is_none() {
            Some(entry.canonicalize().ok()?)
        } else {
            None
        };
        let recipe = serde_json::to_vec(&(
            4,
            module_objects,
            (
                file,
                command_hash,
                options.python_startup(),
                options.runtime_imports,
                &options.extension_paths,
            ),
            env::current_dir().ok()?.join(entry).into_os_string(),
            module_root,
            opt.to_str()?,
            path.to_str()?,
            Stamp::read(&env::current_exe().ok()?)?,
            &runtime,
            Stamp::read(&toolchain.clang)?,
            compiler_libraries,
            dpkg,
            loader_cache,
        ))
        .ok()?;
        let base = env::var_os("IMRE_CACHE_DIR")
            .map(PathBuf::from)
            .or_else(|| env::var_os("XDG_CACHE_HOME").map(|v| PathBuf::from(v).join("imre")))
            .or_else(|| env::var_os("HOME").map(|v| PathBuf::from(v).join(".cache/imre")))?;
        if !base.is_absolute() {
            return None;
        }
        let root = base.join("v1");
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::{DirBuilderExt, MetadataExt};
            builder.mode(0o700).create(&root).ok()?;
            let m = fs::symlink_metadata(&root).ok()?;
            if !m.is_dir()
                || m.mode() & 0o077 != 0
                || m.uid() != fs::metadata("/proc/self").ok()?.uid()
            {
                return None;
            }
        }
        let root = root.canonicalize().ok()?;
        let object_recipe = blake3::hash(&recipe);
        // Both switches only affect the final link. Equivalent flags from the
        // environment and the command line lead to the same executable.
        let relro = match link.relro {
            crate::cli::Relro::Full => 0,
            crate::cli::Relro::Partial => 1,
            crate::cli::Relro::Off => 2,
        };
        let link_inputs =
            serde_json::to_vec(&(Stamp::read(toolchain.linker())?, link.keep_dead_code, relro))
                .ok()?;
        let link_recipe = blake3::keyed_hash(object_recipe.as_bytes(), &link_inputs);
        let manifest = root.join(format!("{}.json", link_recipe.to_hex()));
        let previous = fs::read(&manifest)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Manifest>(&bytes).ok())
            .filter(|m| m.native.iter().all(|(p, stamp)| Stamp::read(p) == *stamp));
        Some(Self {
            root,
            manifest,
            object_recipe,
            toolchain: toolchain.clone(),
            runtime: runtime.resolved,
            previous,
        })
    }

    pub fn lookup(&self) -> Option<(PathBuf, &SourceInputs)> {
        let m = self.previous.as_ref()?;
        if !m.sources.fresh() || blake3::Hash::from_hex(&m.executable).is_err() {
            return None;
        }
        let file = self.root.join(&m.executable);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if file.metadata().ok()?.permissions().mode() & 0o100 == 0 {
                return None;
            }
        }
        (file_hash(&file).ok()? == m.executable).then_some((file, &m.sources))
    }

    pub fn stage(&self) -> Option<TempDir> {
        if self.previous.is_none() {
            let linker = self.toolchain.linker();
            if self.toolchain.lld.is_none() {
                let probe = Command::new(&self.toolchain.clang)
                    .arg("-print-prog-name=ld")
                    .output()
                    .ok()?;
                let selected = Path::new(std::str::from_utf8(&probe.stdout).ok()?.trim());
                if selected.canonicalize().ok()? != linker.canonicalize().ok()? {
                    return None;
                }
            }
            let help = Command::new(linker).arg("--help").output().ok()?;
            if !help.status.success()
                || !String::from_utf8_lossy(&help.stdout).contains("--dependency-file")
            {
                return None;
            }
        }
        tempfile::Builder::new()
            .prefix("compile-")
            .tempdir_in(&self.root)
            .ok()
    }

    pub fn publish(
        &self,
        sources: SourceInputs,
        stage: &Path,
        file: &Path,
        materialized_output: &Path,
    ) -> Option<()> {
        // LLD's Make depfile normalizes POSIX backslashes into separators.
        // Its object trace preserves those names, including archive members.
        let lld = self.toolchain.lld.is_some();
        let bundled_link_dir = self.toolchain.bundle.as_ref().map(|root| root.join("link"));
        let cwd = env::current_dir().ok()?;
        if lld
            && (cwd.as_os_str().as_encoded_bytes().contains(&b'\\')
                || fs::read(stage.join("link.trace")).ok()?.contains(&b'\\'))
        {
            return None;
        }
        // Both linkers list one input per continued line in their first stanza. Only the
        // compiler's private staging objects may disappear after linking.
        let deps = fs::read_to_string(stage.join("link.d")).ok()?;
        let mut paths = BTreeSet::new();
        let mut temporary_object = false;
        for line in deps.split("\n\n").next()?.lines().skip(1) {
            let name = line.trim_start();
            let name = name.strip_suffix(" \\").unwrap_or(name);
            let p = if lld {
                std::path::absolute(lld_dependency_name(name)?)
            } else {
                std::path::absolute(name)
            }
            .ok()?;
            if p.starts_with(stage) && p.extension().and_then(OsStr::to_str) == Some("o") {
                temporary_object = true;
                continue;
            }
            if !p.is_file() || (p.extension() == Some(OsStr::new("a")) && !ordinary_archive(&p)) {
                return None;
            }
            // LLD omits linker scripts and INCLUDEs from its object trace. A
            // custom script can introduce a dependency whose name was normalized.
            if lld {
                let resolved = p.canonicalize().ok()?;
                let supported = |path: &Path| {
                    ["/usr/lib", "/usr/lib64", "/lib", "/lib64"]
                        .iter()
                        .any(|dir| path.starts_with(dir))
                        || bundled_link_dir
                            .as_ref()
                            .is_some_and(|dir| path.starts_with(dir))
                };
                let cwd_symlink = p.parent() == Some(cwd.as_path()) && p.is_symlink();
                if resolved != self.runtime
                    && !(supported(&resolved) && (supported(&p) || cwd_symlink))
                {
                    return None;
                }
            }
            // Linker scripts can look up a bare filename in the caller's working
            // directory before finding the library recorded in the depfile.
            paths.insert(PathBuf::from(p.file_name()?));
            paths.insert(p);
        }
        if !temporary_object || paths.is_empty() {
            return None;
        }
        // An added higher-priority library or a newly selected GCC installation
        // must cause a miss, even if the previously linked files still exist.
        let search_dirs = match &self.previous {
            Some(previous) => previous.search_dirs.clone(),
            None => {
                let mut driver = self.toolchain.driver();
                self.toolchain.native_link_paths(&mut driver);
                let search = driver.arg("-print-search-dirs").output().ok()?;
                let search = std::str::from_utf8(&search.stdout).ok()?;
                let libraries = search
                    .lines()
                    .find_map(|s| s.strip_prefix("libraries: ="))?;
                env::split_paths(libraries).collect()
            }
        };
        paths.extend(search_dirs.iter().cloned());
        paths.insert(Path::new("/usr/lib/gcc").join(format!("{}-linux-gnu", env::consts::ARCH)));
        if let Some(dir) = bundled_link_dir {
            paths.insert(dir);
        }
        let output = materialized_output.canonicalize().ok()?;
        let mut native = Vec::with_capacity(paths.len());
        for path in paths {
            let stamp = Stamp::read(&path);
            // Installing the output can itself create a working-directory
            // shadow. Its post-link stamp cannot describe this build.
            if stamp.as_ref().is_some_and(|s| s.resolved == output) {
                return None;
            }
            native.push((path, stamp));
        }
        if !sources.fresh() {
            return None;
        }
        let executable = file_hash(file).ok()?;
        let object = self.root.join(&executable);
        fs::rename(file, &object).ok()?;
        let record = Manifest {
            sources,
            native,
            search_dirs,
            executable,
        };
        let mut temp = NamedTempFile::new_in(&self.root).ok()?;
        serde_json::to_writer(&mut temp, &record).ok()?;
        temp.flush().ok()?;
        temp.persist(&self.manifest).ok()?;
        Some(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_package_metadata_is_distinct_from_present_metadata() {
        let root = tempfile::tempdir().unwrap();
        let database = root.path().join("status");
        assert!(Stamp::optional(&database).unwrap().is_none());
        fs::write(&database, "").unwrap();
        assert!(Stamp::optional(&database).unwrap().is_some());
    }
}
