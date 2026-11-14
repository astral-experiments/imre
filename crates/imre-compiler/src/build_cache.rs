//! Private native executable and module object cache for supported Clang 18 toolchains.
use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::ffi::OsStr;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;

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

#[derive(Serialize, Deserialize)]
struct ManifestRecord {
    #[serde(flatten)]
    manifest: Manifest,
    checksum: String,
}

impl Manifest {
    // Bind the complete dependency list and executable to the request that produced them.
    fn checksum(&self, link_recipe: &blake3::Hash) -> Option<String> {
        let mut hasher = blake3::Hasher::new_keyed(link_recipe.as_bytes());
        serde_json::to_writer(&mut hasher, self).ok()?;
        Some(hasher.finalize().to_hex().to_string())
    }
}

pub struct Cache {
    pub root: PathBuf,
    manifest: PathBuf,
    object_recipe: blake3::Hash,
    link_recipe: blake3::Hash,
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
        let root = super::installation_file("imre-toolchain").ok()?;
        let clang = root.join("bin/clang");
        clang.is_file().then(|| Self {
            clang,
            lld: Some(root.join("bin/ld.lld")),
            bundle: Some(root),
        })
    }

    pub fn provider_archive(&self, optimization: &OsStr) -> Option<PathBuf> {
        let root = self.bundle.as_ref()?;
        let path = root
            .join("lib/imre-stdlib")
            .join(format!("O{}", optimization.to_str()?))
            .join("providers.a");
        path.is_file().then_some(path)
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

/// Locate the compiler's source resource independently of Cargo and executable caches.
#[cfg(unix)]
pub(super) fn stdlib_archive_path() -> Result<PathBuf, String> {
    // SAFETY: geteuid takes no arguments and has no failure mode.
    let uid = unsafe { libc::geteuid() };
    let digest = blake3::hash(super::packaged_stdlib_zip()).to_hex();
    Ok(std::path::absolute(env::temp_dir())
        .map_err(|e| e.to_string())?
        .join(format!("imre-stdlib-{uid}-{digest}"))
        .join("imre-stdlib.zip"))
}

/// Publish the exact embedded bytes into a private directory; races publish identical data.
#[cfg(unix)]
pub(super) fn prepare_stdlib_archive(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt};
    let parent = path
        .parent()
        .expect("stdlib archive has a parent directory");
    match fs::DirBuilder::new().mode(0o700).create(parent) {
        Ok(()) => {}
        Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {}
        Err(err) => return Err(err),
    }
    let metadata = parent.symlink_metadata()?;
    if !metadata.is_dir()
        || metadata.mode() & 0o077 != 0
        // SAFETY: geteuid takes no arguments and has no failure mode.
        || metadata.uid() != unsafe { libc::geteuid() }
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "unsafe stdlib resource directory",
        ));
    }
    if path
        .symlink_metadata()
        .is_ok_and(|m| m.is_file() && m.len() == super::packaged_stdlib_zip().len() as u64)
        && fs::read(path)? == super::packaged_stdlib_zip()
    {
        return Ok(());
    }
    atomic_write(path, super::packaged_stdlib_zip())
}

/// An action index points to a verified object. Publishing the bytes first
/// lets parallel builds safely finish the same action in either order.
struct ObjectEntry {
    directory: PathBuf,
    action: blake3::Hash,
}

impl ObjectEntry {
    fn index(&self) -> PathBuf {
        self.directory.join(format!("{}.key", self.action))
    }

    // Bind the blob digest to this action, even if a complete record is misplaced.
    fn record(&self, digest: blake3::Hash) -> String {
        let check = blake3::keyed_hash(self.action.as_bytes(), digest.as_bytes());
        format!("{digest} {check}")
    }

    fn restore(&self, staged: &Path) -> Option<()> {
        let record = fs::read_to_string(self.index()).ok()?;
        let (digest, _) = record.split_once(' ')?;
        let expected = blake3::Hash::from_hex(digest).ok()?;
        if record != self.record(expected) {
            return None;
        }
        let bytes = fs::read(self.directory.join(format!("{}.o", expected.to_hex()))).ok()?;
        if blake3::hash(&bytes) != expected {
            return None;
        }
        fs::write(staged, bytes).ok()
    }

    fn publish(&self, staged: &Path) -> io::Result<()> {
        let bytes = fs::read(staged)?;
        let digest = blake3::hash(&bytes);
        atomic_write(&self.directory.join(format!("{digest}.o")), &bytes)?;
        atomic_write(&self.index(), self.record(digest).as_bytes())
    }
}

pub fn compile_objects(
    units: &[CodegenUnit],
    driver: impl Fn() -> Command + Sync,
    opt: &OsStr,
    sanitize: bool,
    stage: &Path,
    cache: Option<&Cache>,
    mut progress: impl FnMut(&str, bool),
) -> Result<Vec<PathBuf>, String> {
    let store = cache.and_then(|c| {
        let dir = c.root.join("objects");
        fs::create_dir_all(&dir).ok()?;
        Some((dir, c.object_recipe))
    });
    let mut pending = Vec::new();
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
                action: action.finalize(),
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
            pending.push((i, entry));
        }
        objects.push(staged);
    }

    // Start the largest units first so they can overlap the smaller compilations.
    pending.sort_unstable_by_key(|(i, _)| std::cmp::Reverse(units[*i].llvm_ir.len()));
    let workers = thread::available_parallelism()
        .map_or(1, usize::from)
        .min(4)
        .min(pending.len());
    let next = AtomicUsize::new(0);
    let compile = || {
        while let Some((i, entry)) = pending.get(next.fetch_add(1, Ordering::Relaxed)) {
            let object = &objects[*i];
            if let Err(err) = compile_object(&units[*i], driver(), opt, sanitize, object) {
                next.store(pending.len(), Ordering::Relaxed);
                return Err(err);
            }
            if let Some(entry) = entry {
                let _ = entry.publish(object);
            }
        }
        Ok(())
    };
    thread::scope(|scope| {
        let handles: Vec<_> = (0..workers).map(|_| scope.spawn(compile)).collect();
        for handle in handles {
            handle.join().expect("codegen worker panicked")?;
        }
        Ok::<(), String>(())
    })?;
    Ok(objects)
}

