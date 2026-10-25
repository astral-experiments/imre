//! Fixed module bindings with ordinary mutable values and ordered initialization.

use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;

use super::*;

type ClassLiteral = (Option<ModuleId>, Rc<[String]>);

#[derive(Default)]
pub(super) struct ModuleValues {
    definitions: RefCell<HashMap<DefinitionId, (usize, Type)>>,
    class_tuples: RefCell<HashMap<(DefinitionId, usize), usize>>,
    literal_tuples: RefCell<HashMap<ClassLiteral, usize>>,
    types: RefCell<Vec<Type>>,
    pub(super) regex_literals: RefCell<HashMap<String, regex::CaptureLayout>>,
    regex_globals: RefCell<HashMap<usize, regex::CaptureLayout>>,
}

impl ModuleValues {
    /// Each declaration owns a single tuple for its entire class lifetime,
    /// including references through aliases and subclasses.
    fn initialize_class_tuples(
        &self,
        definition: DefinitionId,
        class: &crate::classes::Class,
        types: &Types,
    ) -> Vec<Statement> {
        let mut tuples: Vec<_> = class
            .static_bindings
            .values()
            .filter_map(|value| match value {
                crate::classes::ClassValue::Strings {
                    owner,
                    ordinal,
                    values,
                    literal,
                } if *owner == definition => Some((*ordinal, values, *literal)),
                _ => None,
            })
            .collect();
        tuples.sort_unstable_by_key(|(ordinal, ..)| *ordinal);
        tuples.dedup_by_key(|(ordinal, ..)| *ordinal);
        let mut slots = self.types.borrow_mut();
        tuples
            .into_iter()
            .filter_map(|(ordinal, values, literal)| {
                let ty = types.tuple(vec![Type::Str; values.len()]);
                let slot = slots.len();
                // CPython shares literal constants within one compilation unit;
                // the empty tuple is shared across units as well.
                if literal {
                    let key = (
                        (!values.is_empty()).then_some(definition.module),
                        Rc::clone(values),
                    );
                    let existing = *self.literal_tuples.borrow_mut().entry(key).or_insert(slot);
                    if existing != slot {
                        self.class_tuples
                            .borrow_mut()
                            .insert((definition, ordinal), existing);
                        return None;
                    }
                }
                slots.push(ty);
                assert!(
                    self.class_tuples
                        .borrow_mut()
                        .insert((definition, ordinal), slot)
                        .is_none()
                );
                let tuple = Expr {
                    ty,
                    kind: ExprKind::Tuple(
                        values
                            .iter()
                            .map(|value| Expr {
                                ty: Type::Str,
                                kind: ExprKind::Str(value.clone()),
                            })
                            .collect(),
                    ),
                };
                Some(Statement::Expr(Expr {
                    ty: Type::None,
                    kind: ExprKind::Builtin(Builtin::GlobalSet(slot), vec![tuple]),
                }))
            })
            .collect()
    }

    pub(super) fn class_tuple(&self, owner: DefinitionId, ordinal: usize) -> Option<Expr> {
        let slot = *self.class_tuples.borrow().get(&(owner, ordinal))?;
        Some(Expr {
            ty: self.types.borrow()[slot],
            kind: ExprKind::Builtin(Builtin::GlobalGet(slot), vec![]),
        })
    }

    /// Resolves storage only after the defining initializer has been checked.
    pub fn get(&self, definition: DefinitionId) -> Option<(usize, Type)> {
        self.definitions.borrow().get(&definition).copied()
    }

    fn insert(&self, definition: DefinitionId, ty: Type) -> usize {
        let mut types = self.types.borrow_mut();
        let slot = types.len();
        types.push(ty);
        assert!(
            self.definitions
                .borrow_mut()
                .insert(definition, (slot, ty))
                .is_none()
        );
        slot
    }

    pub fn into_types(self) -> Vec<Type> {
        self.types.into_inner()
    }

    pub(super) fn regex_groups(&self, pattern: &Expr) -> Option<regex::CaptureLayout> {
        match &pattern.kind {
            ExprKind::Builtin(Builtin::RegexCompile(source), _) => {
                self.regex_literals.borrow().get(source).cloned()
            }
            ExprKind::Builtin(Builtin::GlobalGet(slot), _) => {
                self.regex_globals.borrow().get(slot).cloned()
            }
            _ => None,
        }
    }
}

