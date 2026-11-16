use super::*;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::fixed_body_source::FixedNode;

    #[test]
    fn a_discarded_call_does_not_hide_a_sibling_argument_contract() {
        let project = tempfile::tempdir().unwrap();
        let path = project.path().join("app.py");
        let prefix = concat!(
            "import json\nfrom pathlib import Path\n\n",
            "def value() -> int:\n    sorted(Path(\".\").glob(\"*.py\"))\n    return 7\n\n",
            "def take(number: int) -> None:\n    print(number)\n\n",
            "def run() -> None:\n",
            "    item = json.loads(\"0\")\n    print(f\"{item}\")\n",
        );
        for (calls, result_hint) in [
            (["value()", "value()"], Some(Type::Int)),
            (["value()", "take(value())"], None),
            (["take(value())", "value()"], None),
        ] {
            let source = format!("{prefix}    {}\n    {}\n\nrun()\n", calls[0], calls[1]);
            std::fs::write(&path, &source).unwrap();
            let mut graph = SourceGraph::load(&path, None, &[]).unwrap();
            crate::checked_lower(&mut graph).unwrap();
            let start = TextSize::try_from(source.find("def run").unwrap()).unwrap();
            let (functions, _) = graph.take_registered_functions();
            let frame = &functions[&graph.entry][&start];
            let mut hints = Vec::new();
            ir::walk_nodes(&frame.function.body, &mut |node, _| {
                if let ir::Node::Expression(value) = node
                    && let Some(call) = value.python_call()
                    && &source[call.range] == "value()"
                {
                    hints.push(call.position.unwrap().1);
                }
            });
            assert_eq!(hints, [result_hint; 2], "{calls:?}");
        }
    }

    #[test]
    fn a_call_argument_can_bind_its_own_assignment_destination() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app.py");
        for (name, hint) in [("other", Some(Type::Int)), ("x", None)] {
            let call = format!("value({name} := 3)");
            let source = format!(
                "import json\nfrom pathlib import Path\n\n\
                 def value(n: int) -> int:\n    sorted(Path('.').glob('*.txt'))\n    return n + 4\n\n\
                 def run() -> None:\n    item = json.loads('0')\n    print(f'{{item}}')\n    x = {call}\n    print(x)\n\nrun()\n"
            );
            std::fs::write(&path, &source).unwrap();
            let mut graph = SourceGraph::load(&path, None, &[]).unwrap();
            crate::checked_lower(&mut graph).unwrap();
            let start = TextSize::try_from(source.find("def run").unwrap()).unwrap();
            let (functions, _) = graph.take_registered_functions();
            let body = &functions[&graph.entry][&start].function.body;
            let mut hints = Vec::new();
            ir::walk_nodes(body, &mut |node, _| {
                if let ir::Node::Expression(value) = node
                    && let Some(site) = value.python_call()
                    && &source[site.range] == call
                {
                    hints.push(site.position.unwrap().1);
                }
            });
            assert_eq!(hints, [hint], "{call}");
        }
    }

    #[test]
    fn a_direct_local_function_preserves_its_callers_checked_return() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("report.py");
        for source in [
            concat!(
                "def report(value: int) -> str:\n",
                "    def clean(x: int) -> str:\n",
                "        return str(x)\n",
                "    return clean(value)\n",
                "print(report(40))\n",
            ),
            concat!(
                "def format_table(values: list[int]) -> str:\n",
                "    def show(count: int, *, bracket: bool = True) -> str:\n",
                "        return '[' + str(count) + ']' if bracket else str(count)\n",
                "    values.sort(key=lambda v: -v, reverse=False)\n",
                "    return show(len(values)) + ' ' + str(values)\n",
                "print(format_table([3, 1, 2]))\n",
            ),
        ] {
            std::fs::write(&path, source).unwrap();
            let mut graph = SourceGraph::load(&path, None, &[]).unwrap();
            graph.registered_startup = true;
            crate::checked_lower(&mut graph).unwrap();
            let functions = graph.registered_functions();
            let caller = &functions[&graph.entry][&TextSize::default()];
            assert_eq!(caller.function.returns, Type::Str, "{source}");
        }
    }

    #[test]
    fn a_direct_local_body_checks_the_element_type_before_placing_its_caller() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("__main__.py"),
            concat!(
                "def report(value: int) -> str:\n",
                "    def clean(x: int) -> str:\n",
                "        xs: list[int] = []\n",
                "        flag: bool = True\n",
                "        xs.append(flag)\n",
                "        return str(x)\n",
                "    return clean(value)\n",
                "print(report(40))\n",
            ),
        )
        .unwrap();
        let options = crate::CompileOptions {
            path_entry: true,
            module_entry: Some("__main__".into()),
            script_startup: Some((Default::default(), "imre".into())),
            ..Default::default()
        };
        let error = crate::compile_file(dir.path(), None, &options)
            .err()
            .expect("the nested append must enforce its annotated element type");
        assert!(
            error.contains("5:19: Imre: expected int, got bool"),
            "{error}"
        );
    }

    #[test]
    fn method_recovery_uses_the_operands_from_its_completed_fixed_check() {
        let text = concat!(
            "class Row:\n",
            "    def values(self, data: list[int]) -> list[int]: return data\n",
            "    def count(self, data: list[int]) -> int:\n",
            "        values = self.values(data)\n",
            "        if values:\n",
            "            return len({value for value in values})\n",
            "        return 0\n",
            "print(Row().count([1, 1, 2]))\n",
        );
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app.py");
        std::fs::write(&path, text).unwrap();
        let mut graph = SourceGraph::load(&path, None, &[]).unwrap();
        graph.registered_startup = true;
        let (_, lowered) =
            caller_cache::observe_lowerings(|| crate::checked_lower(&mut graph).unwrap());
        let start = TextSize::try_from(text.find("def count").unwrap()).unwrap();
        assert!(graph.registered_functions()[&graph.entry].contains_key(&start));
        assert_eq!(lowered.iter().filter(|name| *name == "count").count(), 2);
    }

    #[test]
    fn named_classmethod_factory_keeps_its_checked_arguments() {
        let text = concat!(
            "from dataclasses import dataclass\n",
            "from typing import Any, Self\n",
            "@dataclass\n",
            "class Message:\n",
            "    text: str\n",
            "    @classmethod\n",
            "    def from_mapping(cls, row: dict[str, Any]) -> Self:\n",
            "        return cls(text=row['name'])\n",
            "print(Message.from_mapping({'name': 'example'}).text)\n",
        );
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("example.py");
        std::fs::write(&path, text).unwrap();
        let mut graph = SourceGraph::load(&path, None, &[]).unwrap();
        graph.registered_startup = true;
        let (_, lowered) =
            caller_cache::observe_lowerings(|| crate::checked_lower(&mut graph).unwrap());
        assert_eq!(
            lowered
                .iter()
                .filter(|name| *name == "from_mapping")
                .count(),
            2
        );

        let start = TextSize::try_from(text.find("@classmethod").unwrap()).unwrap();
        let functions = graph.registered_functions();
        let method = &functions[&graph.entry][&start];
        assert!(matches!(&method.inputs, FrameInputs::Protocol { sites }
            if sites.iter().any(|site| matches!(site, FrameSite::Method(_)))));
    }

    #[test]
    fn class_method_keeps_its_checked_return_after_the_factory_needs_a_python_frame() {
        let text = concat!(
            include_str!("../../../../../tests/fixtures/callables/escaping_bound_method.py"),
            "\ndef unopened() -> tuple[Callable[[], int], Callable[[], int]]:\n",
            "    cb = outer()\n",
            "    return cb, cb\n",
        );
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("capture.py");
        std::fs::write(&path, text).unwrap();
        let mut graph = SourceGraph::load(&path, None, &[]).unwrap();
        let (_, lowered) =
            caller_cache::observe_lowerings(|| crate::checked_lower(&mut graph).unwrap());
        let source = &graph.modules[graph.entry].source;
        let start = TextSize::try_from(source.find("@classmethod").unwrap()).unwrap();
        let functions = graph.registered_functions();
        let method = &functions[&graph.entry][&start];
        assert!(matches!(method.inputs, FrameInputs::Protocol { .. }));
        assert_eq!(method.local_names[&method.function.params[0]].name, "cls");
        assert_eq!(method.local_names[&method.function.params[1]].name, "n");
        let sites: Vec<_> = method
            .fixed_sites
            .iter()
            .map(|site| &source[*site])
            .collect();
        assert_eq!(sites, ["return n + 1", "n + 1", "n", "1"]);
        assert_eq!(lowered.iter().filter(|name| *name == "pick").count(), 1);
    }

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
        let owner = graph
            .modules
            .iter()
            .position(|m| m.name == "helper")
            .unwrap();
        crate::checked_lower(&mut graph).unwrap();
        assert!(!graph.registered_functions().contains_key(&owner));
        let (functions, _) = graph.take_registered_functions();
        let body = &functions[&owner][&TextSize::default()];
        assert_eq!(&source[body.fixed_sites[0]], "return 7");
        assert_eq!(&source[body.fixed_sites[1]], "7");
    }

    #[test]
    fn checked_file_argument_reads_the_class_installed_by_its_declaration() {
        let text = concat!(
            "class State(str): pass\n",
            "def read(cls) -> str: return cls.__name__\n",
            "print(read(State))\n",
        );
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app.py");
        std::fs::write(&path, text).unwrap();
        let mut graph = SourceGraph::load(&path, None, &[]).unwrap();
        crate::checked_lower(&mut graph).unwrap();
        let (_, mut initializers) = graph.take_registered_functions();
        let entry = initializers.remove(&graph.entry).unwrap();
        assert!(
            entry
                .fixed_sites
                .iter()
                .any(|range| &text[*range] == "State")
        );
        let original = entry.function.body.as_ptr();
        let module = &graph.modules[graph.entry];
        let operations = HashMap::new();
        let mut prepare =
            SuitePreparation::new(&operations, module.registered_values().context(text), true);
        let suite = prepare.module(&module.parsed.syntax().body, Some((module, entry)));
        assert_eq!(suite.checked.function.body.as_ptr(), original);
        assert_eq!(
            suite.checked.placed_from + 1,
            suite.checked.function.body.len()
        );
    }

    #[test]
    fn checked_file_calls_the_live_class_and_its_method() {
        let text = concat!(
            "class State(str):\n",
            "    def read(self): return 1\n",
            "print(State().read())\n",
        );
        let dir = tempfile::tempdir().unwrap();
        let entry = dir.path().join("app.py");
        std::fs::write(&entry, text).unwrap();
        let mut graph = SourceGraph::load(&entry, None, &[]).unwrap();
        crate::checked_lower(&mut graph).unwrap();
        let (_, mut initializers) = graph.take_registered_functions();
        let body = initializers.remove(&graph.entry).unwrap();
        let written: Vec<_> = body.fixed_sites.iter().map(|range| &text[*range]).collect();
        assert_eq!(
            written,
            [
                "print(State().read())",
                "print(State().read())",
                "print",
                "State().read()",
                "State()",
                "State",
            ]
        );
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
        assert_eq!(
            suite.checked.placed_from + 1,
            suite.checked.function.body.len()
        );
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
    fn placed_module_calls_the_current_binding_without_native_defaults() {
        let source = "from provider import render\nprint(render())\nprint(render(\"four\"))\n";
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("main.py");
        std::fs::write(&path, source).unwrap();
        std::fs::write(
            dir.path().join("provider.py"),
            concat!(
                "import sys\nimport changed_provider\n",
                "def render(indent: str | None = None) -> int: return 3\n",
                "sys.modules[__name__] = changed_provider\n",
            ),
        )
        .unwrap();
        std::fs::write(
            dir.path().join("changed_provider.py"),
            concat!(
                "import sys\ndef second(value): return 91\n",
                "def first(value=None):\n    sys.modules['__main__'].render = second\n    return 37\n",
                "def __getattr__(name):\n    if name == 'render': return first\n    raise AttributeError(name)\n",
            ),
        )
        .unwrap();
        let mut graph = SourceGraph::load(&path, None, &[]).unwrap();
        crate::checked_lower(&mut graph).unwrap();
        let module = &graph.modules[graph.entry];
        let (_, mut initializers) = graph.take_registered_functions();
        let entry = initializers.remove(&graph.entry).unwrap();
        let original = entry.function.body.as_ptr();
        let operations = HashMap::new();
        let mut prepare = SuitePreparation::new(
            &operations,
            module.registered_values().context(source),
            true,
        );
        let suite = prepare.module(&module.parsed.syntax().body, Some((module, entry)));
        assert_eq!(suite.checked.function.body.as_ptr(), original);
        let mut calls = Vec::new();
        ir::walk_nodes(
            &suite.checked.function.body[suite.checked.placed_from..],
            &mut |node, _| {
                if let ir::Node::Expression(Expr {
                    kind: ExprKind::Invoke { args, .. },
                    ..
                }) = node
                {
                    let site = args.site.python().unwrap();
                    calls.push((&source[site.call.range], args.values.len()));
                }
            },
        );
        assert_eq!(calls, [("render()", 0)]);
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
        assert_eq!(&graph.modules[graph.entry].source[site], "run()");
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
        assert!(suite.checked.placed_from > 0);
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
            assert_eq!(
                suite.checked.placed_from + 1,
                suite.checked.function.body.len()
            );
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
            let (program, lowerings) =
                caller_cache::observe_lowerings(|| crate::checked_lower(&mut graph).unwrap());
            assert_eq!(
                lowerings.iter().filter(|name| *name == "options").count(),
                usize::from(named),
                "{lowerings:?}"
            );
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
    fn a_checked_match_body_keeps_its_prefix_and_enforces_its_declared_operands() {
        let source = concat!(
            "def label(value: float, *, better: bool = True) -> str:\n",
            "    if value == 0: return ''\n",
            "    increased = value > 0\n",
            "    items: list[int] = [7]\n",
            "    match (better, increased):\n",
            "        case (True, True):\n",
            "            items.append(4)\n",
            "            return 'up'\n",
            "        case _: return 'other'\n",
            "print(label(3.0))\n",
        );
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("report.py");
        std::fs::write(&file, source).unwrap();
        let mut graph = SourceGraph::load(&file, None, &[]).unwrap();
        graph.registered_startup = true;
        let (_, lowered) =
            caller_cache::observe_lowerings(|| crate::checked_lower(&mut graph).unwrap());
        let functions = graph.registered_functions();
        let frame = &functions[&graph.entry][&TextSize::default()];
        assert!(matches!(
            frame.function.body.last(),
            Some(Statement::Match(_, _))
        ));
        assert_eq!(lowered.iter().filter(|name| *name == "label").count(), 2);
        std::fs::write(
            dir.path().join("__main__.py"),
            source.replace("append(4)", "append(True)"),
        )
        .unwrap();
        let options = crate::CompileOptions {
            path_entry: true,
            module_entry: Some("__main__".into()),
            script_startup: Some((Default::default(), "imre".into())),
            ..Default::default()
        };
        let error = crate::compile_file(dir.path(), None, &options)
            .err()
            .expect("the case must check its list element");
        assert!(
            error.contains("7:26: Imre: expected int, got bool"),
            "{error}"
        );
    }

    #[test]
    fn an_annotated_mapping_keeps_its_checked_python_return() {
        let source = concat!(
            "import json\n",
            "def mapping(text: str) -> dict[str, int]:\n",
            "    value = json.loads(text)\n",
            "    assert isinstance(value, dict)\n",
            "    return value\n",
            "print(mapping('{}'))\n",
        );
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app.py");
        std::fs::write(&path, source).unwrap();
        let mut graph = SourceGraph::load(&path, None, &[]).unwrap();
        crate::checked_lower(&mut graph).unwrap();
        let (functions, _) = graph.take_registered_functions();
        let start = TextSize::try_from(source.find("def mapping").unwrap()).unwrap();
        let frame = &functions[&graph.entry][&start];
        let Some(Statement::Return(value)) = frame.function.body.last() else {
            panic!("expected the checked return");
        };
        assert_eq!(value.ty, Type::PythonObject);
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
    fn the_ordinary_method_check_supplies_the_field_and_its_python_receiver() {
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
        assert_eq!(lowered.iter().filter(|name| *name == "pretty").count(), 1);
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
    fn checked_search_branch_keeps_its_two_slice_receivers_in_a_python_frame() {
        let text = concat!(
            "def section(text: str, marker: str) -> str:\n",
            "    at = text.find(marker)\n",
            "    if at == -1 or at == 0:\n",
            "        return text[:at]\n",
            "    return text[at:]\n",
            "def startup(source: str) -> None: exec(source)\n",
            "print(section('prefix', 'fix'))\n",
        );
        for named in [false, true] {
            let mut graph = SourceGraph::single(text).unwrap();
            graph.registered_startup = named;
            crate::checked_lower(&mut graph).unwrap();
            let (functions, _) = graph.take_registered_functions();
            let body = &functions[&graph.entry][&TextSize::default()];
            let written: Vec<_> = body.fixed_sites.iter().map(|site| &text[*site]).collect();
            for operation in [
                "text.find(marker)",
                "at == -1 or at == 0",
                "text[:at]",
                "text[at:]",
            ] {
                assert!(
                    written.contains(&operation),
                    "missing {operation} in {written:?}"
                );
            }
            assert!(matches!(body.inputs, FrameInputs::Protocol { .. }));
        }
    }

    #[test]
    fn placed_reader_keeps_the_checked_calls_and_failure_branch() {
        let text = concat!(
            "from pathlib import Path\n",
            "def read(path: Path) -> str:\n",
            "    line = path.read_text(encoding='utf-8').strip()\n",
            "    if not line: raise ValueError(f'empty {path}')\n",
            "    item = line.split()[0]\n",
            "    if len(item) != 2: raise ValueError(f'bad {item!r}')\n",
            "    return item\n",
            "def startup(source: str) -> None: exec(source)\n",
            "print(read(Path('input')))\n",
        );
        let mut graph = SourceGraph::single(text).unwrap();
        crate::checked_lower(&mut graph).unwrap();
        let (functions, _) = graph.take_registered_functions();
        let start = TextSize::try_from(text.find("def read").unwrap()).unwrap();
        let body = &functions[&graph.entry][&start];
        assert_eq!(body.function.body.len(), 5);
        assert!(matches!(body.inputs, FrameInputs::Protocol { .. }));
        assert!(
            body.fixed_attributes
                .as_deref()
                .unwrap()
                .iter()
                .any(|(range, attribute)| {
                    &text[*range] == "path.read_text(encoding='utf-8')"
                        && matches!(attribute, FixedAttribute::CallValue { placed: true, .. })
                })
        );
        for operation in [
            "path.read_text(encoding='utf-8')",
            "not line",
            "line.split()[0]",
            "{item!r}",
        ] {
            assert!(
                body.fixed_sites
                    .iter()
                    .any(|site| &text[*site] == operation),
                "{operation}"
            );
        }
    }

    fn checked_exec_operand(body: &RegisteredFunction) -> (&ir::RegisteredCall, &Expr) {
        let [
            Statement::Expr(Expr {
                kind: ExprKind::Invoke { callable, args },
                ..
            }),
        ] = &body.function.body[..]
        else {
            panic!("expected the checked exec invocation");
        };
        let [source] = &args.values[..] else {
            panic!("expected one exec source operand");
        };
        let call = &args.site.python().unwrap().call;
        assert_eq!(body.fixed_sites[0], call.range);
        assert!(matches!(&callable.without_location().kind,
            ExprKind::Builtin(Builtin::RegisteredName(name), values) if name == "exec" && values.is_empty()));
        (call, source)
    }

    #[test]
    fn late_registered_entry_places_the_arithmetic_and_exec_frames() {
        let text = "def scale(x: int) -> int: return x + x\ndef startup(source: str) -> None: exec(source)\nprint(scale(3))\n";
        let mut graph = SourceGraph::single(text).unwrap();
        let (program, lowered) =
            caller_cache::observe_lowerings(|| crate::checked_lower(&mut graph).unwrap());
        assert!(matches!(program, crate::CheckedProgram::Registered(_)));
        assert_eq!(lowered.iter().filter(|name| *name == "startup").count(), 1);
        let functions = graph.registered_functions();
        assert_eq!(functions[&graph.entry].len(), 2);
        assert!(
            !functions[&graph.entry][&TextSize::default()]
                .fixed_sites
                .is_empty()
        );
        let start = TextSize::try_from(text.find("def startup").unwrap()).unwrap();
        let startup = &functions[&graph.entry][&start];
        let (call, source) = checked_exec_operand(startup);
        let ir::RegisteredCallTarget::FixedExec {
            module,
            operand: parameter,
            ..
        } = call.target
        else {
            panic!("expected the fixed checker to select exec for the parameter");
        };
        assert_eq!(module, graph.entry);
        assert_eq!(&text[parameter], "source");
        assert!(matches!(source.kind, ExprKind::Local(slot)
            if startup.local_names[&slot].name == "source"));
    }

    #[test]
    fn imported_exec_keeps_its_checked_operand_and_live_callees() {
        let dir = tempfile::tempdir().unwrap();
        let entry = dir.path().join("app.py");
        let producer = dir.path().join("producer.py");
        std::fs::write(
            &entry,
            "import bridge\nfrom producer import consume\nconsume('ok')\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("leaf.py"),
            "print('enter leaf')\nraise LookupError()\n",
        )
        .unwrap();
        let lower = |bridge: &str, source: &str| {
            std::fs::write(&producer, source).unwrap();
            std::fs::write(dir.path().join("bridge.py"), bridge).unwrap();
            let mut graph = SourceGraph::load(&entry, None, &[]).unwrap();
            let owner = graph.python_file("producer").unwrap();
            let program = crate::checked_lower(&mut graph).unwrap();
            let (mut functions, _) = graph.take_registered_functions();
            let body = functions
                .get_mut(&owner)
                .unwrap()
                .remove(&TextSize::default())
                .unwrap();
            let (compilation, lowered) = caller_cache::observe_lowerings(|| {
                crate::compile_file(&entry, None, &Default::default()).unwrap()
            });
            assert_eq!(
                lowered.iter().filter(|name| *name == "consume").count(),
                1,
                "{lowered:?}"
            );
            (owner, program, body, compilation)
        };
        let source = "def consume(value):\n    exec(input())\n";
        let (owner, program, body, _) = lower("import leaf\n", source);
        let crate::CheckedProgram::Registered(Some(reason)) = program else {
            panic!("expected the checked exec operation to select registered startup");
        };
        assert!(
            reason
                .to_string()
                .contains("producer.py:2:10: Imre: exec requires native Python object operations"),
            "{reason}"
        );
        let (call, inner) = checked_exec_operand(&body);
        assert_eq!(&source[call.range], "exec(input())");
        let ir::RegisteredCallTarget::FixedExec {
            module,
            operand,
            name,
        } = call.target
        else {
            panic!("expected the fixed checker to select exec");
        };
        assert_eq!(&source[name], "exec");
        assert_eq!(module, owner);
        assert_eq!(&source[operand], "input()");
        let binding = body.fixed_attribute(operand).unwrap();
        let (read, []) = binding.call_operands(operand, None, inner).unwrap() else {
            panic!("expected input without arguments");
        };
        assert!(matches!(binding, FixedAttribute::Call(_, call) if call.range == operand));
        assert!(matches!(&read.without_location().kind,
            ExprKind::Builtin(Builtin::RegisteredName(key), args) if key == "input" && args.is_empty()));

        let source = "def consume(value):\n    exec(\"print('producer')\")\n";
        let (_, _, body, compilation) = lower(
            "try:\n    import leaf\nexcept LookupError:\n    print('caught leaf')\n",
            source,
        );
        let (call, literal) = checked_exec_operand(&body);
        assert_eq!(&source[call.range], "exec(\"print('producer')\")");
        assert!(matches!(
            call.target,
            ir::RegisteredCallTarget::Callable { .. }
        ));
        assert!(matches!(&literal.kind, ExprKind::Str(text) if text == "print('producer')"));
        assert_eq!(
            compilation
                .llvm_ir
                .matches("call i64 @imre_dyn_exec_operand(")
                .count(),
            1
        );
    }

    #[test]
    fn placed_positional_invoke_uses_its_own_call_and_operands() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/callables/escaping_rebound_method.py");
        let mut graph = SourceGraph::load(&path, None, &[]).unwrap();
        crate::checked_lower(&mut graph).unwrap();
        let module = &graph.modules[graph.entry];
        let (_, mut initializers) = graph.take_registered_functions();
        let mut entry = initializers.remove(&graph.entry).unwrap();
        let [.., range, callee_range] = *entry.fixed_sites else {
            panic!("expected the invocation and its physical callee");
        };
        assert_eq!(
            (&module.source[range], &module.source[callee_range]),
            ("outer()()", "outer()")
        );
        let mut placed = None;
        ir::walk_nodes(&entry.function.body, &mut |node, _| {
            if let ir::Node::Expression(value) = node
                && matches!(value.kind, ExprKind::Invoke { .. })
            {
                assert!(placed.replace(value.clone()).is_none());
            }
        });
        let value = placed.unwrap();
        let attr = entry.fixed_attribute(range).unwrap();
        let (callee, []) = attr
            .call_operands(range, Some(callee_range), &value)
            .unwrap()
        else {
            panic!("the live bound method supplies its default");
        };
        assert!(matches!(callee.ty, Type::Callable(_)));
        assert!(matches!(callee.kind, ExprKind::Call(_, _)));
        let mut misplaced = value.clone();
        let ExprKind::Invoke { args, .. } = &mut misplaced.kind else {
            unreachable!()
        };
        args.order.push(0);
        assert!(
            attr.call_operands(range, Some(callee_range), &misplaced)
                .is_none()
        );
        crate::verify::registered(entry.region()).unwrap();

        let Some(FixedAttribute::Call(Some(active_callee), _)) =
            entry.fixed_attribute(callee_range)
        else {
            panic!("expected the written callee of outer()");
        };
        let active_callee = active_callee.clone();
        let rejects_invoke = |entry: &RegisteredFunction| {
            let error = crate::verify::registered(entry.region()).unwrap_err();
            assert!(
                error.contains("registered invocation needs a Python callable"),
                "{error}"
            );
        };
        let original = entry
            .fixed_attributes
            .as_mut()
            .unwrap()
            .remove(&range)
            .unwrap();
        rejects_invoke(&entry);
        let FixedAttribute::Call(None, owned) = &original else {
            panic!("expected outer()()");
        };
        for (written, end, replacement) in [
            (range, callee_range.end(), Some(active_callee)),
            (callee_range, callee_range.end(), None),
            (range, callee_range.start() + TextSize::of("outer"), None),
        ] {
            let mut call = owned.clone();
            let ir::RegisteredCallTarget::Callable { end: saved, .. } = &mut call.target else {
                panic!("expected a positional callable");
            };
            *saved = end;
            call.range = written;
            entry
                .fixed_attributes
                .as_mut()
                .unwrap()
                .insert(range, FixedAttribute::Call(replacement, call));
            rejects_invoke(&entry);
        }
        entry
            .fixed_attributes
            .as_mut()
            .unwrap()
            .insert(range, original);
        crate::verify::registered(entry.region()).unwrap();

        // The identical instruction in the unplaced prefix has no call descriptor.
        entry.function.body.insert(0, Statement::Expr(value));
        entry.placed_from += 1;
        rejects_invoke(&entry);
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
        let source = &graph.modules[graph.entry].source;
        let start = TextSize::try_from(source.find("def call(").unwrap()).unwrap();
        let functions = graph.registered_functions();
        let body = &functions[&graph.entry][&start];
        let sites: Vec<_> = body
            .fixed_sites
            .iter()
            .map(|range| &source[*range])
            .collect();
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
    fn file_and_pythonpath_recovery_check_a_later_procedure_in_its_selected_frame_once() {
        let text = concat!(
            "from pathlib import Path\n",
            "def first(directory: Path) -> Path:\n    return sorted(directory.glob('*.py'))[0]\n",
            "def inventory(directory: Path, expected: set[str]) -> None:\n",
            "    names = {row.name for row in directory.iterdir()}\n",
            "    if names != expected:\n        raise ValueError(names)\n",
            "inventory(Path('.'), set())\n",
        );
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app.py");
        std::fs::write(&path, text).unwrap();
        assert_one_inventory_body(SourceGraph::load(&path, None, &[]).unwrap(), text);

        let options = crate::CompileOptions {
            extension_paths: vec![dir.path().to_owned()],
            pythonpath: 0..1,
            ..Default::default()
        };
        let mut named = SourceGraph::module_entry(
            std::path::Path::new("-m"),
            "app",
            &options,
            Default::default(),
            &mut Default::default(),
        )
        .unwrap()
        .unwrap();
        assert!(named.modules[named.entry].is_program_source());
        assert!(!named.modules[named.entry].checks_source_contracts());
        named.registered_startup = true;
        assert_one_inventory_body(named, text);
    }

    fn assert_one_inventory_body(mut graph: SourceGraph, text: &str) {
        let (_, lowerings) =
            caller_cache::observe_lowerings(|| crate::checked_lower(&mut graph).unwrap());
        let start = TextSize::try_from(text.find("def inventory").unwrap()).unwrap();
        let frames = graph.registered_functions();
        let body = &frames[&graph.entry][&start].function.body;
        let mut reads = 0;
        ir::walk_expressions(body, &mut |value| {
            if let ExprKind::Builtin(Builtin::RegisteredAttribute { name, range, .. }, args) =
                &value.kind
                && name == "name"
                && &text[*range] == "row.name"
            {
                assert_eq!(args[0].ty, Type::PythonObject);
                reads += 1;
            }
        });
        assert_eq!(reads, 1);
        assert_eq!(
            lowerings.iter().filter(|name| *name == "inventory").count(),
            1,
            "{lowerings:?}"
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
    fn earlier_refusal_preserves_checked_siblings_at_their_original_slots() {
        let text = concat!(
            "def unused() -> int:\n    values: list[int] = [1]\n    values.append(True)\n    return 1\n",
            "def twice(n: int) -> int:\n    return n * 2\n",
            "def through(n: int) -> int:\n    return twice(n)\n",
            "print(through(21))\n",
        );
        let mut graph = SourceGraph::single(text).unwrap();
        graph.registered_startup = true;
        let report = crate::frontend::lower_with_caller_cache(&mut graph, None, None)
            .err()
            .unwrap();
        assert!(report.to_string().contains(":3:19:"), "{report}");
        let (functions, _) = graph.take_registered_functions();
        let frames = &functions[&graph.entry];
        let frame = |name| &frames[&TextSize::try_from(text.find(name).unwrap()).unwrap()];
        let twice = frame("def twice");
        assert_eq!(&text[twice.fixed_sites[0]], "return n * 2");
        let through = frame("def through");
        let [Statement::Return(value)] = through.function.body.as_slice() else {
            panic!("expected the original nonleaf return");
        };
        assert!(matches!(
            value.without_location().kind,
            ExprKind::Call(1, _)
        ));
        assert_eq!(&text[through.fixed_sites[1]], "twice(n)");
        assert!(matches!(
            through.fixed_attribute(through.fixed_sites[1]),
            Some(FixedAttribute::Call(Some(_), _))
        ));
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
