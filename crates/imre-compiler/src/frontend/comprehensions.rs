//! Eager displays normally use private functions. Displays containing assignment
//! expressions run inline with private iteration slots so writes reach their
//! enclosing function, including when a later iteration raises.

use ruff_python_ast::UnaryOp;
use std::cell::{Cell, RefCell};

use imre_ast::comprehension::{Output, Plan};
use ruff_python_ast::visitor::{Visitor, walk_stmt};

use super::*;

mod slices;

/// Evaluating the dict key precedes the value. A call, property or assignment
/// could change the guarded field through an alias during that interval.
fn preserves_field(value: &Expr) -> bool {
    match &value.kind {
        ExprKind::Local(_) | ExprKind::Int(_) | ExprKind::Bool(_) | ExprKind::Str(_) => true,
        ExprKind::Field(receiver, _) | ExprKind::Unwrap(receiver) => preserves_field(receiver),
        ExprKind::Tuple(items) => items.iter().all(preserves_field),
        ExprKind::Builtin(Builtin::Path(ir::PathOperation::Name) | Builtin::Len, args) => {
            args.iter().all(preserves_field)
        }
        _ => false,
    }
}

pub(super) struct Helper {
    pub module: ModuleId,
    pub function: ir::Function,
    pub calls: HashSet<usize>,
    pub class_reads: HashSet<DefinitionId>,
    /// Local functions and lambdas own frames; implementation helpers run in the caller.
    pub source_frame: bool,
}

/// Source functions reserve the leading IDs; nested displays reserve an ID before
/// checking their bodies, then fill it without disturbing earlier call targets.
pub(super) struct Helpers {
    first: usize,
    // Inference needs stable call edges even when it never emits a helper body.
    reserved: Cell<usize>,
    functions: RefCell<Vec<Option<Helper>>>,
    /// Lifted functions normally leave scope after checking their enclosing body.
    pub(super) local_annotations:
        RefCell<HashMap<usize, (ModuleId, std::rc::Rc<ast::StmtFunctionDef>)>>,
    pub(super) list_builds: RefCell<HashMap<usize, ir::ListBuild>>,
}

impl Helpers {
    pub fn new(first: usize) -> Self {
        Self {
            first,
            reserved: Cell::new(0),
            functions: RefCell::new(Vec::new()),
            local_annotations: RefCell::default(),
            list_builds: RefCell::default(),
        }
    }

    pub(super) fn len(&self) -> usize {
        self.reserved.get()
    }

    pub(super) fn reserve(&self) -> usize {
        let index = self.len();
        self.reserved.set(index + 1);
        self.first + index
    }

    pub(super) fn finish(&self, id: usize, helper: Helper) {
        let mut functions = self.functions.borrow_mut();
        functions.resize_with(self.len(), || None);
        functions[id - self.first] = Some(helper);
    }

    /// Only completed list producers without a frame of their own can move with the caller.
    pub(super) fn caller_list_helpers_since(&self, start: usize, types: &Types) -> bool {
        let helpers = self.functions.borrow();
        start < self.len()
            && helpers.len() == self.len()
            && helpers[start..].iter().all(|helper| {
                helper.as_ref().is_some_and(|helper| {
                    !helper.source_frame && types.element(helper.function.returns).is_some()
                })
            })
    }

    pub(super) fn only_direct_locals_since(
        &self,
        start: usize,
        locals: &HashMap<String, local_functions::Binding>,
    ) -> bool {
        let ids = self.first + start..self.first + self.len();
        ids.is_empty()
            || locals
                .values()
                .filter_map(|local| local.direct_definition())
                .filter(|id| ids.contains(id))
                .count()
                == ids.len()
    }

    pub fn take_functions(&self) -> impl Iterator<Item = Helper> {
        let functions = self.functions.take();
        assert_eq!(functions.len(), self.reserved.replace(0));
        functions
            .into_iter()
            .map(|helper| helper.expect("checked comprehension filled its helper"))
    }
}

/// Frame slots and the Python namespace use the same ordered declaration for each target.
pub(super) fn namespace_targets(plan: &Plan<'_>) -> Vec<String> {
    let mut names = plan.collect_target_names::<Vec<_>>();
    names.sort_unstable();
    names.dedup();
    names
}

/// The creating frame acquires the outer iterator before the target names shadow it.
pub(super) fn acquire_in_namespace(
    input: &mut pipelines::Prepared,
    namespace: usize,
    parent: Option<usize>,
    names: Vec<String>,
) -> Vec<Statement> {
    let mut body = std::mem::take(&mut input.evaluate);
    body.append(&mut input.acquire);
    body.push(Statement::Assign(
        namespace,
        Expr {
            ty: Type::PythonObject,
            kind: ExprKind::Builtin(
                Builtin::RegisteredNamespace(names),
                parent
                    .map(|slot| Expr::local(slot, Type::PythonObject))
                    .into_iter()
                    .collect(),
            ),
        },
    ));
    body
}

/// The result escapes the display. Private slots unwind together, except for
/// assignment expressions that wrote a binding in the surrounding function.
pub(super) fn scoped_result(
    body: Vec<Statement>,
    value: Expr,
    private: std::ops::Range<usize>,
    exported: impl Iterator<Item = usize>,
) -> Expr {
    let exported: HashSet<_> = exported.filter(|slot| private.contains(slot)).collect();
    Expr {
        ty: value.ty,
        kind: ExprKind::Scope {
            body,
            value: Box::new(value),
            clear: private.filter(|slot| !exported.contains(slot)).collect(),
        },
    }
}

/// A consumer can fuse integer addition into an eager list plan.
#[derive(Clone, Copy)]
pub(super) enum Collector {
    Display(Option<Type>),
    IntegerSum,
}

/// The enclosing frame supplies the first iterator. Each target scope checks its filters
/// before opening an inner iterator or checking the result of the last clause.
pub(super) trait ClauseLowering: Sized {
    type Input;
    type Error;
    type Body;

