//! Lower functions whose live Python arguments are supplied by the registered binder.

use super::*;

pub(super) mod booleans;
mod eager;
mod generators;
pub(crate) use generators::RegisteredGenerator;
mod lambdas;
pub(crate) use lambdas::{LambdaBinding, LambdaDefault, RegisteredLambda};
mod templates;
pub(crate) use templates::{PreparedStrings, PreparedTemplate, TemplateField};
pub(super) mod patterns;
pub(super) mod values;

/// Prepare the bindings installed by the existing type-parameter frame constructor.
pub(crate) fn registered_type_parameters(
    params: &ast::TypeParams,
    private_names: &imre_ast::Names,
) -> Vec<ir::RegisteredTypeParameter> {
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
                has_bound_or_default: param.default().is_some()
                    || matches!(param, ast::TypeParam::TypeVar(v) if v.bound.is_some()),
            }
        })
        .collect()
}

/// An expression can own private slots without introducing another Python function.
pub(crate) struct RegisteredOperand {
    pub(crate) value: Expr,
    #[cfg(any(debug_assertions, test, feature = "verify-ir"))]
    locals: Vec<Type>,
    pub(crate) local_names: HashMap<usize, LocalBinding>,
}

#[derive(Clone, Copy)]
enum RegionRoot<'a> {
    Suite(&'a [Statement]),
    Operand(&'a Expr),
}

#[derive(Clone, Copy)]
pub(crate) struct RegisteredRegion<'a> {
    #[cfg(any(debug_assertions, test, feature = "verify-ir"))]
    pub(crate) locals: &'a [Type],
    #[cfg(any(debug_assertions, test, feature = "verify-ir"))]
    pub(crate) params: &'a [usize],
    pub(crate) local_names: &'a HashMap<usize, LocalBinding>,
    root: RegionRoot<'a>,
}

impl<'a> RegisteredRegion<'a> {
    pub(crate) fn walk(self, visit: &mut impl FnMut(ir::Node<'a>, Option<&'a Expr>)) {
        match self.root {
            RegionRoot::Suite(body) => ir::walk_nodes(body, visit),
            RegionRoot::Operand(value) => ir::walk_operand_nodes(value, visit),
        }
    }

    pub(crate) fn private_slots(self) -> std::collections::BTreeSet<usize> {
        let mut slots = std::collections::BTreeSet::new();
        self.walk(&mut |node, _| match node {
            ir::Node::Statement(Statement::Scope { clear, .. })
            | ir::Node::Expression(Expr {
                kind: ExprKind::Scope { clear, .. },
                ..
            }) => {
                slots.extend(clear);
            }
            _ => {}
        });
        slots.retain(|slot| !self.local_names.contains_key(slot));
        slots
    }
}

impl RegisteredOperand {
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
        let operations = HashMap::new();
        let sites = StatementSites::default();
        let mut scope = SourceScope::new(kind, source, &operations, &sites);
        let value = lower(&mut scope)?;
        Some(Self::from_scope(value, scope))
    }

    fn from_scope(value: Expr, scope: SourceScope<'_>) -> Self {
        let (_locals, local_names) = scope.bindings();
        Self {
            value,
            #[cfg(any(debug_assertions, test, feature = "verify-ir"))]
            locals: _locals,
            local_names,
        }
    }

    pub(crate) fn region(&self) -> RegisteredRegion<'_> {
        RegisteredRegion {
            #[cfg(any(debug_assertions, test, feature = "verify-ir"))]
            locals: &self.locals,
            #[cfg(any(debug_assertions, test, feature = "verify-ir"))]
            params: &[],
            local_names: &self.local_names,
            root: RegionRoot::Operand(&self.value),
        }
    }
}

pub(crate) struct RegisteredFunction {
    pub(crate) function: ir::Function,
    pub(crate) local_names: HashMap<usize, LocalBinding>,
}

impl RegisteredFunction {
    pub(super) fn expression(range: TextRange, value: Expr) -> Self {
        let checkpoint = operation(
            Builtin::RegisteredStatement(range, None),
            Type::None,
            vec![],
        );
        let body = vec![Statement::Expr(checkpoint), Statement::Return(value)];
        Self {
            function: ir::Function {
                params: vec![],
                returns: Type::PythonObject,
                locals: vec![],
                body,
            },
            local_names: HashMap::new(),
        }
    }

    pub(crate) fn region(&self) -> RegisteredRegion<'_> {
        RegisteredRegion {
            #[cfg(any(debug_assertions, test, feature = "verify-ir"))]
            locals: &self.function.locals,
            #[cfg(any(debug_assertions, test, feature = "verify-ir"))]
            params: &self.function.params,
            local_names: &self.local_names,
            root: RegionRoot::Suite(&self.function.body),
        }
    }
}

/// Storage in the function frame or in a lexically nested Python namespace.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct LocalBinding {
    pub(crate) name: String,
    pub(crate) namespace: Option<usize>,
}

/// Lower a definition whose binder supplies Python object handles in its normal function scope.
/// A call to a generator returns its resumable frame. Keep the actual checked body
/// so emission can bind the same instructions that established this calling convention.
pub(super) fn generator_body(
    module: &crate::modules::SourceModule,
    definition: &ast::StmtFunctionDef,
    optimization: i32,
    source: &values::ModuleSource,
) -> Option<RegisteredFunction> {
    let physical = module.parsed.syntax().body.iter().find_map(|stmt| {
        stmt.as_function_def_stmt()
            .filter(|original| original.start() == definition.start())
    })?;
    if physical.is_async || !imre_ast::code_flags::is_generator(&physical.body) {
        return None;
    }
    let operations = HashMap::new();
    let preparation = SuitePreparation::new(
        &operations,
        source.context(&module.source),
        &physical.body,
        optimization == 0,
    );
    preparation
        .complete(
            &physical.body,
            RegisteredScope::Function { is_async: false },
        )
        .map(|suite| suite.checked)
}

pub(super) fn lower_definition<'a>(
    mut checker: Checker<'a>,
    definition: &ast::StmtFunctionDef,
    signature: &Signature,
    literals: &'a values::ModuleSource,
) -> Result<RegisteredFunction, String> {
    checker.registered = Some(literals);
    let source = checker.source;
    let helpers = checker.helpers;
    let first_helper = helpers.len();
    let mut local_names = HashMap::new();
    let (body, _) = lower_function(checker, definition, signature, Some(&mut local_names))?;
    if !body.calls.is_empty() || helpers.len() != first_helper {
        return Err(error(
            source,
            definition,
            "the registered frame requires a callable with a Python binding",
        )
        .classify(Kind::ScopedObject));
    }
    let function = RegisteredFunction {
        function: body.function,
        local_names,
    };
    if !crate::llvm::registered::supports(&function) {
        return Err(error(
            source,
            definition,
            "function body has no checked registered-frame representation",
        )
        .classify(Kind::ScopedObject));
    }
    Ok(function)
}

/// A registered body can supply an unresolved calling convention only if its other
/// operations satisfy the first-party source contract. Cache the check per physical module.
#[derive(Default)]
pub(super) struct SourceChecks {
    session: std::cell::OnceCell<Option<crate::analysis::Session>>,
    errors: HashMap<ModuleId, Vec<TextRange>>,
}

impl SourceChecks {
    pub(super) fn accepts(
        &mut self,
        graph: &SourceGraph,
        module: ModuleId,
        definition: &ast::StmtFunctionDef,
    ) -> bool {
        let Some(session) = self
            .session
            .get_or_init(|| graph.session.registered_body_checks().ok())
        else {
            return false;
        };
        self.errors
            .entry(module)
            .or_insert_with(|| session.source_error_ranges(module))
            .iter()
            .all(|range| !definition.range().contains_range(*range))
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
            kind: ExprKind::BooleanValue(and, Box::new(left), Box::new(result)),
        };
    }
    result
}

pub(super) enum Capture<'a> {
    Solved,
    IncludeLive,
    Entry(&'a HashSet<usize>),
}

pub(super) fn capture(
    graph: &SourceGraph,
    signatures: &HashMap<DefinitionId, Signature>,
    classes: &ClassInfo,
    types: &Types,
    inference: &inference::Inference,
    globals: &ModuleValues,
    mode: Capture<'_>,
) {
    let mut sources = HashMap::new();
    let mut checks = SourceChecks::default();
    let mut pending = inference.pending_bodies.borrow_mut();
    let owners = pending.get_or_insert_with(|| {
        // Inference can request recovery before signatures settle.
        signatures
            .keys()
            .copied()
            .filter(|owner| graph.modules[owner.module].is_program_source())
            .collect()
    });
    owners.retain(|&owner| {
        let signature = &signatures[&owner];
        let id = owner.module;
        let module = &graph.modules[id];
        // Use the original definition. The registered binder retains its names, defaults and frame.
        let Some(function) = module.parsed.syntax().body[owner.statement].as_function_def_stmt()
        else {
            return false;
        };
        // Recovery may follow an owning definition that has already checked this site.
        if graph
            .registered_functions()
            .get(&id)
            .is_some_and(|bodies| bodies.contains_key(&function.start()))
        {
            return false;
        }
        let (signature, solved) = if let Some(signature) = inference.registered_signature(signature)
        {
            if !matches!(mode, Capture::IncludeLive) {
                return true;
            }
            (signature, false)
        } else if let Some(mut signature) = inference.checked_signature(signature)
            && signature.params.iter().all(|p| p.ty != Type::PythonObject)
        {
            // The fixed-layout checker owns the source return contract. The Python
            // activation returns a handle, including the result of an open call.
            signature.returns = Type::PythonObject;
            (signature, true)
        } else {
            return true;
        };
        let helpers = Helpers::new(inference.len());
        // A solved definition reads names from the Python namespace. Its trial
        // must not establish a native caller or consume a missing binding.
        let private = solved.then(ModuleValues::default);
        let mut checker = Checker::new(
            graph,
            id,
            signatures,
            classes,
            types,
            &helpers,
            private.as_ref().unwrap_or(globals),
        );
        // The fixed checker validated this physical signature in the settled round.
        checker.checked_callback_effects =
            solved && matches!(mode, Capture::Entry(checked) if checked.contains(&signature.id));
        let literals = sources
            .entry(id)
            .or_insert_with(|| values::ModuleSource::new(module));
        let lower = || lower_definition(checker, function, &signature, literals).ok();
        let body = if solved {
            graph.try_registered_body(id, lower)
        } else {
            lower()
        };
        if let Some(body) = body
            && checks.accepts(graph, id, function)
        {
            graph.register_function(id, function.start(), body);
            return false;
        }
        true
    });
}

pub(super) fn checkpoint(stmt: &ast::Stmt) -> Statement {
    // Truth conversion can call Python after the condition's last operand returns.
    let range = match stmt {
        ast::Stmt::If(branch) => branch.test.range(),
        _ => stmt.range(),
    };
    Statement::Expr(operation(
        Builtin::RegisteredStatement(range, imre_ast::unhighlighted_call(stmt)),
        Type::None,
        vec![],
    ))
}

fn operation(op: Builtin, ty: Type, operands: Vec<Expr>) -> Expr {
    Expr {
        ty,
        kind: ExprKind::Builtin(op, operands),
    }
}

/// The implicit format argument is checked in the lazy evaluator, before its RHS.
pub(crate) fn type_alias_body(
    value: &ast::Expr,
    source: values::RegisteredSource<'_>,
) -> Option<AnnotationCode> {
    let mut code = AnnotationCode::prepare(value, source)?;
    let format = operation(
        Builtin::RegisteredAnnotationFormat,
        Type::None,
        vec![values::name_binding(".format")],
    );
    code.operand.value = Expr {
        ty: code.operand.value.ty,
        kind: ExprKind::Scope {
            body: vec![Statement::Expr(format)],
            value: Box::new(code.operand.value),
            clear: Vec::new(),
        },
    };
    Some(code)
}

pub(crate) fn registered_type_alias(site: TextSize) -> Expr {
    operation(
        Builtin::RegisteredTypeAlias(site),
        Type::PythonObject,
        vec![],
    )
}

/// Preserve the requested module separately from the names bound in the current namespace.
fn registered_import(stmt: &ast::Stmt, private_names: &imre_ast::Names) -> Option<Expr> {
    let import = match stmt {
        ast::Stmt::Import(import) => ir::NamespaceImport::Modules(
            import
                .names
                .iter()
                .map(|alias| {
                    let name = alias.name.as_str();
                    let root = name.split('.').next().unwrap_or(name);
                    ir::ModuleImport {
                        name: name.to_owned(),
                        binding: alias
                            .asname
                            .as_ref()
                            .map_or(root, ast::Identifier::as_str)
                            .to_owned(),
                        bind_leaf: alias.asname.is_some()
                            && !private_names.imports_root(&alias.name),
                    }
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
                        alias.asname.as_ref().unwrap_or(&alias.name).to_string(),
                    )
                })
                .collect(),
        },
        _ => return None,
    };
    Some(operation(
        Builtin::RegisteredImport(import),
        Type::None,
        vec![],
    ))
}

