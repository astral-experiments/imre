use crate::dynamic::emit::{ModuleOrigin, Output, PreparedBody};
use std::path::Path;

#[test]
fn checked_module_places_class_generator_and_following_lambda() {
    use crate::dynamic::emit::SourcePositions;
    use crate::dynamic::emit::{RegisteredBody, SourceMap};
    use crate::frontend::RequiredSite;

    let text = concat!(
        "def read[T](value, offset=1):\n    return value + offset\n",
        "first = lambda: read(3)\n",
        "class C: pass\n",
        "stream = (x for x in ())\n",
        "last = lambda: 2\n",
    );
    let module = PreparedBody::module(
        "children",
        Path::new("children.py"),
        text,
        0,
        ModuleOrigin::Program(None),
    )
    .unwrap();
    assert!(module.initializer().fully_checked());
    let mut output = Output::default();
    let file = output.data(b"children.py");
    let location = SourceMap {
        info: &module.source,
        callable_code: Default::default(),
        exec_code: Default::default(),
        unit: 0,
        positions: &SourcePositions::new(&module.source.path, text, &file),
    };
    let mut function = location.function(
        &mut output,
        0,
        &module.source.scopes.module_globals,
        "",
        false,
    );
    let body = &module.initializer().checked.function.body;
    let mut required = Vec::new();
    RequiredSite::visit(body, Some(&module.source.exec_literals), &mut |site| {
        required.push(site)
    });
    let [
        RequiredSite::Definition(read),
        RequiredSite::Lambda(first),
        RequiredSite::Definition(class),
        RequiredSite::Generator(generator),
        RequiredSite::Lambda(last),
    ] = required.as_slice()
    else {
        panic!("expected the function, lambda, class, generator and final lambda");
    };
    RegisteredBody::Suite(module.initializer()).emit(&mut function);
    let sites = &function.sites;
    assert_eq!(sites.declarations.len(), 2);
    assert_eq!(location.callable_code.borrow().len(), 3);
    assert_eq!(sites.declaration(*class).definition.qualname, "C");
    assert_eq!(sites.declaration(*read).definition.name, "read");
    for site in [
        RequiredSite::Lambda(*first),
        RequiredSite::Generator(*generator),
        RequiredSite::Lambda(*last),
    ] {
        let symbols = location.callable_code.borrow();
        let entry = &symbols[&site];
        assert!(function.frame.body.code.contains(&format!("ptr {entry},")));
    }
    let count = function.frame.output.count;
    RegisteredBody::Suite(module.initializer()).emit(&mut function);
    assert_eq!(function.frame.output.count, count);
}

#[test]
fn checked_module_places_both_plain_and_generic_aliases_before_their_following_lambda() {
    use crate::dynamic::emit::SourcePositions;
    use crate::dynamic::emit::{RegisteredBody, SourceMap};
    use crate::frontend::RequiredSite;

    for alias in ["type Count = int", "type Count[T] = T"] {
        let text = format!("class Entry: pass\n{alias}\nlast = lambda: Entry\n");
        let module = PreparedBody::module(
            "decls",
            Path::new("decls.py"),
            &text,
            0,
            ModuleOrigin::Program(None),
        )
        .unwrap();
        assert!(module.initializer().fully_checked());
        let mut output = Output::default();
        let file = output.data(b"decls.py");
        let location = SourceMap {
            info: &module.source,
            callable_code: Default::default(),
            exec_code: Default::default(),
            unit: 0,
            positions: &SourcePositions::new(&module.source.path, &text, &file),
        };
        let mut function = location.function(
            &mut output,
            0,
            &module.source.scopes.module_globals,
            "",
            false,
        );
        let body = &module.initializer().checked.function.body;
        let mut required = Vec::new();
        RequiredSite::visit(body, Some(&module.source.exec_literals), &mut |site| {
            required.push(site)
        });
        let [
            RequiredSite::Definition(class),
            RequiredSite::Alias(alias),
            RequiredSite::Lambda(last),
        ] = required.as_slice()
        else {
            panic!("expected the class, alias and final lambda");
        };
        assert_eq!(alias.to_u32(), 18);
        let first = function.frame.output.count;
        let symbol = |index: u32| format!("@code_site{}", first + index);
        RegisteredBody::Suite(module.initializer()).emit(&mut function);
        let sites = &function.sites;
        assert_eq!(sites.declaration(*class).definition.code, symbol(0));
        assert_eq!(sites.declaration(*alias).definition.code, symbol(1));
        let last = RequiredSite::Lambda(*last);
        assert_eq!(location.callable_code.borrow()[&last], symbol(2));
        let count = function.frame.output.count;
        RegisteredBody::Suite(module.initializer()).emit(&mut function);
        assert_eq!(function.frame.output.count, count);
    }
}

