use super::CheckedSites;
use super::sites::{CallableSite, CompiledDefinition};
use super::{Output, PreparedSuiteEmitter, RegisteredBody, RegisteredFrame, SourcePositions};
use crate::modules::SourceGraph;
use ruff_text_size::{TextRange, TextSize};
use std::path::Path;

/// Isolated writer tests supply their completed sites without compiling a source module.
pub(crate) struct TestFrame<'f, 'a, 'p>(
    pub(crate) &'f mut RegisteredFrame<'a, 'p>,
    pub(crate) CheckedSites<'p>,
);

impl<'a, 'p> std::ops::Deref for TestFrame<'_, 'a, 'p> {
    type Target = RegisteredFrame<'a, 'p>;
    fn deref(&self) -> &Self::Target {
        self.0
    }
}

impl std::ops::DerefMut for TestFrame<'_, '_, '_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.0
    }
}

impl<'a, 'p> PreparedSuiteEmitter<'a, 'p> for TestFrame<'_, 'a, 'p> {
    fn frame_and_sites(&mut self) -> (&mut RegisteredFrame<'a, 'p>, &CheckedSites<'p>) {
        (self.0, &self.1)
    }
    fn place_definition(&mut self, site: TextSize) -> usize {
        self.1.definitions.get_index_of(&site).unwrap()
    }
    fn place_class_namespace(&mut self, site: TextSize) -> usize {
        self.1.class_namespaces.get_index_of(&site).unwrap()
    }
    fn place_type_alias(&mut self, site: TextSize) -> usize {
        self.1.aliases.get_index_of(&site).unwrap()
    }
    fn callable_at(&mut self, _: CallableSite, _: bool) -> CompiledDefinition {
        unreachable!("isolated writer tests have no lexical callable definitions")
    }
    fn annotation_string(&mut self, _: TextSize, _: &str) -> String {
        unreachable!("isolated writer tests have no decoded annotation")
    }
    fn exec_literal_at(&mut self, _: TextRange) -> Option<super::sites::CompiledExec> {
        None
    }
}

#[test]
fn frame_diagnostics_use_the_original_path_and_byte_columns() {
    let mut output = Output::default();
    let file = output.data(b"<frozen sample>");
    let positions = SourcePositions::new(Path::new("sample.py"), "α = value\né + operand\n", &file);
    let mut frame = RegisteredFrame::new(&mut output, &positions, false, 0, 0.into());
    frame.refuse_at(TextRange::new(5.into(), 23.into()), "first error");
    frame.refuse_at(TextRange::default(), "later error");
    assert_eq!(
        frame.output.diagnostic.as_deref(),
        Some("sample.py:1:6: first error (through 2:13)")
    );
}

fn emit_captured_body(source: &str) -> Output {
    let graph = SourceGraph::single(source).unwrap();
    let _ = crate::frontend::lower(&graph);
    let registered = graph.registered_functions();
    let function = &registered[&graph.entry][&TextSize::default()];
    emit_registered_body(source, function)
}

fn emit_registered_body(source: &str, function: &crate::frontend::RegisteredFunction) -> Output {
    let mut output = Output::default();
    let file = output.data(b"app.py");
    let positions = SourcePositions::new(Path::new("app.py"), source, &file);
    let mut frame = RegisteredFrame::new(&mut output, &positions, false, 0, 0.into());
    RegisteredBody::Function(function).emit(&mut TestFrame(&mut frame, CheckedSites::default()));
    frame.finish_callable(None);
    output
}

