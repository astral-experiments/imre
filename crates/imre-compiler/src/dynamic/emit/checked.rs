//! Checked operations inside a Python function with a registered frame and object arguments.

use super::{CallArguments, Function, RegisteredFrame, permanent};
use crate::frontend::RegisteredFunction;
use crate::ir::{self, Binary, Builtin, Expr, ExprKind, Statement, Type};
use crate::llvm::body::FunctionBody;

use crate::llvm::operations::{CheckedBody, CheckedValue, LoopEdges};
use ruff_text_size::{TextRange, TextSize};
use std::collections::BTreeSet;

/// Only take a body when all its operations have a lowering for the registered calling convention.
pub(crate) fn supports(checked: &RegisteredFunction) -> bool {
    let body = &checked.function.body;
    let mut uses_python_protocol = false;
    let private = private_slots(checked);
    let has_slot = |slot: &usize| private.contains(slot) || checked.local_names.contains_key(slot);
    let mut supported = true;
    ir::walk_statements(body, &mut |stmt| {
        supported &= match stmt {
            Statement::Assign(slot, _) => has_slot(slot),
            Statement::Return(_)
            | Statement::Expr(_)
            | Statement::While(..)
            | Statement::Break
            | Statement::Continue
            | Statement::Exhaust => true,
            Statement::Scope { body, .. } => !suspends(body),
            Statement::SetItem(receiver, _, _) => {
                uses_python_protocol |= receiver.ty == Type::PythonObject;
                matches!(receiver.ty, Type::Dict(_) | Type::PythonObject)
            }
            Statement::Clear(slot) => private.contains(slot),
            Statement::If(_, _, no) => no.is_empty(),
            Statement::IterNext {
                kind: ir::IterNextKind::Generic,
                iterator,
                key,
                value: None,
                ready,
            } => {
                checked.function.locals[*iterator] == Type::PythonObject
                    && [iterator, key, ready]
                        .into_iter()
                        .all(|slot| private.contains(slot))
            }
            _ => false,
        };
    });
    ir::walk_expressions(body, &mut |expr| {
        supported &= match &expr.kind {
            ExprKind::Local(slot) => has_slot(slot),
            ExprKind::None
            | ExprKind::Bool(_)
            | ExprKind::Int(_)
            | ExprKind::Float(_)
            | ExprKind::Str(_)
            | ExprKind::Bytes(_)
            | ExprKind::Not(_) => true,
            ExprKind::Scope { body, value, .. } => {
                let mut suspension = false;
                ir::walk_operand_expressions(value, &mut |expr| {
                    suspension |= matches!(
                        expr.kind,
                        ExprKind::Builtin(Builtin::RegisteredSuspend(..), _)
                    )
                });
                !suspends(body) && !suspension
            }
            ExprKind::Conditional(..) => true,
            ExprKind::List(items) => items.is_empty(),
            ExprKind::Dict { entries, .. } => entries.is_empty(),
            ExprKind::Set { values, .. } => values.is_empty(),
            ExprKind::Builtin(Builtin::Bool, args) => args.len() == 1,
            ExprKind::Builtin(Builtin::Append | Builtin::SetAdd, args) => args.len() == 2,
            ExprKind::Builtin(Builtin::RegisteredUnpack { .. }, args) => args.len() == 1,
            ExprKind::Builtin(Builtin::ListGet, args) => args.len() == 2,
            ExprKind::Builtin(Builtin::RegisteredNamespace(_), args) => args.len() <= 1,
            ExprKind::Binary(Binary::Add, left, right) => {
                expr.ty == Type::PythonObject
                    || left.ty == Type::Int && right.ty == Type::Int && expr.ty == Type::Int
            }
            ExprKind::Builtin(Builtin::RegisteredCall(call), operands) => {
                uses_python_protocol = true;
                operands.len() > usize::from(call.explicit_scope)
            }
            ExprKind::Builtin(Builtin::RegisteredIter, args) => {
                uses_python_protocol = true;
                args.len() == 1
            }
            ExprKind::Builtin(Builtin::RegisteredStatement(..), _) => true,
            ExprKind::Builtin(Builtin::RegisteredLocation(..), operands) => operands.len() == 1,
            ExprKind::Builtin(Builtin::RegisteredAttribute { action, .. }, operands) => {
                use ir::AttributeAction;
                uses_python_protocol |= *action != AttributeAction::Load;
                operands.len()
                    == match action {
                        AttributeAction::Load | AttributeAction::Delete => 1,
                        AttributeAction::Store | AttributeAction::InPlace(_) => 2,
                    }
            }
            ExprKind::Builtin(Builtin::RegisteredUnary(_), operands) => {
                matches!(operands.as_slice(), [operand] if operand.ty == Type::PythonObject)
            }
            ExprKind::Builtin(Builtin::RegisteredSubscript(..), operands) => operands.len() == 2,
            ExprKind::Builtin(Builtin::RegisteredName(_), scope) => scope.len() <= 1,
            ExprKind::Builtin(
                Builtin::RegisteredEllipsis
                | Builtin::RegisteredString(_)
                | Builtin::RegisteredComplex(..),
                operands,
            ) => operands.is_empty(),
            ExprKind::Builtin(Builtin::RegisteredDict(entries), operands) => {
                operands.len()
                    == entries
                        .iter()
                        .map(|entry| match entry {
                            ir::DictEntryKind::Pair => 2,
                            ir::DictEntryKind::Unpack => 1,
                        })
                        .sum::<usize>()
            }
            _ => false,
        };
    });
    supported && uses_python_protocol
}

