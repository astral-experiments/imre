//! Observe the filesystem answers used by the module loader, including failed lookups.
//! A check can fail before it has a complete source graph.
use std::collections::BTreeSet;
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use notify::event::{AccessKind, AccessMode, ModifyKind};
use notify::{EventKind, RecursiveMode, Watcher};

use crate::IncrementalCompiler;

#[derive(Default)]
pub(crate) struct FileSystem<'a>(Option<&'a mut Inputs>);

#[derive(Default)]
struct Inputs(Vec<(PathBuf, Fact)>);

enum Fact {
    File(bool),
    Directory(bool),
    Exists(bool),
    Identity(Result<PathBuf, String>),
    Bytes(Result<blake3::Hash, String>),
    WheelRecords(Vec<PathBuf>),
}

fn outcome<T, U>(result: &io::Result<T>, map: impl FnOnce(&T) -> U) -> Result<U, String> {
    result.as_ref().map(map).map_err(io::Error::to_string)
}

/// Read the selected canonical source through one handle. A symlink introduced
/// after resolution must not substitute another file under the checked identity.
fn with_source<T>(path: &Path, read: impl FnOnce(&mut File) -> io::Result<T>) -> io::Result<T> {
    let changed = || {
        io::Error::new(
            io::ErrorKind::Interrupted,
            "source identity changed during compilation",
        )
    };
    #[cfg(target_os = "linux")]
    {
        use rustix::fs::{CWD, Mode, OFlags, ResolveFlags, openat2};
        use rustix::io::Errno;
        match openat2(
            CWD,
            path,
            OFlags::RDONLY | OFlags::CLOEXEC,
            Mode::empty(),
            ResolveFlags::NO_SYMLINKS,
        ) {
            Ok(fd) => return read(&mut File::from(fd)),
            Err(Errno::LOOP) => return Err(changed()),
            Err(Errno::NOSYS | Errno::PERM) => {}
            Err(err) => return Err(err.into()),
        }
    }
    let mut source = same_file::Handle::from_file(File::open(path)?)?;
    if path.canonicalize()? != path || source != same_file::Handle::from_path(path)? {
        return Err(changed());
    }
    read(source.as_file_mut())
}

impl FileSystem<'_> {
    /// Record another graph load in the same watch snapshot.
    pub(crate) fn reborrow(&mut self) -> FileSystem<'_> {
        FileSystem(self.0.as_deref_mut())
    }

    fn record(&mut self, path: &Path, fact: impl FnOnce() -> Fact) {
        if let Some(inputs) = &mut self.0 {
            inputs.0.push((path.to_owned(), fact()));
        }
    }

    pub(crate) fn wheel_records(&mut self, path: &Path) -> Vec<PathBuf> {
        let records = crate::installed_wheels::records(path);
        self.record(path, || Fact::WheelRecords(records.clone()));
        records
    }

    pub(crate) fn is_file(&mut self, path: &Path) -> bool {
        let value = path.is_file();
        self.record(path, || Fact::File(value));
        value
    }

    pub(crate) fn is_dir(&mut self, path: &Path) -> bool {
        let value = path.is_dir();
        self.record(path, || Fact::Directory(value));
        value
    }

    pub(crate) fn exists(&mut self, path: &Path) -> bool {
        let value = path.exists();
        self.record(path, || Fact::Exists(value));
        value
    }

    pub(crate) fn canonicalize(&mut self, path: &Path) -> io::Result<PathBuf> {
        let value = path.canonicalize();
        self.record(path, || Fact::Identity(outcome(&value, Clone::clone)));
        value
    }

    pub(crate) fn read_source(&mut self, path: &Path) -> io::Result<Vec<u8>> {
        let value = with_source(path, |file| {
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes)?;
            Ok(bytes)
        });
        self.record(path, || Fact::Bytes(outcome(&value, |b| blake3::hash(b))));
        value
    }

    /// Hash a file as it is read, retaining its handle for a seekable ZIP reader.
    pub(crate) fn open_hashed(&mut self, path: &Path) -> io::Result<(std::fs::File, blake3::Hash)> {
        let result = std::fs::File::open(path).and_then(|mut file| {
            let digest = blake3::Hasher::new().update_reader(&mut file)?.finalize();
            Ok((file, digest))
        });
        self.record(path, || {
            Fact::Bytes(outcome(&result, |(_, digest)| *digest))
        });
        result
    }

    pub(crate) fn read(&mut self, path: &Path) -> io::Result<Vec<u8>> {
        let value = std::fs::read(path);
        self.record(path, || Fact::Bytes(outcome(&value, |b| blake3::hash(b))));
        value
    }
}

