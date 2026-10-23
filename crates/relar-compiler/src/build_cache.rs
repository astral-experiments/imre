//! Private native executable and module object cache for the system Clang 18 toolchain.
use std::collections::BTreeSet;
use std::env;
use std::ffi::OsStr;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use relar_compiler::{CodegenUnit, SourceInputs};
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
    clang: PathBuf,
    previous: Option<Manifest>,
}

pub fn trace_enabled() -> bool {
    env::var_os("RELAR_CACHE_TRACE").as_deref() == Some(OsStr::new("1"))
}

pub fn trace(event: &str) {
    if trace_enabled() {
        eprintln!("relar cache {event}");
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
    clang: &Path,
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
                "relar object {} {}",
                if hit { "hit" } else { "miss" },
                unit.name
            );
        }
        if !hit {
            fs::write(&ir_path, &unit.llvm_ir).map_err(|e| e.to_string())?;
            let status = Command::new(clang)
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
pub fn native_clang(runtime: &Path) -> Option<PathBuf> {
    if !cfg!(all(target_os = "linux", target_arch = "x86_64")) || !ordinary_archive(runtime) {
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
    Some(PathBuf::from("/usr/bin/clang-18"))
}

impl Cache {
    pub fn open(
        entry: &Path,
        module_root: Option<&Path>,
        opt: &OsStr,
        runtime: &Path,
        clang: &Path,
        module_objects: bool,
        link: &crate::cli::LinkOptions,
    ) -> Option<Self> {
        let path = env::var_os("PATH")?;
        let module_root = match module_root {
            Some(p) => Some(p.canonicalize().ok()?),
            None => None,
        };
        let recipe = serde_json::to_vec(&(
            3,
            module_objects,
            entry.canonicalize().ok()?,
            module_root,
            opt.to_str()?,
            path.to_str()?,
            Stamp::read(&env::current_exe().ok()?)?,
            Stamp::read(runtime)?,
            Stamp::read(clang)?,
            Stamp::read(Path::new("/usr/bin/ld"))?,
            Stamp::read(Path::new("/var/lib/dpkg/status"))?,
            Stamp::read(Path::new("/etc/ld.so.cache"))?,
        ))
        .ok()?;
        let base = env::var_os("RELAR_CACHE_DIR")
            .map(PathBuf::from)
            .or_else(|| env::var_os("XDG_CACHE_HOME").map(|v| PathBuf::from(v).join("relar")))
            .or_else(|| env::var_os("HOME").map(|v| PathBuf::from(v).join(".cache/relar")))?;
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
        let link_recipe = blake3::keyed_hash(
            object_recipe.as_bytes(),
            &[u8::from(link.keep_dead_code), relro],
        );
        let manifest = root.join(format!("{}.json", link_recipe.to_hex()));
        let previous = fs::read(&manifest)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Manifest>(&bytes).ok())
            .filter(|m| m.native.iter().all(|(p, stamp)| Stamp::read(p) == *stamp));
        Some(Self {
            root,
            manifest,
            object_recipe,
            clang: clang.to_owned(),
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
            let linker = Command::new(&self.clang)
                .arg("-print-prog-name=ld")
                .output()
                .ok()?;
            let linker = Path::new(std::str::from_utf8(&linker.stdout).ok()?.trim());
            if linker.canonicalize().ok()? != Path::new("/usr/bin/ld").canonicalize().ok()? {
                return None;
            }
            let help = Command::new(linker).arg("--help").output().ok()?;
            if !linker
                .canonicalize()
                .ok()?
                .file_name()?
                .to_str()?
                .ends_with("ld.bfd")
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

    pub fn publish(&self, sources: SourceInputs, stage: &Path, file: &Path) -> Option<()> {
        // GNU ld's first stanza lists one input per continued line. Only the
        // compiler's private staging objects may disappear after linking.
        let deps = fs::read_to_string(stage.join("link.d")).ok()?;
        let mut paths = BTreeSet::new();
        let mut temporary_object = false;
        for line in deps.split("\n\n").next()?.lines().skip(1) {
            let p = std::path::absolute(line.trim().trim_end_matches('\\').trim_end()).ok()?;
            if p.starts_with(stage) && p.extension().and_then(OsStr::to_str) == Some("o") {
                temporary_object = true;
                continue;
            }
            if !p.is_file() || (p.extension() == Some(OsStr::new("a")) && !ordinary_archive(&p)) {
                return None;
            }
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
                let search = Command::new(&self.clang)
                    .arg("-print-search-dirs")
                    .output()
                    .ok()?;
                let search = std::str::from_utf8(&search.stdout).ok()?;
                let libraries = search
                    .lines()
                    .find_map(|s| s.strip_prefix("libraries: ="))?;
                env::split_paths(libraries).collect()
            }
        };
        paths.extend(search_dirs.iter().cloned());
        paths.insert(PathBuf::from("/usr/lib/gcc/x86_64-linux-gnu"));
        let native = paths
            .into_iter()
            .map(|p| {
                let stamp = Stamp::read(&p);
                (p, stamp)
            })
            .collect();
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