    fn before_pull(&mut self, _plan: &Plan<'_>, _depth: usize) {}
    fn input(&mut self, clause: &ast::Comprehension) -> StdResult<Self::Input, Self::Error>;
    fn with_target(
        &mut self,
        target: &ast::Expr,
        input: Self::Input,
        output: Output<'_>,
        body: impl FnOnce(&mut Self) -> StdResult<(Vec<Expr>, Self::Body), Self::Error>,
    ) -> StdResult<Self::Body, Self::Error>;
    fn filter(&mut self, test: &ast::Expr) -> StdResult<Expr, Self::Error>;

    fn comprehension_loop<'p>(
        &mut self,
        plan: &Plan<'p>,
        depth: usize,
        input: Self::Input,
        emit: &mut impl FnMut(&mut Self, Output<'p>, &[Expr]) -> StdResult<Self::Body, Self::Error>,
    ) -> StdResult<Self::Body, Self::Error> {
        let clause = &plan.clauses[depth];
        self.with_target(&clause.target, input, plan.output, |scope| {
            let conditions = clause
                .ifs
                .iter()
                .map(|test| scope.filter(test))
                .collect::<StdResult<Vec<_>, _>>()?;
            let body = if let Some(inner) = plan.clauses.get(depth + 1) {
                let input = scope.input(inner)?;
                scope.before_pull(plan, depth + 1);
                stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || {
                    scope.comprehension_loop(plan, depth + 1, input, emit)
                })?
            } else {
                emit(scope, plan.output, &conditions)?
            };
            Ok((conditions, body))
        })
    }
}

pub(super) trait EagerDisplay: ClauseLowering<Body = Vec<Statement>> {
    fn collection_guard(_: &[Expr]) -> Option<(usize, usize)> {
        None
    }
    fn collection_key(
        &mut self,
        key: &ast::Expr,
        expected: Option<Type>,
        guarded: Option<(usize, usize)>,
    ) -> StdResult<Expr, Self::Error>;
    fn collection_value(
        &mut self,
        output: Output<'_>,
        collector: Collector,
        key: Option<&Expr>,
        guarded: Option<(usize, usize)>,
    ) -> StdResult<(Expr, Type), Self::Error>;

    /// Seed the accumulator before its first pull and update it only after the filters pass.
    fn comprehension_body(
        &mut self,
        plan: &Plan<'_>,
        input: Self::Input,
        collector: Collector,
        mut destination: Option<(usize, &mut Vec<Statement>)>,
        mut replaces_loop: impl FnMut(&Self, &Expr, Type) -> bool,
    ) -> StdResult<Type, Self::Error> {
        let mut result_type = Type::None;
        let loops = self.comprehension_loop(plan, 0, input, &mut |scope, output, conditions| {
            let guarded = Self::collection_guard(conditions);
            let key = match (collector, output) {
                (Collector::Display(expected), Output::Dict(key, _)) => {
                    Some(scope.collection_key(key, expected, guarded)?)
                }
                _ => None,
            };
            let (item, ty) = scope.collection_value(output, collector, key.as_ref(), guarded)?;
            result_type = ty;
            if replaces_loop(scope, &item, ty) {
                destination = None;
            }
            let Some((result, ref mut body)) = destination else {
                return Ok(Vec::new());
            };
            let (empty, store) = match (collector, output, key) {
                (Collector::Display(_), Output::Dict(..), Some(key)) => (
                    ExprKind::Dict(Vec::new()),
                    Statement::SetItem(ir::ItemOrder::Display, Expr::local(result, ty), key, item),
                ),
                (Collector::Display(_), Output::List(_) | Output::Set(_), None) => {
                    let (empty, operation) = if matches!(output, Output::Set(_)) {
                        let element = if ty == Type::PythonObject {
                            ty
                        } else {
                            item.ty
                        };
                        (
                            ExprKind::Set {
                                element,
                                values: Vec::new(),
                            },
                            Builtin::SetAdd,
                        )
                    } else {
                        (ExprKind::List(Vec::new()), Builtin::Append)
                    };
                    (
                        empty,
                        Statement::Expr(Expr {
                            ty: Type::None,
                            kind: ExprKind::Builtin(operation, vec![Expr::local(result, ty), item]),
                        }),
                    )
                }
                (Collector::IntegerSum, Output::List(_), None) => (
                    ExprKind::Int("0".into()),
                    Statement::Assign(
                        result,
                        Expr {
                            ty: Type::Int,
                            kind: ExprKind::Binary(
                                Binary::Add,
                                Box::new(Expr::local(result, Type::Int)),
                                Box::new(item),
                            ),
                        },
                    ),
                ),
                _ => unreachable!("invalid eager comprehension collector"),
            };
            body.push(Statement::Assign(result, Expr { ty, kind: empty }));
            Ok(vec![store])
        })?;
        if let Some((_, body)) = destination {
            body.extend(loops);
        }
        Ok(result_type)
    }
}

/// A rejected filter skips the remaining predicates and the whole accepted suite,
/// including acquisition of the next clause's iterator.
pub(super) fn filter_suite(conditions: Vec<Expr>, mut accepted: Vec<Statement>) -> Vec<Statement> {
    for condition in conditions.into_iter().rev() {
        accepted = vec![Statement::If(condition, accepted, Vec::new())];
    }
    accepted
}

pub(super) fn async_pull(clause: &ast::Comprehension) -> Option<ir::AsyncPull> {
    clause.is_async.then(|| ir::AsyncPull {
        range: clause.target.range(),
        header: None,
    })
}