#[derive(Clone, Copy)]
pub(crate) enum RegisteredScope {
    Module,
    Class,
    Lambda,
    Function { is_async: bool },
}

/// Only compound, valid strings outside the bundled library get a separately bound
/// expression. A literal or name retains exactly the ordinary Python str.
pub(crate) fn annotation_expression(
    text: &str,
) -> Option<ruff_python_parser::Parsed<ast::ModExpression>> {
    let parsed = imre_ast::forward_refs::parse(text).ok()?;
    imre_ast::forward_refs::validate(&parsed).ok()?;
    let expr = &parsed.syntax().body;
    (!expr.is_name_expr() && !expr.is_literal_expr()).then_some(parsed)
}

pub(crate) struct DeferredAnnotation {
    pub(crate) range: TextRange,
    pub(crate) name: String,
    pub(crate) value: AnnotationValue,
}

pub(crate) struct ParameterAnnotations {
    pub(crate) entries: Vec<ir::AnnotationOccurrence>,
    pub(crate) body: RegisteredSuite,
    pub(crate) references: HashMap<String, String>,
}

impl ParameterAnnotations {
    fn prepare<R>(
        def: &ast::StmtFunctionDef,
        operands: &HashMap<TextSize, PreparedAnnotation<R>>,
        sites: &mut HashMap<TextSize, DeferredAnnotation>,
    ) -> Option<Self> {
        let annotations = parameter_annotations(&def.parameters, def.returns.as_deref());
        if annotations.is_empty() {
            return None;
        }
        let mut entries = Vec::with_capacity(annotations.len());
        let mut references = HashMap::new();
        for (name, expression) in annotations {
            let site = expression.start();
            if let Some(reference) = scalar_reference(expression) {
                references.insert(name.to_owned(), reference);
            }
            sites.insert(
                site,
                DeferredAnnotation::new(name.to_owned(), expression, operands),
            );
            entries.push(ir::AnnotationOccurrence { site, index: None });
        }
        let body = DeferredAnnotation::prepare_body(sites, &entries);
        Some(Self {
            entries,
            body,
            references,
        })
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

pub(crate) enum AnnotationValue {
    Checked(Expr),
    Source(ast::Expr),
}

impl DeferredAnnotation {
    fn new<R>(
        name: String,
        expression: &ast::Expr,
        operands: &HashMap<TextSize, super::PreparedAnnotation<R>>,
    ) -> Self {
        let value = operands.get(&expression.start()).map_or_else(
            || AnnotationValue::Source(expression.clone()),
            |operand| AnnotationValue::Checked(operand.operand(expression.start())),
        );
        Self {
            name,
            range: expression.range(),
            value,
        }
    }

    /// Number already-checked operands in physical marker order, including repeated finally edges.
    pub(crate) fn prepare_body(
        sites: &HashMap<TextSize, Self>,
        entries: &[ir::AnnotationOccurrence],
    ) -> RegisteredSuite {
        let mut residual = Vec::new();
        let mut body = vec![Statement::Expr(operation(
            Builtin::RegisteredAnnotationStart,
            Type::None,
            vec![],
        ))];
        for (index, occurrence) in entries.iter().enumerate() {
            let value = match &sites[&occurrence.site].value {
                AnnotationValue::Checked(value) => value.clone(),
                AnnotationValue::Source(_) => {
                    SourceRun::record(&mut residual, index, body.len());
                    continue;
                }
            };
            body.push(Statement::Expr(operation(
                Builtin::RegisteredAnnotationInsert(*occurrence),
                Type::None,
                vec![value],
            )));
        }
        body.push(Statement::Return(operation(
            Builtin::RegisteredAnnotationResult,
            Type::PythonObject,
            vec![],
        )));
        RegisteredSuite {
            checked: RegisteredFunction {
                function: ir::Function {
                    params: vec![],
                    returns: Type::PythonObject,
                    locals: vec![],
                    body,
                },
                local_names: HashMap::new(),
            },
            residual,
        }
    }
}

/// Physical source operands shared by namespace initialization and lazy annotation providers.
pub(crate) struct FunctionSite<'a> {
    pub(crate) def: &'a ast::StmtFunctionDef,
    pub(crate) prepare_entry: bool,
    pub(crate) thread_entry: Option<imre_ast::ThreadEntry>,
}

pub(crate) struct ClassSite<'a> {
    pub(crate) def: &'a ast::StmtClassDef,
    enclosing: RegisteredScope,
    parameters: Option<Vec<ir::RegisteredTypeParameter>>,
}

pub(crate) struct RegisteredClass {
    pub(crate) body: RegisteredSuite,
    pub(crate) header: ClassHeader,
}

/// Decorators use the enclosing activation. Generic bases and keywords use the
/// type-parameter activation. A missing operand keeps its individual source callback.
pub(crate) struct ClassHeader {
    pub(crate) parameters: Option<Vec<ir::RegisteredTypeParameter>>,
    pub(crate) layout: ir::PackedArguments,
    pub(crate) operands: Vec<Option<RegisteredOperand>>,
    pub(crate) decorators: Vec<Option<RegisteredOperand>>,
}

#[derive(Default)]
pub(crate) struct RegisteredSites<'a> {
    pub(crate) operations: HashMap<TextRange, Expr>,
    pub(crate) functions: Vec<FunctionSite<'a>>,
    pub(crate) classes: Vec<ClassSite<'a>>,
    pub(crate) annotations: HashMap<TextSize, DeferredAnnotation>,
    pub(crate) parameter_annotations: HashMap<TextSize, ParameterAnnotations>,
}

pub(crate) fn registered_sites<'a, R>(
    body: &'a [ast::Stmt],
    names: &imre_ast::Names,
    operands: &HashMap<TextSize, PreparedAnnotation<R>>,
) -> RegisteredSites<'a> {
    use ast::statement_visitor::{StatementVisitor, walk_stmt};
    struct Collector<'s, 'a, R> {
        names: &'s imre_ast::Names,
        operands: &'s HashMap<TextSize, PreparedAnnotation<R>>,
        scope: RegisteredScope,
        in_unprepared_generic: bool,
        output: RegisteredSites<'a>,
    }
    impl<'a, R> StatementVisitor<'a> for Collector<'_, 'a, R> {
        fn visit_stmt(&mut self, stmt: &'a ast::Stmt) {
            if let Some(import) = registered_import(stmt, self.names) {
                self.output.operations.insert(stmt.range(), import);
            }
            match stmt {
                ast::Stmt::ClassDef(class) => {
                    self.output.classes.push(ClassSite {
                        def: class,
                        enclosing: self.scope,
                        parameters: class
                            .type_params
                            .as_deref()
                            .map(|params| registered_type_parameters(params, self.names)),
                    });
                    let outer = std::mem::replace(&mut self.scope, RegisteredScope::Class);
                    walk_stmt(self, stmt);
                    self.scope = outer;
                }
                ast::Stmt::FunctionDef(def) => {
                    if let Some(provider) = ParameterAnnotations::prepare(
                        def,
                        self.operands,
                        &mut self.output.annotations,
                    ) {
                        self.output
                            .parameter_annotations
                            .insert(def.parameters.start(), provider);
                    }
                    let generic_binding = !matches!(self.scope, RegisteredScope::Function { .. })
                        && super::function_scope::type_parameter_scope(
                            def,
                            self.names.string_annotations,
                            matches!(self.scope, RegisteredScope::Class),
                            Some(self.names),
                        )
                        .is_some();
                    if generic_binding {
                        self.output.operations.insert(
                            def.name.range(),
                            operation(
                                Builtin::RegisteredDefinition(ir::DefinitionSite::Function(
                                    def.start(),
                                )),
                                Type::PythonObject,
                                Vec::new(),
                            ),
                        );
                    }
                    // Nested ordinary functions of an admitted definition take the
                    // same preparation path as any other lexical function.
                    let unprepared = !generic_binding
                        && (self.in_unprepared_generic || def.type_params.is_some());
                    let thread_entry = imre_ast::thread_entry(def);
                    let worker = thread_entry.is_some();
                    self.output.functions.push(FunctionSite {
                        def,
                        prepare_entry: !unprepared || worker,
                        thread_entry,
                    });
                    if worker {
                        // A proven worker contains no nested declarations or namespace effects.
                        return;
                    }
                    let outer_generic =
                        std::mem::replace(&mut self.in_unprepared_generic, unprepared);
                    let outer = std::mem::replace(
                        &mut self.scope,
                        RegisteredScope::Function {
                            is_async: def.is_async,
                        },
                    );
                    walk_stmt(self, stmt);
                    self.scope = outer;
                    self.in_unprepared_generic = outer_generic;
                }
                ast::Stmt::AnnAssign(ann)
                    if !matches!(self.scope, RegisteredScope::Function { .. }) && ann.simple =>
                {
                    let Some(name) = ann.target.as_name_expr() else {
                        return;
                    };

                    let name = name.id.to_string();
                    let (annotation, values) = if self.names.string_annotations {
                        let site = ann.annotation.start();
                        (
                            ir::NamespaceAnnotation::String { name },
                            vec![self.operands[&site].operand(site)],
                        )
                    } else {
                        self.output.annotations.insert(
                            ann.start(),
                            DeferredAnnotation::new(name.clone(), &ann.annotation, self.operands),
                        );
                        (
                            ir::NamespaceAnnotation::Deferred {
                                site: ann.start(),
                                name,
                            },
                            Vec::new(),
                        )
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
                _ => walk_stmt(self, stmt),
            }
        }
    }
    let mut sites = Collector {
        names,
        operands,
        scope: RegisteredScope::Module,
        in_unprepared_generic: false,
        output: RegisteredSites::default(),
    };
    sites.visit_body(body);
    sites.output
}

/// Checked instructions retain their canonical function layout. Source runs are owned
/// by the physical suite and identify the remaining gaps between checked statements.
pub(crate) struct SourceRun {
    pub(crate) at: usize,
    pub(crate) statements: std::ops::Range<usize>,
    pub(crate) effect: Option<SourceEffect>,
}

pub(crate) enum SourceEffect {
    /// Evaluate the RHS once for the following checked assignment targets.
    AssignmentInput(usize),
    /// Evaluate the operands at their place in the operation, in the current frame.
    ObjectOperation(Box<Builtin>, Vec<PhysicalOperand>),
    /// A mixed loop needs its source suites. A literal false test runs only the else suite.
    While(SourceGuard<Box<RegisteredOperand>>),
    /// A loop whose checked lowering left one or more physical inputs or suites.
    For(Box<PreparedFor>),
}

pub(crate) struct PreparedFor {
    pub(crate) range: TextRange,
    pub(crate) iterable_range: TextRange,
    pub(crate) acquire: ir::IteratorOp,
    pub(crate) pull: ir::IterNextKind,
    pub(crate) iterable: PhysicalOperand,
    pub(crate) target: LoopBinding,
    pub(crate) body: LoopSuite,
    pub(crate) otherwise: Option<LoopSuite>,
}

pub(crate) enum LoopBinding {
    Checked {
        entered: usize,
        binding: Box<RegisteredFunction>,
    },
    Source(Box<ast::Expr>),
}

pub(crate) struct LoopSuite {
    pub(crate) plan: RegisteredSuite,
    pub(crate) sources: Vec<ast::Stmt>,
}

impl LoopSuite {
    fn new(mut plan: RegisteredSuite, stmts: &[ast::Stmt]) -> Self {
        let mut sources = Vec::new();
        for run in &mut plan.residual {
            let start = sources.len();
            if !matches!(
                run.effect,
                Some(SourceEffect::For(_) | SourceEffect::ObjectOperation(..))
            ) {
                sources.extend_from_slice(&stmts[run.statements.clone()]);
            }
            run.statements = start..sources.len();
        }
        Self { plan, sources }
    }
}

pub(crate) enum PhysicalOperand {
    Checked(Box<RegisteredOperand>),
    Source(Box<ast::Expr>),
}

impl SourceRun {
    fn record(runs: &mut Vec<Self>, source_index: usize, checked_offset: usize) {
        if let Some(run) = runs.last_mut()
            && run.effect.is_none()
            && run.at == checked_offset
            && run.statements.end == source_index
        {
            run.statements.end += 1;
        } else {
            runs.push(Self {
                at: checked_offset,
                statements: source_index..source_index + 1,
                effect: None,
            });
        }
    }
}

pub(crate) struct RegisteredSuite {
    pub(crate) checked: RegisteredFunction,
    pub(crate) residual: Vec<SourceRun>,
}

impl RegisteredSuite {
    pub(crate) fn fully_checked(&self) -> bool {
        self.residual.is_empty()
    }
}

fn checked_assertion(
    stmt: &ast::StmtAssert,
    mut operand: impl FnMut(&ast::Expr) -> Option<Expr>,
) -> Option<Vec<Statement>> {
    let condition = operand(&stmt.test)?;
    let message = match stmt.msg.as_deref() {
        Some(message) => vec![operand(message)?],
        None => Vec::new(),
    };
    let test = operation(Builtin::Bool, Type::Bool, vec![condition]);
    let failure = operation(
        Builtin::RegisteredAssertionFailure {
            statement: stmt.start(),
            test: stmt.test.start(),
        },
        Type::None,
        message,
    );
    // The implicit truth conversion can call Python on the test's physical line.
    Some(vec![
        Statement::Expr(operation(
            Builtin::RegisteredStatement(stmt.test.range(), None),
            Type::None,
            vec![],
        )),
        Statement::If(test, Vec::new(), vec![Statement::Expr(failure)]),
    ])
}

/// Physical source locations and semantic operations shared by complete and residual suites.
#[derive(Default)]
pub(crate) struct StatementSites {
    pub(crate) expressions: booleans::ExpressionSites,
    pub(crate) assertions: HashMap<TextRange, Vec<Statement>>,
    pub(crate) ifs: HashMap<TextRange, SourceConditional>,
    pub(crate) matches: HashMap<TextRange, Option<patterns::SourceMatch>>,
    pub(crate) contexts: HashMap<TextRange, Vec<ContextInput>>,
}

pub(crate) struct ContextInput {
    pub(crate) site: ir::ContextSite,
    pub(crate) manager: Option<Expr>,
    pub(crate) target: ContextTarget,
}

pub(crate) enum ContextTarget {
    Absent,
    Residual,
    Checked {
        entered: usize,
        binding: Box<RegisteredFunction>,
    },
}

impl ContextTarget {
    fn prepare(
        target: &ast::Expr,
        source: values::RegisteredSource<'_>,
        kind: RegisteredScope,
    ) -> Self {
        match checked_binding(target, source, kind) {
            Some((entered, binding)) => Self::Checked { entered, binding },
            None => Self::Residual,
        }
    }
}

/// Bind one externally supplied value with the scope and slot layout of its target.
fn checked_binding(
    target: &ast::Expr,
    source: values::RegisteredSource<'_>,
    kind: RegisteredScope,
) -> Option<(usize, Box<RegisteredFunction>)> {
    let operations = HashMap::new();
    let sites = StatementSites::default();
    let mut scope = SourceScope::new(kind, source, &operations, &sites);
    let ir::ContextBinding { entered, body } = scope.with_binding(target).ok()?;
    PhysicalRegion::Fallthrough
        .accepts(&body)
        .then(|| (entered, Box::new(scope.finish(body))))
}

fn for_pull(loop_: &ast::StmtFor) -> Option<ir::AsyncPull> {
    loop_.is_async.then(|| ir::AsyncPull {
        range: loop_.iter.range(),
        header: Some(loop_.start()),
    })
}

fn iteration_protocol(
    iterable: TextRange,
    pull: Option<ir::AsyncPull>,
) -> (ir::IteratorOp, ir::IterNextKind) {
    match pull {
        Some(pull) => (ir::IteratorOp::Async(iterable), ir::IterNextKind::Async(pull)),
        None => (ir::IteratorOp::Loop, ir::IterNextKind::Generic),
    }
}

/// Source sites for a statement which could not be lowered as a complete checked block.
pub(crate) struct SourceConditional(Vec<SourceIfArm>);

pub(crate) struct SourceIfArm {
    clause: Option<usize>,
    pub(crate) body_start: Option<TextSize>,
    guard: Option<SourceGuard>,
}

pub(crate) enum SourceGuard<T = Expr> {
    Literal(bool),
    Checked(T),
    Residual,
}

impl SourceIfArm {
    pub(crate) fn test<'a>(&self, source: &'a ast::StmtIf) -> &'a ast::Expr {
        match self.clause {
            Some(index) => source.elif_else_clauses[index]
                .test
                .as_ref()
                .expect("conditional guard"),
            None => &source.test,
        }
    }

    pub(crate) fn body<'a>(&self, source: &'a ast::StmtIf) -> &'a [ast::Stmt] {
        match self.clause {
            Some(index) => &source.elif_else_clauses[index].body,
            None => &source.body,
        }
    }
}