#[test]
fn checked_class_site_uses_registered_metadata_after_module_erasure() {
    let mut module = PreparedBody::module_with_syntax(
        "children",
        Path::new("children.py"),
        "class C:\n    'class doc'\nlast = lambda: C\n",
        0,
        ModuleOrigin::Program(None),
    )
    .unwrap();
    assert!(module.initializer().fully_checked());
    let original = module.standalone().unwrap();
    assert!(module.syntax.take().is_some());
    assert_eq!(module.standalone().unwrap(), original);

    let class = module
        .source
        .class_body_mut(ruff_text_size::TextSize::new(0));
    assert_eq!(class.doc.take().as_deref(), Some("class doc"));
    assert_ne!(module.standalone().unwrap(), original);
}

#[test]
fn typing_initializer_binds_the_checked_reveal_type_definition() {
    use crate::ir::{Builtin, DefinitionSite, ExprKind};

    let source = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/stdlib/typing.py"));
    let start =
        ruff_text_size::TextSize::try_from(source.find("def reveal_type[T]").unwrap()).unwrap();
    PreparedBody::with_module(
        "typing",
        Path::new("<imre stdlib>/typing.py"),
        source,
        0,
        ModuleOrigin::Bundled,
        |module| {
            let initializer = module.initializer();
            assert!(initializer.fully_checked());
            let mut bindings = 0;
            crate::ir::walk_expressions(&initializer.checked.function.body, &mut |value| {
                if let ExprKind::Builtin(
                    Builtin::RegisteredDefinition(DefinitionSite::Function { start: site, .. }),
                    operands,
                ) = &value.kind
                    && *site == start
                {
                    assert!(operands.is_empty());
                    bindings += 1;
                }
            });
            assert_eq!(bindings, 1);
            assert!(module.source.definition_body(start).fully_checked());
            assert_eq!(
                module.source.scopes.functions[&start].type_params[0].binding,
                "T"
            );
            Ok(())
        },
    )
    .unwrap();
}

#[test]
fn suspended_alias_diagnostic_follows_the_emitted_binding() {
    for (suite, reachable) in [
        ("    raise Stop()\n    ALIAS", false),
        (
            "    try:\n        return\n        ALIAS\n    finally:\n        done()",
            false,
        ),
        (
            "    with manager:\n        raise Stop()\n        ALIAS",
            false,
        ),
        (
            "    try:\n        raise Stop()\n    except Stop:\n        ALIAS",
            true,
        ),
        (
            "    try:\n        return\n    finally:\n        ALIAS",
            true,
        ),
    ] {
        let source = format!("def stream():\n    yield 1\n{suite}\n")
            .replace("ALIAS", "type Dormant[T: int] = T");
        let mut module = PreparedBody::module(
            "stream",
            Path::new("stream.py"),
            &source,
            0,
            ModuleOrigin::Provider,
        )
        .unwrap();
        let alias = module.source.type_aliases.values_mut().next().unwrap();
        let parameter = &mut alias.parameters.as_mut().unwrap().declarations[0].fields[0].1;
        assert!(parameter.body.take().is_some());
        let range = parameter.range;
        let outcome = module.standalone();
        if reachable {
            let positions = crate::dynamic::emit::SourcePositions::new(
                &module.source.path,
                &source,
                "stream.py",
            );
            let (line, col) = positions.line_and_column(range.start());
            let (end_line, end_col) = positions.line_and_column(range.end());
            assert_eq!(
                outcome.unwrap_err(),
                format!(
                    "stream.py:{line}:{}: type parameter evaluator has no checked expression operand (through {end_line}:{})",
                    col + 1,
                    end_col + 1
                ),
                "{suite}"
            );
        } else {
            outcome.unwrap();
        }
    }
}

