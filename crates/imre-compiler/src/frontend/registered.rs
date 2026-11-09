//! Lower functions whose live Python arguments are supplied by the registered binder.

use super::*;

mod templates;
pub(crate) use templates::{prepare_templates, template_segments};
pub(super) mod values;

pub(super) struct Frame<'a> {
    pub(super) literals: &'a values::ModuleSource,
    pub(super) deferred: std::cell::RefCell<Option<Failure>>,
}

impl<'a> Frame<'a> {
    pub(super) fn new(literals: &'a values::ModuleSource) -> Self {
        Self {
            literals,
            deferred: Default::default(),
        }
    }

    pub(super) fn require_object(&self, source: &str, site: &impl Ranged) {
        self.deferred.borrow_mut().get_or_insert_with(|| {
            error(
                source,
                site,
                "operation requires its compiled object representation",
            )
            .classify(Kind::ScopedObject)
        });
    }
}

pub(crate) struct RegisteredFunction {
    pub(crate) function: ir::Function,
    pub(crate) local_names: HashMap<usize, LocalBinding>,
}

impl RegisteredFunction {
    /// Only IR scopes name compiler-owned storage. User-visible names stay in the Python frame.
    pub(crate) fn private_slots(&self) -> std::collections::BTreeSet<usize> {
        let body = &self.function.body;
        let mut slots = std::collections::BTreeSet::new();
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
        slots.retain(|slot| !self.local_names.contains_key(slot));
        slots
    }
}

/// Storage in the function frame or in a lexically nested Python namespace.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct LocalBinding {
    pub(crate) name: String,
    pub(crate) namespace: Option<usize>,
}