impl ClauseLowering for Checker<'_> {
    type Input = Iteration;
    type Error = Failure;
    type Body = Vec<Statement>;

    fn before_pull(&mut self, plan: &Plan<'_>, depth: usize) {
        self.forget_comprehension_writes(plan, depth)
    }

    fn input(&mut self, clause: &ast::Comprehension) -> Result<Iteration, String> {
        self.iteration_input(&clause.iter, None, async_pull(clause))
    }

    fn with_target(
        &mut self,
        target: &ast::Expr,
        input: Iteration,
        _: Output<'_>,
        body: impl FnOnce(&mut Self) -> Result<(Vec<Expr>, Vec<Statement>), String>,
    ) -> Result<Vec<Statement>, String> {
        self.lower_iteration_after(target, None, input, &[], Vec::new(), |checker, output| {
            let (conditions, body) = body(checker)?;
            // A discarded display has checked its filters and item without emitting a store.
            if body.is_empty() {
                return Ok(false);
            }
            statements::append_block(output, filter_suite(conditions, body));
            Ok(true)
        })
        .map(|(body, _)| body)
    }

    fn filter(&mut self, test: &ast::Expr) -> Result<Expr, String> {
        let condition = self.condition(test)?;
        self.apply_facts(test, true);
        Ok(condition)
    }
}

impl EagerDisplay for Checker<'_> {
    fn collection_guard(conditions: &[Expr]) -> Option<(usize, usize)> {
        conditions
            .last()
            .and_then(|test| records::present_field(test, true))
    }

    fn collection_key(
        &mut self,
        key: &ast::Expr,
        expected: Option<Type>,
        guarded: Option<(usize, usize)>,
    ) -> Result<Expr, String> {
        let expected = expected
            .and_then(|ty| self.types.mapping(ty))
            .map(|(key, _)| key);
        let value = self.guarded_element(key, expected, guarded)?;
        self.check_dictionary_key(value.ty, key)?;
        Ok(value)
    }

    fn collection_value(
        &mut self,
        output: Output<'_>,
        collector: Collector,
        key: Option<&Expr>,
        guarded: Option<(usize, usize)>,
    ) -> Result<(Expr, Type), String> {
        let item = output.value();
        Ok(match (collector, key) {
            (Collector::IntegerSum, _) => (
                self.guarded_element(item, Some(Type::Int), guarded)?,
                Type::Int,
            ),
            (Collector::Display(expected), Some(key)) => {
                let expected = expected
                    .and_then(|ty| self.types.mapping(ty))
                    .map(|(_, value)| value);
                let value =
                    self.guarded_element(item, expected, guarded.filter(|_| preserves_field(key)))?;
                let ty = self.types.dict(key.ty, value.ty);
                (value, ty)
            }
            (Collector::Display(expected), None) => {
                let is_set = matches!(output, Output::Set(_));
                let element = expected.and_then(|ty| {
                    if is_set {
                        self.types.set_element(ty)
                    } else {
                        self.types.element(ty)
                    }
                });
                let value = self.guarded_element(item, element, guarded)?;
                let ty = if self.registered.is_some() && value.ty == Type::PythonObject {
                    Type::PythonObject
                } else if is_set {
                    if !self.comprehension_key(value.ty) {
                        return Err(error(
                            self.source,
                            item,
                            "set elements must be int, str, bytes, Path, or immutable scalar tuples",
                        ));
                    }
                    self.types.set(value.ty)
                } else {
                    self.types.list(value.ty)
                };
                (value, ty)
            }
        })
    }
}

/// Writes on the backedge of this loop invalidate earlier narrowing facts.
fn repeated_writes(plan: &Plan<'_>, depth: usize) -> HashSet<String> {
    plan.clauses[depth..]
        .iter()
        .flat_map(|clause| std::iter::once(&clause.target).chain(clause.ifs.iter()))
        .chain(plan.output.elements())
        .flat_map(narrowing::expression_assignments)
        .collect()
}

/// Fixed local functions capture reads outside their own enclosing function bindings.
#[derive(Default)]
struct FunctionReads {
    names: HashSet<String>,
    bound: Vec<HashSet<String>>,
}

pub(super) fn function_reads(function: &ast::StmtFunctionDef) -> HashSet<String> {
    let mut reads = FunctionReads::default();
    reads.visit_function(function);
    reads.names
}

impl FunctionReads {
    fn visit_function(&mut self, function: &ast::StmtFunctionDef) {
        let mut bound = narrowing::assigned_names(&function.body);
        bound.extend(closed_calls::parameters(&function.parameters).map(|p| p.name.to_string()));
        self.bound.push(bound);
        self.visit_body(&function.body);
        self.bound.pop();
    }
}

impl<'a> Visitor<'a> for FunctionReads {
    fn visit_stmt(&mut self, statement: &'a ast::Stmt) {
        if let ast::Stmt::FunctionDef(function) = statement {
            self.visit_function(function);
        } else {
            walk_stmt(self, statement);
        }
    }

    fn visit_expr(&mut self, expression: &'a ast::Expr) {
        self.names.extend(
            function_scope::expression_reads(expression)
                .into_iter()
                .filter(|name| !self.bound.iter().any(|scope| scope.contains(name))),
        );
    }
}

/// Caller-local state shadowed by comprehension iteration targets. Walrus
/// bindings are exported to the enclosing scope when the body is complete.
pub(super) struct ComprehensionScope {
    state: LocalState,
    names: HashMap<String, usize>,
    class_receiver: Option<String>,
    targets: HashSet<String>,
    outer_exports: Option<HashMap<String, usize>>,
}

