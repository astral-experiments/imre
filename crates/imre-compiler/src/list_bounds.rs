//! Element intervals for exact integer lists in the closed call graph.
//!
//! Facts describe live list contents, so every unknown call or heap mutation
//! invalidates them. Call parameters gain facts only when every syntactic call
//! site supplies a known interval without intervening argument effects.

use crate::ir::{self, Builtin, Expr, ExprKind, Module, Statement, Type};
use ruff_python_ast::UnaryOp;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Bounds {
    min: i64,
    max: i64,
}

impl Bounds {
    fn union(self, other: Self) -> Self {
        Self {
            min: self.min.min(other.min),
            max: self.max.max(other.max),
        }
    }
}

type Fact = Option<Bounds>;
type State = Vec<Fact>;

fn join(left: Fact, right: Fact) -> Fact {
    Some(left?.union(right?))
}

fn join_states(left: &mut [Fact], right: &[Fact]) {
    for (left, &right) in left.iter_mut().zip(right) {
        *left = join(*left, right);
    }
}

struct Calls {
    seen: bool,
    inputs: Vec<Fact>,
}

/// Infers only facts supported by all callers, starting with unknown parameters.
/// Repeated passes propagate through private helpers without assuming recursive
/// contracts. An unexpected nonconverging graph simply keeps checked reads.
pub(crate) fn specialize(module: &mut Module) {
    if module
        .functions
        .iter()
        .all(|function| function.params.is_empty())
    {
        Analysis::Rewrite.module(module, &[]);
        return;
    }
    let mut inputs = module
        .functions
        .iter()
        .map(|function| vec![None; function.params.len()])
        .collect::<Vec<_>>();
    for _ in 0..=module.functions.len() {
        let mut calls = inputs
            .iter()
            .map(|params| Calls {
                seen: false,
                inputs: vec![None; params.len()],
            })
            .collect::<Vec<_>>();
        Analysis::Infer(&mut calls).module(module, &inputs);
        let next = calls
            .into_iter()
            .map(|call| call.inputs)
            .collect::<Vec<_>>();
        if next == inputs {
            Analysis::Rewrite.module(module, &inputs);
            return;
        }
        inputs = next;
    }
}

enum Analysis<'a> {
    Infer(&'a mut [Calls]),
    Rewrite,
}

