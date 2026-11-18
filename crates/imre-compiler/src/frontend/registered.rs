//! Lower functions whose live Python arguments are supplied by the registered binder.

use super::*;

pub(super) mod booleans;
mod eager;
mod generators;
pub(crate) use generators::{LexicalOperands, RegisteredGenerator};
mod lambdas;
pub(crate) use lambdas::RegisteredLambda;
pub(super) mod patterns;
#[cfg(test)]
mod placement;
mod templates;
pub(super) mod values;

/// Prepare the bindings installed by the existing type-parameter frame constructor.
fn registered_type_parameters(
    params: &ast::TypeParams,
    private_names: &imre_ast::Names,
    source: values::RegisteredSource<'_>,
) -> Vec<ir::RegisteredTypeParameter<ParameterEvaluator>> {
    params
        .iter()
        .map(|param| {
            let kind = match param {
                ast::TypeParam::TypeVar(_) => ir::TypeParameterKind::TypeVar,
                ast::TypeParam::TypeVarTuple(_) => ir::TypeParameterKind::TypeVarTuple,
                ast::TypeParam::ParamSpec(_) => ir::TypeParameterKind::ParamSpec,
            };
            let binding = param.name();
            ir::RegisteredTypeParameter {
                binding: binding.to_string(),
                name: private_names.definition(binding).to_owned(),
                kind,
                fields: imre_ast::TypeParameterField::expressions(param)
                    .map(|(field, rhs)| (field, ParameterEvaluator::prepare(rhs, source)))
                    .collect(),
            }
        })
        .collect()
}

/// An expression can own private slots without introducing another Python function.
pub(crate) struct RegisteredOperand {
    pub(crate) value: Expr,
    pub(crate) inputs: Box<[PhysicalOperand]>,
    pub(crate) frame_slots: FrameSlots,
    #[cfg(any(debug_assertions, test, feature = "verify-ir"))]
    locals: Vec<Type>,
    pub(crate) local_names: HashMap<usize, LocalBinding>,
}

#[derive(Clone, Copy)]
enum RegionRoot<'a, Suite = &'a [Statement]> {
    Suite(Suite),
    Operand(&'a Expr),
}

#[derive(Clone, Copy)]
pub(crate) struct RegisteredRegion<'a> {
    #[cfg(any(debug_assertions, test, feature = "verify-ir"))]
    pub(crate) locals: &'a [Type],
    #[cfg(any(debug_assertions, test, feature = "verify-ir"))]
    pub(crate) params: &'a [usize],
    pub(crate) local_names: &'a HashMap<usize, LocalBinding>,
    pub(crate) frame_slots: &'a FrameSlots,
    root: RegionRoot<'a, &'a RegisteredFunction>,
}

impl<'a> RegisteredRegion<'a> {
    #[cfg(any(debug_assertions, test, feature = "verify-ir"))]
    pub(crate) fn is_registered_invoke(self, sites: &[TextRange], value: &Expr) -> bool {
        if let (RegionRoot::Suite(frame), [range, tail @ ..]) = (self.root, sites)
            && let Some(attr) = frame.fixed_attribute(*range)
            && attr.call_binding().is_some()
        {
            return attr
                .call_operands(*range, tail.first().copied(), value)
                .is_some();
        }
        value.is_registered_invoke()
    }

    #[cfg(test)]
    pub(crate) fn walk(self, visit: &mut impl FnMut(ir::Node<'a>, Option<&'a Expr>)) {
        self.frame_layout(|node, parent, _| {
            visit(node, parent);
            true
        })
        .expect("complete registered source");
    }

    #[cfg(test)]
    pub(crate) fn private_slots(self) -> Vec<usize> {
        self.frame_layout(|_, _, _| true).unwrap().slots().to_vec()
    }

    /// Collect private slots while visiting each node. Positioned nodes receive the remaining
    /// physical sites, starting with their own and then their first positioned operand.
    pub(crate) fn frame_layout(
        self,
        mut supported: impl FnMut(ir::Node<'a>, Option<&'a Expr>, &[TextRange]) -> bool,
    ) -> Option<FrameSlots> {
        let mut slots = std::collections::BTreeSet::new();
        let mut cursors = HashMap::new();
        let mut visit = |node, parent, positions| {
            match &node {
                ir::Node::Statement(Statement::ContainerFor(input)) => {
                    slots.extend(input.slots);
                    cursors.insert(input.slots[0], ir::IterNextKind::Generic);
                }
                ir::Node::Statement(Statement::Scope { clear, .. })
                | ir::Node::Expression(Expr {
                    kind: ExprKind::Scope { clear, .. },
                    ..
                }) => {
                    slots.extend(clear);
                }
                ir::Node::Statement(Statement::IterNext { iterator, kind, .. }) => {
                    // Unpacking a row changes its consumer, not the acquired cursor.
                    let storage = match kind {
                        ir::IterNextKind::Pair(_) => ir::IterNextKind::Generic,
                        kind => *kind,
                    };
                    cursors.insert(*iterator, storage);
                }
                _ => {}
            }
            supported(node, parent, positions)
        };
        let mut unplaced = |node, parent| visit(node, parent, &[]);
        let complete = match self.root {
            RegionRoot::Operand(value) => {
                ir::all_operand_nodes(value, None, ir::NodeOperands::Source, &mut unplaced)
            }
            RegionRoot::Suite(frame) => {
                // Explicit declaration bindings precede the placed suffix and own no fixed sites.
                let (prefix, tail) = frame.function.body.split_at_checked(frame.placed_from)?;
                let mut sites = frame.fixed_sites.iter();
                ir::all_nodes(prefix, ir::NodeOperands::Source, &mut unplaced)
                    && ir::all_nodes(tail, ir::NodeOperands::Source, &mut |node, parent| {
                        if frame.fixed_sites.is_empty() || !node.has_fixed_site_in(parent) {
                            return visit(node, parent, &[]);
                        }
                        let current = sites.as_slice();
                        sites.next().is_some() && visit(node, parent, current)
                    })
                    && sites.len() == 0
            }
        };
        if !complete {
            return None;
        }
        slots.retain(|slot| !self.local_names.contains_key(slot));
        Some(FrameSlots::new(slots.into_iter().collect(), cursors))
    }
}

impl RegisteredOperand {
    /// Bind an operation whose scoped children already own their private locals.
    fn with_inputs(value: Expr, inputs: Box<[PhysicalOperand]>) -> Self {
        Self {
            value,
            inputs,
            frame_slots: FrameSlots::default(),
            #[cfg(any(debug_assertions, test, feature = "verify-ir"))]
            locals: Vec::new(),
            local_names: HashMap::new(),
        }
    }

    /// Lower an inline value with its lexical bindings and compiler-owned slots.
    pub(crate) fn prepare(
        expr: &ast::Expr,
        source: values::RegisteredSource<'_>,
        kind: RegisteredScope,
    ) -> Option<Self> {
        Self::prepare_with(source, kind, |scope| scope.operand(expr))
    }

    fn prepare_with(
        source: values::RegisteredSource<'_>,
        kind: RegisteredScope,
        lower: impl FnOnce(&mut SourceScope<'_>) -> Option<Expr>,
    ) -> Option<Self> {
        let mut scope = SourceScope::new(kind, source, None, false);
        let value = lower(&mut scope)?;
        Some(Self::from_scope(value, scope))
    }

    /// Prepare a required value once and keep the diagnostic chosen by its lexical scope.
    fn prepare_required(
        expr: &ast::Expr,
        source: values::RegisteredSource<'_>,
        kind: RegisteredScope,
        reason: &'static str,
        configure: impl FnOnce(&mut SourceScope<'_>),
    ) -> StdResult<Self, (TextRange, &'static str)> {
        let mut refusal = None;
        Self::prepare_with(source, kind, |scope| {
            configure(scope);
            let value = scope.operand(expr);
            refusal = scope.refusal;
            value
        })
        .ok_or_else(|| refusal.unwrap_or((expr.range(), reason)))
    }

    fn from_scope(value: Expr, scope: SourceScope<'_>) -> Self {
        let bindings = scope.bindings();
        Self {
            value,
            frame_slots: bindings.slots,
            #[cfg(any(debug_assertions, test, feature = "verify-ir"))]
            locals: bindings.locals,
            inputs: Box::default(),
            local_names: bindings.names,
        }
    }

    pub(crate) fn region(&self) -> RegisteredRegion<'_> {
        RegisteredRegion {
            #[cfg(any(debug_assertions, test, feature = "verify-ir"))]
            locals: &self.locals,
            #[cfg(any(debug_assertions, test, feature = "verify-ir"))]
            params: &[],
            local_names: &self.local_names,
            frame_slots: &self.frame_slots,
            root: RegionRoot::Operand(&self.value),
        }
    }
}

pub(crate) enum FrameSite {
    Method(crate::classes::MethodSource),
    Lambda(TextSize),
    Generator(TextRange),
}

/// How a body will receive its arguments. A pending source check needs both contracts.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum FrameArguments {
    Checked,
    Pending,
    Python,
}

/// Argument binding and lexical dependencies of a retained Python frame.
pub(crate) enum FrameInputs {
    Checked {
        sites: Box<[FrameSite]>,
    },
    /// A solved body keeps its checked binder until its source contract settles.
    Deferred {
        sites: Box<[FrameSite]>,
        live: bool,
    },
    /// Scalar parameters reach Python call operands without native coercion.
    Protocol {
        sites: Box<[FrameSite]>,
    },
}

impl Default for FrameInputs {
    fn default() -> Self {
        Self::Checked {
            sites: Box::default(),
        }
    }
}

/// The logical slots and cursor operations selected by this physical region.
#[derive(Clone, Default)]
pub(crate) struct FrameSlots(Option<Arc<PrivateSlots>>);

struct PrivateSlots {
    slots: Box<[usize]>,
    cursors: HashMap<usize, ir::IterNextKind>,
}

impl FrameSlots {
    pub(crate) fn new(slots: Box<[usize]>, cursors: HashMap<usize, ir::IterNextKind>) -> Self {
        Self(
            (!slots.is_empty() || !cursors.is_empty())
                .then(|| Arc::new(PrivateSlots { slots, cursors })),
        )
    }

    /// All unnamed locals of a prepared source scope belong to the activation that allocated them.
    fn from_locals(
        count: usize,
        names: &HashMap<usize, LocalBinding>,
        cursors: HashMap<usize, ir::IterNextKind>,
    ) -> Self {
        if count == names.len() && cursors.is_empty() {
            return Self::default();
        }
        Self::new(
            (0..count)
                .filter(|slot| !names.contains_key(slot))
                .collect(),
            cursors,
        )
    }

    pub(crate) fn slots(&self) -> &[usize] {
        self.0.as_ref().map_or(&[], |layout| &*layout.slots)
    }

    pub(crate) fn index(&self, slot: &usize) -> Option<usize> {
        self.slots().binary_search(slot).ok()
    }

    pub(crate) fn cursor(&self, slot: &usize) -> Option<ir::IterNextKind> {
        self.0.as_ref()?.cursors.get(slot).copied()
    }
}

pub(crate) struct RegisteredFunction {
    /// Initial statements with explicit Python locations before the fixed sites begin.
    pub(crate) placed_from: usize,
    pub(crate) inputs: FrameInputs,
    pub(crate) frame_slots: FrameSlots,
    /// Physical positions for the unchanged fixed operations, in checked node order.
    pub(crate) fixed_sites: Box<[TextRange]>,
    /// Descriptor names and token positions for placed fixed-layout field reads.
    #[expect(
        clippy::box_collection,
        reason = "keep the optional field map one word per registered function"
    )]
    pub(crate) fixed_attributes: Option<Box<HashMap<TextRange, FixedAttribute>>>,
    pub(crate) function: ir::Function,
    pub(crate) local_names: HashMap<usize, LocalBinding>,
}

impl RegisteredFunction {
    pub(crate) fn fixed_attribute(&self, range: TextRange) -> Option<&FixedAttribute> {
        self.fixed_attributes.as_ref()?.get(&range)
    }

    fn owns_lookup_errors(
        &self,
        graph: &SourceGraph,
        module: ModuleId,
        definition: TextRange,
    ) -> bool {
        let errors = graph.registered_source_errors.borrow();
        errors.get(&module).is_some_and(|errors| {
            crate::analysis::RegisteredSourceError::only_attributes(errors, definition, |range| {
                self.fixed_sites.iter().any(|site| {
                    matches!(self.fixed_attribute(*site),
                        Some(FixedAttribute::Call(None, call))
                            if call.checked_nullable_receiver && matches!(call.target, ir::RegisteredCallTarget::Method { attribute, .. } if attribute == range))
                })
            })
        })
    }

    pub(super) fn from_checked_body(
        function: ir::Function,
        local_names: HashMap<usize, LocalBinding>,
        arguments: FrameArguments,
    ) -> Option<Self> {
        let mut checked = Self {
            inputs: FrameInputs::default(),
            frame_slots: FrameSlots::default(),
            placed_from: 0,
            fixed_sites: Box::default(),
            fixed_attributes: None,
            function,
            local_names,
        };
        (checked.inputs, checked.frame_slots) =
            crate::llvm::registered::frame_inputs(&checked, arguments)?;
        Some(checked)
    }

    /// Check that each use of a parameter can consume the caller's live Python value.
    pub(crate) fn bind_python_arguments(&mut self) -> bool {
        match &mut self.inputs {
            FrameInputs::Protocol { .. } => return true,
            FrameInputs::Deferred { live: false, .. } => return false,
            FrameInputs::Deferred { sites, live: true } => {
                self.inputs = FrameInputs::Protocol {
                    sites: std::mem::take(sites),
                };
                return true;
            }
            FrameInputs::Checked { .. } => {}
        }
        let Some((inputs, slots)) =
            crate::llvm::registered::frame_inputs(self, FrameArguments::Python)
        else {
            return false;
        };
        self.inputs = inputs;
        self.frame_slots = slots;
        true
    }

    /// Graph admission needs a complete function. The lexical owner prepares residual operands.
    pub(crate) fn from_suite(
        stmts: &[ast::Stmt],
        kind: RegisteredScope,
        source: values::RegisteredSource<'_>,
        operations: &HashMap<TextRange, Expr>,
        assertions_enabled: bool,
    ) -> Option<Self> {
        let mut scope = SourceScope::new(kind, source, Some(operations), !assertions_enabled);
        let (body, _) = scope.block(stmts).ok()?;
        Some(scope.finish(body))
    }

    #[cfg(test)]
    fn expression(range: TextRange, value: Expr) -> Self {
        let body = Self::returning(range, value);
        Self {
            inputs: FrameInputs::default(),
            frame_slots: FrameSlots::default(),
            placed_from: 0,
            fixed_sites: Box::default(),
            fixed_attributes: None,
            function: ir::Function {
                params: vec![],
                returns: Type::PythonObject,
                locals: vec![],
                body,
            },
            local_names: HashMap::new(),
        }
    }

    fn returning(range: TextRange, value: Expr) -> Vec<Statement> {
        vec![Statement::Position(range, None), Statement::Return(value)]
    }

    pub(crate) fn region(&self) -> RegisteredRegion<'_> {
        RegisteredRegion {
            #[cfg(any(debug_assertions, test, feature = "verify-ir"))]
            locals: &self.function.locals,
            #[cfg(any(debug_assertions, test, feature = "verify-ir"))]
            params: &self.function.params,
            local_names: &self.local_names,
            frame_slots: &self.frame_slots,
            root: RegionRoot::Suite(self),
        }
    }
}

/// Storage in the function frame or in a lexically nested Python namespace.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LocalBinding {
    pub(crate) name: String,
    pub(crate) namespace: Option<usize>,
}

fn generator_definition<'a>(
    modules: &'a [crate::modules::SourceModule],
    session: &crate::analysis::Session,
    owner: DefinitionId,
) -> Option<&'a ast::StmtFunctionDef> {
    let module = &modules[owner.module];
    let definition = module.parsed.syntax().body[owner.statement].as_function_def_stmt()?;
    (module.is_program_source() && session.is_generator_definition(owner.module, definition))
        .then_some(definition)
}

pub(super) fn retain_generator(
    graph: &SourceGraph,
    owner: DefinitionId,
    checks: &SourceChecks,
) -> Option<bool> {
    let definition = generator_definition(&graph.modules, &graph.session, owner)?;
    Some(
        checks.accepts(graph, owner.module, definition.range())
            && retain_generator_body(graph, owner.module, definition),
    )
}

pub(super) fn function_source(graph: &SourceGraph, id: ModuleId) -> RegisteredSource<'_> {
    let module = &graph.modules[id];
    RegisteredSource {
        operand_types: Some(graph.session.operand_types(id)),
        ..module
            .registered_values()
            .operands(&module.source, graph.python_optimization())
    }
}

fn retain_generator_body(
    graph: &SourceGraph,
    id: ModuleId,
    physical: &ast::StmtFunctionDef,
) -> bool {
    let Some(body) = RegisteredFunction::from_suite(
        &physical.body,
        RegisteredScope::Function {
            is_async: physical.is_async,
        },
        function_source(graph, id),
        &HashMap::new(),
        graph.python_optimization() == 0,
    ) else {
        return false;
    };
    graph.register_function(id, physical.start(), body);
    true
}

pub(super) fn lower_definition(
    checker: Checker<'_>,
    definition: &ast::StmtFunctionDef,
    signature: &Signature,
) -> Result<RegisteredFunction, String> {
    let source = checker.source;
    checked_definition(checker, definition, signature)?
        .map(|(_, frame)| frame)
        .ok_or_else(|| {
            error(
                source,
                definition,
                "function body has no checked registered-frame representation",
            )
            .classify(Kind::ScopedObject)
        })
}

/// Which checker owns the function's result contract.
pub(super) enum ReturnCheck<'a> {
    Declared,
    Infer,
    /// The emitting Python frame checks the declaration while returning a live handle.
    Live {
        check_calls: bool,
    },
    /// A Python statement can finish the frame while the fixed checker validates its cases.
    AfterFixed(&'a mut Option<CheckedPlacement>),
}

/// Keep the checked Python entry and the calling convention the original function requires.
/// A native layout refusal carries that frame to the definition that will publish it.
pub(super) fn place_checked_definition(
    original: &Checker<'_>,
    definition: &ast::StmtFunctionDef,
    signature: &Signature,
    inference: &inference::Inference,
    callable_inputs: Option<&mut HashMap<TextRange, FixedAttribute>>,
    result: ReturnCheck<'_>,
) -> Option<BodyResult> {
    let (facts, placement) = prepare_checked_definition(
        original,
        definition,
        signature,
        inference,
        callable_inputs,
        result,
    )
    .ok()??;
    if original.unrepresented.borrow().is_some() {
        if facts.needs_fixed_callee {
            return None;
        }
        return Some(Err(BodyRefusal {
            failure: original.unrepresented.take()?,
            cannot_return: facts.cannot_return,
            prepared: Some(Box::new(PreparedBody::Python(placement))),
        }));
    }
    let frame = &placement.frame.function;
    // Native call validation still consumes the checked formal and local layouts.
    let function = ir::Function {
        params: frame.params.clone(),
        returns: signature.returns,
        locals: frame.locals.clone(),
        body: Vec::new(),
    };
    placement.place(original.graph);
    Some(Ok((LoweredBody { function, facts }, HashMap::new())))
}

pub(super) struct CheckedPlacement {
    module: ModuleId,
    range: TextRange,
    frame: RegisteredFunction,
}

impl CheckedPlacement {
    pub(super) fn retain_for_launch(self, graph: &SourceGraph, final_definition: bool) {
        let start = self.range.start();
        let publish = if final_definition
            && matches!(self.frame.inputs, FrameInputs::Protocol { .. })
            && !graph.has_registered_function(self.module, start)
        {
            SourceGraph::register_function
        } else {
            SourceGraph::retain_lexical_function
        };
        publish(graph, self.module, start, self.frame);
    }

    pub(super) fn place(self, graph: &SourceGraph) {
        graph.register_function(self.module, self.range.start(), self.frame);
    }
}

pub(super) fn prepare_checked_definition(
    original: &Checker<'_>,
    definition: &ast::StmtFunctionDef,
    signature: &Signature,
    inference: &inference::Inference,
    callable_inputs: Option<&mut HashMap<TextRange, FixedAttribute>>,
    result: ReturnCheck<'_>,
) -> Result<Option<(FunctionFacts, CheckedPlacement)>, String> {
    let graph = original.graph;
    let module = original.module;
    let checks = &inference.source_checks;
    let contract = SourceChecks::cached(graph, module, definition.range());
    if contract == Some(false) {
        return Ok(None);
    }
    // A settled signature can use the source check that already owns this module.
    let provisional = checks.pending_arguments && contract.is_none();
    let helpers = Helpers::new(inference.len());
    let values = ModuleValues::default();
    let mut checker = Checker::new(
        graph,
        module,
        original.functions,
        original.classes,
        original.types,
        &helpers,
        &values,
    );
    checker.nonreturning_functions = original.nonreturning_functions;
    if matches!(result, ReturnCheck::Infer) {
        checker.python_result_tail = definition.body.last().map(Ranged::start);
    }
    let live = matches!(result, ReturnCheck::Live { .. });
    checker.checked_call_contracts = !matches!(result, ReturnCheck::Live { check_calls: false });
    if checker.checked_call_contracts {
        checker.boxed_calls = original.boxed_calls.as_ref().map(|_| HashSet::new());
        checker.inference = original.inference;
        if live {
            checker.frame_arguments = original.frame_arguments;
        } else if provisional || original.frame_arguments == FrameArguments::Pending {
            // A provisional launch frame still needs to bind live arguments after admission.
            checker.frame_arguments = FrameArguments::Pending;
        }
    }
    checker.fixed_checks_tail = matches!(result, ReturnCheck::AfterFixed(_));
    checker.settled_attributes = callable_inputs;
    checker.registered_local_start = Some(signature.params.len());
    let mut live_signature;
    let signature = if live {
        checker.registered_result_contract = Some(signature.returns);
        live_signature = signature.clone();
        live_signature.returns = Type::PythonObject;
        &live_signature
    } else {
        signature
    };
    let mut live_error = None;
    let placed = graph
        .try_registered_body(module, || {
            match checked_definition(checker, definition, signature) {
                // A representation trial may be discarded. A live result still owns its diagnostic.
                Err(error) if live => {
                    live_error = Some(error);
                    None
                }
                checked => checked.transpose(),
            }
        })
        .transpose()?;
    if let Some(error) = live_error {
        return Err(error);
    }
    let Some((facts, frame)) = placed else {
        return Ok(None);
    };
    if contract.is_none() && !checks.accepts(graph, module, definition.range()) {
        return Ok(None);
    }
    let erased = frame.function.returns == Type::PythonObject
        && signature.returns != Type::PythonObject
        && definition.returns.is_some();
    let placement = CheckedPlacement {
        module,
        range: definition.range(),
        frame,
    };
    if erased && let ReturnCheck::AfterFixed(after_fixed) = result {
        *after_fixed = Some(placement);
        return Ok(None);
    }
    Ok(Some((facts, placement)))
}

/// The selected Python return has no later checked statements that depend on
/// narrowing or assignment facts. Keep its prefix and finish in the same slots.
pub(super) fn finish_source_statement(
    checker: &mut Checker<'_>,
    function: &ast::StmtFunctionDef,
    statement: &ast::Stmt,
    body: &mut Vec<Statement>,
) -> Option<bool> {
    let mut scope = SourceScope::new(
        RegisteredScope::Function {
            is_async: function.is_async,
        },
        function_source(checker.graph, checker.module),
        None,
        checker.graph.python_optimization() != 0,
    );
    scope.locals = std::mem::take(&mut checker.locals);
    scope.names = std::mem::take(&mut checker.names);
    let locals = scope.locals.len();
    let start = body.len();
    let result = (&mut scope, SourceScope::statement_operand, false)
        .source_statement(statement, false, body)
        .ok()
        .map(|(op, continues)| {
            body.extend(op);
            continues
        });
    if result.is_none() {
        body.truncate(start);
        scope.truncate_locals(locals);
    }
    checker.locals = scope.locals;
    checker.names = scope.names;
    result
}

fn checked_definition(
    mut checker: Checker<'_>,
    definition: &ast::StmtFunctionDef,
    signature: &Signature,
) -> Result<Option<(FunctionFacts, RegisteredFunction)>, String> {
    checker.registered = Some(checker.graph.modules[checker.module].registered_values());
    let source = checker.source;
    let source_calls = checker.checked_call_contracts;
    let helpers = checker.helpers;
    let first_helper = helpers.len();
    let mut local_names = HashMap::new();
    let (body, _) = lower_function(checker, definition, signature, Some(&mut local_names), None)
        .map_err(|refusal| refusal.failure)?;
    if !source_calls && !body.facts.calls.is_empty() || helpers.len() != first_helper {
        return Err(error(
            source,
            definition,
            "the registered frame requires a callable with a Python binding",
        )
        .classify(Kind::ScopedObject));
    }
    let frame = RegisteredFunction::from_checked_body(
        body.function,
        local_names,
        body.facts.frame_arguments,
    );
    Ok(frame.map(|frame| (body.facts, frame)))
}

/// A registered body can supply an unresolved calling convention only if its other
/// operations satisfy the first-party source contract. Cache the check per physical module.
#[derive(Default)]
pub(super) struct SourceChecks {
    session: std::cell::OnceCell<Option<crate::analysis::RegisteredBodyChecks>>,
    /// A provisional source frame must bind its live arguments after admission.
    pub(super) pending_arguments: bool,
}

impl SourceChecks {
    fn accepts_range(
        errors: &[crate::analysis::RegisteredSourceError],
        definition: TextRange,
    ) -> bool {
        errors
            .iter()
            .filter_map(|error| error.range())
            .all(|error| !definition.contains_range(error))
    }

    pub(super) fn cached(
        graph: &SourceGraph,
        module: ModuleId,
        definition: TextRange,
    ) -> Option<bool> {
        graph
            .registered_source_errors
            .borrow()
            .get(&module)
            .map(|errors| Self::accepts_range(errors, definition))
    }

    /// First-party return sites have a final contract check. For an installed
    /// declaration, use the existing source checks at its implicit and explicit exits.
    pub(super) fn accepts_return(
        &self,
        graph: &SourceGraph,
        module: ModuleId,
        function: &ast::StmtFunctionDef,
        annotation: &ast::Expr,
    ) -> bool {
        if graph.modules[module].checks_source_contracts() {
            return true;
        }
        let accepts = |range| self.accepts(graph, module, range);
        accepts(annotation.range())
            && crate::analysis::explicit_returns(function)
                .into_iter()
                .all(|ret| accepts(ret.range()))
    }

    pub(super) fn accepts(
        &self,
        graph: &SourceGraph,
        module: ModuleId,
        definition: TextRange,
    ) -> bool {
        if let Some(accepted) = Self::cached(graph, module, definition) {
            return accepted;
        }
        let Some(errors) = graph
            .session
            .current_registered_source_errors(module)
            .or_else(|| {
                self.session
                    .get_or_init(|| graph.session.registered_body_checks().ok())
                    .as_ref()
                    .and_then(|checks| checks.source_errors(&graph.session, module))
            })
        else {
            return false;
        };
        let accepted = Self::accepts_range(&errors, definition);
        graph
            .registered_source_errors
            .borrow_mut()
            .insert(module, errors);
        accepted
    }
}

/// Both operands are already Python handles. A heterogeneous selection needs no conversion.
fn selected_type(left: Type, right: Type) -> Type {
    if left == right {
        left
    } else {
        Type::PythonObject
    }
}

pub(super) fn truth(value: Expr) -> Expr {
    operation(Builtin::Bool, Type::Bool, vec![value])
}

pub(super) fn select(condition: Expr, yes: Expr, no: Expr) -> Expr {
    Expr {
        ty: selected_type(yes.ty, no.ty),
        kind: ExprKind::Conditional(Box::new(condition), Box::new(yes), Box::new(no)),
    }
}

pub(super) fn selected_operands(and: bool, mut operands: Vec<Expr>) -> Expr {
    let mut result = operands.pop().expect("boolean operands are nonempty");
    for left in operands.into_iter().rev() {
        result = Expr {
            ty: selected_type(left.ty, result.ty),
            kind: ExprKind::BooleanValue(and, Box::new(left), Box::new(result), None),
        };
    }
    result
}

/// Fixed validation borrows these functions. A Python entry that survives the
/// checks takes the original operations, including checked siblings on an error exit.
#[derive(Default)]
pub(crate) struct FixedFunctions {
    pub(crate) functions: Vec<ir::Function>,
    pub(crate) frames: crate::FixedFrameSources,
}

impl FixedFunctions {
    pub(crate) fn register(
        self,
        graph: &SourceGraph,
        owners: &[ModuleId],
        mut accepted: impl FnMut(ModuleId, TextRange) -> bool,
    ) {
        self.place(
            graph,
            owners,
            |_| true,
            |module, definition, frame| {
                if accepted(module, definition) {
                    graph.register_function(module, definition.start(), frame);
                } else if !matches!(frame.inputs, FrameInputs::Protocol { .. })
                    && frame.owns_lookup_errors(graph, module, definition)
                {
                    graph.retain_lexical_function(module, definition.start(), frame);
                }
            },
        );
    }

    pub(super) fn retain_lexical(self, graph: &SourceGraph, owners: &[ModuleId], entry: bool) {
        self.place(
            graph,
            owners,
            |module| {
                let source = &graph.modules[module];
                (module != graph.entry || entry)
                    && (source.is_program_source()
                        || graph.python_file(&source.name) == Some(module))
            },
            |module, definition, frame| {
                // The file's selected source contracts run before these provisional frames
                // are taken and their Python arguments are bound.
                if module != graph.entry
                    || SourceChecks::cached(graph, module, definition) != Some(false)
                    || !matches!(frame.inputs, FrameInputs::Protocol { .. })
                        && frame.owns_lookup_errors(graph, module, definition)
                {
                    graph.retain_lexical_function(module, definition.start(), frame);
                }
            },
        );
    }

    fn place(
        self,
        graph: &SourceGraph,
        owners: &[ModuleId],
        selected: impl Fn(ModuleId) -> bool,
        mut publish: impl FnMut(ModuleId, TextRange, RegisteredFunction),
    ) {
        // Frame sources are recorded in slot order. Functions without a physical
        // source (including synthesized helpers) drop with the iterator.
        let mut functions = self.functions.into_iter();
        let mut next = 0;
        for (slot, definition, source) in self.frames {
            let function = functions.nth(slot - next).expect("checked function slot");
            next = slot + 1;
            let module = owners[slot];
            if selected(module)
                && !graph.has_registered_function(module, definition.start())
                && let Ok(mut placed) = source.place_in(function, graph, module)
                && (!graph.modules[module].bundled || placed.bind_python_arguments())
            {
                publish(module, definition, placed);
            }
        }
    }
}

/// Return the class member whose unchanged syntax supplied this checked signature.
pub(super) fn physical_method<'a>(
    signature: &Signature,
    checked: &ast::StmtFunctionDef,
    classes: &'a ClassInfo,
) -> Option<&'a crate::classes::MethodSource> {
    let method = signature.method?;
    let declaration =
        &classes.declarations.definitions[method.class].methods[method.index].function;
    (signature.physical_frame && std::ptr::eq(checked, &**declaration)).then_some(declaration)
}

/// A declaration without fixed callers uses the live Python argument binder.
#[derive(Clone, Copy)]
pub(super) enum Capture<'a> {
    Declared,
    Solved(Option<&'a HashSet<usize>>),
}

