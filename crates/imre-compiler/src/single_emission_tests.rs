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