/// Only IR scopes name compiler-owned storage. User-visible names stay in the Python frame.
fn private_slots(checked: &RegisteredFunction) -> BTreeSet<usize> {
    let body = &checked.function.body;
    let mut slots = BTreeSet::new();
    ir::walk_statements(body, &mut |stmt| {
        if let Statement::Scope { clear, .. } = stmt {
            slots.extend(clear);
        }
    });
    ir::walk_expressions(body, &mut |expr| {
        if let ExprKind::Scope { clear, .. } = &expr.kind {
            slots.extend(clear);
        }
    });
    slots.retain(|slot| !checked.local_names.contains_key(slot));
    slots
}

fn suspends(body: &[Statement]) -> bool {
    let mut suspension = false;
    ir::walk_expressions(body, &mut |expr| {
        suspension |= matches!(
            expr.kind,
            ExprKind::Builtin(Builtin::RegisteredSuspend(..), _)
        )
    });
    suspension
}

#[derive(Default)]
pub(super) struct IterationState {
    private_slots: Vec<usize>,
    first_slot: usize,
    cursors: BTreeSet<usize>,
    scopes: Vec<Vec<usize>>,
    loops: Vec<(LoopEdges, usize)>,
    in_expression: bool,
}

/// The registered binder owns the scope, argument identities and frame.
pub(crate) struct Frame<'f, 'a, 'p> {
    code: &'f mut Function<'a, 'p>,
    names: Option<&'f RegisteredFunction>,
}