impl Inputs {
    fn fresh(&self) -> bool {
        self.0.iter().all(|(path, fact)| match fact {
            Fact::WheelRecords(value) => crate::installed_wheels::records(path) == *value,
            Fact::File(value) => path.is_file() == *value,
            Fact::Directory(value) => path.is_dir() == *value,
            Fact::Exists(value) => path.exists() == *value,
            Fact::Identity(value) => path.canonicalize().map_err(|e| e.to_string()) == *value,
            Fact::Bytes(value) => outcome(&std::fs::read(path), |b| blake3::hash(b)) == *value,
        })
    }

    fn paths(&self) -> impl Iterator<Item = &Path> {
        self.0.iter().flat_map(|(path, fact)| {
            let target = match fact {
                Fact::Identity(Ok(target)) => Some(target.as_path()),
                _ => None,
            };
            std::iter::once(path.as_path()).chain(target)
        })
    }
}

struct WatchPaths {
    directories: BTreeSet<PathBuf>,
    names: BTreeSet<PathBuf>,
}

struct Subscription {
    directories: BTreeSet<PathBuf>,
    names: Arc<Mutex<BTreeSet<PathBuf>>>,
    _watcher: notify::RecommendedWatcher,
    changed: mpsc::Receiver<()>,
    rearm: Arc<AtomicBool>,
}

impl WatchPaths {
    fn new(inputs: &Inputs) -> Result<Self, String> {
        let mut directories = BTreeSet::new();
        let mut names = BTreeSet::new();
        for path in inputs.paths() {
            let absolute = std::path::absolute(path).map_err(|e| e.to_string())?;
            if !names.insert(absolute.clone()) {
                continue;
            }
            // Watch the closest existing parent. Editors rename over files, and several
            // components may still be missing when an import fails.
            for parent in absolute.ancestors().skip(1) {
                if let Ok(real) = parent.canonicalize()
                    && real.is_dir()
                {
                    names.insert(real.join(absolute.strip_prefix(parent).unwrap()));
                    directories.insert(real);
                    break;
                }
            }
        }

        for (root, fact) in &inputs.0 {
            if matches!(fact, Fact::WheelRecords(_))
                && let Ok(real) = root.canonicalize()
                && real.is_dir()
            {
                names.insert(real.clone());
                directories.insert(real);
            }
        }
        Ok(Self { directories, names })
    }
}

