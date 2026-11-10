use super::CheckedSites;
use super::{Output, RegisteredFrame, SourcePositions};
use crate::modules::SourceGraph;
use ruff_text_size::TextSize;

fn emit_captured_body(source: &str) -> Output {
    let graph = SourceGraph::single(source).unwrap();
    let _ = crate::frontend::lower(&graph);
    let registered = graph.registered_functions();
    let function = &registered[&graph.entry][&TextSize::default()];

    let mut output = Output::default();
    let file = output.data(b"app.py");
    let positions = SourcePositions::new(source, &file);
    let mut frame = RegisteredFrame::new(&mut output, &positions, false, 0, 0.into());
    RegisteredFrame::checked_body(&mut frame, function, &CheckedSites::default());
    frame.finish(false);
    output
}

#[test]
fn captured_publish_emits_its_registered_callback_and_error_return() {
    let output = emit_captured_body(
        "def publish(a: int, b: int, send):\n    total = a + b\n    send(total)\n",
    );
    let body = &output.functions;
    assert_eq!(
        body.matches("call i64 @imre_dyn_positional_call_at(")
            .count(),
        1
    );
    let checkpoints: Vec<_> = body
        .lines()
        .filter(|line| line.contains("call void @imre_dyn_statement_with_exception("))
        .collect();
    assert_eq!(checkpoints.len(), 2);
    assert!(checkpoints[0].contains("i32 2, i64 0)"));
    assert!(checkpoints[1].contains("i32 3, i64 0)"));
    let pos = output.positions[&[3, 3, 4, 15]];
    assert!(body.contains(&format!(
        "call void @imre_dyn_call_position(ptr %ctx, ptr @pos{pos})"
    )));
    assert!(body.contains("store i64 1, ptr %int_owner0"));
    let (_, failure) = body.split_once("\nfailed:\n").unwrap();
    assert!(failure.contains("call void @imre_int_release("));
    assert!(failure.contains("ret i64 0"));
}

#[test]
fn checked_subtraction_selects_integer_arithmetic_in_both_calling_conventions() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("difference.py");
    std::fs::write(
        &path,
        "def difference(a: int, b: int) -> int:\n    return a - b\nprint(difference(int(input()), int(input())))\n",
    )
    .unwrap();
    let fixed = crate::compile_file(&path, None, &crate::CompileOptions::default()).unwrap();
    let (_, function) = fixed
        .llvm_ir
        .split_once("define internal i64 @fn_0(")
        .unwrap();
    let (body, _) = function.split_once("\n}").unwrap();
    let subtract = "call i64 @int_sub(";
    assert_eq!(body.matches(subtract).count(), 1);

    for (signature, checked_integers) in [("a: int, b: int", true), ("a, b", false)] {
        let source =
            format!("def publish({signature}, send):\n    total = a - b\n    send(total)\n");
        let mut output = emit_captured_body(&source);
        let minus = output.data(b"-");
        let protocol = format!("call i64 @imre_dyn_op_at(ptr %ctx, ptr {minus},");
        let body = &output.functions;
        assert_eq!(
            body.matches(subtract).count(),
            usize::from(checked_integers)
        );
        assert_eq!(
            body.contains("call i64 @imre_dyn_exact_int_word("),
            checked_integers
        );
        assert_eq!(body.matches(&protocol).count(), 1);
        assert_eq!(
            body.matches("call i64 @imre_dyn_positional_call_at(")
                .count(),
            1
        );
    }
}

#[test]
fn captured_unpack_roots_its_private_slot_in_the_function_frame() {
    let output =
        emit_captured_body("def post(row, book, read):\n    book[0], book[read()] = row\n");
    let body = &output.functions;
    let (before_checkpoint, _) = body
        .split_once("call void @imre_dyn_statement_with_exception(ptr %ctx, ptr %live,")
        .unwrap();
    assert_eq!(body.matches("%ir_slot0 = alloca i64").count(), 1);
    assert!(before_checkpoint.contains("store i64 0, ptr %ir_slot0"));
    assert!(before_checkpoint.contains("load i64, ptr %ir_slot0"));
    assert_eq!(
        body.matches("call i64 @imre_dyn_positional_call_at(")
            .count(),
        1
    );
}