impl SourceConditional {
    /// Dead literal arms in a suspended frame have no resume entry.
    pub(crate) fn arms(
        &self,
        suspended: bool,
    ) -> impl Iterator<Item = (Option<&SourceGuard>, &SourceIfArm)> {
        self.0.iter().filter_map(move |arm| {
            let test = match &arm.guard {
                Some(SourceGuard::Literal(false)) if suspended => return None,
                Some(SourceGuard::Literal(true)) if suspended => None,
                guard => guard.as_ref(),
            };
            Some((test, arm))
        })
    }
}

fn if_clauses(branch: &ast::StmtIf) -> impl Iterator<Item = (Option<&ast::Expr>, &[ast::Stmt])> {
    std::iter::once((Some(branch.test.as_ref()), branch.body.as_slice())).chain(
        branch
            .elif_else_clauses
            .iter()
            .map(|clause| (clause.test.as_ref(), clause.body.as_slice())),
    )
}

fn statement_sites(
    body: &[ast::Stmt],
    source: values::RegisteredSource<'_>,
    assertions_enabled: bool,
) -> StatementSites {
    use ast::statement_visitor::{StatementVisitor, walk_stmt};

    struct Collector<'a> {
        source: values::RegisteredSource<'a>,
        assertions_enabled: bool,
        sites: StatementSites,
    }

    impl<'a> StatementVisitor<'a> for Collector<'_> {
        fn visit_stmt(&mut self, stmt: &'a ast::Stmt) {
            if let ast::Stmt::Assert(assertion) = stmt {
                // An assertion with an unprepared operand remains in its physical suite.
                if !self.assertions_enabled {
                    self.sites.assertions.insert(stmt.range(), Vec::new());
                } else if let Some(body) = checked_assertion(assertion, |expr| {
                    values::registered_value(expr, self.source)
                }) {
                    self.sites.assertions.insert(stmt.range(), body);
                }
                return;
            }
            if let ast::Stmt::With(with) = stmt {
                let inputs = with
                    .items
                    .iter()
                    .map(|item| ContextInput {
                        site: context_site(self.source.text, &item.context_expr, with.is_async),
                        manager: None,
                        target: if item.optional_vars.is_some() {
                            ContextTarget::Residual
                        } else {
                            ContextTarget::Absent
                        },
                    })
                    .collect();
                self.sites.contexts.insert(stmt.range(), inputs);
            }
            if let ast::Stmt::Match(branch) = stmt {
                self.sites.matches.insert(
                    stmt.range(),
                    patterns::SourceMatch::prepare(branch, self.source),
                );
            }
            if let ast::Stmt::If(branch) = stmt {
                let guard = |expr: &ast::Expr| {
                    if let ast::Expr::BooleanLiteral(literal) = expr {
                        SourceGuard::Literal(literal.value)
                    } else {
                        values::registered_value(expr, self.source)
                            .map_or(SourceGuard::Residual, SourceGuard::Checked)
                    }
                };
                let arms = if_clauses(branch)
                    .enumerate()
                    .map(|(index, (test, body))| SourceIfArm {
                        clause: index.checked_sub(1),
                        body_start: body.first().map(Ranged::start),
                        guard: test.map(guard),
                    })
                    .collect();
                self.sites.ifs.insert(stmt.range(), SourceConditional(arms));
            }
            walk_stmt(self, stmt);
        }
    }

    let mut collector = Collector {
        source,
        assertions_enabled,
        sites: StatementSites::default(),
    };
    collector.visit_body(body);
    collector.sites
}

#[derive(Clone, Copy)]
enum SuiteSelection {
    All,
    Physical,
}

/// Prepare ordered suites while the source tree and its namespace operands are available.
pub(crate) struct SuitePreparation<'a> {
    operations: &'a HashMap<TextRange, Expr>,
    source: values::RegisteredSource<'a>,
    pub(crate) nested: HashMap<TextSize, RegisteredSuite>,
    pub(crate) sites: StatementSites,
}

impl<'a> SuitePreparation<'a> {
    pub(crate) fn new(
        operations: &'a HashMap<TextRange, Expr>,
        source: values::RegisteredSource<'a>,
        body: &[ast::Stmt],
        assertions_enabled: bool,
    ) -> Self {
        Self {
            operations,
            source,
            nested: HashMap::new(),
            sites: statement_sites(body, source, assertions_enabled),
        }
    }

    pub(crate) fn class_suites(
        &mut self,
        classes: Vec<ClassSite<'_>>,
    ) -> HashMap<TextSize, RegisteredClass> {
        classes
            .into_iter()
            .map(|site| {
                let class = site.def;
                let start = class
                    .decorator_list
                    .first()
                    .map_or(class.start(), Ranged::start);
                let decorators = class
                    .decorator_list
                    .iter()
                    .map(|decorator| {
                        let expr = &decorator.expression;
                        let operand = RegisteredOperand::prepare(expr, self.source, site.enclosing);
                        if operand.is_none() {
                            self.sites.expressions.prepare_input(expr, self.source);
                        }
                        operand
                    })
                    .collect();
                let bases_scope = if site.parameters.is_some() {
                    RegisteredScope::Function { is_async: false }
                } else {
                    site.enclosing
                };
                let (layout, operands) =
                    class
                        .arguments
                        .as_deref()
                        .map_or_else(Default::default, |args| {
                            let operands = registered_arguments(args)
                                .map(|expr| {
                                    let operand = RegisteredOperand::prepare(expr, self.source, bases_scope);
                                    if operand.is_none() {
                                        self.sites.expressions.prepare_input(expr, self.source);
                                    }
                                    operand
                                })
                                .collect();
                            (packed_arguments(args), operands)
                        });
                let header = ClassHeader {
                    parameters: site.parameters,
                    layout,
                    operands,
                    decorators,
                };
                let body = self.suite(&class.body, RegisteredScope::Class);
                (start, RegisteredClass { body, header })
            })
            .collect()
    }

    pub(crate) fn suite(&mut self, stmts: &[ast::Stmt], kind: RegisteredScope) -> RegisteredSuite {
        self.selected_suite(stmts, kind, SuiteSelection::All, SuiteSelection::All)
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
        self.selected_suite(stmts, kind, SuiteSelection::All, children)
    }

    /// Keep the enclosing physical frame's entry and cleanup. Its complete target effects
    /// share one slot layout per suite, including when another statement must run from source.
    pub(crate) fn physical_body(&mut self, stmts: &[ast::Stmt], kind: RegisteredScope) {
        self.nested_suite(stmts, kind, SuiteSelection::Physical);
    }

    fn nested_suite(&mut self, stmts: &[ast::Stmt], kind: RegisteredScope, select: SuiteSelection) {
        if let Some(first) = stmts.first() {
            let plan = self.selected_suite(stmts, kind, select, select);
            // An optimized-away assertion has no instructions and still owns its source slot.
            if !matches!(plan.residual.as_slice(), [run]
                if run.effect.is_none() && run.statements == (0..stmts.len()))
            {
                self.nested.insert(first.start(), plan);
            }
        }
    }

    fn selected_suite(
        &mut self,
        stmts: &[ast::Stmt],
        kind: RegisteredScope,
        select: SuiteSelection,
        child_selection: SuiteSelection,
    ) -> RegisteredSuite {
        use ast::statement_visitor::{StatementVisitor, walk_stmt};
        let mut plan = self.lower(stmts, kind, select);
        struct Children<'p, 's> {
            prepare: &'p mut SuitePreparation<'s>,
            kind: RegisteredScope,
            select: SuiteSelection,
        }
        impl<'a> StatementVisitor<'a> for Children<'_, '_> {
            fn visit_body(&mut self, stmts: &'a [ast::Stmt]) {
                self.prepare.nested_suite(stmts, self.kind, self.select);
            }
        }
        let mut children = Children {
            prepare: self,
            kind,
            select: child_selection,
        };
        for run in &plan.residual {
            for stmt in &stmts[run.statements.clone()] {
                if !stmt.is_match_stmt() {
                    children.prepare.sites.expressions.prepare_inputs(stmt, children.prepare.source);
                }
            }
        }
        for run in std::mem::take(&mut plan.residual) {
            if run.effect.is_some() {
                if matches!(run.effect, Some(SourceEffect::While(_))) {
                    walk_stmt(&mut children, &stmts[run.statements.start]);
                }
                plan.residual.push(run);
                continue;
            }
            for index in run.statements {
                let stmt = &stmts[index];
                if let ast::Stmt::For(loop_) = stmt {
                    let region = children.prepare.prepare_for(loop_, kind, child_selection);
                    plan.residual.push(SourceRun {
                        at: run.at,
                        statements: index..index + 1,
                        effect: Some(SourceEffect::For(Box::new(region))),
                    });
                    continue;
                }
                SourceRun::record(&mut plan.residual, index, run.at);
                if let ast::Stmt::With(with) = stmt {
                    let inputs = children
                        .prepare
                        .sites
                        .contexts
                        .get_mut(&stmt.range())
                        .expect("with inputs");
                    let region = match (kind, child_selection) {
                        (RegisteredScope::Function { .. }, SuiteSelection::Physical) => {
                            values::Region::SuspendedOperand
                        }
                        _ => values::Region::Residual,
                    };
                    for (input, item) in inputs.iter_mut().zip(&with.items) {
                        input.manager = region.operand(&item.context_expr, children.prepare.source);
                        if let Some(target) = &item.optional_vars {
                            input.target =
                                ContextTarget::prepare(target, children.prepare.source, kind);
                        }
                    }
                }
                if let ast::Stmt::Match(branch) = stmt
                    && let Some(mut site) = children
                        .prepare
                        .sites
                        .matches
                        .get_mut(&branch.range())
                        .and_then(Option::take)
                {
                    site.subject = RegisteredOperand::prepare_with(
                        children.prepare.source,
                        kind,
                        |scope| scope.frame_value(&branch.subject),
                    );
                    if site.subject.is_none() {
                        children.prepare.sites.expressions.prepare_input(&branch.subject, children.prepare.source);
                    }
                    for (case, input) in site.cases.iter_mut().zip(&branch.cases) {
                        if matches!(case.guard, Some(patterns::MatchGuard::Residual))
                            && let Some(guard) = &input.guard
                        {
                            children.prepare.sites.expressions.prepare_input(guard, children.prepare.source);
                        }
                        case.body = Some(children.prepare.suite(&input.body, kind));
                    }
                    children
                        .prepare
                        .sites
                        .matches
                        .insert(branch.range(), Some(site));
                    continue;
                }
                // A declaration opens a separate activation with its own scope and admission.
                if !matches!(stmt, ast::Stmt::FunctionDef(_) | ast::Stmt::ClassDef(_)) {
                    walk_stmt(&mut children, stmt);
                }
            }
        }
        plan
    }