impl Checker<'_> {
    /// Pure projections need no hashing for up to two elements. A pair uses
    /// ordinary integer equality, including independently allocated huge values.
    pub(super) fn short_set_len(&mut self, source: &ast::Expr, set: &Expr) -> Option<Expr> {
        let ast::Expr::SetComp(display) = source else {
            return None;
        };
        let [clause] = display.generators.as_slice() else {
            return None;
        };
        let ExprKind::Call(_, args) = &set.kind else {
            return None;
        };
        let [input] = args.values.as_slice() else {
            return None;
        };
        let (ast::Expr::Name(source), ExprKind::Local(slot)) = (&clause.iter, &input.kind) else {
            return None;
        };
        if !clause.ifs.is_empty() || self.names.get(source.id.as_str()) != Some(slot) {
            return None;
        }
        let ast::Expr::Name(target) = &clause.target else {
            return None;
        };
        let mut expression = display.elt.as_ref();
        let mut fields = Vec::new();
        while let ast::Expr::Attribute(attribute) = expression {
            fields.push(attribute.attr.as_str());
            expression = &attribute.value;
        }
        if !matches!(expression, ast::Expr::Name(name) if name.id == target.id) {
            return None;
        }
        let element = self.types.element(input.ty)?;
        let mut ty = element;
        let mut path = Vec::new();
        for name in fields.into_iter().rev() {
            let Type::Class(class) = ty else { return None };
            let field = self.classes.declarations.definitions[class]
                .fields
                .iter()
                .position(|field| field.name == name)?;
            ty = self.classes.fields[class][field];
            path.push((field, ty));
        }
        if ty != Type::Int {
            return None;
        }
        let project = |index| {
            let item = pipelines::indexed_item(input.clone(), Expr::integer(index), element, false);
            path.iter().fold(item, |item, &(field, ty)| Expr {
                ty,
                kind: ExprKind::Field(Box::new(item), field),
            })
        };
        let choose = |test, yes: Expr, no| Expr {
            ty: yes.ty,
            kind: ExprKind::Conditional(Box::new(test), Box::new(yes), Box::new(no)),
        };
        let slot = self.hidden(Type::Int);
        let size = self.local(slot);
        let one = Expr::integer(1);
        let two = Expr::integer(2);
        let pair = choose(
            pipelines::compare(Compare::Equal, project(0), project(1)),
            one.clone(),
            two.clone(),
        );
        let larger = choose(
            pipelines::compare(Compare::Equal, size.clone(), two),
            pair,
            pipelines::collection_length(set.clone()),
        );
        Some(Expr {
            ty: Type::Int,
            kind: ExprKind::Scope {
                body: vec![Statement::Assign(
                    slot,
                    pipelines::collection_length(input.clone()),
                )],
                value: Box::new(choose(
                    pipelines::compare(Compare::LessEqual, size.clone(), one),
                    size,
                    larger,
                )),
                clear: vec![slot],
            },
        })
    }

    /// Evaluates the first iterable outside the target scope and passes rooted
    /// values into a private helper. Captures are local bindings, never delayed
    /// field/index reads, so mutation through aliases remains visible in the body.
    ///
    /// ```python
    /// x = [1, 2]
    /// values = [x + 1 for x in x]  # the iterable reads the enclosing x
    /// ```
    pub(super) fn comprehension(
        &mut self,
        expression: &ast::Expr,
        expected: Option<Type>,
    ) -> Result<Expr, String> {
        let display = Plan::from_expr(expression)
            .ok_or_else(|| error(self.source, expression, "unsupported comprehension display"))?;
        self.comprehension_display(expression, &display, Collector::Display(expected))
    }

    pub(super) fn comprehension_display(
        &mut self,
        expression: &ast::Expr,
        display: &Plan<'_>,
        collector: Collector,
    ) -> Result<Expr, String> {
        let mut input = self.input(&display.clauses[0])?;
        // A Python iterator owns its pulls and method dispatch. Only a plain
        // container can supply the exact operand required by a bulk filter.
        if let Collector::Display(expected) = collector
            && let Iteration::Container {
                iterable,
                items: false,
            } = &mut input
        {
            let filtered = match display.output {
                Output::List(_) => self.string_list_filter(
                    display,
                    expected.and_then(|ty| self.types.element(ty)),
                    Some(iterable),
                )?,
                Output::Set(_) => self.character_set_filter(display, expected, iterable)?,
                _ => None,
            };
            if let Some(value) = filtered {
                return Ok(value);
            }
        }
        let prepared = matches!(input, Iteration::Prepared(_));
        if prepared
            // A typed await uses the enclosing coroutine's task and frame.
            || display.body_suspends()
            || pipelines::is_pipeline_expression(self, display.outer)
        {
            let writes = if prepared && self.narrowed.is_empty() {
                HashSet::new()
            } else {
                narrowing::expression_assignments(expression)
            };
            return self.inline_comprehension(display, collector, input, writes);
        }
        // Only the body enters the helper. Its outer iterable was checked in the caller.
        let (targets, captures, writes) = function_scope::display_bindings(display);
        let check_only = self.registered.is_none() && self.discarded_function_body();
        if !writes.is_empty()
            || captures
                .iter()
                .any(|name| self.local_functions.contains_key(name))
            || check_only && captures.iter().all(|name| !self.names.contains_key(name))
        {
            return self.inline_comprehension(
                display,
                collector,
                input,
                writes.into_iter().collect(),
            );
        }
        let id = self.helpers.reserve();
        // Eager displays share the enclosing Python frame, even when lowered to a helper.
        let omitted_locals = self.omitted_assert_locals.clone();
        let omitted_free = self.omitted_assert_free.clone();
        let mut helper = Checker::new(
            self.graph,
            self.module,
            self.functions,
            self.classes,
            self.types,
            self.helpers,
            self.globals,
        );
        helper.frame = LexicalFrame::Positioned(self.fixed_frame());
        helper.omitted_assert_locals = omitted_locals;
        helper.omitted_assert_free = omitted_free;
        helper.inference = self.inference;
        helper.class = self.class;
        helper.class_receiver = self
            .class_receiver
            .clone()
            .filter(|name| !targets.contains(name));
        helper.comprehension_targets = targets;
        let mut params = Vec::new();
        let mut args = Vec::new();
        let mut parameter = |value: Expr, helper: &mut Checker<'_>| {
            // Literal range bounds stay visible to interval analysis. Moving an
            // immutable literal into the helper cannot reorder observable work.
            let mut literal = &value;
            while let ExprKind::Unary(UnaryOp::USub, inner) = &literal.kind {
                literal = inner;
            }
            if matches!(literal.kind, ExprKind::Int(_)) {
                return value;
            }
            let slot = helper.hidden(value.ty);
            helper.assigned.insert(slot);
            params.push(slot);
            args.push(value);
            helper.local(slot)
        };
        let input = match input {
            Iteration::Range(values) => Iteration::Range(
                values
                    .into_iter()
                    .map(|value| parameter(value, &mut helper))
                    .collect(),
            ),
            Iteration::Container { iterable, items } => Iteration::Container {
                iterable: parameter(iterable, &mut helper),
                items,
            },
            Iteration::Prepared(_) => unreachable!("prepared inputs were dispatched inline"),
        };
        for name in captures {
            let Some(&source_slot) = self.names.get(&name) else {
                continue;
            };
            if !self.assigned.contains(&source_slot) {
                return Err(error(
                    self.source,
                    expression,
                    "captured local may be uninitialized on this path",
                ));
            }
            let value = self.read_local(source_slot);
            let slot = helper.declare(&name, value.ty, expression)?;
            helper.assigned.insert(slot);
            params.push(slot);
            args.push(value);
        }
        // The leaf establishes the result type after its target/filter facts are
        // known. No expression can read this private slot before initialization.
        let plain_iterable = match &input {
            Iteration::Container {
                iterable,
                items: false,
            } => Some(iterable),
            _ => None,
        };
        let gather_input = plain_iterable
            .filter(|value| value.ty == Type::Str)
            .cloned();
        let list_binding = if self.inference.is_none()
            && matches!(collector, Collector::Display(_))
            && matches!(display.output, Output::List(_))
            && let [clause] = display.clauses
            && clause.ifs.is_empty()
            && let Some(target) = clause.target.as_name_expr()
            && let Some(Expr {
                ty,
                kind: ExprKind::Local(iterable),
            }) = plain_iterable
            && self.types.element(*ty).is_some()
        {
            Some((*iterable, target.id.as_str()))
        } else {
            None
        };
        let slice = matches!(collector, Collector::IntegerSum)
            && display.clauses.len() == 1
            && args
                .first()
                .is_some_and(|arg| matches!(arg.kind, ExprKind::Builtin(Builtin::Slice, _)));
        let mut body = Vec::new();
        let first_private = helper.locals.len();
        let result =
            (self.inference.is_none() && !check_only || slice).then(|| helper.hidden(Type::None));
        let mut mapping = None;
        let ty = helper.comprehension_body(
            display,
            input,
            collector,
            result.map(|slot| (slot, &mut body)),
            |checker, item, ty| {
                if gather_input.is_some() && ty == Type::ListInt {
                    mapping = checker.dictionary_gather_mapping(display, item);
                }
                mapping.is_some()
            },
        )?;
        if let (Some(result), Some((iterable, target))) = (result, list_binding) {
            self.helpers.list_builds.borrow_mut().insert(
                id,
                ir::ListBuild {
                    iterable,
                    item: helper.names[target],
                    result,
                },
            );
        }
        if let Some(mut input) = gather_input
            && let Some(mapping) = mapping
        {
            // Validate at exactly the original decode's evaluation point, before
            // subsequent captures and before entering the helper. Immutable bytes
            // then keep the gather's borrowed UTF-8 view valid without a Str copy.
            if let ExprKind::Local(slot) = &input.kind
                && params.first() == Some(slot)
                && let Some(argument) = args.first_mut()
                && let ExprKind::Builtin(operation, decoded) = &mut argument.kind
                && matches!(operation, Builtin::Decode)
                && matches!(decoded.as_slice(), [value] if value.ty == Type::Bytes)
            {
                *operation = Builtin::BytesValidateUtf8;
                argument.ty = Type::Bytes;
                helper.locals[*slot] = Type::Bytes;
                input.ty = Type::Bytes;
            }
            let operation = if input.ty == Type::Bytes {
                Builtin::DictGatherBytesInt
            } else {
                Builtin::DictGatherStrInt
            };
            let gather = Expr {
                ty: Type::ListInt,
                kind: ExprKind::Conditional(
                    Box::new(Expr {
                        ty: Type::Bool,
                        kind: ExprKind::Builtin(Builtin::Bool, vec![input.clone()]),
                    }),
                    Box::new(Expr {
                        ty: Type::ListInt,
                        kind: ExprKind::Builtin(operation, vec![input, mapping]),
                    }),
                    Box::new(Expr {
                        ty: Type::ListInt,
                        kind: ExprKind::List(Vec::new()),
                    }),
                ),
            };
            // The gather uses only the incoming iterable and a captured or global mapping.
            helper.locals.truncate(first_private);
            body.push(Statement::Return(gather));
        } else if let Some(result) = result {
            helper.locals[result] = ty;
            body.push(Statement::Return(helper.local(result)));
        }
        if slice {
            slices::fuse(&mut body, &params, &mut args);
        }
        helper.finish_region(Ok(()))?;
        if self.inference.is_none() {
            self.helpers.finish(
                id,
                Helper {
                    module: self.module,
                    function: ir::Function {
                        params,
                        returns: ty,
                        locals: helper.locals,
                        body,
                    },
                    calls: helper.calls,
                    class_reads: helper.class_reads.into_inner(),
                    source_frame: false,
                },
            );
        }
        self.calls.insert(id);
        Ok(Expr {
            ty,
            kind: ExprKind::Call(id, ir::CallArgs::positional(args)),
        })
    }

    /// The fused helper reads the same mapping as the accepted character lookup.
    fn dictionary_gather_mapping(&self, display: &Plan<'_>, item: &Expr) -> Option<Expr> {
        let [clause] = display.clauses else {
            return None;
        };
        let ExprKind::Builtin(Builtin::DictGet, operands) = &item.kind else {
            return None;
        };
        let [mapping, key] = operands.as_slice() else {
            return None;
        };
        let ExprKind::Local(key) = &key.kind else {
            return None;
        };
        (clause.ifs.is_empty()
            && self.names.get(clause.target.as_name_expr()?.id.as_str()) == Some(key)
            && self.types.mapping(mapping.ty) == Some((Type::Str, Type::Int))
            && matches!(
                mapping.kind,
                ExprKind::Local(_) | ExprKind::Builtin(Builtin::GlobalGet(_), _)
            ))
        .then(|| mapping.clone())
    }

    pub(super) fn enter_comprehension(&mut self, targets: Vec<String>) -> ComprehensionScope {
        let scope = ComprehensionScope {
            state: self.local_state(),
            names: self.names.clone(),
            class_receiver: self.class_receiver.clone(),
            targets: self.comprehension_targets.clone(),
            outer_exports: self.comprehension_exports.replace(HashMap::new()),
        };
        for name in targets {
            self.names.remove(&name);
            if self.class_receiver.as_ref() == Some(&name) {
                self.class_receiver = None;
            }
            self.comprehension_targets.insert(name);
        }
        scope
    }

    pub(super) fn leave_comprehension(
        &mut self,
        scope: ComprehensionScope,
    ) -> HashMap<String, usize> {
        let exports = std::mem::replace(&mut self.comprehension_exports, scope.outer_exports)
            .expect("entered comprehension");
        self.comprehension_targets = scope.targets;
        self.names = scope.names;
        self.class_receiver = scope.class_receiver;
        for (name, &slot) in &exports {
            if !self.comprehension_targets.contains(name) {
                self.names.insert(name.clone(), slot);
            }
            if let Some(outer) = &mut self.comprehension_exports {
                outer.insert(name.clone(), slot);
            }
        }
        self.restore_state(scope.state);
        exports
    }

    /// Shares enclosing bindings while reserving private targets and owners.
    /// Exported assignments survive cleanup; all remaining private slots are
    /// cleared on both normal completion and exceptional loop exits.
    fn inline_comprehension(
        &mut self,
        display: &Plan<'_>,
        collector: Collector,
        input: Iteration,
        writes: HashSet<String>,
    ) -> Result<Expr, String> {
        let first_private = self.locals.len();
        let first_pull = matches!(&input, Iteration::Container { iterable, .. } if iterable.is_nonempty_list_literal());
        // Acquire the outer iterator before declaring the comprehension's shadowing names.
        let mut input = input;
        let enclosing = self.namespace;
        let (mut prefix, targets) = if self.registered.is_some() {
            let Iteration::Prepared(cursor) = &mut input else {
                unreachable!("Python iterables have an explicit acquisition")
            };
            let slot = self.hidden(Type::PythonObject);
            let targets = namespace_targets(display);
            let prefix = acquire_in_namespace(cursor, slot, enclosing, targets.clone());
            self.namespace = Some(slot);
            (prefix, targets)
        } else {
            (Vec::new(), display.collect_target_names())
        };
        let scope = self.enter_comprehension(targets);
        let result = self.hidden(Type::None);
        // The outer loop can only invalidate a narrowing when the display writes a name.
        // Displays checked in a private helper have already proved this set empty.
        if !writes.is_empty() {
            self.before_pull(display, 0);
        }
        let checked = self.comprehension_body(
            display,
            input,
            collector,
            (!self.discarded_function_body()).then_some((result, &mut prefix)),
            // A refused fixed region still needs the element checks and result type.
            |cx, _, _| cx.registered.is_none() && cx.unrepresented.borrow().is_some(),
        );
        self.namespace = enclosing;
        let exports = self.leave_comprehension(scope);
        self.locals[result] = checked?;
        if first_pull && !writes.is_empty() {
            // A filter runs before it can suppress the output or the next clause.
            let mut bind = |expression| {
                for name in narrowing::guaranteed_assignments(expression, None) {
                    if let Some(&slot) = exports.get(&name) {
                        self.assigned.insert(slot);
                    }
                }
            };
            let passes = display.clauses[0].ifs.iter().all(|test| {
                bind(test);
                ast::helpers::Truthiness::from_expr(test, |_| false).into_bool() == Some(true)
            });
            if passes && display.clauses.len() == 1 {
                display.output.elements().for_each(bind);
            }
        }
        for name in writes {
            if let Some(slot) = self.names.get(&name) {
                self.narrowed.remove_mut(slot);
            }
        }
        Ok(scoped_result(
            prefix,
            self.local(result),
            first_private..self.locals.len(),
            exports.into_values(),
        ))
    }

    pub(super) fn forget_comprehension_writes(&mut self, plan: &Plan<'_>, depth: usize) {
        if self.narrowed.is_empty() {
            return;
        }
        for name in repeated_writes(plan, depth) {
            if let Some(slot) = self.names.get(&name) {
                self.narrowed.remove_mut(slot);
            }
        }
    }

    pub(super) fn check_dictionary_key(&self, ty: Type, site: &impl Ranged) -> Result<(), String> {
        if self.comprehension_key(ty) {
            Ok(())
        } else {
            let failure = error(
                self.source,
                site,
                "dictionary keys must be int, str, bytes, Path, or immutable scalar tuples",
            );
            // A parsed value can be a Python key even when its native view has no hash layout.
            Err(if matches!(ty, Type::JsonRoot | Type::JsonValue) {
                failure.classify(Kind::UnsupportedRepresentation)
            } else {
                failure
            })
        }
    }

    fn comprehension_key(&self, ty: Type) -> bool {
        self.types.hashable_key(ty) || self.registered.is_some() && ty == Type::PythonObject
    }

    fn guarded_element(
        &mut self,
        expression: &ast::Expr,
        expected: Option<Type>,
        guarded: Option<(usize, usize)>,
    ) -> Result<Expr, String> {
        let depth = self.present_fields.len();
        self.guard_field_read(expression, guarded);
        let value = match expected {
            Some(ty) => self.expr_expected(expression, ty),
            None => self.expr(expression),
        };
        self.present_fields.truncate(depth);
        value
    }
}