impl Frame<'_, '_, '_> {
    fn evaluate(&mut self, expr: &Expr) -> CheckedValue {
        CheckedBody::registered(Frame {
            code: self.code,
            names: self.names,
        })
        .emit_value(expr)
    }

    fn evaluate_handle(&mut self, expr: &Expr) -> String {
        let value = self.evaluate(expr);
        self.materialize(&value)
    }

    pub(crate) fn lookup_local(&mut self, slot: usize) -> String {
        if self.code.iteration.private_slots.binary_search(&slot).is_ok() {
            return self.load_private(slot);
        }
        let binding = &self.names.expect("registered local names").local_names[&slot];
        if let Some(slot) = binding.namespace {
            let scope = self.load_private(slot);
            self.code.named_op("lookup", &scope, &binding.name, "0")
        } else {
            self.code.lookup(&binding.name)
        }
    }

    pub(crate) fn assign(&mut self, slot: usize, value: CheckedValue) {
        let handle = self.materialize(&value);
        if self.code.iteration.private_slots.binary_search(&slot).is_ok() {
            self.store_private(slot, &handle);
            return;
        }
        let binding = &self.names.expect("registered local names").local_names[&slot];
        let scope = if let Some(slot) = binding.namespace {
            self.load_private(slot)
        } else {
            self.code.scope.clone()
        };
        let name = self.code.code_name(&binding.name);
        let mark = self.target_mark();
        self.code
            .assign_owned_name(&scope, &name, &handle, mark.as_deref());
    }
    pub(crate) fn body(&mut self) -> &mut FunctionBody {
        self.code.flush_check();
        &mut self.code.body
    }

    pub(crate) fn block_name(&mut self) -> String {
        self.code.name("b")
    }
    pub(crate) fn invert(&mut self, value: &str) -> String {
        self.code.value(&format!("xor i1 {value}, true"))
    }
    pub(crate) fn mark(&mut self) -> String {
        self.code.temporary_mark()
    }
    pub(crate) fn target_mark(&mut self) -> Option<String> {
        self.code.iteration.in_expression.then(|| self.mark())
    }
    pub(crate) fn drop_truth(&mut self, mark: &str) {
        self.code.discard_truth(mark);
    }
    pub(crate) fn truth(&mut self, value: &str) -> String {
        self.code.truth(value)
    }
    pub(crate) fn target_release(&mut self, mark: Option<&str>) {
        self.code.release_target(mark);
    }
    pub(crate) fn op(&mut self, operation: &str, operands: &[&str]) -> String {
        self.code.op(operation, operands)
    }
    pub(crate) fn unpack(&mut self, value: &str, count: usize, star: Option<usize>) -> String {
        let count = self.code.integer(count);
        let star = star.map_or_else(|| "0".into(), |index| self.code.integer(index));
        self.code.op("unpack_read", &[value, &count, &star])
    }
    pub(crate) fn comprehension_scope(
        &mut self,
        parent: Option<String>,
        names: &[String],
    ) -> String {
        let parent = parent.unwrap_or_else(|| self.code.scope.clone());
        let names = self.code.lit(1, names.join(" ").as_bytes());
        self.code.op("comprehension_scope", &[&parent, &names])
    }
    pub(crate) fn start_expression(&mut self) -> (String, bool) {
        (
            self.mark(),
            std::mem::replace(&mut self.code.iteration.in_expression, true),
        )
    }
    pub(crate) fn finish_expression(&mut self, mark: &str, outer: bool, result: &CheckedValue) {
        self.code.iteration.in_expression = outer;
        let handle = match result {
            CheckedValue::Handle { id, .. } => id.as_str(),
            CheckedValue::IntOrHandle { fallback, .. } => fallback.as_str(),
            CheckedValue::Native(..) => "0",
        };
        self.code.keep_operands(mark, handle, "0");
    }
    fn physical_slot(&self, slot: usize) -> usize {
        let frame = &self.code.iteration;
        frame.first_slot + frame.private_slots.binary_search(&slot).expect("checked private slot")
    }

    fn load_private(&mut self, slot: usize) -> String {
        let slot = self.physical_slot(slot);
        self.code.value(&format!("load i64, ptr %ir_slot{slot}"))
    }
    fn store_private(&mut self, slot: usize, value: &str) {
        let slot = self.physical_slot(slot);
        self.code
            .line(&format!("store i64 {value}, ptr %ir_slot{slot}"));
    }
    pub(crate) fn enter_scope(&mut self, clear: &[usize]) {
        self.code.iteration.scopes.push(clear.to_vec());
    }
    pub(crate) fn leave_scope(&mut self) {
        self.code.iteration.scopes.pop();
    }
    pub(crate) fn clear_slots(&mut self, slots: &[usize]) {
        for &slot in slots {
            if self
                .names
                .is_some_and(|n| n.local_names.contains_key(&slot))
            {
                continue;
            }
            if self.code.iteration.cursors.contains(&slot) {
                let old = self.load_private(slot);
                self.store_private(slot, "0");
                self.code.op("break_iter", &[&old]);
            } else {
                self.store_private(slot, "0");
            }
        }
    }
    pub(crate) fn enter_loop(&mut self, edges: &LoopEdges) {
        let depth = self.code.iteration.scopes.len();
        self.code.iteration.loops.push((edges.clone(), depth));
    }
    pub(crate) fn leave_loop(&mut self) {
        self.code.iteration.loops.pop();
    }
    pub(crate) fn transfer_loop(&mut self, stmt: &Statement) {
        let (edges, depth) = self.code.iteration.loops.last().expect("loop transfer");
        let dest = match stmt {
            Statement::Break => &edges.done,
            Statement::Continue => &edges.step,
            _ => &edges.exhausted,
        }
        .clone();
        for slots in self.code.iteration.scopes[*depth..].to_vec().iter().rev() {
            self.clear_slots(slots);
        }
        self.code.jump(&dest);
    }
    pub(crate) fn begin_step(&mut self, mark: &str) {
        self.code.line(&format!(
            "call void @imre_dyn_comprehension_step(ptr %ctx, i64 {mark})"
        ));
    }
    pub(crate) fn pull_ready(&mut self, iterator: usize, ready: usize) -> (String, String) {
        let cursor = self.load_private(iterator);
        let has_else = self
            .code
            .iteration
            .loops
            .last()
            .is_some_and(|(edges, _)| edges.exhausted != edges.done);
        let more = self
            .code
            .op(if has_else { "more_else" } else { "more" }, &[&cursor]);
        self.store_private(ready, &more);
        let available = self
            .code
            .value(&format!("icmp eq i64 {more}, {}", permanent::TRUE));
        (cursor, available)
    }
    pub(crate) fn pull_item(&mut self, cursor: &str, key: usize) {
        let item = self.code.op("next", &[cursor]);
        self.store_private(key, &item);
    }
    pub(crate) fn declare_int_add(&mut self) {
        self.code.declare_integer_arithmetic();
    }

    pub(crate) fn integer(&mut self, text: &str, integer: Option<i64>) -> CheckedValue {
        match integer {
            Some(n) => CheckedValue::rooted_integer(self.code.integer(n), n),
            None => CheckedValue::handle(self.code.lit(0, text.as_bytes())),
        }
    }

    pub(crate) fn string(&mut self, value: &str) -> CheckedValue {
        CheckedValue::handle(self.code.text(value))
    }

    pub(crate) fn float(&mut self, value: f64) -> CheckedValue {
        CheckedValue::handle(self.code.lit(3, &value.to_le_bytes()))
    }

    pub(crate) fn bytes(&mut self, value: &[u8]) -> CheckedValue {
        CheckedValue::handle(self.code.lit(2, value))
    }

    pub(crate) fn boolean(&mut self, value: bool) -> CheckedValue {
        CheckedValue::handle(
            (if value {
                permanent::TRUE
            } else {
                permanent::FALSE
            })
            .to_string(),
        )
    }

    pub(crate) fn consume_expression(&mut self, expr: &Expr) -> bool {
        if let ExprKind::Builtin(Builtin::RegisteredStatement(range, simple), _) = &expr.kind {
            self.code.checkpoint(range);
            self.code.simple_call = *simple;
            true
        } else if matches!(
            expr.kind,
            ExprKind::Builtin(Builtin::RegisteredAttribute { action: ir::AttributeAction::Store, .. }, _)
        ) {
            let mark = self.target_mark();
            self.evaluate(expr);
            self.target_release(mark.as_deref());
            true
        } else if matches!(
            expr.kind,
            ExprKind::Builtin(
                Builtin::GlobalSet(_)
                    | Builtin::RegisteredImport(_)
                    | Builtin::RegisteredDeleteName(..)
                    | Builtin::RegisteredModuleAnnotation(_)
                    | Builtin::RegisteredAnnotationStart
                    | Builtin::RegisteredAnnotationInsert { .. }
                    | Builtin::RegisteredAttribute {
                        action: ir::AttributeAction::Delete | ir::AttributeAction::InPlace(_),
                        ..
                    },
                _
            )
        ) {
            self.evaluate(expr);
            true
        } else {
            false
        }
    }

    pub(crate) fn discard_expression(&mut self, value: &CheckedValue) {
        // Only the Python edge can produce an unawaited coroutine.
        let handle = match value {
            CheckedValue::Handle { id, .. } => Some(id),
            CheckedValue::IntOrHandle { fallback, .. } => Some(fallback),
            CheckedValue::Native(..) => None,
        };
        if let Some(handle) = handle {
            self.code.line(&format!(
                "call void @imre_dyn_discard_expression(ptr %ctx, i64 {handle})"
            ));
        }
    }

    pub(crate) fn return_result(&mut self, value: CheckedValue) {
        let value = self.materialize(&value);
        self.code.return_from_scope(value);
    }

    pub(crate) fn terminated(&self) -> bool {
        self.code.body.terminated
    }

    pub(crate) fn release_word(&mut self, owner: usize) {
        let word = self.code.value(&format!("load i64, ptr %int_owner{owner}"));
        self.code
            .line(&format!("store i64 1, ptr %int_owner{owner}"));
        self.code
            .line(&format!("call void @imre_int_release(i64 {word})"));
    }

    pub(crate) fn select(&mut self, test: &Expr, yes: &Expr, no: &Expr) -> CheckedValue {
        let names = self.names;
        self.code.select(
            |code, yes, no| {
                let condition = code.test_with_temporaries(|code| {
                    let handle = Frame { code, names }.evaluate_handle(test);
                    code.truth(&handle)
                });
                code.branch(&condition, yes, no);
            },
            |code| Frame { code, names }.evaluate(yes),
            |code| Frame { code, names }.evaluate(no),
        )
    }

    pub(crate) fn builtin(&mut self, op: &Builtin, operands: &[Expr]) -> CheckedValue {
        let handle = match (op, operands) {
            (Builtin::RegisteredName(name), []) => self.code.lookup(name),
            (Builtin::RegisteredName(name), [scope]) => {
                let scope = self.evaluate_handle(scope);
                self.code.named_op("lookup", &scope, name, "0")
            }
            (Builtin::RegisteredSuspend(kind, range), [value]) => {
                let names = self.names;
                self.code.suspend_value(*kind, *range, |code| {
                    Frame { code, names }.evaluate_handle(value)
                })
            }
            (Builtin::RegisteredEllipsis, []) => permanent::ELLIPSIS.to_string(),
            (Builtin::RegisteredComplex(real, imag), []) => self
                .code
                .lit(4, &[real.to_le_bytes(), imag.to_le_bytes()].concat()),
            (Builtin::RegisteredString(points), []) if self.code.source.has_templates => {
                self.code.linked_string(points.clone())
            }
            (Builtin::RegisteredString(points), []) => self.code.lit(7, points),
            (Builtin::RegisteredLocation(range, position), [value]) => {
                let previous_range = std::mem::replace(&mut self.code.current_range, *range);
                let previous_line = self.code.enter_expression(*position);
                let value = self.evaluate(value);
                self.code.leave_expression(previous_line);
                self.code.current_range = previous_range;
                return value;
            }
            (Builtin::RegisteredDefinition(site), header) => {
                let header = header
                    .iter()
                    .map(|value| self.evaluate_handle(value))
                    .collect::<Vec<_>>();
                self.code.registered_definition(*site, &header)
            }
            (Builtin::RegisteredImport(import), []) => {
                self.code.namespace_import(import);
                "0".to_owned()
            }
            (Builtin::RegisteredAnnotationStart, []) => {
                self.code.begin_annotations(None);
                "0".to_owned()
            }
            (Builtin::RegisteredAnnotationInsert { index, name }, [value]) => {
                let names = self.names;
                self.code.insert_annotation(Some(*index), name, |code| {
                    Frame { code, names }.evaluate_handle(value)
                });
                "0".to_owned()
            }
            (Builtin::RegisteredAnnotationResult, []) => self.code.take_annotation_result(),
            (Builtin::RegisteredModuleAnnotation(annotation), []) => {
                self.code.registered_module_annotation(annotation);
                "0".to_owned()
            }
            (Builtin::RegisteredDeleteName(name, range), []) => {
                let name = self.code.code_name(name);
                let scope = self.code.scope.clone();
                let (line, start) = self.code.source.positions.line_and_column(range.start());
                let (_, end) = self.code.source.positions.line_and_column(range.end());
                self.code
                    .frame
                    .op_at("delname", &[&scope, &name], Some([line, start, start, end]))
            }
            (Builtin::GlobalSet(slot), [value]) => {
                let value = self.evaluate_handle(value);
                let name = self.names.expect("registered local names").local_names[slot]
                    .name
                    .clone();
                self.bind(&name, &value);
                return CheckedValue::handle("0".into());
            }
            (Builtin::RegisteredSubscript(range, base_end), [base, key]) => {
                let names = self.names;
                self.code
                    .subscript(base, key, Some((*range, *base_end)), |code, expr| {
                        Frame { code, names }.evaluate_handle(expr)
                    })
            }
            (Builtin::RegisteredDict(entries), values) => {
                let mut values = values.iter();
                let items = entries.iter().map(|entry| {
                    let key = (*entry == ir::DictEntryKind::Pair)
                        .then(|| values.next().expect("checked dictionary key"));
                    (key, values.next().expect("checked dictionary value"))
                });
                let names = self.names;
                self.code.dict_display(items, |code, expr| {
                    Frame { code, names }.evaluate_handle(expr)
                })
            }
            _ => unreachable!("unsupported registered operation"),
        };
        CheckedValue::handle(handle)
    }

    pub(crate) fn bind(&mut self, name: &str, handle: &str) {
        let name = self.code.code_name(name);
        let scope = self.code.scope.clone();
        self.code.assign_owned_name(&scope, &name, handle, None);
    }

    pub(crate) fn object_operation(&mut self, op: &Builtin, operands: &[Expr]) -> CheckedValue {
        let (target, args) = operands
            .split_first()
            .expect("checked object operation has a target");
        let names = self.names;
        let handle = self
            .code
            .object_operation(op, target, args.iter(), |code, expr| {
                Frame { code, names }.evaluate_handle(expr)
            });
        CheckedValue::handle(handle)
    }

    pub(crate) fn exact_word(&mut self, handle: &str) -> String {
        self.code.value(&format!(
            "call i64 @imre_dyn_exact_int_word(ptr %ctx, i64 {handle})"
        ))
    }

    pub(crate) fn own_word(&mut self, word: &str) -> usize {
        let slot = self.code.integer_owners;
        self.code.integer_owners += 1;
        self.code
            .line(&format!("store i64 {word}, ptr %int_owner{slot}"));
        slot
    }

    pub(crate) fn materialize(&mut self, value: &CheckedValue) -> String {
        match value {
            CheckedValue::Handle { id, .. } => id.clone(),
            CheckedValue::IntOrHandle {
                word,
                fallback,
                owner,
            } => {
                let frame = &mut self.code;
                let native = frame.name("box_int");
                let done = frame.name("handle");
                let exact = frame.value(&format!("icmp ne i64 {word}, 0"));
                let mut join = crate::llvm::operations::ValueJoin::default();
                frame.flush_check();
                join.incoming(&mut frame.body, CheckedValue::handle(fallback.clone()));
                frame.branch(&exact, &native, &done);
                frame.label(&native);
                let boxed = frame.move_integer(*owner, word);
                frame.flush_check();
                join.jump(&mut frame.body, &done, CheckedValue::handle(boxed));
                frame.label(&done);
                let Some(CheckedValue::Handle { id, .. }) = join.finish(&mut frame.body) else {
                    unreachable!("both conversion edges hold Python handles")
                };
                id
            }
            CheckedValue::Native(value, owner) => {
                let (word, small) = value.int_word(owner.is_some());
                if let Some(slot) = owner {
                    return self.code.move_integer(*slot, word);
                }
                self.code.declare_integer_arithmetic();
                if !small {
                    self.code
                        .line(&format!("call void @imre_int_retain(i64 {word})"));
                }
                self.code.checked(&format!(
                    "call i64 @imre_dyn_take_int(ptr %ctx, i64 {word})"
                ))
            }
        }
    }

    pub(crate) fn protocol_add(&mut self, left: &str, right: &str) -> String {
        self.code.op("+", &[left, right])
    }
}

