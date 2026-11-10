use super::*;
use std::cell::Cell;

#[test]
fn bound_wrapper_emits_native_only_if_registration_declines() {
    let project = tempfile::tempdir().unwrap();
    let path = project.path().join("wrapper.py");
    let wrapper = include_str!("../../../tests/fixtures/callables/bound_methods.py");

    for (source, registered) in [
        (wrapper.to_owned(), false),
        (
            format!("import sys\n{wrapper}\nprint(sys.version_info.major)\n"),
            true,
        ),
    ] {
        std::fs::write(&path, source).unwrap();
        let native_calls = Cell::new(0);
        let compilation = object_fallback(
            BoxedLayout::Whole,
            ProgramEntry::File(&path, None),
            &CompileOptions::default(),
            Default::default(),
            false,
            |graph, call_error| {
                let checked = checked_lower_with(graph, None, call_error)?;
                assert!(matches!(checked, CheckedProgram::Native(_)));
                Ok(checked)
            },
            |_, (module, _), program| {
                assert!(program.prefer_objects);
                native_calls.set(native_calls.get() + 1);
                let ranges = ranges::analyze(&module);
                let llvm_ir = llvm::emit_file(&module, &ranges, false, program)?;
                Ok(vec![CodegenUnit {
                    name: "__main__".into(),
                    llvm_ir,
                }])
            },
        )
        .unwrap();
        assert_eq!(native_calls.get(), usize::from(!registered));
        assert_eq!(compilation.source_inputs.boxed_imports, registered);
    }
}

#[test]
fn a_file_build_records_the_root_of_its_registered_package() {
    let project = tempfile::tempdir().unwrap();
    let root = project.path().canonicalize().unwrap();
    let package = root.join("parcel");
    std::fs::create_dir(&package).unwrap();
    let initializer = package.join("__init__.py");
    std::fs::write(&initializer, "def program(): return 'printf'\n").unwrap();
    let entry = package.join("__main__.py");
    std::fs::write(
        &entry,
        "import os, sys\nfrom parcel import program\nif __name__ == '__main__':\n    cmd = program()\n    os.execvp(cmd, [cmd, *sys.argv[1:]])\n",
    )
    .unwrap();

    let compilation = compile_file(&entry, None, &CompileOptions::default()).unwrap();
    let inputs = compilation.source_inputs;
    assert!(inputs.files.iter().any(|(path, _, _)| path == &initializer));
    assert!(inputs.fresh());
    std::fs::write(root.join("__init__.py"), "").unwrap();
    assert!(!inputs.fresh());
}

#[test]
fn fixed_file_selects_its_product_for_the_requested_startup() {
    let project = tempfile::tempdir().unwrap();
    let path = project.path().join("app.py");
    std::fs::write(
        &path,
        "def add(a: int, b: int) -> int:\n    return a + b\n\nprint(add(int(input()), 2))\n",
    )
    .unwrap();

    for sitecustomize in [false, true] {
        let options = CompileOptions {
            sitecustomize,
            ..Default::default()
        };
        let observed = Cell::new(false);
        object_fallback(
            BoxedLayout::Check,
            ProgramEntry::File(&path, None),
            &options,
            Default::default(),
            false,
            |graph, call_error| {
                let product = checked_lower_with(graph, None, call_error)?;
                if sitecustomize {
                    assert!(matches!(product, CheckedProgram::Registered(None)));
                } else {
                    assert!(matches!(product, CheckedProgram::Native(_)));
                }
                observed.set(true);
                Ok(product)
            },
            |_, _, _| unreachable!("checking does not emit LLVM"),
        )
        .unwrap();
        assert!(observed.get());
    }
}

#[test]
fn required_object_call_keeps_its_admission_reason() {
    let project = tempfile::tempdir().unwrap();
    let path = project.path().join("app.py");
    std::fs::write(
        &path,
        "def render(data: list[int] | None, spec: str) -> str:\n    return f'{data:{spec}}'\n\nprint(render([17, 19], input()))\n",
    )
    .unwrap();
    let mut graph = modules::SourceGraph::load(&path, None, &[]).unwrap();
    let reason = match checked_lower(&mut graph).unwrap() {
        CheckedProgram::Registered(Some(reason)) => reason,
        CheckedProgram::Runtime(reason) => panic!("unexpected lowering refusal: {reason}"),
        _ => panic!("the checked formatting operation requires a registered entry"),
    };
    let expected = format!(
        "{}:2:14: Imre: this f-string field requires native Python object operations",
        path.display()
    );
    assert_eq!(reason.kind, LoweringKind::Ordinary);
    assert_eq!(reason.to_string(), expected);
    assert_eq!(check_file(&path, None).unwrap_err(), expected);
}

#[test]
fn interleaved_import_preserves_another_modules_list_contract() {
    let project = tempfile::tempdir().unwrap();
    let entry = project.path().join("app.py");
    let scores = project.path().join("scores.py");
    std::fs::write(&entry, "import library\nimport scores\n").unwrap();
    std::fs::write(
        project.path().join("library.py"),
        "print('loaded')\ndef later() -> int:\n    return 2\n",
    )
    .unwrap();
    std::fs::write(
        &scores,
        "def score() -> int:\n    values: list[int] = [1]\n    values.append(True)\n    return values[0]\n",
    )
    .unwrap();
    let expected = format!(
        "{}:3:19: Imre: expected int, got bool; annotations are enforced contracts",
        scores.display()
    );
    assert_eq!(check_file(&entry, None).unwrap_err(), expected);
    let options = CompileOptions {
        sitecustomize: true,
        ..Default::default()
    };
    assert_eq!(
        compile_file(&entry, None, &options).err().unwrap(),
        expected
    );
}