#[test]
fn checked_assertion_message_and_raise_use_the_failure_edge() {
    use crate::ir::{Builtin, Expr, ExprKind, Statement, Type};

    let mut output = Output::default();
    let file = output.data(b"app.py");
    let positions = SourcePositions::new("assert (\nFalse), message", &file);
    let mut frame = RegisteredFrame::new(&mut output, &positions, false, 0, 0.into());
    frame.body.error_target = "caught".into();

    let message = Expr {
        ty: Type::PythonObject,
        kind: ExprKind::Builtin(Builtin::RegisteredName("message".into()), vec![]),
    };
    let failure = Statement::Expr(Expr {
        ty: Type::None,
        kind: ExprKind::Builtin(
            Builtin::RegisteredAssertionFailure {
                statement: 0.into(),
                test: 9.into(),
            },
            vec![message],
        ),
    });
    frame.checked_statement(
        &Statement::If(Expr::boolean(false), vec![], vec![failure]),
        &CheckedSites::default(),
    );

    let code = &frame.body.code;
    let branch = code
        .lines()
        .find(|line| line.contains("br i1 false,"))
        .unwrap();
    let (_, failure_label) = branch.rsplit_once("label %").unwrap();
    let (success, failure) = code.split_once(&format!("\n{failure_label}:\n")).unwrap();
    assert!(!success.contains("@imre_dyn_lookup("));
    let (message, after_message) = failure
        .split_once("call void @imre_dyn_set_statement_line(ptr %ctx, i32 1)")
        .unwrap();
    assert!(message.contains("@imre_dyn_lookup("));
    assert!(message.contains("label %caught,"));
    let (_, raise) = after_message
        .split_once("call void @imre_dyn_set_statement_line(ptr %ctx, i32 2)")
        .unwrap();
    assert!(raise.contains("call i64 @imre_dyn_op_at("));
    assert!(raise.contains("br label %caught"));
}

#[test]
fn async_pull_parks_the_iterator_returned_by_the_protocol_once() {
    use crate::ir::AsyncPull;
    use ruff_text_size::TextRange;

    let mut output = Output::default();
    let file = output.data(b"stream.py");
    let source = SourcePositions::new("async for item in stream:\n    pass", &file);
    let mut frame = RegisteredFrame::new(&mut output, &source, true, 0, 0.into());
    frame.body.error_target = "outer".into();
    let pull = AsyncPull {
        range: TextRange::new(18.into(), 24.into()),
        header: Some(0.into()),
    };
    frame.async_pull("%stream", pull, "item", "exhausted");

    assert_eq!(frame.body.error_target, "outer");
    let body = &frame.body.code;
    let next = frame.output.strings[b"async_next_iter".as_slice()];
    assert_eq!(
        body.matches(&format!("ptr @str{next}, i64 %stream"))
            .count(),
        1
    );
    assert_eq!(
        body.matches("call void @imre_dyn_suspend_operands(")
            .count(),
        1
    );
    assert!(!frame.output.strings.contains_key(b"await_iter".as_slice()));
    assert!(body.contains("label %exhausted,"));
    assert!(body.contains("br label %outer"));
}

#[test]
fn inner_handler_preserves_the_enclosing_cursor_slot() {
    let mut output = Output::default();
    let file = output.data(b"stream.py");
    let positions = SourcePositions::new("", &file);
    let mut code = RegisteredFrame::new(&mut output, &positions, true, 0, 0.into());
    let sites = CheckedSites::default();
    let mut frame = super::Frame {
        code: &mut code,
        names: None,
        sites: &sites,
    };
    frame.iteration.private_slots = vec![2, 8];
    frame.iteration.first_slot = 3;
    frame.body.error_target = "outer".into();
    frame.enter_scope(&[2]);
    frame.body.error_target = "inner".into();
    frame.enter_scope(&[8]);
    frame.leave_scope();
    frame.jump("inner");
    frame.label("inner");
    frame.leave_scope();
    frame.jump("outer");
    frame.label("outer");

    let (_, handlers) = frame.body.code.split_once("\ninner:\n").unwrap();
    let (inner, outer) = handlers.split_once("\nouter:\n").unwrap();
    assert!(inner.contains("store i64 0, ptr %ir_slot4"));
    assert!(!inner.contains("%ir_slot3"));
    assert!(outer.contains("store i64 0, ptr %ir_slot3"));
    assert!(!outer.contains("%ir_slot4"));
    assert!(!handlers.contains("call "));
}