#[cfg(test)]
mod tests {
    use crate::ir::{Builtin, ExprKind, Statement, Type, walk_expressions};
    use crate::{CompileOptions, check, compile};

    #[test]
    fn provisional_displays_keep_call_edges_and_slice_rewrites_without_filling_helpers() {
        use super::{
            Checker, ClassInfo, Classes, HashMap, Helpers, ModuleValues, SourceGraph, Types, ast,
            inference,
        };

        let graph = SourceGraph::single(concat!(
            "sum([n + 1 for n in [1, 5] if n])\n",
            "sum([n + 1 for n in [1, 5][::-1]])\n",
            "[n + 1 for n in [1, 5]]\n",
        ))
        .unwrap();
        let module = graph.entry;
        let types = Types::default();
        let mut declarations = Classes::collect(&graph).unwrap();
        let (classes, _) = ClassInfo::new(&graph, &types, &mut declarations).unwrap();
        let signatures = HashMap::new();
        let globals = ModuleValues::default();
        let helpers = Helpers::new(0);
        let inference = inference::Inference::default();
        let mut checker = Checker::new(
            &graph,
            module,
            &signatures,
            &classes,
            &types,
            &helpers,
            &globals,
        );
        checker.inference = Some(&inference);
        for (id, stmt) in graph.modules[module]
            .parsed
            .syntax()
            .body
            .iter()
            .enumerate()
        {
            let ast::Stmt::Expr(stmt) = stmt else {
                unreachable!()
            };
            let value = checker.expr(&stmt.value).unwrap();
            assert_eq!(value.ty, [Type::Int, Type::Int, Type::ListInt][id]);
            let ExprKind::Call(callee, args) = value.kind else {
                panic!("expected a helper call")
            };
            assert_eq!(callee, id);
            assert!(matches!(args.values[0].kind, ExprKind::List(_)));
            assert!(checker.calls.contains(&id));
        }
        assert_eq!(helpers.len(), 3);
        assert_eq!(helpers.functions.borrow().capacity(), 0);
    }