#[test]
fn forward_annotations_bind_prepared_children() {
    let source = r#"checked: 'select(Child)'
eager: '[x for x in Child]'
projected: "Child.partition(',')[1]"
width: "f'{Child!r:>{Width}}'"
closure: '(lambda: Child)()'
lazy: '(x for x in Child)'
wrapped: 'list(x for x in Child)'
nested: 'lambda: (x for x in Child)'
collected: 'lambda: [box.make(x).value for x in Child]'
bound: 'lambda: (v := box.make(9)).value'
default: 'lambda vals=[box.make(x).value for x in Child]: vals'
template: "t'{client.lookup(*args)}{[v for v in values]}'"
template_generator: "t'{client.lookup(*args)}{(v for v in values)}'"
argument: 'resolve([Child for _ in (0,)])'
invalid: 'Child['
"#;
    let body = PreparedBody::module(
        "example",
        Path::new("example.py"),
        source,
        0,
        ModuleOrigin::Provider,
    )
    .unwrap();
    let child = |spelling: &str| &body.source.annotation_children[spelling];
    assert!(
        !child("[x for x in Child]")
            .code
            .operand
            .region()
            .private_slots()
            .is_empty()
    );
    assert!(
        child("(lambda: Child)()").scope.lambdas[&1.into()]
            .body
            .is_some()
    );
    for quoted in [
        "(x for x in Child)",
        "list(x for x in Child)",
        "lambda: (x for x in Child)",
    ] {
        assert_eq!(child(quoted).scope.generators.len(), 1, "{quoted}");
    }
    assert!(
        child("lambda: (x for x in Child)").scope.lambdas[&0.into()]
            .body
            .is_some()
    );
    for quoted in [
        "lambda: [box.make(x).value for x in Child]",
        "lambda: (v := box.make(9)).value",
        "lambda vals=[box.make(x).value for x in Child]: vals",
    ] {
        assert!(
            child(quoted).scope.lambdas[&0.into()].body.is_some(),
            "{quoted}"
        );
    }
    assert!(
        !child("t'{client.lookup(*args)}{[v for v in values]}'")
            .code
            .operand
            .region()
            .private_slots()
            .is_empty()
    );
    assert!(
        !child("resolve([Child for _ in (0,)])")
            .code
            .operand
            .region()
            .private_slots()
            .is_empty()
    );
    assert_eq!(
        child("t'{client.lookup(*args)}{(v for v in values)}'")
            .scope
            .generators
            .len(),
        1
    );
    let mut output = Output::default();
    let filename = output.data(b"example.py");
    body.emit(&mut output, 0, &filename).unwrap();
    let children: Vec<_> = output
        .functions
        .lines()
        .filter(|line| line.contains("call void @imre_dyn_bind_expression("))
        .map(|line| line.rsplit_once("ptr ").unwrap().1.trim_end_matches(')'))
        .collect();
    assert_eq!(children.len(), 14);
    assert_ne!(children[0], children[1]);
    for child in children {
        assert_eq!(
            output
                .functions
                .matches(&format!("define internal i64 {child}("))
                .count(),
            1
        );
    }
}