#[expect(
    clippy::too_many_arguments,
    reason = "a recovery attempt borrows the graph's current signature and inference tables"
)]
pub(super) fn capture(
    graph: &SourceGraph,
    signatures: &HashMap<DefinitionId, Signature>,
    definitions: &[FunctionDefinition<'_>],
    owners: &[DefinitionId],
    classes: &ClassInfo,
    types: &Types,
    inference: &inference::Inference,
    source: Capture,
    candidates: impl IntoIterator<Item = (usize, Option<PreparedBody>)>,
) {
    // Methods under the same class share a module definition, but have separate signature slots.
    for (slot, prepared) in candidates {
        let (id, checked, signature) = &definitions[slot];
        let id = *id;
        if signature.registered_frame() || !graph.modules[id].is_program_source() {
            continue;
        }
        let owner = owners[slot];
        let method = physical_method(signature, checked, classes);
        let function = match method {
            Some(physical) => &**physical,
            None if signatures
                .get(&owner)
                .is_some_and(|original| original.id == slot) =>
            {
                let Some(function) =
                    graph.modules[id].parsed.syntax().body[owner.statement].as_function_def_stmt()
                else {
                    continue;
                };
                function
            }
            _ => continue,
        };
        // Recovery may follow an owning definition that has already checked this site.
        if graph.has_registered_function(id, function.start()) {
            continue;
        }
        let contract = SourceChecks::cached(graph, id, function.range());
        if contract == Some(false) {
            continue;
        }
        let checks = &inference.source_checks;
        let provisional = checks.pending_arguments
            && contract.is_none()
            && matches!(source, Capture::Solved(None));
        let helpers = Helpers::new(inference.len());
        // A solved definition reads names from the Python namespace. Its trial
        // must not establish a native caller or consume a missing binding.
        let private = ModuleValues::default();
        let checker = || Checker::new(graph, id, signatures, classes, types, &helpers, &private);
        let checked = matches!(source, Capture::Solved(Some(ids)) if ids.contains(&slot));
        // A revision can invalidate a physical method's last check. Check its settled
        // signature before trying the registered return convention.
        let check_method =
            method.is_some() && !checked && !matches!(prepared, Some(PreparedBody::Attributes(_)));
        let (prepared, mut attributes, lexical) = match prepared {
            Some(PreparedBody::Fixed(body)) => (Some(body), HashMap::new(), None),
            Some(PreparedBody::Attributes(attributes)) => (None, attributes, None),
            Some(PreparedBody::Python(frame)) => (None, HashMap::new(), Some(frame)),
            Some(PreparedBody::Rejected) => continue,
            None => (None, HashMap::new(), None),
        };
        let placed = if prepared.is_some() || check_method {
            let body = prepared.or_else(|| {
                let mut fixed = checker();
                fixed.fixed_sites = Some(fixed_body_source::SourceSites::new(true));
                fixed.placement_only = true;
                fixed.python_method_frame = true;
                fixed.settled_attributes = Some(&mut attributes);
                graph.try_registered_body(id, || lower_body(fixed, function, signature).ok())
            });
            let Some((body, _)) = body else {
                continue;
            };
            body.facts
                .source
                .and_then(|source| match source.place_in(body.function, graph, id) {
                    Ok(placed) => Some(placed),
                    Err(settled) => {
                        attributes = settled;
                        None
                    }
                })
        } else {
            None
        };
        let body = placed.or_else(|| {
            // The prepared function can serve the live caller when its own operand sites
            // accept Python arguments and the caller needs no checked callback coercion.
            if !checked
                && let Some(mut frame) = lexical
                && frame.frame.bind_python_arguments()
            {
                return Some(frame.frame);
            }
            let mut signature = signature.clone();
            signature.returns = Type::PythonObject;
            let mut checker = checker();
            checker.registered_local_start = Some(signature.params.len());
            checker.settled_attributes = (!attributes.is_empty()).then_some(&mut attributes);
            checker.checked_callback_effects = checked;
            checker.frame_arguments = match source {
                Capture::Declared => {
                    for parameter in &mut signature.params {
                        parameter.ty = Type::PythonObject;
                    }
                    FrameArguments::Python
                }
                Capture::Solved(_) if method.is_some() => FrameArguments::Python,
                Capture::Solved(_) if provisional => FrameArguments::Pending,
                Capture::Solved(_) => FrameArguments::Checked,
            };
            let mut body = graph
                .try_registered_body(id, || lower_definition(checker, function, &signature).ok())?;
            if let Some(method) = method {
                let FrameInputs::Protocol { sites } = &mut body.inputs else {
                    return None;
                };
                let mut required = std::mem::take(sites).into_vec();
                required.push(FrameSite::Method(method.clone()));
                *sites = required.into_boxed_slice();
            }
            Some(body)
        });
        if let Some(body) = body {
            if contract.unwrap_or_else(|| checks.accepts(graph, id, function.range())) {
                graph.register_function(id, function.start(), body);
            } else if provisional && !matches!(body.inputs, FrameInputs::Protocol { .. }) {
                graph.retain_lexical_function(id, function.start(), body);
            }
        }
    }
}

/// Written loop jumps carry their checkpoint directly on the terminating instruction.
pub(super) fn push_checkpoint(body: &mut Vec<Statement>, stmt: &ast::Stmt) {
    if !matches!(stmt, ast::Stmt::Break(_) | ast::Stmt::Continue(_)) {
        body.push(checkpoint(stmt));
    }
}

fn checkpoint(stmt: &ast::Stmt) -> Statement {
    // Truth conversion and iterator advancement can call Python after evaluating the operand.
    let range = match stmt {
        ast::Stmt::If(branch) => branch.test.range(),
        ast::Stmt::For(loop_) if !loop_.is_async => loop_.iter.range(),
        _ => stmt.range(),
    };
    Statement::Position(range, imre_ast::unhighlighted_call(stmt))
}

pub(super) fn assertion_statements(
    stmt: &ast::StmtAssert,
    test: Expr,
    message: Option<Expr>,
) -> [Statement; 2] {
    let failure = operation(
        Builtin::RegisteredAssertionFailure {
            statement: stmt.start(),
            test: stmt.test.range(),
        },
        Type::None,
        message.into_iter().collect(),
    );
    [
        Statement::Position(stmt.test.range(), None),
        Statement::If(test, Vec::new(), vec![Statement::Expr(failure)]),
    ]
}

pub(super) fn checked_local_scalar(expected: Option<Type>, value: Expr) -> Expr {
    if expected == value.kind.scalar_literal_type() {
        return value;
    }
    match expected.and_then(Type::builtin_scalar_name) {
        Some(name) => operation(Builtin::RegisteredLocalScalar(name), value.ty, vec![value]),
        None => value,
    }
}

fn operation(op: Builtin, ty: Type, operands: Vec<Expr>) -> Expr {
    Expr {
        ty,
        kind: ExprKind::Builtin(op, operands),
    }
}

pub(super) fn in_place(op: ast::Operator, left: Expr, right: Expr) -> Expr {
    operation(
        Builtin::RegisteredInPlace(op.as_str()),
        Type::PythonObject,
        vec![left, right],
    )
}

pub(crate) enum AnnotationExpression {
    Decoded,
    FormatEvaluator,
}

impl AnnotationExpression {
    /// Compile the expression in the frame that will evaluate its annotations.
    pub(crate) fn prepare(
        self,
        rhs: &ast::Expr,
        source: values::RegisteredSource<'_>,
    ) -> Option<Box<RegisteredFunction>> {
        let mut scope = SourceScope::new(
            RegisteredScope::Function { is_async: false },
            source,
            None,
            false,
        );
        let body = match self {
            Self::Decoded => RegisteredFunction::returning(rhs.range(), scope.operand(rhs)?),
            Self::FormatEvaluator => {
                let value = super::annotation_operands::checked_annotation_value(rhs, |expr| {
                    scope.operand(expr)
                })?;
                let format = operation(
                    Builtin::RegisteredAnnotationFormat,
                    Type::None,
                    vec![values::name_binding(".format")],
                );
                vec![
                    Statement::Position(rhs.range(), None),
                    Statement::Expr(format),
                    Statement::Return(value),
                ]
            }
        };
        Some(Box::new(scope.finish(body)))
    }
}

pub(crate) struct ParameterEvaluator {
    pub(crate) range: TextRange,
    reads: super::function_scope::ParameterReads,
    pub(crate) body: Option<Box<RegisteredFunction>>,
}

impl ParameterEvaluator {
    fn prepare(rhs: &ast::Expr, source: values::RegisteredSource<'_>) -> Self {
        Self {
            range: rhs.range(),
            reads: super::function_scope::parameter_reads(&[("", rhs)], false),
            body: AnnotationExpression::FormatEvaluator.prepare(rhs, source),
        }
    }

    pub(crate) fn scope(
        &self,
        class: Option<&super::function_scope::ClassBindings>,
    ) -> super::function_scope::FunctionScope {
        super::function_scope::FunctionScope::parameter(self.reads.clone(), class)
    }
}

/// Preserve the requested module separately from the names bound in the current namespace.
pub(super) fn registered_import(
    stmt: &ast::Stmt,
    private_names: Option<&imre_ast::Names>,
) -> Option<Statement> {
    let import = match stmt {
        ast::Stmt::Import(import) => ir::NamespaceImport::Modules(
            import
                .names
                .iter()
                .map(|alias| ir::ModuleImport {
                    name: alias.name.id.clone(),
                    asname: alias.asname.as_ref().map(|name| name.id.clone()),
                    bind_leaf: alias.asname.is_some()
                        && !private_names.is_some_and(|names| names.imports_root(&alias.name)),
                })
                .collect(),
        ),
        ast::Stmt::ImportFrom(import) => ir::NamespaceImport::From {
            module: import
                .module
                .as_ref()
                .map_or("", ast::Identifier::as_str)
                .to_owned(),
            level: import.level,
            names: import
                .names
                .iter()
                .map(|alias| {
                    (
                        alias.name.to_string(),
                        alias.asname.as_ref().map(ToString::to_string),
                    )
                })
                .collect(),
        },
        _ => return None,
    };
    Some(Statement::Expr(operation(
        Builtin::RegisteredImport(import),
        Type::None,
        vec![],
    )))
}

#[derive(Clone, Copy)]
pub(crate) enum RegisteredScope {
    Module,
    Class,
    Lambda,
    Function { is_async: bool },
}

/// Select a quoted annotation whose decoded expression needs its own executable body.
pub(crate) fn annotation_expression(
    text: &str,
) -> Option<ruff_python_parser::Parsed<ast::ModExpression>> {
    let parsed = imre_ast::forward_refs::parse(text).ok()?;
    imre_ast::forward_refs::validate(&parsed).ok()?;
    Some(parsed)
}

#[derive(Clone)]
pub(crate) struct DeferredAnnotation {
    pub(crate) range: TextRange,
    pub(crate) name: String,
    pub(crate) value: StdResult<Expr, (TextRange, &'static str)>,
}

pub(crate) type CheckedProvider = StdResult<RegisteredFunction, OperandRefusal>;
pub(crate) type AnnotationProvider = CheckedProvider;

pub(crate) struct ParameterAnnotations {
    pub(crate) start: TextSize,
    pub(crate) scope: super::function_scope::FunctionScope,
    pub(crate) body: AnnotationProvider,
    /// Scalar names used by the callable's checked argument and return boundary.
    pub(crate) references: HashMap<String, String>,
}

impl ParameterAnnotations {
    fn prepare(
        def: &ast::StmtFunctionDef,
        source: values::RegisteredSource<'_>,
        operand: &mut dyn FnMut(&ast::Expr, bool) -> Option<Expr>,
        checked_annotations: bool,
        lexical: impl FnOnce() -> super::function_scope::FunctionScope,
    ) -> Option<Box<Self>> {
        let mut references = HashMap::new();
        // Each annotation is evaluated in the same provider activation.
        let mut scope = SourceScope::new(
            RegisteredScope::Function { is_async: false },
            source,
            None,
            false,
        );
        let mut fields = parameter_annotations(&def.parameters, def.returns.as_deref()).map(
            |(name, expression)| {
                if checked_annotations && let Some(reference) = scalar_reference(expression) {
                    references.insert(name.to_owned(), reference);
                }
                let value = operand(expression, false);
                (
                    DeferredAnnotation::new(name.to_owned(), expression, value, &mut scope),
                    None,
                )
            },
        );
        let first = fields.next()?;
        let body = DeferredAnnotation::return_body(std::iter::once(first).chain(&mut fields));
        // A refused provider still prepares its later annotation operands and lexical children.
        fields.for_each(drop);
        let body = body.map(|body| scope.finish(body));
        Some(Box::new(Self {
            start: def.parameters.start(),
            scope: lexical(),
            body,
            references,
        }))
    }
}

/// Names that can refer directly to a type without calling Python.
fn scalar_reference(expr: &ast::Expr) -> Option<String> {
    match expr {
        ast::Expr::NoneLiteral(_) => Some("None".to_owned()),
        ast::Expr::Name(name) if !name.id.starts_with("__") => Some(name.id.to_string()),
        ast::Expr::Attribute(attr) => {
            let module = attr.value.as_name_expr()?;
            (!module.id.starts_with("__") && !attr.attr.starts_with("__"))
                .then(|| format!("{}.{}", module.id, attr.attr))
        }
        _ => None,
    }
}

#[derive(Clone, Default)]
pub(crate) struct RegisteredBindings {
    locals: Vec<Type>,
    names: HashMap<usize, LocalBinding>,
    slots: FrameSlots,
}

impl DeferredAnnotation {
    fn new(
        name: String,
        expression: &ast::Expr,
        checked: Option<Expr>,
        scope: &mut SourceScope<'_>,
    ) -> Self {
        let value = match checked {
            Some(value) => Ok(value),
            None => {
                let start = scope.locals.len();
                scope.refusal = None;
                super::annotation_operands::checked_annotation_value(expression, |input| {
                    scope.operand(input)
                })
                .ok_or_else(|| {
                    scope.truncate_locals(start);
                    scope.refusal.take().unwrap_or((
                        expression.range(),
                        "annotation expression cannot be compiled ahead of time",
                    ))
                })
            }
        };
        Self {
            name,
            range: expression.range(),
            value,
        }
    }

    /// Build one provider in the order its checked fields become visible.
    pub(crate) fn prepare_body(
        declarations: impl IntoIterator<Item = (Self, Option<usize>)>,
        bindings: RegisteredBindings,
    ) -> AnnotationProvider {
        Self::return_body(declarations).map(|body| bindings.finish(body))
    }

    fn return_body(
        declarations: impl IntoIterator<Item = (Self, Option<usize>)>,
    ) -> StdResult<Vec<Statement>, OperandRefusal> {
        let mut fields = Vec::new();
        let mut values = Vec::new();
        for (field, index) in declarations {
            fields.push(ir::AnnotationField {
                name: field.name,
                range: field.range,
                index,
            });
            values.push(field.value?);
        }
        Ok(vec![Statement::Return(operation(
            Builtin::RegisteredAnnotations {
                fields: fields.into(),
            },
            Type::PythonObject,
            values,
        ))])
    }
}

/// Physical source operands shared by namespace initialization and lazy annotation providers.
pub(crate) struct FunctionSite<'a> {
    pub(crate) def: &'a ast::StmtFunctionDef,
    pub(crate) annotations: Option<Box<ParameterAnnotations>>,
    pub(crate) parameters: Vec<ir::RegisteredTypeParameter<ParameterEvaluator>>,
    module_scope: bool,
    pub(crate) thread_entry: Option<imre_ast::ThreadEntry>,
}

pub(crate) struct RegisteredClass {
    pub(crate) annotations: super::ClassAnnotations,
    pub(crate) name: ast::Identifier,
    pub(crate) range: TextRange,
    pub(crate) doc: Option<String>,
    pub(crate) body: RegisteredSuite,
    pub(crate) header: Option<ClassHeader>,
}

/// The type-parameter scope evaluates generic bases and keywords.
pub(crate) struct ClassHeader {
    pub(crate) parameters: TypeParameterValues,
    pub(crate) body: CheckedProvider,
}

/// Parameter fields close over their declaration's peers and any enclosing class dictionary.
pub(crate) struct TypeParameterValues {
    pub(crate) declarations: Vec<ir::RegisteredTypeParameter<ParameterEvaluator>>,
    pub(crate) enclosing_class: Option<TextSize>,
}

impl TypeParameterValues {
    pub(crate) fn prepare(
        params: &ast::TypeParams,
        names: &imre_ast::Names,
        source: values::RegisteredSource<'_>,
        enclosing_class: Option<TextSize>,
    ) -> Self {
        Self {
            declarations: registered_type_parameters(params, names, source),
            enclosing_class,
        }
    }
}

/// Module declarations and their runtime marks use the same conditional indices.
#[derive(Default)]
pub(crate) struct ModuleAnnotations {
    fields: Vec<DeferredAnnotation>,
    bindings: RegisteredBindings,
}

impl ModuleAnnotations {
    pub(crate) fn into_body(self) -> Option<AnnotationProvider> {
        if self.fields.is_empty() {
            return None;
        }
        let fields = self
            .fields
            .into_iter()
            .enumerate()
            .map(|(index, field)| (field, Some(index)));
        Some(DeferredAnnotation::prepare_body(fields, self.bindings))
    }
}

pub(crate) enum DeclarationSite<'a> {
    Function(FunctionSite<'a>),
    Class(ClassSite<'a>),
}

pub(crate) struct ClassSite<'a> {
    def: &'a ast::StmtClassDef,
    annotations: super::ClassAnnotations,
}

#[derive(Default)]
pub(crate) struct RegisteredSites<'a> {
    pub(crate) operations: HashMap<TextRange, Expr>,
    pub(crate) declarations: Vec<DeclarationSite<'a>>,
    pub(crate) module_annotations: ModuleAnnotations,
}

pub(crate) fn registered_sites<'a, R>(
    body: (&'a [ast::Stmt], super::function_scope::FunctionChildren<'a>),
    scopes: &mut super::function_scope::ScopeFacts<'_>,
    names: &imre_ast::Names,
    source: values::RegisteredSource<'_>,
    bundled: bool,
    checked_annotations: bool,
    prepare_child: impl Fn(
        &str,
        &ruff_python_parser::Parsed<ast::ModExpression>,
    ) -> Option<AnnotationBody<R>>,
) -> (
    RegisteredSites<'a>,
    indexmap::IndexMap<String, AnnotationBody<R>>,
) {
    let mut children = indexmap::IndexMap::new();
    let sites = collect_sites(
        body,
        scopes,
        names,
        source,
        checked_annotations,
        |expr, module| {
            super::annotation_operands::annotation_operand(
                expr,
                module,
                source,
                names.string_annotations,
                bundled,
                &mut children,
                &prepare_child,
            )
        },
    );
    (sites, children)
}

pub(super) fn collect_sites<'a>(
    (body, children): (&'a [ast::Stmt], super::function_scope::FunctionChildren<'a>),
    scopes: &mut super::function_scope::ScopeFacts<'_>,
    names: &imre_ast::Names,
    source: values::RegisteredSource<'_>,
    checked_annotations: bool,
    mut operand: impl FnMut(&ast::Expr, bool) -> Option<Expr>,
) -> RegisteredSites<'a> {
    use ast::statement_visitor::{StatementVisitor, walk_body, walk_stmt};
    #[derive(Default)]
    struct ClassFields {
        plan: super::ClassAnnotations,
        fields: Vec<(DeferredAnnotation, Option<usize>)>,
        conditional: bool,
    }
    struct Collector<'s, 'a, 'p> {
        scopes: &'s mut super::function_scope::ScopeFacts<'p>,
        owners: Vec<&'a ast::Stmt>,
        children: &'s [&'a ast::Stmt],
        names: &'s imre_ast::Names,
        source: values::RegisteredSource<'s>,
        checked_annotations: bool,
        operand: &'s mut dyn FnMut(&ast::Expr, bool) -> Option<Expr>,
        annotations: SourceScope<'s>,
        class: ClassFields,
        scope: RegisteredScope,
        output: RegisteredSites<'a>,
    }
    impl<'a> StatementVisitor<'a> for Collector<'_, 'a, '_> {
        fn visit_body(&mut self, body: &'a [ast::Stmt]) {
            let outer = std::mem::replace(&mut self.class.conditional, true);
            walk_body(self, body);
            self.class.conditional = outer;
        }

        fn visit_stmt(&mut self, stmt: &'a ast::Stmt) {
            match stmt {
                ast::Stmt::ClassDef(class) => {
                    let site = self.output.declarations.len();
                    self.output
                        .declarations
                        .push(DeclarationSite::Class(ClassSite {
                            def: class,
                            annotations: Default::default(),
                        }));
                    let outer = std::mem::replace(&mut self.scope, RegisteredScope::Class);
                    let child = SourceScope::new(
                        self.annotations.kind,
                        self.source,
                        None,
                        self.annotations.omit_assertions,
                    );
                    let outer_annotations = std::mem::replace(&mut self.annotations, child);
                    let outer_fields = std::mem::take(&mut self.class);
                    self.owners.push(stmt);
                    walk_body(self, &class.body);
                    self.owners.pop();
                    let ClassFields {
                        mut plan, fields, ..
                    } = std::mem::replace(&mut self.class, outer_fields);
                    self.scope = outer;
                    let child = std::mem::replace(&mut self.annotations, outer_annotations);
                    if !fields.is_empty() {
                        plan.body = Some(Box::new(DeferredAnnotation::prepare_body(
                            fields,
                            child.bindings(),
                        )));
                    }
                    self.output.declarations[site] = DeclarationSite::Class(ClassSite {
                        def: class,
                        annotations: plan,
                    });
                }
                ast::Stmt::FunctionDef(def) => {
                    let annotations = ParameterAnnotations::prepare(
                        def,
                        self.source,
                        self.operand,
                        self.checked_annotations,
                        || self.scopes.annotation_scope(def, &self.owners),
                    );
                    let parameters = def.type_params.as_deref().map_or_else(Vec::new, |params| {
                        registered_type_parameters(params, self.names, self.source)
                    });
                    let thread_entry = imre_ast::thread_entry(def);
                    let worker = thread_entry.is_some();
                    self.output
                        .declarations
                        .push(DeclarationSite::Function(FunctionSite {
                            def,
                            annotations,
                            parameters,
                            module_scope: matches!(self.scope, RegisteredScope::Module),
                            thread_entry,
                        }));
                    if worker {
                        // A proven worker contains no nested declarations or namespace effects.
                        return;
                    }
                    let outer = std::mem::replace(
                        &mut self.scope,
                        RegisteredScope::Function {
                            is_async: def.is_async,
                        },
                    );
                    // Local annotations and executable statements belong to the function body.
                    let after = self
                        .children
                        .partition_point(|child| child.start() <= def.start());
                    self.owners.push(stmt);
                    let mut children = &self.children[after..];
                    while let Some((&child, rest)) = children.split_first() {
                        if child.start() >= def.end() {
                            break;
                        }
                        self.visit_stmt(child);
                        // The child just visited owns all declarations within its range.
                        children = &rest[rest.partition_point(|next| next.start() < child.end())..];
                    }
                    self.owners.pop();
                    self.scope = outer;
                }
                ast::Stmt::AnnAssign(ann)
                    if !matches!(self.scope, RegisteredScope::Function { .. }) =>
                {
                    let module = matches!(self.scope, RegisteredScope::Module);
                    if !module && self.names.string_annotations {
                        self.class.plan.postponed = true;
                    }
                    let Some(name) = ann.target.as_name_expr().filter(|_| ann.simple) else {
                        return;
                    };

                    let name = name.id.to_string();
                    let operand = (self.operand)(&ann.annotation, module);
                    let (annotation, values) = if self.names.string_annotations {
                        (
                            ir::NamespaceAnnotation::String { name },
                            vec![operand.expect("prepared postponed annotation")],
                        )
                    } else {
                        let field = DeferredAnnotation::new(
                            name.clone(),
                            &ann.annotation,
                            operand,
                            &mut self.annotations,
                        );
                        let mark = if module {
                            let fields = &mut self.output.module_annotations.fields;
                            let index = fields.len();
                            fields.push(field);
                            ir::NamespaceAnnotation::Module { index }
                        } else {
                            let index = self.class.conditional.then(|| {
                                let index = self.class.plan.conditional_count;
                                self.class.plan.conditional_count += 1;
                                index
                            });
                            self.class.fields.push((field, index));
                            ir::NamespaceAnnotation::Class { index, name }
                        };
                        (mark, Vec::new())
                    };
                    self.output.operations.insert(
                        ann.target.range(),
                        operation(
                            Builtin::RegisteredNamespaceAnnotation(annotation),
                            Type::None,
                            values,
                        ),
                    );
                }
                // Ordinary try numbers its else before the handlers. Except-star uses source order.
                ast::Stmt::Try(region)
                    if (matches!(self.scope, RegisteredScope::Module)
                        || matches!(self.scope, RegisteredScope::Class)
                            && !self.names.string_annotations)
                        && !region.is_star
                        && !region.orelse.is_empty() =>
                {
                    self.visit_body(&region.body);
                    self.visit_body(&region.orelse);
                    for handler in &region.handlers {
                        self.visit_except_handler(handler);
                    }
                    self.visit_body(&region.finalbody);
                }
                _ => walk_stmt(self, stmt),
            }
        }
    }
    let mut sites = Collector {
        scopes,
        owners: Vec::new(),
        children: &children,
        names,
        source,
        checked_annotations,
        operand: &mut operand,
        annotations: SourceScope::new(
            RegisteredScope::Function { is_async: false },
            source,
            None,
            false,
        ),
        scope: RegisteredScope::Module,
        class: Default::default(),
        output: RegisteredSites::default(),
    };
    sites.visit_body(body);
    sites.output.module_annotations.bindings = sites.annotations.bindings();
    sites.output
}

/// Statements are appended in evaluation order. Consecutive ordinary statements
/// keep their declaration walk until a physical operand or a child suite intervenes.
struct SuiteBody {
    statements: Vec<Statement>,
    check_from: Option<usize>,
    falls_through: bool,
    refusal: Option<PreparedRefusal>,
}

impl SuiteBody {
    fn check_run(&mut self, preparation: &SuitePreparation<'_>) {
        self.check_through(preparation, self.statements.len());
    }

    fn check_through(&mut self, preparation: &SuitePreparation<'_>, end: usize) {
        if let Some(start) = self.check_from {
            self.refusal = self
                .refusal
                .or_else(|| preparation.checked_declaration_refusal(&self.statements[start..end]));
            self.check_from = self.falls_through.then_some(end);
        }
    }
}

fn bind_operand(operands: &mut Vec<PhysicalOperand>, value: PhysicalOperand, ty: Type) -> Expr {
    let index = operands.len();
    operands.push(value);
    Expr::registered_input(index, ty)
}

/// The graph lends its checked function. Source preparation owns either a complete
/// lexical function or a suite with additional operands and child frames.
pub(crate) enum NamedBody<'p> {
    Borrowed(&'p RegisteredFunction),
    Owned(Box<RegisteredFunction>),
    Suite(Box<RegisteredSuite>),
}

/// LLVM receives the same function body regardless of which frontend owns it.
pub(crate) enum RegisteredBody<'p> {
    Function(&'p RegisteredFunction),
    Suite(&'p RegisteredSuite),
}

impl NamedBody<'_> {
    pub(crate) fn from_suite(suite: RegisteredSuite) -> Self {
        if suite.fully_checked() {
            Self::Owned(Box::new(suite.checked))
        } else {
            Self::Suite(Box::new(suite))
        }
    }

    pub(crate) fn as_ref(&self) -> RegisteredBody<'_> {
        match self {
            Self::Borrowed(body) => RegisteredBody::Function(body),
            Self::Owned(body) => RegisteredBody::Function(body),
            Self::Suite(suite) => RegisteredBody::Suite(suite),
        }
    }
}

pub(crate) struct RegisteredSuite {
    pub(crate) refusal: Option<PreparedRefusal>,
    falls_through: bool,
    pub(crate) checked: RegisteredFunction,
    pub(crate) operands: Vec<PhysicalOperand>,
    pub(crate) suites: Vec<RegisteredSuite>,
}

impl RegisteredSuite {
    pub(crate) fn fully_checked(&self) -> bool {
        self.operands.is_empty() && self.suites.is_empty()
    }

    #[cfg(test)]
    pub(crate) fn child_suite(&self, body: &[Statement]) -> Option<&RegisteredSuite> {
        let [
            Statement::Expr(Expr {
                kind: ExprKind::Builtin(Builtin::RegisteredSuite(index), _),
                ..
            }),
        ] = body
        else {
            return None;
        };
        Some(&self.suites[*index])
    }

    fn new(checked: RegisteredFunction, operands: Vec<PhysicalOperand>) -> Self {
        Self {
            checked,
            operands,
            suites: Vec::new(),
            refusal: None,
            falls_through: true,
        }
    }

    pub(crate) fn operand(&mut self, value: PhysicalOperand, ty: Type) -> Expr {
        bind_operand(&mut self.operands, value, ty)
    }

    fn bind_refusal(
        operands: &mut Vec<PhysicalOperand>,
        (range, reason): OperandRefusal,
    ) -> Statement {
        let rejected = booleans::OwnedExpression {
            range,
            selected: std::rc::Rc::new(booleans::Selection::Refused(range, reason)),
        };
        let value = bind_operand(
            operands,
            PhysicalOperand::Residual(rejected),
            Type::PythonObject,
        );
        Statement::Expr(value)
    }

    fn child(&mut self, suite: RegisteredSuite) -> Vec<Statement> {
        suite.into_child(&mut self.suites)
    }

    fn into_child(self, suites: &mut Vec<RegisteredSuite>) -> Vec<Statement> {
        // A source suite with no private operands or locals can run in its enclosing frame.
        if self.checked.function.locals.is_empty()
            && self.operands.is_empty()
            && self.suites.is_empty()
        {
            return self.checked.function.body;
        }
        let index = suites.len();
        suites.push(self);
        vec![Statement::Expr(operation(
            Builtin::RegisteredSuite(index),
            Type::None,
            vec![],
        ))]
    }
}

pub(crate) type PhysicalOperand =
    booleans::SourceOperand<Box<RegisteredOperand>, booleans::OwnedExpression>;

pub(crate) type OperandRefusal = (TextRange, &'static str);

/// First reachable refusal selected in this lexical source during suite preparation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PreparedRefusal {
    Operand(OperandRefusal),
    ExecLiteral(TextRange),
}

/// Physical source locations and semantic operations shared by complete and residual suites.
#[derive(Default)]
pub(crate) struct StatementSites {
    pub(crate) expressions: booleans::ExpressionSites,
    omit_assertions: bool,
}

pub(super) fn for_pull(loop_: &ast::StmtFor) -> Option<ir::AsyncPull> {
    loop_.is_async.then(|| ir::AsyncPull {
        range: loop_.iter.range(),
        header: Some(loop_.start()),
    })
}

/// Prepare a Python pull. An acquisition site belongs to this frame; without one
/// the defining frame has already obtained the iterator passed to the generator.
pub(super) fn cursor_input(
    value: Expr,
    acquire_at: Option<TextRange>,
    pull: Option<ir::AsyncPull>,
    hidden: impl FnMut(Type, Option<ir::IterNextKind>) -> usize,
) -> pipelines::Prepared {
    let (acquire, kind) = match pull {
        Some(site) => (
            acquire_at.map(ir::IteratorOp::Async),
            ir::IterNextKind::Async(site),
        ),
        None => (
            Some(acquire_at.map_or(ir::IteratorOp::LoopCursor, ir::IteratorOp::Loop)),
            ir::IterNextKind::Generic,
        ),
    };
    let cursor = match acquire {
        Some(op) => operation(Builtin::RegisteredIter(op), Type::PythonObject, vec![value]),
        None => value,
    };
    pipelines::Prepared::pull_cursor(cursor, Type::PythonObject, kind, hidden)
}

#[derive(Clone, Copy, Default)]
enum SuiteSelection {
    #[default]
    All,
    Physical,
}

/// Compounds lower in their source scope. A physical child owns the operands and
/// slots that could not be kept in the enclosing function.
#[derive(Default)]
struct CompoundSuites<'p, 'a> {
    physical: Option<&'p mut SuitePreparation<'a>>,
    selection: SuiteSelection,
    suspended: bool,
    operands: Vec<PhysicalOperand>,
    suites: Vec<RegisteredSuite>,
}

impl CompoundSuites<'_, '_> {
    fn block(
        &mut self,
        scope: &mut SourceScope<'_>,
        stmts: &[ast::Stmt],
        reachable: bool,
    ) -> StdResult<statements::Block, ()> {
        let Some(prepare) = self.physical.as_deref_mut() else {
            return scope.block(stmts);
        };
        if stmts.is_empty() {
            return Ok((Vec::new(), true));
        }
        let child = prepare.selected_suite(
            stmts,
            scope.kind,
            self.selection,
            self.selection,
            self.suspended,
            None,
        );
        if reachable {
            prepare.refusal = prepare.refusal.or(child.refusal);
        }
        let falls = child.falls_through;
        Ok((child.into_child(&mut self.suites), falls))
    }

    fn value(
        &mut self,
        scope: &mut SourceScope<'_>,
        expr: &ast::Expr,
        site: Option<(TextRange, TextSize)>,
    ) -> StdResult<Expr, ()> {
        let Some(prepare) = self.physical.as_deref_mut() else {
            return scope.frame_value(expr).ok_or(());
        };
        let value = scope.try_operand(|scope| scope.selected_operand(expr, self.selection));
        let value =
            prepare.region_operand(expr, scope.kind, &mut self.operands, value, self.suspended);
        Ok(match site {
            Some((range, at)) => values::located(range, at, value),
            None => value,
        })
    }

    fn binding_sites(&mut self, body: &[Statement]) {
        if let Some(prepare) = self.physical.as_deref_mut() {
            prepare.refusal = prepare
                .refusal
                .or_else(|| prepare.literal_refusal(RegionRoot::Suite(body)));
        }
    }

    fn condition(
        &mut self,
        scope: &mut SourceScope<'_>,
        expr: &ast::Expr,
        branch: bool,
    ) -> StdResult<Expr, ()> {
        let Some(prepare) = self.physical.as_deref_mut() else {
            return scope.condition_operand(expr).ok_or(());
        };
        if !branch && let ast::Expr::BooleanLiteral(value) = expr {
            return Ok(Expr::boolean(value.value));
        }
        let input = prepare.owned_operand(expr, scope.kind, self.suspended, |owner| {
            if branch && !self.suspended {
                owner.selected_operand(expr, self.selection)
            } else {
                owner.condition_operand(expr)
            }
        });
        Ok(bind_operand(&mut self.operands, input, Type::Bool))
    }

    fn branch(
        &mut self,
        scope: &mut SourceScope<'_>,
        branch: &ast::StmtIf,
        mut out: &mut Vec<Statement>,
    ) -> StdResult<bool, ()> {
        let mut falls = false;
        for (test, suite) in statements::if_clauses(branch) {
            // A dead arm in a suspended frame has no resume entry.
            let test = match test {
                Some(ast::Expr::BooleanLiteral(value)) if self.suspended => {
                    if !value.value {
                        continue;
                    }
                    None
                }
                test => test,
            };
            let condition = test
                .map(|test| self.condition(scope, test, true))
                .transpose()?;
            let (child, continues) = self.block(scope, suite, true)?;
            falls |= continues;
            if let Some(condition) = condition {
                let (yes, otherwise) = statements::push_if(out, condition);
                *yes = child;
                out = otherwise;
            } else {
                statements::append_block(out, child);
                return Ok(falls);
            }
        }
        Ok(true)
    }

    fn while_loop(
        &mut self,
        scope: &mut SourceScope<'_>,
        loop_: &ast::StmtWhile,
        out: &mut Vec<Statement>,
    ) -> StdResult<bool, ()> {
        let condition = self.condition(scope, &loop_.test, false)?;
        let disabled = ir::control::boolean_literal(&condition) == Some(false);
        let body = if disabled {
            Vec::new()
        } else {
            scope
                .in_loop(LoopScope::While, |scope| {
                    self.block(scope, &loop_.body, true)
                })?
                .0
        };
        let (otherwise, falls) = self.block(scope, &loop_.orelse, true)?;
        out.push(Statement::While(condition, body, otherwise));
        Ok(!disabled || falls && !mandatory_suite_exits(&loop_.orelse))
    }

    fn for_loop(
        &mut self,
        scope: &mut SourceScope<'_>,
        loop_: &ast::StmtFor,
        out: &mut Vec<Statement>,
    ) -> StdResult<bool, ()> {
        let pull = for_pull(loop_);
        let mut input = if self.physical.is_some() {
            let iter = self.value(scope, &loop_.iter, None)?;
            cursor_input(iter, Some(loop_.iter.range()), pull, |ty, cursor| {
                scope.reserve_cursor_slot(ty, cursor)
            })
        } else {
            scope
                .creating_iterable(|scope| scope.iteration_input(&loop_.iter, pull))
                .ok_or(())?
        };
        let binding = input
            .bind_loop_target(scope, &loop_.target)
            .inspect_err(|_| {
                if self.physical.is_some() {
                    scope.refusal = Some(scope.target_refusal(&loop_.target));
                }
            })?;
        self.binding_sites(binding);
        let (body, _) = scope.in_loop(LoopScope::WithCursor, |scope| {
            self.block(scope, &loop_.body, true)
        })?;
        let (otherwise, _) = self.block(scope, &loop_.orelse, true)?;
        statements::append_block(out, input.for_suite(body, otherwise, Vec::new()));
        Ok(true)
    }

    fn try_statement(
        &mut self,
        scope: &mut SourceScope<'_>,
        region: &ast::StmtTry,
        out: &mut Vec<Statement>,
    ) -> StdResult<bool, ()> {
        let (body, body_falls) = self.block(scope, &region.body, true)?;
        // Physical children retain their first reachable refusal in execution order.
        let early_else = self
            .physical
            .is_some()
            .then(|| self.block(scope, &region.orelse, body_falls))
            .transpose()?;
        let mut handlers = Vec::with_capacity(region.handlers.len());
        let mut handler_falls = false;
        for ast::ExceptHandler::ExceptHandler(handler) in &region.handlers {
            let matcher = exception_matcher(handler, region.is_star, |class, site| {
                self.value(scope, class, Some((site, class.start())))
            })?;
            let name = handler.name.as_ref().map(|name| scope.slot(name.as_str()));
            let (body, falls) = self.block(scope, &handler.body, true)?;
            handler_falls |= falls;
            handlers.push(ir::ExceptHandler {
                matcher,
                name,
                body,
            });
        }
        let (orelse, else_falls) = match early_else {
            Some(block) => block,
            None => self.block(scope, &region.orelse, body_falls)?,
        };
        let finalbody = self.block(scope, &region.finalbody, true)?;
        Ok(statements::finish_try(
            Some(out),
            body,
            handlers,
            orelse,
            body_falls && else_falls || handler_falls,
            finalbody,
        ))
    }