impl Subscription {
    fn for_inputs(previous: Option<Self>, inputs: &Inputs) -> Result<Self, String> {
        let WatchPaths { directories, names } = WatchPaths::new(inputs)?;
        if let Some(previous) = previous
            && previous.directories == directories
            && !previous.rearm.load(Ordering::Relaxed)
        {
            // A change to first-party imports can select different files in the same directories.
            *previous.names.lock().unwrap() = names;
            return Ok(previous);
        }
        let names = Arc::new(Mutex::new(names));
        let watched_names = Arc::clone(&names);
        // One wakeup covers a burst, including a directory change after a file change.
        let (send, changed) = mpsc::sync_channel(1);
        let rearm = Arc::new(AtomicBool::default());
        let needs_rearm = Arc::clone(&rearm);
        let mut watcher =
            notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                let rearm = match event {
                    Err(_) => true,
                    Ok(event) if event.need_rescan() || event.paths.is_empty() => true,
                    Ok(event) => {
                        match event.kind {
                            EventKind::Access(AccessKind::Close(AccessMode::Write)) => {}
                            EventKind::Access(_) => return,
                            _ => {}
                        }
                        let names = watched_names.lock().unwrap();
                        if !event.paths.iter().any(|p| {
                            names.iter().any(|name| name.starts_with(p))
                                || (p.extension().is_some_and(|e| e == "dist-info")
                                    && p.parent().is_some_and(|parent| names.contains(parent)))
                        }) {
                            return;
                        }
                        let structural = matches!(
                            event.kind,
                            EventKind::Create(_)
                                | EventKind::Remove(_)
                                | EventKind::Modify(ModifyKind::Name(_) | ModifyKind::Any)
                                | EventKind::Any
                                | EventKind::Other
                        );
                        structural
                            && event
                                .paths
                                .iter()
                                .any(|p| names.iter().any(|name| name != p && name.starts_with(p)))
                    }
                };
                if rearm {
                    needs_rearm.store(true, Ordering::Relaxed);
                }
                let _ = send.try_send(());
            })
            .map_err(|e| format!("could not start the file watcher: {e}"))?;
        let mut subscriptions = watcher.paths_mut();
        for directory in &directories {
            if let Err(error) = subscriptions.add(directory, RecursiveMode::NonRecursive) {
                match &error.kind {
                    // A directory can disappear during setup. The freshness check below
                    // and the periodic fallback still observe the same filesystem facts.
                    notify::ErrorKind::PathNotFound => {}
                    notify::ErrorKind::Io(e)
                        if matches!(
                            e.kind(),
                            io::ErrorKind::NotFound
                                | io::ErrorKind::NotADirectory
                                | io::ErrorKind::PermissionDenied
                        ) => {}
                    _ => return Err(format!("could not watch {}: {error}", directory.display())),
                }
            }
        }
        subscriptions.commit().map_err(|e| e.to_string())?;
        Ok(Self {
            directories,
            names,
            _watcher: watcher,
            changed,
            rearm,
        })
    }

    fn wait_for_quiet(&self) -> Result<(), String> {
        // Editors may write a file in several chunks. Wait for quiet after the last
        // notification, and still check progress during a continuous stream of saves.
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let timeout = deadline
                .saturating_duration_since(Instant::now())
                .min(Duration::from_millis(10));
            if timeout.is_zero() {
                return Ok(());
            }
            match self.changed.recv_timeout(timeout) {
                Ok(()) => {}
                Err(mpsc::RecvTimeoutError::Timeout) => return Ok(()),
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err("file watcher stopped".into());
                }
            }
        }
    }
}

impl Inputs {
    fn wait_for_change(
        &self,
        previous: Option<Subscription>,
        ready: impl FnOnce(),
    ) -> Result<Subscription, String> {
        let mut subscription = Subscription::for_inputs(previous, self)?;
        // Subscribe before validating the bytes and lookups actually consumed. This
        // also catches a save during the first check or while subscriptions change.
        if !self.fresh() {
            return Ok(subscription);
        }
        ready();
        loop {
            match subscription.changed.recv_timeout(Duration::from_secs(2)) {
                // Check for silent event loss and indirect symlinks outside the graph.
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err("file watcher stopped".into());
                }
                Ok(()) => {
                    subscription.wait_for_quiet()?;
                    if subscription.rearm.swap(false, Ordering::Relaxed) {
                        // An empty package directory moves the native watch closer to a
                        // still-missing __init__.py, even though the check result is unchanged.
                        subscription = Subscription::for_inputs(None, self)?;
                    }
                }
            }
            if !self.fresh() {
                return Ok(subscription);
            }
        }
    }
}

