//! Eager displays normally use private functions. Displays containing assignment
//! expressions run inline with private iteration slots so writes reach their
//! enclosing function, including when a later iteration raises.

use std::cell::RefCell;

pub(crate) use imre_ast::comprehension::collect_target_names as target_names;
use imre_ast::comprehension::{Output, Plan, Step};
use ruff_python_ast::visitor::{Visitor, walk_expr, walk_stmt};

use super::*;

mod slices;

/// Only a stored field of the same local can be read again under its guard.
fn local_field(value: &Expr) -> Option<(usize, usize)> {
    if let ExprKind::Field(receiver, field) = &value.kind
        && let ExprKind::Local(local) = receiver.kind
    {
        Some((local, *field))
    } else {
        None
    }
}

fn present_field(condition: &Expr) -> Option<(usize, usize)> {
    if let ExprKind::Not(inner) = &condition.kind
        && let ExprKind::Builtin(Builtin::IsNone, args) = &inner.kind
        && let [value] = args.as_slice()
    {
        local_field(value)
    } else {
        None
    }
}

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
    pub recursion_error: String,
    /// Local functions and lambdas own frames; implementation helpers run in the caller.
    pub source_frame: bool,
}

/// Source functions reserve the leading IDs; nested displays reserve an ID before
/// checking their bodies, then fill it without disturbing earlier call targets.
pub(super) struct Helpers {
    first: usize,
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
            functions: RefCell::new(Vec::new()),
            local_annotations: RefCell::default(),
            list_builds: RefCell::default(),
        }
    }

    pub(super) fn len(&self) -> usize {
        self.functions.borrow().len()
    }

    pub(super) fn reserve(&self) -> usize {
        let mut functions = self.functions.borrow_mut();
        let id = self.first + functions.len();
        functions.push(None);
        id
    }

    pub(super) fn finish(&self, id: usize, helper: Helper) {
        self.functions.borrow_mut()[id - self.first] = Some(helper);
    }

    pub fn take_functions(&self) -> impl Iterator<Item = Helper> {
        self.functions
            .take()
            .into_iter()
            .map(|helper| helper.expect("checked comprehension filled its helper"))
    }
}

/// A consumer can fuse integer addition into an eager list plan.
#[derive(Clone, Copy)]
pub(super) enum Collector {
    Display(Option<Type>),
    IntegerSum,
}

/// Translate the structured plan once, with each frontend supplying its operand
/// and target types. The outer iterator is already acquired when the first target binds.
pub(super) trait EagerDisplay: Sized {
    type Input;
    type Destination: Copy;
    type Error;