impl Function<'_, '_> {
    fn namespace_import(&mut self, import: &ir::NamespaceImport) {
        match import {
            ir::NamespaceImport::Modules(modules) => {
                for item in modules {
                    let mut module = self.import_name(&item.name, "0", 0);
                    if item.bind_leaf {
                        for part in item.name.split('.').skip(1) {
                            let member = self.code_name(part);
                            module = self.op("from", &[&module, &member]);
                        }
                    }
                    self.bind(&item.binding, &module);
                }
            }
            ir::NamespaceImport::From {
                module,
                level,
                names,
            } => {
                let fromlist = self.op("list", &[]);
                let members: Vec<_> = names.iter().map(|(name, _)| self.code_name(name)).collect();
                for member in &members {
                    self.op("append", &[&fromlist, member]);
                }
                let fromlist = self.op("tuple", &[&fromlist]);
                let module = self.import_name(module, &fromlist, *level);
                for ((name, binding), member) in names.iter().zip(members) {
                    if name == "*" {
                        self.frame
                            .op("import_star", &[&self.scope.clone(), &module]);
                    } else {
                        let value = self.op("from", &[&module, &member]);
                        self.bind(binding, &value);
                    }
                }
            }
        }
    }

    fn import_name(&mut self, name: &str, fromlist: &str, level: u32) -> String {
        let name = self.code_name(name);
        self.frame.checked(&format!(
            "call i64 @imre_dyn_import_name(ptr %ctx, i64 {}, i64 {name}, i64 {fromlist}, i32 {level})",
            self.scope
        ))
    }

