//! Observe the filesystem answers used by the module loader, including failed lookups.
//! A check can fail before it has a complete source graph.
use std::collections::BTreeSet;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Duration;

use notify::event::{AccessKind, AccessMode, ModifyKind};
use notify::{EventKind, RecursiveMode, Watcher};

use crate::analysis::AnalysisCache;

#[derive(Default)]
pub(crate) struct FileSystem<'a>(Option<&'a mut Inputs>);

#[derive(Default)]
struct Inputs(Vec<(PathBuf, Fact)>);

enum Fact {
    File(bool),
    Directory(bool),
    Exists(bool),
    Identity(Result<PathBuf, String>),
    Content(Result<blake3::Hash, String>),
}

fn outcome<T, U>(result: &io::Result<T>, map: impl FnOnce(&T) -> U) -> Result<U, String> {
    result.as_ref().map(map).map_err(io::Error::to_string)
}

impl FileSystem<'_> {
    fn record(&mut self, path: &Path, fact: impl FnOnce() -> Fact) {
        if let Some(inputs) = &mut self.0 {
            inputs.0.push((path.to_owned(), fact()));
        }
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

    pub(crate) fn read_to_string(&mut self, path: &Path) -> io::Result<String> {
        let value = std::fs::read_to_string(path);
        self.record(path, || {
            Fact::Content(outcome(&value, |s| blake3::hash(s.as_bytes())))
        });
        value
    }
}

impl Inputs {
    fn fresh(&self) -> bool {
        self.0.iter().all(|(path, fact)| match fact {
            Fact::File(value) => path.is_file() == *value,
            Fact::Directory(value) => path.is_dir() == *value,
            Fact::Exists(value) => path.exists() == *value,
            Fact::Identity(value) => path.canonicalize().map_err(|e| e.to_string()) == *value,
            Fact::Content(value) => {
                outcome(&std::fs::read_to_string(path), |s| {
                    blake3::hash(s.as_bytes())
                }) == *value
            }
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

struct Subscription {
    _watcher: notify::RecommendedWatcher,
    changed: mpsc::Receiver<()>,
    rearm: Arc<AtomicBool>,
}

impl Subscription {
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
                        if !event
                            .paths
                            .iter()
                            .any(|p| names.iter().any(|name| name.starts_with(p)))
                        {
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
        for directory in directories {
            if let Err(error) = subscriptions.add(&directory, RecursiveMode::NonRecursive) {
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
            _watcher: watcher,
            changed,
            rearm,
        })
    }
}

impl Inputs {
    fn wait_for_change(&self, ready: impl FnOnce()) -> Result<(), String> {
        let mut subscription = Subscription::new(self)?;
        // Subscribe before validating the bytes and lookups actually consumed. This
        // also catches a save during the first check or while subscriptions change.
        if !self.fresh() {
            return Ok(());
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
                    // Coalesce the create, write and rename events from an editor save.
                    std::thread::sleep(Duration::from_millis(8));
                    let _ = subscription.changed.try_recv();
                    if subscription.rearm.swap(false, Ordering::Relaxed) {
                        // An empty package directory moves the native watch closer to a
                        // still-missing __init__.py, even though the check result is unchanged.
                        subscription = Subscription::new(self)?;
                    }
                }
            }
            if !self.fresh() {
                return Ok(());
            }
        }
    }
}

pub(crate) fn check(
    cache: &mut AnalysisCache,
    path: &Path,
    module_root: Option<&Path>,
    mut report: impl FnMut(Result<(), String>),
) -> Result<(), String> {
    loop {
        let mut inputs = Inputs::default();
        let result = crate::check_file_with(
            path,
            module_root,
            Some(cache),
            FileSystem(Some(&mut inputs)),
        );
        inputs.wait_for_change(|| report(result))?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edits_between_loading_and_subscribing_suppress_stale_results() {
        let dir = tempfile::tempdir().unwrap();
        let entry = dir.path().join("main.py");
        let dep = dir.path().join("dep.py");
        std::fs::write(&entry, "from dep import value\nprint(value())\n").unwrap();
        let mut inputs = Inputs::default();
        let missing = crate::check_file_with(&entry, None, None, FileSystem(Some(&mut inputs)));
        assert!(missing.unwrap_err().contains("cannot resolve"));
        std::fs::write(&dep, "def value() -> int:\n    return 1\n").unwrap();
        inputs
            .wait_for_change(|| panic!("reported a stale missing import"))
            .unwrap();

        let mut inputs = Inputs::default();
        crate::check_file_with(&entry, None, None, FileSystem(Some(&mut inputs))).unwrap();
        std::fs::write(&dep, "def value() -> int:\n    return 'one'\n").unwrap();
        inputs
            .wait_for_change(|| panic!("reported a stale success"))
            .unwrap();
    }
}
