//! Discover the finite Python files of wheel distributions already used by the program.
use crate::{SourceInputs, watch::FileSystem};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;

pub(crate) fn records(root: &Path) -> Vec<PathBuf> {
    let mut paths: Vec<_> = std::fs::read_dir(root)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "dist-info"))
        .map(|path| path.join("RECORD"))
        .collect();
    paths.sort_unstable();
    paths
}

/// Application and installed sources on the search entry selected by the module launcher.
pub(crate) struct ModuleEntryRoot<'a> {
    root: PathBuf,
    application: bool,
    installed: &'a HashSet<Rc<Path>>,
}

impl<'a> ModuleEntryRoot<'a> {
    pub(crate) fn select(
        root: &Path,
        entry: &Path,
        path_zero: bool,
        records: &'a mut WheelRecords,
        options: &crate::CompileOptions,
        inputs: &mut SourceInputs,
        fs: &mut FileSystem<'_>,
    ) -> Option<Self> {
        if !fs.is_dir(root) {
            return None;
        }
        if !path_zero {
            if options.pythonpath.is_empty() {
                return None;
            }
            let identity = fs.canonicalize(root).ok()?;
            let mut matches =
                |path: &PathBuf| fs.canonicalize(path).ok().as_ref() == Some(&identity);
            let index = options.extension_paths.iter().position(&mut matches)?;
            if !options.pythonpath.contains(&index) || options.installed_dirs().any(matches) {
                return None;
            }
        }
        let wheels = records.discover(root, inputs, fs);
        let installed = records.membership(wheels);
        Some(Self {
            application: path_zero || !installed.contains(entry),
            root: root.to_owned(),
            installed,
        })
    }

    pub(crate) fn is_program(&self, root: &Path, path: &Path) -> Option<bool> {
        (root == self.root).then(|| self.application && !self.installed.contains(path))
    }
}

pub(crate) struct InstalledWheel {
    root: PathBuf,
    canonical_root: Option<PathBuf>,
    members: Vec<(Rc<Path>, String)>,
}

/// The checked resolver and the module registry read the same installed distribution metadata.
#[derive(Default)]
pub(crate) struct WheelRecords {
    records: HashMap<PathBuf, Record>,
    entry_wheels: Vec<Rc<InstalledWheel>>,
    entry_paths: HashSet<Rc<Path>>,
}

struct Record {
    digest: String,
    paths: Vec<PathBuf>,
    // A RECORD may be reached through more than one spelling of its installation root.
    wheels: Vec<Rc<InstalledWheel>>,
}

impl WheelRecords {
    // Both entry classifiers normally see the same wheels. Discovery still records
    // each phase's dependencies and can omit a wheel whose RECORD is unreadable.
    fn membership(&mut self, wheels: Vec<Rc<InstalledWheel>>) -> &HashSet<Rc<Path>> {
        if self.entry_wheels.len() != wheels.len()
            || !self
                .entry_wheels
                .iter()
                .zip(&wheels)
                .all(|(a, b)| Rc::ptr_eq(a, b))
        {
            self.entry_paths = wheels
                .iter()
                .flat_map(|wheel| wheel.members.iter().map(|(path, _)| Rc::clone(path)))
                .collect();
            self.entry_wheels = wheels;
        }
        &self.entry_paths
    }

    fn read(&mut self, path: PathBuf, fs: &mut FileSystem<'_>) -> std::io::Result<&mut Record> {
        use std::collections::hash_map::Entry;
        Ok(match self.records.entry(path) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => {
                let bytes = fs.read(entry.key())?;
                let paths = csv::ReaderBuilder::new()
                    .has_headers(false)
                    .flexible(true)
                    .from_reader(bytes.as_slice())
                    .records()
                    .filter_map(Result::ok)
                    .filter(|row| row.len() == 3)
                    .map(|row| PathBuf::from(&row[0]))
                    .collect();
                entry.insert(Record {
                    digest: blake3::hash(&bytes).to_hex().to_string(),
                    paths,
                    wheels: Vec::new(),
                })
            }
        })
    }

    pub(crate) fn discover(
        &mut self,
        root: &Path,
        inputs: &mut SourceInputs,
        filesystem: &mut FileSystem<'_>,
    ) -> Vec<Rc<InstalledWheel>> {
        let prior = inputs.wheel_records.iter().find(|(dir, _)| dir == root);
        let register = prior.is_none();
        let records = match prior {
            Some((_, records)) => records.clone(),
            None => filesystem.wheel_records(root),
        };
        let canonical_root = filesystem.canonicalize(root).ok();
        let mut wheels = Vec::new();
        for path in &records {
            if !filesystem.is_file(path) {
                inputs.file_checks.insert(path.clone(), false);
                continue;
            }
            let Ok(identity) = filesystem.canonicalize(path) else {
                inputs.missing.push(path.clone());
                continue;
            };
            let Ok(record) = self.read(identity.clone(), filesystem) else {
                inputs.missing.push(path.clone());
                continue;
            };
            if register {
                inputs
                    .files
                    .push((path.clone(), identity, record.digest.clone()));
            }
            wheels.push(record.wheel(root, canonical_root.as_deref()));
        }
        if register {
            inputs.wheel_records.push((root.to_owned(), records));
        }
        wheels
    }
}