    pub(super) fn object_operation<'e, T: 'e>(
        &mut self,
        op: &Builtin,
        target: &'e T,
        args: impl Iterator<Item = &'e T>,
        mut evaluate: impl FnMut(&mut Self, &T) -> String,
    ) -> String {
        match op {
            Builtin::RegisteredCall(call) => self.call_value(call, target, args, None, evaluate),
            Builtin::RegisteredUnary(op) => self.unary_value(*op, |f| evaluate(f, target)),
            Builtin::RegisteredAttribute {
                action,
                name,
                range,
            } => self.attribute_operation(
                *action,
                name,
                *range,
                std::iter::once(target).chain(args),
                evaluate,
            ),
            _ => unreachable!("selected Python object operation"),
        }
    }

    pub(super) fn unary_value(
        &mut self,
        op: ir::ObjectUnary,
        evaluate: impl FnOnce(&mut Self) -> String,
    ) -> String {
        let mark = self.temporary_mark();
        self.unwind_operands(&mark, |f| {
            let operand = evaluate(f);
            let name = match op {
                ir::ObjectUnary::Positive => "pos",
                ir::ObjectUnary::Negative => "neg",
                ir::ObjectUnary::Invert => "~",
                ir::ObjectUnary::Not => "not",
            };
            let value = f.op(name, &[&operand]);
            if op == ir::ObjectUnary::Not {
                f.discard_truth(&mark);
            } else {
                f.keep_operands(&mark, &value, "0");
            }
            value
        })
    }

