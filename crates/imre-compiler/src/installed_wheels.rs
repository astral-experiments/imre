//! Discover the finite Python files of wheel distributions already used by the program.
use crate::{SourceInputs, watch::FileSystem};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

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

pub(crate) struct InstalledWheel {
    members: Vec<(PathBuf, String)>,
}

impl InstalledWheel {
    pub(crate) fn discover(
        root: &Path,
        inputs: &mut SourceInputs,
        filesystem: &mut FileSystem<'_>,
    ) -> Vec<Self> {
        let records = filesystem.wheel_records(root);
        let canonical_root = filesystem.canonicalize(root).ok();
        let mut wheels = Vec::new();
        for path in &records {
            if !filesystem.is_file(path) {
                inputs.file_checks.insert(path.clone(), false);
                continue;
            }
            let (Ok(bytes), Ok(identity)) = (filesystem.read(path), filesystem.canonicalize(path))
            else {
                inputs.missing.push(path.clone());
                continue;
            };
            inputs.files.push((
                path.clone(),
                identity,
                blake3::hash(&bytes).to_hex().to_string(),
            ));
            let mut csv = csv::ReaderBuilder::new()
                .has_headers(false)
                .flexible(true)
                .from_reader(bytes.as_slice());
            let members = csv
                .records()
                .filter_map(Result::ok)
                .filter_map(|row| {
                    if row.len() != 3 {
                        return None;
                    }
                    let member = Path::new(row.get(0)?);
                    let relative = if member.is_absolute() {
                        member
                            .strip_prefix(root)
                            .ok()
                            .or_else(|| member.strip_prefix(canonical_root.as_deref()?).ok())?
                    } else {
                        member
                    };
                    let name = crate::zip_entry::python_module_name(relative.to_str()?)?;
                    Some((root.join(relative), name))
                })
                .collect();
            wheels.push(Self { members });
        }
        inputs.wheel_records.push((root.to_owned(), records));
        wheels
    }

    pub(crate) fn take_active(
        pending: &mut Vec<Self>,
        sources: &[(String, PathBuf, String)],
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
                if !files.contains(path.as_path()) {
                    candidates
                        .entry(name.clone())
                        .or_default()
                        .insert(path.clone());
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
            "bad row\ndemo/__init__.py,,\n\"demo/with,comma.py\",,\n../outside.py,,\n",
        )
        .unwrap();
        let mut inputs = SourceInputs::default();
        let mut wheels = InstalledWheel::discover(root, &mut inputs, &mut FileSystem::default());
        let mut candidates = HashMap::new();
        assert!(InstalledWheel::take_active(&mut wheels, &[], &mut candidates).is_none());
        let files = vec![(
            "demo".to_owned(),
            root.join("demo/__init__.py"),
            String::new(),
        )];
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
        let mut wheels = InstalledWheel::discover(&alias, &mut inputs, &mut FileSystem::default());
        let mut candidates = HashMap::new();
        let sources = vec![("plugins".to_owned(), alias.join("plugins"), String::new())];
        let names = InstalledWheel::take_active(&mut wheels, &sources, &mut candidates).unwrap();
        assert_eq!(names, BTreeSet::from(["plugins.one".to_owned()]));
        assert_eq!(
            candidates["plugins.one"],
            HashSet::from([alias.join("plugins/one.py")])
        );
    }
}