impl Analysis<'_> {
    /// Visits the function bodies using the input bounds from the preceding iteration.
    fn module(&mut self, module: &mut Module, inputs: &[Vec<Fact>]) {
        for (id, function) in module
            .functions
            .iter_mut()
            .chain([&mut module.entry])
            .enumerate()
        {
            let mut state = vec![None; function.locals.len()];
            if let Some(inputs) = inputs.get(id) {
                for (&slot, &fact) in function.params.iter().zip(inputs) {
                    state[slot] = fact;
                }
            }
            self.block(&mut function.body, &mut state);
        }
    }

    /// Joins one call site's arguments into the callee's incoming bounds.
    fn record_call(&mut self, id: usize, argument: impl Fn(usize) -> Fact) {
        let Self::Infer(calls) = self else { return };
        let call = &mut calls[id];
        for (parameter, input) in call.inputs.iter_mut().enumerate() {
            let fact = argument(parameter);
            *input = if call.seen { join(*input, fact) } else { fact };
        }
        call.seen = true;
    }

    fn block(&mut self, body: &mut [Statement], state: &mut State) {
        for statement in body {
            match statement {
                Statement::Assign(slot, value) => {
                    let fact = self.expression(value, state);
                    state[*slot] = fact;
                }
                Statement::Move(target, source) => {
                    state[*target] = state[*source].take();
                }
                Statement::Clear(slot) => state[*slot] = None,
                Statement::Scope { body, clear } => {
                    self.block(body, state);
                    for &slot in clear.iter() {
                        state[slot] = None;
                    }
                }
                Statement::Expr(value) | Statement::Return(value) => {
                    self.expression(value, state);
                }
                Statement::Match(..) => {
                    // A failed equality or guard may have mutated any live list.
                    ir::statement_children_mut(statement, |child| {
                        state.fill(None);
                        match child {
                            ir::ChildMut::Expression(expr) => {
                                self.expression(expr, state);
                            }
                            ir::ChildMut::Suite(body) => self.block(body, state),
                        }
                    });
                    state.fill(None);
                }
                Statement::If(test, yes, no) => {
                    self.expression(test, state);
                    let mut other = state.clone();
                    self.block(yes, state);
                    self.block(no, &mut other);
                    join_states(state, &other);
                }
                Statement::While(test, body, orelse) => {
                    self.loop_body(Some(test), body, orelse, state);
                }
                Statement::For {
                    args, body, orelse, ..
                } => {
                    for arg in args {
                        self.expression(arg, state);
                    }
                    self.loop_body(None, body, orelse, state);
                }
                Statement::With {
                    manager,
                    binding,
                    body,
                    ..
                } => {
                    self.expression(manager, state);
                    state.fill(None);
                    if let Some(binding) = binding {
                        self.block(&mut binding.body, state);
                    }
                    self.block(body, state);
                    state.fill(None);
                }
                Statement::Try {
                    body,
                    handlers,
                    orelse,
                    finalbody,
                } => {
                    // Any prefix can raise, and handlers/finally see different
                    // ownership states. Fresh producers inside each region can
                    // still establish their own local facts.
                    state.fill(None);
                    self.block(body, state);
                    self.block(orelse, state);
                    for handler in handlers {
                        state.fill(None);
                        self.block(&mut handler.body, state);
                    }
                    state.fill(None);
                    self.block(finalbody, state);
                    state.fill(None);
                }
                Statement::SetItem(order, receiver, index, value) => {
                    order.evaluate(receiver, index, value, |expr| self.expression(expr, state));
                    state.fill(None);
                }
                Statement::AugmentItem(receiver, index, _, value) => {
                    self.expression(receiver, state);
                    self.expression(index, state);
                    self.expression(value, state);
                    state.fill(None);
                }
                Statement::SetField(receiver, _, value) => {
                    self.expression(value, state);
                    self.expression(receiver, state);
                    state.fill(None);
                }
                Statement::AugmentField(receiver, _, _, value) => {
                    self.expression(receiver, state);
                    self.expression(value, state);
                    state.fill(None);
                }
                Statement::Raise(value) => {
                    if let Some(value) = value {
                        self.expression(&mut value.exception, state);
                        if let Some(cause) = &mut value.cause {
                            self.expression(cause, state);
                        }
                    }
                }
                Statement::IterNext {
                    key, value, ready, ..
                } => {
                    for slot in [Some(*key), *value, Some(*ready)].into_iter().flatten() {
                        state[slot] = None;
                    }
                }
                Statement::Position(..)
                | Statement::Break(_)
                | Statement::Continue(_)
                | Statement::Exhaust => {}
            }
        }
    }

    /// Backedges may repeat any body write. Mutating or callback-capable bodies
    /// discard all incoming heap facts before the first analyzed iteration.
    fn loop_body(
        &mut self,
        test: Option<&mut Expr>,
        body: &mut [Statement],
        orelse: &mut [Statement],
        state: &mut State,
    ) {
        let mut header = state.clone();
        if !block_readonly(body) || test.as_deref().is_some_and(|test| !readonly(test)) {
            header.fill(None);
        } else {
            let mut invalidate = |slot: usize, _: Option<&Expr>| header[slot] = None;
            ir::walk_writes(body, &mut invalidate);
            if let Some(test) = test.as_deref() {
                ir::walk_expression_writes(test, invalidate);
            }
        }
        if let Some(test) = test {
            self.expression(test, &mut header);
        }
        self.block(body, &mut header.clone());
        self.block(orelse, &mut header);
        // Zero iterations, breaks, and exhausted iterations can expose distinct
        // local values. No post-loop heap fact is needed for this narrow pass.
        state.fill(None);
    }

    fn expression(&mut self, expression: &mut Expr, state: &mut State) -> Fact {
        let python_unary = expression.uses_python_unary_protocol();
        match &mut expression.kind {
            ExprKind::Unary(_, operand) if python_unary => {
                self.expression(operand, state);
                state.fill(None);
                None
            }
            ExprKind::Local(slot) => state[*slot],
            ExprKind::Builtin(operation, args) => {
                let arguments_readonly = args.iter().all(readonly);
                let facts = args
                    .iter_mut()
                    .map(|arg| self.expression(arg, state))
                    .collect::<Vec<_>>();
                if !operation.contract(args).preserves_list_storage() {
                    state.fill(None);
                    return None;
                }
                match operation {
                    Builtin::StringIntList(_) => Some(Bounds { min: 0, max: 9 }),
                    Builtin::Slice if expression.ty == Type::ListInt && arguments_readonly => {
                        facts.first().copied().flatten()
                    }
                    Builtin::ListGet(origin)
                        if arguments_readonly && args[0].ty == Type::ListInt =>
                    {
                        if matches!(self, Self::Rewrite)
                            && let Some(bounds) = facts.first().copied().flatten()
                        {
                            *operation = Builtin::ListGetBounded {
                                min: bounds.min,
                                max: bounds.max,
                                origin: origin.take(),
                            };
                        }
                        None
                    }
                    _ => None,
                }
            }
            ExprKind::Call(id, args) => {
                let infer = matches!(self, Self::Infer(_)) && args.values.iter().all(readonly);
                let mut values = Vec::new();
                for argument in &mut args.values {
                    let fact = self.expression(argument, state);
                    if infer {
                        values.push(fact);
                    }
                }
                self.record_call(*id, |parameter| {
                    let &source = args.order.get(parameter)?;
                    values.get(source).copied().flatten()
                });
                state.fill(None);
                None
            }
            ExprKind::Callable { function, receiver } => {
                if let Some(receiver) = receiver {
                    self.expression(receiver, state);
                }
                // A callback may receive inputs absent from direct call sites.
                self.record_call(*function, |_| None);
                None
            }
            ExprKind::Invoke { callable, args } => {
                self.expression(callable, state);
                for argument in &mut args.values {
                    self.expression(argument, state);
                }
                state.fill(None);
                None
            }
            ExprKind::Construct { initializer, args } => {
                for arg in &mut args.values {
                    self.expression(arg, state);
                }
                // Initializers have an implicit receiver; avoid guessing their
                // positional mapping and keep every parameter unknown.
                self.record_call(*initializer, |_| None);
                state.fill(None);
                None
            }
            ExprKind::Scope { body, value, clear } => {
                self.block(body, state);
                let fact = self.expression(value, state);
                for &slot in clear.iter() {
                    state[slot] = None;
                }
                fact
            }
            ExprKind::List(values) => {
                let mut bounds = None;
                let mut all_literals = true;
                for value in values.iter_mut() {
                    self.expression(value, state);
                    if let Some(value) = literal(value) {
                        let next = Bounds {
                            min: value,
                            max: value,
                        };
                        bounds = Some(bounds.map_or(next, |old: Bounds| old.union(next)));
                    } else {
                        all_literals = false;
                    }
                }
                (expression.ty == Type::ListInt && all_literals)
                    .then_some(bounds.unwrap_or(Bounds { min: 0, max: 0 }))
            }
            ExprKind::Tuple(values) | ExprKind::Set { values, .. } => {
                for value in values {
                    self.expression(value, state);
                }
                None
            }
            ExprKind::Dict(entries) => {
                for crate::ir::DictEntry { key, value } in entries {
                    if let Some(key) = key {
                        self.expression(key, state);
                    }
                    self.expression(value, state);
                }
                if expression.ty == Type::PythonObject {
                    state.fill(None);
                }
                None
            }
            ExprKind::Compare(values, ops)
                if crate::ir::Compare::uses_python_protocol(expression.ty, ops) =>
            {
                for (index, value) in values.iter_mut().enumerate() {
                    self.expression(value, state);
                    if index != 0 {
                        state.fill(None);
                    }
                }
                None
            }
            ExprKind::Compare(values, _) | ExprKind::Boolean(_, values) => {
                for (index, value) in values.iter_mut().enumerate() {
                    let skipped = state.clone();
                    self.expression(value, state);
                    if index != 0 {
                        join_states(state, &skipped);
                    }
                }
                None
            }
            ExprKind::Conditional(test, yes, no) => {
                self.expression(test, state);
                let mut other = state.clone();
                let yes = self.expression(yes, state);
                let no = self.expression(no, &mut other);
                join_states(state, &other);
                join(yes, no)
            }
            ExprKind::BooleanValue(_, left, right, predicate) => {
                let left = self.expression(left, state);
                if let Some(predicate) = predicate {
                    state[predicate.receiver] = left;
                    self.expression(&mut predicate.condition, state);
                    state[predicate.receiver] = None;
                }
                let skipped = state.clone();
                let right = self.expression(right, state);
                join_states(state, &skipped);
                join(left, right)
            }
            ExprKind::Binary(_, left, right) => {
                self.expression(left, state);
                self.expression(right, state);
                None
            }
            ExprKind::Bind(slot, value) => {
                self.expression(value, state);
                state[*slot] = None;
                None
            }
            ExprKind::Present(value)
            | ExprKind::Absent(value)
            | ExprKind::Unwrap(value)
            | ExprKind::Unary(_, value)
            | ExprKind::Field(value, _) => {
                self.expression(value, state);
                None
            }
            ExprKind::Exception { message, .. } => {
                if let Some(message) = message {
                    self.expression(message, state);
                }
                None
            }
            ExprKind::Int(_)
            | ExprKind::Float(_)
            | ExprKind::Bool(_)
            | ExprKind::Str(_)
            | ExprKind::Bytes(_)
            | ExprKind::None => None,
        }
    }
}