    /// Evaluate the callable before its arguments. A lone *iterable is converted
    /// at invocation, after the keywords; a mixed positional list expands each * in place.
    pub(super) fn call_value<'e, T: 'e>(
        &mut self,
        call: &ir::RegisteredCall,
        target: &T,
        mut args: impl Iterator<Item = &'e T>,
        partition: Option<(u32, String)>,
        mut evaluate: impl FnMut(&mut Self, &T) -> String,
    ) -> String {
        use ir::{RegisteredArguments, RegisteredCallTarget};

        let mark = self.temporary_mark();
        if self.native_suspension
            && let RegisteredCallTarget::Callable { suspended_name: Some(name), .. } = &call.target
        {
            self.call_position(*name);
        }
        let (scope, target) = if call.explicit_scope {
            let scope = evaluate(self, target);
            (scope, args.next().expect("scoped call has a target"))
        } else {
            (self.scope.clone(), target)
        };
        let (callable, receiver, callee_end) = match &call.target {
            RegisteredCallTarget::Callable { end, .. } => (evaluate(self, target), None, *end),
            RegisteredCallTarget::Method { name, attribute } => self.unwind_operands(&mark, |f| {
                let receiver = evaluate(f, target);
                let direct = matches!(call.arguments, RegisteredArguments::Positional);
                let (callee, bound) = f.method_target(&receiver, name, *attribute, direct);
                f.keep_operands(&mark, &callee, bound.as_deref().unwrap_or("0"));
                (callee, bound, attribute.end())
            }),
        };
        let operands = match &call.arguments {
            RegisteredArguments::Positional => {
                let receiver_slot = receiver.is_some();
                let values: Vec<_> = receiver
                    .into_iter()
                    .chain(args.map(|a| evaluate(self, a)))
                    .collect();
                let (array, count) = self.handle_array(&values);
                if let Some((index, get_position)) = partition {
                    debug_assert!(receiver_slot && count == 2);
                    CallArguments::PartitionField {
                        array,
                        index,
                        get_position,
                    }
                } else {
                    CallArguments::Positional {
                        array,
                        count,
                        receiver_slot,
                    }
                }
            }
            RegisteredArguments::Keywords { positional, names } => {
                let values: Vec<_> = args.map(|a| evaluate(self, a)).collect();
                let (array, _) = self.handle_array(&values);
                CallArguments::FixedKeywords {
                    array,
                    positional: *positional,
                    keywords: names.len(),
                    names: self.output.data(names.join("\0").as_bytes()),
                }
            }
            RegisteredArguments::Packed(pack) => self.unwind_operands(&mark, |f| {
                let (args, kwargs) = f.argument_pack(pack, args, Some(&callable), &mut evaluate);
                CallArguments::Packed { args, kwargs }
            }),
        };
        let span = self.expression_range_span(call.range, callee_end, b'(');
        self.call_position(call.range);
        self.call_in_scope(&callable, operands, &mark, span, &scope)
    }

