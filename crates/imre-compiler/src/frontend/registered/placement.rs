use super::*;

/// Register an existing checked function with its original frame positions.
/// The caller has matched these positions to the current operations.
/// The registered frame boxes each returned value during emission.
pub(crate) fn place_fixed_function(
    function: ir::Function,
    source: FixedBodySource,
    inputs: FrameInputs,
) -> RegisteredFunction {
    RegisteredFunction {
        inputs,
        function,
        local_names: source.local_names,
        fixed_sites: source.positions,
        fixed_attributes: (!source.attributes.is_empty()).then(|| Box::new(source.attributes)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovering_a_method_keeps_the_checked_field_and_its_python_receiver() {
        let text = concat!(
            "import argparse, dataclasses\n",
            "@dataclasses.dataclass\n",
            "class Crate:\n",
            "    name: str\n",
            "    def pretty(self) -> str:\n",
            "        return self.name\n",
            "def options() -> argparse.Namespace:\n",
            "    return argparse.ArgumentParser().parse_args([])\n",
            "print(Crate('core').pretty(), options())\n",
        );
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app.py");
        std::fs::write(&path, text).unwrap();
        let mut graph = SourceGraph::load(&path, None, &[]).unwrap();
        let (_, lowered) =
            caller_cache::observe_lowerings(|| crate::checked_lower(&mut graph).unwrap());
        let functions = graph.registered_functions();
        let start = TextSize::try_from(text.find("def pretty").unwrap()).unwrap();
        let body = &functions[&graph.entry][&start];
        let [Statement::Return(field)] = body.function.body.as_slice() else {
            panic!("expected the checked field return")
        };
        assert!(matches!(&field.kind, ExprKind::Field(receiver, 0)
            if matches!(receiver.kind, ExprKind::Local(0))));
        let positions: Vec<_> = body.fixed_sites.iter().map(|range| &text[*range]).collect();
        assert_eq!(positions, ["return self.name", "self.name", "self"]);
        let attributes = body.fixed_attributes.as_ref().unwrap();
        assert!(matches!(&*attributes[&body.fixed_sites[1]].operation,
            Builtin::RegisteredAttribute { name, .. } if name == "name"));
        assert!(matches!(&body.inputs, FrameInputs::Protocol { sites }
            if matches!(sites.as_ref(), [FrameSite::Method(method)]
                if method.start() == start && method.name.as_str() == "pretty")));
        assert_eq!(lowered.iter().filter(|name| *name == "pretty").count(), 1);
    }

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
        assert_eq!(body.function.returns, Type::Int);
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
    fn named_list_method_uses_the_checked_receiver_and_argument() {
        let text = include_str!("../../../../../tests/fixtures/truthiness/conditional_contexts.py");
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app.py");
        std::fs::write(&path, text).unwrap();
        let mut graph = SourceGraph::load(&path, None, &[]).unwrap();
        graph.registered_startup = true;
        let (_, lowered) =
            caller_cache::observe_lowerings(|| crate::checked_lower(&mut graph).unwrap());
        assert_eq!(lowered.iter().filter(|name| *name == "right").count(), 1);

        let start = TextSize::try_from(text.find("def right").unwrap()).unwrap();
        let functions = graph.registered_functions();
        let placed = &functions[&graph.entry][&start];
        let attributes = placed.fixed_attributes.as_ref().unwrap();
        assert_eq!(attributes.len(), 1);
        let (range, attribute) = attributes.iter().next().unwrap();
        assert_eq!(&text[*range], "events.append(mark)");
        assert!(
            matches!(&*attribute.operation, Builtin::RegisteredCall(call)
            if matches!(&call.target, ir::RegisteredCallTarget::Method { name, attribute, .. }
                if name == "append" && &text[*attribute] == "events.append"))
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
        assert_eq!(function.returns, Type::Int);
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
    fn earlier_provider_body_keeps_the_checked_scalar_sibling() {
        let text = concat!(
            "import traceback\n",
            "def unused():\n    return traceback.extract_stack()\n",
            "def scalar(n: int) -> int:\n    return n * 2\n",
            "print(scalar(4))\n",
        );
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app.py");
        std::fs::write(&path, text).unwrap();
        let mut graph = SourceGraph::load(&path, None, &[]).unwrap();
        let (_, lowerings) =
            caller_cache::observe_lowerings(|| crate::checked_lower(&mut graph).unwrap());
        assert_eq!(lowerings.iter().filter(|name| *name == "scalar").count(), 1);
        let scalar = TextSize::try_from(text.find("def scalar").unwrap()).unwrap();
        let functions = graph.registered_functions();
        let placed = &functions[&graph.entry][&scalar];
        assert_eq!(placed.function.returns, Type::Int);
        assert_eq!(&text[placed.fixed_sites[0]], "return n * 2");
    }

    #[test]
    fn imported_exception_handler_keeps_the_checked_arithmetic_body() {
        let text = concat!(
            "def scale(n: int) -> int:\n    return n * 3 + 1\n",
            "from urllib.error import URLError as NetworkError\n",
            "try:\n    int('bad')\n",
            "except NetworkError:\n    pass\n",
            "except ValueError:\n    print(scale(7))\n",
        );
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app.py");
        std::fs::write(&path, text).unwrap();
        let mut graph = SourceGraph::load(&path, None, &[]).unwrap();
        crate::checked_lower(&mut graph).unwrap();
        let functions = graph.registered_functions();
        let placed = &functions[&graph.entry][&TextSize::default()];
        assert_eq!(placed.function.locals[placed.function.params[0]], Type::Int);
        let sites: Vec<_> = placed
            .fixed_sites
            .iter()
            .map(|range| &text[*range])
            .collect();
        assert_eq!(
            sites,
            ["return n * 3 + 1", "n * 3 + 1", "n * 3", "n", "3", "1"]
        );
    }

    #[test]
    fn original_sites_survive_only_unchanged_checked_operations() {
        let text = concat!(
            "class Receiver:\n    def read(self, value: str) -> str:\n        return value\n",
            "def append(left: str, right: str) -> str:\n",
            "    return left + right\n",
            "def changed(left: str, right: str) -> str:\n",
            "    return left + right\n",
            "print(append('a', 'b'))\n",
        );
        let mut graph = SourceGraph::single(text).unwrap();
        let crate::CheckedProgram::Native(mut fixed) = crate::checked_lower(&mut graph).unwrap()
        else {
            panic!("expected fixed checked operations")
        };
        let body = fixed.ir.0.functions[0].body.as_ptr();
        let &(_, start, ref source) = fixed
            .frame_sources
            .iter()
            .find(|(id, ..)| *id == 0)
            .unwrap();
        let positions = source.positions.as_ptr();
        let changed = TextSize::try_from(text.find("def changed").unwrap()).unwrap();
        let [Statement::Return(result)] = fixed.ir.0.functions[1].body.as_mut_slice() else {
            panic!()
        };
        let ExprKind::Binary(_, _, right) = &mut result.kind else {
            panic!()
        };
        right.kind = ExprKind::Local(0);
        fixed.register_definitions(&graph);
        let functions = graph.registered_functions();
        let definitions = &functions[&graph.entry];
        assert!(!definitions.contains_key(&changed));
        let placed = &definitions[&start];
        assert_eq!(placed.function.returns, Type::Str);
        assert_eq!(placed.fixed_sites.as_ptr(), positions);
        assert_eq!(placed.function.body.as_ptr(), body);
        assert_eq!(placed.local_names[&0].name, "left");
        assert_eq!(placed.local_names[&1].name, "right");
        let spellings: Vec<_> = placed.fixed_sites.iter().map(|r| &text[*r]).collect();
        assert_eq!(
            spellings,
            ["return left + right", "left + right", "left", "right"]
        );
    }
}