struct Checkpoint {
    statement: usize,
    body: Range<usize>,
    initialized: usize,
    definitions: HashSet<DefinitionId>,
    class_reads: HashSet<DefinitionId>,
}

impl Checker<'_> {
    fn initialize_global(
        &mut self,
        statement: &ast::Stmt,
        definition: DefinitionId,
    ) -> Result<Statement, String> {
        let value = match statement {
            ast::Stmt::Assign(assignment) => self.expr(&assignment.value)?,
            ast::Stmt::AnnAssign(assignment) => {
                let value = assignment.value.as_deref().ok_or_else(|| {
                    error(
                        self.source,
                        statement,
                        "module values require an initializer",
                    )
                })?;
                if self.resolve(&assignment.annotation)
                    == Some(Binding::Intrinsic(Intrinsic::Final))
                {
                    self.expr(value)?
                } else {
                    let ty = annotation(
                        &assignment.annotation,
                        self.graph,
                        self.module,
                        &self.classes.declarations,
                        self.types,
                    )?;
                    self.expr_expected(value, ty)?
                }
            }
            _ => unreachable!(),
        };
        if callables::contains_callable(value.ty, self.types) {
            return Err(error(
                self.source,
                statement,
                "imported module values cannot contain callable values",
            ));
        }
        let slot = self.globals.insert(definition, value.ty);
        if let Some(groups) = self.globals.regex_groups(&value) {
            self.globals.regex_globals.borrow_mut().insert(slot, groups);
        }
        Ok(Statement::Expr(Expr {
            ty: Type::None,
            kind: ExprKind::Builtin(Builtin::GlobalSet(slot), vec![value]),
        }))
    }
}

pub(super) struct Initialization {
    pub module: ModuleId,
    pub function: ir::Function,
    pub calls: HashSet<usize>,
    checkpoints: Vec<Checkpoint>,
}

impl Initialization {
    /// Infers values in execution order while deferring callable body checks.
    pub fn lower(
        mut checker: Checker<'_>,
        available: &mut HashSet<DefinitionId>,
        range: Range<usize>,
    ) -> Result<Self, String> {
        checker.module_scope = true;
        let module = &checker.graph.modules[checker.module];
        let mut body = Vec::new();
        let mut checkpoints = Vec::new();
        for index in range {
            let statement = &module.parsed.syntax().body[index];
            if module.type_guards.binary_search(&index).is_ok()
                || local_functions::type_variable(checker.graph, checker.module, statement)
            {
                continue;
            }
            let definition = DefinitionId {
                module: checker.module,
                statement: index,
            };
            if checker.graph.unused_literal_global(definition) {
                continue;
            }
            match statement {
                ast::Stmt::ClassDef(_) => {
                    let declarations = &checker.classes.declarations;
                    if let Some(&class) = declarations.ids.get(&definition) {
                        body.extend(checker.globals.initialize_class_tuples(
                            definition,
                            &declarations.definitions[class],
                            checker.types,
                        ));
                    }
                    available.insert(definition);
                    continue;
                }
                ast::Stmt::FunctionDef(_) => {
                    available.insert(definition);
                    continue;
                }
                ast::Stmt::ImportFrom(import)
                    if crate::modules::errno_import(statement).is_some() =>
                {
                    let start = body.len();
                    body.extend(import.names.iter().map(|alias| {
                        checker
                            .graph
                            .checked_builtin_import(checker.module, alias.range());
                        checker.check_errno_import(alias.name.as_str())
                    }));
                    checkpoints.push(Checkpoint {
                        statement: index,
                        body: start..body.len(),
                        initialized: checker.globals.types.borrow().len(),
                        definitions: available.clone(),
                        class_reads: std::mem::take(checker.class_reads.get_mut()),
                    });
                    continue;
                }
                ast::Stmt::Import(_) | ast::Stmt::ImportFrom(_) => continue,
                _ if crate::modules::value_name(statement).is_some_and(|name| {
                    module.bindings.get(name) == Some(&Binding::Definition(definition))
                }) =>
                {
                    // A statically declared class factory binds a definition, not a
                    // stored module value. Its source position still controls use.
                    available.insert(definition);
                    continue;
                }
                _ => {}
            }
            if checker.module == checker.graph.entry
                && module.entry_globals.binary_search(&index).is_err()
            {
                continue;
            }
            let initialized = checker.globals.types.borrow().len();
            let start = body.len();
            if crate::modules::value_name(statement).is_some() {
                body.push(checker.initialize_global(statement, definition)?);
            } else {
                body.extend(checker.statement(statement)?.0);
            }
            checkpoints.push(Checkpoint {
                statement: index,
                body: start..body.len(),
                initialized,
                definitions: available.clone(),
                class_reads: std::mem::take(checker.class_reads.get_mut()),
            });
        }
        Ok(Self {
            module: checker.module,
            function: ir::Function {
                params: Vec::new(),
                returns: Type::None,
                locals: checker.locals,
                body,
            },
            calls: checker.calls,
            checkpoints,
        })
    }

