use super::*;

/// Register an existing checked function with its original frame positions.
/// Native analysis has already verified the operation sequence after specialization.
pub(crate) fn place_fixed_function(
    mut function: ir::Function,
    source: FixedBodySource,
) -> RegisteredFunction {
    function.returns = Type::PythonObject;
    RegisteredFunction {
        inputs: FrameInputs::Checked,
        function,
        local_names: source.local_names,
        fixed_sites: source.positions,
        fixed_fields: (!source.fields.is_empty()).then(|| Box::new(source.fields)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn named_entry_places_the_checked_arithmetic_body() {
        let text = "def scale(n: int) -> int:\n    return n * 31415927 + 17\n\nprint(scale(3))\n";
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app.py");
        std::fs::write(&path, text).unwrap();
        let mut graph = SourceGraph::load(&path, None, &[]).unwrap();
        graph.registered_startup = true;
        let (program, lowered) =
            caller_cache::observe_lowerings(|| crate::checked_lower(&mut graph).unwrap());
        assert!(matches!(program, crate::CheckedProgram::Registered(_)));
        assert_eq!(lowered.iter().filter(|name| *name == "scale").count(), 1);
        let functions = graph.registered_functions();
        let body = &functions[&graph.entry][&TextSize::default()];
        let positions: Vec<_> = body.fixed_sites.iter().map(|range| &text[*range]).collect();
        assert_eq!(
            positions,
            [
                "return n * 31415927 + 17",
                "n * 31415927 + 17",
                "n * 31415927",
                "n",
                "31415927",
                "17"
            ]
        );
    }

    #[test]
    fn file_object_recovery_places_the_checked_arithmetic_body() {
        let text = concat!(
            "def scale(n: int) -> int:\n    return n * 31415927 + 17\n\n",
            "m = map(int, \"12\")\nprint(scale(3), list(m))\n",
        );
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app.py");
        std::fs::write(&path, text).unwrap();
        let mut graph = SourceGraph::load(&path, None, &[]).unwrap();
        let (_, lowered) =
            caller_cache::observe_lowerings(|| crate::checked_lower(&mut graph).unwrap());
        assert_eq!(lowered.iter().filter(|name| *name == "scale").count(), 1);
        let functions = graph.registered_functions();
        let placed = &functions[&graph.entry][&TextSize::default()];
        let function = &placed.function;
        assert_eq!(function.returns, Type::PythonObject);
        assert_eq!(function.locals[function.params[0]], Type::Int);
        assert_eq!(placed.local_names[&function.params[0]].name, "n");
        let positions: Vec<_> = placed
            .fixed_sites
            .iter()
            .map(|range| (u32::from(range.start()), u32::from(range.end())))
            .collect();
        assert_eq!(
            positions,
            [(30, 54), (37, 54), (37, 49), (37, 38), (41, 49), (52, 54)]
        );
    }

    #[test]
    fn original_sites_survive_only_unchanged_checked_operations() {
        let text = concat!(
            "class Receiver:\n    def read(self, value: str) -> str:\n        return value\n",
            "def append(left: str, right: str) -> str:\n",
            "    return left + right\n",
            "print(append('a', 'b'))\n",
        );
        let mut graph = SourceGraph::single(text).unwrap();
        let crate::CheckedProgram::Native(fixed) = crate::checked_lower(&mut graph).unwrap() else {
            panic!("expected fixed checked operations")
        };
        let mut module = fixed.ir.0;
        let mut sites = fixed.frame_sources;
        let mut changed = sites.clone();
        crate::llvm::analyze_program(&module, &mut sites).unwrap();
        let (_, source) = sites.remove(&0).unwrap();
        let function = module.functions[0].clone();
        let body = function.body.as_ptr();
        let positions = source.positions.as_ptr();
        let placed = place_fixed_function(function, source);
        assert_eq!(placed.fixed_sites.as_ptr(), positions);
        assert_eq!(placed.function.body.as_ptr(), body);
        assert_eq!(placed.local_names[&0].name, "left");
        assert_eq!(placed.local_names[&1].name, "right");
        let spellings: Vec<_> = placed.fixed_sites.iter().map(|r| &text[*r]).collect();
        assert_eq!(
            spellings,
            ["return left + right", "left + right", "left", "right"]
        );

        // An optimizer can change a slot without changing the operation count or type.
        let [Statement::Return(result)] = module.functions[0].body.as_mut_slice() else {
            panic!()
        };
        let ExprKind::Binary(_, _, right) = &mut result.kind else {
            panic!()
        };
        right.kind = ExprKind::Local(0);
        crate::llvm::analyze_program(&module, &mut changed).unwrap();
        assert!(!changed.contains_key(&0));
    }
}