    /// Python calls convert a lone starred operand after the keywords. Class bases
    /// and mixed positional operands are expanded before the following operand.
    pub(super) fn argument_pack<'e, T: 'e>(
        &mut self,
        layout: &ir::PackedArguments,
        mut args: impl Iterator<Item = &'e T>,
        callable: Option<&str>,
        evaluate: &mut impl FnMut(&mut Self, &T) -> String,
    ) -> (String, String) {
        let (positional, deferred) =
            if let Some(callee) = callable.filter(|_| layout.starred.as_slice() == [true]) {
                let mark = self.temporary_mark();
                let iterable = evaluate(self, args.next().expect("single starred operand"));
                (iterable, Some((callee, mark)))
            } else {
                let values = self.op("list", &[]);
                self.pack_positionals(
                    &values,
                    layout.starred.iter().copied().zip(&mut args),
                    evaluate,
                );
                (values, None)
            };
        let kwargs = self.pack_keywords(
            layout.keywords.iter().map(Option::as_deref).zip(args),
            callable,
            evaluate,
        );
        if let Some((callee, mark)) = deferred {
            let values = self.pack_operand("spread_arguments", &[&positional, callee]);
            self.keep_operands(&mark, &values, &kwargs);
            (values, kwargs)
        } else {
            (positional, kwargs)
        }
    }

    pub(super) fn pack_positionals<'e, T: 'e>(
        &mut self,
        list: &str,
        args: impl Iterator<Item = (bool, &'e T)>,
        evaluate: &mut impl FnMut(&mut Self, &T) -> String,
    ) {
        for (starred, operand) in args {
            if starred {
                self.extend_operand(list, |f| evaluate(f, operand));
            } else {
                let value = evaluate(self, operand);
                self.op("append", &[list, &value]);
            }
        }
    }

    /// Each explicit group after ** is evaluated before merging and checking for duplicates.
    fn pack_keywords<'n, 'e, T: 'e>(
        &mut self,
        keywords: impl Iterator<Item = (Option<&'n str>, &'e T)>,
        callable: Option<&str>,
        evaluate: &mut impl FnMut(&mut Self, &T) -> String,
    ) -> String {
        let mut keywords = keywords.peekable();
        if keywords.peek().is_none() {
            return "0".into();
        }
        let (merge, callable) = match callable {
            Some(callable) => ("keywords", callable),
            None => ("class_keywords", "0"),
        };
        let kwargs = self.op("dict", &[]);
        let mut unpacked = false;
        while let Some((name, _)) = keywords.peek() {
            if name.is_none() {
                let (_, operand) = keywords.next().unwrap();
                let mark = self.temporary_mark();
                self.unwind_operands(&mark, |f| {
                    let value = evaluate(f, operand);
                    f.pack_operand(merge, &[&kwargs, &value, callable]);
                    // The merged dictionary now owns the keyword values.
                    f.discard_truth(&mark);
                });
                unpacked = true;
            } else {
                let group = if unpacked {
                    self.op("dict", &[])
                } else {
                    kwargs.clone()
                };
                while let Some((Some(name), operand)) = keywords.next_if(|(name, _)| name.is_some())
                {
                    let value = evaluate(self, operand);
                    self.named_op("keyword", &group, name, &value);
                }
                if unpacked {
                    self.pack_operand(merge, &[&kwargs, &group, callable]);
                }
            }
        }
        kwargs
    }

    pub(super) fn subscript<T>(
        &mut self,
        base: &T,
        key: &T,
        site: Option<(TextRange, TextSize)>,
        mut evaluate: impl FnMut(&mut Self, &T) -> String,
    ) -> String {
        let base = evaluate(self, base);
        let key = evaluate(self, key);
        let span =
            site.and_then(|(range, base_end)| self.expression_range_span(range, base_end, b'['));
        self.op_at("get", &[&base, &key], span)
    }