    fn with_statement(
        &mut self,
        scope: &mut SourceScope<'_>,
        stmt: &ast::StmtWith,
        out: &mut Vec<Statement>,
    ) -> StdResult<bool, ()> {
        let mut protected = out;
        for item in &stmt.items {
            let site = context_site(scope.source.text, &item.context_expr, stmt.is_async);
            let manager = self.value(scope, &item.context_expr, Some((site.range, site.anchor)))?;
            let report = scope.report_child_refusals;
            scope.report_child_refusals |= self.physical.is_some();
            let binding = item
                .optional_vars
                .as_deref()
                .map(|target| contexts::python_binding(scope, target))
                .transpose();
            scope.report_child_refusals = report;
            match binding {
                Ok(binding) => {
                    if let Some(binding) = &binding {
                        self.binding_sites(&binding.body);
                    }
                    protected = contexts::enter_python(protected, site, manager, binding);
                }
                Err(()) => {
                    let Some(prepare) = self.physical.as_deref_mut() else {
                        return Err(());
                    };
                    let target = item.optional_vars.as_deref().expect("refused with target");
                    let fault = scope.target_refusal(target);
                    prepare
                        .refusal
                        .get_or_insert(PreparedRefusal::Operand(fault));
                    contexts::enter_python(protected, site, manager, None)
                        .push(RegisteredSuite::bind_refusal(&mut self.operands, fault));
                    return Ok(true);
                }
            }
        }
        statements::append_block(protected, self.block(scope, &stmt.body, true)?.0);
        // A manager can suppress an exception from its protected suite.
        Ok(true)
    }
}

/// Prepare ordered suites while the source tree and its namespace operands are available.
pub(crate) struct SuitePreparation<'a> {
    call_operands: Option<&'a std::cell::RefCell<crate::dynamic::CallOperands>>,
    /// Import occurrences whose binding is lexical but whose instruction has no incoming edge.
    pub(crate) unreachable_imports: Option<&'a mut HashSet<TextRange>>,
    operations: &'a HashMap<TextRange, Expr>,
    source: values::RegisteredSource<'a>,
    pub(crate) declaration_refusals: HashMap<TextSize, PreparedRefusal>,
    pub(crate) literal_refusals: Option<&'a crate::dynamic::literal::LiteralSites>,
    pub(crate) profile_exclusions: Option<&'a HashMap<TextSize, String>>,
    refusal: Option<PreparedRefusal>,
    pub(crate) sites: StatementSites,
}

impl<'a> SuitePreparation<'a> {
    pub(crate) fn new(
        operations: &'a HashMap<TextRange, Expr>,
        source: values::RegisteredSource<'a>,
        assertions_enabled: bool,
    ) -> Self {
        Self {
            unreachable_imports: None,
            call_operands: None,
            operations,
            source,
            sites: StatementSites {
                omit_assertions: !assertions_enabled,
                ..StatementSites::default()
            },
            declaration_refusals: HashMap::new(),
            literal_refusals: None,
            profile_exclusions: None,
            refusal: None,
        }
    }

    fn source_scope(&self, kind: RegisteredScope) -> SourceScope<'a> {
        let mut scope = SourceScope::new(
            kind,
            self.source,
            Some(self.operations),
            self.sites.omit_assertions,
        );
        scope.call_operands = self.call_operands;
        scope
    }

    pub(crate) fn class_suite(
        &mut self,
        ClassSite {
            def: class,
            mut annotations,
        }: ClassSite<'_>,
        names: &imre_ast::Names,
        classes: &mut HashMap<TextSize, super::function_scope::classes::ClassScope>,
    ) {
        let start = class.start();
        let scope = classes.get_mut(&start).expect("class scope");
        let header = class.type_params.as_deref().map(|params| {
            let parameters =
                TypeParameterValues::prepare(params, names, self.source, scope.enclosing_class);
            let mut scope = SourceScope::new(
                RegisteredScope::Function { is_async: false },
                self.source,
                None,
                self.sites.omit_assertions,
            );
            scope.report_child_refusals = true;
            let args = class
                .arguments
                .as_deref()
                .into_iter()
                .flat_map(imre_ast::argument_values)
                .map(|expr| {
                    scope.refusal = None;
                    scope.operand(expr).ok_or_else(|| {
                        scope
                            .refusal
                            .unwrap_or((expr.range(), "expression has no checked operand"))
                    })
                })
                .collect::<StdResult<Vec<_>, _>>();
            let body = args.map(|args| {
                let value = operation(
                    Builtin::RegisteredClassNamespace(start, class_argument_layout(class)),
                    Type::PythonObject,
                    args,
                );
                scope.finish(RegisteredFunction::returning(class.name.range(), value))
            });
            ClassHeader { parameters, body }
        });
        let keyword = if class.decorator_list.is_empty() {
            start
        } else {
            // The class keyword is the last non-trivia token before its name.
            ruff_python_trivia::BackwardsTokenizer::up_to(class.name.start(), self.source.text, &[])
                .skip_trivia()
                .next()
                .expect("parsed class keyword")
                .start()
        };
        let range = TextRange::new(keyword, class.end());
        let body = self.suite(&class.body, RegisteredScope::Class);
        if let Some(refusal) = header
            .as_ref()
            .and_then(|header| match &header.body {
                Ok(provider) => self.literal_refusal(RegionRoot::Suite(&provider.function.body)),
                Err(reason) => Some(PreparedRefusal::Operand(*reason)),
            })
            .or(body.refusal)
        {
            self.declaration_refusals.insert(start, refusal);
        }
        if annotations.body.is_some() {
            annotations.reads = std::mem::take(&mut scope.annotation_reads);
            annotations.reads.insert("__classdict__".into());
            if annotations.conditional_count != 0 {
                annotations
                    .reads
                    .insert("__conditional_annotations__".into());
            }
        }
        scope.body = Some(RegisteredClass {
            annotations,
            name: class.name.clone(),
            range,
            doc: docstring(&class.body),
            body,
            header,
        });
    }

    pub(crate) fn suite(&mut self, stmts: &[ast::Stmt], kind: RegisteredScope) -> RegisteredSuite {
        self.selected_suite(
            stmts,
            kind,
            SuiteSelection::All,
            SuiteSelection::All,
            false,
            None,
        )
    }

    /// Place the executable tail only in its original module, after the same declarations.
    pub(crate) fn module(
        &mut self,
        stmts: &[ast::Stmt],
        entry: Option<(&crate::modules::SourceModule, Box<RegisteredFunction>)>,
    ) -> RegisteredSuite {
        if let Some((module, mut entry)) = entry
            && let Some((_, tail)) = module.executable_suffix()
            && let Some(prefix) = stmts.strip_suffix(tail)
            && entry.bind_python_arguments()
        {
            return self.selected_suite(
                prefix,
                RegisteredScope::Module,
                SuiteSelection::All,
                SuiteSelection::All,
                false,
                Some(entry),
            );
        }
        self.suite(stmts, RegisteredScope::Module)
    }

    pub(crate) fn interactive(&self, stmts: &[ast::Stmt]) -> Option<RegisteredSuite> {
        let mut scope = self.source_scope(RegisteredScope::Module);
        scope.interactive = true;
        let (body, falls_through) = scope.block(stmts).ok()?;
        let refusal = self.checked_declaration_refusal(&body);
        Some(RegisteredSuite {
            falls_through,
            refusal,
            ..RegisteredSuite::new(scope.finish(body), Vec::new())
        })
    }

    /// A residual statement in a suspended function owns its cursor and pending exits.
    /// Its child suites retain their physical-frame contract while its siblings use checked runs.
    pub(crate) fn function_suite(
        &mut self,
        stmts: &[ast::Stmt],
        kind: RegisteredScope,
        suspends: bool,
    ) -> RegisteredSuite {
        let children = if suspends {
            SuiteSelection::Physical
        } else {
            SuiteSelection::All
        };
        self.selected_suite(stmts, kind, SuiteSelection::All, children, suspends, None)
    }

    pub(crate) fn prepare_function<'p>(
        &mut self,
        header: &mut super::function_scope::NamedFunction<'p>,
        site: FunctionSite<'_>,
        checked: Option<&'p RegisteredFunction>,
        call_operands: Option<&'a std::cell::RefCell<crate::dynamic::CallOperands>>,
    ) {
        let def = site.def;
        header.type_params = site.parameters;
        header.annotations = site.annotations;
        // A checked frame can reference lexical activations prepared at this definition.
        let mut child_refusal = None;
        let checked = checked.filter(|body| {
            let (FrameInputs::Checked { sites }
            | FrameInputs::Deferred { sites, .. }
            | FrameInputs::Protocol { sites }) = &body.inputs;
            sites.iter().all(|site| match site {
                FrameSite::Method(declaration) => &**declaration == def,
                FrameSite::Lambda(start) => self
                    .source
                    .lambdas
                    .and_then(|lambdas| lambdas.get(start))
                    .is_some_and(|lambda| {
                        if lambda.body.is_some() {
                            return true;
                        }
                        if let ExpressionSelection::Refused(range, reason) = lambda.binding.as_ref()
                        {
                            child_refusal = Some(PreparedRefusal::Operand((*range, reason)));
                        }
                        false
                    }),
                FrameSite::Generator(range) => self
                    .source
                    .generators
                    .is_some_and(|generators| generators.contains_key(range)),
            })
        });
        let (body, refusal) = if let Some(checked) = checked {
            (
                NamedBody::Borrowed(checked),
                self.checked_declaration_refusal(&checked.function.body),
            )
        } else {
            let kind = RegisteredScope::Function {
                is_async: def.is_async,
            };
            let statements = function_text::executable_body(&def.body, header.doc.is_some());
            self.call_operands = call_operands;
            let suite =
                self.function_suite(statements, kind, header.metadata.suspension_flags() != 0);
            self.call_operands = None;
            let refusal = suite.refusal;
            (NamedBody::from_suite(suite), refusal)
        };
        // Only a module-level function with checked parameters fixes a scalar
        // argument. Other functions bind through their live annotation references.
        let fixed_parameters = match body.as_ref() {
            RegisteredBody::Function(frame)
                if site.module_scope && !matches!(frame.inputs, FrameInputs::Protocol { .. }) =>
            {
                Some(frame)
            }
            _ => None,
        };
        let bound_candidate = if refusal.is_none() && child_refusal.is_none() {
            match body.as_ref() {
                RegisteredBody::Function(frame) => Some(frame),
                RegisteredBody::Suite(_) => None,
            }
        } else {
            None
        };
        header.prepare_signature(def, site.thread_entry, fixed_parameters, bound_candidate);
        header.body = Some(body);
        // A residual suite may stop at a return before reaching a refused lexical child.
        if let Some(refusal) = refusal.or(child_refusal) {
            self.declaration_refusals.insert(def.start(), refusal);
        }
    }

    #[cfg(test)]
    pub(crate) fn physical_function_suite(
        &mut self,
        stmts: &[ast::Stmt],
        kind: RegisteredScope,
        suspended: bool,
    ) -> RegisteredSuite {
        let physical = SuiteSelection::Physical;
        self.selected_suite(stmts, kind, physical, physical, suspended, None)
    }

    fn prepare_child(
        &mut self,
        stmt: &ast::Stmt,
        kind: RegisteredScope,
        child_selection: SuiteSelection,
        suspended: bool,
    ) -> StdResult<RegisteredSuite, OperandRefusal> {
        if let ast::Stmt::Match(branch) = stmt {
            return self.prepare_match(branch, kind, suspended);
        }
        // The parent owns the loop checkpoint. A disabled loop has only its else frame.
        if let ast::Stmt::While(loop_) = stmt
            && disabled_while(loop_)
        {
            let mut suite = self.physical_suite(&loop_.orelse, kind, child_selection, suspended);
            suite.falls_through &= !mandatory_suite_exits(&loop_.orelse);
            return Ok(suite);
        }
        let mut scope = self.source_scope(kind);
        let mut children = CompoundSuites {
            physical: Some(self),
            selection: child_selection,
            suspended,
            ..CompoundSuites::default()
        };
        let mut body = match stmt {
            ast::Stmt::While(_) => Vec::new(),
            ast::Stmt::If(_) | ast::Stmt::With(_) => vec![Statement::Position(stmt.range(), None)],
            _ => vec![checkpoint(stmt)],
        };
        let falls = match stmt {
            ast::Stmt::If(branch) => children.branch(&mut scope, branch, &mut body),
            ast::Stmt::While(loop_) => children.while_loop(&mut scope, loop_, &mut body),
            ast::Stmt::For(loop_) => children.for_loop(&mut scope, loop_, &mut body),
            ast::Stmt::Try(region) => children.try_statement(&mut scope, region, &mut body),
            ast::Stmt::With(stmt) => children.with_statement(&mut scope, stmt, &mut body),
            _ => return Err((stmt.range(), "cannot compile this statement ahead of time")),
        }
        .map_err(|()| {
            scope
                .refusal
                .unwrap_or((stmt.range(), "cannot compile this statement ahead of time"))
        })?;
        Ok(RegisteredSuite {
            checked: scope.finish(body),
            suites: children.suites,
            operands: children.operands,
            falls_through: falls,
            refusal: None,
        })
    }

    pub(crate) fn checked_declaration_refusal(
        &self,
        block: &[Statement],
    ) -> Option<PreparedRefusal> {
        self.required_refusal(RegionRoot::Suite(block))
    }

    fn operand_refusal(&self, operand: &PhysicalOperand) -> Option<PreparedRefusal> {
        operand.refusal(self, |checked| &checked.value)
    }

    fn operand_refusals(&self, operands: &[PhysicalOperand]) -> Option<PreparedRefusal> {
        operands
            .iter()
            .find_map(|input| self.operand_refusal(input))
    }

    fn literal_refusal(&self, root: RegionRoot<'_>) -> Option<PreparedRefusal> {
        if self.literal_refusals.is_some() {
            self.required_refusal(root)
        } else {
            None
        }
    }

    fn required_refusal(&self, root: RegionRoot<'_>) -> Option<PreparedRefusal> {
        self.check_required_sites(root, false).err()
    }

    /// A successful check returns whether the declaration scanner reaches the next statement.
    fn check_required_sites(
        &self,
        root: RegionRoot<'_>,
        physical: bool,
    ) -> StdResult<bool, PreparedRefusal> {
        let has_refusals = !self.declaration_refusals.is_empty() || self.literal_refusals.is_some();
        if !physical && !has_refusals {
            return Ok(true);
        }
        let mut refusal = None;
        let mut visit = has_refusals.then_some(|site| {
            refusal = refusal.or_else(|| match site {
                RequiredSite::Definition(start)
                | RequiredSite::DefinitionAfterInputs(start)
                | RequiredSite::ClassNamespace(start) => {
                    self.declaration_refusals.get(&start).copied()
                }
                // Each activation was prepared from its lexical children before its binding.
                RequiredSite::Lambda(start) if self.literal_refusals.is_some() => {
                    let body = self.source.lambdas?.get(&start)?.body.as_ref()?;
                    self.checked_declaration_refusal(&body.function.body)
                }
                RequiredSite::Generator(range) if self.literal_refusals.is_some() => {
                    let generator = self.source.generators?.get(&range)?;
                    self.checked_declaration_refusal(&generator.body.function.body)
                }
                RequiredSite::ExecLiteral(site) => self
                    .literal_refusals?
                    .refused(site)
                    .then_some(PreparedRefusal::ExecLiteral(site)),
                _ => None,
            });
        });
        let continues = match root {
            RegionRoot::Suite(block) => {
                RequiredSite::visit_block(block, physical, self.literal_refusals, &mut visit)
            }
            RegionRoot::Operand(value) => {
                RequiredSite::visit_operand(value, self.literal_refusals, &mut visit);
                true
            }
        };
        refusal.map_or(Ok(continues), Err)
    }

    /// Prepare the operand in the scope that already owns its consuming operation.
    fn region_operand(
        &mut self,
        expr: &ast::Expr,
        kind: RegisteredScope,
        operands: &mut Vec<PhysicalOperand>,
        checked: Option<Expr>,
        suspended: bool,
    ) -> Expr {
        if let Some(value) = checked {
            self.refusal = self
                .refusal
                .or_else(|| self.literal_refusal(RegionRoot::Operand(&value)));
            return value;
        }
        let value = PhysicalOperand::Residual(self.owned_input(expr, kind, suspended));
        self.refusal = self.refusal.or_else(|| self.operand_refusal(&value));
        bind_operand(operands, value, Type::PythonObject)
    }

    fn owned_operand(
        &mut self,
        expr: &ast::Expr,
        kind: RegisteredScope,
        suspended: bool,
        lower: impl FnOnce(&mut SourceScope<'_>) -> Option<Expr>,
    ) -> PhysicalOperand {
        let operand = RegisteredOperand::prepare_with(self.source, kind, lower);
        let input = self.retain_operand(expr, kind, suspended, operand);
        self.refusal = self.refusal.or_else(|| self.operand_refusal(&input));
        input
    }

    fn retain_operand(
        &self,
        expr: &ast::Expr,
        kind: RegisteredScope,
        suspended: bool,
        operand: Option<RegisteredOperand>,
    ) -> PhysicalOperand {
        match operand {
            Some(value) => PhysicalOperand::Checked(Box::new(value)),
            None => PhysicalOperand::Residual(self.owned_input(expr, kind, suspended)),
        }
    }

    fn physical_suite(
        &mut self,
        stmts: &[ast::Stmt],
        kind: RegisteredScope,
        selection: SuiteSelection,
        suspended: bool,
    ) -> RegisteredSuite {
        let child = self.selected_suite(stmts, kind, selection, selection, suspended, None);
        self.refusal = self.refusal.or(child.refusal);
        child
    }

    fn region(&self, kind: RegisteredScope) -> RegisteredSuite {
        let scope = self.source_scope(kind);
        RegisteredSuite::new(scope.finish(Vec::new()), Vec::new())
    }

    fn selected_suite(
        &mut self,
        stmts: &[ast::Stmt],
        kind: RegisteredScope,
        select: SuiteSelection,
        child_selection: SuiteSelection,
        suspended: bool,
        mut placed: Option<Box<RegisteredFunction>>,
    ) -> RegisteredSuite {
        let mut scope = self.source_scope(kind);
        if let Some(reason) = stmts
            .first()
            .and_then(|first| self.profile_exclusions?.get(&first.start()))
        {
            let exception = Expr {
                ty: Type::Exception,
                kind: ExprKind::Exception {
                    kind: exception_kind("ValueError").unwrap(),
                    message: Some(Box::new(Expr {
                        ty: Type::Str,
                        kind: ExprKind::Str(reason.clone()),
                    })),
                },
            };
            let mut suite =
                RegisteredSuite::new(scope.finish(vec![Statement::raise(exception)]), Vec::new());
            suite.falls_through = false;
            return suite;
        }
        let mut statements = Vec::new();
        if let Some(entry) = &mut placed {
            // The caller reserved space for these declarations after its checked statements.
            scope.locals = std::mem::take(&mut entry.function.locals);
            statements = std::mem::take(&mut entry.function.body);
        }
        let tail_len = statements.len();
        let mut body = SuiteBody {
            statements,
            check_from: Some(tail_len),
            falls_through: true,
            refusal: None,
        };
        let mut suites = Vec::new();
        let mut operands = Vec::new();
        for stmt in stmts {
            // A non-raise transfer has already ended the declaration walk.
            // Avoid constructing code objects for declarations that cannot bind.
            if !body.falls_through
                && body.check_from.is_none()
                && matches!(stmt, ast::Stmt::FunctionDef(_) | ast::Stmt::ClassDef(_))
            {
                continue;
            }
            if let Some(transfer) = statements::loop_transfer(stmt) {
                body.statements.push(transfer);
                body.falls_through = false;
                continue;
            }
            let slots_before = scope.locals.len();
            scope.refusal = None;
            let profiled = !matches!(stmt, ast::Stmt::FunctionDef(_) | ast::Stmt::ClassDef(_))
                && self.profile_exclusions.is_some_and(|exclusions| {
                    exclusions.keys().any(|start| stmt.range().contains(*start))
                });
            let physical = profiled
                || matches!(select, SuiteSelection::Physical)
                || (suspended
                    && matches!(
                        stmt,
                        ast::Stmt::If(_) | ast::Stmt::While(_) | ast::Stmt::Try(_)
                    ));
            let inline = !profiled && (!physical || scope.checked_physical_statement(stmt));
            let lowered = if profiled {
                Err(())
            } else if !inline && !stmt.is_assign_stmt() {
                scope.physical_statement(stmt, &mut body, self)
            } else {
                let first = operands.len();
                let enclosing = self.refusal.take();
                let body_start = body.statements.len();
                let mut group = stmt
                    .as_ann_assign_stmt()
                    .is_some_and(|ann| ann.value.is_none());
                let lowered =
                    scope.written_statement(stmt, &mut body.statements, false, |scope, input| {
                        if let StatementOperand::Result(expr) = input {
                            let checked = scope.try_operand(|scope| scope.frame_value(expr));
                            Some(self.region_operand(expr, kind, &mut operands, checked, suspended))
                        } else {
                            group = true;
                            if inline { input.lower(scope) } else { None }
                        }
                    });
                let lowered = if lowered.is_err() && group {
                    scope.truncate_locals(slots_before);
                    scope.written_statement(stmt, &mut body.statements, true, |_, input| {
                        let held =
                            self.owned_operand(input.expression(), kind, suspended, |owner| {
                                input.retained(owner, suspended)
                            });
                        Some(match input {
                            StatementOperand::Assignment(_) => self.bind_value(&mut operands, held),
                            StatementOperand::Condition(_) => {
                                bind_operand(&mut operands, held, Type::Bool)
                            }
                            _ => bind_operand(&mut operands, held, Type::PythonObject),
                        })
                    })
                } else {
                    lowered
                };
                let lowered = lowered.or_else(|()| {
                    imre_ast::assignment_parts(stmt).ok_or(())?;
                    scope.truncate_locals(slots_before);
                    operands.truncate(first);
                    let fault = scope.refusal.unwrap_or((
                        stmt.range(),
                        "cannot compile this assignment target ahead of time",
                    ));
                    self.refusal = Some(PreparedRefusal::Operand(fault));
                    body.statements.push(checkpoint(stmt));
                    Ok((
                        Some(RegisteredSuite::bind_refusal(&mut operands, fault)),
                        true,
                    ))
                });
                let refusal = std::mem::replace(&mut self.refusal, enclosing);
                match lowered {
                    Ok((operation, continues)) => {
                        if operands.len() != first {
                            body.check_through(self, body_start);
                            if body.check_from.is_some() {
                                body.refusal = body.refusal.or(refusal);
                            }
                        }
                        body.statements.extend(operation);
                        Ok(continues)
                    }
                    Err(()) => {
                        operands.truncate(first);
                        Err(())
                    }
                }
            };
            if let Ok(continues) = lowered {
                if !body.falls_through
                    && let Some(imports) = &mut self.unreachable_imports
                    && matches!(stmt, ast::Stmt::Import(_) | ast::Stmt::ImportFrom(_))
                {
                    imports.insert(stmt.range());
                }
                if !continues {
                    body.falls_through = false;
                    // A checked raise retains the surrounding declaration diagnostics.
                    if !stmt.is_raise_stmt() {
                        body.check_run(self);
                    }
                }
                continue;
            }
            scope.truncate_locals(slots_before);
            if matches!(stmt, ast::Stmt::While(_)) {
                body.statements.push(checkpoint(stmt));
            }
            body.check_run(self);
            let enclosing = self.refusal.take();
            let child = self.prepare_child(stmt, kind, child_selection, suspended);
            let refusal = std::mem::replace(&mut self.refusal, enclosing);
            if body.check_from.is_some() {
                body.refusal = body
                    .refusal
                    .or(refusal)
                    .or_else(|| child.as_ref().err().copied().map(PreparedRefusal::Operand));
            }
            match child {
                Ok(child) => {
                    body.falls_through &= child.falls_through;
                    body.statements.extend(child.into_child(&mut suites));
                }
                Err(refusal) => body
                    .statements
                    .push(RegisteredSuite::bind_refusal(&mut operands, refusal)),
            }
            // The child checked its own declarations in its physical scope.
            body.check_from = body.falls_through.then_some(body.statements.len());
        }
        body.check_run(self);
        let checked = if let Some(mut entry) = placed {
            // Bind the saved targets where these declarations installed their names.
            // The written global lookup stays live when this scope has no local binding.
            for attribute in entry
                .fixed_attributes
                .iter_mut()
                .flat_map(|attrs| attrs.values_mut())
            {
                if let FixedAttribute::Call(Some(target), _) = attribute
                    && let ExprKind::Builtin(Builtin::RegisteredLocation(..), operands) =
                        &mut target.kind
                    && let [binding] = operands.as_mut_slice()
                    && let ExprKind::Builtin(Builtin::RegisteredName(name), _) = &binding.kind
                    && let Some(value) = scope.local_read(name)
                {
                    *binding = value;
                }
            }
            if body.falls_through && body.refusal.is_none() {
                body.refusal = self.checked_declaration_refusal(&body.statements[..tail_len]);
            }
            body.statements.rotate_left(tail_len);
            let placed_from = body.statements.len() - tail_len;
            scope.namespaces.extend(entry.local_names);
            let mut checked = scope.finish(body.statements);
            checked.placed_from = placed_from;
            checked.fixed_sites = entry.fixed_sites;
            checked.fixed_attributes = entry.fixed_attributes;
            checked
        } else {
            scope.finish(body.statements)
        };
        RegisteredSuite {
            checked,
            operands,
            suites,
            falls_through: body.falls_through,
            refusal: body.refusal,
        }
    }
}

pub(super) fn exception_matcher<E>(
    handler: &ast::ExceptHandlerExceptHandler,
    is_star: bool,
    operand: impl FnOnce(&ast::Expr, TextRange) -> StdResult<Expr, E>,
) -> StdResult<ir::ExceptionMatcher, E> {
    let Some(class) = handler.type_.as_deref() else {
        return Ok(ir::ExceptionMatcher::Any);
    };
    if !is_star && matches!(class, ast::Expr::Tuple(tuple) if tuple.elts.is_empty()) {
        return Ok(ir::ExceptionMatcher::Categories(Vec::new()));
    }
    let site = if is_star {
        handler.range()
    } else {
        class.range()
    };
    let operand = Box::new(operand(class, site)?);
    Ok(if is_star {
        ir::ExceptionMatcher::Subgroup(site, operand)
    } else {
        ir::ExceptionMatcher::Object(site, operand)
    })
}

/// A target's Python expression inputs in evaluation order. The receiver is evaluated
/// once, including for an in-place operation that both reads and writes its descriptor.
pub(crate) struct AttributeEffect<'a> {
    pub(crate) action: ir::AttributeAction,
    pub(crate) field: &'a ast::ExprAttribute,
    value: Option<&'a ast::Expr>,
}

impl<'a> AttributeEffect<'a> {
    pub(super) fn statement(&self, operands: Vec<Expr>) -> Statement {
        attribute_statement(self.action, self.field, operands)
    }

    pub(crate) fn operands(&self) -> impl Iterator<Item = &'a ast::Expr> {
        let receiver = Some(self.field.value.as_ref());
        if self.action == ir::AttributeAction::Store {
            [self.value, receiver]
        } else {
            [receiver, self.value]
        }
        .into_iter()
        .flatten()
    }
}

pub(super) fn attribute_statement(
    action: ir::AttributeAction,
    field: &ast::ExprAttribute,
    values: Vec<Expr>,
) -> Statement {
    Statement::Expr(operation(
        attribute_opcode(action, field),
        Type::None,
        values,
    ))
}

pub(super) fn attribute_opcode(action: ir::AttributeAction, field: &ast::ExprAttribute) -> Builtin {
    Builtin::RegisteredAttribute {
        action,
        name: field.attr.clone(),
        range: field.range(),
        hint: None,
    }
}

/// Deletion groups evaluate their leaves from left to right without constructing a tuple or list.
fn delete_targets(delete: &ast::StmtDelete) -> Vec<&ast::Expr> {
    let mut remaining: Vec<_> = delete.targets.iter().rev().collect();
    let mut leaves = Vec::new();
    while let Some(target) = remaining.pop() {
        match target {
            ast::Expr::Tuple(group) => remaining.extend(group.elts.iter().rev()),
            ast::Expr::List(group) => remaining.extend(group.elts.iter().rev()),
            _ => leaves.push(target),
        }
    }
    leaves
}

/// Resolve the implicit manager protocol's source anchor while its syntax and
/// original source text are available. LLVM only converts accepted offsets to columns.
pub(super) fn context_site(source: &str, manager: &ast::Expr, is_async: bool) -> ir::ContextSite {
    let range = manager.range();
    let delimiter = match manager {
        ast::Expr::Call(call) => Some((call.func.end(), b'(')),
        ast::Expr::Subscript(index) => Some((index.value.end(), b'[')),
        _ => None,
    };
    let anchor = delimiter
        .and_then(|(prefix, opener)| {
            imre_ast::traceback_delimiter(
                source.as_bytes(),
                usize::from(prefix),
                usize::from(range.end()),
                opener,
            )
        })
        .map_or(range.start(), |offset| TextSize::try_from(offset).unwrap());
    ir::ContextSite {
        is_async,
        range,
        anchor,
    }
}

pub(super) fn attribute_write(stmt: &ast::Stmt) -> Option<AttributeEffect<'_>> {
    use ir::AttributeAction::{InPlace, Store};
    let (target, action, value) = match stmt {
        ast::Stmt::Assign(assign) => {
            let [target] = assign.targets.as_slice() else {
                return None;
            };
            (target, Store, Some(assign.value.as_ref()))
        }
        ast::Stmt::AnnAssign(assign) => (
            assign.target.as_ref(),
            Store,
            Some(assign.value.as_deref()?),
        ),
        ast::Stmt::AugAssign(assign) => (
            assign.target.as_ref(),
            InPlace(assign.op.as_str()),
            Some(assign.value.as_ref()),
        ),
        _ => return None,
    };
    Some(AttributeEffect {
        action,
        field: target.as_attribute_expr()?,
        value,
    })
}

pub(super) fn attribute_effects(stmt: &ast::Stmt) -> Option<Vec<AttributeEffect<'_>>> {
    let ast::Stmt::Delete(delete) = stmt else {
        return Some(vec![attribute_write(stmt)?]);
    };
    delete_targets(delete)
        .into_iter()
        .map(|target| {
            Some(AttributeEffect {
                action: ir::AttributeAction::Delete,
                field: target.as_attribute_expr()?,
                value: None,
            })
        })
        .collect()
}

pub(crate) fn delete_item(receiver: Expr, key: Expr) -> Expr {
    operation(Builtin::DeleteItem, Type::None, vec![receiver, key])
}

pub(super) fn raised_value<E>(
    raise: &ast::StmtRaise,
    value: &ast::Expr,
    mut operand: impl FnMut(&ast::Expr) -> StdResult<Expr, E>,
) -> StdResult<Statement, E> {
    let exception = operand(value)?;
    let cause = raise.cause.as_deref().map(operand).transpose()?;
    Ok(Statement::raise_at(exception, cause, None))
}

// Nested suites own their operand placement. The reader supplies this statement's inputs.
impl<'s, F> statements::StatementLowering for (&mut SourceScope<'s>, F, bool)
where
    F: FnMut(&mut SourceScope<'s>, StatementOperand<'_>) -> Option<Expr>,
{
    type Error = ();

    fn assignment(
        &mut self,
        annotation: Option<&ast::StmtAnnAssign>,
        targets: &[ast::Expr],
        rhs: &ast::Expr,
        body: &mut Vec<Statement>,
    ) -> StdResult<bool, ()> {
        self.0
            .assign(annotation, targets, rhs, body, &mut self.1, self.2)?;
        Ok(true)
    }

    fn branch(&mut self, branch: &ast::StmtIf, output: &mut Vec<Statement>) -> StdResult<bool, ()> {
        CompoundSuites::default().branch(self.0, branch, output)
    }

    fn definition(
        &mut self,
        def: &ast::StmtFunctionDef,
        body: &mut Vec<Statement>,
    ) -> StdResult<Option<Statement>, ()> {
        let (scope, read, _) = self;
        // Decorators and parameter defaults execute in the enclosing frame, including for generics.
        let header = scope
            .with_binding_operands(|scope| {
                imre_ast::function_definition_expressions(def)
                    .map(|expr| {
                        let value = read(scope, StatementOperand::Suspendable(expr))?;
                        if !matches!(
                            value.kind,
                            ExprKind::Builtin(Builtin::RegisteredInput(_), _)
                        ) {
                            return Some(value);
                        }
                        // A later default may suspend. Keep this value until the definition binds.
                        let slot = scope.hidden(Type::PythonObject);
                        body.push(Statement::Assign(slot, value));
                        Some(unpacking::take_pattern_temporary(
                            Expr::local(slot, Type::PythonObject),
                            slot,
                        ))
                    })
                    .collect::<Option<Vec<_>>>()
            })
            .ok_or(())?;
        Ok(Some(scope.store(
            def.name.as_str(),
            function_definition(def, header),
        )))
    }

    fn effect(&mut self, value: &ast::Expr) -> StdResult<(Option<Statement>, bool), ()> {
        let (scope, read, _) = self;
        if !scope.interactive && value.is_string_literal_expr() {
            return Ok((None, true));
        }
        let value = read(scope, StatementOperand::Result(value)).ok_or(())?;
        let effect = if scope.interactive {
            operation(Builtin::RegisteredDisplayHook, Type::None, vec![value])
        } else {
            value
        };
        Ok((Some(Statement::Expr(effect)), true))
    }

    fn returned(&mut self, ret: &ast::StmtReturn) -> StdResult<Statement, ()> {
        let (scope, read, _) = self;
        Ok(Statement::Return(match ret.value.as_deref() {
            Some(value) => read(scope, StatementOperand::Result(value)).ok_or(())?,
            None => Expr::none(),
        }))
    }

    fn for_loop(&mut self, loop_: &ast::StmtFor) -> StdResult<statements::Block, ()> {
        let mut body = Vec::new();
        let falls = CompoundSuites::default().for_loop(self.0, loop_, &mut body)?;
        Ok((body, falls))
    }

    fn raised_value(
        &mut self,
        raise: &ast::StmtRaise,
        value: &ast::Expr,
    ) -> StdResult<Statement, ()> {
        let (scope, read, _) = self;
        raised_value(raise, value, |expr| {
            read(scope, StatementOperand::Result(expr)).ok_or(())
        })
    }

    fn record_loop_transfer(&mut self, _: &Statement) -> bool {
        self.0.loop_scope != LoopScope::Outside
    }

    fn other_statement(
        &mut self,
        stmt: &ast::Stmt,
        output: &mut Vec<Statement>,
    ) -> StdResult<bool, ()> {
        self.0.other_statement(stmt, output, &mut self.1, self.2)
    }
}