    fn prepare_for(
        &mut self,
        loop_: &ast::StmtFor,
        kind: RegisteredScope,
        selection: SuiteSelection,
    ) -> PreparedFor {
        let iterable = RegisteredOperand::prepare_with(self.source, kind, |scope| {
            match (kind, selection) {
                (RegisteredScope::Function { .. }, SuiteSelection::Physical) => {
                    scope.operand(&loop_.iter)
                }
                _ => scope.ordinary_operand(&loop_.iter),
            }
        });
        let iterable = match iterable {
            Some(operand) => PhysicalOperand::Checked(Box::new(operand)),
            None => PhysicalOperand::Source(loop_.iter.clone()),
        };
        let target = match checked_binding(&loop_.target, self.source, kind) {
            Some((entered, binding)) => LoopBinding::Checked { entered, binding },
            None => LoopBinding::Source(loop_.target.clone()),
        };
        let (acquire, pull) = iteration_protocol(loop_.iter.range(), for_pull(loop_));
        let mut suite = |stmts: &[ast::Stmt]| {
            LoopSuite::new(self.selected_suite(stmts, kind, selection, selection), stmts)
        };
        PreparedFor {
            range: loop_.range(),
            iterable_range: loop_.iter.range(),
            acquire,
            pull,
            iterable,
            target,
            body: suite(&loop_.body),
            otherwise: (!loop_.orelse.is_empty()).then(|| suite(&loop_.orelse)),
        }
    }

    fn lower(
        &self,
        stmts: &[ast::Stmt],
        kind: RegisteredScope,
        select: SuiteSelection,
    ) -> RegisteredSuite {
        let mut scope = SourceScope::new(kind, self.source, self.operations, &self.sites);
        let mut body = Vec::new();
        let mut residual: Vec<SourceRun> = Vec::new();
        for (i, stmt) in stmts.iter().enumerate() {
            let slots_before = scope.locals.len();
            let lowered = match select {
                SuiteSelection::All => scope.statement(stmt, &mut body),
                SuiteSelection::Physical => scope.physical_statement(stmt, &mut body),
            };
            if lowered.is_ok() {
                continue;
            }
            scope.truncate_locals(slots_before);
            if let ast::Stmt::Assign(assign) = stmt {
                let input = scope.hidden(Type::PythonObject);
                let value = unpacking::take_pattern_temporary(
                    Expr::local(input, Type::PythonObject),
                    input,
                );
                if let Ok(targets) = scope.assignment_targets(&assign.targets, value) {
                    body.push(checkpoint(stmt));
                    residual.push(SourceRun {
                        at: body.len(),
                        statements: i..i + 1,
                        effect: Some(SourceEffect::AssignmentInput(input)),
                    });
                    body.extend(targets);
                    continue;
                }
                scope.truncate_locals(slots_before);
            } else if let ast::Stmt::Delete(delete) = stmt {
                // A missing operand keeps its place between the other targets' checked effects.
                body.push(checkpoint(stmt));
                for target in delete_targets(delete) {
                    let (op, inputs) = deletion(target);
                    let slots = scope.locals.len();
                    if let Some(values) = inputs
                        .clone()
                        .map(|input| input.lower(&mut scope))
                        .collect()
                    {
                        body.push(Statement::Expr(operation(op, Type::None, values)));
                    } else {
                        scope.truncate_locals(slots);
                        residual.push(SourceRun {
                            at: body.len(),
                            statements: i..i + 1,
                            effect: Some(SourceEffect::ObjectOperation(
                                Box::new(op),
                                inputs
                                    .map(|input| input.physical(self.source, kind))
                                    .collect(),
                            )),
                        });
                    }
                }
                continue;
            }
            if let ast::Stmt::While(loop_) = stmt {
                let condition = match &*loop_.test {
                    ast::Expr::BooleanLiteral(value) => SourceGuard::Literal(value.value),
                    test => RegisteredOperand::prepare_with(self.source, kind, |scope| {
                        scope.while_condition(test)
                    })
                    .map_or(SourceGuard::Residual, |operand| {
                        SourceGuard::Checked(Box::new(operand))
                    }),
                };
                body.push(checkpoint(stmt));
                residual.push(SourceRun {
                    at: body.len(),
                    statements: i..i + 1,
                    effect: Some(SourceEffect::While(condition)),
                });
                continue;
            }
            SourceRun::record(&mut residual, i, body.len());
        }
        RegisteredSuite {
            checked: scope.finish(body),
            residual,
        }
    }

    pub(crate) fn complete(
        &self,
        stmts: &[ast::Stmt],
        kind: RegisteredScope,
    ) -> Option<RegisteredSuite> {
        let plan = self.lower(stmts, kind, SuiteSelection::All);
        plan.fully_checked().then_some(plan)
    }
}

/// A target's Python expression inputs in evaluation order. The receiver is evaluated
/// once, including for an in-place operation that both reads and writes its descriptor.
pub(crate) struct AttributeEffect<'a> {
    pub(crate) action: ir::AttributeAction,
    pub(crate) field: &'a ast::ExprAttribute,
    value: Option<&'a ast::Expr>,
}