/// Compile one LLVM module through the selected driver, keeping its IR in memory.
fn compile_object(
    unit: &CodegenUnit,
    mut driver: Command,
    opt: &OsStr,
    sanitize: bool,
    output: &Path,
) -> Result<(), String> {
    let error = |e| format!("Could not compile {}: {e}", unit.name);
    if sanitize {
        driver.args(["-fsanitize=address", "-fno-omit-frame-pointer"]);
    }
    let mut child = driver
        .arg(format!("-O{}", opt.to_string_lossy()))
        .args(["-Wno-override-module", "-c", "-x", "ir", "-", "-o"])
        .arg(output)
        .stdin(Stdio::piped())
        .spawn()
        .map_err(error)?;
    let input = child
        .stdin
        .take()
        .expect("piped clang input")
        .write_all(unit.llvm_ir.as_bytes());
    let status = child.wait().map_err(error)?;
    if !status.success() {
        return Err(format!(
            "LLVM compilation of {} failed: {status}",
            unit.name
        ));
    }
    input.map_err(error)
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
    // Also support a standalone linker beside Imre, as in earlier releases.
    let lld = [
        super::installation_file("imre-ld.lld").ok()?,
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
        let file = if command_hash.is_none() && options.module_entry.is_none() {
            Some(entry.canonicalize().ok()?)
        } else {
            None
        };
        // A unit's full IR captures entry-specific codegen decisions. Identical
        // IR can share an object across invocations using the same native tools.
        let object_inputs = serde_json::to_vec(&(
            1,
            (env::consts::ARCH, env::consts::OS),
            opt.to_str()?,
            path.to_str()?,
            Stamp::read(&env::current_exe().ok()?)?,
            &runtime,
            Stamp::read(&toolchain.clang)?,
            toolchain.bundle.is_some(),
            compiler_libraries,
            dpkg,
            loader_cache,
        ))
        .ok()?;
        let object_recipe = blake3::hash(&object_inputs);
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
        // Executable lookup runs before IR generation, so it also needs the
        // source query, startup configuration, and final link inputs.
        let relro = match link.relro {
            crate::cli::Relro::Full => 0,
            crate::cli::Relro::Partial => 1,
            crate::cli::Relro::Off => 2,
        };
        // An unreadable or incomplete index can select local provider bodies without
        // placing the archive in the linker's dependency file. Its identity still
        // determines whether this executable can be reused.
        let provider_archive = match &options.bundled_provider_archive {
            Some(path) => Some(Stamp::read(path)?),
            None => None,
        };
        let link_inputs = serde_json::to_vec(&(
            6,
            provider_archive,
            module_objects,
            (
                file,
                command_hash,
                &options.module_entry,
                options.path_entry,
                options.python_startup(),
                (&options.extension_paths, &options.pythonpath),
                &options.include_modules,
                &options.python_archive_indices,
                (&options.stdlib_dir, &options.site_dirs),
                (&options.user_site_pth, &options.venv_site_pth),
                (options.sitecustomize, options.stdlib_sitecustomize),
                (&options.stdlib_fallback, &options.installed_stdlib_archive),
            ),
            env::current_dir().ok()?.join(entry).into_os_string(),
            module_root,
            Stamp::read(toolchain.linker())?,
            link.keep_dead_code,
            relro,
        ))
        .ok()?;
        let link_recipe = blake3::keyed_hash(object_recipe.as_bytes(), &link_inputs);
        let manifest = root.join(format!("{}.json", link_recipe.to_hex()));
        let previous = fs::read(&manifest)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<ManifestRecord>(&bytes).ok())
            .and_then(|record| {
                (record.manifest.checksum(&link_recipe)? == record.checksum)
                    .then_some(record.manifest)
            })
            .filter(|m| m.native.iter().all(|(p, stamp)| Stamp::read(p) == *stamp));
        Some(Self {
            root,
            manifest,
            object_recipe,
            link_recipe,
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
        extension_archives: &[PathBuf],
        bundled_provider_archive: Option<&Path>,
    ) -> Option<()> {
        // A wheel symlink's physical target need not have an archive suffix.
        if extension_archives
            .iter()
            .any(|path| !ordinary_archive(path))
        {
            return None;
        }
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
                // Ordinary archives explicitly selected by the compiler have known identities.
                let is_provider = (bundled_provider_archive == Some(p.as_path())
                    || extension_archives.contains(&p))
                    && ordinary_archive(&resolved);
                if resolved != self.runtime
                    && !is_provider
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
        let manifest = Manifest {
            sources,
            native,
            search_dirs,
            executable,
        };
        let record = ManifestRecord {
            checksum: manifest.checksum(&self.link_recipe)?,
            manifest,
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
