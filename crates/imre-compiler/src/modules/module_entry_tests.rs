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

fn assert_entry_file(graph: &SourceGraph, expected: &Path) {
    assert_eq!(
        graph.module_filename(graph.entry).canonicalize().unwrap(),
        expected.canonicalize().unwrap()
    );
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
    assert_entry_file(&graph, &module);
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
    assert_entry_file(&graph, &main);
    assert_eq!(
        graph.modules[graph.entry].name,
        "selected_pkg.child.__main__"
    );
    let (inputs, _) = graph.disk_inputs(None);
    let files: Vec<_> = inputs
        .files
        .iter()
        .map(|(_, identity, _)| identity)
        .collect();
    for expected in [
        package.join("__init__.py"),
        package.join("child/__init__.py"),
        main,
    ] {
        assert!(
            files.contains(&&expected.canonicalize().unwrap()),
            "{expected:?}"
        );
    }
    let late_root = late.path().canonicalize().unwrap();
    assert!(!files.iter().any(|path| path.starts_with(&late_root)));
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
    assert_entry_file(&graph, &late_package.join("__main__.py"));
    let (inputs, _) = graph.disk_inputs(None);
    assert!(inputs.fresh());
    let early_package = early.path().join("selected_ns");
    fs::create_dir(&early_package).unwrap();
    fs::write(early_package.join("__main__.py"), "print(1)\n").unwrap();
    assert!(!inputs.fresh());
    let graph = resolve(Path::new("-m"), "selected_ns", &options).unwrap();
    assert_entry_file(&graph, &early_package.join("__main__.py"));
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
    assert_entry_file(&graph, &dir.path().join("__main__.py"));
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
    crate::checked_lower(&mut graph).unwrap();
    let error = graph
        .source_contract
        .as_ref()
        .unwrap()
        .original(&graph)
        .as_ref()
        .unwrap_err();
    assert!(
        error.contains("__init__.py:2:33: Imre: expected int, got str"),
        "{error}"
    );
}

#[test]
fn demanded_shipped_classes_use_the_defining_source() {
    use super::Binding;
    use ruff_text_size::Ranged;

    let root = tempfile::tempdir().unwrap();
    let entry = root.path().join("main.py");
    fs::write(
        root.path().join("helper.py"),
        "from asyncio import Future as F\nfrom traceback import FrameSummary as S\n",
    )
    .unwrap();
    fs::write(&entry,"import asyncio as tasks\nimport traceback as tb\nfrom helper import F, S\ntasks.Future\ntb.FrameSummary\n").unwrap();
    let graph = SourceGraph::load(&entry, Some(root.path()), &[]).unwrap();
    let module = &graph.modules[graph.entry];
    assert!(matches!(
        graph.session.shipped_declaration("traceback", "format_exc"),
        Some(Binding::RegisteredValue(_))
    ));
    // asyncio imports tasks with a wildcard. Its explicit __all__ omits this class.
    assert!(
        graph
            .session
            .shipped_declaration("asyncio.tasks", "_GatheringFuture")
            .is_some()
    );
    assert!(
        graph
            .session
            .shipped_declaration("asyncio", "_GatheringFuture")
            .is_none()
    );
    for (pos, alias, class, filename) in [
        (3, "F", "Future", "asyncio/futures.py"),
        (4, "S", "FrameSummary", "traceback.py"),
    ] {
        let (file, parsed, _) = graph
            .session
            .selected_file(&Path::new(crate::dynamic::BUNDLED_SOURCE_ROOT).join(filename))
            .unwrap();
        let definition = parsed
            .suite()
            .iter()
            .filter_map(ruff_python_ast::Stmt::as_class_def_stmt)
            .find(|c| c.name.as_str() == class)
            .unwrap();
        let expected = Some(Binding::RegisteredClass(ruff_db::files::FileRange::new(
            file,
            definition.name.range(),
        )));
        assert_eq!(module.export(alias), expected);
        let qualified = &module.parsed.suite()[pos].as_expr_stmt().unwrap().value;
        assert_eq!(graph.resolve(graph.entry, qualified), expected);
    }
}

#[test]
fn explicit_shipped_class_import_uses_its_verified_alias() {
    let dir = tempfile::tempdir().unwrap();
    let entry = dir.path().join("main.py");
    fs::write(&entry, "from glob import _PathGlobber as Globber\n").unwrap();
    crate::check_with(
        crate::ProgramEntry::File(&entry, None),
        &CompileOptions::default(),
        None,
        Default::default(),
    )
    .unwrap();
}