fn literal(value: &Expr) -> Option<i64> {
    let value = match &value.kind {
        ExprKind::Int(value) => value.parse().ok()?,
        ExprKind::Unary(UnaryOp::USub, value) => literal(value)?.checked_neg()?,
        _ => return None,
    };
    (-(1_i64 << 62)..(1_i64 << 62))
        .contains(&value)
        .then_some(value)
}

/// Local rebinding is harmless to the contents of a previously captured list;
/// heap writes and callbacks can change aliases and are never treated as pure.
fn readonly(expression: &Expr) -> bool {
    match &expression.kind {
        ExprKind::Unary(..) if expression.uses_python_unary_protocol() => false,
        ExprKind::Call(..) | ExprKind::Construct { .. } | ExprKind::Invoke { .. } => false,
        ExprKind::Callable { receiver, .. } => receiver.as_deref().is_none_or(readonly),
        ExprKind::Builtin(operation, args) => {
            operation.contract(args).preserves_list_storage() && args.iter().all(readonly)
        }
        ExprKind::Scope { body, value, .. } => block_readonly(body) && readonly(value),
        ExprKind::List(values)
        | ExprKind::Tuple(values)
        | ExprKind::Set { values, .. }
        | ExprKind::Boolean(_, values) => values.iter().all(readonly),
        ExprKind::Compare(values, ops) => {
            !crate::ir::Compare::uses_python_protocol(expression.ty, ops)
                && values.iter().all(readonly)
        }
        ExprKind::Dict(entries) => {
            expression.ty != Type::PythonObject
                && entries
                    .iter()
                    .all(|entry| entry.key.as_ref().is_some_and(readonly) && readonly(&entry.value))
        }
        ExprKind::Bind(_, value)
        | ExprKind::Present(value)
        | ExprKind::Absent(value)
        | ExprKind::Unwrap(value)
        | ExprKind::Unary(_, value)
        | ExprKind::Field(value, _) => readonly(value),
        ExprKind::BooleanValue(_, left, right, predicate) => {
            readonly(left)
                && predicate.as_ref().is_none_or(|p| readonly(&p.condition))
                && readonly(right)
        }
        ExprKind::Binary(_, left, right) => readonly(left) && readonly(right),
        ExprKind::Conditional(test, yes, no) => readonly(test) && readonly(yes) && readonly(no),
        ExprKind::Exception { message, .. } => message.as_deref().is_none_or(readonly),
        _ => true,
    }
}