#[test]
fn placed_field_uses_the_ordinary_descriptor_and_attribute_line() {
    use crate::frontend::{RegisteredFunction, RegisteredScope, RegisteredSource};
    use crate::ir::{ExprKind, Statement};
    use ruff_text_size::Ranged;

    let source = concat!(
        "from functools import update_wrapper\n",
        "from typing import Callable\n",
        "class Item:\n",
        "    text: str\n",
        "    def __init__(self, text: str) -> None: self.text = text\n",
        "    def read(self, suffix: str) -> str:\n",
        "        return (self\n",
        "                .text)\n",
        "def repeat(value: str) -> str: return value + value\n",
        "def main() -> None:\n",
        "    method: Callable[[str], str] = Item('hello').read\n",
        "    wrapper = update_wrapper(repeat, method, (), ())\n",
        "    print(wrapper.__wrapped__('!'))\n",
        "main()\n",
    );
    let mut graph = SourceGraph::single(source).unwrap();
    graph.registered_startup = true;
    assert!(matches!(
        crate::checked_lower(&mut graph).unwrap(),
        crate::CheckedProgram::Registered(_)
    ));
    let statements = &graph.modules[graph.entry].parsed.syntax().body;
    let class = statements
        .iter()
        .find_map(|stmt| stmt.as_class_def_stmt())
        .unwrap();
    let definition = class
        .body
        .iter()
        .filter_map(|stmt| stmt.as_function_def_stmt())
        .find(|function| function.name.as_str() == "read")
        .unwrap();
    let functions = graph.registered_functions();
    let placed = &functions[&graph.entry][&definition.start()];
    assert!(
        matches!(placed.function.body.as_slice(), [Statement::Return(value)] if matches!(value.kind, ExprKind::Field(_, 0)))
    );
    assert!(
        matches!(&placed.inputs, crate::frontend::FrameInputs::Protocol { sites }
        if matches!(sites.as_ref(), [crate::frontend::FrameSite::Method(method)]
            if method.range() == definition.range()))
    );

    let values = RegisteredSource {
        python_optimization: 0,
        operand_types: None,
        text: source,
        replacements: &Default::default(),
        template_fields: None,
        generators: None,
        lambdas: None,
        method_imports: &Default::default(),
        private_names: None,
    };
    let operations = Default::default();
    let scope = RegisteredScope::Function { is_async: false };
    let ordinary =
        RegisteredFunction::from_suite(&definition.body, scope, values, &operations, true).unwrap();
    let expected = emit_registered_body(source, &ordinary);
    let actual = emit_registered_body(source, placed);
    assert_eq!(actual.functions, expected.functions);
    assert_eq!(actual.constants, expected.constants);
    assert!(actual.interned_identifiers.contains("text"));
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
    assert_eq!(body.matches("call i64 @imre_dyn_take_int(").count(), 1);
    assert!(!body.contains("%int_owner"));
    let (_, failure) = body.split_once("\nfailed:\n").unwrap();
    assert!(failure.contains("ret i64 0"));
}