#[test]
fn shipped_function_imports_keep_the_defining_name_range() {
    use super::Binding;
    use ruff_text_size::Ranged;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("app.py");
    fs::write(
        &path,
        "import urllib.request as request\nfrom urllib.request import parse_http_list as parse, getproxies\nrequest.getproxies\n",
    )
    .unwrap();
    let graph = SourceGraph::load(&path, Some(dir.path()), &[]).unwrap();
    let module = &graph.modules[graph.entry];
    let (file, parsed, _) = graph
        .session
        .selected_file(&Path::new(crate::dynamic::BUNDLED_SOURCE_ROOT).join("urllib/request.py"))
        .unwrap();
    for (local, name) in [
        ("parse", "parse_http_list"),
        ("getproxies", "getproxies_environment"),
    ] {
        let function = parsed
            .suite()
            .iter()
            .filter_map(ruff_python_ast::Stmt::as_function_def_stmt)
            .find(|def| def.name.as_str() == name)
            .unwrap();
        let expected = Some(Binding::RegisteredValue(ruff_db::files::FileRange::new(
            file,
            function.name.range(),
        )));
        assert_eq!(module.export(local), expected);
        if local == "getproxies" {
            let qualified = &module.parsed.suite()[2].as_expr_stmt().unwrap().value;
            assert_eq!(graph.resolve(graph.entry, qualified), expected);
        }
    }
    assert!(
        graph
            .session
            .shipped_declaration("urllib.request", "_opener")
            .is_none()
    );
}

#[test]
fn decorated_shipped_import_and_attribute_share_the_defining_slot() {
    use super::Binding;
    use ruff_text_size::Ranged;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("app.py");
    fs::write(
        &path,
        "import typing\nfrom typing import TypeAlias as marker\ntyping.TypeAlias\ntyping.Missing\n",
    )
    .unwrap();
    let graph = SourceGraph::load(&path, Some(dir.path()), &[]).unwrap();
    let module = &graph.modules[graph.entry];
    let (file, parsed, _) = graph
        .session
        .selected_file(&Path::new(crate::dynamic::BUNDLED_SOURCE_ROOT).join("typing.py"))
        .unwrap();
    let definition = parsed
        .suite()
        .iter()
        .filter_map(ruff_python_ast::Stmt::as_function_def_stmt)
        .find(|def| def.name.as_str() == "TypeAlias")
        .unwrap();
    assert!(!definition.decorator_list.is_empty());
    let site = ruff_db::files::FileRange::new(file, definition.name.range());
    let expected = Some(Binding::RegisteredValue(site));
    assert!(!graph.session.is_exception_declaration(site));
    assert_eq!(module.export("marker"), expected);
    let qualified = &module.parsed.suite()[2].as_expr_stmt().unwrap().value;
    assert_eq!(graph.resolve(graph.entry, qualified), expected);
    let missing = &module.parsed.suite()[3].as_expr_stmt().unwrap().value;
    assert_eq!(graph.resolve(graph.entry, missing), None);
}

#[test]
fn imported_type_variable_uses_its_shipped_assignment() {
    use super::Binding;
    use ruff_text_size::Ranged;

    let graph = SourceGraph::single(
        "from typing import AnyStr as text\nimport typing\ntyping.AnyStr\n",
    )
    .unwrap();
    let module = &graph.modules[graph.entry];
    let (file, parsed, _) = graph
        .session
        .selected_file(&Path::new(crate::dynamic::BUNDLED_SOURCE_ROOT).join("typing.py"))
        .unwrap();
    let target = parsed
        .suite()
        .iter()
        .filter_map(ruff_python_ast::Stmt::as_assign_stmt)
        .flat_map(|assign| &assign.targets)
        .find(|target| {
            target
                .as_name_expr()
                .is_some_and(|name| name.id == "AnyStr")
        })
        .unwrap();
    let expected = Some(Binding::RegisteredValue(ruff_db::files::FileRange::new(
        file,
        target.range(),
    )));
    assert_eq!(module.export("text"), expected);
    let qualified = &module.parsed.suite()[2].as_expr_stmt().unwrap().value;
    assert_eq!(graph.resolve(graph.entry, qualified), expected);
}

#[test]
fn checked_cleandoc_keeps_the_complete_shipped_inspect_namespace() {
    let graph = SourceGraph::single_with_mode(
        "from inspect import cleandoc, isclass\nprint(cleandoc('  text'))\n",
        true,
    )
    .unwrap();
    assert!(
        graph
            .session
            .shipped_declaration("inspect", "isclass")
            .is_some()
    );
}

#[test]
fn checked_missing_keeps_the_root_import_diagnostic() {
    let result = SourceGraph::single_with_mode(
        "from dataclasses import MISSING\nfrom dataclasses import _dataclass_getstate\n",
        true,
    );
    assert!(
        result
            .err()
            .unwrap()
            .contains("unsupported sealed standard-library export dataclasses._dataclass_getstate")
    );
}