    #[test]
    fn outer_await_acquires_a_synchronous_generator_iterator_in_its_caller() {
        let source = r#"
async def items() -> list[int]:
    return [1, 2]

async def total() -> int:
    return sum(x for x in await items())
"#;
        let graph = crate::modules::SourceGraph::single(source).unwrap();
        super::super::lower(&graph).unwrap();
    }

    #[test]
    fn synchronous_consumer_requires_a_synchronous_generator_protocol() {
        let source = r#"
async def identity(value: int) -> int:
    return value

async def invalid(values: list[int]) -> int:
    return sum(await identity(x) for x in values)
"#;
        let graph = crate::modules::SourceGraph::single(source).unwrap();
        let diagnostic = super::super::lower(&graph)
            .err()
            .expect("async generator requires an async iterator");
        assert!(
            diagnostic.contains("asynchronous generator expressions are unsupported"),
            "{diagnostic}"
        );
    }

    #[test]
    fn unrelated_guard_keeps_context_for_bound_method_inference() {
        let source = "from typing import Callable\nclass Row:\n    tag: str | None\n    def __init__(self) -> None:\n        self.tag = None\n    def apply(self, value):\n        return value\ndef choose(rows: list[Row]) -> list[Callable[[int], int]]:\n    return [r.apply for r in rows if r.tag is not None]\n";
        // Both forms must infer the method before the existing aggregate
        // storage restriction rejects the program.
        for program in [
            source.to_owned(),
            source.replace(" if r.tag is not None", ""),
        ] {
            let (result, lowered) =
                super::super::caller_cache::observe_lowerings(|| check(&program));
            assert_eq!(
                result.unwrap_err(),
                "callable values cannot be stored in containers or tuples"
            );
            assert!(
                !lowered.iter().any(|name| name == "__init__"),
                "{lowered:?}"
            );
        }
    }