    /// Construct a Python dictionary with CPython's BUILD_MAP, MAP_ADD and DICT_UPDATE order.
    pub(super) fn dict_display<'e, T: 'e>(
        &mut self,
        items: impl Iterator<Item = (Option<&'e T>, &'e T)>,
        mut evaluate: impl FnMut(&mut Self, &T) -> String,
    ) -> String {
        // CPython's codegen_dict flushes on the pair *after* the buffered operands exceed
        // its stack-use guideline. codegen_subdict streams any chunk over that guideline.
        const STACK_USE_GUIDELINE: usize = 30;
        let mut items = items.peekable();
        if items.peek().is_none() {
            return self.op("dict", &[]);
        }

        let mark = self.temporary_mark();
        self.unwind_operands(&mark, |f| {
            let mut result = None;
            while let Some((key, value)) = items.next() {
                let Some(key) = key else {
                    let dict = result.get_or_insert_with(|| f.op("dict", &[]));
                    let mapping = evaluate(f, value);
                    f.op("update", &[dict, &mapping]);
                    f.keep_operands(&mark, dict, "0");
                    continue;
                };

                let mut chunk = vec![(key, value)];
                while chunk.len() < STACK_USE_GUIDELINE / 2 + 2
                    && let Some((Some(key), value)) = items.next_if(|(key, _)| key.is_some())
                {
                    chunk.push((key, value));
                }
                let streamed = (2 * chunk.len() > STACK_USE_GUIDELINE).then(|| f.op("dict", &[]));
                let mut pending = Vec::new();
                for (key, value) in chunk {
                    let key = evaluate(f, key);
                    let value = evaluate(f, value);
                    if let Some(part) = &streamed {
                        f.op("put", &[part, &key, &value]);
                        f.keep_operands(&mark, part, result.as_deref().unwrap_or("0"));
                    } else {
                        pending.push((key, value));
                    }
                }
                let part = streamed.unwrap_or_else(|| {
                    let part = f.op("dict", &[]);
                    for (key, value) in pending {
                        f.op("put", &[&part, &key, &value]);
                    }
                    f.keep_operands(&mark, &part, result.as_deref().unwrap_or("0"));
                    part
                });
                if let Some(dict) = &result {
                    f.op("update", &[dict, &part]);
                    f.keep_operands(&mark, dict, "0");
                } else {
                    result = Some(part);
                }
            }
            result.expect("nonempty dictionary display")
        })
    }

    /// Emit a registered expression whose names are resolved through this live frame.
    pub(super) fn registered_value(&mut self, expression: &Expr) -> String {
        let first = self.integer_owners;
        let outer = self.expression_integer_start.replace(first);
        let result = Frame {
            code: self,
            names: None,
        }
        .evaluate_handle(expression);
        self.expression_integer_start = outer;
        result
    }

    pub(super) fn checked_block(
        &mut self,
        statements: &[Statement],
        checked: &RegisteredFunction,
    ) {
        CheckedBody::registered(Frame {
            code: self,
            names: Some(checked),
        })
        .emit_block(statements);
    }

    /// Give each prepared suite private storage in the prologue of its physical function.
    /// Residual source can enter another prepared suite before returning to this one.
    pub(super) fn with_checked_slots(
        &mut self,
        checked: &RegisteredFunction,
        emit: impl FnOnce(&mut Self),
    ) {
        let enclosing = std::mem::take(&mut self.iteration);
        let roots = self.live_slots.len();
        let first = self.private_slot_count;
        self.iteration.private_slots = private_slots(checked).into_iter().collect();
        self.iteration.first_slot = first;
        let end = first + self.iteration.private_slots.len();
        self.private_slot_count = end;
        self.live_slots.extend(first..end);
        ir::walk_statements(&checked.function.body, &mut |stmt| {
            if let Statement::IterNext { iterator, .. } = stmt {
                self.iteration.cursors.insert(*iterator);
            }
        });
        emit(self);
        self.live_slots.truncate(roots);
        self.iteration = enclosing;
    }

    pub(super) fn checked_body(&mut self, checked: &RegisteredFunction) {
        self.with_checked_slots(checked, |code| {
            CheckedBody::registered(Frame {
                code,
                names: Some(checked),
            })
            .emit_function(&checked.function);
        });
    }
}

impl RegisteredFrame<'_, '_> {
    /// The heap takes the reference even if adding its temporary root fails.
    fn move_integer(&mut self, slot: usize, word: &str) -> String {
        self.line(&format!("store i64 1, ptr %int_owner{slot}"));
        self.checked(&format!(
            "call i64 @imre_dyn_take_int(ptr %ctx, i64 {word})"
        ))
    }

    fn declare_integer_arithmetic(&mut self) {
        self.output.declarations.checked_integer_arithmetic = true;
    }
}