/// Lower a definition whose binder supplies Python object handles in its normal function scope.
pub(super) fn lower_definition<'a>(
    mut checker: Checker<'a>,
    definition: &ast::StmtFunctionDef,
    signature: &Signature,
    literals: &'a values::ModuleSource,
) -> Result<RegisteredFunction, String> {
    checker.registered = Some(Frame::new(literals));
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
    if !crate::dynamic::supports_registered_function(&function) {
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

pub(super) fn capture(
    graph: &SourceGraph,
    signatures: &HashMap<DefinitionId, Signature>,
    classes: &ClassInfo,
    types: &Types,
    inference: &inference::Inference,
    globals: &ModuleValues,
    retain_live_parameters: bool,
) {
    let mut registered = HashMap::<ModuleId, HashMap<TextSize, RegisteredFunction>>::new();
    let mut sources = HashMap::new();
    let mut checks = SourceChecks::default();
    for (&owner, signature) in signatures {
        let id = owner.module;
        let module = &graph.modules[id];
        if !module.is_program_source() {
            continue;
        }
        // Use the original definition. The registered binder retains its names, defaults and frame.
        let Some(function) = module.parsed.syntax().body[owner.statement].as_function_def_stmt()
        else {
            continue;
        };
        let Some(signature) = inference.registered_signature(signature) else {
            if let Some(signature) = inference.checked_signature(signature) {
                let helpers = Helpers::new(inference.len());
                let checker =
                    Checker::new(graph, id, signatures, classes, types, &helpers, globals);
                retained::capture(checker, function, &signature, inference.len(), false);
            }
            continue;
        };
        if !retain_live_parameters {
            continue;
        }
        let helpers = Helpers::new(inference.len());
        let checker = Checker::new(graph, id, signatures, classes, types, &helpers, globals);
        let literals = sources
            .entry(id)
            .or_insert_with(|| values::ModuleSource::new(module));
        if let Ok(body) = lower_definition(checker, function, &signature, literals) {
            if !checks.accepts(graph, id, function) {
                continue;
            }
            registered
                .entry(id)
                .or_default()
                .insert(function.start(), body);
        }
    }
    if retain_live_parameters {
        graph.set_registered_functions(registered);
    }
}

pub(super) fn checkpoint(stmt: &ast::Stmt) -> Statement {
    Statement::Expr(operation(
        Builtin::RegisteredStatement(stmt.range(), imre_ast::unhighlighted_call(stmt)),
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
    Function { is_async: bool },
}

/// Source facts needed to prepare registered bodies, including definitions inside class scopes.
#[derive(Default)]
pub(crate) struct RegisteredSites<'a> {
    pub(crate) operations: HashMap<TextRange, Expr>,
    pub(crate) workers: Vec<(&'a ast::StmtFunctionDef, imre_ast::ThreadEntry)>,
}

pub(crate) fn registered_sites<'a>(
    stmts: &'a [ast::Stmt],
    private_names: &imre_ast::Names,
) -> RegisteredSites<'a> {
    use ast::statement_visitor::{StatementVisitor, walk_stmt};
    struct Collector<'s, 'a> {
        private_names: &'s imre_ast::Names,
        sites: RegisteredSites<'a>,
    }
    impl<'a> StatementVisitor<'a> for Collector<'_, 'a> {
        fn visit_stmt(&mut self, stmt: &'a ast::Stmt) {
            if let ast::Stmt::FunctionDef(def) = stmt {
                if let Some(entry) = imre_ast::thread_entry(def) {
                    self.sites.workers.push((def, entry));
                    // An admitted suite contains no imports, deletes, or declarations.
                    return;
                }
            } else if let Some(import) = registered_import(stmt, self.private_names) {
                self.sites.operations.insert(stmt.range(), import);
            } else if let ast::Stmt::Delete(delete) = stmt {
                for target in delete_targets(delete) {
                    if let Some(binding) = registered_delete_name(target) {
                        self.sites.operations.insert(target.range(), binding);
                    }
                }
            }
            walk_stmt(self, stmt);
        }
    }
    let mut collector = Collector {
        private_names,
        sites: RegisteredSites::default(),
    };
    collector.visit_body(stmts);
    collector.sites
}

pub(crate) struct DeferredModuleAnnotation {
    pub(crate) name: String,
    pub(crate) value: AnnotationValue,
}

pub(crate) enum AnnotationValue {
    Checked(Expr),
    Bound(Box<AnnotationChild>),
    Source(ast::Expr),
}

pub(crate) struct AnnotationChild {
    pub(crate) id: ir::AnnotationCodeId,
    pub(crate) text: String,
    pub(crate) body: RegisteredFunction,
    pub(crate) has_templates: bool,
}

impl AnnotationChild {
    fn new(
        id: ir::AnnotationCodeId,
        text: &str,
        parsed: &ruff_python_parser::Parsed<ast::ModExpression>,
    ) -> Option<Self> {
        let expression = &parsed.syntax().body;
        let tokens = parsed.tokens();
        let replacements = values::literal_replacements(text, tokens);
        let has_templates = tokens
            .iter()
            .any(|token| token.kind() == ast::token::TokenKind::TStringStart);
        // An eval expression has its own source positions and looks up names in
        // the namespace supplied by its eventual caller.
        let source = values::RegisteredSource {
            text,
            replacements: &replacements,
            has_templates,
            method_imports: &Default::default(),
            private_names: None,
        };
        let value = values::registered_value(expression, source)?;
        let checkpoint = operation(
            Builtin::RegisteredStatement(expression.range(), None),
            Type::None,
            vec![],
        );
        Some(Self {
            id,
            text: text.to_owned(),
            has_templates,
            body: RegisteredFunction {
                function: ir::Function {
                    params: vec![],
                    returns: Type::PythonObject,
                    locals: vec![],
                    body: vec![Statement::Expr(checkpoint), Statement::Return(value)],
                },
                local_names: HashMap::new(),
            },
        })
    }
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

impl DeferredModuleAnnotation {
    fn new(
        id: ir::AnnotationCodeId,
        name: String,
        expression: &ast::Expr,
        source: values::RegisteredSource<'_>,
        bundled: bool,
    ) -> Self {
        let value = if let ast::Expr::StringLiteral(literal) = expression
            && !source.has_templates
            && source.replacements.is_empty()
        {
            let text = literal.value.to_str();
            if !bundled && let Some(parsed) = annotation_expression(text) {
                AnnotationChild::new(id, text, &parsed).map_or_else(
                    || AnnotationValue::Source(expression.clone()),
                    |child| AnnotationValue::Bound(Box::new(child)),
                )
            } else {
                // annotation_text returns an unpositioned normal str on this path.
                AnnotationValue::Checked(Expr {
                    ty: Type::Str,
                    kind: ExprKind::Str(text.to_owned()),
                })
            }
        } else {
            values::registered_value(expression, source).map_or_else(
                || AnnotationValue::Source(expression.clone()),
                AnnotationValue::Checked,
            )
        };
        Self { name, value }
    }

    /// Number already-checked operands in physical marker order, including repeated finally edges.
    pub(crate) fn prepare_body(entries: &[&Self]) -> RegisteredSuite {
        let mut residual = Vec::new();
        let mut body = vec![Statement::Expr(operation(
            Builtin::RegisteredAnnotationStart,
            Type::None,
            vec![],
        ))];
        for (index, entry) in entries.iter().enumerate() {
            let value = match &entry.value {
                AnnotationValue::Checked(value) => value.clone(),
                AnnotationValue::Bound(child) => operation(
                    Builtin::BindAnnotationExpression(child.id),
                    Type::Str,
                    vec![Expr {
                        ty: Type::Str,
                        kind: ExprKind::Str(child.text.clone()),
                    }],
                ),
                AnnotationValue::Source(_) => {
                    SourceRun::record(&mut residual, index, body.len());
                    continue;
                }
            };
            body.push(Statement::Expr(operation(
                Builtin::RegisteredAnnotationInsert {
                    index,
                    name: entry.name.clone(),
                },
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

/// Prepare module markers and the delayed operands of their native annotation callback.
pub(crate) fn module_annotations(
    body: &[ast::Stmt],
    string_annotations: bool,
    operations: &mut HashMap<TextRange, Expr>,
    source: values::RegisteredSource<'_>,
    bundled: bool,
) -> HashMap<TextSize, DeferredModuleAnnotation> {
    use ast::statement_visitor::{StatementVisitor, walk_stmt};
    struct Annotations<'a> {
        string_annotations: bool,
        operations: &'a mut HashMap<TextRange, Expr>,
        deferred: HashMap<TextSize, DeferredModuleAnnotation>,
        source: values::RegisteredSource<'a>,
        bundled: bool,
    }
    impl<'a> StatementVisitor<'a> for Annotations<'_> {
        fn visit_stmt(&mut self, stmt: &'a ast::Stmt) {
            match stmt {
                ast::Stmt::FunctionDef(_) | ast::Stmt::ClassDef(_) => {}
                ast::Stmt::AnnAssign(ann) => {
                    if ann.simple
                        && let Some(name) = ann.target.as_name_expr()
                    {
                        let name = name.id.to_string();
                        let annotation = if self.string_annotations {
                            let spelling = super::annotation_spelling(&ann.annotation);
                            if !self.bundled && annotation_expression(&spelling).is_some() {
                                // The source statement still binds an evaluable string to its native code.
                                return;
                            }
                            ir::ModuleAnnotation::String { name, spelling }
                        } else {
                            let site = DeferredModuleAnnotation::new(
                                ir::AnnotationCodeId(self.deferred.len()),
                                name,
                                &ann.annotation,
                                self.source,
                                self.bundled,
                            );
                            self.deferred.insert(ann.start(), site);
                            ir::ModuleAnnotation::Deferred(ann.start())
                        };
                        self.operations.insert(
                            ann.target.range(),
                            operation(
                                Builtin::RegisteredModuleAnnotation(annotation),
                                Type::None,
                                vec![],
                            ),
                        );
                    }
                }
                _ => walk_stmt(self, stmt),
            }
        }
    }
    let mut annotations = Annotations {
        string_annotations,
        operations,
        deferred: HashMap::new(),
        source,
        bundled,
    };
    annotations.visit_body(body);
    annotations.deferred
}

/// Checked instructions retain their canonical function layout. Source runs are owned
/// by the physical suite and identify the remaining gaps between checked statements.
pub(crate) struct SourceRun {
    pub(crate) at: usize,
    pub(crate) statements: std::ops::Range<usize>,
}

impl SourceRun {
    fn record(runs: &mut Vec<Self>, source_index: usize, checked_offset: usize) {
        if let Some(run) = runs.last_mut()
            && run.statements.end == source_index
        {
            run.statements.end += 1;
        } else {
            runs.push(Self {
                at: checked_offset,
                statements: source_index..source_index + 1,
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

fn plain_class_header(class: &ast::StmtClassDef) -> bool {
    class.type_params.is_none() && class.arguments.as_ref().is_none_or(|args| args.is_empty())
}

/// Prepare ordered suites while the source tree and its namespace operands are available.
pub(crate) struct SuitePreparation<'a> {
    operations: &'a HashMap<TextRange, Expr>,
    source: values::RegisteredSource<'a>,
    pub(crate) nested: HashMap<TextSize, RegisteredSuite>,
}

impl<'a> SuitePreparation<'a> {
    pub(crate) fn new(
        operations: &'a HashMap<TextRange, Expr>,
        source: values::RegisteredSource<'a>,
    ) -> Self {
        Self {
            operations,
            source,
            nested: HashMap::new(),
        }
    }

    pub(crate) fn suite(&mut self, stmts: &[ast::Stmt], kind: RegisteredScope) -> RegisteredSuite {
        use ast::statement_visitor::{StatementVisitor, walk_stmt};
        let plan = self.lower(stmts, kind);
        struct Children<'p, 's> {
            prepare: &'p mut SuitePreparation<'s>,
            kind: RegisteredScope,
        }
        impl<'a> StatementVisitor<'a> for Children<'_, '_> {
            fn visit_body(&mut self, stmts: &'a [ast::Stmt]) {
                if let Some(first) = stmts.first() {
                    let plan = self.prepare.suite(stmts, self.kind);
                    self.prepare.nested.insert(first.start(), plan);
                }
            }
        }
        let mut children = Children {
            prepare: self,
            kind,
        };
        for run in &plan.residual {
            for stmt in &stmts[run.statements.clone()] {
                // A declaration opens a separate activation with its own scope and admission.
                if !matches!(stmt, ast::Stmt::FunctionDef(_) | ast::Stmt::ClassDef(_)) {
                    walk_stmt(&mut children, stmt);
                }
            }
        }
        plan
    }

    fn lower(&self, stmts: &[ast::Stmt], kind: RegisteredScope) -> RegisteredSuite {
        let module = matches!(kind, RegisteredScope::Module);
        let mut scope = SourceScope {
            kind,
            operations: self.operations,
            source: self.source,
            names: HashMap::new(),
            locals: Vec::new(),
            loop_depth: 0,
        };
        let mut body = Vec::new();
        let mut residual: Vec<SourceRun> = Vec::new();
        for (i, stmt) in stmts.iter().enumerate() {
            let slots_before = scope.locals.len();
            if let Ok((lowered, _)) =
                statements::StatementLowering::source_statement(&mut scope, stmt, module)
            {
                body.push(checkpoint(stmt));
                body.extend(lowered);
            } else {
                // A partially lowered target can allocate slots before a later operand fails.
                if scope.locals.len() != slots_before {
                    scope.locals.truncate(slots_before);
                    scope.names.retain(|_, slot| *slot < slots_before);
                }
                SourceRun::record(&mut residual, i, body.len());
            }
        }
        let locals = scope.locals;
        RegisteredSuite {
            checked: RegisteredFunction {
                function: ir::Function {
                    params: Vec::new(),
                    returns: Type::PythonObject,
                    locals,
                    body,
                },
                local_names: scope
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
                    .collect(),
            },
            residual,
        }
    }

    pub(crate) fn complete(
        &self,
        stmts: &[ast::Stmt],
        kind: RegisteredScope,
    ) -> Option<RegisteredSuite> {
        let plan = self.lower(stmts, kind);
        plan.fully_checked().then_some(plan)
    }

    pub(crate) fn class_body(&self, class: &ast::StmtClassDef) -> Option<RegisteredSuite> {
        if !plain_class_header(class)
            || !class.body.iter().all(|stmt| {
                matches!(
                    stmt,
                    ast::Stmt::Assign(_) | ast::Stmt::Expr(_) | ast::Stmt::Pass(_)
                )
            })
        {
            return None;
        }
        self.complete(&class.body, RegisteredScope::Class)
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
        Builtin::RegisteredAttribute {
            action,
            name: field.attr.to_string(),
            range: field.range(),
        },
        Type::None,
        values,
    ))
}

/// Deletion groups evaluate their leaves from left to right without constructing a tuple or list.
pub(crate) fn delete_targets(delete: &ast::StmtDelete) -> Vec<&ast::Expr> {
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

pub(crate) fn attribute_effects(stmt: &ast::Stmt) -> Option<Vec<AttributeEffect<'_>>> {
    use ir::AttributeAction::{Delete, InPlace, Store};
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
        ast::Stmt::Delete(delete) => {
            return delete_targets(delete)
                .into_iter()
                .map(|target| {
                    Some(AttributeEffect {
                        action: Delete,
                        field: target.as_attribute_expr()?,
                        value: None,
                    })
                })
                .collect();
        }
        _ => return None,
    };
    Some(vec![AttributeEffect {
        action,
        field: target.as_attribute_expr()?,
        value,
    }])
}

/// Prepare only an indexed target's receiver and key. Its caller owns RHS evaluation
/// or unpacking, and retains each finished target when a later target raises.
pub(crate) fn registered_index_target(
    target: &ast::Expr,
    source: values::RegisteredSource<'_>,
) -> Option<(Expr, Expr)> {
    let index = target.as_subscript_expr()?;
    let base = values::registered_value(&index.value, source)?;
    let key = values::registered_index_key(&index.slice, source)?;
    Some((base, key))
}

pub(crate) fn delete_item(receiver: Expr, key: Expr) -> Expr {
    operation(
        Builtin::RegisteredDeleteItem,
        Type::None,
        vec![receiver, key],
    )
}

impl statements::StatementLowering for SourceScope<'_> {
    type Error = ();

    fn attribute_statements(&mut self, stmt: &ast::Stmt) -> StdResult<Option<Vec<Statement>>, ()> {
        let Some(effects) = attribute_effects(stmt) else {
            return Ok(None);
        };
        effects
            .iter()
            .map(|effect| {
                let values = effect
                    .operands()
                    .map(|node| self.ordinary_operand(node))
                    .collect::<Option<_>>()
                    .ok_or(())?;
                Ok(attribute_statement(effect.action, effect.field, values))
            })
            .collect::<StdResult<Vec<_>, _>>()
            .map(Some)
    }

    fn assignment(&mut self, assign: &ast::StmtAssign) -> StdResult<Vec<Statement>, ()> {
        let [
            target @ (ast::Expr::Name(_)
            | ast::Expr::Subscript(_)
            | ast::Expr::Tuple(_)
            | ast::Expr::List(_)),
        ] = assign.targets.as_slice()
        else {
            return Err(());
        };
        let value = if target.is_name_expr() {
            self.operand(&assign.value)
        } else {
            self.ordinary_operand(&assign.value)
        }
        .ok_or(())?;
        unpacking::bind_targets(self, target, value, None)
    }

    fn definition(&mut self, def: &ast::StmtFunctionDef) -> StdResult<Option<Statement>, ()> {
        if def.type_params.is_some() {
            return Err(());
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
            Builtin::RegisteredDefinition(def.start()),
            Type::PythonObject,
            header,
        );
        Ok(Some(self.store(def.name.as_str(), function)))
    }

    fn effect(&mut self, value: &ast::Expr) -> StdResult<(Option<Statement>, bool), ()> {
        let effect = if value.is_string_literal_expr() {
            None
        } else {
            Some(Statement::Expr(self.operand(value).ok_or(())?))
        };
        Ok((effect, true))
    }

    fn returned(&mut self, ret: &ast::StmtReturn) -> StdResult<Expr, ()> {
        match ret.value.as_deref() {
            Some(value) => self.operand(value).ok_or(()),
            None => Ok(Expr::none()),
        }
    }

    fn for_loop(&mut self, loop_: &ast::StmtFor) -> StdResult<statements::Block, ()> {
        let source = self.ordinary_operand(&loop_.iter).ok_or(())?;
        let cursor = operation(Builtin::RegisteredIter, Type::PythonObject, vec![source]);
        let input = pipelines::Prepared::cursor(cursor, Type::PythonObject, |ty| self.hidden(ty));
        let binding = unpacking::bind_targets(
            self,
            &loop_.target,
            Expr::local(input.value, Type::PythonObject),
            None,
        )?;
        let body = self.loop_body(&loop_.body)?;
        let otherwise = self.suite(&loop_.orelse)?;
        Ok((input.for_suite(binding, body, otherwise, Vec::new()), true))
    }

    fn other_statement(&mut self, stmt: &ast::Stmt) -> StdResult<statements::Block, ()> {
        match stmt {
            ast::Stmt::AugAssign(assign) => {
                let (receiver, key) =
                    registered_index_target(&assign.target, self.source).ok_or(())?;
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
            ast::Stmt::ClassDef(class) if plain_class_header(class) => {
                let decorators = class
                    .decorator_list
                    .iter()
                    .map(|d| self.ordinary_operand(&d.expression))
                    .collect::<Option<_>>()
                    .ok_or(())?;
                let value = operation(
                    Builtin::RegisteredDefinition(class.start()),
                    Type::PythonObject,
                    decorators,
                );
                Ok((vec![self.store(class.name.as_str(), value)], true))
            }
            ast::Stmt::While(loop_) => {
                let condition = self.ordinary_operand(&loop_.test).ok_or(())?;
                let body = self.loop_body(&loop_.body)?;
                let otherwise = self.suite(&loop_.orelse)?;
                Ok((vec![Statement::While(condition, body, otherwise)], true))
            }
            ast::Stmt::Break(_) if self.loop_depth != 0 => Ok((vec![Statement::Break], false)),
            ast::Stmt::Continue(_) if self.loop_depth != 0 => {
                Ok((vec![Statement::Continue], false))
            }
            ast::Stmt::Global(_) | ast::Stmt::Nonlocal(_) => Ok((Vec::new(), true)),
            ast::Stmt::AnnAssign(ann) if matches!(self.kind, RegisteredScope::Module) => {
                let marker = self.operations.get(&ann.target.range()).ok_or(())?.clone();
                let mut body = Vec::new();
                if let Some(value) = &ann.value {
                    let name = ann.target.as_name_expr().ok_or(())?;
                    let value = self.operand(value).ok_or(())?;
                    body.push(self.store(name.id.as_str(), value));
                }
                body.push(Statement::Expr(marker));
                Ok((body, true))
            }
            ast::Stmt::Delete(delete) => {
                let body = delete_targets(delete)
                    .into_iter()
                    .map(|target| {
                        if let Some(op) = self.operations.get(&target.range()) {
                            return Some(Statement::Expr(op.clone()));
                        }
                        if let Some(sub) = target.as_subscript_expr() {
                            let receiver = self.ordinary_operand(&sub.value)?;
                            let key = values::index_key(&sub.slice, &mut |e| {
                                self.ordinary_operand(e).ok_or(())
                            })
                            .ok()?;
                            return Some(Statement::Expr(delete_item(receiver, key)));
                        }
                        let field = target.as_attribute_expr()?;
                        let receiver = self.ordinary_operand(&field.value)?;
                        Some(attribute_statement(
                            ir::AttributeAction::Delete,
                            field,
                            vec![receiver],
                        ))
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

/// Attribute and subscript targets retain their ordinary receiver evaluation.
fn registered_delete_name(target: &ast::Expr) -> Option<Expr> {
    let name = target.as_name_expr()?;
    Some(operation(
        Builtin::RegisteredDeleteName(name.id.to_string(), name.range()),
        Type::None,
        vec![],
    ))
}

struct SourceScope<'a> {
    kind: RegisteredScope,
    operations: &'a HashMap<TextRange, Expr>,
    source: values::RegisteredSource<'a>,
    names: HashMap<String, usize>,
    locals: Vec<Type>,
    loop_depth: usize,
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

impl SourceScope<'_> {
    fn suite(&mut self, stmts: &[ast::Stmt]) -> StdResult<Vec<Statement>, ()> {
        let mut body = Vec::new();
        for stmt in stmts {
            body.push(checkpoint(stmt));
            let (lowered, _) = statements::StatementLowering::source_statement(
                self,
                stmt,
                matches!(self.kind, RegisteredScope::Module),
            )?;
            body.extend(lowered);
        }
        Ok(body)
    }

    fn loop_body(&mut self, statements: &[ast::Stmt]) -> StdResult<Vec<Statement>, ()> {
        self.loop_depth += 1;
        let body = self.suite(statements);
        self.loop_depth -= 1;
        body
    }

    fn hidden(&mut self, ty: Type) -> usize {
        let slot = self.locals.len();
        self.locals.push(ty);
        slot
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
        let slot = self.slot(name);
        if matches!(self.kind, RegisteredScope::Module) {
            Statement::Expr(operation(Builtin::GlobalSet(slot), Type::None, vec![value]))
        } else {
            Statement::Assign(slot, value)
        }
    }

    fn ordinary_operand(&mut self, expr: &ast::Expr) -> Option<Expr> {
        let value = if let ast::Expr::Named(named) = expr {
            let target = named.target.as_name_expr()?;
            let value = self.ordinary_operand(&named.value)?;
            Expr {
                ty: value.ty,
                kind: ExprKind::Bind(self.slot(&target.id), Box::new(value)),
            }
        } else {
            values::operation(
                expr,
                self.source,
                None,
                |_| true,
                |child| self.ordinary_operand(child).ok_or(()),
            )
            .ok()??
        };
        Some(self.source.located(expr, value))
    }

    fn operand(&mut self, expr: &ast::Expr) -> Option<Expr> {
        // The current loop cursor lives in this native activation; a suspension
        // across a pull needs a persisted cursor in the generator frame.
        if self.loop_depth == 0
            && let RegisteredScope::Function { is_async } = self.kind
            && let Some((kind, value)) = ir::SuspendValue::operand(expr)
            && !matches!(
                (kind, is_async),
                (ir::SuspendValue::Await, false) | (ir::SuspendValue::Delegate, true)
            )
        {
            // Admit a suspension only at this whole-expression boundary. All previous
            // statements have committed their owners to the observable frame.
            let value = match value {
                Some(value) => self.ordinary_operand(value)?,
                None => Expr::none(),
            };
            return Some(self.source.located(
                expr,
                operation(
                    Builtin::RegisteredSuspend(kind, expr.range()),
                    Type::PythonObject,
                    vec![value],
                ),
            ));
        }
        self.ordinary_operand(expr)
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

    fn registered_suite(
        stmts: &[ast::Stmt],
        kind: RegisteredScope,
        source: values::RegisteredSource<'_>,
    ) -> Option<RegisteredFunction> {
        let default_names = imre_ast::Names::default();
        let names = source.private_names.unwrap_or(&default_names);
        let mut operations = registered_sites(stmts, names).operations;
        if matches!(kind, RegisteredScope::Module) {
            module_annotations(
                stmts,
                names.string_annotations,
                &mut operations,
                source,
                false,
            );
        }
        SuitePreparation::new(&operations, source)
            .complete(stmts, kind)
            .map(|plan| plan.checked)
    }

    #[test]
    fn annotation_callback_numbers_physical_occurrences_and_keeps_unsupported_values_late() {
        let text = "first: int\nsecond: list[int]\ncomplex: dict[str, int]\nforward: 'list[int]'\ndeferred: (lambda: int)()\n";
        let graph = SourceGraph::single(text).unwrap();
        let module = &graph.modules[graph.entry];
        let stmts = &module.parsed.syntax().body;
        let literals = values::ModuleSource::new(module);
        let source = literals.context(text);
        let sites = module_annotations(stmts, false, &mut HashMap::new(), source, false);
        let first = &sites[&stmts[0].start()];
        let second = &sites[&stmts[1].start()];
        let whole = DeferredModuleAnnotation::prepare_body(&[first, second, first]);
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
                                Builtin::RegisteredAnnotationInsert { index, name },
                                operands,
                            ),
                        ..
                    }) = stmt
                    else {
                        panic!("expected a guarded annotation insertion")
                    };
                    assert_eq!(operands.len(), 1);
                    (*index, name.as_str())
                })
                .collect();
        assert_eq!(numbered, [(0, "first"), (1, "second"), (2, "first")]);
        let bundled_sites = module_annotations(stmts, false, &mut HashMap::new(), source, true);
        let quoted_site = stmts[3].start();
        assert!(
            matches!(&sites[&quoted_site].value, AnnotationValue::Bound(child) if child.text == "list[int]")
        );
        assert!(
            matches!(&bundled_sites[&quoted_site].value, AnnotationValue::Checked(Expr {kind: ExprKind::Str(text), ..}) if text == "list[int]")
        );
        let forward = DeferredModuleAnnotation::prepare_body(&[&sites[&quoted_site]]);
        assert!(forward.fully_checked());
        assert!(matches!(&forward.checked.function.body[1],
            Statement::Expr(Expr { kind: ExprKind::Builtin(
                Builtin::RegisteredAnnotationInsert { index: 0, .. }, values), .. })
                if matches!(values.as_slice(), [Expr { kind: ExprKind::Builtin(
                    Builtin::BindAnnotationExpression(id), args), .. }] if *id == ir::AnnotationCodeId(3) &&
                    matches!(args.as_slice(), [Expr { kind: ExprKind::Str(text), .. }] if text == "list[int]"))));
        {
            assert!(matches!(
                sites[&stmts[2].start()].value,
                AnnotationValue::Checked(_)
            ));
            let unavailable = &sites[&stmts[4].start()];
            assert!(matches!(unavailable.value, AnnotationValue::Source(_)));
            let mixed = DeferredModuleAnnotation::prepare_body(&[first, unavailable, second]);
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
        let text =
            "from . import a\nx: int = a\nif a:\n    del a\ny: int = (lambda: 1)()\nimport z\n";
        let mut stmts = ruff_python_parser::parse_module(text)
            .unwrap()
            .into_syntax()
            .body;
        let names = imre_ast::lower(&mut stmts);
        let mut ops = registered_sites(&stmts, &names).operations;
        let replacements = HashMap::new();
        let imports = imre_ast::module_imports(&stmts);
        let source = RegisteredSource {
            templates: None,
            text,
            private_names: Some(&names),
            replacements: &replacements,
            has_templates: false,
            method_imports: &imports,
        };
        let annotations =
            module_annotations(&stmts, names.string_annotations, &mut ops, source, false);
        assert_eq!(annotations.len(), 2);
        let mut prepare = SuitePreparation::new(&ops, source);
        let plan = prepare.suite(&stmts, RegisteredScope::Module);
        let [run] = &plan.residual[..] else {
            panic!("expected one source range")
        };
        assert_eq!(run.statements, 2..4);
        assert_eq!(run.at, 5);
        assert_eq!(plan.checked.function.body.len(), 7);
        assert!(prepare.nested.values().all(RegisteredSuite::fully_checked));
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
        let result = crate::checked_lower_with(&mut graph, None, false).unwrap();
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
            ExprKind::Builtin(Builtin::RegisteredCall(..), _) => callbacks += 1,
            _ => {}
        });
        assert_eq!((adds, callbacks), (1, 1));
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
        let mut object_calls = 0;
        ir::walk_expressions(&body.body, &mut |expr| match expr.kind {
            ExprKind::Builtin(Builtin::RegisteredStatement(range, _), _) => {
                checkpoints.push(&graph.modules[graph.entry].source[range]);
            }
            ExprKind::Binary(Binary::Add, ..) => checked_adds += 1,
            ExprKind::Builtin(Builtin::RegisteredCall(..), _) => object_calls += 1,
            _ => {}
        });
        assert_eq!(checkpoints, ["answer = x + y", "pass", "sink(answer)"]);
        assert_eq!((checked_adds, object_calls), (1, 1));
    }

    #[test]
    fn callback_result_uses_python_addition_in_the_checked_body() {
        let graph = SourceGraph::single(
            "def report(x: int, sink, y: int):\n    first = x + y\n    reply = sink(first)\n    sink(reply + y)\n",
        ).unwrap();
        assert!(super::super::lower(&graph).is_err());
        let functions = graph.registered_functions();
        let checked = &functions[&graph.entry][&TextSize::default()];
        assert!(crate::dynamic::supports_registered_function(checked));
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
        assert!(crate::dynamic::supports_registered_function(checked));
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
            ExprKind::Builtin(Builtin::RegisteredCall(..), _) => operations.push("call"),
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
            Ok(crate::CheckedProgram::Registered) => {}
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
            "s.partition('-')[1]",
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
    fn suspending_loop_keeps_its_resumable_generator_body() {
        let graph =
            SourceGraph::single("def sales(rows):\n    for row in rows:\n        yield row\n")
                .unwrap();
        let definition = graph.modules[graph.entry].parsed.syntax().body[0]
            .as_function_def_stmt()
            .unwrap();
        assert!(
            registered_suite(
                &definition.body,
                RegisteredScope::Function { is_async: false },
                values::ModuleSource::new(&graph.modules[graph.entry])
                    .context(&graph.modules[graph.entry].source)
            )
            .is_none()
        );
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
            ExprKind::Builtin(Builtin::RegisteredIter, _) => opens += 1,
            ExprKind::Builtin(Builtin::RegisteredCall(..), _) => calls += 1,
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
        ))
        .unwrap();
        let _ = super::super::lower(&graph);
        let bodies = graph.registered_functions();
        let checked = &bodies[&graph.entry][&TextSize::default()];
        let mut namespaces = 0;
        let mut scoped_calls = 0;
        let mut pulls = 0;
        ir::walk_expressions(&checked.function.body, &mut |expr| match &expr.kind {
            ExprKind::Builtin(Builtin::RegisteredNamespace(_), _) => namespaces += 1,
            ExprKind::Builtin(Builtin::RegisteredCall(call), _) if call.explicit_scope => {
                scoped_calls += 1
            }
            _ => {}
        });
        ir::walk_statements(&checked.function.body, &mut |stmt| {
            pulls += usize::from(matches!(stmt, Statement::IterNext { .. }));
        });
        assert_eq!((namespaces, scoped_calls, pulls), (3, 5, 3));
        assert_eq!(
            checked
                .local_names
                .values()
                .filter(|binding| binding.namespace.is_some())
                .count(),
            3
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