impl Record {
    fn wheel(&mut self, root: &Path, canonical_root: Option<&Path>) -> Rc<InstalledWheel> {
        if let Some(wheel) = self.wheels.iter().find(|wheel| {
            wheel.root.as_os_str() == root.as_os_str()
                && wheel.canonical_root.as_deref() == canonical_root
        }) {
            return Rc::clone(wheel);
        }
        let members = self
            .paths
            .iter()
            .filter_map(|member| {
                let relative = if member.is_absolute() {
                    member
                        .strip_prefix(root)
                        .ok()
                        .or_else(|| member.strip_prefix(canonical_root?).ok())?
                } else {
                    member
                };
                let name = crate::zip_entry::python_module_name(relative.to_str()?)?;
                Some((Rc::from(root.join(relative)), name))
            })
            .collect();
        let wheel = Rc::new(InstalledWheel {
            root: root.to_owned(),
            canonical_root: canonical_root.map(Path::to_owned),
            members,
        });
        self.wheels.push(Rc::clone(&wheel));
        wheel
    }
}

impl InstalledWheel {
    pub(crate) fn take_active(
        pending: &mut Vec<Rc<Self>>,
        sources: &[crate::dynamic::ModuleSource<'_>],
        candidates: &mut HashMap<String, HashSet<PathBuf>>,
    ) -> Option<BTreeSet<String>> {
        if pending.is_empty() {
            return None;
        }
        let files: HashSet<_> = sources.iter().map(|(_, p, _)| p.as_path()).collect();
        let mut modules = BTreeSet::new();
        pending.retain(|wheel| {
            // A selected namespace portion has no initializer. Every installed wheel
            // contributing a Python file below that portion is a possible provider.
            if !wheel
                .members
                .iter()
                .any(|(path, _)| path.ancestors().any(|parent| files.contains(parent)))
            {
                return true;
            }
            for (path, name) in &wheel.members {
                if !files.contains(path.as_ref()) {
                    candidates
                        .entry(name.clone())
                        .or_default()
                        .insert(path.to_path_buf());
                    modules.insert(name.clone());
                }
            }
            false
        });
        (!modules.is_empty()).then_some(modules)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_installed_wheel_uses_record_members_and_tracks_catalog_changes() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let info = root.join("demo-1.dist-info");
        std::fs::create_dir(&info).unwrap();
        let record = info.join("RECORD");
        std::fs::write(
            &record,
            "bad row\ndemo/__init__.py,,\n\"demo/with,comma.py\",,\ndemo/__init__.py,,\n../outside.py,,\n",
        )
        .unwrap();
        let mut inputs = SourceInputs::default();
        let mut catalog = WheelRecords::default();
        let mut wheels = catalog.discover(root, &mut inputs, &mut FileSystem::default());
        let published = Rc::clone(&wheels[0]);
        let ordered: Vec<_> = published
            .members
            .iter()
            .map(|(_, name)| name.as_str())
            .collect();
        assert_eq!(ordered, ["demo", "demo.with,comma", "demo"]);
        let mut candidates = HashMap::new();
        assert!(InstalledWheel::take_active(&mut wheels, &[], &mut candidates).is_none());
        let files = vec![("demo".to_owned(), root.join("demo/__init__.py"), "".into())];
        let names = InstalledWheel::take_active(&mut wheels, &files, &mut candidates).unwrap();
        assert_eq!(names, BTreeSet::from(["demo.with,comma".to_owned()]));
        assert_eq!(
            candidates["demo.with,comma"],
            HashSet::from([root.join("demo/with,comma.py")])
        );
        assert!(inputs.fresh());
        std::fs::create_dir(root.join("other-1.dist-info")).unwrap();
        assert!(!inputs.fresh());
        std::fs::remove_dir(root.join("other-1.dist-info")).unwrap();
        std::fs::write(record, "demo/replacement.py,,\n").unwrap();
        assert!(!inputs.fresh());
        let mut registered = SourceInputs::default();
        let repeated = catalog.discover(root, &mut registered, &mut FileSystem::default());
        assert!(Rc::ptr_eq(&published, &repeated[0]));
        catalog.discover(root, &mut registered, &mut FileSystem::default());
        assert_eq!(registered.files, inputs.files);
        assert_eq!(registered.wheel_records, inputs.wheel_records);
    }

    #[cfg(unix)]
    #[test]
    fn namespace_can_select_absolute_member_of_symlinked_installation() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("physical");
        let alias = temp.path().join("site");
        let info = root.join("plugin-1.dist-info");
        std::fs::create_dir_all(&info).unwrap();
        std::os::unix::fs::symlink(&root, &alias).unwrap();
        let member = root.join("plugins/one.py");
        std::fs::write(info.join("RECORD"), format!("{},,\n", member.display())).unwrap();
        let mut inputs = SourceInputs::default();
        let mut records = WheelRecords::default();
        let mut wheels = records.discover(&alias, &mut inputs, &mut FileSystem::default());
        let physical = records.discover(&root, &mut inputs, &mut FileSystem::default());
        assert!(records.membership(physical).contains(member.as_path()));
        assert!(
            !records
                .membership(wheels.clone())
                .contains(member.as_path())
        );
        assert!(Rc::ptr_eq(
            &wheels[0],
            &records.discover(&alias, &mut inputs, &mut FileSystem::default())[0],
        ));
        let mut candidates = HashMap::new();
        let sources = vec![("plugins".to_owned(), alias.join("plugins"), "".into())];
        let names = InstalledWheel::take_active(&mut wheels, &sources, &mut candidates).unwrap();
        assert_eq!(names, BTreeSet::from(["plugins.one".to_owned()]));
        assert_eq!(
            candidates["plugins.one"],
            HashSet::from([alias.join("plugins/one.py")])
        );
    }
}