impl<'a> AttributeEffect<'a> {
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

fn attribute_opcode(action: ir::AttributeAction, field: &ast::ExprAttribute) -> Builtin {
    Builtin::RegisteredAttribute {
        action,
        name: field.attr.to_string(),
        range: field.range(),
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
fn context_site(source: &str, manager: &ast::Expr, is_async: bool) -> ir::ContextSite {
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

pub(crate) fn attribute_write(stmt: &ast::Stmt) -> Option<AttributeEffect<'_>> {
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
    operation(
        Builtin::RegisteredDeleteItem,
        Type::None,
        vec![receiver, key],
    )
}

pub(super) fn raised<E>(
    raise: &ast::StmtRaise,
    mut operand: impl FnMut(&ast::Expr) -> StdResult<Expr, E>,
) -> StdResult<Statement, E> {
    let exception = raise.exc.as_deref().map(&mut operand).transpose()?;
    let value = match (exception, raise.cause.as_deref()) {
        (Some(value), Some(cause)) => Some(operation(
            Builtin::ExceptionWithCause,
            Type::Exception,
            vec![value, operand(cause)?],
        )),
        (value, _) => value,
    };
    Ok(Statement::Raise(value))
}

impl statements::StatementLowering for SourceScope<'_> {
    type Error = ();

    fn attribute_statements(&mut self, stmt: &ast::Stmt) -> StdResult<Option<Vec<Statement>>, ()> {
        if stmt.is_assign_stmt() {
            return Ok(None);
        }
        let Some(effect) = attribute_write(stmt) else {
            return Ok(None);
        };
        let values = effect
            .operands()
            .map(|node| self.ordinary_operand(node))
            .collect::<Option<_>>()
            .ok_or(())?;
        Ok(Some(vec![attribute_statement(
            effect.action,
            effect.field,
            values,
        )]))
    }

    fn assignment(&mut self, assign: &ast::StmtAssign) -> StdResult<Vec<Statement>, ()> {
        let value = self.frame_value(&assign.value).ok_or(())?;
        self.assignment_targets(&assign.targets, value)
    }

    fn branch(&mut self, branch: &ast::StmtIf) -> StdResult<statements::Block, ()> {
        let mut arms = Vec::new();
        let mut otherwise = (Vec::new(), true);
        let mut falls = false;
        for (test, suite) in if_clauses(branch) {
            if let Some(test) = test {
                let condition = truth(self.operand(test).ok_or(())?);
                let (body, continues) = self.block(suite)?;
                arms.push((condition, body));
                falls |= continues;
            } else {
                otherwise = self.block(suite)?;
            }
        }
        Ok((statements::if_arms(arms, otherwise.0), falls || otherwise.1))
    }

    fn definition(&mut self, def: &ast::StmtFunctionDef) -> StdResult<Option<Statement>, ()> {
        if def.type_params.is_some() {
            let binding = self.operations.get(&def.name.range()).ok_or(())?;
            if matches!(self.kind, RegisteredScope::Class) {
                if !def.decorator_list.is_empty()
                    || def
                        .parameters
                        .iter_non_variadic_params()
                        .any(|p| p.default.is_some())
                {
                    return Err(());
                }
                return Ok(Some(self.store(def.name.as_str(), binding.clone())));
            }
        }
        let decorators = def.decorator_list.iter().map(|d| &d.expression);
        let defaults = def
            .parameters
            .posonlyargs
            .iter()
            .chain(&def.parameters.args)
            .chain(&def.parameters.kwonlyargs)
            .filter_map(|p| p.default.as_deref());
        let header = decorators
            .chain(defaults)
            .map(|e| self.ordinary_operand(e))
            .collect::<Option<_>>()
            .ok_or(())?;
        let function = operation(
            Builtin::RegisteredDefinition(ir::DefinitionSite::Function(def.start())),
            Type::PythonObject,
            header,
        );
        Ok(Some(self.store(def.name.as_str(), function)))
    }

    fn effect(&mut self, value: &ast::Expr) -> StdResult<(Option<Statement>, bool), ()> {
        let effect = if value.is_string_literal_expr() {
            None
        } else {
            Some(Statement::Expr(self.frame_value(value).ok_or(())?))
        };
        Ok((effect, true))
    }

    fn returned(&mut self, ret: &ast::StmtReturn) -> StdResult<Expr, ()> {
        match ret.value.as_deref() {
            Some(value) => self.frame_value(value).ok_or(()),
            None => Ok(Expr::none()),
        }
    }

    fn async_iteration(&self) -> bool {
        matches!(self.kind, RegisteredScope::Function { is_async: true })
    }

    fn for_loop(&mut self, loop_: &ast::StmtFor) -> StdResult<statements::Block, ()> {
        let input = self.iteration_input(&loop_.iter, for_pull(loop_)).ok_or(())?;
        let (binding, body) =
            self.iteration_body(&loop_.target, &input, |scope| scope.suite(&loop_.body))?;
        let otherwise = self.suite(&loop_.orelse)?;
        Ok((input.for_suite(binding, body, otherwise, Vec::new()), true))
    }

    fn other_statement(&mut self, stmt: &ast::Stmt) -> StdResult<statements::Block, ()> {
        match stmt {
            ast::Stmt::AugAssign(assign) => {
                if let Some(name) = assign.target.as_name_expr() {
                    let left = self.ordinary_operand(&assign.target).ok_or(())?;
                    let right = self.ordinary_operand(&assign.value).ok_or(())?;
                    let value = operation(
                        Builtin::RegisteredInPlace(assign.op.as_str()),
                        Type::PythonObject,
                        vec![left, right],
                    );
                    return Ok((vec![self.store(name.id.as_str(), value)], true));
                }
                let target = assign.target.as_subscript_expr().ok_or(())?;
                let receiver = TargetInput::Receiver(&target.value).lower(self).ok_or(())?;
                let key = TargetInput::Index(&target.slice).lower(self).ok_or(())?;
                let rhs = self.ordinary_operand(&assign.value).ok_or(())?;
                Ok((
                    vec![Statement::AugmentItem(
                        receiver,
                        key,
                        ir::ItemUpdate::Protocol(assign.op.as_str()),
                        rhs,
                    )],
                    true,
                ))
            }
            ast::Stmt::TypeAlias(alias) => {
                let value = self.operations.get(&stmt.range()).ok_or(())?.clone();
                let name = alias.name.as_name_expr().ok_or(())?;
                Ok((vec![self.store(name.id.as_str(), value)], true))
            }
            ast::Stmt::Raise(raise) => Ok((
                vec![raised(raise, |value| {
                    self.ordinary_operand(value).ok_or(())
                })?],
                false,
            )),
            ast::Stmt::Match(branch) => self.match_statement(branch),
            ast::Stmt::ClassDef(class) => {
                let mut operands = class
                    .decorator_list
                    .iter()
                    .map(|d| self.ordinary_operand(&d.expression))
                    .collect::<Option<Vec<_>>>()
                    .ok_or(())?;
                let arguments = if class.type_params.is_some() {
                    None
                } else {
                    let mut layout = ir::PackedArguments::default();
                    if let Some(arguments) = &class.arguments {
                        layout = super::packed_arguments(arguments);
                        for operand in super::registered_arguments(arguments) {
                            operands.push(self.ordinary_operand(operand).ok_or(())?);
                        }
                    }
                    Some(layout)
                };
                let site = ir::DefinitionSite::Class {
                    start: class.start(),
                    arguments,
                };
                let value = operation(
                    Builtin::RegisteredDefinition(site),
                    Type::PythonObject,
                    operands,
                );
                Ok((vec![self.store(class.name.as_str(), value)], true))
            }
            ast::Stmt::Try(region) => {
                let body = self.suite(&region.body)?;
                let mut handlers = Vec::with_capacity(region.handlers.len());
                for ast::ExceptHandler::ExceptHandler(handler) in &region.handlers {
                    let matcher = match handler.type_.as_deref() {
                        None => ir::ExceptionMatcher::Categories(Vec::new()),
                        Some(class) => {
                            let operand = Box::new(self.ordinary_operand(class).ok_or(())?);
                            if region.is_star {
                                ir::ExceptionMatcher::Subgroup(handler.range(), operand)
                            } else {
                                ir::ExceptionMatcher::Object(class.range(), operand)
                            }
                        }
                    };
                    handlers.push(ir::ExceptHandler {
                        matcher,
                        name: handler.name.as_ref().map(|name| self.slot(name.as_str())),
                        body: self.suite(&handler.body)?,
                    });
                }
                let orelse = self.suite(&region.orelse)?;
                let finalbody = self.suite(&region.finalbody)?;
                Ok((
                    vec![Statement::Try {
                        body,
                        handlers,
                        orelse,
                        finalbody,
                    }],
                    true,
                ))
            }
            ast::Stmt::With(stmt) if !stmt.is_async || self.async_iteration() => Ok((
                self.with_items(&stmt.items, &self.sites.contexts[&stmt.range()], &stmt.body)?,
                true,
            )),
            ast::Stmt::While(loop_) => {
                let condition = self.while_condition(&loop_.test).ok_or(())?;
                let body = if matches!(&*loop_.test, ast::Expr::BooleanLiteral(value) if !value.value) {
                    Vec::new()
                } else {
                    self.in_loop(LoopScope::While, |scope| scope.suite(&loop_.body))?
                };
                let otherwise = self.suite(&loop_.orelse)?;
                Ok((vec![Statement::While(condition, body, otherwise)], true))
            }
            ast::Stmt::Break(_) if self.loop_scope != LoopScope::Outside => {
                Ok((vec![Statement::Break], false))
            }
            ast::Stmt::Continue(_) if self.loop_scope != LoopScope::Outside => {
                Ok((vec![Statement::Continue], false))
            }
            ast::Stmt::Global(_) | ast::Stmt::Nonlocal(_) => Ok((Vec::new(), true)),
            ast::Stmt::AnnAssign(ann) => {
                let name = ann.target.as_name_expr().ok_or(())?;
                let mut body = Vec::new();
                if let Some(value) = &ann.value {
                    let value = self.operand(value).ok_or(())?;
                    body.push(self.store(name.id.as_str(), value));
                }
                // A function-local annotation has no runtime operand. A simple
                // namespace annotation records its declaration for later evaluation.
                if !matches!(self.kind, RegisteredScope::Function { .. }) && ann.simple {
                    let marker = self.operations.get(&ann.target.range()).ok_or(())?;
                    body.push(Statement::Expr(marker.clone()));
                }
                Ok((body, true))
            }
            ast::Stmt::Delete(delete) => {
                let body = delete_targets(delete)
                    .into_iter()
                    .map(|target| {
                        let (op, inputs) = deletion(target);
                        let values = inputs
                            .map(|input| input.lower(self))
                            .collect::<Option<_>>()?;
                        Some(Statement::Expr(operation(op, Type::None, values)))
                    })
                    .collect::<Option<_>>()
                    .ok_or(())?;
                Ok((body, true))
            }
            _ => self
                .operations
                .get(&stmt.range())
                .cloned()
                .map(|import| (vec![Statement::Expr(import)], true))
                .ok_or(()),
        }
    }
}

#[derive(Clone, Copy)]
enum TargetInput<'a> {
    Receiver(&'a ast::Expr),
    Index(&'a ast::Expr),
}

impl TargetInput<'_> {
    fn lower(self, scope: &mut SourceScope<'_>) -> Option<Expr> {
        match self {
            Self::Receiver(expr) => scope.ordinary_operand(expr),
            Self::Index(key) => {
                values::index_key(key, &mut |expr| scope.ordinary_operand(expr).ok_or(())).ok()
            }
        }
    }

    fn physical(
        self,
        source: values::RegisteredSource<'_>,
        kind: RegisteredScope,
    ) -> PhysicalOperand {
        if let Some(value) =
            RegisteredOperand::prepare_with(source, kind, |scope| self.lower(scope))
        {
            PhysicalOperand::Checked(Box::new(value))
        } else {
            let (Self::Receiver(expr) | Self::Index(expr)) = self;
            PhysicalOperand::Source(Box::new(expr.clone()))
        }
    }
}

fn deletion(target: &ast::Expr) -> (Builtin, impl Iterator<Item = TargetInput<'_>> + Clone) {
    use TargetInput::{Index, Receiver};
    let (op, inputs) = match target {
        ast::Expr::Name(name) => (
            Builtin::RegisteredDeleteName(name.id.to_string(), name.range()),
            [None, None],
        ),
        ast::Expr::Attribute(field) => (
            attribute_opcode(ir::AttributeAction::Delete, field),
            [Some(Receiver(&field.value)), None],
        ),
        ast::Expr::Subscript(sub) => (
            Builtin::RegisteredDeleteItem,
            [Some(Receiver(&sub.value)), Some(Index(&sub.slice))],
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
    sites: &'a StatementSites,
    operations: &'a HashMap<TextRange, Expr>,
    source: values::RegisteredSource<'a>,
    names: HashMap<String, usize>,
    namespaces: HashMap<usize, LocalBinding>,
    comprehension: Option<eager::Namespace>,
    generator_expression: bool,
    locals: Vec<Type>,
    loop_scope: LoopScope,
    suspending_operands: bool,
    refused_comprehension: Option<TextRange>,
}

/// Prepare an inline expression that can require private slots in the enclosing activation.
pub(crate) fn inline_operand(
    expr: &ast::Expr,
    source: values::RegisteredSource<'_>,
    kind: RegisteredScope,
    native_suspension: bool,
) -> StdResult<Option<RegisteredOperand>, (TextRange, &'static str)> {
    match expr {
        ast::Expr::Dict(_) => {
            let mut refusal = (expr.range(), "cannot compile this dictionary display ahead of time");
            let operand = RegisteredOperand::prepare_with(source, kind, |scope| {
                scope.suspending_operands = native_suspension;
                let value = scope.operand(expr);
                if let Some(range) = scope.refused_comprehension {
                    refusal = (range, "cannot compile this comprehension ahead of time");
                }
                value
            });
            operand.map(Some).ok_or(refusal)
        }
        ast::Expr::Named(_)
        | ast::Expr::ListComp(_)
        | ast::Expr::SetComp(_)
        | ast::Expr::DictComp(_)
        | ast::Expr::Generator(_)
        | ast::Expr::FString(_) => Ok(RegisteredOperand::prepare(expr, source, kind)),
        _ => Ok(None),
    }
}

impl unpacking::PatternBinder for SourceScope<'_> {
    type Error = ();

    fn pattern_hidden(&mut self, ty: Type) -> usize {
        self.hidden(ty)
    }

    fn pattern_local(&self, slot: usize) -> Expr {
        Expr::local(slot, self.locals[slot])
    }

    fn pattern_store(&mut self, target: &ast::Expr, value: Expr) -> StdResult<Statement, ()> {
        match target {
            ast::Expr::Name(name) => Ok(self.store(name.id.as_str(), value)),
            ast::Expr::Attribute(attr) => {
                let receiver = self.ordinary_operand(&attr.value).ok_or(())?;
                Ok(attribute_statement(
                    ir::AttributeAction::Store,
                    attr,
                    vec![value, receiver],
                ))
            }
            ast::Expr::Subscript(sub) => {
                let receiver = self.ordinary_operand(&sub.value).ok_or(())?;
                let key =
                    values::index_key(&sub.slice, &mut |e| self.ordinary_operand(e).ok_or(()))?;
                Ok(Statement::SetItem(receiver, key, value))
            }
            _ => Err(()),
        }
    }

    fn multiple_stars(&self, _: &ast::Expr) {}
}

impl<'a> SourceScope<'a> {
    fn new(
        kind: RegisteredScope,
        source: values::RegisteredSource<'a>,
        operations: &'a HashMap<TextRange, Expr>,
        sites: &'a StatementSites,
    ) -> Self {
        Self {
            kind,
            source,
            operations,
            sites,
            names: HashMap::new(),
            namespaces: HashMap::new(),
            comprehension: None,
            generator_expression: false,
            locals: Vec::new(),
            loop_scope: LoopScope::Outside,
            suspending_operands: false,
            refused_comprehension: None,
        }
    }

    fn bindings(self) -> (Vec<Type>, HashMap<usize, LocalBinding>) {
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
        (self.locals, names)
    }

    /// Discard bindings allocated by operands before a later operand failed to lower.
    fn truncate_locals(&mut self, len: usize) {
        if self.locals.len() != len {
            self.locals.truncate(len);
            self.names.retain(|_, slot| *slot < len);
            self.namespaces.retain(|slot, _| *slot < len);
        }
    }

    fn finish(self, body: Vec<Statement>) -> RegisteredFunction {
        let (locals, local_names) = self.bindings();
        RegisteredFunction {
            function: ir::Function {
                params: Vec::new(),
                returns: Type::PythonObject,
                locals,
                body,
            },
            local_names,
        }
    }
}

/// Which control transfers a checked region can handle inside a physical frame.
#[derive(Clone, Copy, PartialEq, Eq)]
enum PhysicalRegion {
    Fallthrough,
    Loop,
    Protected,
}

impl PhysicalRegion {
    fn accepts(self, body: &[Statement]) -> bool {
        let mut self_contained = true;
        ir::walk_nodes(body, &mut |node, _| match node {
            // Named definitions in loops and protected regions still need their source headers.
            ir::Node::Expression(Expr {
                kind: ExprKind::Builtin(Builtin::RegisteredDefinition(_), _),
                ..
            }) if self != Self::Fallthrough => self_contained = false,
            ir::Node::Statement(Statement::Break | Statement::Continue | Statement::Exhaust)
                if self != Self::Loop =>
            {
                self_contained = false;
            }
            ir::Node::Statement(Statement::Return(_) | Statement::Raise(_))
                if self != Self::Protected =>
            {
                self_contained = false;
            }
            ir::Node::Statement(Statement::IterNext {
                kind: ir::IterNextKind::Async(_),
                ..
            })
            | ir::Node::Statement(Statement::With {
                site: ir::ContextSite { is_async: true, .. },
                ..
            })
            | ir::Node::Expression(Expr {
                kind: ExprKind::Builtin(Builtin::RegisteredSuspend(..), _),
                ..
            }) => self_contained = false,
            _ => {}
        });
        self_contained
    }
}

impl SourceScope<'_> {
    fn assignment_targets(
        &mut self,
        targets: &[ast::Expr],
        value: Expr,
    ) -> StdResult<Vec<Statement>, ()> {
        self.with_assignment_operands(|scope| {
            unpacking::bind_assignment(scope, targets, value)
        })
    }

    fn with_assignment_operands<T>(&mut self, lower: impl FnOnce(&mut Self) -> T) -> T {
        if self.loop_scope == LoopScope::WithCursor
            && matches!(self.kind, RegisteredScope::Function { is_async: true })
        {
            return lower(self);
        }
        self.with_suspending_operands(lower)
    }

    fn physical_statement(
        &mut self,
        stmt: &ast::Stmt,
        body: &mut Vec<Statement>,
    ) -> StdResult<bool, ()> {
        if let ast::Stmt::Assign(assign) = stmt {
            // The physical frame owns an assignment that yields or awaits its value.
            if ir::SuspendValue::operand(&assign.value).is_some() {
                return Err(());
            }
            return self.statement(stmt, body);
        }
        if stmt.is_return_stmt()
            || stmt.is_expr_stmt()
            || stmt.is_delete_stmt()
            || stmt.is_aug_assign_stmt()
            || stmt.is_raise_stmt()
            || self.sites.assertions.contains_key(&stmt.range())
            || attribute_write(stmt).is_some()
        {
            return self.statement(stmt, body);
        }
        if !matches!(self.kind, RegisteredScope::Function { .. }) {
            return Err(());
        }
        let region = match stmt {
            ast::Stmt::If(_) => PhysicalRegion::Fallthrough,
            ast::Stmt::While(_) => PhysicalRegion::Loop,
            ast::Stmt::For(loop_) if !loop_.is_async => PhysicalRegion::Loop,
            ast::Stmt::Try(region) if !region.is_star => PhysicalRegion::Protected,
            _ => return Err(()),
        };
        let begin = body.len();
        let continues = self.statement(stmt, body)?;
        if !region.accepts(&body[begin..]) {
            body.truncate(begin);
            return Err(());
        }
        Ok(continues)
    }

    fn statement(&mut self, stmt: &ast::Stmt, body: &mut Vec<Statement>) -> StdResult<bool, ()> {
        if let ast::Stmt::Assert(assertion) = stmt {
            if let Some(statements) = self.sites.assertions.get(&stmt.range()) {
                body.extend_from_slice(statements);
            } else {
                // A prepared activation can also supply storage for named expressions
                // and admit suspension at a whole operand's boundary.
                body.extend(checked_assertion(assertion, |expr| self.operand(expr)).ok_or(())?);
            }
            return Ok(true);
        }
        let lower = |scope: &mut Self| {
            statements::StatementLowering::source_statement(
                scope,
                stmt,
                matches!(scope.kind, RegisteredScope::Module),
            )
        };
        let (lowered, continues) = if stmt.is_aug_assign_stmt() {
            self.with_suspending_operands(lower)
        } else {
            lower(self)
        }?;
        body.push(checkpoint(stmt));
        body.extend(lowered);
        Ok(continues)
    }

    fn suite(&mut self, stmts: &[ast::Stmt]) -> StdResult<Vec<Statement>, ()> {
        self.block(stmts).map(|(body, _)| body)
    }

    fn with_binding(&mut self, target: &ast::Expr) -> StdResult<ir::ContextBinding, ()> {
        let slot = self.hidden(Type::PythonObject);
        let value = Expr::local(slot, Type::PythonObject);
        let body = if pattern_elements(target).is_some() {
            unpacking::bind_targets(self, target, value, None)?
        } else {
            self.assignment_targets(std::slice::from_ref(target), value)?
        };
        Ok(ir::ContextBinding {
            entered: slot,
            body: vec![Statement::Scope {
                body,
                clear: vec![slot],
            }],
        })
    }

    fn with_items(
        &mut self,
        items: &[ast::WithItem],
        inputs: &[ContextInput],
        suite: &[ast::Stmt],
    ) -> StdResult<Vec<Statement>, ()> {
        let Some((item, rest)) = items.split_first() else {
            return self.suite(suite);
        };
        let manager = self.ordinary_operand(&item.context_expr).ok_or(())?;
        let (input, remaining) = inputs.split_first().expect("with input");
        let binding = item
            .optional_vars
            .as_ref()
            .map(|target| self.with_binding(target))
            .transpose()?;
        let body = self.with_items(rest, remaining, suite)?;
        Ok(vec![Statement::With {
            manager,
            site: input.site,
            binding,
            body,
        }])
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

    fn hidden(&mut self, ty: Type) -> usize {
        let slot = self.locals.len();
        self.locals.push(ty);
        slot
    }

    fn match_statement(&mut self, branch: &ast::StmtMatch) -> StdResult<statements::Block, ()> {
        let mut cases = Vec::with_capacity(branch.cases.len());
        let mut falls = false;
        let mut exhaustive = false;
        let patterns = &self
            .sites
            .matches
            .get(&branch.range())
            .and_then(Option::as_ref)
            .ok_or(())?
            .cases;
        for (case, planned) in branch.cases.iter().zip(patterns) {
            if exhaustive {
                return Err(());
            }
            let pattern = planned.pattern.clone();
            let captures = pattern.names.iter().map(|name| self.slot(name)).collect();
            let guard = case
                .guard
                .as_deref()
                .map(|value| self.operand(value).map(truth).ok_or(()))
                .transpose()?;
            exhaustive = guard.is_none() && case.pattern.is_irrefutable();
            let (body, continues) = self.block(&case.body)?;
            cases.push(ir::MatchCase {
                pattern,
                captures,
                guard,
                body,
            });
            falls |= continues;
        }
        // Only a complete match owns its subject in this suite's slot layout.
        let subject = self.frame_value(&branch.subject).ok_or(())?;
        Ok((vec![Statement::Match(subject, cases)], falls || !exhaustive))
    }

    fn block(&mut self, stmts: &[ast::Stmt]) -> StdResult<statements::Block, ()> {
        let mut body = Vec::new();
        let mut falls = true;
        for stmt in stmts {
            falls &= self.statement(stmt, &mut body)?;
        }
        Ok((body, falls))
    }

    fn slot(&mut self, name: &str) -> usize {
        if let Some(slot) = self.names.get(name) {
            return *slot;
        }
        let slot = self.hidden(Type::PythonObject);
        self.names.insert(name.to_owned(), slot);
        slot
    }

    fn store(&mut self, name: &str, value: Expr) -> Statement {
        if let Some(slot) = self
            .comprehension
            .as_ref()
            .and_then(|scope| scope.targets.get(name))
        {
            return Statement::Assign(*slot, value);
        }
        let slot = self.slot(name);
        if matches!(self.kind, RegisteredScope::Module) {
            Statement::Expr(operation(Builtin::GlobalSet(slot), Type::None, vec![value]))
        } else {
            Statement::Assign(slot, value)
        }
    }

    fn iteration_body(
        &mut self,
        target: &ast::Expr,
        input: &pipelines::Prepared,
        body: impl FnOnce(&mut Self) -> StdResult<Vec<Statement>, ()>,
    ) -> StdResult<(Vec<Statement>, Vec<Statement>), ()> {
        let binding = unpacking::bind_targets(
            self,
            target,
            Expr::local(input.value, Type::PythonObject),
            None,
        )?;
        let body = self.in_loop(LoopScope::WithCursor, body)?;
        Ok((binding, body))
    }

    fn iteration_input(
        &mut self,
        iterable: &ast::Expr,
        pull: Option<ir::AsyncPull>,
    ) -> Option<pipelines::Prepared> {
        let value = self.ordinary_operand(iterable)?;
        let (acquire, kind) = iteration_protocol(iterable.range(), pull);
        let cursor = operation(Builtin::RegisteredIter(acquire), Type::PythonObject, vec![value]);
        Some(pipelines::Prepared::pull_cursor(
            cursor,
            Type::PythonObject,
            kind,
            |ty| self.hidden(ty),
        ))
    }

    /// The activation owns the complete value before a return, discard or store.
    /// Its temporary roots keep earlier arguments live across a later suspension.
    fn frame_value(&mut self, expr: &ast::Expr) -> Option<Expr> {
        if matches!(self.kind, RegisteredScope::Function { .. }) {
            self.with_assignment_operands(|scope| scope.operand(expr))
        } else {
            self.operand(expr)
        }
    }

    fn operand(&mut self, expr: &ast::Expr) -> Option<Expr> {
        self.lower_operand(expr, true)
    }

    fn while_condition(&mut self, test: &ast::Expr) -> Option<Expr> {
        self.with_suspending_operands(|scope| scope.operand(test))
    }

    fn with_suspending_operands<T>(&mut self, lower: impl FnOnce(&mut Self) -> T) -> T {
        let previous = std::mem::replace(&mut self.suspending_operands, true);
        let value = lower(self);
        self.suspending_operands = previous;
        value
    }

    fn lambda_binding(&mut self, lambda: &ast::ExprLambda) -> Option<Expr> {
        let defaults = values::lambda_defaults(lambda)
            .map(|default| self.ordinary_operand(default))
            .collect::<Option<_>>()?;
        Some(values::lambda_binding(lambda, defaults))
    }

    fn ordinary_operand(&mut self, expr: &ast::Expr) -> Option<Expr> {
        self.lower_operand(expr, false)
    }

    fn lower_operand(&mut self, expr: &ast::Expr, whole: bool) -> Option<Expr> {
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
            )
        {
            self.suspend_operand(kind, value, expr.range())?
        } else if let ast::Expr::TString(template) = expr {
            self.with_suspending_operands(|scope| {
                scope.source.template(
                    template,
                    matches!(
                        scope.kind,
                        RegisteredScope::Function { .. } | RegisteredScope::Lambda
                    ),
                    |child| scope.ordinary_operand(child),
                )
            })?
        } else if let ast::Expr::FString(formatted) = expr {
            self.with_suspending_operands(|scope| {
                fstrings::registered_fstring(formatted, scope.source, |child| {
                    scope.ordinary_operand(child).ok_or(())
                })
            })
            .ok()?
        } else if matches!(expr, ast::Expr::ListComp(_) | ast::Expr::SetComp(_) | ast::Expr::DictComp(_)) {
            let value = self.eager_comprehension(expr);
            if value.is_none() {
                self.refused_comprehension = Some(expr.range());
            }
            value?
        } else if (self.comprehension.is_some() || self.generator_expression)
            && matches!(self.kind, RegisteredScope::Function { is_async: true })
            && let ast::Expr::Await(awaited) = expr
        {
            self.suspend_operand(ir::SuspendValue::Await, Some(&awaited.value), expr.range())?
        } else if let ast::Expr::Lambda(lambda) = expr {
            if !values::has_checked_lambda_body(lambda, self.source) {
                return None;
            }
            let binding = self.lambda_binding(lambda)?;
            // A generator expression retains its own clause cursors across an awaited default.
            if self.loop_scope != LoopScope::Outside
                && !self.generator_expression
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
                self.with_assignment_operands(|scope| scope.lower_operand(&named.value, whole))?;
            Expr {
                ty: value.ty,
                kind: ExprKind::Bind(self.slot(&target.id), Box::new(value)),
            }
        } else {
            values::operation(
                expr,
                self.source,
                None,
                matches!(self.kind, RegisteredScope::Class),
                |_| true,
                |child| {
                    match expr {
                        ast::Expr::Generator(_) => {
                            self.creating_iterable(|scope| scope.ordinary_operand(child))
                        }
                        // The dictionary retains earlier operands while a key, value or spread suspends.
                        ast::Expr::Dict(_) => self.operand(child),
                        ast::Expr::Set(_) | ast::Expr::Subscript(_) if child.is_await_expr() => {
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
        Some(self.source.located(expr, value))
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
        Some(operation(
            Builtin::RegisteredSuspend(kind, range),
            Type::PythonObject,
            vec![value],
        ))
    }
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

    fn prepared_sites<'a>(
        stmts: &'a [ast::Stmt],
        source: values::RegisteredSource<'_>,
        bundled: bool,
    ) -> RegisteredSites<'a> {
        let default_names = imre_ast::Names::default();
        let names = source.private_names.unwrap_or(&default_names);
        let strings = annotation_operands(
            stmts,
            names.string_annotations,
            source,
            bundled,
            crate::frontend::annotation_operands::test_annotation_body,
            &mut HashMap::new(),
        );
        registered_sites(stmts, names, &strings)
    }

    pub(super) fn registered_suite(
        stmts: &[ast::Stmt],
        kind: RegisteredScope,
        source: values::RegisteredSource<'_>,
    ) -> Option<RegisteredFunction> {
        let operations = prepared_sites(stmts, source, false).operations;
        SuitePreparation::new(&operations, source, stmts, true)
            .complete(stmts, kind)
            .map(|plan| plan.checked)
    }

    #[test]
    fn generic_method_closure_uses_ordinary_function_sites() {
        let source = "class Box:\n    def capture[T](self, __value: T):\n        def get():\n            return __value\n        return get\n";
        let mut body = ruff_python_parser::parse_module(source)
            .unwrap()
            .into_syntax()
            .body;
        let names = imre_ast::lower(&mut body);
        let sites = registered_sites::<()>(&body, &names, &HashMap::new());
        let [method, closure] = sites.functions.as_slice() else {
            panic!("expected the method and its lexical function");
        };
        assert!(method.prepare_entry && closure.prepare_entry);
        assert!(sites.operations.contains_key(&method.def.name.range()));
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
            template_fields: None,
            generators: None,
            lambdas: None,
            text,
            private_names: None,
            method_imports: &imre_ast::module_imports(body),
            replacements: &HashMap::new(),
            has_templates: false,
        };
        let stmts = std::slice::from_ref(stmt);
        let operations = HashMap::new();
        let mut preparation = SuitePreparation::new(&operations, source, stmts, true);
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
    fn assertion_admission_keeps_the_message_on_the_failure_edge() {
        let text = "assert (lost := probe()), f'{(item for item in values)}'\nassert (selected := [value][0]), [message(), values[1:]]\n";
        let graph = SourceGraph::single(text).unwrap();
        let module = &graph.modules[graph.entry];
        let body = &module.parsed.syntax().body;
        let literals = values::ModuleSource::new(module);
        let operations = HashMap::new();
        let prepare = |enabled| {
            SuitePreparation::new(&operations, literals.context(text), body, enabled)
                .suite(body, RegisteredScope::Function { is_async: false })
        };
        let plan = prepare(true);
        assert_eq!(plan.checked.local_names.len(), 1);
        assert_eq!(plan.checked.local_names[&0].name, "selected");
        let [physical] = plan.residual.as_slice() else {
            panic!("expected the generator-expression message to remain physical");
        };
        assert_eq!(physical.at, 0);
        assert_eq!(physical.statements, 0..1);
        let [_, Statement::If(test, succeeds, fails)] = plan.checked.function.body.as_slice()
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
        let (exception, cause) = value.explicit_cause().unwrap();
        for operand in [exception, cause] {
            let mut calls = 0;
            ir::walk_operand_expressions(operand, &mut |value| {
                calls += usize::from(value.is_registered_invoke());
            });
            assert_eq!(calls, 1);
        }
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
        let provider = &sites.parameter_annotations[&def.parameters.start()];
        assert!(provider.body.fully_checked());
        let fields: Vec<_> = provider
            .entries
            .iter()
            .map(|entry| &sites.annotations[&entry.site])
            .collect();
        let names: Vec<_> = fields.iter().map(|field| field.name.as_str()).collect();
        assert_eq!(names, ["b", "a", "args", "c", "kw", "return"]);
        assert!(matches!(
            &fields[0].value,
            AnnotationValue::Checked(Expr {
                kind: ExprKind::Builtin(Builtin::BindAnnotationExpression(site), _), ..
            }) if *site == fields[0].range.start()
        ));
        assert_eq!(provider.references["a"], "mod.T");
        assert_eq!(provider.references["return"], "None");
        assert!(!provider.references.contains_key("b"));
    }

    #[test]
    fn annotation_callback_numbers_physical_occurrences_and_keeps_unsupported_values_late() {
        let text = "first: int\nsecond: list[int]\ncomplex: dict[str, int]\nforward: 'list[int]'\nprepared: (lambda: int)()\ndeferred: next(t for t in (int,))\nquoted: '(lambda: Child)()'\n";
        let graph = SourceGraph::single(text).unwrap();
        let module = &graph.modules[graph.entry];
        let stmts = &module.parsed.syntax().body;
        let literals = values::ModuleSource::new(module);
        let source = literals.context(text);
        let prepared = |bundled| prepared_sites(stmts, source, bundled).annotations;
        let sites = prepared(false);
        let callback = |occurrences: &[usize]| {
            let entries: Vec<_> = occurrences
                .iter()
                .enumerate()
                .map(|(index, slot)| ir::AnnotationOccurrence {
                    site: stmts[*slot].start(),
                    index: Some(index),
                })
                .collect();
            DeferredAnnotation::prepare_body(&sites, &entries)
        };
        let whole = callback(&[0, 1, 0]);
        assert!(whole.fully_checked());
        let checked = &whole.checked;
        let [
            Statement::Expr(start),
            members @ ..,
            Statement::Return(result),
        ] = checked.function.body.as_slice()
        else {
            panic!("expected a single callback body")
        };
        assert!(matches!(
            start.kind,
            ExprKind::Builtin(Builtin::RegisteredAnnotationStart, _)
        ));
        assert!(matches!(
            result.kind,
            ExprKind::Builtin(Builtin::RegisteredAnnotationResult, _)
        ));
        let numbered: Vec<_> =
            members
                .iter()
                .map(|stmt| {
                    let Statement::Expr(Expr {
                        kind:
                            ExprKind::Builtin(
                                Builtin::RegisteredAnnotationInsert(occurrence),
                                operands,
                            ),
                        ..
                    }) = stmt
                    else {
                        panic!("expected a guarded annotation insertion")
                    };
                    assert_eq!(operands.len(), 1);
                    (occurrence.index, sites[&occurrence.site].name.as_str())
                })
                .collect();
        assert_eq!(
            numbered,
            [(Some(0), "first"), (Some(1), "second"), (Some(2), "first")]
        );
        let bundled_sites = prepared(true);
        let quoted_site = stmts[3].start();
        assert!(
            matches!(&sites[&quoted_site].value, AnnotationValue::Checked(Expr { kind: ExprKind::Builtin(Builtin::BindAnnotationExpression(site), _), .. }) if *site == stmts[3].as_ann_assign_stmt().unwrap().annotation.start())
        );
        assert!(
            matches!(&bundled_sites[&quoted_site].value, AnnotationValue::Checked(Expr {kind: ExprKind::Str(text), ..}) if text == "list[int]")
        );
        let forward = callback(&[3]);
        assert!(forward.fully_checked());
        assert!(matches!(&forward.checked.function.body[1],
            Statement::Expr(Expr { kind: ExprKind::Builtin(
                Builtin::RegisteredAnnotationInsert(ir::AnnotationOccurrence { index: Some(0), .. }), values), .. })
                if matches!(values.as_slice(), [Expr { kind: ExprKind::Builtin(
                    Builtin::BindAnnotationExpression(site), _), .. }] if *site == stmts[3].as_ann_assign_stmt().unwrap().annotation.start())));
        {
            assert!(matches!(
                sites[&stmts[2].start()].value,
                AnnotationValue::Checked(_)
            ));
            assert!(matches!(
                sites[&stmts[4].start()].value,
                AnnotationValue::Checked(_)
            ));
            let unavailable = &sites[&stmts[5].start()];
            assert!(matches!(unavailable.value, AnnotationValue::Source(_)));
            assert!(matches!(
                sites[&stmts[6].start()].value,
                AnnotationValue::Checked(_)
            ));
            let mixed = callback(&[0, 5, 1]);
            let [gap] = mixed.residual.as_slice() else {
                panic!("expected one unprepared operand")
            };
            assert_eq!(gap.at, 2);
            assert_eq!(gap.statements, 1..2);
            assert_eq!(mixed.checked.function.body.len(), 4);
        }
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
            template_fields: None,
            generators: None,
            lambdas: None,
            text,
            private_names: Some(&names),
            replacements: &replacements,
            has_templates: false,
            method_imports: &imports,
        };
        let operands = prepared_sites(&stmts, source, false);
        let ops = operands.operations;
        let annotations = operands.annotations;
        assert_eq!(annotations.len(), 2);
        let mut prepare = SuitePreparation::new(&ops, source, &stmts, true);
        let plan = prepare.suite(&stmts, RegisteredScope::Module);
        let [run] = &plan.residual[..] else {
            panic!("expected one source range")
        };
        // Only the annotated list assignment remains in the source run. The branch
        // now owns the delete operation, and the last import still follows the run.
        assert_eq!(run.statements, 3..4);
        let (before, after) = plan.checked.function.body.split_at(run.at);
        let Some(Statement::If(_, yes, no)) = before.last() else {
            panic!("expected the checked conditional immediately before y")
        };
        assert!(no.is_empty());
        assert!(matches!(yes.as_slice(), [_, Statement::Expr(Expr {
            kind: ExprKind::Builtin(Builtin::RegisteredDeleteName(name, _), _), ..
        })] if name == "a"));
        assert!(matches!(
            after,
            [
                _,
                Statement::Expr(Expr {
                    kind: ExprKind::Builtin(Builtin::RegisteredImport(_), _),
                    ..
                })
            ]
        ));
        assert!(prepare.nested.is_empty());
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
        let mut counts = [0; 6];
        ir::walk_expressions(body, &mut |expr| match &expr.kind {
            ExprKind::Conditional(..) => counts[0] += 1,
            ExprKind::Boolean(..) => counts[1] += 1,
            ExprKind::BooleanValue(..) => counts[2] += 1,
            ExprKind::Builtin(Builtin::RegisteredName(name), _) if name == "bool" => counts[3] += 1,
            ExprKind::Builtin(Builtin::RegisteredCall(..), _) => counts[4] += 1,
            ExprKind::Invoke { .. } => counts[5] += 1,
            _ => {}
        });
        assert_eq!(counts, [1, 1, 2, 1, 0, 3]);
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
            Statement::SetItem(receiver, key, rhs),
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
            matches!(&deletion.kind, ExprKind::Builtin(Builtin::RegisteredDeleteItem, inputs) if inputs.len()==2)
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
        let import = registered_import(&class.body[0], &names).unwrap();
        let ExprKind::Builtin(Builtin::RegisteredImport(ir::NamespaceImport::Modules(modules)), _) =
            import.kind
        else {
            panic!("expected an import operation");
        };
        let [module] = modules.as_slice() else {
            panic!("expected one module")
        };
        assert_eq!(module.name, "__pkg.child");
        assert_eq!(module.binding, "_C__pkg");
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
            ExprKind::Builtin(Builtin::RegisteredCall(call), operands) => match &call.target {
                ir::RegisteredCallTarget::Method { name, .. } => {
                    assert_eq!(call.arguments, ir::RegisteredArguments::Positional);
                    assert_eq!(operands.len(), 2);
                    calls.push(name.as_str());
                }
                ir::RegisteredCallTarget::Callable { .. } => {
                    assert_eq!(operands.len(), 3);
                    assert_eq!(
                        call.arguments,
                        ir::RegisteredArguments::Packed(ir::PackedArguments {
                            starred: vec![true],
                            keywords: vec![None],
                        })
                    );
                    calls.push("packed");
                }
            },
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
    fn inline_bind_uses_the_assigning_frame() {
        let prepare = |expr: &str, is_async| {
            let prefix = if is_async { "async " } else { "" };
            let text = format!("{prefix}def run(value):\n    return {expr}\n");
            let graph = SourceGraph::single(&text).unwrap();
            let module = &graph.modules[graph.entry];
            let def = module.parsed.syntax().body[0].as_function_def_stmt().unwrap();
            let expr = def.body[0].as_return_stmt().unwrap().value.as_deref().unwrap();
            let literals = values::ModuleSource::new(module);
            inline_operand(expr, literals.context(&text), RegisteredScope::Function { is_async }, true)
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
        assert!(operand.local_names.values().any(|local| local.name == "bound"));
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
            inline_operand(value, source, RegisteredScope::Function { is_async }, suspended)
        };
        assert!(prepare(false, true).is_err());
        assert!(prepare(true, false).is_err());
        let operand = prepare(true, true).unwrap().unwrap();
        crate::verify::registered(operand.region()).unwrap();
        assert!(operand.local_names.values().any(|binding| binding.name == "saved"));
        let mut awaits = Vec::new();
        ir::walk_operand_expressions(&operand.value, &mut |value| {
            if let ExprKind::Builtin(Builtin::RegisteredSuspend(ir::SuspendValue::Await, range), _) =
                &value.kind
            {
                awaits.push(&text[*range]);
            }
        });
        assert_eq!(awaits, ["await first", "await second", "await nested", "await mapping"]);
    }

    #[test]
    fn eager_first_iterable_can_delegate_before_the_cursor_exists() {
        let text = "def gather():\n    return [value for value in ((yield from rows) or ())]\n";
        let graph = SourceGraph::single(text).unwrap();
        let module = &graph.modules[graph.entry];
        let function = module.parsed.syntax().body[0].as_function_def_stmt().unwrap();
        let expr = function.body[0].as_return_stmt().unwrap().value.as_deref().unwrap();
        let literals = values::ModuleSource::new(module);
        let source = literals.context(text);
        let prepare = |kind, suspended| inline_operand(expr, source, kind, suspended).unwrap();
        let value = prepare(RegisteredScope::Function { is_async: false }, true)
            .expect("the creating generator owns the outer yield");
        crate::verify::registered(value.region()).unwrap();
        assert!(prepare(RegisteredScope::Module, false).is_none());
        assert!(prepare(RegisteredScope::Function { is_async: true }, true).is_none());
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
            let [Statement::Expr(_), store] = suite.function.body.as_slice() else {
                panic!("expected one checkpoint followed by one store")
            };
            let operand = match (module, store) {
                (false, Statement::Assign(0, value)) => value,
                (
                    true,
                    Statement::Expr(Expr {
                        kind: ExprKind::Builtin(Builtin::GlobalSet(0), operands),
                        ..
                    }),
                ) => &operands[0],
                _ => panic!("expected the destination's namespace store"),
            };
            let ExprKind::Builtin(Builtin::RegisteredLocation(site, _), operands) = &operand.kind
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
            let ExprKind::Builtin(Builtin::RegisteredDict(entries), elements) = &display[0].kind
            else {
                panic!("expected the registered Python dictionary protocol")
            };
            assert_eq!(
                entries,
                &[ir::DictEntryKind::Pair, ir::DictEntryKind::Unpack]
            );
            assert_eq!(elements.len(), 3);
            let mut names = Vec::new();
            ir::walk_expressions(&suite.function.body, &mut |expr| {
                if let ExprKind::Builtin(Builtin::RegisteredName(name), _) = &expr.kind {
                    names.push(name.clone());
                }
            });
            assert_eq!(names, ["key", "callback", "arg", "more", "index"]);
        }
    }

    #[test]
    fn an_imported_checked_callback_keeps_its_registered_body() {
        let project = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/registered_imported_callback");
        let entry = project.join("callable_effect.py");
        let mut graph = SourceGraph::load(&entry, Some(&project), &[]).unwrap();
        let error = crate::initializer_call_error(&graph, false);
        let (result, lowered) = caller_cache::observe_lowerings(|| {
            crate::checked_lower_with(&mut graph, None, error.as_ref()).unwrap()
        });
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
    }

    #[test]
    fn recovery_reuses_the_solved_body_and_lowers_the_missing_callback() {
        let graph = SourceGraph::single(concat!(
            "def square(n: int) -> int:\n    return n * n\n",
            "def publish(send) -> None:\n    send(1)\n",
        ))
        .unwrap();
        let module = graph.entry;
        let definitions = &graph.modules[module].parsed.syntax().body;
        let types = Types::default();
        let classes = ClassInfo::new(&graph, &types).unwrap();
        let inference = inference::Inference::default();
        let mut signatures = HashMap::new();
        for (statement, definition) in definitions.iter().enumerate() {
            let signature = signature(
                definition.as_function_def_stmt().unwrap(),
                &graph,
                keyword_forwarding::Defaults::local(module),
                &classes.declarations,
                &types,
                None,
                &inference,
            )
            .unwrap();
            signatures.insert(DefinitionId { module, statement }, signature);
        }
        let globals = ModuleValues::default();
        let capture_bodies = |live| {
            caller_cache::observe_lowerings(|| {
                capture(
                    &graph,
                    &signatures,
                    &classes,
                    &types,
                    &inference,
                    &globals,
                    if live {
                        Capture::IncludeLive
                    } else {
                        Capture::Solved
                    },
                )
            })
            .1
        };

        assert_eq!(capture_bodies(false), ["square"]);
        assert!(graph.registered_functions()[&module].contains_key(&definitions[0].start()));
        assert_eq!(capture_bodies(true), ["publish"]);
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
            ExprKind::Builtin(Builtin::RegisteredCall(_), _) | ExprKind::Invoke { .. } => calls += 1,
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
        let error = crate::initializer_call_error(&graph, false);
        let (result, lowered) = caller_cache::observe_lowerings(|| {
            crate::checked_lower_with(&mut graph, None, error.as_ref()).unwrap()
        });
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
        let file = ir
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(" = private unnamed_addr constant ")?;
                value.ends_with(r#"__main__.py\00""#).then_some(name)
            })
            .unwrap();
        let statement = format!("ptr {file}, i32 2, i64 0)");
        let bump = ir
            .split("\ndefine internal i64 @code")
            .skip(1)
            .filter_map(|code| code.split_once("\n}").map(|(body, _)| body))
            .find(|body| body.contains(&statement))
            .unwrap();
        assert!(bump.contains("call i64 @int_add("));
        assert!(bump.contains("call i64 @imre_dyn_exact_int_word("));
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
        ir::walk_expressions(&body.body, &mut |expr| match expr.kind {
            ExprKind::Builtin(Builtin::RegisteredStatement(range, _), _) => {
                checkpoints.push(&graph.modules[graph.entry].source[range]);
            }
            ExprKind::Binary(Binary::Add, ..) => checked_adds += 1,
            ExprKind::Invoke { ref args, .. } => {
                let site = args.python_site.as_ref().unwrap();
                let source = &graph.modules[graph.entry].source;
                assert_eq!(&source[site.range], "sink(answer)");
                let name = site.suspended_name.unwrap();
                assert_eq!(&source[name], "sink");
                assert_eq!(name.end(), site.callee_end);
                assert_eq!(args.order, [0]);
                assert_eq!(expr.ty, Type::PythonObject);
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
            "def apply(factory, obj, value):\n",
            "    factory()(value)\n",
            "    (obj.action if value else factory)(value)\n",
            "    obj.action(value)\n",
            "    factory(*value)\n",
        );
        let graph = SourceGraph::single(source).unwrap();
        let _ = super::super::lower(&graph);
        let functions = graph.registered_functions();
        let checked = &functions[&graph.entry][&TextSize::default()];
        assert!(crate::llvm::registered::supports(checked));
        let mut calls = Vec::new();
        ir::walk_expressions(&checked.function.body, &mut |expr| match &expr.kind {
            ExprKind::Invoke { callable, args } => {
                let site = args.python_site.as_ref().unwrap();
                let ExprKind::Builtin(Builtin::RegisteredLocation(range, _), callee) =
                    &callable.kind
                else {
                    panic!("expected the checked callable operand");
                };
                assert_eq!(site.callee_end, range.end());
                let kind = match &callee[0].kind {
                    ExprKind::Local(_) => "local",
                    ExprKind::Invoke { .. } => "result",
                    ExprKind::Conditional(..) => "conditional",
                    _ => panic!("unexpected positional callable"),
                };
                calls.push((&source[site.range], kind));
            }
            ExprKind::Builtin(Builtin::RegisteredCall(site), _) => {
                let kind = match (&site.target, &site.arguments) {
                    (ir::RegisteredCallTarget::Method { .. }, ir::RegisteredArguments::Positional) => {
                        "method"
                    }
                    (_, ir::RegisteredArguments::Packed(_)) => "packed",
                    _ => panic!("unexpected registered call"),
                };
                calls.push((&source[site.range], kind));
            }
            _ => {}
        });
        assert_eq!(
            calls,
            [
                ("factory()(value)", "result"),
                ("factory()", "local"),
                ("(obj.action if value else factory)(value)", "conditional"),
                ("obj.action(value)", "method"),
                ("factory(*value)", "packed"),
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
            ExprKind::Builtin(Builtin::RegisteredDict(entries), _) => {
                assert_eq!(entries, &[ir::DictEntryKind::Pair]);
                operations.push("dict");
            }
            ExprKind::Builtin(Builtin::RegisteredName(name), _) if name == "KEY" => {
                operations.push("name")
            }
            ExprKind::Builtin(Builtin::RegisteredSubscript(..), _) => operations.push("get"),
            ExprKind::Invoke { callable, args } => {
                let ExprKind::Builtin(Builtin::RegisteredLocation(_, _), operands) = &callable.kind
                else {
                    panic!("expected the located callback");
                };
                assert!(matches!(
                    operands.as_slice(),
                    [Expr { kind: ExprKind::Builtin(Builtin::RegisteredSubscript(..), _), .. }]
                ));
                assert_eq!(args.values.len(), 1);
                assert_eq!(args.order, [0]);
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
        assert_eq!(operations, [
            ("*", Type::Int, Type::Int, Type::Int),
            ("+", Type::Int, Type::Bool, Type::Int),
            ("**", Type::Int, Type::Bool, Type::Int),
        ]);
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
            if let ExprKind::Builtin(
                Builtin::RegisteredCall(ir::RegisteredCall {
                    target: ir::RegisteredCallTarget::Method { name, .. },
                    ..
                }),
                _,
            ) = &expr.kind
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
        let graph = SourceGraph::single(positive).unwrap();
        let selected = super::super::lower_current(&graph, None, &mut HashSet::new());
        match selected {
            Ok(crate::CheckedProgram::Registered(None)) => {}
            Err(error) | Ok(crate::CheckedProgram::Runtime(error)) => {
                assert!(
                    matches!(error.kind, Kind::UnresolvedParameter(owner, start)
                    if owner == graph.entry && start == TextSize::default()),
                    "{error}"
                );
            }
            _ => panic!("expected a registered Python callable"),
        }
        graph
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
            if let ExprKind::Builtin(Builtin::RegisteredUnary(op), args) = &e.kind {
                assert!(matches!(args.as_slice(), [value] if value.ty == Type::PythonObject));
                ops.push(*op);
            }
        });
        use ir::ObjectUnary::{Invert, Negative, Not, Positive};
        assert_eq!(ops, [Positive, Negative, Invert, Not]);
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
            Statement::SetItem(_, _, last),
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
                assert!(matches!(body.last(), Some(Statement::Break)));
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
    fn context_sites_are_prepared_for_every_manager_before_lowering() {
        let text = "async def capture():\n    async with manager:\n        pass\nwith λ[0], install(2):\n    pass\n";
        let graph = SourceGraph::single(text).unwrap();
        let module = &graph.modules[graph.entry];
        let sites = statement_sites(
            &module.parsed.syntax().body,
            values::ModuleSource::new(module).context(text),
            true,
        );
        let mut anchors: Vec<_> = sites
            .contexts
            .values()
            .flatten()
            .map(|input| {
                let site = input.site;
                (
                    &text[site.range],
                    &text[site.range.start().to_usize()..site.anchor.to_usize()],
                )
            })
            .collect();
        anchors.sort_unstable();
        assert_eq!(
            anchors,
            [("install(2)", "install"), ("manager", ""), ("λ[0]", "λ")]
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
            calls += usize::from(matches!(
                value.kind,
                ExprKind::Invoke { .. } | ExprKind::Builtin(Builtin::RegisteredCall(_), _)
            ));
        });
        assert_eq!(calls, 0);
    }

    #[test]
    fn prepared_namespace_assignments_and_loops_stage_the_same_targets() {
        for source in [
            "owners[0], (sink.field, *rest) = row\n",
            "for owners[0], (sink.field, *rest) in rows:\n    publish(rest)\n",
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
                let mut snapshots = Vec::new();
                let mut attrs = 0;
                let mut items = 0;
                ir::walk_expressions(&body.function.body, &mut |expr| match &expr.kind {
                    ExprKind::Builtin(Builtin::RegisteredUnpack { count, star }, _) => {
                        snapshots.push((*count, *star))
                    }
                    ExprKind::Builtin(
                        Builtin::RegisteredAttribute {
                            action: ir::AttributeAction::Store,
                            name,
                            ..
                        },
                        _,
                    ) if name == "field" => attrs += 1,
                    _ => {}
                });
                ir::walk_statements(&body.function.body, &mut |stmt| {
                    items += usize::from(matches!(stmt, Statement::SetItem(..)));
                });
                assert_eq!(snapshots, [(2, None), (2, Some(1))]);
                assert_eq!((items, attrs), (1, 1));
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
                template_fields: None,
                generators: None,
                lambdas: None,
                text,
                private_names: None,
                method_imports: &imre_ast::module_imports(body),
                replacements: &HashMap::new(),
                has_templates: false,
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
    fn awaited_calls_retain_the_checked_loop_cursor() {
        let source =
            "async def visit(rows, call):\n    for row in rows:\n        await call(row)\n";
        assert_cursor_suspensions(source, &["await call(row)"]);
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
        let mut cursors = Vec::new();
        let mut suspensions = Vec::new();
        ir::walk_statements(&checked.function.body, &mut |stmt| {
            if let Statement::IterNext { iterator, .. } = stmt {
                cursors.push(*iterator);
            }
        });
        ir::walk_expressions(&checked.function.body, &mut |expr| {
            if let ExprKind::Builtin(Builtin::RegisteredSuspend(kind, range), _) = &expr.kind {
                assert_eq!(matches!(kind, ir::SuspendValue::Await), definition.is_async);
                suspensions.push(&source[*range]);
            }
        });
        assert_eq!(cursors.len(), 1);
        assert!(checked.region().private_slots().contains(&cursors[0]));
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
            let mut unpacks = 0;
            let mut stores = 0;
            ir::walk_expressions(&function.body, &mut |expr| {
                if matches!(
                    expr.kind,
                    ExprKind::Builtin(
                        Builtin::RegisteredUnpack {
                            count: 2,
                            star: None
                        },
                        _
                    )
                ) {
                    unpacks += 1;
                }
            });
            ir::walk_statements(&function.body, &mut |stmt| {
                stores += usize::from(matches!(stmt, Statement::SetItem(..)));
            });
            assert_eq!((unpacks, stores), (1, 2), "{source}");
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
            ExprKind::Builtin(Builtin::RegisteredIter(ir::IteratorOp::Loop), _) => opens += 1,
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
                if let Some(site) = &args.python_site
                    && let Some(scope) = &site.scope =>
            {
                let ExprKind::Local(namespace) = scope.kind else {
                    panic!("expected the comprehension's namespace slot")
                };
                scoped_calls += 1;
                if &graph.modules[graph.entry].source[site.range]
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
                if let ExprKind::Builtin(Builtin::RegisteredCall(call), _) = &expr.kind {
                    calls.push(&call.arguments);
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
        let filename = ir
            .lines()
            .find_map(|line| {
                let (symbol, data) = line.split_once(" = private unnamed_addr constant ")?;
                data.ends_with(r#"report.py\00""#).then_some(symbol)
            })
            .expect("report.py source filename");
        let body_at = |line: usize| {
            let location = format!("ptr {filename}, i32 {line}, i64 0)");
            let mut matches = ir
                .split("\ndefine ")
                .filter_map(|definition| definition.split_once("\n}").map(|(body, _)| body))
                .filter(|body| {
                    body.lines().any(|line| {
                        line.contains("call void @imre_dyn_statement_with_exception(")
                            && line.ends_with(&location)
                    })
                });
            let body = matches.next().expect("compiled source body");
            assert!(matches.next().is_none(), "source body emitted twice");
            body
        };
        let report = body_at(2);
        let add = "call i64 @int_add(";
        let callback = "call i64 @imre_dyn_positional_call_at(";
        assert_eq!(report.matches(add).count(), 1);
        assert_eq!(report.matches(callback).count(), 1);
        assert!(report.find(add).unwrap() < report.find(callback).unwrap());
        let pair = body_at(7);
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