pub(crate) fn check(
    cache: &mut IncrementalCompiler,
    path: &Path,
    module_root: Option<&Path>,
    options: &crate::CompileOptions,
    mut report: impl FnMut(Result<(), String>),
) -> Result<(), String> {
    let mut subscription = None;
    loop {
        let mut inputs = Inputs::default();
        let result = crate::check_file_with(
            path,
            module_root,
            options,
            Some(cache),
            FileSystem(Some(&mut inputs)),
        );
        subscription = Some(inputs.wait_for_change(subscription, || report(result))?);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn selected_source_follows_the_open_handle() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("helper.py");
        let other = dir.path().join("other.py");
        std::fs::write(&path, "checked").unwrap();
        std::fs::write(&other, "replacement").unwrap();
        let identity = path.canonicalize().unwrap();
        let saved = with_source(&identity, |file| {
            std::fs::rename(&path, dir.path().join("previous.py"))?;
            symlink(&other, &path)?;
            io::read_to_string(file)
        });
        assert_eq!(saved.unwrap(), "checked");
        let mut fs = FileSystem::default();
        assert_eq!(
            fs.read_source(&identity).unwrap_err().kind(),
            io::ErrorKind::Interrupted
        );
        assert_eq!(
            fs.read_source(&path.canonicalize().unwrap()).unwrap(),
            b"replacement"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_later_source_keeps_shadowed_native_candidates_out_of_the_watch() {
        let project = tempfile::tempdir().unwrap();
        let vendor = tempfile::tempdir().unwrap();
        let entry = project.path().join("main.py");
        let native = project.path().join("helper.so");
        let source = vendor.path().join("helper.py");
        std::fs::write(&entry, "import helper\nprint(helper.value)\n").unwrap();
        std::fs::write(&native, []).unwrap();
        std::fs::write(&source, "value = 1\n").unwrap();
        let options = crate::CompileOptions {
            extension_paths: vec![vendor.path().to_owned()],
            ..Default::default()
        };
        let mut inputs = Inputs::default();
        crate::check_file_with(&entry, None, &options, None, FileSystem(Some(&mut inputs)))
            .unwrap();

        std::fs::remove_file(native).unwrap();
        assert!(inputs.fresh());
        std::fs::write(source, "value = 2\n").unwrap();
        assert!(!inputs.fresh());
    }

    #[test]
    fn creating_or_editing_an_entry_profile_invalidates_the_check() {
        let dir = tempfile::tempdir().unwrap();
        let entry = dir.path().join("main.py");
        std::fs::write(&entry, "print(1)\n").unwrap();
        let profile = dir.path().join("profile.json");
        let options = crate::CompileOptions {
            reachable_only: true,
            entry_profile: Some(profile.clone()),
            ..Default::default()
        };
        for (success, replacement) in [(false, "{"), (false, "{}"), (true, "")] {
            let mut inputs = Inputs::default();
            let result =
                crate::check_file_with(&entry, None, &options, None, FileSystem(Some(&mut inputs)));
            assert_eq!(result.is_ok(), success, "{result:?}");
            assert!(inputs.fresh());
            std::fs::write(&profile, replacement).unwrap();
            assert!(!inputs.fresh());
        }
    }

    #[test]
    fn edits_and_new_imports_in_watched_directories_reuse_the_subscription() {
        let dir = tempfile::tempdir().unwrap();
        let entry = dir.path().join("main.py");
        std::fs::write(
            dir.path().join("dep.py"),
            "def value() -> int:\n    return 1\n",
        )
        .unwrap();
        let mut subscription = None;
        let mut first = None;
        for source in [
            "print(1)\n",
            "print(2)\n",
            "from dep import value\nprint(value())\n",
        ] {
            std::fs::write(&entry, source).unwrap();
            let mut inputs = Inputs::default();
            crate::check_file_with(
                &entry,
                None,
                &Default::default(),
                None,
                FileSystem(Some(&mut inputs)),
            )
            .unwrap();
            let current = Subscription::for_inputs(subscription, &inputs).unwrap();
            assert!(Arc::ptr_eq(
                first.get_or_insert_with(|| Arc::clone(&current.names)),
                &current.names
            ));
            subscription = Some(current);
        }
    }

    #[test]
    fn edits_between_loading_and_subscribing_suppress_stale_results() {
        let dir = tempfile::tempdir().unwrap();
        let entry = dir.path().join("main.py");
        let dep = dir.path().join("dep.py");
        std::fs::write(&entry, "from dep import value\nprint(value())\n").unwrap();
        let mut inputs = Inputs::default();
        let missing = crate::check_file_with(
            &entry,
            None,
            &Default::default(),
            None,
            FileSystem(Some(&mut inputs)),
        );
        assert!(missing.unwrap_err().contains("cannot resolve"));
        std::fs::write(&dep, "def value() -> int:\n    return 1\n").unwrap();
        inputs
            .wait_for_change(None, || panic!("reported a stale missing import"))
            .unwrap();

        let mut inputs = Inputs::default();
        crate::check_file_with(
            &entry,
            None,
            &Default::default(),
            None,
            FileSystem(Some(&mut inputs)),
        )
        .unwrap();
        std::fs::write(&dep, "def value() -> int:\n    return 'one'\n").unwrap();
        inputs
            .wait_for_change(None, || panic!("reported a stale success"))
            .unwrap();
    }
}