    /// Rejects early direct and transitive reads, including hidden comprehensions.
    /// Requiring every potential callee to be available is deliberately conservative.
    pub fn validate(
        &self,
        graph: &SourceGraph,
        functions: &[ir::Function],
        class_reads: &[HashSet<DefinitionId>],
        callables: &[DefinitionId],
        targets: &HashMap<Type, HashSet<usize>>,
    ) -> Result<(), String> {
        let module = &graph.modules[self.module];
        for checkpoint in &self.checkpoints {
            let mut reads = HashSet::new();
            let mut calls = Vec::new();
            let mut references = Vec::new();
            let mut visited = HashSet::new();
            let mut required_classes = checkpoint.class_reads.clone();
            collect(
                &self.function.body[checkpoint.body.clone()],
                &mut reads,
                &mut calls,
                &mut references,
                targets,
            );
            while let Some(id) = calls.pop() {
                if !visited.insert(id) {
                    continue;
                }
                if let Some(definition) = callables.get(id)
                    && !checkpoint.definitions.contains(definition)
                {
                    return Err(module.error(
                        &module.parsed.syntax().body[checkpoint.statement],
                        "module initialization calls a definition before it is available",
                    ));
                }
                required_classes.extend(&class_reads[id]);
                collect(
                    &functions[id].body,
                    &mut reads,
                    &mut calls,
                    &mut references,
                    targets,
                );
            }
            if !required_classes.is_subset(&checkpoint.definitions) {
                return Err(module.error(
                    &module.parsed.syntax().body[checkpoint.statement],
                    "module initialization references a class before it is available",
                ));
            }
            if references.iter().any(|&id| {
                callables
                    .get(id)
                    .is_some_and(|definition| !checkpoint.definitions.contains(definition))
            }) {
                return Err(module.error(
                    &module.parsed.syntax().body[checkpoint.statement],
                    "module initialization references a function before it is available",
                ));
            }
            if reads.iter().any(|&slot| slot >= checkpoint.initialized) {
                return Err(module.error(
                    &module.parsed.syntax().body[checkpoint.statement],
                    "module initialization may read a value before it is initialized",
                ));
            }
        }
        Ok(())
    }
}

fn collect(
    body: &[Statement],
    reads: &mut HashSet<usize>,
    calls: &mut Vec<usize>,
    references: &mut Vec<usize>,
    targets: &HashMap<Type, HashSet<usize>>,
) {
    ir::walk_expressions(body, &mut |value| match &value.kind {
        ExprKind::Builtin(Builtin::GlobalGet(slot), _) => {
            reads.insert(*slot);
        }
        ExprKind::Call(id, _)
        | ExprKind::Builtin(
            Builtin::Process(
                ir::ProcessOperation::Coroutine { function: id, .. }
                | ir::ProcessOperation::ContextNew { function: id },
            ),
            _,
        )
        | ExprKind::Construct {
            initializer: id, ..
        } => calls.push(*id),
        ExprKind::Callable { function, .. } => references.push(*function),
        _ if callables::invoked_signature(value).is_some() => {
            if let Some(targets) =
                callables::invoked_signature(value).and_then(|ty| targets.get(&ty))
            {
                calls.extend(targets);
            }
        }
        _ => {}
    });
}