/// Checks that a region cannot mutate heap contents or invoke user callbacks.
/// Local assignments and rebinding remain allowed; callers that require stable
/// local identities must separately reject writes to those locals.
pub(crate) fn block_readonly(body: &[Statement]) -> bool {
    body.iter().all(|statement| match statement {
        Statement::Assign(_, value) | Statement::Expr(value) | Statement::Return(value) => {
            readonly(value)
        }
        Statement::Scope { body, .. } => block_readonly(body),
        Statement::If(test, yes, no) | Statement::While(test, yes, no) => {
            readonly(test) && block_readonly(yes) && block_readonly(no)
        }
        Statement::For {
            args, body, orelse, ..
        } => args.iter().all(readonly) && block_readonly(body) && block_readonly(orelse),
        Statement::Raise(value) => value.as_ref().is_none_or(|value| {
            readonly(&value.exception) && value.cause.as_deref().is_none_or(readonly)
        }),
        Statement::Move(..)
        | Statement::Clear(_)
        | Statement::Position(..)
        | Statement::Break(_)
        | Statement::Continue(_)
        | Statement::Exhaust => true,
        _ => false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{CallArgs, walk_expressions};

    fn lower(source: &str) -> Module {
        let graph = crate::modules::SourceGraph::single(source).unwrap();
        let mut module = crate::frontend::lower(&graph).unwrap();
        crate::tuples::scalarize(&mut module);
        module
    }

    fn reads(module: &Module) -> Vec<(i64, i64)> {
        let mut reads = Vec::new();
        for function in module.functions.iter().chain([&module.entry]) {
            walk_expressions(&function.body, &mut |value| {
                if let ExprKind::Builtin(Builtin::ListGetBounded { min, max, .. }, _) = value.kind {
                    reads.push((min, max));
                }
            });
        }
        reads
    }

    #[test]
    fn a_unary_protocol_call_invalidates_aliased_list_bounds() {
        let mut state = vec![Some(Bounds { min: 0, max: 9 }), None];
        let mut value = Expr::unary(
            UnaryOp::Invert,
            Expr::local(1, Type::PythonObject),
            Type::PythonObject,
        );
        assert!(!readonly(&value));
        Analysis::Rewrite.expression(&mut value, &mut state);
        assert_eq!(state, vec![None, None]);
    }

    #[test]
    fn digits_and_slices_keep_bounds_through_a_private_helper() {
        let mut module = lower(
            r#"
def checksum(text: str) -> int:
    digits = list(map(int, text))
    odd = sum(digits[-1::-2])
    return odd + sum([sum(divmod(2 * digit, 10)) for digit in digits[-2::-2]])
"#,
        );
        specialize(&mut module);
        assert_eq!(reads(&module), vec![(0, 9)]);
        crate::list_specialization::specialize(&mut module);
        let ranges = crate::ranges::analyze(&module);
        let mut guarded = false;
        for function in &module.functions {
            walk_expressions(&function.body, &mut |value| {
                if matches!(
                    value.kind,
                    ExprKind::Builtin(Builtin::ListGetBounded { .. }, _)
                ) {
                    assert_eq!(
                        ranges.integer(value),
                        Some(crate::ranges::IntRange { min: 0, max: 9 })
                    );
                }
                guarded |= matches!(value.kind, ExprKind::Builtin(Builtin::ListAllI32, _));
            });
        }
        assert!(
            !guarded,
            "proven list contents do not need a runtime rescan"
        );
    }

    #[test]
    fn all_call_sites_join_bounds_and_bind_keyword_arguments() {
        let mut module = lower(
            r#"
def read(index: int, values: list[int]) -> int:
    return values[index]

def caller(text: str) -> int:
    first = read(values=list(map(int, text)), index=0)
    return first + read(0, [-3, 20])
"#,
        );
        specialize(&mut module);
        assert_eq!(reads(&module), vec![(-3, 20)]);
    }

    #[test]
    fn unknown_calls_alias_mutation_and_later_argument_effects_drop_facts() {
        for source in [
            r#"
def read(values: list[int]) -> int:
    return values[0]

def caller(text: str, unknown: list[int]) -> int:
    return read(list(map(int, text))) + read(unknown)
"#,
            r#"
def read(values: list[int]) -> int:
    return values[0]

def caller(text: str) -> int:
    digits = list(map(int, text))
    alias = digits
    alias.append(1000)
    return read(digits)
"#,
            r#"
def mutate(values: list[int]) -> int:
    values[0] = 1000
    return 0

def read(values: list[int], index: int) -> int:
    return values[index]

def caller(text: str) -> int:
    digits = list(map(int, text))
    return read(digits, mutate(digits))
"#,
        ] {
            let mut module = lower(source);
            specialize(&mut module);
            assert!(reads(&module).is_empty());
        }
    }

    #[test]
    fn loop_backedges_reject_rebound_and_mutated_sources() {
        for source in [
            r#"
def read(values: list[int], other: list[int]) -> int:
    total = 0
    for index in range(2):
        total += values[0]
        values = other
    return total

def caller(text: str, unknown: list[int]) -> int:
    return read(list(map(int, text)), unknown)
"#,
            r#"
def caller(text: str) -> int:
    digits = list(map(int, text))
    alias = digits
    total = 0
    for index in range(2):
        total += digits[0]
        alias[0] = 1000
    return total
"#,
        ] {
            let mut module = lower(source);
            specialize(&mut module);
            assert!(reads(&module).is_empty());
        }
    }

    #[test]
    fn recursive_unknown_call_site_cannot_bootstrap_a_parameter_fact() {
        let mut module = lower(
            r#"
def read(values: list[int], unknown: list[int]) -> int:
    return values[0]

def caller(text: str, unknown: list[int]) -> int:
    return read(list(map(int, text)), unknown)
"#,
        );
        let unknown = module.functions[0].params[1];
        let argument = Expr::local(unknown, Type::ListInt);
        // The frontend rejects recursive source. Inject an additional unknown
        // syntactic site to exercise the pass's independent closed-graph proof.
        module.functions[0].body.push(Statement::Expr(Expr {
            ty: Type::Int,
            kind: ExprKind::Call(0, CallArgs::positional(vec![argument.clone(), argument])),
        }));
        specialize(&mut module);
        assert!(reads(&module).is_empty());
    }

    #[test]
    fn address_taken_functions_cannot_inherit_only_direct_call_bounds() {
        let mut module = lower(
            r#"
def read(values: list[int]) -> int:
    return values[0]

def caller(text: str) -> int:
    return read(list(map(int, text)))
"#,
        );
        let ty = module.types.callable(vec![Type::ListInt], Type::Int);
        module.entry.body.push(Statement::Expr(Expr {
            ty,
            kind: ExprKind::Callable {
                function: 0,
                receiver: None,
            },
        }));
        specialize(&mut module);
        assert!(reads(&module).is_empty());
    }

    #[test]
    fn noninteger_storage_and_arbitrary_precision_literals_keep_generic_reads() {
        let mut module = lower(
            r#"
def empty_text() -> str:
    values: list[str] = []
    return values[0]

def huge_integer() -> int:
    values = [1000000000000000000000000000000000000]
    return values[0]
"#,
        );
        specialize(&mut module);
        assert!(reads(&module).is_empty());
    }
}