impl SourceScope<'_> {
    fn other_statement(
        &mut self,
        stmt: &ast::Stmt,
        output: &mut Vec<Statement>,
        read: &mut impl FnMut(&mut Self, StatementOperand<'_>) -> Option<Expr>,
        retained: bool,
    ) -> StdResult<bool, ()> {
        let (statements, continues) = match stmt {
            ast::Stmt::AugAssign(assign) => {
                return self.with_suspending_operands(|scope| {
                    let statement = if let Some(effect) = attribute_write(stmt) {
                        let values = effect
                            .operands()
                            .map(|node| read(scope, StatementOperand::Suspendable(node)))
                            .collect::<Option<_>>()
                            .ok_or(())?;
                        effect.statement(values)
                    } else {
                        scope
                            .augmented_assignment(assign, |input, scope| read(scope, input))
                            .ok_or(())?
                    };
                    output.push(statement);
                    Ok(true)
                });
            }
            ast::Stmt::TypeAlias(alias) => {
                let name = alias.name.as_name_expr().ok_or(())?;
                let value = operation(
                    Builtin::RegisteredTypeAlias(alias.start()),
                    Type::PythonObject,
                    vec![],
                );
                Ok((vec![self.store(name.id.as_str(), value)], true))
            }
            ast::Stmt::Match(branch) => self.match_statement(branch),
            ast::Stmt::ClassDef(class) => {
                let operands = self
                    .with_binding_operands(|scope| {
                        imre_ast::class_definition_expressions(class)
                            .map(|expr| read(scope, StatementOperand::ClassHeader(expr)))
                            .collect::<Option<_>>()
                    })
                    .ok_or(())?;
                let binding = self.store(class.name.as_str(), class_definition(class, operands));
                Ok((vec![binding], true))
            }
            ast::Stmt::Try(region) => {
                return CompoundSuites::default().try_statement(self, region, output);
            }
            ast::Stmt::With(stmt) if !stmt.is_async || self.is_async_function() => {
                return CompoundSuites::default().with_statement(self, stmt, output);
            }
            ast::Stmt::While(loop_) => {
                return CompoundSuites::default().while_loop(self, loop_, output);
            }
            ast::Stmt::Global(_) | ast::Stmt::Nonlocal(_) => Ok((Vec::new(), true)),
            ast::Stmt::AnnAssign(ann) => {
                if ann.value.is_none()
                    && let Some(inputs) = values::bare_target_operands(&ann.target)
                {
                    let operands = inputs
                        .into_iter()
                        .map(|expr| read(self, StatementOperand::Frame(expr)))
                        .collect::<Option<_>>()
                        .ok_or(())?;
                    let effect = operation(Builtin::RegisteredDiscard, Type::None, operands);
                    output.push(Statement::Expr(effect));
                    return Ok(true);
                }
                let mut body = Vec::new();
                if let Some(value) = &ann.value {
                    self.assign(
                        Some(ann),
                        std::slice::from_ref(ann.target.as_ref()),
                        value,
                        &mut body,
                        read,
                        retained,
                    )?;
                }
                // A simple namespace annotation records its declaration when the provider is present.
                if !matches!(self.kind, RegisteredScope::Function { .. }) && ann.simple {
                    match self
                        .suite_operations
                        .and_then(|operations| operations.get(&ann.target.range()))
                    {
                        Some(marker) => body.push(Statement::Expr(marker.clone())),
                        None if !retained => return Err(()),
                        None => {}
                    }
                }
                Ok((body, true))
            }
            ast::Stmt::Delete(delete) => {
                let body = delete_targets(delete)
                    .into_iter()
                    .map(|target| {
                        let (op, inputs) = deletion(target);
                        let slots = self.locals.len();
                        let values = inputs
                            .clone()
                            .map(|input| input.in_binding(self))
                            .collect::<Option<Vec<_>>>()
                            .or_else(|| {
                                self.truncate_locals(slots);
                                inputs
                                    .map(|input| {
                                        self.with_binding_operands(|scope| read(scope, input))
                                    })
                                    .collect()
                            })?;
                        Some(Statement::Expr(operation(op, Type::None, values)))
                    })
                    .collect::<Option<_>>()
                    .ok_or(())?;
                Ok((body, true))
            }
            _ => {
                output.push(registered_import(stmt, self.source.private_names).ok_or(())?);
                return Ok(true);
            }
        }?;
        statements::append_block(output, statements);
        Ok(continues)
    }
}