    #[test]
    fn character_lookup_fusion_requires_an_exact_pure_comprehension() {
        for (expression, fused) in [
            ("[mapping[c] for c in text]", true),
            ("[mapping[c] for c in text if c]", false),
            ("[mapping[c] for c in text for repeat in range(2)]", false),
            ("[mapping[c] + 1 for c in text]", false),
        ] {
            let source = format!(
                "def gather(mapping: dict[str, int], text: str) -> list[int]:\n    return {expression}\n"
            );
            let llvm = compile(&source, &CompileOptions::default()).unwrap();
            assert_eq!(
                llvm.contains("call ptr @imre_dict_gather_str_int"),
                fused,
                "{expression}"
            );
        }
        let contextual = "def gather(mapping: dict[str, int], text: str) -> list[int | None]:\n    return [mapping[c] for c in text]\n";
        let llvm = compile(contextual, &CompileOptions::default()).unwrap();
        assert!(!llvm.contains("call ptr @imre_dict_gather_str_int"));
    }

    #[test]
    fn decoded_character_fusion_requires_the_exact_decode_input() {
        for (expression, fused) in [
            ("[mapping[c] for c in data.decode()]", true),
            ("[mapping[c] for c in data.decode().rstrip()]", false),
            ("[mapping[c] for c in data.decode() if c]", false),
            ("[mapping[c] + 1 for c in data.decode()]", false),
        ] {
            let source = format!(
                "def gather(mapping: dict[str, int], data: bytes) -> list[int]:\n    return {expression}\n"
            );
            let llvm = compile(&source, &CompileOptions::default()).unwrap();
            assert_eq!(
                llvm.contains("call ptr @imre_bytes_validate_utf8"),
                fused,
                "{expression}"
            );
            assert_eq!(llvm.contains("call ptr @imre_dict_gather_bytes_int"), fused);
            assert_eq!(llvm.contains("call ptr @imre_bytes_decode"), !fused);
        }
    }

