use super::*;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::fixed_body_source::FixedNode;

    #[test]
    fn imported_function_keeps_its_return_site_after_the_file_entry_declines() {
        let dir = tempfile::tempdir().unwrap();
        let helper = dir.path().join("helper.py");
        let source = "def value(): return 7\n";
        std::fs::write(&helper, source).unwrap();
        let entry = dir.path().join("app.py");
        std::fs::write(
            &entry,
            "from helper import value\nbuf = bytearray()\nbuf.append(value())\nprint(buf)\n",
        )
        .unwrap();
        let mut graph = SourceGraph::load(&entry, None, &[]).unwrap();
        let owner = graph.modules.iter().position(|m| m.path == helper).unwrap();
        crate::checked_lower(&mut graph).unwrap();
        assert!(!graph.registered_functions().contains_key(&owner));
        let (functions, _) = graph.take_registered_functions();
        let body = &functions[&owner][&TextSize::default()];
        assert_eq!(&source[body.fixed_sites[0]], "return 7");
        assert_eq!(&source[body.fixed_sites[1]], "7");
    }

    #[test]
    fn module_binds_the_checked_call_after_its_declarations() {
        let text = concat!(
            "import json\n",
            "def render(data: int | None, spec: str) -> str:\n    return f'{data:{spec}}'\n\n",
            "print(ｒｅｎｄｅｒ(17, input()))\n",
        );
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app.py");
        std::fs::write(&path, text).unwrap();
        let mut graph = SourceGraph::load(&path, None, &[]).unwrap();
        assert!(matches!(
            crate::checked_lower(&mut graph).unwrap(),
            crate::CheckedProgram::Registered(_)
        ));
        let module = &graph.modules[graph.entry];
        let (_, mut initializers) = graph.take_registered_functions();
        assert_eq!(initializers.len(), 1);
        let entry = initializers.remove(&graph.entry).unwrap();
        let original = entry.function.body.as_ptr();
        let operations = HashMap::new();
        let mut prepare =
            SuitePreparation::new(&operations, module.registered_values().context(text), true);
        let suite = prepare.module(&module.parsed.syntax().body, Some((module, entry)));
        assert_eq!(suite.checked.function.body.as_ptr(), original);
        assert_eq!(suite.placed_from + 1, suite.checked.function.body.len());
        let site = suite.checked.fixed_sites[2];
        assert_eq!(&text[site], "ｒｅｎｄｅｒ(17, input())");
        let Some(FixedAttribute::Call(Some(target), _)) = suite.checked.fixed_attribute(site)
        else {
            panic!("expected the checked render call");
        };
        let ExprKind::Local(slot) = target.without_location().kind else {
            panic!("expected the installed module binding");
        };
        assert_eq!(suite.checked.local_names[&slot].name, "render");
    }

    #[test]
    fn recovered_function_keeps_its_checked_module_caller() {
        let text = include_str!("../../../../../tests/fixtures/callables/map_escaping.py");
        let project = tempfile::tempdir().unwrap();
        let path = project.path().join("app.py");
        std::fs::write(&path, text).unwrap();
        let mut graph = SourceGraph::load(&path, None, &[]).unwrap();
        crate::checked_lower(&mut graph).unwrap();

        let (_, initializers) = graph.take_registered_functions();
        let caller = &initializers[&graph.entry];
        assert!(matches!(
            caller.function.body.as_slice(),
            [Statement::Expr(Expr {
                kind: ExprKind::Call(..),
                ..
            })]
        ));
        let site = *caller.fixed_sites.last().unwrap();
        assert_eq!(&text[site], "run()");
        assert!(matches!(
            caller.fixed_attribute(site),
            Some(FixedAttribute::Call(Some(_), _))
        ));
    }

    #[test]
    fn module_prefix_keeps_the_checked_tails_named_operands() {
        let text = "def width(text: str) -> int: return len(text)\nsaved = 'é'\nprint(len(saved), width('name'))\n";
        let project = tempfile::tempdir().unwrap();
        let path = project.path().join("app.py");
        std::fs::write(&path, text).unwrap();
        let mut graph = SourceGraph::load(&path, None, &[]).unwrap();
        graph.registered_startup = true;
        crate::checked_lower(&mut graph).unwrap();
        let module = &graph.modules[graph.entry];
        let (_, mut initializers) = graph.take_registered_functions();
        let entry = initializers.remove(&graph.entry).unwrap();
        let Statement::Assign(saved, _) = entry.function.body[0] else {
            panic!("expected the checked store to saved");
        };
        let original = entry.function.body.as_ptr();
        let operations = HashMap::new();
        let mut prepare =
            SuitePreparation::new(&operations, module.registered_values().context(text), true);
        let suite = prepare.module(&module.parsed.syntax().body, Some((module, entry)));
        assert_eq!(suite.checked.function.body.as_ptr(), original);
        assert_eq!(suite.checked.local_names[&saved].name, "saved");
        assert!(suite.placed_from > 0);
    }

    #[test]
    fn imported_initializer_keeps_its_checked_tail_after_the_definition() {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("main.py");
        let text = "def increment(x: int) -> int: return x + 1\nprint('leaf initialized')\n";
        std::fs::write(dir.path().join("leaf.py"), text).unwrap();
        std::fs::write(&main, "from leaf import increment\nprint(increment(3))\n").unwrap();
        let mut graph = SourceGraph::load(&main, None, &[]).unwrap();
        graph.registered_startup = true;
        assert!(matches!(
            crate::checked_lower(&mut graph).unwrap(),
            crate::CheckedProgram::Registered(_)
        ));
        let mut seen = false;
        let (_, initializers) = graph.take_registered_functions();
        for (id, body) in initializers {
            if id == graph.entry {
                continue;
            }
            let module = &graph.modules[id];
            assert_eq!(module.name, "leaf");
            let original = body.function.body.as_ptr();
            let operations = HashMap::new();
            let mut prepare =
                SuitePreparation::new(&operations, module.registered_values().context(text), true);
            let suite = prepare.module(&module.parsed.syntax().body, Some((module, body)));
            assert_eq!(suite.checked.function.body.as_ptr(), original);
            assert_eq!(suite.placed_from + 1, suite.checked.function.body.len());
            assert!(!std::mem::replace(&mut seen, true));
        }
        assert!(seen);
    }

    #[test]
    fn a_settled_return_keeps_the_live_parser_call() {
        let text = concat!(
            "import argparse\nfrom pathlib import Path\n",
            "def options() -> argparse.Namespace:\n",
            "    parser = argparse.ArgumentParser()\n",
            "    parser.add_argument('--manifest', type=Path, required=True)\n",
            "    return parser.parse_args()\n",
            "print(options())\n",
        );
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app.py");
        std::fs::write(&path, text).unwrap();
        let start = TextSize::try_from(text.find("def options").unwrap()).unwrap();
        for named in [false, true] {
            let mut graph = SourceGraph::load(&path, None, &[]).unwrap();
            graph.registered_startup = named;
            let program = crate::checked_lower(&mut graph).unwrap();
            let (functions, _) = graph.take_registered_functions();
            let frame = functions
                .get(&graph.entry)
                .and_then(|bodies| bodies.get(&start));
            if named {
                assert!(matches!(program, crate::CheckedProgram::Registered(_)));
                let body = &frame.expect("expected the checked Python frame").function;
                assert_eq!(body.returns, Type::PythonObject);
                let Some(Statement::Return(value)) = body.body.last() else {
                    panic!("expected the live return");
                };
                let call = value.without_location().python_call().unwrap();
                assert_eq!(&text[call.range], "parser.parse_args()");
            } else {
                let crate::CheckedProgram::Native(fixed) = program else {
                    panic!("expected a fixed namespace result");
                };
                assert!(frame.is_none());
                let function = &fixed.ir.0.functions[0];
                let [Statement::Return(value)] = function.body.as_slice() else {
                    panic!("expected the fixed parser return");
                };
                assert!(matches!(
                    &value.without_location().kind,
                    ExprKind::Builtin(Builtin::ParseArgs { namespace, process_argv: true, .. }, _)
                        if function.returns == Type::Class(*namespace) && value.ty == function.returns
                ));
            }
        }
    }

    #[test]
    fn a_python_return_preserves_the_checked_scalar_and_its_live_caller() {
        let text = concat!(
            "import argparse\n",
            "def parse(args: list[str]) -> argparse.Namespace:\n",
            "    return argparse.ArgumentParser().parse_args(args)\n",
            "def adjusted(value: int) -> int:\n",
            "    return value * 3 + 2\n",
            "def through(value: int) -> int:\n",
            "    return adjusted(value)\n",
            "def add(left: float, right: float) -> float:\n",
            "    return left + right\n",
            "exec('print(through(7), add(1.5, 2.0), parse([]))')\n",
        );
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app.py");
        std::fs::write(&path, text).unwrap();
        let mut graph = SourceGraph::load(&path, None, &[]).unwrap();
        crate::checked_lower(&mut graph).unwrap();
        let functions = graph.registered_functions();
        for (declaration, returned, result) in [
            ("def adjusted", "return value * 3 + 2", Type::Int),
            ("def through", "return adjusted(value)", Type::Int),
            ("def add", "return left + right", Type::Float),
        ] {
            let start = TextSize::try_from(text.find(declaration).unwrap()).unwrap();
            let body = &functions[&graph.entry][&start];
            assert_eq!(body.function.returns, result);
            assert!(matches!(
                body.function.body.as_slice(),
                [Statement::Return(_)]
            ));
            assert_eq!(&text[body.fixed_sites[0]], returned);
            assert!(matches!(body.inputs, FrameInputs::Protocol { .. }));
        }
    }

    #[test]
    fn free_function_places_its_checked_field_store() {
        let text = concat!(
            "class Crate:\n",
            "    name: str\n",
            "    def __init__(self, name: str) -> None: self.name = name\n",
            "def rename(box: Crate, name: str) -> None:\n",
            "    box.name = name\n",
            "crate = Crate('old')\n",
            "rename(crate, 'first')\n",
            "print(crate.name)\n",
        );
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app.py");
        std::fs::write(&path, text).unwrap();
        let mut graph = SourceGraph::load(&path, None, &[]).unwrap();
        graph.registered_startup = true;
        crate::checked_lower(&mut graph).unwrap();
        let (functions, _) = graph.take_registered_functions();
        let start = TextSize::try_from(text.find("def rename").unwrap()).unwrap();
        let body = functions
            .get(&graph.entry)
            .unwrap()
            .get(&start)
            .expect("checked store");
        assert!(matches!(body.function.body.as_slice(),
            [Statement::SetField(receiver, _, value)]
            if matches!(receiver.kind, ExprKind::Local(0)) && matches!(value.kind, ExprKind::Local(1))));
        assert!(matches!(body.inputs, FrameInputs::Protocol { .. }));
        assert_eq!(
            body.fixed_sites
                .iter()
                .map(|site| &text[*site])
                .collect::<Vec<_>>(),
            ["box.name = name", "name", "box"]
        );
    }

    #[test]
    fn discovery_supplies_the_checked_field_and_its_python_receiver() {
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
        assert!(
            matches!(&attributes[&body.fixed_sites[1]], FixedAttribute::Load(name) if name == "name")
        );
        assert!(matches!(&body.inputs, FrameInputs::Protocol { sites }
            if matches!(sites.as_ref(), [FrameSite::Method(method)]
                if method.start() == start && method.name.as_str() == "pretty")));
        assert!(!lowered.iter().any(|name| name == "pretty"));
    }

    #[test]
    fn placed_module_read_keeps_its_slot_and_lexical_name() {
        let text = "other = 'old'\nKey = 'start'\ndef read() -> str: return Key\nprint(read())\n";
        let mut graph = SourceGraph::single(text).unwrap();
        graph.registered_startup = true;
        crate::checked_lower(&mut graph).unwrap();
        let functions = graph.registered_functions();
        let start = TextSize::try_from(text.find("def read").unwrap()).unwrap();
        let body = &functions[&graph.entry][&start];
        let [Statement::Return(value)] = &body.function.body[..] else {
            panic!("expected the placed module read")
        };
        let slot = match &value.kind {
            ExprKind::Builtin(Builtin::GlobalGet(slot), args) if args.is_empty() => *slot,
            _ => panic!("expected the original global slot"),
        };
        let [statement, name] = body.fixed_sites[..] else {
            panic!("expected the placed statement and expression")
        };
        assert_eq!(&text[statement], "return Key");
        assert_eq!(&text[name], "Key");
        let read = &body.fixed_attributes.as_ref().unwrap()[&name];
        assert!(matches!(read, FixedAttribute::ModuleName(key) if key == "Key"));
        let binding = FixedNode::checked(&ir::Node::Expression(value)).unwrap();
        for kind in [
            ExprKind::Builtin(Builtin::GlobalGet(slot + 1), vec![]),
            ExprKind::Builtin(Builtin::GlobalGet(slot), vec![value.clone()]),
            ExprKind::Local(slot),
        ] {
            let changed = Expr { ty: value.ty, kind };
            assert!(FixedNode::checked(&ir::Node::Expression(&changed)) != Some(binding.clone()));
        }

        let project = tempfile::tempdir().unwrap();
        let entry = project.path().join("app.py");
        std::fs::write(&entry, text).unwrap();
        let options = crate::CompileOptions {
            module_entry: Some("app".into()),
            extension_paths: vec![project.path().to_owned()],
            ..Default::default()
        };
        let llvm = crate::compile_file(&entry, None, &options).unwrap().llvm_ir;
        let key = llvm
            .lines()
            .find_map(|line| {
                line.strip_suffix(r#" = private unnamed_addr constant [4 x i8] c"Key\00""#)
            })
            .expect("normalized module binding");
        assert!(llvm.contains(&format!(
            "@imre_dyn_lookup(ptr %ctx, i64 %scope, ptr {key}, i64 3)"
        )));
    }

    #[test]
    fn fixed_float_sites_distinguish_zero_signs_and_nan_payloads() {
        let key = |bits| {
            let value = Expr {
                ty: Type::Float,
                kind: ExprKind::Float(f64::from_bits(bits)),
            };
            FixedNode::checked(&ir::Node::Expression(&value)).unwrap()
        };
        let first_nan = f64::NAN.to_bits();
        assert!(key(0.0_f64.to_bits()) != key((-0.0_f64).to_bits()));
        assert!(key(first_nan) == key(first_nan));
        assert!(key(first_nan) != key(first_nan + 1));
    }

    #[test]
    fn fallthrough_keeps_each_executable_source_position() {
        let text = "def nothing():\n    pass\n    pass\nprint(nothing())\n";
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app.py");
        std::fs::write(&path, text).unwrap();
        let mut graph = SourceGraph::load(&path, None, &[]).unwrap();
        graph.registered_startup = true;
        crate::checked_lower(&mut graph).unwrap();
        let functions = graph.registered_functions();
        let body = &functions[&graph.entry][&TextSize::default()];
        let [
            Statement::Position(first, None),
            Statement::Position(last, None),
        ] = body.function.body.as_slice()
        else {
            panic!("expected both checkpoints and an implicit None return");
        };
        assert_eq!(&*body.fixed_sites, &[*first, *last]);
        assert_eq!((&text[*first], &text[*last]), ("pass", "pass"));
        assert!(first.end() < last.start());
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
    fn named_full_list_slice_places_the_checked_receiver() {
        let text = "def copy(items: list[str]): return items[:]\nprint('ready')\n";
        let mut graph = SourceGraph::single(text).unwrap();
        graph.registered_startup = true;
        crate::checked_lower(&mut graph).unwrap();
        let functions = graph.registered_functions();
        let placed = &functions[&graph.entry][&TextSize::default()];
        let sites: Vec<_> = placed
            .fixed_sites
            .iter()
            .map(|range| &text[*range])
            .collect();
        assert_eq!(sites, ["return items[:]", "items[:]", "items"]);
        let [Statement::Return(value)] = &placed.function.body[..] else {
            panic!("expected the original checked return")
        };
        assert!(
            matches!(&value.kind, ExprKind::Builtin(Builtin::Slice, args)
            if matches!(args.as_slice(), [Expr { kind: ExprKind::Local(slot), .. }]
                if placed.local_names[slot].name == "items"))
        );
    }

    #[test]
    fn late_registered_entry_places_only_existing_frames() {
        let text = "def scale(x: int) -> int: return x + x\ndef startup(source: str) -> None: exec(source)\nprint(scale(3))\n";
        let mut graph = SourceGraph::single(text).unwrap();
        let (program, lowered) =
            caller_cache::observe_lowerings(|| crate::checked_lower(&mut graph).unwrap());
        assert!(matches!(program, crate::CheckedProgram::Registered(_)));
        assert_eq!(lowered.iter().filter(|name| *name == "startup").count(), 1);
        let functions = graph.registered_functions();
        assert_eq!(functions[&graph.entry].len(), 1);
        assert!(
            !functions[&graph.entry][&TextSize::default()]
                .fixed_sites
                .is_empty()
        );
    }

    #[test]
    fn named_callback_keeps_its_checked_callable_and_argument() {
        let text = include_str!("../../../../../tests/fixtures/callables/cache_parameters.py");
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app.py");
        std::fs::write(&path, text).unwrap();
        let mut graph = SourceGraph::load(&path, None, &[]).unwrap();
        graph.registered_startup = true;
        let (_, lowered) =
            caller_cache::observe_lowerings(|| crate::checked_lower(&mut graph).unwrap());
        assert_eq!(lowered.iter().filter(|name| *name == "call").count(), 1);
        let start = TextSize::try_from(text.find("def call(").unwrap()).unwrap();
        let functions = graph.registered_functions();
        let body = &functions[&graph.entry][&start];
        let sites: Vec<_> = body.fixed_sites.iter().map(|range| &text[*range]).collect();
        assert_eq!(sites, ["return fn(value)", "fn(value)", "fn", "value"]);
    }

    #[test]
    fn named_list_method_uses_the_checked_receiver_and_argument() {
        let fixture =
            include_str!("../../../../../tests/fixtures/truthiness/conditional_contexts.py");
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app.py");
        std::fs::write(&path, fixture).unwrap();
        let mut graph = SourceGraph::load(&path, None, &[]).unwrap();
        graph.registered_startup = true;
        let (_, lowered) =
            caller_cache::observe_lowerings(|| crate::checked_lower(&mut graph).unwrap());
        assert_eq!(lowered.iter().filter(|name| *name == "right").count(), 1);

        let text = &graph.modules[graph.entry].source;
        let start = TextSize::try_from(text.find("def right").unwrap()).unwrap();
        let functions = graph.registered_functions();
        let placed = &functions[&graph.entry][&start];
        let attributes = placed.fixed_attributes.as_ref().unwrap();
        assert_eq!(attributes.len(), 1);
        let (range, attribute) = attributes.iter().next().unwrap();
        assert_eq!(&text[*range], "events.append(mark)");
        let FixedAttribute::Call(None, site) = attribute else {
            panic!("expected bound method")
        };
        assert!(
            matches!(&site.target, ir::RegisteredCallTarget::Method { name, attribute, .. }
            if name == "append" && &text[*attribute] == "events.append")
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
            "def erased(n: int) -> int: return +n\n",
            "def occupied(n: int) -> int: return n\n",
            "def last(n: int) -> int: return n + 1\n",
            "print(append('a', 'b'))\n",
        );
        let mut graph = SourceGraph::single(text).unwrap();
        let crate::CheckedProgram::Native(mut fixed) = crate::checked_lower(&mut graph).unwrap()
        else {
            panic!("expected fixed checked operations")
        };
        let body = fixed.ir.0.functions[0].body.as_ptr();
        let &(_, definition, ref source) = fixed
            .frame_sources
            .iter()
            .find(|(id, ..)| *id == 0)
            .unwrap();
        let positions = source
            .clone()
            .place(fixed.ir.0.functions[0].clone())
            .ok()
            .unwrap()
            .fixed_sites;
        let changed = TextSize::try_from(text.find("def changed").unwrap()).unwrap();
        let [Statement::Return(result)] = fixed.ir.0.functions[1].body.as_mut_slice() else {
            panic!()
        };
        let ExprKind::Binary(_, _, right) = &mut result.kind else {
            panic!()
        };
        right.kind = ExprKind::Local(0);
        let [Statement::Return(erased)] = fixed.ir.0.functions[2].body.as_mut_slice() else {
            panic!()
        };
        *erased = Expr::local(0, Type::Int);
        let occupied = TextSize::try_from(text.find("def occupied").unwrap()).unwrap();
        let (slot, _, source) = fixed
            .frame_sources
            .iter()
            .find(|(_, range, _)| range.start() == occupied)
            .unwrap();
        let earlier = source
            .clone()
            .place(fixed.ir.0.functions[*slot].clone())
            .ok()
            .unwrap();
        let earlier_body = earlier.function.body.as_ptr();
        graph.register_function(graph.entry, occupied, earlier);
        let last = TextSize::try_from(text.find("def last").unwrap()).unwrap();
        let mut checked = Vec::new();
        FixedFunctions {
            functions: fixed.ir.0.functions,
            frames: fixed.frame_sources,
        }
        .register(&graph, &fixed.ir.1, |module, range| {
            checked.push((module, range.start()));
            true
        });
        assert_eq!(
            checked,
            [(graph.entry, definition.start()), (graph.entry, last)]
        );
        let functions = graph.registered_functions();
        let definitions = &functions[&graph.entry];
        assert!(!definitions.contains_key(&changed));
        assert_eq!(definitions[&occupied].function.body.as_ptr(), earlier_body);
        let placed = &definitions[&definition.start()];
        assert_eq!(placed.function.returns, Type::Str);
        assert_eq!(placed.fixed_sites, positions);
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
