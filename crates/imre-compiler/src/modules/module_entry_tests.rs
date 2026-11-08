use super::{SiteProbes, SourceGraph};
use crate::CompileOptions;
use std::fs;
use std::path::Path;

fn resolve(path: &Path, name: &str, options: &CompileOptions) -> Option<SourceGraph> {
    SourceGraph::module_entry(
        path,
        name,
        options,
        Default::default(),
        &mut SiteProbes::default(),
    )
    .unwrap()
}

#[test]
fn named_module_retains_its_import_name_and_actual_source() {
    let dir = tempfile::tempdir().unwrap();
    let module = dir.path().join("selected_module.py");
    fs::write(&module, "print(3)\n").unwrap();
    let options = CompileOptions {
        extension_paths: vec![dir.path().to_owned()],
        ..CompileOptions::default()
    };
    let mut graph = resolve(Path::new("-m"), "selected_module", &options).unwrap();
    assert_eq!(graph.modules[graph.entry].name, "selected_module");
    assert_eq!(graph.module_filename(graph.entry), module);
    let (inputs, _) = graph.disk_inputs(None);
    assert!(inputs.fresh());
    fs::write(&module, "print(4)\n").unwrap();
    assert!(!inputs.fresh());
}

#[test]
fn regular_package_selects_its_own_main_and_initializers() {
    let early = tempfile::tempdir().unwrap();
    let late = tempfile::tempdir().unwrap();
    let package = early.path().join("selected_pkg");
    fs::create_dir_all(package.join("child")).unwrap();
    fs::write(package.join("__init__.py"), "outer = 3\n").unwrap();
    fs::write(package.join("child/__init__.py"), "inner = 4\n").unwrap();
    let main = package.join("child/__main__.py");
    fs::write(&main, "from . import inner\nprint(inner)\n").unwrap();
    fs::create_dir_all(late.path().join("selected_pkg/child")).unwrap();
    fs::write(
        late.path().join("selected_pkg/child/__main__.py"),
        "broken(?\n",
    )
    .unwrap();
    let options = CompileOptions {
        extension_paths: vec![early.path().to_owned(), late.path().to_owned()],
        ..CompileOptions::default()
    };
    let mut graph = resolve(Path::new("-m"), "selected_pkg.child", &options).unwrap();
    assert_eq!(graph.module_filename(graph.entry), main);
    assert_eq!(
        graph.modules[graph.entry].name,
        "selected_pkg.child.__main__"
    );
    let (inputs, _) = graph.disk_inputs(None);
    let files: Vec<_> = inputs.files.iter().map(|(path, _, _)| path).collect();
    assert!(files.contains(&&package.join("__init__.py")));
    assert!(files.contains(&&package.join("child/__init__.py")));
    assert!(files.contains(&&main));
    assert!(!files.iter().any(|path| path.starts_with(late.path())));
    assert!(inputs.fresh());
    fs::write(package.join("__init__.py"), "outer = 4\n").unwrap();
    assert!(!inputs.fresh());
}

#[test]
fn a_new_earlier_namespace_portion_invalidates_the_selected_entry() {
    let early = tempfile::tempdir().unwrap();
    let late = tempfile::tempdir().unwrap();
    let late_package = late.path().join("selected_ns");
    fs::create_dir(&late_package).unwrap();
    fs::write(late_package.join("__main__.py"), "print(2)\n").unwrap();
    let options = CompileOptions {
        extension_paths: vec![early.path().to_owned(), late.path().to_owned()],
        ..CompileOptions::default()
    };
    let mut graph = resolve(Path::new("-m"), "selected_ns", &options).unwrap();
    assert_eq!(
        graph.module_filename(graph.entry),
        late_package.join("__main__.py")
    );
    let (inputs, _) = graph.disk_inputs(None);
    assert!(inputs.fresh());
    let early_package = early.path().join("selected_ns");
    fs::create_dir(&early_package).unwrap();
    fs::write(early_package.join("__main__.py"), "print(1)\n").unwrap();
    assert!(!inputs.fresh());
    let graph = resolve(Path::new("-m"), "selected_ns", &options).unwrap();
    assert_eq!(
        graph.module_filename(graph.entry),
        early_package.join("__main__.py")
    );
}

#[test]
fn directory_operand_and_open_module_loader_are_distinct() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("__main__.py"), "print(4)\n").unwrap();
    let options = CompileOptions {
        path_entry: true,
        ..Default::default()
    };
    let graph = resolve(dir.path(), "__main__", &options).unwrap();
    assert_eq!(
        graph.module_filename(graph.entry),
        dir.path().join("__main__.py")
    );
    fs::write(
        dir.path().join("runpy.py"),
        "def _run_module_as_main(*args): pass\n",
    )
    .unwrap();
    assert!(resolve(dir.path(), "__main__", &options).is_none());
}

#[test]
fn the_existing_startup_main_and_bundled_providers_stay_with_the_launcher() {
    for name in ["__main__", "sys"] {
        assert!(resolve(Path::new("-m"), name, &CompileOptions::default()).is_none());
    }
}

#[test]
fn a_program_root_package_keeps_the_initializer_diagnostic_on_its_own_source() {
    let dir = tempfile::tempdir().unwrap();
    let package = dir.path().join("checked_initializer");
    fs::create_dir(&package).unwrap();
    fs::write(
        package.join("__init__.py"),
        "def take(value: int) -> int: return value\ndef check() -> int: return take('x')\n",
    )
    .unwrap();
    fs::write(package.join("__main__.py"), "print(2)\n").unwrap();
    let external = CompileOptions {
        extension_paths: vec![dir.path().to_owned()],
        ..Default::default()
    };
    let mut dependency = resolve(Path::new("-m"), "checked_initializer", &external).unwrap();
    assert!(!dependency.modules[dependency.entry].is_program_source());
    // The external winner is available to callers. Its dormant argument error
    // does not impose project-wide declaration checks on an installed package.
    assert!(crate::checked_lower(&mut dependency).is_ok());
    let options = CompileOptions {
        path_entry: true,
        ..Default::default()
    };
    let mut graph = resolve(dir.path(), "checked_initializer", &options).unwrap();
    let error = crate::checked_lower(&mut graph).err().unwrap().to_string();
    assert!(
        error.contains("__init__.py:2:33: Imre: expected int, got str"),
        "{error}"
    );
}