    #[test]
    fn an_entry_comprehension_before_a_nonreturning_call_has_one_helper() {
        let graph = crate::modules::SourceGraph::single(
            "def stop(selected: bool) -> None:\n    if selected:\n        raise ValueError('first')\n    else:\n        raise LookupError('second')\ndef emit(value: int) -> None:\n    print(value)\nif __name__ == '__main__':\n    print([str(i) for i in range(4)])\n    stop(True)\n    emit(0)\n",
        )
        .unwrap();
        let module = crate::frontend::lower(&graph).unwrap();
        let helpers = module
            .functions
            .iter()
            .filter(|function| module.types.element(function.returns) == Some(Type::Str));
        assert_eq!(helpers.count(), 1);
    }

    #[test]
    fn decoded_gather_validates_before_calling_the_lazy_global_helper() {
        let directory = tempfile::tempdir().unwrap();
        let entry = directory.path().join("entry.py");
        std::fs::write(
            &entry,
            "from codec import gather\nvalues = gather(b'x')\nprint(len(values))\n",
        )
        .unwrap();
        std::fs::write(
            directory.path().join("codec.py"),
            "mapping: dict[str, int] = {'x': 1}\ndef gather(data: bytes) -> list[int]:\n    return [mapping[c] for c in data.decode()]\n",
        )
        .unwrap();
        let graph = crate::modules::SourceGraph::load(&entry, Some(directory.path()), &[]).unwrap();
        let module = crate::frontend::lower(&graph).unwrap();
        let mut helper = None;
        walk_expressions(&module.functions[0].body, &mut |value| {
            if let ExprKind::Call(id, arguments) = &value.kind {
                assert_eq!(arguments.values.len(), 1);
                assert_eq!(arguments.values[0].ty, Type::Bytes);
                assert!(matches!(
                    arguments.values[0].kind,
                    ExprKind::Builtin(Builtin::BytesValidateUtf8, _)
                ));
                helper = Some(*id);
            }
        });
        let helper = &module.functions[helper.unwrap()];
        assert_eq!(helper.locals.len(), helper.params.len());
        assert_eq!(helper.locals[helper.params[0]], Type::Bytes);
        let [Statement::Return(value)] = helper.body.as_slice() else {
            panic!("the fused helper must contain only its conditional gather")
        };
        let ExprKind::Conditional(condition, nonempty, empty) = &value.kind else {
            panic!("empty input must bypass the mapping load")
        };
        let ExprKind::Builtin(Builtin::Bool, predicate) = &condition.kind else {
            panic!("the helper must test only whether the byte input is empty")
        };
        assert_eq!(predicate[0].ty, Type::Bytes);
        assert!(matches!(predicate[0].kind, ExprKind::Local(slot) if slot == helper.params[0]));
        let ExprKind::Builtin(Builtin::DictGatherBytesInt, arguments) = &nonempty.kind else {
            panic!("the nonempty arm must gather from the validated bytes")
        };
        assert!(matches!(
            arguments[1].kind,
            ExprKind::Builtin(Builtin::GlobalGet(_), _)
        ));
        assert!(matches!(&empty.kind, ExprKind::List(values) if values.is_empty()));
    }

    #[test]
    fn contextual_nested_displays_preserve_private_target_types() {
        let source = r#"
def values() -> list[list[int | None]]:
    item = "outside"
    rows: list[list[int | None]] = [[item for item in range(3)] for row in range(2)]
    print(item)
    empty: list[dict[str, int]] = [{} for item in range(0)]
    return rows
"#;
        compile(source, &CompileOptions::default()).unwrap();
    }

    #[test]
    fn later_targets_are_local_even_before_their_binding() {
        let source = r#"
def values() -> list[int]:
    later = [1]
    return [item for first in [0] for item in later for later in [[2]]]
"#;
        assert!(check(source).is_err());
    }

    #[test]
    fn recursive_comprehension_calls_share_a_stack_growth_entry() {
        let source = r#"
def recur(count: int) -> list[int]:
    return [len(recur(item)) for item in range(count)]
"#;
        let llvm = compile(source, &CompileOptions::default()).unwrap();
        assert_eq!(llvm.matches("call void @imre_on_call_stack(").count(), 1);
    }

    #[test]
    fn comprehension_targets_do_not_invalidate_outer_optional_facts() {
        let source = r#"
def value(item: int | None) -> int:
    assert item is not None
    for turn in range(2):
        values = [item for item in [None]]
        print(item + len(values))
    return item
"#;
        compile(source, &CompileOptions::default()).unwrap();
    }

    #[test]
    fn constructor_self_shadowing_preserves_partial_object_restriction() {
        let accepted = r#"
class Values:
    values: list[int]
    def __init__(self) -> None:
        self.values = [self for self in range(3)]
"#;
        compile(accepted, &CompileOptions::default()).unwrap();
        for value in [
            "[len(self.values) for item in range(3)]",
            "[self for self in self.values]",
        ] {
            let source = accepted.replace("[self for self in range(3)]", value);
            let diagnostic = check(&source).unwrap_err();
            assert!(
                diagnostic.contains("self before construction"),
                "{diagnostic}"
            );
        }
    }

    #[test]
    fn literal_outer_range_bounds_survive_helper_capture() {
        for (arguments, small_cursor) in [
            ("3", true),
            ("3, -4, -2", true),
            ("-4611686018427387905, -4611686018427387902", false),
        ] {
            let source = format!("values = [item for item in range({arguments})]\n");
            let graph = crate::modules::SourceGraph::single(&source).unwrap();
            let module = super::super::lower(&graph).unwrap();
            assert_eq!(module.functions.len(), 1);
            assert!(module.functions[0].params.is_empty());
            let super::Statement::Scope { body, .. } = &module.functions[0].body[1] else {
                panic!("scoped range")
            };
            assert!(matches!(&body[0], super::Statement::For { .. }));
            let ranges = crate::ranges::analyze(&module);
            assert_eq!(ranges.range_cursor(&body[0]).is_some(), small_cursor);
        }
    }
}