#[test]
fn a_checked_local_uses_python_membership_alongside_native_multiplication() {
    let source = concat!(
        "def probe(callback, left: int, right: int):\n",
        "    module = callback.__module__\n",
        "    if module not in ('__main__', 'builtins'):\n",
        "        callback(left * right)\n",
    );
    let body = emit_captured_body(source).functions;
    let flag = body
        .lines()
        .find(|line| line.contains("call i64 @imre_dyn_contains_literal_tuple("))
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap();
    assert!(body.contains(&format!("icmp eq i64 {flag}, {}", super::permanent::TRUE)));
    assert!(body.contains("call i64 @int_mul("));
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
fn nested_additions_keep_their_own_protocol_positions() {
    let source = "def publish(n: int, send):\n    send((n + 2) + 3)\n";
    let mut output = emit_captured_body(source);
    let plus = output.data(b"+");
    let fallback = format!("call i64 @imre_dyn_op_at(ptr %ctx, ptr {plus},");
    for position in [[2, 10, 12, 15], [2, 9, 17, 20]] {
        let pointer = output.position(position);
        assert!(
            output.functions.lines().any(|line| {
                line.contains(&fallback) && line.ends_with(&format!("ptr {pointer})"))
            }),
            "missing arithmetic site {position:?}"
        );
    }
}

#[test]
fn integer_literal_handle_is_only_loaded_on_the_protocol_edge() {
    let mut output = emit_captured_body("def scale(n: int, send):\n    send(n * 31415927 + 17)\n");
    let literal = output.data(b"31415927");
    let body = &output.functions;
    let (_, after_product) = body.split_once("call i64 @int_mul(").unwrap();
    let load = format!("ptr {literal}, i64 8)");
    assert_eq!(body.matches(&load).count(), 1);
    let (_, protocol) = after_product.split_once(&load).unwrap();
    assert!(protocol.contains("call i64 @imre_dyn_op_at("));
}

#[test]
fn checked_integer_results_use_the_permanent_cache_before_transferring_ownership() {
    let output = emit_captured_body("def adjust(value: int, receive):\n    receive(value - 1)\n");
    let (_, result) = output.functions.split_once("call i64 @int_sub(").unwrap();
    let (cache, transfer) = result.split_once("call i64 @imre_dyn_take_int(").unwrap();
    let width = super::permanent::SMALL_INT_MAX - super::permanent::SMALL_INT_MIN;
    assert!(
        cache
            .lines()
            .any(|line| line.contains("icmp ule i64") && line.ends_with(&format!(", {width}")))
    );
    assert!(cache.contains("trunc i64"));
    assert!(cache.contains("br i1 "));
    assert!(transfer.contains("phi i64"));
    assert!(transfer.contains("call i64 @imre_dyn_positional_call_at("));
}

#[test]
fn checked_integer_remainder_keeps_its_operation_with_a_registered_sibling() {
    let mut output = emit_captured_body(concat!(
        "def remainder(a: int, b: int) -> int:\n    return a % b\n",
        "def publish(send):\n    send(1)\n",
    ));
    let modulo = output.data(b"%");
    let body = &output.functions;
    assert_eq!(body.matches("call i64 @int_mod(").count(), 1);
    assert_eq!(
        body.matches("call i64 @imre_dyn_exact_int_word(").count(),
        2
    );
    assert!(body.contains("icmp ugt i64"));
    assert_eq!(
        body.matches(&format!("call i64 @imre_dyn_op_at(ptr %ctx, ptr {modulo},"))
            .count(),
        1
    );
    output.integer_prelude();
    assert!(output.functions.contains("define internal i64 @int_mod("));
    assert!(
        !output
            .functions
            .contains("define internal i64 @int_floordiv(")
    );
}

#[test]
fn registered_subscription_consumes_its_checked_integer_key() {
    let output = emit_captured_body(
        "def publish(values: list[int], i: int, n: int, send):\n    send(values[i % n])\n",
    );
    assert!(output.functions.contains("call i64 @int_mod("));
    assert!(
        output
            .functions
            .contains("call i64 @imre_dyn_get_int_item(")
    );
}

#[test]
fn checked_floor_division_can_call_a_python_override() {
    let mut output = emit_captured_body("def publish(a: int, b: int, send):\n    send(a // b)\n");
    let floor = output.data(b"//");
    let body = &output.functions;
    assert_eq!(
        body.matches("call i64 @imre_dyn_exact_int_word(").count(),
        2
    );
    assert!(body.contains("icmp ugt i64"));
    assert!(body.contains("call i64 @int_floordiv("));
    let fallback = body
        .lines()
        .find(|line| line.contains(&format!("call i64 @imre_dyn_op_at(ptr %ctx, ptr {floor},")))
        .unwrap();
    let position = output.positions[&[2, 9, 11, 15]];
    assert!(fallback.ends_with(&format!("ptr @pos{position})")));
}

#[test]
fn checked_right_shift_guards_the_count_before_using_the_tagged_helper() {
    let mut output = emit_captured_body("def shift(a: int, b: int, send):\n    send(a >> b)\n");
    let operator = output.data(b">>");
    let body = &output.functions;
    assert_eq!(
        body.matches("call i64 @imre_dyn_exact_int_word(").count(),
        2
    );
    assert!(body.contains("icmp sge i64"));
    assert!(body.contains("call i64 @int_rshift("));
    assert!(body.contains(&format!(
        "call i64 @imre_dyn_op_at(ptr %ctx, ptr {operator},"
    )));
    output.integer_prelude();
    assert_eq!(
        output
            .functions
            .matches("define internal i64 @int_rshift(")
            .count(),
        1
    );
    assert!(!output.functions.contains("@int_lshift("));
}

#[test]
fn cached_power_of_two_remainder_needs_no_integer_owner() {
    let mut output = emit_captured_body(concat!(
        "def modulo(a: int) -> int:\n    return a % 8\n",
        "def publish(send):\n    send(1)\n",
    ));
    let eight = super::abi::permanent::small_integer_id(8).unwrap();
    let body = &output.functions;
    let call = body
        .lines()
        .find(|line| line.contains("call i64 @imre_dyn_masked_remainder("))
        .unwrap();
    assert!(call.ends_with(&format!("i64 {eight})")));
    assert!(!body.contains("int_owner"));
    assert!(!body.contains("@int_mod("));
    assert!(!body.contains("@imre_dyn_exact_int_word("));
    output.integer_prelude();
    assert!(
        !output
            .functions
            .contains("define internal void @release_int")
    );
}

#[test]
fn integer_product_consumes_its_operands_without_reprojecting_the_sum() {
    let mut output =
        emit_captured_body("def publish(scale, value, send):\n    send(scale * (value + 1))\n");
    let plus = output.data(b"+");
    let product = output.data(b"*");
    let body = &output.functions;
    assert_eq!(
        body.matches("call i64 @imre_dyn_exact_int_word(").count(),
        2
    );
    assert_eq!(body.matches("call i64 @int_add(").count(), 1);
    assert_eq!(body.matches("call i64 @int_mul(").count(), 1);
    let (_, failure) = body.split_once("\nfailed:\n").unwrap();
    assert!(failure.contains("call void @release_int_nonzero("));
    for op in [plus, product] {
        assert_eq!(
            body.matches(&format!("call i64 @imre_dyn_op_at(ptr %ctx, ptr {op},"))
                .count(),
            1
        );
    }
    output.integer_prelude();
    assert!(
        output
            .functions
            .contains("define internal void @release_int_nonzero(")
    );
    assert!(
        !output
            .functions
            .contains("define internal void @release_int(")
    );
}

#[test]
fn captured_float_product_uses_the_registered_multiply_protocol() {
    let mut output =
        emit_captured_body("def deliver(a: float, b: float, send):\n    send(a * b)\n");
    let star = output.data(b"*");
    let body = &output.functions;
    assert_eq!(
        body.matches(&format!("call i64 @imre_dyn_op_at(ptr %ctx, ptr {star},"))
            .count(),
        1
    );
    assert!(!body.contains("call i64 @imre_dyn_exact_int_word("));
    assert!(!body.contains("call i64 @int_mul("));
}

#[test]
fn placed_arithmetic_keeps_the_python_operator_and_integer_guards() {
    use crate::ir::{Binary, ExprKind, Statement};

    for (op, signature, result, call) in [
        (Binary::Multiply, "a: float, b: float", "float", "2.5, 3.0"),
        (Binary::Multiply, "a: str, b: int", "str", "'a', 3"),
        (Binary::Add, "a: str, b: str", "str", "'a', 'b'"),
        (
            Binary::Add,
            "a: list[int], b: list[int]",
            "list[int]",
            "[1], [2]",
        ),
        (Binary::Add, "a: str | None, b: str", "str", "'a', 'b'"),
    ] {
        let operator = op.protocol();
        let source = format!(
            "class Receiver:\n    def read(self, value: int) -> int:\n        return value\n\
             def combine({signature}) -> {result}:\n    return a {operator} b\nprint(combine({call}))\n"
        );
        let mut graph = SourceGraph::single(&source).unwrap();
        let crate::CheckedProgram::Native(mut fixed) = crate::checked_lower(&mut graph).unwrap()
        else {
            panic!("expected a fixed function");
        };
        let (_, _, frame) = fixed
            .frame_sources
            .into_iter()
            .find(|(id, ..)| *id == 0)
            .unwrap();
        let function = fixed.ir.0.functions.remove(0);
        let [Statement::Return(value)] = function.body.as_slice() else {
            panic!()
        };
        assert!(matches!(
            &value.kind,
            ExprKind::Binary(actual, ..) if *actual == op
        ));
        let placed = frame.place(function).ok().unwrap();
        let mut output = Output::default();
        let file = output.data(b"combine.py");
        let positions = SourcePositions::new(Path::new("combine.py"), &source, &file);
        let mut frame = RegisteredFrame::new(&mut output, &positions, false, 0, 0.into());
        RegisteredBody::Function(&placed).emit(&mut TestFrame(&mut frame, CheckedSites::default()));
        frame.finish_callable(None);
        let operator = output.data(operator.as_bytes());
        let body = &output.functions;
        assert_eq!(
            body.matches("call i64 @imre_dyn_exact_int_word(").count(),
            if op == Binary::Multiply { 2 } else { 0 }
        );
        assert_eq!(
            body.matches("call i64 @int_mul(").count(),
            usize::from(op == Binary::Multiply)
        );
        assert_eq!(
            body.matches(&format!(
                "call i64 @imre_dyn_op_at(ptr %ctx, ptr {operator},"
            ))
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
        .split_once("call void @imre_dyn_statement_with_exception(ptr %ctx, ptr %ir_slot0,")
        .unwrap();
    assert_eq!(body.matches("%ir_slot0 = alloca [").count(), 1);
    assert!(before_checkpoint.contains("zeroinitializer, ptr %ir_slot0"));
    assert!(!before_checkpoint.contains("load i64, ptr %ir_slot0"));
    assert_eq!(
        body.matches("call i64 @imre_dyn_positional_call_at(")
            .count(),
        1
    );
}

#[test]
fn checkpoints_preserve_the_selected_roots_of_nested_regions() {
    let mut output = Output::default();
    let file = output.data(b"nested.py");
    let positions = SourcePositions::new(Path::new("nested.py"), "", &file);
    let mut frame = RegisteredFrame::new(&mut output, &positions, false, 0, 0.into());
    let graph =
        SourceGraph::single("def post(row, book, read):\n    book[0], book[read()] = row\n")
            .unwrap();
    let _ = crate::frontend::lower(&graph);
    let functions = graph.registered_functions();
    let region = functions[&graph.entry][&TextSize::default()].region();
    let count = region.frame_slots.slots().len();
    assert!(count > 0);
    let outer = frame.enter_checked_slots(region).unwrap();
    let first = frame.enter_checked_slots(region).unwrap();
    frame.checkpoint(Default::default());
    let both = 2 * count;
    assert!(
        frame
            .body
            .code
            .contains(&format!("ptr %ir_slot0, i64 {both},"))
    );

    frame.restore_checked_slots(first);
    let sibling = frame.enter_checked_slots(region).unwrap();
    frame.body.code.clear();
    frame.checkpoint(Default::default());
    assert!(frame.body.code.contains(&format!("ptr %live, i64 {both},")));
    assert!(frame.body.code.contains("ptr %ir_slot0,"));
    assert!(frame.body.code.contains(&format!("ptr %ir_slot{both},")));
    assert!(!frame.body.code.contains(&format!("ptr %ir_slot{count},")));

    frame.restore_checked_slots(sibling);
    frame.body.code.clear();
    frame.checkpoint_with_roots(Default::default(), &["%exception"]);
    assert!(frame.body.code.contains("store i64 %exception,"));
    assert!(
        frame
            .body
            .code
            .contains(&format!("ptr %live, i64 {},", count + 1))
    );
    frame.restore_checked_slots(outer);
}

#[test]
fn checkpoint_batches_active_slots_after_the_register_prefix() {
    let mut output = Output::default();
    let file = output.data(b"nested.py");
    let positions = SourcePositions::new(Path::new("nested.py"), "", &file);
    let mut frame = RegisteredFrame::new(&mut output, &positions, false, 0, 0.into());
    frame.live.push("%exception".into());
    frame.live_slots = vec![0, 1, 4, 5, 6];
    frame.checkpoint(Default::default());

    let body = &frame.body.code;
    let (_, suffix) = body.split_once("store i64 %exception,").unwrap();
    let (_, suffix) = suffix
        .split_once("getelementptr i64, ptr %live, i64 1")
        .unwrap();
    let (_, suffix) = suffix
        .split_once("ptr %ir_slot0, i64 16, i1 false)")
        .unwrap();
    let (_, suffix) = suffix
        .split_once("getelementptr i64, ptr %live, i64 3")
        .unwrap();
    let (_, suffix) = suffix
        .split_once("ptr %ir_slot4, i64 24, i1 false)")
        .unwrap();
    assert!(suffix.contains("ptr %live, i64 6,"));
    assert_eq!(body.matches("call void @llvm.memcpy").count(), 2);
}

#[test]
fn checked_assertion_message_and_raise_use_the_failure_edge() {
    use crate::ir::{Builtin, Expr, ExprKind, Statement, Type};

    let mut output = Output::default();
    let file = output.data(b"app.py");
    let positions = SourcePositions::new(Path::new("app.py"), "assert (\nFalse), message", &file);
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
                test: TextRange::new(9.into(), 14.into()),
            },
            vec![message],
        ),
    });
    {
        let mut owner = TestFrame(&mut frame, CheckedSites::default());
        let frame = super::Frame::for_expression(&mut owner);
        super::CheckedBody::registered(frame).emit_statement(&Statement::If(
            Expr::boolean(false),
            vec![],
            vec![failure],
        ));
    }

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
    let test_position = frame.output.positions[&[2, 2, 0, 5]];
    let (_, raise) = after_message
        .split_once(&format!(
            "call void @imre_dyn_definition_position(ptr %ctx, ptr @pos{test_position})"
        ))
        .unwrap();
    assert!(raise.contains("call i64 @imre_dyn_op_at("));
    assert!(raise.contains("br label %caught"));
}

