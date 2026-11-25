//! Exercise admission directly, independent of which Python features gain fixed lowering.
use super::{CheckedProgram, CodegenLayout, CompileOptions, ProgramEntry, ProgramOutput};

#[test]
fn registered_entry_without_an_error_is_still_refused() {
    for checking in [true, false] {
        let mut output = ProgramOutput::new(CodegenLayout::Whole);
        let Err(error) = super::object_fallback(
            (!checking).then_some(&mut output),
            ProgramEntry::Source("print(42)\n"),
            &CompileOptions::default(),
            Default::default(),
            None,
            |_, _, _| Ok(CheckedProgram::Registered(None)),
        ) else {
            panic!("even an otherwise valid registered program must be refused");
        };
        assert!(error.contains("native-only compilation refused"), "{error}");
        assert!(error.contains("registered entry"), "{error}");
        assert!(output.compilation.units.is_empty());
        assert!(output.whole.is_none());
    }
}

#[test]
fn a_runtime_refusal_keeps_the_original_diagnostic() {
    let Err(error) = super::object_fallback(
        None,
        ProgramEntry::Source("print(42)\n"),
        &CompileOptions::default(),
        Default::default(),
        None,
        |_, _, _| {
            Ok(CheckedProgram::Runtime(
                "cannot represent this operation".into(),
            ))
        },
    ) else {
        panic!("runtime-only operation must be refused");
    };
    assert!(error.contains("cannot represent this operation"), "{error}");
    assert!(error.contains("native-only compilation refused"), "{error}");
}