    fn before_pull(&mut self, _plan: &Plan<'_>, _depth: usize) {}
    fn input(&mut self, iter: &ast::Expr) -> StdResult<Self::Input, Self::Error>;
    fn with_target(
        &mut self,
        target: &ast::Expr,
        input: Self::Input,
        body: impl FnOnce(&mut Self) -> StdResult<Vec<Statement>, Self::Error>,
    ) -> StdResult<Vec<Statement>, Self::Error>;
    fn filter(&mut self, test: &ast::Expr) -> StdResult<Expr, Self::Error>;
    fn invalid_edges(&self, plan: &Plan<'_>) -> Self::Error;
    fn collect(
        &mut self,
        output: Output<'_>,
        destination: Self::Destination,
        result: usize,
        conditions: &[Expr],
    ) -> StdResult<Vec<Statement>, Self::Error>;

    fn eager_filters(
        &mut self,
        plan: &Plan<'_>,
        pull: usize,
        parent: Option<usize>,
    ) -> StdResult<(usize, Vec<Expr>), Self::Error> {
        let filters = plan
            .loop_filters(pull, parent)
            .ok_or_else(|| self.invalid_edges(plan))?;
        let accepted = filters.end;
        let mut conditions = Vec::with_capacity(filters.len());
        for step in &plan.steps[filters] {
            let Step::Filter { test, .. } = step else {
                unreachable!("validated filters")
            };
            conditions.push(self.filter(test)?);
        }
        Ok((accepted, conditions))
    }

    fn eager_loop(
        &mut self,
        plan: &Plan<'_>,
        pc: usize,
        parent: Option<usize>,
        input: Self::Input,
        destination: Self::Destination,
        result: usize,
    ) -> StdResult<Vec<Statement>, Self::Error> {
        let Step::Next { depth, target, .. } = plan.steps[pc] else {
            unreachable!("an iterator is followed by its pull")
        };
        self.before_pull(plan, depth);
        self.with_target(target, input, |lower| {
            let (next, conditions) = lower.eager_filters(plan, pc, parent)?;
            let mut body = match plan.steps[next] {
                Step::Emit { output, .. } => {
                    lower.collect(output, destination, result, &conditions)?
                }
                Step::Open { iterable, .. } => {
                    let input = lower.input(iterable)?;
                    lower.eager_loop(plan, next + 1, Some(pc), input, destination, result)?
                }
                _ => unreachable!("an accepted input opens an iterator or emits"),
            };
            for condition in conditions.into_iter().rev() {
                body = vec![Statement::If(condition, body, Vec::new())];
            }
            Ok(body)
        })
    }
}

impl EagerDisplay for Checker<'_> {
    type Input = Iteration;
    type Destination = Collector;
    type Error = Failure;

    fn before_pull(&mut self, plan: &Plan<'_>, depth: usize) {
        self.forget_comprehension_writes(plan, depth)
    }

    fn input(&mut self, iter: &ast::Expr) -> Result<Iteration, String> {
        self.iteration_input(iter, None)
    }

    fn with_target(
        &mut self,
        target: &ast::Expr,
        input: Iteration,
        body: impl FnOnce(&mut Self) -> Result<Vec<Statement>, String>,
    ) -> Result<Vec<Statement>, String> {
        self.lower_iteration(target, input, &[], body)
            .map(|(body, _)| body)
    }

    fn filter(&mut self, test: &ast::Expr) -> Result<Expr, String> {
        let condition = self.condition(test)?;
        self.apply_facts(test, true);
        Ok(condition)
    }

    fn invalid_edges(&self, plan: &Plan<'_>) -> Failure {
        error(
            self.source,
            plan.outer,
            "unsupported comprehension loop edges",
        )
    }

    fn collect(
        &mut self,
        output: Output<'_>,
        destination: Collector,
        result: usize,
        conditions: &[Expr],
    ) -> Result<Vec<Statement>, String> {
        self.comprehension_leaf(
            output,
            destination,
            result,
            conditions.last().and_then(present_field),
        )
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

/// Free reads respect nested comprehension scopes: only their leftmost iterable
/// belongs to the surrounding scope; all target names shadow throughout the rest.
struct Reads {
    names: HashSet<String>,
    bound: Vec<HashSet<String>>,
}

/// Finds lexical free reads for capture rejection, including through nested
/// lambdas. Comprehension targets and each lambda's own locals shadow normally.
pub(super) fn free_reads(expression: &ast::Expr, bound: HashSet<String>) -> HashSet<String> {
    let mut reads = Reads {
        names: HashSet::new(),
        bound: vec![bound],
    };
    reads.visit_expr(expression);
    reads.names
}

pub(super) fn function_reads(function: &ast::StmtFunctionDef) -> HashSet<String> {
    let mut reads = Reads {
        names: HashSet::new(),
        bound: Vec::new(),
    };
    reads.visit_function(function);
    reads.names
}

impl Reads {
    fn visit_function(&mut self, function: &ast::StmtFunctionDef) {
        let mut bound = narrowing::assigned_names(&function.body);
        bound.extend(closed_calls::parameters(&function.parameters).map(|p| p.name.to_string()));
        self.bound.push(bound);
        self.visit_body(&function.body);
        self.bound.pop();
    }
}

impl<'a> Visitor<'a> for Reads {
    fn visit_stmt(&mut self, statement: &'a ast::Stmt) {
        if let ast::Stmt::FunctionDef(function) = statement {
            self.visit_function(function);
        } else {
            walk_stmt(self, statement);
        }
    }

    fn visit_expr(&mut self, expression: &'a ast::Expr) {
        if let ast::Expr::Lambda(lambda) = expression {
            let mut bound = narrowing::expression_assignments(&lambda.body);
            if let Some(parameters) = &lambda.parameters {
                for parameter in parameters.iter_non_variadic_params() {
                    if let Some(default) = &parameter.default {
                        self.visit_expr(default);
                    }
                    bound.insert(parameter.name().to_string());
                }
            }
            self.bound.push(bound);
            self.visit_expr(&lambda.body);
            self.bound.pop();
        } else if let Some(plan) = Plan::from_expr(expression) {
            self.visit_expr(plan.outer);
            self.bound.push(plan.target_names());
            plan.visit_body(|expr| self.visit_expr(expr));
            self.bound.pop();
        } else if let ast::Expr::Name(name) = expression {
            if name.ctx == ast::ExprContext::Load
                && !self
                    .bound
                    .iter()
                    .any(|scope| scope.contains(name.id.as_str()))
            {
                self.names.insert(name.id.to_string());
            }
        } else {
            walk_expr(self, expression);
        }
    }
}

/// Caller-local state shadowed by comprehension iteration targets. Walrus
/// bindings are exported to the enclosing scope when the body is complete.
pub(super) struct ComprehensionScope {
    state: LocalState,
    names: HashMap<String, usize>,
    class_receiver: Option<String>,
    targets: HashSet<String>,
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
        let length = |value| Expr {
            ty: Type::Int,
            kind: ExprKind::Builtin(Builtin::Len, vec![value]),
        };
        let project = |index| {
            let item = Expr {
                ty: element,
                kind: ExprKind::Builtin(
                    if matches!(input.ty, Type::List(_)) {
                        Builtin::SequenceGetInBounds
                    } else {
                        Builtin::ListGet
                    },
                    vec![input.clone(), Expr::integer(index)],
                ),
            };
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
            length(set.clone()),
        );
        Some(Expr {
            ty: Type::Int,
            kind: ExprKind::Scope {
                body: vec![Statement::Assign(slot, length(input.clone()))],
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
        self.lower_display(expression, &display, expected)
    }

    pub(super) fn lower_display(
        &mut self,
        expression: &ast::Expr,
        display: &Plan<'_>,
        expected: Option<Type>,
    ) -> Result<Expr, String> {
        if self.registered.is_some() {
            return self.comprehension_display(expression, display, Collector::Display(expected));
        }
        let filtered = match display.output {
            Output::Set(_) => self.character_set_filter(display, expected)?,
            Output::List(_) => {
                self.string_list_filter(display, expected.and_then(|ty| self.types.element(ty)))?
            }
            _ => None,
        };
        if let Some(filtered) = filtered {
            return Ok(filtered);
        }
        self.comprehension_display(expression, display, Collector::Display(expected))
    }

    /// A dict can consume an exact two-column zip in the same order as a
    /// dictionary comprehension; the iterator itself never escapes.
    pub(super) fn dict_from_zip(&mut self, call: &ast::ExprCall) -> Result<Option<Expr>, String> {
        let [source @ ast::Expr::Call(zip)] = call.arguments.args.as_ref() else {
            return Ok(None);
        };
        if !call.arguments.keywords.is_empty()
            || zip.arguments.args.len() != 2
            || !matches!(&*zip.func,ast::Expr::Name(n) if n.id == "zip" && !self.names.contains_key("zip"))
            || self.resolve(&zip.func).is_some()
        {
            return Ok(None);
        }
        let range = call.range();
        let node_index = || ast::AtomicNodeIndex::NONE;
        let item = |name: &str, ctx| {
            ast::Expr::Name(ast::ExprName {
                range,
                node_index: node_index(),
                id: name.into(),
                ctx,
            })
        };
        let target = ast::Expr::Tuple(ast::ExprTuple {
            range,
            node_index: node_index(),
            parenthesized: false,
            ctx: ast::ExprContext::Store,
            elts: vec![
                item("@zipkey", ast::ExprContext::Store),
                item("@zipvalue", ast::ExprContext::Store),
            ],
        });
        let clauses = [ast::Comprehension {
            range,
            node_index: node_index(),
            target,
            iter: source.clone(),
            ifs: Vec::new(),
            is_async: false,
        }];
        let key = item("@zipkey", ast::ExprContext::Load);
        let value = item("@zipvalue", ast::ExprContext::Load);
        self.comprehension_display(
            &call.func,
            &Plan::new(&clauses, Output::Dict(&key, &value)),
            Collector::Display(None),
        )
        .map(Some)
    }

    /// Reduces an immediately consumed integer comprehension in its original
    /// eager order. Integer addition has no callbacks or recoverable failures;
    /// the private list and its element owners cannot otherwise be observed.
    pub(super) fn sum_comprehension(&mut self, expression: &ast::Expr) -> Result<Expr, String> {
        let ast::Expr::ListComp(value) = expression else {
            unreachable!("sum selected a list comprehension");
        };
        self.comprehension_display(
            expression,
            &Plan::new(&value.generators, Output::List(&value.elt)),
            Collector::IntegerSum,
        )
    }

    pub(super) fn check_comprehension_inputs(
        &self,
        plan: &Plan<'_>,
        node: &impl Ranged,
        async_error: &str,
    ) -> Result<(), String> {
        let requires_async_iterator = match plan.output {
            // A single await in a deferred expression gives it the async
            // generator protocol even if every source is synchronous.
            Output::Generator(_) => plan.body_suspends(),
            _ => plan
                .steps
                .iter()
                .any(|step| matches!(step, Step::Next { is_async: true, .. })),
        };
        if requires_async_iterator {
            return Err(error(self.source, node, async_error));
        }
        if plan
            .clauses
            .iter()
            .any(|clause| !narrowing::expression_assignments(&clause.iter).is_empty())
        {
            return Err(error(
                self.source,
                node,
                "assignment expressions are forbidden in comprehension iterables",
            ));
        }
        Ok(())
    }

    fn comprehension_display(
        &mut self,
        expression: &ast::Expr,
        display: &Plan<'_>,
        collector: Collector,
    ) -> Result<Expr, String> {
        self.check_comprehension_inputs(
            display,
            expression,
            "asynchronous comprehensions are unsupported",
        )?;
        let input = self.iteration_input(display.outer, None)?;
        if matches!(input, Iteration::Prepared(_))
            // A typed await uses the enclosing coroutine's task and frame.
            || display.body_suspends()
            || !narrowing::expression_assignments(expression).is_empty()
            || pipelines::is_pipeline_expression(self, display.outer)
            || !self.local_functions.is_empty()
                && free_reads(expression, HashSet::new())
                    .iter()
                    .any(|name| self.local_functions.contains_key(name))
        {
            return self.inline_comprehension(expression, display, collector, input);
        }
        let targets = display.target_names();
        let mut reads = Reads {
            names: HashSet::new(),
            bound: vec![targets.clone()],
        };
        display.visit_body(|expr| reads.visit_expr(expr));
        let mut captures = reads.names.into_iter().collect::<Vec<_>>();
        captures.sort();
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
            while let ExprKind::Negate(inner) = &literal.kind {
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
            Iteration::Container {
                iterable,
                items,
                start,
            } => Iteration::Container {
                iterable: parameter(iterable, &mut helper),
                items,
                start: start.map(|start| parameter(start, &mut helper)),
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
                start: None,
            } => Some(iterable),
            _ => None,
        };
        let gather_input = plain_iterable
            .filter(|value| value.ty == Type::Str)
            .cloned();
        let result = helper.hidden(Type::None);
        let list_binding = if matches!(collector, Collector::Display(_))
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
        let (ty, mut body) = helper.comprehension_body(display, input, collector, result)?;
        if let Some((iterable, target)) = list_binding {
            self.helpers.list_builds.borrow_mut().insert(
                id,
                ir::ListBuild {
                    iterable,
                    item: helper.names[target],
                    result,
                },
            );
        }
        body.push(Statement::Return(helper.local(result)));
        if ty == Type::ListInt
            && let Some(mut input) = gather_input
            && let Some(mapping) = helper.dictionary_gather_mapping(display)?
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
            body = vec![Statement::Return(gather)];
        }
        if matches!(collector, Collector::IntegerSum) && display.clauses.len() == 1 {
            slices::fuse(&mut body, &params, &mut args);
        }
        helper.finish_region(Ok(()))?;
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
                recursion_error: self.graph.modules[self.module].error(
                    expression,
                    "recursive calls are unsupported in this prototype",
                ),
            },
        );
        self.calls.insert(id);
        Ok(Expr {
            ty,
            kind: ExprKind::Call(id, ir::CallArgs::positional(args)),
        })
    }

    /// Finds the mapping in a checked, callback-free character comprehension.
    /// Ordinary captures and type checks remain intact; the replacement skips
    /// this mapping load for an empty source, just as the original loop does.
    fn dictionary_gather_mapping(&mut self, display: &Plan<'_>) -> Result<Option<Expr>, String> {
        let [clause] = display.clauses else {
            return Ok(None);
        };
        if !clause.ifs.is_empty() {
            return Ok(None);
        }
        let ast::Expr::Name(target) = &clause.target else {
            return Ok(None);
        };
        let Output::List(ast::Expr::Subscript(subscript)) = display.output else {
            return Ok(None);
        };
        let ast::Expr::Name(key) = &*subscript.slice else {
            return Ok(None);
        };
        let ast::Expr::Name(mapping) = &*subscript.value else {
            return Ok(None);
        };
        if key.id != target.id || mapping.id == target.id {
            return Ok(None);
        }
        let mapping = self.expr(&subscript.value)?;
        if self.types.mapping(mapping.ty) != Some((Type::Str, Type::Int))
            || !matches!(
                mapping.kind,
                ExprKind::Local(_) | ExprKind::Builtin(Builtin::GlobalGet(_), _)
            )
        {
            return Ok(None);
        }
        Ok(Some(mapping))
    }

    pub(super) fn enter_comprehension(&mut self, targets: HashSet<String>) -> ComprehensionScope {
        let scope = ComprehensionScope {
            state: self.local_state(),
            names: self.names.clone(),
            class_receiver: self.class_receiver.clone(),
            targets: self.comprehension_targets.clone(),
        };
        for name in &targets {
            self.names.remove(name);
        }
        if self
            .class_receiver
            .as_ref()
            .is_some_and(|name| targets.contains(name))
        {
            self.class_receiver = None;
        }
        self.comprehension_targets.extend(targets);
        self.comprehension_depth += 1;
        scope
    }

    pub(super) fn leave_comprehension(&mut self, scope: ComprehensionScope) {
        self.comprehension_depth -= 1;
        self.comprehension_targets = scope.targets;
        self.names = scope.names;
        self.class_receiver = scope.class_receiver;
        for (name, &slot) in &self.comprehension_exports {
            if !self.comprehension_targets.contains(name) {
                self.names.insert(name.clone(), slot);
            }
        }
        self.restore_state(scope.state);
    }

    /// Shares enclosing bindings while reserving private targets and owners.
    /// Exported assignments survive cleanup; all remaining private slots are
    /// cleared on both normal completion and exceptional loop exits.
    fn inline_comprehension(
        &mut self,
        expression: &ast::Expr,
        display: &Plan<'_>,
        collector: Collector,
        input: Iteration,
    ) -> Result<Expr, String> {
        let first_private = self.locals.len();
        let writes = narrowing::expression_assignments(expression);
        // Acquire the outer iterator before declaring the comprehension's shadowing names.
        let mut input = input;
        let mut prefix = Vec::new();
        let enclosing = self.namespace;
        let targets = display.target_names();
        if self.registered.is_some() {
            let Iteration::Prepared(cursor) = &mut input else {
                unreachable!("Python iterables have an explicit acquisition")
            };
            prefix.append(&mut cursor.evaluate);
            prefix.append(&mut cursor.acquire);
            let slot = self.hidden(Type::PythonObject);
            let mut declared: Vec<_> = targets.iter().cloned().collect();
            declared.sort();
            prefix.push(Statement::Assign(
                slot,
                Expr {
                    ty: Type::PythonObject,
                    kind: ExprKind::Builtin(
                        Builtin::RegisteredNamespace(declared),
                        enclosing
                            .map(|parent| self.local(parent))
                            .into_iter()
                            .collect(),
                    ),
                },
            ));
            self.namespace = Some(slot);
        }
        let scope = self.enter_comprehension(targets.clone());
        let result = self.hidden(Type::None);
        let body = self.comprehension_body(display, input, collector, result);
        if let Some(namespace) = self.namespace {
            for name in &targets {
                if let Some(&slot) = self.names.get(name) {
                    self.namespace_bindings.insert(
                        slot,
                        registered::LocalBinding {
                            name: name.clone(),
                            namespace: Some(namespace),
                        },
                    );
                }
            }
        }
        self.namespace = enclosing;
        self.leave_comprehension(scope);
        let (ty, mut body) = body?;
        if !prefix.is_empty() {
            prefix.append(&mut body);
            body = prefix;
        }
        for name in writes {
            if let Some(slot) = self.names.get(&name) {
                self.narrowed.remove_mut(slot);
            }
        }
        let exported = self
            .comprehension_exports
            .values()
            .copied()
            .collect::<HashSet<_>>();
        let clear = (first_private..self.locals.len())
            .filter(|slot| !exported.contains(slot))
            .collect();
        Ok(Expr {
            ty,
            kind: ExprKind::Scope {
                body,
                value: Box::new(self.local(result)),
                clear,
            },
        })
    }

    /// Lower the loops first so their leaf determines the type of the initial value.
    fn comprehension_body(
        &mut self,
        display: &Plan<'_>,
        input: Iteration,
        collector: Collector,
        result: usize,
    ) -> Result<(Type, Vec<Statement>), String> {
        let loops = self.eager_loop(display, 0, None, input, collector, result)?;
        let ty = self.locals[result];
        let empty = match (collector, display.output) {
            (Collector::Display(_), Output::List(_)) => ExprKind::List(Vec::new()),
            (Collector::IntegerSum, Output::List(_)) => ExprKind::Int("0".into()),
            (Collector::Display(_), Output::Set(_)) => ExprKind::Set {
                element: self.types.set_element(ty).unwrap(),
                values: Vec::new(),
            },
            (Collector::Display(_), Output::Dict(..)) => {
                let (key, value) = self.types.mapping(ty).unwrap();
                ExprKind::Dict {
                    key,
                    value,
                    entries: Vec::new(),
                }
            }
            _ => unreachable!("invalid eager comprehension collector"),
        };
        let mut body = vec![Statement::Assign(result, Expr { ty, kind: empty })];
        body.extend(loops);
        Ok((ty, body))
    }

    pub(super) fn forget_comprehension_writes(&mut self, plan: &Plan<'_>, depth: usize) {
        for name in repeated_writes(plan, depth) {
            if let Some(slot) = self.names.get(&name) {
                self.narrowed.remove_mut(slot);
            }
        }
    }

    pub(super) fn comprehension_filters(
        &mut self,
        plan: &Plan<'_>,
        pull: usize,
        parent: Option<usize>,
    ) -> Result<(usize, Vec<Expr>), String> {
        self.eager_filters(plan, pull, parent)
    }

    /// A dict display evaluates and roots its key before its value. Ordinary
    /// subscript assignment cannot express that order because it evaluates RHS first.
    fn comprehension_leaf(
        &mut self,
        value: Output<'_>,
        collector: Collector,
        result: usize,
        guarded: Option<(usize, usize)>,
    ) -> Result<Vec<Statement>, String> {
        match (collector, value) {
            (Collector::IntegerSum, Output::List(expression)) => {
                let value = self.guarded_element(expression, Some(Type::Int), guarded)?;
                self.locals[result] = Type::Int;
                Ok(vec![Statement::Assign(
                    result,
                    Expr {
                        ty: Type::Int,
                        kind: ExprKind::Binary(
                            Binary::Add,
                            Box::new(self.local(result)),
                            Box::new(value),
                        ),
                    },
                )])
            }
            (Collector::Display(expected), Output::List(expression) | Output::Set(expression)) => {
                let is_set = matches!(value, Output::Set(_));
                let element = expected.and_then(|ty| {
                    if is_set {
                        self.types.set_element(ty)
                    } else {
                        self.types.element(ty)
                    }
                });
                let value = self.guarded_element(expression, element, guarded)?;
                if is_set && !self.comprehension_key(value.ty) {
                    return Err(error(
                        self.source,
                        expression,
                        "set elements must be int, str, bytes, Path, or immutable scalar tuples",
                    ));
                }
                self.locals[result] = if is_set {
                    self.types.set(value.ty)
                } else {
                    self.types.list(value.ty)
                };
                Ok(vec![Statement::Expr(Expr {
                    ty: Type::None,
                    kind: ExprKind::Builtin(
                        if is_set {
                            Builtin::SetAdd
                        } else {
                            Builtin::Append
                        },
                        vec![self.local(result), value],
                    ),
                })])
            }
            (Collector::Display(expected), Output::Dict(key, value)) => {
                let expected = expected.and_then(|ty| self.types.mapping(ty));
                let key_value = self.guarded_element(key, expected.map(|(ty, _)| ty), guarded)?;
                if !self.comprehension_key(key_value.ty) {
                    return Err(error(
                        self.source,
                        key,
                        "dictionary keys must be int, str, bytes, Path, or immutable scalar tuples",
                    ));
                }
                let captured = self.hidden(key_value.ty);
                let guarded = guarded.filter(|_| preserves_field(&key_value));
                let value = self.guarded_element(value, expected.map(|(_, ty)| ty), guarded)?;
                self.locals[result] = self.types.dict(key_value.ty, value.ty);
                Ok(vec![Statement::Scope {
                    body: vec![
                        Statement::Assign(captured, key_value),
                        Statement::SetItem(self.local(result), self.local(captured), value),
                    ],
                    clear: vec![captured],
                }])
            }
            _ => unreachable!("invalid eager comprehension collector"),
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
        if let Some((slot, field)) = guarded
            && let ast::Expr::Attribute(attribute) = expression
            && let ast::Expr::Name(receiver) = &*attribute.value
            && self.names.get(receiver.id.as_str()) == Some(&slot)
            && let Type::Class(class) = self.locals[slot]
            && self.classes.declarations.definitions[class].fields[field].name
                == attribute.attr.as_str()
            && let Some(payload) = self.types.payload(self.classes.fields[class][field])
        {
            let value = self.expr(expression)?;
            let unwrapped = Expr {
                ty: payload,
                kind: ExprKind::Unwrap(Box::new(value)),
            };
            return match expected {
                Some(ty) => self.coerce(unwrapped, ty, expression),
                None => Ok(unwrapped),
            };
        }
        match expected {
            Some(ty) => self.expr_expected(expression, ty),
            None => self.expr(expression),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::ir::{Builtin, ExprKind, Statement, Type, walk_expressions};
    use crate::{CompileOptions, check, compile};

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
            assert_eq!(
                check(&program).unwrap_err(),
                "callable values cannot be stored in containers or tuples"
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
    fn helper_calls_participate_in_recursion_validation() {
        let source = r#"
def recur(count: int) -> list[int]:
    return [len(recur(item)) for item in range(count)]
"#;
        let diagnostic = check(source).unwrap_err();
        assert!(diagnostic.contains("recursive calls"), "{diagnostic}");
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