#[test]
fn checked_cursor_pull_keeps_one_projection_for_both_representations() {
    let mut output = emit_captured_body(
        "def collect(source, project):\n    return [project(item) for item in source]\n",
    );
    let more = output.data(b"more");
    let next = output.data(b"next");
    let body = &output.functions;
    for call in [
        "call i32 @imre_dyn_list_cursor_pull(",
        &format!("call i64 @imre_dyn_op_at(ptr %ctx, ptr {more},"),
        &format!("call i64 @imre_dyn_op_at(ptr %ctx, ptr {next},"),
        "call i64 @imre_dyn_positional_call_at(",
    ] {
        assert_eq!(body.matches(call).count(), 1, "{call}");
    }
}

#[test]
fn async_pull_parks_the_iterator_returned_by_the_protocol_once() {
    use crate::ir::AsyncPull;
    use ruff_text_size::TextRange;

    let mut output = Output::default();
    let file = output.data(b"stream.py");
    let source = SourcePositions::new(
        Path::new("stream.py"),
        "async for item in stream:\n    pass",
        &file,
    );
    let mut frame = RegisteredFrame::new(&mut output, &source, true, 0, 0.into());
    frame.body.error_target = "outer".into();
    let pull = AsyncPull {
        range: TextRange::new(18.into(), 24.into()),
        header: Some(0.into()),
    };
    frame.async_pull("%stream", pull, "item", "exhausted");

    assert_eq!(frame.body.error_target, "outer");
    let body = &frame.body.code;
    let next = frame
        .output
        .strings
        .get_index_of(b"async_next_iter".as_slice())
        .unwrap();
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
    assert!(!frame.output.strings.contains(b"await_iter".as_slice()));
    assert!(body.contains("label %exhausted,"));
    assert!(body.contains("br label %outer"));
}

#[test]
fn inner_handler_preserves_the_enclosing_cursor_slot() {
    let mut output = Output::default();
    let file = output.data(b"stream.py");
    let positions = SourcePositions::new(Path::new("stream.py"), "", &file);
    let mut code = RegisteredFrame::new(&mut output, &positions, true, 0, 0.into());
    let mut owner = TestFrame(&mut code, CheckedSites::default());
    let mut frame = super::Frame::for_expression(&mut owner);
    frame.iteration.storage = crate::frontend::FrameSlots::new([2, 8].into(), Default::default());
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