#[derive(Clone, Copy)]
enum StatementOperand<'a> {
    Assignment(&'a ast::Expr),
    Result(&'a ast::Expr),
    Frame(&'a ast::Expr),
    Condition(&'a ast::Expr),
    Value(&'a ast::Expr),
    Suspendable(&'a ast::Expr),
    ClassHeader(&'a ast::Expr),
    Index(&'a ast::Expr),
}

impl<'a> StatementOperand<'a> {
    fn expression(self) -> &'a ast::Expr {
        match self {
            Self::Assignment(expr)
            | Self::Result(expr)
            | Self::Frame(expr)
            | Self::Condition(expr)
            | Self::Value(expr)
            | Self::Suspendable(expr)
            | Self::ClassHeader(expr)
            | Self::Index(expr) => expr,
        }
    }

    fn retained(self, owner: &mut SourceScope<'_>, suspended: bool) -> Option<Expr> {
        match self {
            Self::Assignment(expr) if !suspended => owner.ordinary_operand(expr),
            Self::ClassHeader(expr) => owner.operand(expr),
            Self::Suspendable(expr) if suspended => owner.frame_value(expr),
            _ => self.lower(owner),
        }
    }

    /// Read the value or key where its binding will run, including a suspension in that frame.
    fn in_binding(self, scope: &mut SourceScope<'_>) -> Option<Expr> {
        scope.with_binding_operands(|scope| self.lower(scope))
    }

    fn lower(self, scope: &mut SourceScope<'_>) -> Option<Expr> {
        match self {
            Self::Assignment(expr) | Self::Frame(expr) | Self::Result(expr) => {
                scope.frame_value(expr)
            }
            Self::Condition(expr) => scope.condition_operand(expr),
            Self::Value(expr) | Self::Suspendable(expr) | Self::ClassHeader(expr) => {
                scope.ordinary_operand(expr)
            }
            Self::Index(key) => {
                values::index_key(key, &mut |expr| scope.ordinary_operand(expr).ok_or(())).ok()
            }
        }
    }
}

fn deletion(target: &ast::Expr) -> (Builtin, impl Iterator<Item = StatementOperand<'_>> + Clone) {
    use StatementOperand::{Index, Value};
    let (op, inputs) = match target {
        ast::Expr::Name(name) => (
            Builtin::RegisteredDeleteName(name.id.to_string(), name.range()),
            [None, None],
        ),
        ast::Expr::Attribute(field) => (
            attribute_opcode(ir::AttributeAction::Delete, field),
            [Some(Value(&field.value)), None],
        ),
        ast::Expr::Subscript(sub) => (
            Builtin::DeleteItem,
            [Some(Value(&sub.value)), Some(Index(&sub.slice))],
        ),
        _ => unreachable!("validated deletion target"),
    };
    (op, inputs.into_iter().flatten())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LoopScope {
    Outside,
    While,
    WithCursor,
}

struct SourceScope<'a> {
    kind: RegisteredScope,
    interactive: bool,
    omit_assertions: bool,
    // Only source statement suites own namespace operations; expression providers have none.
    suite_operations: Option<&'a HashMap<TextRange, Expr>>,
    call_operands: Option<&'a std::cell::RefCell<crate::dynamic::CallOperands>>,
    source: values::RegisteredSource<'a>,
    names: HashMap<String, usize>,
    namespaces: Vec<(usize, LocalBinding)>,
    comprehension: Option<eager::Namespace>,
    generator_expression: bool,
    locals: Vec<Type>,
    cursors: HashMap<usize, ir::IterNextKind>,
    loop_scope: LoopScope,
    suspending_operands: bool,
    refusal: Option<(TextRange, &'static str)>,
    // Nested dictionaries own their diagnostic. Other required operands report a refused child.
    report_child_refusals: bool,
}

const FORMATTED_STRING_REFUSAL: &str = "cannot compile this formatted string ahead of time";
const GENERATOR_REFUSAL: &str = "cannot compile this generator expression ahead of time";
const COMPREHENSION_REFUSAL: &str = "cannot compile this comprehension ahead of time";

/// Prepare an inline expression that can require private slots in the enclosing activation.
pub(crate) fn inline_operand(
    expr: &ast::Expr,
    source: values::RegisteredSource<'_>,
    kind: RegisteredScope,
    native_suspension: bool,
) -> StdResult<Option<RegisteredOperand>, (TextRange, &'static str)> {
    let reason = match expr {
        ast::Expr::List(_)
        | ast::Expr::Tuple(_)
        | ast::Expr::Set(_)
        | ast::Expr::Dict(_)
        | ast::Expr::Named(_)
        | ast::Expr::TString(_) => {
            let reason = match expr {
                ast::Expr::Dict(_) => "cannot compile this dictionary display ahead of time",
                ast::Expr::Named(_) => "cannot compile this assignment expression ahead of time",
                ast::Expr::TString(_) => "cannot compile this template ahead of time",
                _ => "cannot compile this collection display ahead of time",
            };
            return RegisteredOperand::prepare_required(expr, source, kind, reason, |scope| {
                if expr.is_named_expr() || expr.is_t_string_expr() {
                    scope.report_child_refusals = true;
                } else {
                    scope.suspending_operands = native_suspension;
                }
            })
            .map(Some);
        }
        ast::Expr::FString(_) => FORMATTED_STRING_REFUSAL,
        ast::Expr::Generator(_) => GENERATOR_REFUSAL,
        ast::Expr::ListComp(_) | ast::Expr::SetComp(_) | ast::Expr::DictComp(_) => {
            COMPREHENSION_REFUSAL
        }
        _ => return Ok(None),
    };
    RegisteredOperand::prepare(expr, source, kind)
        .map(Some)
        .ok_or((expr.range(), reason))
}

impl unpacking::PatternBinder for SourceScope<'_> {
    type Error = ();

    fn pattern_hidden(&mut self, ty: Type) -> usize {
        self.hidden(ty)
    }

    fn pattern_local(&self, slot: usize) -> Expr {
        Expr::local(slot, self.locals[slot])
    }

    fn pattern_store(
        &mut self,
        target: &ast::Expr,
        value: Expr,
        output: &mut Vec<Statement>,
    ) -> StdResult<(), ()> {
        let store = match target {
            ast::Expr::Name(name) => self.store(name.id.as_str(), value),
            ast::Expr::Attribute(attr) => {
                let receiver = StatementOperand::Value(&attr.value)
                    .in_binding(self)
                    .ok_or(())?;
                attribute_statement(ir::AttributeAction::Store, attr, vec![value, receiver])
            }
            ast::Expr::Subscript(sub) => {
                let receiver = StatementOperand::Value(&sub.value)
                    .in_binding(self)
                    .ok_or(())?;
                let key = StatementOperand::Index(&sub.slice)
                    .in_binding(self)
                    .ok_or(())?;
                Statement::assign_item(receiver, key, value)
            }
            _ => return Err(()),
        };
        output.push(store);
        Ok(())
    }

    fn multiple_stars(&self, _: &ast::Expr) {}
}

impl<'a> SourceScope<'a> {
    fn new(
        kind: RegisteredScope,
        source: values::RegisteredSource<'a>,
        suite_operations: Option<&'a HashMap<TextRange, Expr>>,
        omit_assertions: bool,
    ) -> Self {
        Self {
            kind,
            interactive: false,
            source,
            suite_operations,
            call_operands: None,
            omit_assertions,
            names: HashMap::new(),
            namespaces: Vec::new(),
            comprehension: None,
            generator_expression: false,
            locals: Vec::new(),
            cursors: HashMap::new(),
            loop_scope: LoopScope::Outside,
            suspending_operands: false,
            refusal: None,
            report_child_refusals: false,
        }
    }

    fn bindings(self) -> RegisteredBindings {
        let names = self
            .names
            .into_iter()
            .map(|(name, slot)| {
                (
                    slot,
                    LocalBinding {
                        name,
                        namespace: None,
                    },
                )
            })
            .chain(self.namespaces)
            .collect();
        let slots = FrameSlots::from_locals(self.locals.len(), &names, self.cursors);
        RegisteredBindings {
            locals: self.locals,
            names,
            slots,
        }
    }

    /// Discard bindings allocated by operands before a later operand failed to lower.
    fn truncate_locals(&mut self, len: usize) {
        if self.locals.len() != len {
            self.locals.truncate(len);
            self.cursors.retain(|slot, _| *slot < len);
            self.names.retain(|_, slot| *slot < len);
            self.namespaces.retain(|(slot, _)| *slot < len);
        }
    }

    fn try_operand(&mut self, read: impl FnOnce(&mut Self) -> Option<Expr>) -> Option<Expr> {
        let start = self.locals.len();
        let value = read(self);
        self.refusal = None;
        if value.is_none() {
            self.truncate_locals(start);
        }
        value
    }

    fn finish(self, body: Vec<Statement>) -> RegisteredFunction {
        self.bindings().finish(body)
    }
}

impl RegisteredBindings {
    fn finish(self, body: Vec<Statement>) -> RegisteredFunction {
        RegisteredFunction {
            inputs: FrameInputs::default(),
            frame_slots: self.slots,
            placed_from: 0,
            fixed_sites: Box::default(),
            fixed_attributes: None,
            function: ir::Function {
                params: Vec::new(),
                returns: Type::PythonObject,
                locals: self.locals,
                body,
            },
            local_names: self.names,
        }
    }
}

fn disabled_while(loop_: &ast::StmtWhile) -> bool {
    matches!(&*loop_.test, ast::Expr::BooleanLiteral(literal) if !literal.value)
}

/// A disabled loop must take its else suite. Exact literal branches within
/// that suite can prove that the following declaration cannot be reached.
fn mandatory_suite_exits(block: &[ast::Stmt]) -> bool {
    block.iter().any(|stmt| match stmt {
        ast::Stmt::Return(_) | ast::Stmt::Raise(_) => true,
        ast::Stmt::If(branch) => {
            for (test, body) in statements::if_clauses(branch) {
                match test {
                    Some(ast::Expr::BooleanLiteral(value)) if !value.value => continue,
                    Some(ast::Expr::BooleanLiteral(_)) | None => {
                        return mandatory_suite_exits(body);
                    }
                    _ => return false,
                }
            }
            false
        }
        _ => false,
    })
}

fn class_argument_layout(class: &ast::StmtClassDef) -> ir::PackedArguments {
    class
        .arguments
        .as_deref()
        .map(packed_arguments)
        .unwrap_or_default()
}

pub(super) fn function_definition(function: &ast::StmtFunctionDef, header: Vec<Expr>) -> Expr {
    let site = ir::DefinitionSite::Function {
        start: function.start(),
        decorators: function.decorator_list.len(),
    };
    definition_value(site, &function.decorator_list, header)
}

pub(super) fn class_definition(class: &ast::StmtClassDef, header: Vec<Expr>) -> Expr {
    let site = ir::DefinitionSite::Class {
        start: class.start(),
        arguments: class
            .type_params
            .is_none()
            .then(|| class_argument_layout(class)),
    };
    definition_value(site, &class.decorator_list, header)
}

fn definition_value(
    site: ir::DefinitionSite,
    decorators: &[ast::Decorator],
    mut header: Vec<Expr>,
) -> Expr {
    for (decorator, value) in decorators.iter().zip(&mut header) {
        let range = decorator.expression.range();
        if value
            .written_range()
            .is_none_or(|written| written.start() != range.start())
        {
            // A suspended or extracted operand can reach the binding as a temporary.
            let operand = std::mem::replace(value, Expr::none());
            *value = values::located(range, range.start(), operand);
        }
    }
    operation(
        Builtin::RegisteredDefinition(site),
        Type::PythonObject,
        header,
    )
}

impl SourceScope<'_> {
    fn local_initializer(&self, ann: &ast::StmtAnnAssign, value: Expr) -> Expr {
        if !matches!(self.kind, RegisteredScope::Function { .. }) || !ann.target.is_name_expr() {
            return value;
        }
        let expected = self
            .source
            .operand_types
            .and_then(|types| types.local_scalar_annotation(&ann.annotation));
        checked_local_scalar(expected, value)
    }

    fn target_refusal(&self, target: &ast::Expr) -> OperandRefusal {
        self.refusal.unwrap_or((
            target.range(),
            "cannot compile this assignment target ahead of time",
        ))
    }

    fn assign(
        &mut self,
        annotation: Option<&ast::StmtAnnAssign>,
        targets: &[ast::Expr],
        rhs: &ast::Expr,
        output: &mut Vec<Statement>,
        read: &mut impl FnMut(&mut Self, StatementOperand<'_>) -> Option<Expr>,
        retained: bool,
    ) -> StdResult<(), ()> {
        let mut value = |scope: &mut Self| {
            let value = read(scope, StatementOperand::Assignment(rhs)).ok_or(())?;
            Ok(if let Some(ann) = annotation {
                scope.local_initializer(ann, value)
            } else {
                value
            })
        };
        if let Some(field) = annotation.and_then(|ann| ann.target.as_attribute_expr()) {
            let value = value(self)?;
            let receiver = self
                .with_binding_operands(|scope| {
                    read(scope, StatementOperand::Suspendable(&field.value))
                })
                .ok_or(())?;
            output.push(attribute_statement(
                ir::AttributeAction::Store,
                field,
                vec![value, receiver],
            ));
            return Ok(());
        }

        // Prepare retained targets before the RHS, then store its value before any target runs.
        let start = output.len();
        let slot = retained.then(|| self.hidden(Type::PythonObject));
        let input = match slot {
            Some(slot) => {
                unpacking::take_pattern_temporary(Expr::local(slot, Type::PythonObject), slot)
            }
            None => value(self)?,
        };
        let report = self.report_child_refusals;
        self.report_child_refusals |= retained;
        let bound = unpacking::bind_assignment(self, targets, input, output);
        self.report_child_refusals = report;
        bound?;
        if let Some(slot) = slot {
            output.insert(start, Statement::Assign(slot, value(self)?));
        }
        Ok(())
    }

    fn augmented_assignment(
        &mut self,
        assign: &ast::StmtAugAssign,
        mut operand: impl FnMut(StatementOperand<'_>, &mut Self) -> Option<Expr>,
    ) -> Option<Statement> {
        match assign.target.as_ref() {
            ast::Expr::Name(name) => {
                let left = operand(StatementOperand::Suspendable(&assign.target), self)?;
                let right = operand(StatementOperand::Suspendable(&assign.value), self)?;
                Some(self.store(name.id.as_str(), in_place(assign.op, left, right)))
            }
            ast::Expr::Subscript(target) => Some(Statement::AugmentItem(
                operand(StatementOperand::Suspendable(&target.value), self)?,
                operand(StatementOperand::Index(&target.slice), self)?,
                ir::ItemUpdate::Protocol(assign.op.as_str()),
                operand(StatementOperand::Suspendable(&assign.value), self)?,
            )),
            _ => None,
        }
    }

    fn with_binding_operands<T>(&mut self, lower: impl FnOnce(&mut Self) -> T) -> T {
        if self.loop_scope == LoopScope::WithCursor
            && matches!(self.kind, RegisteredScope::Function { is_async: true })
        {
            return lower(self);
        }
        self.with_suspending_operands(lower)
    }

    fn checked_physical_statement(&self, stmt: &ast::Stmt) -> bool {
        match stmt {
            // A physical frame owns the yielded or awaited assignment value.
            ast::Stmt::Assign(assign) => ir::SuspendValue::operand(&assign.value).is_none(),
            ast::Stmt::If(_)
            | ast::Stmt::For(_)
            | ast::Stmt::While(_)
            | ast::Stmt::Try(_)
            | ast::Stmt::With(_)
            | ast::Stmt::Match(_)
            | ast::Stmt::Break(_)
            | ast::Stmt::Continue(_)
            | ast::Stmt::IpyEscapeCommand(_) => false,
            ast::Stmt::TypeAlias(_)
            | ast::Stmt::Pass(_)
            | ast::Stmt::Global(_)
            | ast::Stmt::Nonlocal(_)
            | ast::Stmt::Import(_)
            | ast::Stmt::ImportFrom(_)
            | ast::Stmt::ClassDef(_)
            | ast::Stmt::Assert(_)
            | ast::Stmt::Return(_)
            | ast::Stmt::Expr(_)
            | ast::Stmt::Delete(_)
            | ast::Stmt::AnnAssign(_)
            | ast::Stmt::AugAssign(_)
            | ast::Stmt::Raise(_)
            | ast::Stmt::FunctionDef(_) => true,
        }
    }

    fn physical_statement(
        &mut self,
        stmt: &ast::Stmt,
        suite: &mut SuiteBody,
        preparation: &SuitePreparation<'_>,
    ) -> StdResult<bool, ()> {
        let body = &mut suite.statements;
        if !matches!(self.kind, RegisteredScope::Function { .. }) {
            return Err(());
        }
        match stmt {
            ast::Stmt::Match(_) => return self.statement(stmt, body),
            ast::Stmt::For(loop_) if !loop_.is_async => return self.statement(stmt, body),
            ast::Stmt::If(_) | ast::Stmt::While(_) => {}
            ast::Stmt::Try(region) if !region.is_star => {}
            _ => return Err(()),
        }
        let begin = body.len();
        let continues = self.statement(stmt, body)?;
        let checked = &body[begin..];
        let complete = if stmt.is_if_stmt() {
            // A checked raise can still lead the declaration scanner into the next statement.
            preparation
                .check_required_sites(RegionRoot::Suite(checked), true)
                .is_ok_and(|reaches_next| !continues || reaches_next)
        } else {
            !matches!(checked.last(), Some(Statement::While(test, _, _)) if ir::control::boolean_literal(test) == Some(false))
                && preparation.checked_declaration_refusal(checked).is_none()
        };
        if complete {
            return Ok(continues);
        }
        // A disabled loop selects only its else. The successful checked lowering
        // already allocated that block's private slots in this scope, so keep it
        // in the same frame instead of preparing a separate physical child.
        if let ast::Stmt::While(loop_) = stmt
            && disabled_while(loop_)
            && let Some(Statement::While(_, _, otherwise)) = body.pop()
        {
            suite.check_run(preparation);
            suite.statements.extend(otherwise);
            suite.falls_through &= continues;
            suite.check_run(preparation);
            return Ok(continues);
        }
        body.truncate(begin);
        Err(())
    }

    fn assertion(
        &mut self,
        stmt: &ast::StmtAssert,
        read: &mut impl FnMut(&mut Self, StatementOperand<'_>) -> Option<Expr>,
    ) -> StdResult<[Statement; 2], ()> {
        let test = read(self, StatementOperand::Condition(&stmt.test)).ok_or(())?;
        let message = stmt
            .msg
            .as_deref()
            .filter(|_| ir::control::boolean_literal(&test) != Some(true))
            .map(|message| read(self, StatementOperand::Frame(message)).ok_or(()))
            .transpose()?;
        Ok(assertion_statements(stmt, test, message))
    }

    fn statement_operand(&mut self, input: StatementOperand<'_>) -> Option<Expr> {
        input.lower(self)
    }

    fn statement(&mut self, stmt: &ast::Stmt, body: &mut Vec<Statement>) -> StdResult<bool, ()> {
        let (operation, continues) =
            self.written_statement(stmt, body, false, Self::statement_operand)?;
        body.extend(operation);
        Ok(continues)
    }

    fn written_statement(
        &mut self,
        stmt: &ast::Stmt,
        body: &mut Vec<Statement>,
        retained: bool,
        mut read: impl FnMut(&mut Self, StatementOperand<'_>) -> Option<Expr>,
    ) -> StdResult<(Option<Statement>, bool), ()> {
        if let ast::Stmt::Assert(assertion) = stmt {
            if !self.omit_assertions {
                let [checkpoint, branch] = self.assertion(assertion, &mut read)?;
                body.push(checkpoint);
                return Ok((Some(branch), true));
            }
            return Ok((None, true));
        }
        let start = body.len();
        push_checkpoint(body, stmt);
        let module = matches!(self.kind, RegisteredScope::Module);
        (self, read, retained)
            .source_statement(stmt, module, body)
            .inspect_err(|_| body.truncate(start))
    }

    fn suite(&mut self, stmts: &[ast::Stmt]) -> StdResult<Vec<Statement>, ()> {
        self.block(stmts).map(|(body, _)| body)
    }

    fn in_loop<T>(&mut self, kind: LoopScope, lower: impl FnOnce(&mut Self) -> T) -> T {
        let enclosing = self.loop_scope;
        if enclosing != LoopScope::WithCursor {
            self.loop_scope = kind;
        }
        let body = lower(self);
        self.loop_scope = enclosing;
        body
    }

    fn reserve_cursor_slot(&mut self, ty: Type, cursor: Option<ir::IterNextKind>) -> usize {
        let slot = self.hidden(ty);
        if let Some(kind) = cursor {
            self.cursors.insert(slot, kind);
        }
        slot
    }

    fn hidden(&mut self, ty: Type) -> usize {
        let slot = self.locals.len();
        self.locals.push(ty);
        slot
    }

    fn match_slots(&mut self, pattern: &ir::MatchPattern) -> Vec<usize> {
        pattern.names.iter().map(|name| self.slot(name)).collect()
    }

    fn match_statement(&mut self, branch: &ast::StmtMatch) -> StdResult<statements::Block, ()> {
        let mut cases = patterns::prepare_cases(branch, self.source).map_err(|_| ())?;
        let mut exhaustive = false;
        for (case, written) in cases.iter_mut().zip(&branch.cases) {
            if exhaustive {
                return Err(());
            }
            case.captures = self.match_slots(&case.pattern);
            case.guard = written
                .guard
                .as_deref()
                .map(|value| self.condition_operand(value).map(truth).ok_or(()))
                .transpose()?;
            exhaustive = case.guard.is_none() && written.pattern.is_irrefutable();
            case.body = self.suite(&written.body)?;
        }
        // Only a complete match owns its subject in this suite's slot layout.
        let subject = self.frame_value(&branch.subject).ok_or(())?;
        // Registered match placement retains the join after the last case.
        Ok((vec![Statement::Match(subject, cases)], true))
    }

    fn block(&mut self, stmts: &[ast::Stmt]) -> StdResult<statements::Block, ()> {
        let mut body = Vec::new();
        let falls = self.block_into(stmts, &mut body)?;
        Ok((body, falls))
    }

    fn block_into(
        &mut self,
        stmts: &[ast::Stmt],
        body: &mut Vec<Statement>,
    ) -> StdResult<bool, ()> {
        let mut falls = true;
        for stmt in stmts {
            falls &= self.statement(stmt, body)?;
        }
        Ok(falls)
    }

    fn slot(&mut self, name: &str) -> usize {
        if let Some(slot) = self.names.get(name) {
            return *slot;
        }
        let slot = self.hidden(Type::PythonObject);
        self.names.insert(name.to_owned(), slot);
        slot
    }

    fn local_read(&self, name: &str) -> Option<Expr> {
        let slot = if self.comprehension.is_some() {
            self.comprehension_target(name)?
        } else {
            *self.names.get(name)?
        };
        Some(Expr::local(slot, Type::PythonObject))
    }

    fn comprehension_target(&self, name: &str) -> Option<usize> {
        let scope = self.comprehension.as_ref()?;
        let bindings = &self.namespaces[scope.targets.clone()];
        let index = bindings
            .binary_search_by(|(_, binding)| binding.name.as_str().cmp(name))
            .ok()?;
        Some(bindings[index].0)
    }

    fn store(&mut self, name: &str, value: Expr) -> Statement {
        let slot = self
            .comprehension_target(name)
            .unwrap_or_else(|| self.slot(name));
        Statement::Assign(slot, value)
    }

    fn is_async_function(&self) -> bool {
        matches!(self.kind, RegisteredScope::Function { is_async: true })
    }

    fn iteration_input(
        &mut self,
        iterable: &ast::Expr,
        pull: Option<ir::AsyncPull>,
    ) -> Option<pipelines::Prepared> {
        if pull.is_some() && !self.is_async_function() {
            return None;
        }
        let value = self.ordinary_operand(iterable)?;
        Some(cursor_input(
            value,
            Some(iterable.range()),
            pull,
            |ty, cursor| self.reserve_cursor_slot(ty, cursor),
        ))
    }

    /// The activation owns the complete value before a return, discard or store.
    /// Its temporary roots keep earlier arguments live across a later suspension.
    fn frame_value(&mut self, expr: &ast::Expr) -> Option<Expr> {
        if matches!(self.kind, RegisteredScope::Function { .. }) {
            self.with_binding_operands(|scope| scope.operand(expr))
        } else {
            self.operand(expr)
        }
    }

    fn operand(&mut self, expr: &ast::Expr) -> Option<Expr> {
        self.lower_operand(expr, true)
    }

    fn condition_operand(&mut self, test: &ast::Expr) -> Option<Expr> {
        self.with_suspending_operands(|scope| scope.operand(test))
    }

    fn with_suspending_operands<T>(&mut self, lower: impl FnOnce(&mut Self) -> T) -> T {
        let previous = std::mem::replace(&mut self.suspending_operands, true);
        let value = lower(self);
        self.suspending_operands = previous;
        value
    }

    fn lambda_binding(
        &mut self,
        lambda: &ast::ExprLambda,
    ) -> StdResult<Expr, (TextRange, &'static str)> {
        let defaults = values::lambda_defaults(lambda)
            .map(|default| {
                self.ordinary_operand(default).ok_or_else(|| {
                    self.refusal.unwrap_or((
                        default.range(),
                        "cannot compile this lambda default ahead of time",
                    ))
                })
            })
            .collect::<StdResult<_, _>>()?;
        Ok(values::lambda_binding(lambda, defaults))
    }

    fn selected_operand(&mut self, expr: &ast::Expr, selection: SuiteSelection) -> Option<Expr> {
        match (self.kind, selection) {
            (RegisteredScope::Function { .. }, SuiteSelection::Physical) => self.operand(expr),
            _ => self.ordinary_operand(expr),
        }
    }

    fn ordinary_operand(&mut self, expr: &ast::Expr) -> Option<Expr> {
        self.lower_operand(expr, false)
    }

    fn lower_operand(&mut self, expr: &ast::Expr, whole: bool) -> Option<Expr> {
        if expr.is_dict_expr() && self.report_child_refusals {
            self.report_child_refusals = false;
            let value = self.checked_operand(expr, whole);
            self.report_child_refusals = true;
            if value.is_none() {
                self.refusal.get_or_insert((
                    expr.range(),
                    "cannot compile this dictionary display ahead of time",
                ));
            }
            return value;
        }

        let value = self.checked_operand(expr, whole);
        if value.is_none() && self.report_child_refusals {
            let reason = match expr {
                ast::Expr::FString(_) => FORMATTED_STRING_REFUSAL,
                ast::Expr::Generator(_) => GENERATOR_REFUSAL,
                _ => return None,
            };
            self.refusal = Some((expr.range(), reason));
        }
        value
    }

    fn checked_operand(&mut self, expr: &ast::Expr, whole: bool) -> Option<Expr> {
        // A generator lambda can resume within its return expression. Eager
        // display bodies use their own iteration and operand boundaries.
        let suspension = match self.kind {
            RegisteredScope::Function { is_async } if whole || self.suspending_operands => {
                Some(is_async)
            }
            RegisteredScope::Lambda if self.comprehension.is_none() => Some(false),
            _ => None,
        };
        let mut value = if let Some(is_async) = suspension
            && let Some((kind, value)) = ir::SuspendValue::operand(expr)
            && !matches!(
                (kind, is_async),
                (ir::SuspendValue::Await, false) | (ir::SuspendValue::Delegate, true)
            ) {
            self.suspend_operand(kind, value, expr.range())?
        } else if let ast::Expr::TString(template) = expr {
            self.with_suspending_operands(|scope| {
                scope.source.template(
                    template,
                    matches!(
                        scope.kind,
                        RegisteredScope::Function { .. } | RegisteredScope::Lambda
                    ),
                    |child| {
                        let value = scope.ordinary_operand(child);
                        if value.is_none() && scope.report_child_refusals {
                            scope.refusal.get_or_insert((
                                child.range(),
                                "cannot compile this template field ahead of time",
                            ));
                        }
                        value
                    },
                )
            })?
        } else if let ast::Expr::FString(formatted) = expr {
            self.with_suspending_operands(|scope| {
                fstrings::registered_fstring(formatted, scope.source, |child| {
                    scope.ordinary_operand(child).ok_or(())
                })
            })
            .ok()?
        } else if matches!(
            expr,
            ast::Expr::ListComp(_) | ast::Expr::SetComp(_) | ast::Expr::DictComp(_)
        ) {
            let value = self.eager_comprehension(expr);
            if value.is_none() {
                self.refusal = Some((expr.range(), COMPREHENSION_REFUSAL));
            }
            value?
        } else if (self.comprehension.is_some()
            || self.generator_expression
            || self.loop_scope == LoopScope::WithCursor)
            && matches!(self.kind, RegisteredScope::Function { is_async: true })
            && let ast::Expr::Await(awaited) = expr
        {
            self.suspend_operand(ir::SuspendValue::Await, Some(&awaited.value), expr.range())?
        } else if let ast::Expr::Lambda(lambda) = expr {
            let binding = self.checked_lambda_binding(lambda)?;
            // The pending result of an eager comprehension lives across its element defaults.
            if self.comprehension.is_some()
                && !self.generator_expression
                && self.loop_scope != LoopScope::Outside
            {
                let mut suspends = false;
                ir::walk_operand_expressions(&binding, &mut |child| {
                    suspends |= matches!(
                        child.kind,
                        ExprKind::Builtin(Builtin::RegisteredSuspend(..), _)
                    );
                });
                if suspends {
                    return None;
                }
            }
            binding
        } else if let ast::Expr::Named(named) = expr {
            let target = named.target.as_name_expr()?;
            let value =
                self.with_binding_operands(|scope| scope.lower_operand(&named.value, whole))?;
            Expr {
                ty: value.ty,
                kind: ExprKind::Bind(self.slot(&target.id), Box::new(value)),
            }
        } else {
            if let Some(name) = expr.as_name_expr()
                && let Some(local) = self.local_read(&name.id)
            {
                return Some(self.source.located(expr, local));
            }
            let class = matches!(self.kind, RegisteredScope::Class);
            let placed = self
                .call_operands
                .filter(|_| self.comprehension.is_none())
                .and_then(|sites| {
                    let call = expr.as_call_expr()?;
                    values::take_source_call(
                        &mut sites.borrow_mut(),
                        call,
                        self.source.operand_types?,
                        &self.names,
                    )
                });
            values::operation(
                expr,
                self.source,
                placed,
                |method, _, value| {
                    let ty = value.ty;
                    class
                        || expr.is_unary_op_expr()
                            && matches!(
                                value.without_location().kind,
                                ExprKind::Int(_) | ExprKind::Float(_)
                            )
                        || ty == Type::Str
                            && method
                                .and_then(ast::Expr::as_attribute_expr)
                                .is_some_and(|field| {
                                    imre_ast::strings::STR_METHODS.contains(&field.attr.as_str())
                                })
                },
                |_| true,
                |child, _| {
                    match expr {
                        ast::Expr::Generator(_) => {
                            self.creating_iterable(|scope| scope.ordinary_operand(child))
                        }
                        // The dictionary retains earlier operands while a key, value or spread suspends.
                        ast::Expr::Dict(_) => self.operand(child),
                        ast::Expr::Call(_) | ast::Expr::Set(_) | ast::Expr::Subscript(_)
                            if child.is_await_expr() =>
                        {
                            self.operand(child)
                        }
                        _ => self.ordinary_operand(child),
                    }
                    .ok_or(())
                },
            )
            .ok()??
        };
        if let Some(scope) = &self.comprehension {
            values::in_namespace(&mut value, Expr::local(scope.slot, Type::PythonObject));
        }
        Some(self.source.located_operation(expr, value))
    }

    fn suspend_operand(
        &mut self,
        kind: ir::SuspendValue,
        operand: Option<&ast::Expr>,
        range: TextRange,
    ) -> Option<Expr> {
        let value = match operand {
            Some(operand) => self.ordinary_operand(operand)?,
            None => Expr::none(),
        };
        Some(suspend(kind, range, value))
    }
}

pub(super) fn suspend(kind: ir::SuspendValue, range: TextRange, value: Expr) -> Expr {
    operation(
        Builtin::RegisteredSuspend(kind, range),
        Type::PythonObject,
        vec![value],
    )
}

impl ir::SuspendValue {
    /// Preserve the original suspension kind and the optional operand of a bare yield.
    pub(crate) fn operand(expr: &ast::Expr) -> Option<(Self, Option<&ast::Expr>)> {
        match expr {
            ast::Expr::Await(expr) => Some((Self::Await, Some(&expr.value))),
            ast::Expr::Yield(expr) => Some((Self::Yield, expr.value.as_deref())),
            ast::Expr::YieldFrom(expr) => Some((Self::Delegate, Some(&expr.value))),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::registered_body_at;

    fn prepared_sites<'a>(
        stmts: &'a [ast::Stmt],
        source: values::RegisteredSource<'_>,
        bundled: bool,
    ) -> RegisteredSites<'a> {
        let default_names = imre_ast::Names::default();
        let names = source.private_names.unwrap_or(&default_names);
        let (mut scopes, children) =
            super::super::function_scope::ScopeFacts::prepare(stmts, names, |_, _| {}, |_| {});
        registered_sites(
            (stmts, children),
            &mut scopes,
            names,
            source,
            bundled,
            true,
            crate::frontend::annotation_operands::test_annotation_body,
        )
        .0
    }

    pub(super) fn registered_suite(
        stmts: &[ast::Stmt],
        kind: RegisteredScope,
        source: values::RegisteredSource<'_>,
    ) -> Option<RegisteredFunction> {
        let operations = prepared_sites(stmts, source, false).operations;
        RegisteredFunction::from_suite(stmts, kind, source, &operations, true)
    }

    #[test]
    fn a_registered_if_passes_its_comparison_to_the_branch() {
        let text = "if value is None:\n    pass\n";
        let graph = SourceGraph::single(text).unwrap();
        let module = &graph.modules[graph.entry];
        let source = values::ModuleSource::new(module);
        let suite = registered_suite(
            &module.parsed.syntax().body,
            RegisteredScope::Module,
            source.context(text),
        )
        .unwrap();
        let condition = suite.function.body.iter().find_map(|stmt| match stmt {
            Statement::If(test, _, _) => Some(&test.without_location().kind),
            _ => None,
        });
        assert!(
            matches!(condition, Some(ExprKind::Compare(_, ops)) if ops.as_slice() == [ast::CmpOp::Is])
        );
    }

    #[test]
    fn a_module_call_loads_the_place_that_received_its_definition() {
        let text = "import token\ndef take(value: int) -> int:\n    return value\n\ndef text():\n    return 'wrong'\nprint(take(text()))\n";
        let graph = SourceGraph::single(text).unwrap();
        let module = &graph.modules[graph.entry];
        let source = values::ModuleSource::new(module);
        let suite = registered_suite(
            &module.parsed.syntax().body,
            RegisteredScope::Module,
            source.context(text),
        )
        .unwrap();
        let mut stores = Vec::new();
        let mut calls = Vec::new();
        ir::walk_nodes(&suite.function.body, &mut |node, _| match node {
            ir::Node::Statement(Statement::Assign(slot, value))
                if matches!(
                    value.kind,
                    ExprKind::Builtin(Builtin::RegisteredDefinition(_), _)
                ) =>
            {
                stores.push(*slot);
            }
            ir::Node::Expression(Expr {
                kind: ExprKind::Invoke { callable, .. },
                ..
            }) => {
                ir::walk_operand_expressions(callable, &mut |target| {
                    if let ExprKind::Local(slot) = target.kind {
                        calls.push(slot);
                    }
                });
            }
            _ => {}
        });
        assert_eq!(
            stores
                .iter()
                .map(|slot| suite.local_names[slot].name.as_str())
                .collect::<Vec<_>>(),
            ["take", "text"]
        );
        assert_eq!(calls, stores);
    }

    #[test]
    fn interactive_and_ordinary_blocks_agree_on_fallthrough() {
        for (text, falls_through) in [
            ("for item in items:\n    item\n", true),
            ("try:\n    act()\nfinally:\n    finish()\n", true),
            ("try:\n    act()\nfinally:\n    raise Error()\n", false),
            ("try:\n    raise E\nexcept E:\n    raise F\n", false),
            (
                "try:\n    act()\nexcept E:\n    raise F\nelse:\n    raise G\n",
                false,
            ),
            (
                "try:\n    act()\nexcept E:\n    pass\nelse:\n    raise G\n",
                true,
            ),
            ("raise Error()\n", false),
            ("del first, second\n", true),
        ] {
            let graph = SourceGraph::single(text).unwrap();
            let module = &graph.modules[graph.entry];
            let stmts = &module.parsed.syntax().body;
            let source = module.registered_values().context(text);
            let operations = prepared_sites(stmts, source, false).operations;
            let mut prepare = SuitePreparation::new(&operations, source, true);
            let ordinary = prepare.suite(stmts, RegisteredScope::Module);
            let interactive = prepare.interactive(stmts).unwrap();
            for suite in [&ordinary, &interactive] {
                assert!(suite.fully_checked(), "{text}");
                crate::verify::registered(suite.checked.region()).unwrap();
                assert_eq!(suite.falls_through, falls_through, "{text}");
            }
        }
    }

    #[test]
    fn a_false_loop_else_keeps_its_suspending_iterator_in_the_same_frame() {
        let text = "def f():\n    while False:\n        yield 0\n    else:\n        for x in [1, 2]:\n            yield x\n";
        let graph = SourceGraph::single(text).unwrap();
        let module = &graph.modules[graph.entry];
        let def = module.parsed.syntax().body[0]
            .as_function_def_stmt()
            .unwrap();
        let source = module.registered_values().context(text);
        let operations = HashMap::new();
        let mut prepare = SuitePreparation::new(&operations, source, true);
        let suite = prepare.function_suite(
            &def.body,
            RegisteredScope::Function { is_async: false },
            true,
        );
        assert!(suite.fully_checked());
        assert!(suite.refusal.is_none());
        crate::verify::registered(suite.checked.region()).unwrap();
        let mut cursors = Vec::new();
        ir::walk_statements(&suite.checked.function.body, &mut |stmt| {
            if let Statement::IterNext { iterator, .. } = stmt {
                cursors.push(*iterator);
            }
        });
        assert!(!suite.checked.function.body.iter().any(|stmt| matches!(
            stmt, Statement::While(test, _, _) if ir::control::boolean_literal(test) == Some(false)
        )));
        assert_eq!(cursors.len(), 1);
        assert!(suite.checked.region().private_slots().contains(&cursors[0]));
        assert!(suite.checked.frame_slots.cursor(&cursors[0]).is_some());
    }

    #[test]
    fn a_false_loop_leaves_through_its_else_suite() {
        for (ending, falls_through) in [
            ("return 1", false),
            ("raise KeyError()", false),
            ("pass", true),
        ] {
            let text = format!(
                "async def f():\n    while False:\n        pass\n    else:\n        {ending}\n"
            );
            let graph = SourceGraph::single(&text).unwrap();
            let module = &graph.modules[graph.entry];
            let function = module.parsed.syntax().body[0]
                .as_function_def_stmt()
                .unwrap();
            let source = module.registered_values().context(&text);
            let operations = HashMap::new();
            let mut prepare = SuitePreparation::new(&operations, source, true);
            let suite = prepare.function_suite(
                &function.body,
                RegisteredScope::Function { is_async: true },
                true,
            );
            assert_eq!(suite.falls_through, falls_through, "{ending}");
            assert!(
                !suite
                    .checked
                    .function
                    .body
                    .iter()
                    .any(|stmt| matches!(stmt, Statement::While(..))),
                "{ending}"
            );
        }
    }

    #[test]
    fn a_type_alias_binds_at_its_written_position_without_a_prepared_operation() {
        let text = "before = 1\ntype Alias = int\nafter = 2\n";
        let graph = SourceGraph::single(text).unwrap();
        let module = &graph.modules[graph.entry];
        let body = &module.parsed.syntax().body;
        let source = values::ModuleSource::new(module);
        let operations = HashMap::new();
        let mut prepare = SuitePreparation::new(&operations, source.context(text), true);
        let suite = prepare.suite(body, RegisteredScope::Module);
        assert!(suite.fully_checked());
        let frame = &suite.checked;
        let mut stores = Vec::new();
        for statement in &frame.function.body {
            if let Statement::Assign(slot, value) = statement {
                let site = match &value.kind {
                    ExprKind::Builtin(Builtin::RegisteredTypeAlias(site), _) => Some(*site),
                    _ => None,
                };
                stores.push((frame.local_names[slot].name.as_str(), site));
            }
        }
        assert_eq!(
            stores,
            [
                ("before", None),
                ("Alias", Some(body[1].start())),
                ("after", None)
            ]
        );
    }

    #[test]
    fn suspended_loops_and_exiting_branches_keep_their_checked_bodies() {
        let text = "def stream(flag, data):\n    while flag:\n        yield flag\n        break\n    if flag:\n        yield 1\n        return 2\n    elif data:\n        return 3\n    else:\n        raise EOFError\n";
        let graph = SourceGraph::single(text).unwrap();
        let module = &graph.modules[graph.entry];
        let function = module.parsed.syntax().body[0]
            .as_function_def_stmt()
            .unwrap();
        let operations = HashMap::new();
        let mut prepare =
            SuitePreparation::new(&operations, module.registered_values().context(text), true);
        let suite = prepare.physical_function_suite(
            &function.body,
            RegisteredScope::Function { is_async: false },
            true,
        );
        assert!(suite.refusal.is_none());
        assert!(suite.fully_checked());
        assert!(!suite.falls_through);
    }

    #[test]
    fn entirely_mixed_suite_keeps_its_prepared_entry() {
        let text = "def outer():\n    received = yield 'value'\n";
        let graph = SourceGraph::single(text).unwrap();
        let module = &graph.modules[graph.entry];
        let body = &module.parsed.syntax().body[0]
            .as_function_def_stmt()
            .unwrap()
            .body;
        let literals = values::ModuleSource::new(module);
        let operations = HashMap::new();
        let mut prepare = SuitePreparation::new(&operations, literals.context(text), true);
        let suite = prepare.physical_function_suite(
            body,
            RegisteredScope::Function { is_async: false },
            true,
        );
        assert!(suite.refusal.is_none());
        assert!(!suite.fully_checked());
        assert_eq!(suite.operands.len(), 1);
        assert!(suite.checked.function.body.iter().any(|statement| matches!(
            statement,
            Statement::Assign(
                _,
                Expr {
                    kind: ExprKind::Builtin(Builtin::RegisteredInput(0), _),
                    ..
                }
            )
        )));
        crate::verify::registered(suite.checked.region()).unwrap();
    }

    #[test]
    fn generic_method_closure_uses_ordinary_function_sites() {
        let source = "class Box:\n    def capture[__T](self, __value: __T):\n        def get():\n            return __value\n        return get\n";
        let graph = SourceGraph::single(source).unwrap();
        let module = &graph.modules[graph.entry];
        let mut body = module.parsed.syntax().body.clone();
        let names = imre_ast::lower(&mut body);
        let literals = values::ModuleSource::new(module);
        let (mut scopes, children) =
            super::super::function_scope::ScopeFacts::prepare(&body, &names, |_, _| {}, |_| {});
        let sites = registered_sites(
            (&body, children),
            &mut scopes,
            &names,
            literals.context(source),
            false,
            true,
            super::super::annotation_operands::test_annotation_body,
        )
        .0;
        let [
            DeclarationSite::Class(_),
            DeclarationSite::Function(method),
            DeclarationSite::Function(closure),
        ] = sites.declarations.as_slice()
        else {
            panic!("expected the method and its lexical function");
        };
        assert!(closure.parameters.is_empty());
        let [parameter] = method.parameters.as_slice() else {
            panic!("expected the method's type parameter");
        };
        assert_eq!(parameter.binding, "_Box__T");
        assert_eq!(parameter.name, "__T");
        assert_eq!(closure.def.name.as_str(), "get");
    }

    #[test]
    fn datetime_assertion_formats_its_message_on_the_failure_edge() {
        let text = include_str!("../../stdlib/_pydatetime.py");
        let module = ruff_python_parser::parse_module(text).unwrap();
        let body = &module.syntax().body;
        let function = body
            .iter()
            .filter_map(ast::Stmt::as_function_def_stmt)
            .find(|def| def.name.as_str() == "_days_before_month")
            .unwrap();
        let stmt = function
            .body
            .iter()
            .find(|stmt| stmt.is_assert_stmt())
            .unwrap();
        let assertion = stmt.as_assert_stmt().unwrap();
        assert!(assertion.test.is_compare_expr());
        assert!(matches!(
            assertion.msg.as_deref(),
            Some(ast::Expr::FString(_))
        ));
        let source = RegisteredSource {
            python_optimization: 0,
            operand_types: None,
            template_fields: None,
            generators: None,
            lambdas: None,
            text,
            private_names: None,
            method_imports: &imre_ast::module_imports(body),
            replacements: &HashMap::new(),
        };
        let stmts = std::slice::from_ref(stmt);
        let operations = HashMap::new();
        let mut preparation = SuitePreparation::new(&operations, source, true);
        let plan = preparation.suite(stmts, RegisteredScope::Function { is_async: false });
        assert!(plan.fully_checked());
        let [_, Statement::If(_, succeeds, fails)] = plan.checked.function.body.as_slice() else {
            panic!("expected the checked assertion checkpoint and branch");
        };
        assert!(succeeds.is_empty());
        let mut fields = 0;
        ir::walk_nodes(fails, &mut |node, _| {
            if let ir::Node::Expression(Expr {
                kind: ExprKind::Builtin(Builtin::FormatField(_), _),
                ..
            }) = node
            {
                fields += 1;
            }
        });
        assert_eq!(fields, 1);
    }

    #[test]
    fn a_decorator_held_in_a_temporary_keeps_its_application_position() {
        let graph = SourceGraph::single("@decorate\ndef f(value=1): pass\n").unwrap();
        let module = &graph.modules[graph.entry];
        let function = module.parsed.syntax().body[0]
            .as_function_def_stmt()
            .unwrap();
        let body = function_definition(
            function,
            vec![Expr::local(0, Type::PythonObject), Expr::none()],
        );
        let ExprKind::Builtin(Builtin::RegisteredDefinition(_), header) = body.kind else {
            panic!("expected the checked definition");
        };
        assert_eq!(
            header[0].written_range(),
            Some(function.decorator_list[0].expression.range())
        );
        assert!(matches!(
            header[0].without_location().kind,
            ExprKind::Local(0)
        ));
        assert_eq!(header[1].written_range(), None);
    }

    #[test]
    fn assertion_admission_keeps_the_message_on_the_failure_edge() {
        let text = "assert (early := probe()), f'{(unused := 1)}{(item for item in values)}'\nassert (selected := [value][0]), [message(), values[1:]]\n";
        let graph = SourceGraph::single(text).unwrap();
        let module = &graph.modules[graph.entry];
        let body = &module.parsed.syntax().body;
        let literals = values::ModuleSource::new(module);
        let operations = HashMap::new();
        let prepare = |enabled| {
            SuitePreparation::new(&operations, literals.context(text), enabled)
                .suite(body, RegisteredScope::Function { is_async: false })
        };
        let plan = prepare(true);
        assert_eq!(plan.checked.local_names.len(), 1);
        assert_eq!(plan.checked.local_names[&0].name, "selected");
        let assertion = body[0].as_assert_stmt().unwrap();
        let Some(Statement::Position(range, None)) = plan.checked.function.body.first() else {
            panic!("expected a checkpoint before the physical assertion");
        };
        assert_eq!(*range, assertion.test.range());
        let [
            PhysicalOperand::Checked(test),
            PhysicalOperand::Residual(message),
        ] = plan.operands.as_slice()
        else {
            panic!("expected the checked test followed by its physical message");
        };
        assert_eq!(test.local_names.len(), 1);
        assert_eq!(test.local_names[&0].name, "early");
        assert_eq!(message.range, assertion.msg.as_deref().unwrap().range());
        let Some(Statement::If(test, success, failure)) = plan.checked.function.body.get(1) else {
            panic!("expected a conditional assertion");
        };
        assert!(matches!(
            test.kind,
            ExprKind::Builtin(crate::ir::Builtin::RegisteredInput(0), _)
        ));
        assert_eq!(test.ty, Type::Bool);
        assert!(success.is_empty());
        let [Statement::Expr(fail)] = failure.as_slice() else {
            panic!("expected only the checked assertion failure");
        };
        let ExprKind::Builtin(Builtin::RegisteredAssertionFailure { statement, test }, args) =
            &fail.kind
        else {
            panic!("expected the shared assertion failure");
        };
        assert_eq!(
            (*statement, *test),
            (assertion.start(), assertion.test.range())
        );
        assert!(matches!(
            args.as_slice(),
            [Expr {
                ty: Type::PythonObject,
                kind: ExprKind::Builtin(crate::ir::Builtin::RegisteredInput(1), _),
                ..
            }]
        ));
        let [_, _, _, Statement::If(test, succeeds, fails)] = plan.checked.function.body.as_slice()
        else {
            panic!("expected only the checked assertion checkpoint and branch");
        };
        assert!(succeeds.is_empty());
        let [
            Statement::Expr(Expr {
                kind:
                    ExprKind::Builtin(Builtin::RegisteredAssertionFailure { statement, .. }, message),
                ..
            }),
        ] = fails.as_slice()
        else {
            panic!("expected an assertion failure on the false edge");
        };
        assert_eq!(*statement, body[1].start());
        let call_count = |operand| {
            let mut calls = 0;
            ir::walk_operand_expressions(operand, &mut |expr| {
                calls += usize::from(expr.is_registered_invoke());
            });
            calls
        };
        assert_eq!(call_count(test), 0);
        assert_eq!(message.len(), 1);
        assert_eq!(call_count(&message[0]), 1);

        let optimized = prepare(false);
        assert!(optimized.fully_checked());
        assert!(optimized.checked.function.body.is_empty());
    }

    #[test]
    fn suspending_assertion_operands_stay_checked_on_their_own_edges() {
        let text = "def g():assert+(yield)\ndef m():assert False, +(yield)\n";
        let graph = SourceGraph::single(text).unwrap();
        let module = &graph.modules[graph.entry];
        let literals = values::ModuleSource::new(module);
        let operations = HashMap::new();
        for def in &module.parsed.syntax().body {
            let def = def.as_function_def_stmt().unwrap();
            let prepare = |enabled| {
                SuitePreparation::new(&operations, literals.context(text), enabled).function_suite(
                    &def.body,
                    RegisteredScope::Function { is_async: false },
                    true,
                )
            };
            let plan = prepare(true);
            assert!(plan.fully_checked());
            let [_, Statement::If(test, yes, no)] = plan.checked.function.body.as_slice() else {
                panic!("expected a checked assertion");
            };
            assert!(yes.is_empty());
            let [Statement::Expr(failure)] = no.as_slice() else {
                panic!("expected only the assertion failure");
            };
            let yields = |expr| {
                let mut count = 0;
                ir::walk_operand_expressions(expr, &mut |node| {
                    if matches!(
                        node.kind,
                        ExprKind::Builtin(
                            Builtin::RegisteredSuspend(ir::SuspendValue::Yield, _),
                            _
                        )
                    ) {
                        count += 1;
                    }
                });
                count
            };
            assert_eq!(
                (yields(test), yields(failure)),
                if def.name.as_str() == "g" {
                    (1, 0)
                } else {
                    (0, 1)
                }
            );
            let omitted = prepare(false);
            assert!(omitted.fully_checked() && omitted.checked.function.body.is_empty());
        }
    }

    #[test]
    fn prepared_raise_keeps_both_callback_operands_in_the_shared_body() {
        let text = "def deliver(make, cause):\n    raise make() from cause()\n";
        let graph = SourceGraph::single(text).unwrap();
        let module = &graph.modules[graph.entry];
        let definition = module.parsed.syntax().body[0]
            .as_function_def_stmt()
            .unwrap();
        let literals = values::ModuleSource::new(module);
        let checked = registered_suite(
            &definition.body,
            RegisteredScope::Function { is_async: false },
            literals.context(text),
        )
        .expect("the whole raise suite uses checked operations");
        assert!(crate::llvm::registered::supports(&checked));
        let Some(Statement::Raise(Some(value))) = checked.function.body.last() else {
            panic!("expected a terminal raise");
        };
        for operand in [&value.exception, value.cause.as_deref().unwrap()] {
            let mut calls = 0;
            ir::walk_operand_expressions(operand, &mut |value| {
                calls += usize::from(value.is_registered_invoke());
            });
            assert_eq!(calls, 1);
        }
    }

    #[test]
    fn a_mixed_raise_retains_only_its_declined_cause() {
        let text = "raise make() from cause((-1).__format__)\n";
        let graph = SourceGraph::single(text).unwrap();
        let module = &graph.modules[graph.entry];
        let ops = HashMap::new();
        let mut prepare =
            SuitePreparation::new(&ops, module.registered_values().context(text), true);
        let suite = prepare.suite(&module.parsed.syntax().body, RegisteredScope::Module);
        assert!(suite.refusal.is_none());
        assert_eq!(suite.operands.len(), 1);
        let [Statement::Position(..), Statement::Raise(Some(raised))] =
            &*suite.checked.function.body
        else {
            panic!("expected the written checkpoint and raise");
        };
        assert!(raised.exception.without_location().python_call().is_some());
        assert!(matches!(
            raised.cause.as_deref().unwrap().kind,
            ExprKind::Builtin(Builtin::RegisteredInput(0), _)
        ));
    }

    #[test]
    fn parameter_annotation_callback_preserves_field_order_and_string_operands() {
        let text =
            "def f(a: mod.T, /, b: 'list[T]', *args: int, c: bool, **kw: bytes) -> None: pass\n";
        let graph = SourceGraph::single(text).unwrap();
        let module = &graph.modules[graph.entry];
        let stmts = &module.parsed.syntax().body;
        let sites = prepared_sites(
            stmts,
            values::ModuleSource::new(module).context(text),
            false,
        );
        let def = stmts[0].as_function_def_stmt().unwrap();
        assert!(sites.module_annotations.fields.is_empty());
        let [DeclarationSite::Function(site)] = sites.declarations.as_slice() else {
            panic!("expected a function declaration");
        };
        let provider = site.annotations.as_ref().unwrap();
        assert_eq!(provider.start, def.parameters.start());
        let mut signature = serde_json::json!({});
        provider.scope.write_signature(&mut signature);
        assert!(
            signature["scope_reads"]
                .as_array()
                .unwrap()
                .iter()
                .any(|name| name == "mod")
        );
        let class_text = "class C:\n mod = 1\n def f(a: mod.T): pass\n";
        let class_graph = SourceGraph::single(class_text).unwrap();
        let class_module = &class_graph.modules[class_graph.entry];
        let class_sites = prepared_sites(
            &class_module.parsed.syntax().body,
            values::ModuleSource::new(class_module).context(class_text),
            false,
        );
        let DeclarationSite::Function(method) = &class_sites.declarations[1] else {
            panic!("expected a method");
        };
        let mut signature = serde_json::json!({});
        method
            .annotations
            .as_ref()
            .unwrap()
            .scope
            .write_signature(&mut signature);
        assert!(
            !signature["scope_reads"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|name| name == "mod")
        );
        let body = &provider.body.as_ref().unwrap().function.body;
        let [
            Statement::Return(Expr {
                kind: ExprKind::Builtin(Builtin::RegisteredAnnotations { fields, .. }, values),
                ..
            }),
        ] = body.as_slice()
        else {
            panic!("expected checked function annotations")
        };
        let names: Vec<_> = fields.iter().map(|field| field.name.as_str()).collect();
        assert_eq!(names, ["b", "a", "args", "c", "kw", "return"]);
        assert!(matches!(
            &values[0].kind,
            ExprKind::Builtin(Builtin::BindAnnotationExpression(_), _)
        ));
        assert_eq!(provider.references["a"], "mod.T");
        assert_eq!(provider.references["return"], "None");
        assert!(!provider.references.contains_key("b"));
    }

    #[test]
    fn module_annotation_order_distinguishes_exception_groups() {
        for (handler, expected) in [
            ("except", ["body", "otherwise", "caught"]),
            ("except*", ["body", "caught", "otherwise"]),
        ] {
            let text = format!(
                "try:\n    body: int\n{handler} ValueError:\n    caught: str\nelse:\n    otherwise: bool\n"
            );
            let graph = SourceGraph::single(&text).unwrap();
            let module = &graph.modules[graph.entry];
            let sites = prepared_sites(
                &module.parsed.syntax().body,
                values::ModuleSource::new(module).context(&text),
                false,
            );
            let names: Vec<_> = sites
                .module_annotations
                .fields
                .iter()
                .map(|field| field.name.as_str())
                .collect();
            assert_eq!(names, expected);
        }
    }

    #[test]
    fn registering_try_else_keeps_nested_declarations_in_earlier_handlers() {
        let suites = "\
try:
    pass
except ValueError:
    def handler():
        def caught(): pass
        return caught
else:
    def otherwise():
        def normal(): pass
        return normal
";
        let class = format!(
            "class C:\n{}",
            suites
                .lines()
                .map(|line| format!("    {line}\n"))
                .collect::<String>()
        );
        for text in [suites, class.as_str()] {
            let graph = SourceGraph::single(text).unwrap();
            let module = &graph.modules[graph.entry];
            let sites = prepared_sites(
                &module.parsed.syntax().body,
                values::ModuleSource::new(module).context(text),
                false,
            );
            let functions: Vec<_> = sites
                .declarations
                .iter()
                .filter_map(|site| match site {
                    DeclarationSite::Function(function) => Some(function.def.name.as_str()),
                    DeclarationSite::Class(_) => None,
                })
                .collect();
            assert_eq!(functions, ["otherwise", "normal", "handler", "caught"]);
        }
    }

    #[test]
    fn annotation_callback_numbers_physical_occurrences_and_preserves_a_lowering_error() {
        let text = "first: int\nsecond: list[int]\ncomplex: dict[str, int]\nforward: 'list[int]'\nprepared: (lambda: int)()\ndeferred: next(t for t in (int,))\nquoted: '(lambda: Child)()'\nscoped: [item for item in items]\n";
        let graph = SourceGraph::single(text).unwrap();
        let module = &graph.modules[graph.entry];
        let stmts = &module.parsed.syntax().body;
        let literals = values::ModuleSource::new(module);
        let source = literals.context(text);
        let prepared = |bundled| prepared_sites(stmts, source, bundled);
        let module = prepared(false).module_annotations;
        let sites = &module.fields;
        let callback = |occurrences: &[usize]| {
            let fields = occurrences
                .iter()
                .enumerate()
                .map(|(index, &slot)| (sites[slot].clone(), Some(index)));
            DeferredAnnotation::prepare_body(fields, module.bindings.clone())
        };
        let scoped = callback(&[7, 0, 7]).unwrap();
        assert!(!scoped.region().private_slots().is_empty());
        crate::verify::registered(scoped.region()).unwrap();
        let checked = callback(&[0, 1, 0]).unwrap();
        let [
            Statement::Return(Expr {
                kind: ExprKind::Builtin(Builtin::RegisteredAnnotations { fields }, values),
                ..
            }),
        ] = checked.function.body.as_slice()
        else {
            panic!("expected checked annotations")
        };
        assert_eq!(values.len(), fields.len());
        assert_eq!(
            fields
                .iter()
                .map(|field| (field.index, field.name.as_str()))
                .collect::<Vec<_>>(),
            [(Some(0), "first"), (Some(1), "second"), (Some(2), "first")]
        );
        let bundled_sites = prepared(true).module_annotations.fields;
        assert!(matches!(
            &sites[3].value,
            Ok(Expr {
                kind: ExprKind::Builtin(Builtin::BindAnnotationExpression(0), _),
                ..
            })
        ));
        assert!(
            matches!(&bundled_sites[3].value, Ok(Expr {kind: ExprKind::Str(text), ..}) if text == "list[int]")
        );
        let forward = callback(&[3]).unwrap();
        assert!(matches!(&forward.function.body[0],
            Statement::Return(Expr { kind: ExprKind::Builtin(
                Builtin::RegisteredAnnotations { .. }, values), .. })
                if matches!(values.as_slice(), [Expr { kind: ExprKind::Builtin(
                    Builtin::BindAnnotationExpression(0), _), .. }])));
        for slot in [2, 4, 6] {
            assert!(sites[slot].value.is_ok());
        }
        // This fixture omits the generator registry. Keep that exact operand error.
        let field = &sites[5];
        let error = field.value.as_ref().err().unwrap();
        assert!(field.range.contains_range(error.0));
        assert!(!error.1.is_empty());
        let Err(provider_error) = callback(&[0, 5, 1]) else {
            panic!("a missing generator body must prevent the provider from compiling");
        };
        assert_eq!(provider_error, *error);
    }

    #[test]
    fn source_guard_is_prepared_only_for_a_residual_conditional() {
        let text = concat!(
            "if checked:\n    del name\n",
            "if physical:\n    result: int = next(n for n in (1,))\n",
        );
        let graph = SourceGraph::single(text).unwrap();
        let module = &graph.modules[graph.entry];
        let stmts = &module.parsed.syntax().body;
        let literals = values::ModuleSource::new(module);
        let source = literals.context(text);
        let operations = prepared_sites(stmts, source, false).operations;
        let mut prepare = SuitePreparation::new(&operations, source, true);
        let plan = prepare.suite(stmts, RegisteredScope::Module);
        assert!(
            plan.checked
                .function
                .body
                .iter()
                .any(|stmt| matches!(stmt, Statement::If(..)))
        );
        assert!(plan.operands.is_empty());
        assert!(plan.checked.function.body.iter().any(|stmt| matches!(
            stmt,
            Statement::Expr(Expr {
                kind: ExprKind::Builtin(Builtin::RegisteredSuite(0), _),
                ..
            })
        )));
        let [region] = plan.suites.as_slice() else {
            panic!("expected the conditional to bind its child suite")
        };
        assert!(matches!(region.checked.function.body.as_slice(),
            [Statement::Position(range, None), Statement::If(..)] if *range == stmts[1].range()
        ));
        assert!(matches!(
            region.operands.as_slice(),
            [PhysicalOperand::Checked(_)]
        ));
    }

    #[test]
    fn prepared_source_runs_keep_their_order_and_namespace_ownership() {
        let text = "from . import a\nx: int = a\nif a:\n    del a\ny: int = next(n for n in (1,))\nimport z\n";
        let mut stmts = ruff_python_parser::parse_module(text)
            .unwrap()
            .into_syntax()
            .body;
        let names = imre_ast::lower(&mut stmts);
        let replacements = HashMap::new();
        let imports = imre_ast::module_imports(&stmts);
        let source = RegisteredSource {
            python_optimization: 0,
            operand_types: None,
            template_fields: None,
            generators: None,
            lambdas: None,
            text,
            private_names: Some(&names),
            replacements: &replacements,
            method_imports: &imports,
        };
        let operands = prepared_sites(&stmts, source, false);
        let ops = operands.operations;
        assert_eq!(ops.len(), 2);
        assert_eq!(operands.module_annotations.fields.len(), 2);
        let mut prepare = SuitePreparation::new(&ops, source, true);
        let plan = prepare.suite(&stmts, RegisteredScope::Module);
        assert_eq!(plan.operands.len(), 1);
        // Only the generator RHS needs an external operand. After storing it, the
        // namespace records y's annotation before executing the final import.
        let body = &plan.checked.function.body;
        let at = body
            .iter()
            .position(|stmt| {
                matches!(
                    stmt,
                    Statement::Assign(
                        _,
                        Expr {
                            kind: ExprKind::Builtin(Builtin::RegisteredInput(0), _),
                            ..
                        }
                    )
                )
            })
            .unwrap();
        let (before, after) = (&body[..at], &body[at + 1..]);
        let [.., Statement::If(_, yes, no), _] = before else {
            panic!("expected the checked conditional before the checkpoint for y")
        };
        assert!(no.is_empty());
        assert!(matches!(yes.as_slice(), [_, Statement::Expr(Expr {
            kind: ExprKind::Builtin(Builtin::RegisteredDeleteName(name, _), _), ..
        })] if name == "a"));
        assert!(matches!(
            after,
            [
                Statement::Assign(_, _),
                Statement::Expr(Expr {
                    kind: ExprKind::Builtin(Builtin::RegisteredNamespaceAnnotation(_), _),
                    ..
                }),
                _,
                Statement::Expr(Expr {
                    kind: ExprKind::Builtin(Builtin::RegisteredImport(_), _),
                    ..
                })
            ]
        ));
    }

    #[test]
    fn prepared_match_retains_the_guard_capture_and_both_conditional_arms() {
        let text = concat!(
            "if enabled:\n",
            "    match subject:\n",
            "        case Row(value=item) if accept(item):\n",
            "            record(item if choose else other)\n",
            "else:\n",
            "    record(other)\n",
        );
        let graph = SourceGraph::single(text).unwrap();
        let module = &graph.modules[graph.entry];
        let source = values::ModuleSource::new(module);
        let body = registered_suite(
            &module.parsed.syntax().body,
            RegisteredScope::Module,
            source.context(text),
        )
        .unwrap();
        assert!(crate::llvm::registered::supports(&body));
        let mut matched = false;
        ir::walk_statements(&body.function.body, &mut |stmt| {
            if let Statement::Match(_, cases) = stmt {
                let [case] = cases.as_slice() else {
                    panic!("expected one match case")
                };
                assert_eq!(case.pattern.names, ["item"]);
                assert_eq!(case.captures.len(), 1);
                assert!(case.guard.is_some());
                let mut selected = false;
                ir::walk_expressions(&case.body, &mut |expr| {
                    selected |= matches!(expr.kind, ExprKind::Conditional(..));
                });
                assert!(selected);
                matched = true;
            }
        });
        assert!(matched);
    }

    #[test]
    fn registered_branches_keep_selected_operands_and_live_bool_calls() {
        let graph = SourceGraph::single(concat!(
            "def choose(flag: int, left, right, sink):\n",
            "    if flag and left:\n",
            "        sink(left if flag else right)\n",
            "    else:\n",
            "        sink(bool(flag), (left and right) or flag)\n",
        ))
        .unwrap();
        assert!(super::super::lower(&graph).is_err());
        let bodies = graph.registered_functions();
        let body = &bodies[&graph.entry][&TextSize::default()].function.body;
        assert!(body.iter().any(|stmt| matches!(stmt, Statement::If(..))));
        let mut counts = [0; 5];
        ir::walk_expressions(body, &mut |expr| match &expr.kind {
            ExprKind::Conditional(..) => counts[0] += 1,
            ExprKind::Boolean(..) => counts[1] += 1,
            ExprKind::BooleanValue(..) => counts[2] += 1,
            ExprKind::Builtin(Builtin::RegisteredName(name), _) if name == "bool" => counts[3] += 1,
            ExprKind::Invoke { .. } => counts[4] += 1,
            _ => {}
        });
        assert_eq!(counts, [1, 1, 2, 1, 3]);
    }

    #[test]
    fn name_updates_compile_to_a_load_and_an_inplace_operation() {
        let graph = SourceGraph::single("result += operand()\n").unwrap();
        let entry = &graph.modules[graph.entry];
        for scope in [
            RegisteredScope::Module,
            RegisteredScope::Function { is_async: false },
        ] {
            let plan = registered_suite(
                &entry.parsed.syntax().body,
                scope,
                values::ModuleSource::new(entry).context(&entry.source),
            )
            .expect("the whole update has a checked representation");
            let mut operations = Vec::new();
            ir::walk_expressions(&plan.function.body, &mut |expr| {
                if let ExprKind::Builtin(Builtin::RegisteredInPlace(operator), operands) =
                    &expr.kind
                {
                    operations.push((*operator, operands.len()));
                }
            });
            assert_eq!(operations, [("+", 2)]);
        }
    }

    #[test]
    fn extended_index_statements_keep_their_operands_and_source_order() {
        let graph =
            SourceGraph::single("recv()[lo():hi():-1, *keys()] = rhs()\ndel recv()[lo():hi()]\n")
                .unwrap();
        let entry = &graph.modules[graph.entry];
        let plan = registered_suite(
            &entry.parsed.syntax().body,
            RegisteredScope::Module,
            values::ModuleSource::new(entry).context(&entry.source),
        )
        .unwrap();
        let [
            _,
            Statement::SetItem(_, receiver, key, rhs),
            _,
            Statement::Expr(deletion),
        ] = plan.function.body.as_slice()
        else {
            panic!("expected an item store and delete")
        };
        let mut evaluated = Vec::new();
        for expr in [rhs, receiver, key] {
            ir::walk_operand_expressions(expr, &mut |child| {
                if let ExprKind::Builtin(Builtin::RegisteredName(name), _) = &child.kind {
                    evaluated.push(name.clone());
                }
            });
        }
        assert_eq!(evaluated, ["rhs", "recv", "lo", "hi", "keys"]);
        assert!(
            matches!(&deletion.kind, ExprKind::Builtin(Builtin::DeleteItem, inputs) if inputs.len()==2)
        );
    }

    #[test]
    fn private_dotted_import_keeps_the_root_binding_in_checked_ir() {
        let mut body = ruff_python_parser::parse_module("class C:\n    import __pkg.child\n")
            .unwrap()
            .into_syntax()
            .body;
        let names = imre_ast::lower(&mut body);
        let class = body[0].as_class_def_stmt().unwrap();
        let Some(Statement::Expr(import)) = registered_import(&class.body[0], Some(&names)) else {
            panic!("expected an import statement");
        };
        let ExprKind::Builtin(Builtin::RegisteredImport(ir::NamespaceImport::Modules(modules)), _) =
            import.kind
        else {
            panic!("expected an import operation");
        };
        let [module] = modules.as_slice() else {
            panic!("expected one module")
        };
        assert_eq!(module.name, "__pkg.child");
        assert_eq!(module.asname.as_deref(), Some("_C__pkg"));
        assert!(!module.bind_leaf);
    }

    #[test]
    fn a_registered_callback_does_not_certify_an_invalid_sibling_statement() {
        for invalid in ["n[0]", "undefined_name", "n.bit_length(1)"] {
            for callback in ["sink(obj.bit_length())", "sink(n)"] {
                let source =
                    format!("def act(n: int, obj, sink):\n    {invalid}\n    {callback}\n");
                let graph = SourceGraph::single(&source).unwrap();
                let _ = super::super::lower(&graph);
                assert!(
                    graph.registered_functions().is_empty(),
                    "{invalid}; {callback}"
                );
            }
        }
    }

    #[test]
    fn nested_walruses_share_the_checked_method_and_packed_call() {
        let graph =
            SourceGraph::single("(outer := (inner := obj.method(callback(*args, **kw))))\n")
                .unwrap();
        let entry = &graph.modules[graph.entry];
        let source = values::ModuleSource::new(entry);
        let suite = registered_suite(
            &entry.parsed.syntax().body,
            RegisteredScope::Module,
            source.context(&entry.source),
        )
        .unwrap();
        let mut binds = Vec::new();
        let mut calls = Vec::new();
        ir::walk_expressions(&suite.function.body, &mut |expr| match &expr.kind {
            ExprKind::Bind(slot, rhs) => {
                assert_eq!(expr.ty, rhs.ty);
                binds.push(suite.local_names[slot].name.as_str());
            }
            ExprKind::Invoke { args, .. } => {
                let call = &args.site.python().unwrap().call;
                match &call.target {
                    ir::RegisteredCallTarget::Method { name, .. } => {
                        assert_eq!(call.arguments, ir::RegisteredArguments::Positional);
                        assert_eq!(args.values.len(), 1);
                        calls.push(name.as_str());
                    }
                    ir::RegisteredCallTarget::FixedExec { .. } => panic!("unexpected exec target"),
                    ir::RegisteredCallTarget::Callable { .. } => {
                        assert_eq!(args.values.len(), 2);
                        assert_eq!(
                            call.arguments,
                            ir::RegisteredArguments::Packed(ir::PackedArguments {
                                starred: vec![true],
                                keywords: vec![None],
                            })
                        );
                        calls.push("packed");
                    }
                }
            }
            _ => {}
        });
        assert_eq!(binds, ["outer", "inner"]);
        assert_eq!(calls, ["method", "packed"]);
    }

    #[test]
    fn starred_class_header_carries_its_argument_layout() {
        let graph = SourceGraph::single("@dec\nclass C(*bases, metaclass=meta, **options): pass\n")
            .unwrap();
        let entry = &graph.modules[graph.entry];
        let source = values::ModuleSource::new(entry);
        let suite = registered_suite(
            &entry.parsed.syntax().body,
            RegisteredScope::Module,
            source.context(&entry.source),
        )
        .unwrap();
        let mut headers = 0;
        ir::walk_expressions(&suite.function.body, &mut |expr| {
            if let ExprKind::Builtin(
                Builtin::RegisteredDefinition(ir::DefinitionSite::Class {
                    arguments: Some(arguments),
                    ..
                }),
                operands,
            ) = &expr.kind
            {
                headers += 1;
                assert_eq!(arguments.starred, [true]);
                assert_eq!(arguments.keywords, [Some("metaclass".into()), None]);
                assert_eq!(operands.len(), 4);
            }
        });
        assert_eq!(headers, 1);
    }

    #[test]
    fn checked_string_methods_keep_the_evaluated_receiver() {
        let text = "f'<{value}>'.encode(), f'{value}'.encode()\nf'<{value}>'.enocde(),\n";
        let graph = SourceGraph::single(text).unwrap();
        let module = &graph.modules[graph.entry];
        let source = values::ModuleSource::new(module);
        let prepare = |index: usize| {
            let stmt = module.parsed.syntax().body[index].as_expr_stmt().unwrap();
            inline_operand(
                &stmt.value,
                source.context(text),
                RegisteredScope::Function { is_async: false },
                false,
            )
        };
        let operand = prepare(0).unwrap().unwrap();
        crate::verify::registered(operand.region()).unwrap();
        let mut receivers = Vec::new();
        ir::walk_operand_expressions(&operand.value, &mut |expr| {
            if let ExprKind::Invoke { callable, args } = &expr.kind {
                assert!(
                    matches!(&args.site.python().unwrap().call.target, ir::RegisteredCallTarget::Method { name, .. } if name == "encode")
                );
                receivers.push(callable.ty);
            }
        });
        assert_eq!(receivers, [Type::Str, Type::PythonObject]);
        assert!(prepare(1).is_err());
    }

    #[test]
    fn inline_bind_uses_the_assigning_frame() {
        let prepare = |expr: &str, is_async| {
            let prefix = if is_async { "async " } else { "" };
            let text = format!("{prefix}def run(value):\n    return {expr}\n");
            let graph = SourceGraph::single(&text).unwrap();
            let module = &graph.modules[graph.entry];
            let def = module.parsed.syntax().body[0]
                .as_function_def_stmt()
                .unwrap();
            let expr = def.body[0]
                .as_return_stmt()
                .unwrap()
                .value
                .as_deref()
                .unwrap();
            let literals = values::ModuleSource::new(module);
            inline_operand(
                expr,
                literals.context(&text),
                RegisteredScope::Function { is_async },
                true,
            )
            .ok()
            .flatten()
        };
        let operand = prepare("(saved := (inner := await value))", true).unwrap();
        crate::verify::registered(operand.region()).unwrap();
        assert_eq!(operand.local_names.len(), 2);
        assert!(prepare("(saved := await value)", false).is_none());
        let operand = prepare(
            r#"{"eager": [item for item in [1]], "value": keep((bound := (yield "nested")))}"#,
            false,
        )
        .unwrap();
        crate::verify::registered(operand.region()).unwrap();
        assert!(
            operand
                .local_names
                .values()
                .any(|local| local.name == "bound")
        );
    }

    #[test]
    fn list_and_set_comprehensions_unpack_into_their_result() {
        let text = "result = ([*row for row in rows], {*row for row in rows})\n";
        let graph = SourceGraph::single(text).unwrap();
        let module = &graph.modules[graph.entry];
        let source = values::ModuleSource::new(module);
        let suite = registered_suite(
            &module.parsed.syntax().body,
            RegisteredScope::Module,
            source.context(text),
        )
        .unwrap();
        crate::verify::registered(suite.region()).unwrap();
        let mut collectors = Vec::new();
        ir::walk_expressions(&suite.function.body, &mut |expr| {
            if let ExprKind::Builtin(op @ (Builtin::Append | Builtin::SetAdd), args) = &expr.kind {
                let [out, item] = args.as_slice() else {
                    panic!("expected accumulator and projection");
                };
                assert!(matches!(out.kind, ExprKind::Local(_)));
                assert!(matches!(
                    &item.kind,
                    ExprKind::Builtin(Builtin::RegisteredSpread, inner) if inner.len() == 1
                ));
                collectors.push(matches!(op, Builtin::SetAdd));
            }
        });
        assert_eq!(collectors, [false, true]);
    }

    #[test]
    fn standalone_dictionary_uses_the_current_async_scope() {
        let text = concat!(
            "async def gather(stream, first, second, nested, mapping):\n",
            "    return {0: (saved := [item async for item in stream]),\n",
            "            1: {await first, await second}, 2: keep([await nested]), **await mapping}\n",
        );
        let graph = SourceGraph::single(text).unwrap();
        let module = &graph.modules[graph.entry];
        let def = module.parsed.syntax().body[0]
            .as_function_def_stmt()
            .unwrap();
        let value = def.body[0]
            .as_return_stmt()
            .unwrap()
            .value
            .as_deref()
            .unwrap();
        let literals = values::ModuleSource::new(module);
        let source = literals.context(text);
        let prepare = |is_async, suspended| {
            inline_operand(
                value,
                source,
                RegisteredScope::Function { is_async },
                suspended,
            )
        };
        assert!(prepare(false, true).is_err());
        assert!(prepare(true, false).is_err());
        let operand = prepare(true, true).unwrap().unwrap();
        crate::verify::registered(operand.region()).unwrap();
        assert!(
            operand
                .local_names
                .values()
                .any(|binding| binding.name == "saved")
        );
        let mut awaits = Vec::new();
        ir::walk_operand_expressions(&operand.value, &mut |value| {
            if let ExprKind::Builtin(
                Builtin::RegisteredSuspend(ir::SuspendValue::Await, range),
                _,
            ) = &value.kind
            {
                awaits.push(&text[*range]);
            }
        });
        assert_eq!(
            awaits,
            [
                "await first",
                "await second",
                "await nested",
                "await mapping"
            ]
        );
    }

    #[test]
    fn ordinary_admission_visits_both_parts_of_an_eager_scope() {
        let range = TextRange::default();
        let suspend = operation(
            Builtin::RegisteredSuspend(ir::SuspendValue::Yield, range),
            Type::PythonObject,
            vec![Expr::none()],
        );
        for (step, value, accepted) in [
            (Expr::none(), Expr::none(), true),
            (suspend.clone(), Expr::none(), false),
            (Expr::none(), suspend, false),
        ] {
            let scoped = Expr {
                ty: value.ty,
                kind: ExprKind::Scope {
                    body: vec![Statement::Expr(step)],
                    value: Box::new(value),
                    clear: Vec::new(),
                },
            };
            let body = RegisteredFunction::expression(range, scoped);
            crate::verify::registered(body.region()).unwrap();
            assert_eq!(crate::llvm::registered::supports(&body), accepted);
        }
    }

    #[test]
    fn admission_requires_private_storage_for_a_clear() {
        let mut body = RegisteredFunction::expression(TextRange::default(), Expr::none());
        body.function.locals.push(Type::PythonObject);
        body.function.body.insert(0, Statement::Clear(0));
        assert!(!crate::llvm::registered::supports(&body));
        body.function.body.push(Statement::Scope {
            body: vec![],
            clear: vec![0],
        });
        assert!(crate::llvm::registered::supports(&body));
        body.local_names.insert(
            0,
            LocalBinding {
                name: "value".to_owned(),
                namespace: None,
            },
        );
        assert!(!crate::llvm::registered::supports(&body));
    }

    #[test]
    fn eager_first_iterable_can_delegate_before_the_cursor_exists() {
        let text = "def gather():\n    return [value for value in ((yield from rows) or ())]\n";
        let graph = SourceGraph::single(text).unwrap();
        let module = &graph.modules[graph.entry];
        let function = module.parsed.syntax().body[0]
            .as_function_def_stmt()
            .unwrap();
        let expr = function.body[0]
            .as_return_stmt()
            .unwrap()
            .value
            .as_deref()
            .unwrap();
        let literals = values::ModuleSource::new(module);
        let source = literals.context(text);
        let prepare = |kind, suspended| inline_operand(expr, source, kind, suspended);
        let value = prepare(RegisteredScope::Function { is_async: false }, true)
            .unwrap()
            .expect("the creating generator owns the outer yield");
        crate::verify::registered(value.region()).unwrap();

        let refusal = Some((expr.range(), COMPREHENSION_REFUSAL));
        assert_eq!(prepare(RegisteredScope::Module, false).err(), refusal);
        assert_eq!(
            prepare(RegisteredScope::Function { is_async: true }, true).err(),
            refusal
        );
    }

    #[test]
    fn module_and_function_stores_share_the_registered_dictionary_operand() {
        let graph = SourceGraph::single("out = {key: callback(arg), **more}[index]\n").unwrap();
        let entry = &graph.modules[graph.entry];
        let source = values::ModuleSource::new(entry);
        for module in [false, true] {
            let suite = registered_suite(
                &entry.parsed.syntax().body,
                if module {
                    RegisteredScope::Module
                } else {
                    RegisteredScope::Function { is_async: false }
                },
                source.context(&entry.source),
            )
            .unwrap();
            assert_eq!(
                suite.local_names,
                HashMap::from([(
                    0,
                    LocalBinding {
                        name: "out".into(),
                        namespace: None
                    }
                )])
            );
            let [Statement::Position(..), Statement::Assign(0, operand)] =
                suite.function.body.as_slice()
            else {
                panic!("expected one checkpoint followed by one store")
            };
            let ExprKind::Builtin(Builtin::RegisteredLocation(site, ..), operands) = &operand.kind
            else {
                panic!("expected the subscript's original range")
            };
            let ExprKind::Builtin(Builtin::RegisteredSubscript(index, _), indexed) =
                &operands[0].kind
            else {
                panic!("expected registered subscription")
            };
            assert_eq!(site, index);
            let ExprKind::Builtin(Builtin::RegisteredLocation(..), display) = &indexed[0].kind
            else {
                panic!("expected a located dictionary operand")
            };
            let ExprKind::Dict(entries) = &display[0].kind else {
                panic!("expected a dictionary display")
            };
            let [pair, expansion] = entries.as_slice() else {
                panic!("expected a pair followed by a mapping expansion")
            };
            assert!(pair.key.is_some());
            assert!(expansion.key.is_none());
            let mut names = Vec::new();
            ir::walk_expressions(&suite.function.body, &mut |expr| {
                if let ExprKind::Builtin(Builtin::RegisteredName(name), _) = &expr.kind {
                    names.push(name.clone());
                }
            });
            assert_eq!(names, ["key", "callback", "arg", "more", "index"]);
        }
    }

    fn checked_named_source(text: &str) -> SourceGraph {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app.py");
        std::fs::write(&path, text).unwrap();
        let mut graph = SourceGraph::load(&path, None, &[]).unwrap();
        graph.registered_startup = true;
        crate::checked_lower(&mut graph).unwrap();
        graph
    }

    #[test]
    fn a_checked_python_frame_reads_its_conditionally_assigned_local() {
        let graph = checked_named_source(
            "def read(enabled: bool) -> int:
    try:
        if enabled:
            value: int = 7
    except ValueError:
        pass
    return value
print(read(True))
",
        );
        let functions = graph.registered_functions();
        let frame = &functions[&graph.entry][&TextSize::default()];
        let (&slot, _) = frame
            .local_names
            .iter()
            .find(|(_, binding)| binding.name == "value")
            .unwrap();
        assert_eq!(frame.function.locals[slot], Type::Int);
        let mut stored = false;
        let mut read = false;
        frame.region().walk(&mut |node, _| match node {
            ir::Node::Statement(Statement::Assign(target, value)) if *target == slot => {
                stored |= matches!(&value.without_location().kind, ExprKind::Int(n) if n == "7");
            }
            ir::Node::Expression(Expr {
                kind: ExprKind::Local(source),
                ..
            }) if *source == slot => {
                read = true;
            }
            _ => {}
        });
        assert!(stored && read);
    }

    #[test]
    fn an_unread_scalar_parameter_allows_a_python_frame_to_fall_through() {
        let body = ir::Function {
            params: vec![0],
            returns: Type::PythonObject,
            locals: vec![Type::Str],
            body: vec![Statement::Position(TextRange::default(), None)],
        };
        let frame =
            RegisteredFunction::from_checked_body(body, HashMap::new(), FrameArguments::Python)
                .expect("a fallthrough frame does not read its argument");
        assert!(matches!(frame.inputs, FrameInputs::Protocol { .. }));
    }

    #[test]
    fn a_named_scalar_copy_keeps_its_source_and_live_receiver() {
        let text = concat!(
            "def read_alias(name: str) -> str:\n",
            "    alias = name\n",
            "    return alias.title()\n",
            "import argparse\n",
            "parser = argparse.ArgumentParser()\nparser.add_argument('name')\n",
            "print(read_alias(parser.parse_args().name))\n",
        );
        let (graph, lowered) = caller_cache::observe_lowerings(|| checked_named_source(text));
        let preparations = lowered.iter().filter(|name| *name == "read_alias").count();
        assert_eq!(preparations, 1);
        let functions = graph.registered_functions();
        let frame = &functions[&graph.entry][&TextSize::default()];
        assert!(matches!(frame.inputs, FrameInputs::Protocol { .. }));
        let mut sites = frame.fixed_sites.iter().map(|range| &text[*range]);
        assert_eq!(sites.next(), Some("alias = name"));
        assert_eq!(sites.next(), Some("name"));
        let Statement::Assign(alias, value) = &frame.function.body[0] else {
            panic!("expected the checked assignment");
        };
        assert_eq!(frame.local_names[alias].name, "alias");
        assert!(matches!(value.kind, ExprKind::Local(slot) if frame.function.params == [slot]));
    }

    #[test]
    fn a_scalar_parameter_can_be_forwarded_through_an_object_local() {
        use crate::llvm::registered::frame_inputs;

        for destination in [Type::PythonObject, Type::Str] {
            let result = Expr {
                ty: Type::PythonObject,
                kind: ExprKind::List(vec![Expr::local(1, destination)]),
            };
            let mut frame = RegisteredFunction::expression(TextRange::default(), result);
            frame.function.params = vec![0];
            frame.function.locals = vec![Type::Str, destination];
            for (slot, name) in ["parameter", "alias"].into_iter().enumerate() {
                frame.local_names.insert(
                    slot,
                    LocalBinding {
                        name: name.to_owned(),
                        namespace: None,
                    },
                );
            }
            frame
                .function
                .body
                .insert(0, Statement::Assign(1, Expr::local(0, Type::Str)));
            assert_eq!(
                frame_inputs(&frame, FrameArguments::Python).is_some(),
                destination == Type::PythonObject
            );
        }
    }

    #[test]
    fn a_stub_layout_does_not_discard_the_checked_python_call() {
        let text =
            "from textwrap import dedent\nif __name__ == '__main__':\n    print(dedent(0))\n";
        let graph = checked_named_source(text);
        let (_, initializers) = graph.take_registered_functions();
        let mut calls = Vec::new();
        ir::walk_expressions(&initializers[&graph.entry].function.body, &mut |expr| {
            if let Some(site) = expr.python_call() {
                calls.push(&text[site.range]);
            }
        });
        assert!(calls.contains(&"dedent(0)"), "{calls:?}");
    }

    #[test]
    fn a_prior_definition_keeps_the_nullable_member_obligation() {
        let text = "import re\ndef before() -> int: return 1\ndef select(line: str) -> str:\n    return re.match('x', line).group()\n";
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app.py");
        std::fs::write(&path, text).unwrap();
        for warm in [false, true] {
            let graph = SourceGraph::load(&path, None, &[]).unwrap();
            let module = graph.entry;
            let body = &graph.modules[module].parsed.syntax().body;
            let checks = SourceChecks::default();
            if warm {
                assert!(checks.accepts(&graph, module, body[1].range()));
            }
            assert!(!checks.accepts(&graph, module, body[2].range()));
            assert!(crate::analysis::RegisteredSourceError::only_attributes(
                &graph.registered_source_errors.borrow()[&module],
                body[2].range(),
                |range| &text[range] == "re.match('x', line).group",
            ));
        }
    }

    #[test]
    fn a_replaced_named_frame_keeps_its_source_error() {
        let dir = tempfile::tempdir().unwrap();
        let entry = dir.path().join("main.py");
        std::fs::write(
            &entry,
            "from scores import add\nclass Dormant:\n    async def __aiter__(self):\n        yield 1\nprint(add(19, 23))\n",
        )
        .unwrap();
        let scores = concat!(
            "class Item:\n    values: dict[str, int]\n",
            "    def __init__(self) -> None:\n        self.values = {'a': 1}\n",
            "def independent(obj: Item) -> None:\n    obj.values['a'] = 'wrong'\n",
            "print('entry')\ndef add(value: int, increment: int) -> int:\n",
            "    return value + increment\n",
        );
        std::fs::write(dir.path().join("scores.py"), scores).unwrap();
        let mut graph = SourceGraph::load(&entry, None, &[]).unwrap();
        graph.registered_startup = true;
        crate::checked_lower(&mut graph).unwrap();
        let module = graph.python_file("scores").unwrap();
        let store = "obj.values['a'] = 'wrong'";
        let start = TextSize::try_from(scores.find(store).unwrap()).unwrap();
        let range = TextRange::at(start, TextSize::of(store));
        assert!(
            graph.registered_source_errors.borrow()[&module]
                .iter()
                .any(|error| error.range() == Some(range))
        );
    }

    #[test]
    fn checked_frame_assigns_joined_fields_to_a_string_local() {
        let graph = checked_named_source(
            "def message(value: str) -> str:
    text = 'empty'
    if value == 'x':
        text = f'<{value}>'
    return text
print(message('x'))
",
        );
        let functions = graph.registered_functions();
        let frame = &functions[&graph.entry][&TextSize::default()];
        assert!(matches!(frame.inputs, FrameInputs::Protocol { .. }));
        let mut join_type = None;
        ir::walk_expressions(&frame.function.body, &mut |value| {
            if matches!(value.kind, ExprKind::Builtin(Builtin::FormatJoin, _)) {
                join_type = Some(value.ty);
            }
        });
        assert_eq!(join_type, Some(Type::Str));
    }

    #[test]
    fn a_display_keeps_its_live_scalar_elements() {
        let text = "def show(label: str) -> str:
    return f'{[label]} {(label,)} { {label} }'
def raw(label: str) -> str:
    message = f'{[label]}'
    return label * 2
print(show('ok'), raw('ok'))
";
        let graph = checked_named_source(text);
        let functions = graph.registered_functions();
        let entries = &functions[&graph.entry];
        assert!(matches!(
            entries[&TextSize::default()].inputs,
            FrameInputs::Protocol { .. }
        ));
        let raw = &graph.modules[graph.entry].parsed.syntax().body[1];
        assert!(!entries.contains_key(&raw.start()));
    }

    #[test]
    fn a_generator_iterates_the_live_scalar_argument() {
        let text = "def render(words: str) -> str:
    return ''.join(word.upper() for word in words)
def raw(words: str) -> str:
    joined = ''.join(word.upper() for word in words)
    return words * 2
print(render('ok'), raw('ok'))
";
        let graph = checked_named_source(text);
        let functions = graph.registered_functions();
        let entries = &functions[&graph.entry];
        let FrameInputs::Protocol { sites } = &entries[&TextSize::default()].inputs else {
            panic!("expected the live generator iterable");
        };
        assert!(matches!(
            sites.as_ref(),
            [FrameSite::Generator(range)] if &graph.modules[graph.entry].source[*range] == "word.upper() for word in words"
        ));
        let raw = &graph.modules[graph.entry].parsed.syntax().body[1];
        assert!(!entries.contains_key(&raw.start()));
    }

    #[test]
    fn checked_function_asserts_against_the_live_global() {
        let text =
            "events = ['truth']\ndef current():\n    assert events == ['truth']\ncurrent()\n";
        let graph = checked_named_source(text);
        let current = graph.modules[graph.entry].parsed.syntax().body[1]
            .as_function_def_stmt()
            .unwrap();
        let assertion = current.body[0].as_assert_stmt().unwrap();
        let functions = graph.registered_functions();
        let body = &functions[&graph.entry][&current.start()].function.body;
        let [
            Statement::Position(range, None),
            Statement::If(_, pass, fail),
        ] = &body[..]
        else {
            panic!("expected the checked assertion to fall through")
        };
        assert_eq!(*range, assertion.test.range());
        assert!(pass.is_empty());
        let [Statement::Expr(failure)] = &fail[..] else {
            panic!("expected the failure on the false branch");
        };
        let ExprKind::Builtin(Builtin::RegisteredAssertionFailure { statement, test }, args) =
            &failure.kind
        else {
            panic!("expected a Python assertion failure")
        };
        assert_eq!(
            (*statement, *test),
            (assertion.start(), assertion.test.range())
        );
        assert!(args.is_empty());
    }

    #[test]
    fn checked_frame_keeps_the_process_field_and_receiver_only_method() {
        let graph = checked_named_source(
            r#"from subprocess import CompletedProcess

def clean(result: CompletedProcess[str]) -> str:
    text = result.stdout
    return text.strip()

print(clean(CompletedProcess([], 0, " data ")))
"#,
        );
        let function = graph.modules[graph.entry].parsed.syntax().body[1]
            .as_function_def_stmt()
            .unwrap();
        let functions = graph.registered_functions();
        let body = &functions[&graph.entry][&function.start()];
        assert!(matches!(
            body.function.body.as_slice(),
            [Statement::Assign(_, _), Statement::Return(_)]
        ));
        let attributes = body.fixed_attributes.as_ref().unwrap();
        assert!(attributes.values().any(|field| matches!(field,
            FixedAttribute::Load(name) if name == "stdout")));
        assert!(attributes.values().any(|field| matches!(field,
            FixedAttribute::Call(None, site)
                if matches!(&site.target, ir::RegisteredCallTarget::Method { name, .. } if name == "strip"))));
    }

    #[test]
    fn checked_frames_call_the_live_imported_member() {
        let source = r#"import argparse
import subprocess
from pathlib import Path

args = argparse.ArgumentParser().parse_args()

def run(command: list[str], *, cwd: Path) -> None:
    subprocess.run(command, cwd=cwd, check=True)

def capture(command: list[str], *, cwd: Path) -> str:
    result = subprocess.run(command, cwd=cwd, check=True, capture_output=True, text=True)
    return result.stdout.strip()

run(["echo", "run"], cwd=Path("."))
print(capture(["echo", "capture"], cwd=Path(".")))
"#;
        let graph = checked_named_source(source);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app.py");
        std::fs::write(&path, source).unwrap();
        let mut file = SourceGraph::load(&path, None, &[]).unwrap();
        crate::checked_lower(&mut file).unwrap();
        let bodies = graph.registered_functions();
        for definition in graph.modules[graph.entry]
            .parsed
            .syntax()
            .body
            .iter()
            .filter_map(ast::Stmt::as_function_def_stmt)
        {
            let retained = bodies
                .get(&graph.entry)
                .and_then(|functions| functions.get(&definition.start()));
            let body = retained.unwrap_or_else(|| panic!("missing {}", definition.name));
            let mut calls = Vec::new();
            ir::walk_expressions(&body.function.body, &mut |value| {
                let Some(site) = value.python_call() else {
                    return;
                };
                let name: &str = match &site.target {
                    ir::RegisteredCallTarget::FixedExec { .. } => panic!("unexpected exec target"),
                    ir::RegisteredCallTarget::Callable { .. } => {
                        let ExprKind::Invoke { callable, .. } = &value.kind else {
                            panic!("expected a checked invocation")
                        };
                        let ExprKind::Builtin(Builtin::RegisteredAttribute { name, .. }, _) =
                            &callable.without_location().kind
                        else {
                            // Other positional invocations are unrelated to the imported member.
                            return;
                        };
                        name
                    }
                    ir::RegisteredCallTarget::Method { name, .. } => name,
                };
                calls.push(name.to_owned());
            });
            let expected: &[&str] = if definition.name.as_str() == "run" {
                &["run"]
            } else {
                let returned = definition.body.last().unwrap().as_return_stmt().unwrap();
                let range = returned.value.as_ref().unwrap().range();
                let functions = file.registered_functions();
                let placed = &functions[&file.entry][&definition.start()];
                assert!(matches!(placed.fixed_attribute(range),
                    Some(FixedAttribute::Call(None, site))
                        if matches!(&site.target, ir::RegisteredCallTarget::Method { name, .. } if name == "strip")));
                &["run", "strip"]
            };
            assert_eq!(calls, expected);
        }
    }

    #[test]
    fn a_file_keeps_the_checked_command_inside_its_exception_handler() {
        let source = r#"import json
import subprocess
import sys

def process(path: str, cwd: str) -> None:
    try:
        subprocess.run(["tool", path], cwd=cwd, capture_output=True, check=False)
    except ＦileNotFoundError:
        pass
        raise

def main() -> None:
    args = json.load(sys.stdin)
    process(args["path"], args["cwd"])

if __name__ == "__main__":
    main()
"#;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("app.py");
        std::fs::write(&path, source).unwrap();
        let mut graph = SourceGraph::load(&path, None, &[]).unwrap();
        crate::checked_lower(&mut graph).unwrap();
        let module = &graph.modules[graph.entry];
        let def = module.parsed.syntax().body[3]
            .as_function_def_stmt()
            .unwrap();
        let functions = graph.registered_functions();
        let frame = &functions[&graph.entry][&def.start()];
        let [Statement::Try { handlers, .. }] = frame.function.body.as_slice() else {
            panic!("expected the original fixed protected suite");
        };
        assert!(matches!(&handlers[0].matcher,
            ir::ExceptionMatcher::WrittenCategory(_, range, name)
                if name == "FileNotFoundError" && &source[*range] == "ＦileNotFoundError"));
        assert!(matches!(
            handlers[0].body.last(),
            Some(Statement::Raise(None))
        ));
        let protected = def.body[0].as_try_stmt().unwrap();
        assert!(matches!(
            frame.fixed_attribute(protected.body[0].range()),
            Some(FixedAttribute::Call(..))
        ));
    }

    #[test]
    fn a_checked_frame_keeps_the_parser_truth_test_and_protected_suite() {
        let text = r#"import argparse
import tempfile

DEFAULT = "here"
def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repository", default=DEFAULT, help=f"repository {DEFAULT}")
    args = parser.parse_args()
    value = args.repository
    if not value:
        return
    with tempfile.TemporaryDirectory() as directory:
        print(directory)

main()
"#;
        let graph = checked_named_source(text);
        let bodies = graph.registered_functions();
        let main = bodies[&graph.entry].values().next().unwrap();
        let mut constructed = false;
        let mut registered = false;
        let mut negated = false;
        let mut protected = false;
        main.region().walk(&mut |node, _| match node {
            ir::Node::Expression(value) => match &value.kind {
                ExprKind::Builtin(Builtin::RegisteredAttribute { name, .. }, _) if name == "ArgumentParser" => constructed = true,
                ExprKind::Invoke { args, .. } => {
                    registered |= matches!(&args.site.python().unwrap().call.target, ir::RegisteredCallTarget::Method { name, .. } if name == "add_argument");
                }
                ExprKind::Unary(ast::UnaryOp::Not, operand) => {
                    negated |= matches!(&operand.kind, ExprKind::Builtin(Builtin::Bool, args)
                        if matches!(args.as_slice(), [value] if value.ty == Type::PythonObject));
                }
                _ => {}
            },
            ir::Node::Statement(Statement::With { manager, binding: Some(binding), body, .. }) => {
                protected = manager.ty == Type::PythonObject
                    && !binding.body.is_empty()
                    && body.iter().any(|stmt| matches!(stmt, Statement::Expr(value) if value.without_location().is_registered_invoke()));
            }
            _ => {}
        });
        assert!(constructed && registered && negated && protected);
    }

    #[test]
    fn a_checked_method_evaluates_its_computed_receiver_before_arguments() {
        let text = r#"import argparse
args = argparse.ArgumentParser().parse_args()
class Holder:
    def selected(self, a: str, b: str) -> str:
        return a + b
def factory() -> Holder:
    return Holder()
def choices() -> list[Holder]:
    return [Holder()]
def first() -> str:
    return "a"
def second() -> str:
    return "b"
def direct() -> str:
    return factory().selected(first(), second())
def indexed() -> str:
    return choices()[0].selected(first(), second())
exec("print(1)")
"#;
        let graph = checked_named_source(text);
        let bodies = graph.registered_functions();
        for (name, producer) in [("direct", "factory"), ("indexed", "choices")] {
            let start = TextSize::try_from(text.find(&format!("def {name}(")).unwrap()).unwrap();
            let mut names = Vec::new();
            let mut selections = 0;
            bodies[&graph.entry][&start].region().walk(&mut |node, _| {
                let ir::Node::Expression(value) = node else { return };
                match &value.kind {
                    ExprKind::Invoke { args, .. }
                        if matches!(&args.site.python().unwrap().call.target, ir::RegisteredCallTarget::Method { name, .. } if name == "selected") => {
                            assert_eq!(args.values.len(), 2);
                            selections += 1;
                        }
                    ExprKind::Invoke { callable, .. } => {
                        if let ExprKind::Builtin(Builtin::RegisteredName(name), _) = &callable.without_location().kind {
                            names.push(name.clone());
                        }
                    }
                    _ => {}
                }
            });
            assert_eq!(selections, 1);
            assert_eq!(names, [producer, "first", "second"]);
        }
    }

    #[test]
    fn a_function_local_import_keeps_its_checked_body_and_runtime_binding() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let entry = root.join("app.py");
        std::fs::write(
            &entry,
            r#"def publish(value: int, send):
    from helper import adjusted
    send(adjusted(value))


def callback(x):
    print(x)


publish(5, callback)
"#,
        )
        .unwrap();
        std::fs::write(
            root.join("helper.py"),
            "def adjusted(value):\n    return value * 3 + 1\n",
        )
        .unwrap();

        let mut graph = SourceGraph::load(&entry, Some(root), &[]).unwrap();
        assert!(graph.python_file("helper").is_none());
        crate::checked_lower(&mut graph).unwrap();

        let publish = graph.modules[graph.entry].parsed.syntax().body[0].start();
        let bodies = graph.registered_functions();
        let body = &bodies[&graph.entry][&publish].function;
        let mut imports = 0;
        let mut calls = 0;
        ir::walk_expressions(&body.body, &mut |expr| match &expr.kind {
            ExprKind::Builtin(
                Builtin::RegisteredImport(ir::NamespaceImport::From { module, names, .. }),
                _,
            ) if module == "helper"
                && names
                    .iter()
                    .any(|(name, binding)| name == "adjusted" && binding.is_none()) =>
            {
                imports += 1
            }
            ExprKind::Invoke { .. } => calls += 1,
            _ => {}
        });
        assert_eq!((imports, calls), (1, 2));
    }

    #[test]
    fn imported_reduction_retains_its_registered_in_place_update() {
        let project = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/inference/imported");
        let mut graph = SourceGraph::load(&project.join("main.py"), Some(&project), &[]).unwrap();
        graph.registered_startup = true;
        crate::checked_lower(&mut graph).unwrap();
        let owner = graph.python_file("package.worker").unwrap();
        let definition = graph.modules[owner]
            .parsed
            .syntax()
            .body
            .iter()
            .find_map(|stmt| {
                stmt.as_function_def_stmt()
                    .filter(|def| def.name.as_str() == "compute")
            })
            .unwrap();
        let bodies = graph.registered_functions();
        let checked = &bodies[&owner][&definition.start()];
        let mut operations = Vec::new();
        ir::walk_expressions(&checked.function.body, &mut |value| {
            if let ExprKind::Builtin(Builtin::RegisteredInPlace(op), _) = value.kind {
                operations.push(op);
            }
        });
        assert_eq!(operations, ["+"]);
    }

    #[test]
    fn an_imported_checked_callback_keeps_its_registered_body() {
        let project = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/registered_imported_callback");
        let entry = project.join("callable_effect.py");
        let mut graph = SourceGraph::load(&entry, Some(&project), &[]).unwrap();
        let (result, lowered) =
            caller_cache::observe_lowerings(|| crate::checked_lower(&mut graph).unwrap());
        assert_eq!(
            lowered.iter().filter(|name| *name == "publish").count(),
            1,
            "{lowered:?}"
        );
        if let crate::CheckedProgram::Runtime(reason) = result {
            panic!("{reason}");
        }
        let provider = graph.python_file("publish").unwrap();
        let site = graph.modules[provider].parsed.syntax().body[0].start();
        let registered = graph.registered_functions();
        let body = &registered[&provider][&site].function;
        let params: Vec<_> = body.params.iter().map(|&slot| body.locals[slot]).collect();
        assert_eq!(params, [Type::Int, Type::Int, Type::PythonObject]);
        let mut adds = 0;
        let mut callbacks = 0;
        ir::walk_expressions(&body.body, &mut |expr| match expr.kind {
            ExprKind::Binary(Binary::Add, ..) => adds += 1,
            ExprKind::Invoke { .. } => callbacks += 1,
            _ => {}
        });
        assert_eq!((adds, callbacks), (1, 1));

        let source = &graph.modules[graph.entry].source;
        let init = TextSize::try_from(source.find("def __init__").unwrap()).unwrap();
        let method = &registered[&graph.entry][&init];
        let fields = method
            .function
            .body
            .iter()
            .map(|statement| match statement {
                Statement::SetField(_, ir::StoreField::Written(_, name), _) => name.as_str(),
                _ => panic!("expected a checked field store"),
            });
        assert_eq!(fields.collect::<Vec<_>>(), ["name", "fail"]);
    }

    #[test]
    fn a_live_context_checks_its_declared_function_once() {
        let text = r#"import urllib.request

def invoke(fn):
    fn()

def available(url: str) -> bool:
    req = urllib.request.Request(url)
    try:
        with urllib.request.urlopen(req) as reply:
            if reply.status == 200:
                return True
            raise ValueError("bad status")
    except OSError:
        return False
"#;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("app.py");
        std::fs::write(&path, text).unwrap();
        let mut graph = SourceGraph::load(&path, None, &[]).unwrap();
        let (_, lowered) =
            caller_cache::observe_lowerings(|| crate::checked_lower(&mut graph).unwrap());
        assert_eq!(
            lowered.iter().filter(|name| *name == "available").count(),
            1
        );
        let start = TextSize::try_from(text.find("def available").unwrap()).unwrap();
        let functions = graph.registered_functions();
        let frame = &functions[&graph.entry][&start].function;
        assert_eq!(frame.returns, Type::PythonObject);
        assert!(matches!(frame.body.last(), Some(Statement::Try { .. })));

        let invalid = text.replace(
            "if reply.status",
            "status: int = True\n            if reply.status",
        );
        std::fs::write(&path, invalid).unwrap();
        let error = crate::check_file(&path, None).unwrap_err();
        assert!(
            error.ends_with(
                ":10:27: Imre: expected int, got bool; annotations are enforced contracts"
            ),
            "{error}"
        );
    }

    #[test]
    fn an_inferred_procedure_keeps_its_checked_registered_body() {
        let source = concat!(
            "probe = map(int, '12')\n",
            "def emit(*, label: str) -> str:\n    return label\n",
            "def publish(label: str):\n    emit(label=label)\n",
            "publish('ready')\n",
        );
        let graph = SourceGraph::single(source).unwrap();
        let (_, lowered) = caller_cache::observe_lowerings(|| super::super::lower(&graph));
        assert_eq!(
            lowered.iter().filter(|name| *name == "publish").count(),
            1,
            "{lowered:?}"
        );
        let site = graph.modules[graph.entry].parsed.syntax().body[2].start();
        assert!(graph.registered_functions()[&graph.entry].contains_key(&site));
    }

    #[test]
    fn recovery_reuses_the_solved_body_and_lowers_the_missing_callback() {
        let mut graph = SourceGraph::single(concat!(
            "def square(n: int) -> int:\n    return n * n\n",
            "def publish(send) -> None:\n    send(1)\n",
        ))
        .unwrap();
        let (_, lowered) =
            caller_cache::observe_lowerings(|| crate::checked_lower(&mut graph).unwrap());
        let module = graph.entry;
        let definitions = &graph.modules[module].parsed.syntax().body;
        for name in ["square", "publish"] {
            assert_eq!(
                lowered.iter().filter(|function| *function == name).count(),
                1
            );
        }
        let bodies = graph.registered_functions();
        assert!(
            definitions
                .iter()
                .all(|def| bodies[&module].contains_key(&def.start()))
        );
    }

    #[test]
    fn solved_reduction_keeps_the_registered_loop_and_python_calls() {
        let graph = SourceGraph::single(concat!(
            "def send_result(x: int, sink):\n    sink(x)\n",
            "def reduction(s: str) -> int:\n",
            "    values = list(map(int, s))\n",
            "    return sum([n * (n + 1) for n in values])\n",
        ))
        .unwrap();
        let (result, lowered) = caller_cache::observe_lowerings(|| super::super::lower(&graph));
        assert!(result.is_err());
        assert_eq!(
            lowered.iter().filter(|name| *name == "reduction").count(),
            1,
            "{lowered:?}"
        );
        let registered = graph.registered_functions();
        let site = graph.modules[graph.entry].parsed.syntax().body[1].start();
        let body = &registered[&graph.entry][&site].function.body;
        let mut calls = 0;
        let mut multiplies = 0;
        let mut advances = 0;
        ir::walk_expressions(body, &mut |expr| match &expr.kind {
            ExprKind::Binary(Binary::Multiply, ..) => multiplies += 1,
            ExprKind::Invoke { .. } => calls += 1,
            _ => {}
        });
        ir::walk_statements(body, &mut |stmt| {
            if matches!(stmt, Statement::IterNext { .. }) {
                advances += 1;
            }
        });
        assert_eq!((calls, multiplies, advances), (3, 1, 1));
    }

    #[test]
    fn imported_reduction_and_callback_lower_once_at_their_owning_definitions() {
        let dir = tempfile::tempdir().unwrap();
        let source = concat!(
            "def reduction(s: str) -> int:\n",
            "    values = list(map(int, s))\n",
            "    return sum([n * (n + 1) for n in values])\n",
            include_str!("../../../../tests/fixtures/registered_imported_callback/publish.py"),
        );
        std::fs::write(dir.path().join("publish.py"), source).unwrap();
        let entry = dir.path().join("app.py");
        let caller = concat!(
            "from publish import reduction\n",
            include_str!(
                "../../../../tests/fixtures/registered_imported_callback/callable_effect.py"
            ),
            "\nprint(reduction(input()))\n",
        );
        std::fs::write(&entry, caller).unwrap();
        let mut graph = SourceGraph::load(&entry, Some(dir.path()), &[]).unwrap();
        let (result, lowered) =
            caller_cache::observe_lowerings(|| crate::checked_lower(&mut graph).unwrap());
        if let crate::CheckedProgram::Runtime(reason) = result {
            panic!("{reason}");
        }
        let provider = graph.python_file("publish").unwrap();
        let registered = graph.registered_functions();
        for stmt in &graph.modules[provider].parsed.syntax().body[..2] {
            let name = stmt.as_function_def_stmt().unwrap().name.as_str();
            assert!(registered[&provider].contains_key(&stmt.start()), "{name}");
            assert_eq!(
                lowered.iter().filter(|n| *n == name).count(),
                1,
                "{name}: {lowered:?}"
            );
        }
    }

    #[test]
    fn representation_failure_keeps_its_original_diagnostic() {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("app.py");
        std::fs::write(
            &main,
            concat!(
                "import types\n",
                "def bump(x: int) -> int:\n    return x + 1\n",
                "Active = types.new_class('Active')\n",
                "print(bump(7))\n",
            ),
        )
        .unwrap();
        let graph = SourceGraph::load(&main, None, &[]).unwrap();
        let Err(report) = super::super::lower(&graph) else {
            panic!("a registered class factory is required")
        };
        assert!(
            report.contains("types.new_class requires native Python object operations"),
            "{report}"
        );
    }

    #[test]
    fn ordinary_async_generator_retains_its_pull_and_yield() {
        let mut graph = SourceGraph::single(concat!(
            "async def selected(response):\n",
            "    async for part in response.aiter_bytes():\n",
            "        if part:\n",
            "            yield part\n",
        ))
        .unwrap();
        crate::checked_lower(&mut graph).unwrap();
        let table = graph.registered_functions();
        let body = &table[&graph.entry][&TextSize::default()];
        crate::verify::registered(body.region()).unwrap();
        let mut pulls = 0;
        let mut yields = 0;
        ir::walk_statements(&body.function.body, &mut |stmt| {
            if matches!(
                stmt,
                Statement::IterNext {
                    kind: ir::IterNextKind::Async(_),
                    ..
                }
            ) {
                pulls += 1;
            }
        });
        ir::walk_expressions(&body.function.body, &mut |expr| {
            if matches!(
                expr.kind,
                ExprKind::Builtin(Builtin::RegisteredSuspend(ir::SuspendValue::Yield, _), _)
            ) {
                yields += 1;
            }
        });
        assert_eq!((pulls, yields), (1, 1));
    }

    #[test]
    fn ordinary_generator_calls_retain_their_checked_body_and_typed_neighbor() {
        let graph = SourceGraph::single(concat!(
            "def bump(value: int) -> int:\n",
            "    return value + 1\n",
            "def values():\n",
            "    for item in (1, 2):\n",
            "        yield item\n",
            "for value in values(): print(bump(5), value)\n",
        ))
        .unwrap();
        let _ = super::super::lower(&graph);
        let module = &graph.modules[graph.entry];
        let table = graph.registered_functions();
        let bodies = &table[&graph.entry];
        let body = &bodies[&module.parsed.syntax().body[1].start()]
            .function
            .body;
        let mut suspension = false;
        ir::walk_expressions(body, &mut |expr| {
            suspension |= matches!(
                expr.kind,
                ExprKind::Builtin(Builtin::RegisteredSuspend(..), _)
            )
        });
        assert!(suspension);
        let body = &bodies[&module.parsed.syntax().body[0].start()]
            .function
            .body;
        let mut addition = false;
        ir::walk_expressions(body, &mut |expr| {
            addition |=
                expr.ty == Type::Int && matches!(expr.kind, ExprKind::Binary(ir::Binary::Add, ..))
        });
        assert!(addition);
    }

    #[test]
    fn solved_scalar_arithmetic_uses_the_registered_entry() {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("__main__.py");
        std::fs::write(
            &main,
            "def bump(x: int) -> int:\n    return x + 1\n\nprint(bump(7))\n",
        )
        .unwrap();
        let options = crate::CompileOptions {
            path_entry: true,
            module_entry: Some("__main__".to_owned()),
            ..crate::CompileOptions::default()
        };
        let ir = crate::compile_file(dir.path(), None, &options)
            .unwrap()
            .llvm_ir;
        let bump = registered_body_at(&ir, "__main__.py", 2);
        assert!(bump.contains("call i64 @int_add("));
        assert!(bump.contains("call i64 @imre_dyn_exact_int_word("));
    }

    #[test]
    fn retained_list_membership_caller_keeps_its_checked_integer_addition() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("__main__.py"),
            include_str!("../../../../tests/fixtures/containers/int_list_membership.py"),
        )
        .unwrap();
        let options = crate::CompileOptions {
            path_entry: true,
            module_entry: Some("__main__".to_owned()),
            ..crate::CompileOptions::default()
        };
        let ir = crate::compile_file(dir.path(), None, &options)
            .unwrap()
            .llvm_ir;
        let main = registered_body_at(&ir, "__main__.py", 14);
        assert!(main.contains("call i64 @imre_dyn_exact_int_word("));
        assert!(main.contains("call i64 @int_add("));
    }

    #[test]
    fn a_late_import_keeps_the_selected_sources_operand_types() {
        let dir = tempfile::tempdir().unwrap();
        let entry = dir.path().join("app.py");
        std::fs::write(
            &entry,
            concat!(
                "import sys\nsys.path = list(sys.path)\nimport prices\n",
                "print(prices.add(13, 27), prices.join('a', 'b'), prices.sub(27, 13))\n",
                "print(prices.bump([4]), prices.remainder(7, -3))\n",
            ),
        )
        .unwrap();
        std::fs::write(
            dir.path().join("prices.py"),
            concat!(
                "def add(n: int, other: int):\n    return n + other\n",
                "def join(n: str, other: str):\n    return n + other\n",
                "def sub(n: int, other: int):\n    return n - other\n",
                "def bump(values: list[int]):\n    return values[0] + 1\n",
                "def remainder(a: int, b: int) -> int:\n    return a % b\n",
            ),
        )
        .unwrap();
        let ir = crate::compile_file(&entry, None, &Default::default())
            .unwrap()
            .llvm_ir;
        for (line, native, projections) in [
            (2, "@int_add(", 2),
            (6, "@int_sub(", 2),
            (8, "@int_add(", 1),
            (10, "@int_mod(", 2),
        ] {
            let body = registered_body_at(&ir, "prices.py", line);
            assert_eq!(
                body.matches("call i64 @imre_dyn_exact_int_word(").count(),
                projections
            );
            assert!(body.contains(native) && body.contains("call i64 @imre_dyn_op_at("));
        }
        assert!(!registered_body_at(&ir, "prices.py", 4).contains("@int_add("));
        let constant = |initializer: &str| {
            ir.lines()
                .find_map(|line| {
                    let (name, data) = line.split_once(" = private unnamed_addr constant ")?;
                    (data == initializer).then_some(name)
                })
                .unwrap()
        };
        let modulo = constant(r#"[2 x i8] c"%\00""#);
        let position = constant("[4 x i32] [i32 10, i32 11, i32 13, i32 16]");
        let operation = format!("call i64 @imre_dyn_op_at(ptr %ctx, ptr {modulo},");
        assert!(
            registered_body_at(&ir, "prices.py", 10).lines().any(
                |line| line.contains(&operation) && line.ends_with(&format!("ptr {position})"))
            )
        );
    }

    #[test]
    fn a_python_invocation_forwards_the_checked_argument_handle() {
        for (read, argument, retained) in [
            ("value", "value", true),
            ("abs('bad')", "value", false),
            ("value", "value + 1", true),
        ] {
            let graph = checked_named_source(&format!(
                "def run(value: int) -> int:\n    return sum((item for item in [1]), origin(lambda: {read}, {argument}))\n\n\
                 def origin(read: object, value: int) -> int:\n    return 0\n\nprint(run(1))\n"
            ));
            let bodies = graph.registered_functions();
            assert_eq!(
                bodies
                    .get(&graph.entry)
                    .is_some_and(|functions| functions.contains_key(&TextSize::default())),
                retained,
                "{read}, {argument}",
            );
        }
    }

    #[test]
    fn named_function_compares_the_original_checked_scalar_argument() {
        for (annotation, left, right, operand, result) in [
            ("int", "0", "2", Type::Int, Type::Bool),
            (
                "Literal['diff', 'github']",
                "'diff'",
                "'github'",
                Type::Str,
                Type::PythonObject,
            ),
        ] {
            let source = format!(
                "from typing import Literal
def choose(mode: {annotation}) -> int:
    if mode == {left}:
        return 7
    return 9
print(choose({left}), choose({right}))
"
            );
            let start = TextSize::try_from(source.find("def choose").unwrap()).unwrap();
            let graph = checked_named_source(&source);
            let functions = graph.registered_functions();
            let frame = &functions[&graph.entry][&start];
            crate::verify::registered(frame.region()).unwrap();
            let mut comparisons = Vec::new();
            ir::walk_expressions(&frame.function.body, &mut |expr| {
                if let ExprKind::Compare(values, _) = &expr.kind {
                    comparisons.push((values[0].ty, values[1].ty, expr.ty));
                }
            });
            assert_eq!(comparisons, [(operand, operand, result)]);
        }
    }

    #[test]
    fn registered_arguments_share_the_checked_integer_body() {
        let graph = SourceGraph::single(
            "def report(x: int, sink, y: int):\n    answer = x + y\n    pass\n    sink(answer)\n",
        )
        .unwrap();
        assert!(super::super::lower(&graph).is_err());
        let bodies = graph.registered_functions();
        let body = &bodies[&graph.entry][&TextSize::default()].function;
        assert_eq!(
            body.locals,
            [Type::Int, Type::PythonObject, Type::Int, Type::Int]
        );
        let mut checkpoints = Vec::new();
        let mut checked_adds = 0;
        let mut invokes = 0;
        ir::walk_nodes(&body.body, &mut |node, _| match node {
            ir::Node::Statement(Statement::Position(range, _)) => {
                checkpoints.push(&graph.modules[graph.entry].source[*range]);
            }
            ir::Node::Expression(Expr {
                kind: ExprKind::Binary(Binary::Add, ..),
                ..
            }) => checked_adds += 1,
            ir::Node::Expression(Expr {
                kind: ExprKind::Invoke { args, .. },
                ty,
            }) => {
                let site = args.site.python().unwrap();
                let source = &graph.modules[graph.entry].source;
                assert_eq!(&source[site.call.range], "sink(answer)");
                let ir::RegisteredCallTarget::Callable {
                    end,
                    suspended_name: Some(name),
                    ..
                } = site.call.target
                else {
                    panic!("expected a named callable")
                };
                assert_eq!(&source[name], "sink");
                assert_eq!(name.end(), end);
                assert!(args.order.is_empty());
                assert_eq!(*ty, Type::PythonObject);
                invokes += 1;
            }
            _ => {}
        });
        assert_eq!(checkpoints, ["answer = x + y", "pass", "sink(answer)"]);
        assert_eq!((checked_adds, invokes), (1, 1));
    }

    #[test]
    fn computed_positional_calls_bind_the_evaluated_callee() {
        let source = concat!(
            "def apply(factory, obj, value, exec):\n",
            "    factory()(value)\n",
            "    (obj.action if value else factory)(value)\n",
            "    obj.action(value)\n",
            "    factory(*value)\n",
            "    exec('class Hidden: pass', value)\n",
        );
        let graph = SourceGraph::single(source).unwrap();
        let _ = super::super::lower(&graph);
        let functions = graph.registered_functions();
        let checked = &functions[&graph.entry][&TextSize::default()];
        assert!(crate::llvm::registered::supports(checked));
        let mut calls = Vec::new();
        ir::walk_expressions(&checked.function.body, &mut |expr| {
            if let ExprKind::Invoke { callable, args } = &expr.kind {
                let site = args.site.python().unwrap();
                if matches!(site.call.target, ir::RegisteredCallTarget::Method { .. }) {
                    assert_eq!(site.call.arguments, ir::RegisteredArguments::Positional);
                    calls.push((&source[site.call.range], "method"));
                    return;
                }
                let range = callable
                    .written_range()
                    .expect("expected the checked callable operand");
                assert!(
                    matches!(&site.call.target, ir::RegisteredCallTarget::Callable { end, .. } if *end == range.end())
                );
                let kind = match &callable.without_location().kind {
                    ExprKind::Local(_)
                        if matches!(site.call.arguments, ir::RegisteredArguments::Packed(_)) =>
                    {
                        "packed"
                    }
                    ExprKind::Local(_) => "local",
                    ExprKind::Invoke { .. } => "result",
                    ExprKind::Conditional(..) => "conditional",
                    _ => panic!("unexpected positional callable"),
                };
                calls.push((&source[site.call.range], kind));
            }
        });
        assert_eq!(
            calls,
            [
                ("factory()(value)", "result"),
                ("factory()", "local"),
                ("(obj.action if value else factory)(value)", "conditional"),
                ("obj.action(value)", "method"),
                ("factory(*value)", "packed"),
                ("exec('class Hidden: pass', value)", "local"),
            ]
        );
    }

    #[test]
    fn callback_result_uses_python_addition_in_the_checked_body() {
        let graph = SourceGraph::single(
            "def report(x: int, sink, y: int):\n    first = x + y\n    reply = sink(first)\n    sink(reply + y)\n",
        ).unwrap();
        assert!(super::super::lower(&graph).is_err());
        let functions = graph.registered_functions();
        let checked = &functions[&graph.entry][&TextSize::default()];
        assert!(crate::llvm::registered::supports(checked));
        let mut additions = Vec::new();
        ir::walk_expressions(&checked.function.body, &mut |expr| {
            if let ExprKind::Binary(Binary::Add, ref left, ref right) = expr.kind {
                additions.push((left.ty, right.ty, expr.ty));
            }
        });
        assert_eq!(
            additions,
            [
                (Type::Int, Type::Int, Type::Int),
                (Type::PythonObject, Type::Int, Type::PythonObject),
            ]
        );
    }

    #[test]
    fn registered_conditional_keeps_the_object_test_and_mixed_arm_types() {
        let graph = SourceGraph::single(concat!(
            "def report(flag, x: int, y: int, sink):\n",
            "    sink(((x + y) if flag else sink(x)) + y)\n",
        ))
        .unwrap();
        assert!(super::super::lower(&graph).is_err());
        let bodies = graph.registered_functions();
        let checked = &bodies[&graph.entry][&TextSize::default()];
        assert!(crate::llvm::registered::supports(checked));
        let mut branches = Vec::new();
        ir::walk_expressions(&checked.function.body, &mut |expr| {
            if let ExprKind::Conditional(test, yes, no) = &expr.kind {
                branches.push((test.ty, yes.ty, no.ty, expr.ty));
            }
        });
        assert_eq!(
            branches,
            [(
                Type::PythonObject,
                Type::Int,
                Type::PythonObject,
                Type::PythonObject
            )]
        );
    }

    #[test]
    fn callback_body_uses_registered_dictionary_global_and_subscript_with_checked_add() {
        let graph = SourceGraph::single(concat!(
            "KEY = 'target'\n",
            "def report(x: int, sink, y: int):\n",
            "    answer = x + y\n",
            "    callbacks = {KEY: sink}\n",
            "    callbacks[KEY](answer)\n",
        ))
        .unwrap();
        assert!(super::super::lower(&graph).is_err());
        let bodies = graph.registered_functions();
        let body = bodies[&graph.entry]
            .values()
            .next()
            .expect("registered callback");
        let mut operations = Vec::new();
        ir::walk_expressions(&body.function.body, &mut |value| match &value.kind {
            ExprKind::Dict(entries) => {
                assert!(matches!(
                    entries.as_slice(),
                    [ir::DictEntry { key: Some(_), .. }]
                ));
                operations.push("dict");
            }
            ExprKind::Builtin(Builtin::RegisteredName(name), _) if name == "KEY" => {
                operations.push("name")
            }
            ExprKind::Builtin(Builtin::RegisteredSubscript(..), _) => operations.push("get"),
            ExprKind::Invoke { callable, args } => {
                let ExprKind::Builtin(Builtin::RegisteredLocation(..), operands) = &callable.kind
                else {
                    panic!("expected the located callback");
                };
                assert!(matches!(
                    operands.as_slice(),
                    [Expr {
                        kind: ExprKind::Builtin(Builtin::RegisteredSubscript(..), _),
                        ..
                    }]
                ));
                assert_eq!(args.values.len(), 1);
                assert!(args.order.is_empty());
                operations.push("call");
            }
            ExprKind::Binary(Binary::Add, left, right) => {
                assert_eq!(
                    (left.ty, right.ty, value.ty),
                    (Type::Int, Type::Int, Type::Int)
                );
                operations.push("add");
            }
            _ => {}
        });
        operations.sort_unstable();
        assert_eq!(operations, ["add", "call", "dict", "get", "name", "name"]);
    }

    #[test]
    fn a_mixed_definition_keeps_its_original_list_write_diagnostic() {
        let graph = SourceGraph::single(concat!(
            "def publish(send):\n",
            "    previous: int = 0\n",
            "    previous = send(1)\n",
            "    values: list[int] = [3]\n",
            "    values.append(True)\n",
            "    print(locals()['previous'])\n",
            "def echo(value):\n    return 41\n",
            "entries = {'publish': publish}\n",
            "entries[input()]({'send': echo}[input()])\n",
        ))
        .unwrap();
        let diagnostic = super::super::lower(&graph).err().unwrap();
        assert!(diagnostic.contains(":5:19:"), "{diagnostic}");
        assert!(
            diagnostic.contains("expected int, got bool"),
            "{diagnostic}"
        );
    }

    #[test]
    fn declared_displays_in_mixed_functions_keep_their_element_diagnostics() {
        for (assignment, diagnostic) in [
            (
                "values: list[int] = [True]",
                "expected list[int], got list[bool]",
            ),
            (
                "values: tuple[str, int] = ('ok', True)",
                "expected tuple[str, int], got tuple[str, bool]",
            ),
        ] {
            let source = format!(
                "def publish(send):\n    previous: int = 0\n    previous = send(1)\n    {assignment}\n    print(locals()['previous'])\ndef echo(value):\n    return 41\nentries = {{'publish': publish}}\nentries[input()]({{'send': echo}}[input()])\n"
            );
            let graph = SourceGraph::single(&source).unwrap();
            let error = super::super::lower(&graph).err().unwrap();
            assert!(error.contains(diagnostic), "{error}");
        }
    }

    #[test]
    fn an_annotated_noncallable_local_keeps_its_checked_call_error() {
        let graph = SourceGraph::single("def bad(x: int, arg):\n    x(arg)\n").unwrap();
        assert!(super::super::lower(&graph).is_err());
        assert!(graph.registered_functions().is_empty());
    }

    #[test]
    fn registered_arithmetic_keeps_scalar_types_and_original_protocol_operands() {
        let source = concat!(
            "def deliver(a: int, b: int, flag: bool, send):\n",
            "    send(a * b)\n",
            "    send(a + flag)\n",
            "    send(a ** flag)\n",
        );
        let graph = SourceGraph::single(source).unwrap();
        assert!(super::super::lower(&graph).is_err());
        let functions = graph.registered_functions();
        let body = &functions[&graph.entry][&TextSize::default()].function.body;
        let mut operations = Vec::new();
        ir::walk_expressions(body, &mut |expr| {
            if let ExprKind::Binary(op, left, right) = &expr.kind {
                operations.push((op.protocol(), left.ty, right.ty, expr.ty));
            }
        });
        assert_eq!(
            operations,
            [
                ("*", Type::Int, Type::Int, Type::Int),
                ("+", Type::Int, Type::Bool, Type::Int),
                ("**", Type::Int, Type::Bool, Type::Int),
            ]
        );
    }

    #[test]
    fn solved_writer_formats_the_argument_and_loads_the_live_receiver() {
        let source = concat!(
            "from pathlib import Path\n",
            "target = Path('output')\n",
            "def notify(value: int, sink):\n    sink(value)\n",
            "def write(label: str) -> None:\n    target.write_text(f'<{label}:{label:>4}>')\n",
        );
        let graph = SourceGraph::single(source).unwrap();
        let _ = super::super::lower(&graph);
        let bodies = graph.registered_functions();
        let offset = TextSize::try_from(source.find("def write").unwrap()).unwrap();
        let body = &bodies[&graph.entry][&offset];
        assert!(matches!(body.inputs, FrameInputs::Protocol { .. }));
        let mut operations = Vec::new();
        ir::walk_expressions(&body.function.body, &mut |expr| match &expr.kind {
            ExprKind::Invoke { args, .. } if matches!(&args.site.python().unwrap().call.target, ir::RegisteredCallTarget::Method { name, .. } if name == "write_text") =>
            {
                operations.push("write_text");
            }
            ExprKind::Builtin(Builtin::RegisteredName(name), _) if name == "target" => {
                operations.push("target");
            }
            ExprKind::Builtin(Builtin::FormatField(_), args) => match args.as_slice() {
                [_] => operations.push("default format"),
                [_, spec] if matches!(&spec.kind, ExprKind::Str(s) if s == ">4") => {
                    operations.push("explicit format")
                }
                _ => panic!("unexpected format specification"),
            },
            _ => {}
        });
        assert_eq!(
            operations,
            ["write_text", "target", "default format", "explicit format"]
        );
    }

    #[test]
    fn runtime_string_format_retains_the_checked_sum_and_keyword_operand() {
        let source = concat!(
            "def render(template: str, left: int, right: int, label):\n",
            "    template.format(left + right, name=label)\n",
        );
        let graph = SourceGraph::single(source).unwrap();
        let _ = super::super::lower(&graph);
        let bodies = graph.registered_functions();
        let body = &bodies[&graph.entry][&TextSize::default()];
        assert!(crate::llvm::registered::supports(body));

        let mut adds = 0;
        let mut calls = 0;
        let mut methods = Vec::new();
        let mut slots = Vec::new();
        ir::walk_expressions(&body.function.body, &mut |value| match &value.kind {
            ExprKind::Binary(Binary::Add, left, right) => {
                assert_eq!(
                    (left.ty, right.ty, value.ty),
                    (Type::Int, Type::Int, Type::Int)
                );
                adds += 1;
            }
            ExprKind::Builtin(Builtin::RegisteredAttribute { name, .. }, receiver) => {
                assert_eq!(receiver[0].ty, Type::Str);
                methods.push(name.as_str());
            }
            ExprKind::Invoke { args, .. } => {
                let call = &args.site.python().unwrap().call;
                assert_eq!(
                    &source[call.range],
                    "template.format(left + right, name=label)"
                );
                assert_eq!(
                    call.arguments,
                    ir::RegisteredArguments::Keywords {
                        positional: 1,
                        names: vec!["name".into()],
                    }
                );
                assert_eq!(args.values.len(), 2);
                calls += 1;
            }
            ExprKind::Local(slot) => slots.push(*slot),
            _ => {}
        });
        assert_eq!((adds, calls), (1, 1));
        assert_eq!(methods, ["format"]);
        assert_eq!(slots, body.function.params);
    }

    #[test]
    fn checked_capture_requires_an_object_receiver_for_late_method_lookup() {
        for expression in [
            "n.unknown",
            "n.unknown()",
            "n.bit_length(1)",
            "n.to_bytes('x')",
            "n.bit_length()",
        ] {
            let source = format!("def act(n: int, sink):\n    {expression}\n    sink(n)\n");
            let graph = SourceGraph::single(&source).unwrap();
            assert!(super::super::lower(&graph).is_err());
            assert!(graph.registered_functions().is_empty(), "{expression}");
        }

        // A generic handle or an unknown type from an invalid expression is insufficient.
        for body in [
            "    {'key': n}.unknown()",
            "    {'key': n}.get('key', n, n)",
            "    alias = {'key': n}\n    alias.unknown()",
            "    alias = {'key': n}\n    alias.get('key', n, n)",
            "    n[0].unknown()",
            "    undefined_name.unknown()",
            "    sink = n[0]\n    sink.unknown()",
            "    alias = sink\n    alias.unknown()",
        ] {
            let source = format!("def act(n: int, sink):\n{body}\n    sink(n)\n");
            let graph = SourceGraph::single(&source).unwrap();
            assert!(super::super::lower(&graph).is_err());
            assert!(graph.registered_functions().is_empty(), "{body}");
        }

        let graph =
            SourceGraph::single("def act(obj, sink):\n    obj.send(1)\n    sink(obj)\n").unwrap();
        assert!(super::super::lower(&graph).is_err());
        let bodies = graph.registered_functions();
        let body = &bodies[&graph.entry][&TextSize::default()];
        let mut named = Vec::new();
        ir::walk_expressions(&body.function.body, &mut |expr| {
            if let Some(ir::RegisteredCall {
                target: ir::RegisteredCallTarget::Method { name, .. },
                ..
            }) = expr.python_call()
            {
                named.push(name.as_str());
            }
        });
        assert_eq!(named, ["send"]);
    }

    fn checked_positive_with_invalid_siblings(positive: &str, invalid: &str) -> SourceGraph {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("callback.py");
        let combined = format!("{positive}{invalid}");
        std::fs::write(&path, combined).unwrap();
        assert!(crate::check_file(&path, None).is_err());
        std::fs::write(&path, positive).unwrap();
        crate::check_file(&path, None).unwrap();
        let mut graph = SourceGraph::single(positive).unwrap();
        let selected = super::super::lower_with_caller_cache(&mut graph, None);
        match selected {
            Ok(crate::CheckedProgram::Registered(None)) => {}
            Err(error) | Ok(crate::CheckedProgram::Runtime(error)) => {
                assert!(
                    matches!(error.kind, Kind::UnresolvedLayout(owner, start)
                    if owner == graph.entry && start == TextSize::default()),
                    "{error}"
                );
            }
            _ => panic!("expected a registered Python callable"),
        }
        graph
    }

    #[test]
    fn written_assignments_accept_unary_operations_on_numeric_literals() {
        let text = "x = 1 - ---1 * 1\ny = -+2.0\n";
        let graph = SourceGraph::single(text).unwrap();
        let module = &graph.modules[graph.entry];
        let suite = registered_suite(
            &module.parsed.syntax().body,
            RegisteredScope::Function { is_async: false },
            module.registered_values().context(text),
        )
        .expect("the written assignments should lower in their own frame");
        crate::verify::registered(suite.region()).unwrap();
    }

    #[test]
    fn unary_protocol_requires_a_materialized_operand_in_a_checked_frame() {
        let positive =
            "def run(operand, sink):\n    sink(+operand, -operand, ~operand, not operand)\n";
        let invalid = concat!(
            "def call_result(make, sink):\n",
            "    sink(-make())\n",
            "def dormant(text: str, sink):\n",
            "    sink(-text)\n",
            "def literal(n: int, sink):\n",
            "    sink(-{'x': n})\n",
            "def alias(n: int, sink):\n",
            "    data = {'x': n}\n",
            "    sink(-data)\n",
        );
        let graph = checked_positive_with_invalid_siblings(positive, invalid);
        let bodies = graph.registered_functions();
        let functions = &bodies[&graph.entry];
        assert_eq!(functions.len(), 1);
        let mut ops = Vec::new();
        ir::walk_expressions(&functions[&TextSize::default()].function.body, &mut |e| {
            if let ExprKind::Unary(op, value) = &e.kind {
                assert_eq!(value.ty, Type::PythonObject);
                ops.push(*op);
            }
        });
        use ast::UnaryOp::{Invert, Not, UAdd, USub};
        assert_eq!(ops, [UAdd, USub, Invert, Not]);
    }

    #[test]
    fn a_registered_function_keeps_its_indexed_comparisons() {
        let source = concat!(
            "def improve(best, neighbor, candidate, accept):\n",
            "    if best[neighbor] == -1 or candidate < best[neighbor]:\n",
            "        accept(candidate)\n",
        );
        let mut graph = SourceGraph::single(source).unwrap();
        let _ = super::super::lower_with_caller_cache(&mut graph, None);
        let registered = graph.registered_functions();
        let checked = &registered[&graph.entry][&TextSize::default()].function;
        let mut comparisons = Vec::new();
        ir::walk_expressions(&checked.body, &mut |expr| {
            if let ExprKind::Compare(_, ops) = &expr.kind {
                comparisons.extend(ops.iter().copied());
            }
        });
        assert_eq!(comparisons, [ast::CmpOp::Eq, ast::CmpOp::Lt]);
    }

    #[test]
    fn partial_regions_preserve_specialized_and_unsupported_operands() {
        for source in [
            "s.partition((sep for sep in separators))[1]",
            "f(*(x for x in items))",
            "f(k=lambda: value)",
            "{'key': recv.method(*(x for x in args))}",
        ] {
            let graph = SourceGraph::single(source).unwrap();
            let ast::Stmt::Expr(statement) = &graph.modules[graph.entry].parsed.syntax().body[0]
            else {
                panic!("expected an expression")
            };
            assert!(
                values::registered_value(
                    &statement.value,
                    values::ModuleSource::new(&graph.modules[graph.entry]).context(source)
                )
                .is_none(),
                "{source}"
            );
        }
    }

    #[test]
    fn grouped_deletion_interleaves_names_and_attributes_in_source_order() {
        let text = "del (first, [obj.field, last]), obj.tail\n";
        let graph = SourceGraph::single(text).unwrap();
        let module = &graph.modules[graph.entry];
        let source = values::ModuleSource::new(module);
        let suite = registered_suite(
            &module.parsed.syntax().body,
            RegisteredScope::Module,
            source.context(text),
        )
        .unwrap();
        let sites: Vec<_> = suite
            .function
            .body
            .iter()
            .filter_map(|statement| {
                let Statement::Expr(Expr {
                    kind: ExprKind::Builtin(op, _),
                    ..
                }) = statement
                else {
                    return None;
                };
                match op {
                    Builtin::RegisteredDeleteName(_, range)
                    | Builtin::RegisteredAttribute {
                        action: ir::AttributeAction::Delete,
                        range,
                        ..
                    } => Some(&text[*range]),
                    _ => None,
                }
            })
            .collect();
        assert_eq!(sites, ["first", "obj.field", "last", "obj.tail"]);
    }

    #[test]
    fn chained_targets_share_one_rhs_and_transfer_it_to_the_last_store() {
        let text = "response.code = values[key()] = status()\n";
        let graph = SourceGraph::single(text).unwrap();
        let module = &graph.modules[graph.entry];
        let literals = values::ModuleSource::new(module);
        let checked = registered_suite(
            &module.parsed.syntax().body,
            RegisteredScope::Module,
            literals.context(text),
        )
        .unwrap();
        let [_, Statement::Scope { body, clear }] = checked.function.body.as_slice() else {
            panic!("expected a checked assignment chain")
        };
        let [
            Statement::Assign(slot, _),
            Statement::Expr(store),
            Statement::SetItem(_, _, _, last),
        ] = body.as_slice()
        else {
            panic!("expected one RHS followed by the two target stores")
        };
        assert_eq!(clear, &[*slot]);
        assert!(matches!(&store.kind, ExprKind::Builtin(
            Builtin::RegisteredAttribute { action: ir::AttributeAction::Store, .. }, inputs
        ) if matches!(inputs.first().map(|value| &value.kind), Some(ExprKind::Local(id)) if id == slot)));
        assert!(matches!(&last.kind, ExprKind::Scope { value, clear, .. }
            if clear == &[*slot] && matches!(&value.kind, ExprKind::Local(id) if id == slot)));
    }

    #[test]
    fn subgroup_and_context_target_share_the_prepared_body() {
        let graph = SourceGraph::single(concat!(
            "try:\n",
            "    with resource as (left, right):\n",
            "        send(left, right)\n",
            "except* select() as group:\n",
            "    send(group)\n",
        ))
        .unwrap();
        let module = &graph.modules[graph.entry];
        let checked = registered_suite(
            &module.parsed.syntax().body,
            RegisteredScope::Function { is_async: false },
            values::ModuleSource::new(module).context(&module.source),
        )
        .expect("complete protected body");
        let mut targets = 0;
        let mut filters = 0;
        ir::walk_statements(&checked.function.body, &mut |stmt| match stmt {
            Statement::With {
                binding: Some(binding),
                ..
            } => {
                assert!(
                    matches!(binding.body.as_slice(), [Statement::Scope { clear, .. }] if clear == &[binding.entered])
                );
                targets += 1;
            }
            Statement::Try { handlers, .. } => {
                assert!(
                    matches!(handlers.as_slice(), [handler] if matches!(handler.matcher, ir::ExceptionMatcher::Subgroup(..)))
                );
                filters += 1;
            }
            _ => {}
        });
        assert_eq!((targets, filters), (1, 1));
    }

    #[test]
    fn loop_exit_through_finally_keeps_a_prepared_region() {
        let source =
            "for item in items:\n    try:\n        break\n    finally:\n        emit(item)\n";
        let graph = SourceGraph::single(source).unwrap();
        let module = &graph.modules[graph.entry];
        let checked = registered_suite(
            &module.parsed.syntax().body,
            RegisteredScope::Function { is_async: false },
            values::ModuleSource::new(module).context(&module.source),
        )
        .expect("complete registered loop");
        let mut found = false;
        ir::walk_statements(&checked.function.body, &mut |stmt| {
            if let Statement::Try {
                body,
                handlers,
                finalbody,
                ..
            } = stmt
            {
                assert!(handlers.is_empty() && !finalbody.is_empty());
                assert!(matches!(body.last(), Some(Statement::Break(_))));
                found = true;
            }
        });
        assert!(found);
    }

    #[test]
    fn named_handler_keeps_its_live_filter_and_original_binding() {
        let text = "try:\n    run()\nexcept choose() as fault:\n    handle(fault)\n";
        let graph = SourceGraph::single(text).unwrap();
        let module = &graph.modules[graph.entry];
        let checked = registered_suite(
            &module.parsed.syntax().body,
            RegisteredScope::Function { is_async: false },
            values::ModuleSource::new(module).context(&module.source),
        )
        .expect("prepared live match");
        let [_, Statement::Try { handlers, .. }] = checked.function.body.as_slice() else {
            panic!("expected registered protected region");
        };
        let ir::ExceptionMatcher::Object(range, _) = &handlers[0].matcher else {
            panic!("expected a Python object filter");
        };
        assert_eq!(&text[*range], "choose()");
        assert_eq!(
            checked.local_names[&handlers[0].name.unwrap()].name,
            "fault"
        );
        assert!(crate::llvm::registered::supports(&checked));
    }

    #[test]
    fn checked_function_retains_the_live_exception_matcher() {
        let text = concat!(
            "def guarded() -> None:\n",
            "    try:\n        print(1)\n",
            "    except TypeError as caught:\n        print(caught)\n",
            "def publish(send):\n    send(1)\n",
        );
        let graph = SourceGraph::single(text).unwrap();
        assert!(super::super::lower(&graph).is_err());
        let functions = graph.registered_functions();
        let guarded = &functions[&graph.entry][&TextSize::default()];
        let handler = guarded
            .function
            .body
            .iter()
            .find_map(|statement| match statement {
                Statement::Try { handlers, .. } => handlers.first(),
                _ => None,
            })
            .unwrap();
        let ir::ExceptionMatcher::Object(range, class) = &handler.matcher else {
            panic!("expected a live handler class");
        };
        assert_eq!(&text[*range], "TypeError");
        assert!(matches!(&class.without_location().kind,
            ExprKind::Builtin(Builtin::RegisteredName(name), _) if name == "TypeError"));
        assert_eq!(guarded.local_names[&handler.name.unwrap()].name, "caught");
    }

    #[test]
    fn checked_managers_retain_their_source_sites() {
        let text = "async def capture():\n    async with manager:\n        pass\nwith λ[0], install(2):\n    pass\n";
        let graph = SourceGraph::single(text).unwrap();
        let module = &graph.modules[graph.entry];
        let (definition, rest) = module.parsed.syntax().body.split_first().unwrap();
        let capture = definition.as_function_def_stmt().unwrap();
        let source = values::ModuleSource::new(module);
        let mut anchors = Vec::new();
        for (body, scope) in [
            (rest, RegisteredScope::Module),
            (
                capture.body.as_slice(),
                RegisteredScope::Function { is_async: true },
            ),
        ] {
            let checked = registered_suite(body, scope, source.context(text)).unwrap();
            ir::walk_statements(&checked.function.body, &mut |stmt| {
                if let Statement::With { site, .. } = stmt {
                    anchors.push((
                        &text[site.range],
                        &text[site.range.start().to_usize()..site.anchor.to_usize()],
                        site.is_async,
                    ));
                }
            });
        }
        anchors.sort_unstable();
        assert_eq!(
            anchors,
            [
                ("install(2)", "install", false),
                ("manager", "", true),
                ("λ[0]", "λ", false)
            ]
        );
    }

    #[test]
    fn bare_handler_has_checked_body_and_distinct_else() {
        let graph =
            SourceGraph::single("try:\n    run()\nexcept:\n    failed()\nelse:\n    clean()\n")
                .unwrap();
        let module = &graph.modules[graph.entry];
        let checked = registered_suite(
            &module.parsed.syntax().body,
            RegisteredScope::Function { is_async: false },
            values::ModuleSource::new(module).context(&module.source),
        )
        .expect("complete registered try");
        let [
            _,
            Statement::Try {
                body,
                handlers,
                orelse,
                finalbody,
            },
        ] = checked.function.body.as_slice()
        else {
            panic!("expected one prepared protected region");
        };
        assert_eq!(
            (body.len(), handlers[0].body.len(), orelse.len()),
            (2, 2, 2)
        );
        assert!(finalbody.is_empty() && crate::llvm::registered::supports(&checked));
    }

    #[test]
    fn local_annotation_uses_only_the_assigned_value() {
        let graph = SourceGraph::single("header: missing() = ()\nother: missing()\n").unwrap();
        let entry = &graph.modules[graph.entry];
        let checked = registered_suite(
            &entry.parsed.syntax().body,
            RegisteredScope::Function { is_async: false },
            values::ModuleSource::new(entry).context(&entry.source),
        )
        .expect("function-local annotations need no execution-time provider");
        let mut calls = 0;
        ir::walk_expressions(&checked.function.body, &mut |value| {
            calls += usize::from(matches!(value.kind, ExprKind::Invoke { .. }));
        });
        assert_eq!(calls, 0);
    }

    #[test]
    fn prepared_namespace_assignments_and_loops_stage_the_same_targets() {
        #[derive(Clone, Copy, Debug, PartialEq)]
        enum Stage {
            PullPair,
            Unpack(usize, Option<usize>),
            SetItem,
            SetAttribute,
        }
        for (source, outer) in [
            (
                "owners[0], (sink.field, *rest) = row\n",
                Stage::Unpack(2, None),
            ),
            (
                "for owners[0], (sink.field, *rest) in rows:\n    publish(rest)\n",
                Stage::PullPair,
            ),
        ] {
            let graph = SourceGraph::single(source).unwrap();
            let suite = &graph.modules[graph.entry].parsed.syntax().body;
            for module in [false, true] {
                let body = registered_suite(
                    suite,
                    if module {
                        RegisteredScope::Module
                    } else {
                        RegisteredScope::Function { is_async: false }
                    },
                    values::ModuleSource::new(&graph.modules[graph.entry])
                        .context(&graph.modules[graph.entry].source),
                )
                .unwrap();
                let mut stages = Vec::new();
                ir::walk_nodes(&body.function.body, &mut |node, _| {
                    stages.push(match node {
                        ir::Node::Statement(Statement::IterNext {
                            kind: ir::IterNextKind::Pair(target),
                            value: Some(_),
                            ..
                        }) => (Stage::PullPair, &source[*target]),
                        ir::Node::Statement(Statement::SetItem(..)) => (Stage::SetItem, ""),
                        ir::Node::Expression(Expr {
                            kind: ExprKind::Builtin(op, _),
                            ..
                        }) => match op {
                            Builtin::RegisteredUnpack {
                                count,
                                star,
                                target: Some(target),
                            } => (Stage::Unpack(*count, *star), &source[*target]),
                            Builtin::RegisteredAttribute {
                                action: ir::AttributeAction::Store,
                                range,
                                ..
                            } => (Stage::SetAttribute, &source[*range]),
                            _ => return,
                        },
                        _ => return,
                    });
                });
                assert_eq!(
                    stages,
                    [
                        (outer, "owners[0], (sink.field, *rest)"),
                        (Stage::SetItem, ""),
                        (Stage::Unpack(2, Some(1)), "(sink.field, *rest)"),
                        (Stage::SetAttribute, "sink.field"),
                    ]
                );
            }
        }
    }

    #[test]
    fn traceback_while_suspends_without_a_for_cursor() {
        fn prepare(text: &str, name: &str) -> Option<RegisteredFunction> {
            let module = ruff_python_parser::parse_module(text).unwrap();
            let body = &module.syntax().body;
            let function = body
                .iter()
                .filter_map(ast::Stmt::as_function_def_stmt)
                .find(|def| def.name.as_str() == name)
                .unwrap();
            let source = RegisteredSource {
                python_optimization: 0,
                operand_types: None,
                template_fields: None,
                generators: None,
                lambdas: None,
                text,
                private_names: None,
                method_imports: &imre_ast::module_imports(body),
                replacements: &HashMap::new(),
            };
            registered_suite(
                &function.body,
                RegisteredScope::Function { is_async: false },
                source,
            )
        }

        let checked = prepare(include_str!("../../stdlib/traceback.py"), "walk_tb").unwrap();
        let mut yields_per_loop = Vec::new();
        ir::walk_statements(&checked.function.body, &mut |stmt| {
            if let Statement::While(_, body, _) = stmt {
                let mut yields = 0;
                ir::walk_expressions(body, &mut |value| {
                    if matches!(
                        value.kind,
                        ExprKind::Builtin(
                            Builtin::RegisteredSuspend(ir::SuspendValue::Yield, _),
                            _
                        )
                    ) {
                        yields += 1;
                    }
                });
                yields_per_loop.push(yields);
            }
        });
        assert_eq!(yields_per_loop, [1]);
        let nested = "def walk(rows, test):\n    for row in rows:\n        while test(row):\n            yield row\n";
        assert!(prepare(nested, "walk").is_some());
    }

    #[test]
    fn synchronous_suspensions_retain_the_checked_loop_cursor() {
        let source = concat!(
            "def sales(rows):\n",
            "    for row in rows:\n",
            "        received = yield row\n",
            "        yield from received\n",
        );
        assert_cursor_suspensions(source, &["yield row", "yield from received"]);
    }

    #[test]
    fn generic_function_default_suspends_in_the_enclosing_loop() {
        let source = concat!(
            "def f(rows):\n",
            "    for x, y in rows:\n",
            "        def hold[T](value=(yield 1)):\n",
            "            return value\n",
        );
        assert_cursor_suspensions(source, &["yield 1"]);
    }

    #[test]
    fn awaited_calls_retain_the_checked_loop_cursor() {
        let source = concat!(
            "async def visit(rows, call, results):\n",
            "    for row in rows:\n",
            "        await call(row)\n",
            "        results.append(await row.value())\n",
        );
        assert_cursor_suspensions(source, &["await call(row)", "await row.value()"]);
    }

    #[test]
    fn for_iterable_can_suspend_before_acquiring_the_checked_cursor() {
        let source = concat!(
            "async def visit(make, left, right, record):\n",
            "    for key, value in make(left(), await right(), strict=False):\n",
            "        record(key, value)\n",
            "    else:\n",
            "        record('done')\n",
        );
        assert_cursor_suspensions(source, &["await right()"]);
    }

    #[test]
    fn awaited_subscription_binds_its_result_in_the_checked_loop() {
        let source = concat!(
            "async def visit(rows, key):\n",
            "    for result in rows:\n",
            "        left, right = (await result)[await key()]\n",
        );
        assert_cursor_suspensions(source, &["await result", "await key()"]);
    }

    #[test]
    fn nested_awaits_use_the_current_loop_cursor() {
        let source = concat!(
            "async def visit(rows, key, call):\n",
            "    async for row in rows:\n",
            "        args = row if key is None else await key(row)\n",
            "        await (await call(*args))\n",
        );
        assert_cursor_suspensions(
            source,
            &[
                "await key(row)",
                "await (await call(*args))",
                "await call(*args)",
            ],
        );
    }

    fn assert_cursor_suspensions(source: &str, expected: &[&str]) {
        let graph = SourceGraph::single(source).unwrap();
        let module = &graph.modules[graph.entry];
        let definition = module.parsed.syntax().body[0]
            .as_function_def_stmt()
            .unwrap();
        let checked = registered_suite(
            &definition.body,
            RegisteredScope::Function {
                is_async: definition.is_async,
            },
            values::ModuleSource::new(module).context(&module.source),
        )
        .expect("complete suspended body");
        let mut cursors = std::collections::BTreeSet::new();
        let mut suspensions = Vec::new();
        ir::walk_statements(&checked.function.body, &mut |stmt| {
            if let Statement::IterNext { iterator, .. } = stmt {
                cursors.insert(*iterator);
            }
        });
        ir::walk_expressions(&checked.function.body, &mut |expr| {
            if let ExprKind::Builtin(Builtin::RegisteredSuspend(kind, range), _) = &expr.kind {
                assert_eq!(matches!(kind, ir::SuspendValue::Await), definition.is_async);
                suspensions.push(&source[*range]);
            }
        });
        assert_eq!(cursors.len(), 1);
        assert!(
            checked
                .region()
                .private_slots()
                .contains(cursors.first().unwrap())
        );
        assert_eq!(suspensions, expected);
    }

    #[test]
    fn ordinary_and_iteration_targets_share_unpack_and_store_order() {
        for source in [
            "def post(row, book, read):\n    book[0], book[read()] = row\n",
            "def post(rows, book, read):\n    for book[0], book[read()] in rows:\n        pass\n",
        ] {
            let graph = SourceGraph::single(source).unwrap();
            let _ = super::super::lower(&graph);
            let registered = graph.registered_functions();
            let function = &registered[&graph.entry][&TextSize::default()].function;
            let mut operations = Vec::new();
            ir::walk_nodes(&function.body, &mut |node, _| match node {
                ir::Node::Expression(Expr {
                    kind:
                        ExprKind::Builtin(
                            Builtin::RegisteredUnpack {
                                count: 2,
                                star: None,
                                ..
                            },
                            _,
                        ),
                    ..
                })
                | ir::Node::Statement(Statement::IterNext {
                    kind: ir::IterNextKind::Pair(_),
                    ..
                }) => operations.push("unpack"),
                ir::Node::Statement(Statement::SetItem(..)) => operations.push("store"),
                _ => {}
            });
            assert_eq!(operations, ["unpack", "store", "store"], "{source}");
        }
    }

    #[test]
    fn prepared_namespace_suite_uses_the_same_pull_and_exhaustion() {
        let graph = SourceGraph::single("for entry in rows:\n    emit(entry)\nelse:\n    done()\n")
            .unwrap();
        let source = &graph.modules[graph.entry].parsed.syntax().body;
        for module in [true, false] {
            let suite = registered_suite(
                source,
                if module {
                    RegisteredScope::Module
                } else {
                    RegisteredScope::Function { is_async: false }
                },
                values::ModuleSource::new(&graph.modules[graph.entry])
                    .context(&graph.modules[graph.entry].source),
            )
            .unwrap();
            let mut pulls = 0;
            let mut exits = 0;
            ir::walk_statements(&suite.function.body, &mut |stmt| match stmt {
                Statement::IterNext { .. } => pulls += 1,
                Statement::Exhaust => exits += 1,
                _ => {}
            });
            assert_eq!((pulls, exits), (1, 1));
            let entry = suite
                .local_names
                .values()
                .find(|v| v.name == "entry")
                .unwrap();
            assert!(entry.namespace.is_none());
        }
    }

    #[test]
    fn registered_cursor_keeps_the_checked_loop_target() {
        let graph = SourceGraph::single(concat!(
            "def deliver(items: list[int], emit):\n",
            "    for item in items:\n",
            "        emit(item)\n",
        ))
        .unwrap();
        let _ = super::super::lower(&graph);
        let bodies = graph.registered_functions();
        let body = &bodies[&graph.entry][&TextSize::default()];
        crate::verify::registered(body.region()).unwrap();
        let (&item, _) = body
            .local_names
            .iter()
            .find(|(_, name)| name.name == "item")
            .unwrap();
        assert_eq!(body.function.locals[item], Type::Int);
        let mut keys = Vec::new();
        ir::walk_statements(&body.function.body, &mut |statement| {
            if let Statement::IterNext {
                kind: ir::IterNextKind::Generic,
                key,
                ..
            } = statement
            {
                keys.push(body.function.locals[*key]);
            }
        });
        assert_eq!(keys, [Type::PythonObject]);
    }

    #[test]
    fn registered_for_uses_the_prepared_iterator_and_its_exhaustion_edge() {
        let graph = SourceGraph::single(concat!(
            "def deliver(items, emit, exhausted):\n",
            "    for item in items:\n",
            "        emit(item)\n",
            "    else:\n",
            "        exhausted()\n",
        ))
        .unwrap();
        let _ = super::super::lower(&graph);
        let bodies = graph.registered_functions();
        let function = &bodies[&graph.entry][&TextSize::default()].function;
        let mut opens = 0;
        let mut calls = 0;
        ir::walk_expressions(&function.body, &mut |expr| match &expr.kind {
            ExprKind::Builtin(Builtin::RegisteredIter(ir::IteratorOp::Loop(_)), _) => opens += 1,
            ExprKind::Invoke { .. } => calls += 1,
            _ => {}
        });
        let mut pulls = 0;
        let mut exhausts = 0;
        ir::walk_statements(&function.body, &mut |stmt| match stmt {
            Statement::IterNext { iterator, key, .. } => {
                assert_eq!(function.locals[*iterator], Type::PythonObject);
                assert_eq!(function.locals[*key], Type::PythonObject);
                pulls += 1;
            }
            Statement::Exhaust => exhausts += 1,
            _ => {}
        });
        assert_eq!((opens, pulls, exhausts, calls), (1, 1, 1, 2));
    }

    #[test]
    fn retained_pair_target_uses_one_pull_at_its_written_target() {
        let text = concat!(
            "def consume() -> None:\n",
            "    for key, value in available_rows:\n",
            "        print(key, value)\n",
            "available_rows = {'left': 1, 'right': 2}.items()\n",
            "consume()\n",
        );
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app.py");
        std::fs::write(&path, text).unwrap();
        let mut graph = SourceGraph::load(&path, None, &[]).unwrap();
        graph.registered_startup = true;
        crate::checked_lower(&mut graph).unwrap();
        let bodies = graph.registered_functions();
        let function = &bodies[&graph.entry][&TextSize::default()].function;
        let mut acquired = Vec::new();
        let mut pulls = Vec::new();
        ir::walk_statements(&function.body, &mut |stmt| match stmt {
            Statement::Assign(slot, value)
                if matches!(
                    value.kind,
                    ExprKind::Builtin(Builtin::RegisteredIter(ir::IteratorOp::Loop(_)), _)
                ) =>
            {
                acquired.push(*slot);
            }
            Statement::IterNext {
                iterator,
                kind,
                value,
                ..
            } => {
                let ir::IterNextKind::Pair(range) = *kind else {
                    panic!("expected a single pair pull")
                };
                assert_eq!(&text[range], "key, value");
                pulls.push((*iterator, value.is_some()));
            }
            _ => {}
        });
        let [cursor] = acquired[..] else {
            panic!("expected one acquired cursor")
        };
        assert_eq!(pulls, [(cursor, true)]);
    }

    #[test]
    fn eager_displays_use_the_same_pull_and_capture_the_lexical_namespace() {
        let graph = SourceGraph::single(concat!(
            "def collect(rows, project, accept, sink):\n",
            "    sink([project(rows) for rows in rows if accept(rows)])\n",
            "    sink({project(x) for x in rows})\n",
            "    sink({project(x): project(x) for x in rows})\n",
            "    sink([(locals if observed else globals)() for observed in rows])\n",
        ))
        .unwrap();
        let _ = super::super::lower(&graph);
        let bodies = graph.registered_functions();
        let checked = &bodies[&graph.entry][&TextSize::default()];
        let mut namespaces = 0;
        let mut scoped_calls = 0;
        let mut observers = 0;
        let mut pulls = 0;
        ir::walk_expressions(&checked.function.body, &mut |expr| match &expr.kind {
            ExprKind::Builtin(Builtin::RegisteredNamespace(_), _) => namespaces += 1,
            ExprKind::Invoke { args, .. }
                if let Some(site) = args.site.python()
                    && let Some(scope) = &site.scope =>
            {
                let ExprKind::Local(namespace) = scope.kind else {
                    panic!("expected the comprehension's namespace slot")
                };
                scoped_calls += 1;
                if &graph.modules[graph.entry].source[site.call.range]
                    == "(locals if observed else globals)()"
                {
                    assert!(checked.local_names.values().any(|binding| {
                        binding.name == "observed" && binding.namespace == Some(namespace)
                    }));
                    observers += 1;
                }
            }
            _ => {}
        });
        ir::walk_statements(&checked.function.body, &mut |stmt| {
            pulls += usize::from(matches!(stmt, Statement::IterNext { .. }));
        });
        assert_eq!((namespaces, scoped_calls, observers, pulls), (4, 6, 1, 4));
        assert_eq!(
            checked
                .local_names
                .values()
                .filter(|binding| binding.namespace.is_some())
                .count(),
            4
        );
    }

    #[test]
    fn a_packed_callback_retains_the_checked_body_contract() {
        let positive = "def forward(call, args, kwargs):\n    call(*args, **kwargs)\n";
        let invalid = concat!(
            "def bad(n: int, call, kwargs):\n",
            "    n[0]\n",
            "    call(**kwargs)\n",
            "def noncallable(n: int, kwargs):\n",
            "    n(**kwargs)\n",
        );
        let graph = checked_positive_with_invalid_siblings(positive, invalid);
        let bodies = graph.registered_functions();
        let functions = &bodies[&graph.entry];
        assert_eq!(functions.len(), 1);
        let mut calls = Vec::new();
        ir::walk_expressions(
            &functions[&TextSize::default()].function.body,
            &mut |expr| {
                if let ExprKind::Invoke { args, .. } = &expr.kind {
                    calls.push(&args.site.python().unwrap().call.arguments);
                }
            },
        );
        assert_eq!(
            calls,
            [&ir::RegisteredArguments::Packed(ir::PackedArguments {
                starred: vec![true],
                keywords: vec![None],
            })]
        );
    }

    #[test]
    fn checked_async_iteration_owns_its_resumable_pull() {
        for (source, owner, operand) in [
            (
                "async def collect(feed):\n    return [item async for item in feed]\n",
                0,
                "item",
            ),
            (
                "async def consume(feed):\n    async for item in feed:\n        pass\n",
                0,
                "feed",
            ),
            (
                concat!(
                    "async def rows():\n    yield 1\n",
                    "async def collect():\n    return [item async for item in rows() if item]\n",
                ),
                1,
                "item",
            ),
        ] {
            let mut graph = SourceGraph::single(source).unwrap();
            crate::checked_lower(&mut graph).unwrap();
            let start = graph.modules[graph.entry].parsed.syntax().body[owner].start();
            let registered = graph.registered_functions();
            let body = &registered[&graph.entry][&start];
            crate::verify::registered(body.region()).unwrap();
            let mut pulls = Vec::new();
            ir::walk_statements(&body.function.body, &mut |stmt| {
                if let Statement::IterNext {
                    kind: ir::IterNextKind::Async(pull),
                    ..
                } = stmt
                {
                    pulls.push(pull.range);
                }
            });
            assert_eq!(pulls.len(), 1);
            assert_eq!(&source[pulls[0]], operand);
        }
    }

    #[test]
    fn an_uncalled_coroutine_checks_exact_operands_across_its_async_display() {
        let dir = tempfile::tempdir().unwrap();
        let app = dir.path().join("app.py");
        for body in [
            "selected = [i async for i in rows()]\n    score(True)\n    return selected",
            "return [score(True) async for i in rows()]",
            "return [i async for i in rows() if False if score(True)]",
            "return [i async for i in rows()]\n    score(True)",
        ] {
            let source = format!(
                "def score(n: int) -> int:\n    return n\nasync def rows():\n    yield 1\nasync def collect():\n    {body}\n"
            );
            std::fs::write(&app, source).unwrap();
            let diagnostic = crate::check_file(&app, None).unwrap_err();
            assert!(
                diagnostic.contains("expected int, got bool"),
                "{diagnostic}"
            );
        }
    }

    #[test]
    fn a_returning_async_display_keeps_an_unreachable_child_refusal() {
        let dir = tempfile::tempdir().unwrap();
        let app = dir.path().join("app.py");
        std::fs::write(
            &app,
            concat!(
                "async def rows():\n    yield 1\n",
                "async def collect():\n    return [i async for i in rows()]\n",
                "    f = lambda row: -(not row)\nprint('uncalled')\n",
            ),
        )
        .unwrap();
        let error = crate::check_file(&app, None).unwrap_err();
        assert!(
            error.contains("cannot compile this lambda body ahead of time"),
            "{error}"
        );
    }

    #[test]
    fn a_registered_loop_binds_its_opaque_row_to_the_existing_inferred_local() {
        let graph = SourceGraph::single(concat!(
            "def consume() -> None:\n",
            "    offset = 7\n",
            "    for offset in (value for value in [1, 2, 3]):\n",
            "        print('entered', offset)\n",
            "    print('last', offset)\n",
            "def publish(sink):\n    sink(1)\n",
        ))
        .unwrap();
        assert!(super::super::lower(&graph).is_err());
        let functions = graph.registered_functions();
        let body = &functions[&graph.entry][&TextSize::default()];
        assert!(body.local_names.iter().any(|(&slot, binding)| {
            binding.name == "offset"
                && binding.namespace.is_none()
                && body.function.locals[slot] == Type::PythonObject
        }));
    }

    #[test]
    fn registered_function_retains_the_checked_nullable_local() {
        let graph = SourceGraph::single(concat!(
            "def consume() -> None:\n",
            "    value: int | None = 1\n",
            "    print(value)\n",
            "    value = None\n",
            "    print(value)\n",
            "def publish(sink):\n    sink(1)\n",
        ))
        .unwrap();
        assert!(super::super::lower(&graph).is_err());
        let functions = graph.registered_functions();
        let body = &functions[&graph.entry][&TextSize::default()];
        let mut present = false;
        let mut absent = false;
        ir::walk_expressions(&body.function.body, &mut |expr| match &expr.kind {
            ExprKind::Present(_) => present = true,
            ExprKind::Absent(_) => absent = true,
            _ => {}
        });
        assert!(present && absent);
    }

    #[test]
    fn set_comprehensions_keep_their_checked_key_representation() {
        let graph = SourceGraph::single(concat!(
            "def collect(items: list[int], load):\n",
            "    return {x for x in items}, {x for x in load()}\n",
        ))
        .unwrap();
        let _ = super::super::lower(&graph);
        let functions = graph.registered_functions();
        let body = &functions[&graph.entry][&TextSize::default()];
        crate::verify::registered(body.region()).unwrap();
        let mut sets = Vec::new();
        ir::walk_expressions(&body.function.body, &mut |expr| {
            if let ExprKind::Set { element, .. } = &expr.kind {
                sets.push((expr.ty, *element));
            }
        });
        assert!(matches!(
            sets.as_slice(),
            [
                (Type::Set(_), Type::Int),
                (Type::PythonObject, Type::PythonObject)
            ]
        ));
    }

    #[test]
    fn registered_iteration_preserves_known_operand_contracts() {
        for source in [
            "def post(batches: tuple[int, int], sink):\n    for batch in batches:\n        for item in batch:\n            sink(item)\n",
            "def f(sink):\n    for value in 42:\n        sink(value)\n",
            "def f(sink):\n    sink([x for x in None])\n",
            "def f(rows, key: list[int], sink):\n    sink({key: x for x in rows})\n",
        ] {
            let graph = SourceGraph::single(source).unwrap();
            assert!(super::super::lower(&graph).is_err());
            assert!(graph.registered_functions().is_empty(), "{source}");
        }
    }

    #[test]
    fn fixed_layout_folds_cannot_erase_object_operations() {
        // A runtime int subclass can supply a distinct implementation for each operation.
        for operation in ["x += y", "x = +x", "x = int(x)", "x = x.real"] {
            let program =
                format!("def report(x: int, sink, y: int):\n    {operation}\n    sink(x)\n");
            let graph = SourceGraph::single(&program).unwrap();
            assert!(super::super::lower(&graph).is_err());
            assert!(graph.registered_functions().is_empty(), "{operation}");
        }
    }

    #[test]
    fn an_imported_initializer_keeps_its_priority_over_a_registered_body_error() {
        let dir = tempfile::tempdir().unwrap();
        let app = dir.path().join("app.py");
        std::fs::write(&app, "import operations\n").unwrap();
        let registered =
            "def apply(send):\n    items: list[int] = [1]\n    items.append(True)\n    send(1)\n";
        let initialize =
            "def report(value):\n    print(value)\nhandlers = {'cb': report}\ncount: int = 'bad'\n";
        for source in [
            format!("{registered}{initialize}apply(handlers[input()])\n"),
            format!("{initialize}{registered}apply(handlers[input()])\n"),
        ] {
            std::fs::write(dir.path().join("operations.py"), source).unwrap();
            let error = crate::check_file(&app, None).unwrap_err();
            assert!(error.contains("expected int, got str"), "{error}");
        }
    }

    #[test]
    fn file_entry_uses_one_native_body_for_arithmetic_and_object_dispatch() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("report.py");
        let source = concat!(
            "def report(x: int, sink, y: int):\n",
            "    answer = x + y\n",
            "    sink(answer)\n",
            "def echo(value):\n    print(value)\n",
            "def pair(value):\n    print(value + value, value + 1)\n",
            "choices = {'a': echo, 'b': pair}\n",
            "report(int(input()), choices[input()], int(input()))\n",
        );
        std::fs::write(&path, source.replace('\n', "\r\n")).unwrap();
        crate::check_file(&path, None).unwrap();
        let compilation =
            crate::compile_file(&path, None, &crate::CompileOptions::default()).unwrap();
        let ir = &compilation.llvm_ir;
        let report = registered_body_at(ir, "report.py", 2);
        let add = "call i64 @int_add(";
        let callback = "call i64 @imre_dyn_positional_call_at(";
        assert_eq!(report.matches(add).count(), 1);
        assert_eq!(report.matches(callback).count(), 1);
        assert!(report.find(add).unwrap() < report.find(callback).unwrap());
        let pair = registered_body_at(ir, "report.py", 7);
        assert!(
            !pair.contains("call i64 @imre_dyn_exact_int_word("),
            "{pair}"
        );
        assert!(!pair.contains(add));
        let plus = ir
            .lines()
            .find_map(|line| {
                let (symbol, data) = line.split_once(" = private unnamed_addr constant ")?;
                data.ends_with(r#"c"+\00""#).then_some(symbol)
            })
            .expect("Python addition protocol name");
        assert_eq!(
            pair.matches(&format!("call i64 @imre_dyn_op_at(ptr %ctx, ptr {plus},"))
                .count(),
            2
        );
    }
}
