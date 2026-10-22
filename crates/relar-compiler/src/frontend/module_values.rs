//! Fixed module bindings with ordinary mutable values and ordered initialization.

use std::cell::RefCell;
use std::ops::Range;

use super::*;

#[derive(Default)]
pub(super) struct ModuleValues {
    definitions: RefCell<HashMap<DefinitionId, (usize, Type)>>,
    types: RefCell<Vec<Type>>,
    pub(super) regex_literals: RefCell<HashMap<String, regex::CaptureNames>>,
    regex_globals: RefCell<HashMap<usize, regex::CaptureNames>>,
}

impl ModuleValues {
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

    pub(super) fn regex_groups(&self, pattern: &Expr) -> Option<regex::CaptureNames> {
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
                let ty = annotation(
                    &assignment.annotation,
                    self.graph,
                    self.module,
                    &self.classes.declarations,
                    self.types,
                )?;
                self.expr_expected(value, ty)?
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

    /// A standalone entry script can declare fixed patterns between its imports
    /// and definitions. Their literal initializers cannot call first-party code.
    pub(super) fn entry_patterns(&mut self) -> Result<Vec<Statement>, String> {
        let module = &self.graph.modules[self.module];
        module
            .entry_patterns
            .iter()
            .map(|&statement| {
                let definition = DefinitionId {
                    module: self.module,
                    statement,
                };
                self.initialize_global(&module.parsed.syntax().body[statement], definition)
            })
            .collect()
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
    ) -> Result<Self, String> {
        checker.module_scope = true;
        let module = &checker.graph.modules[checker.module];
        let mut body = Vec::new();
        let mut checkpoints = Vec::new();
        for (index, statement) in module.parsed.syntax().body.iter().enumerate() {
            let definition = DefinitionId {
                module: checker.module,
                statement: index,
            };
            match statement {
                ast::Stmt::FunctionDef(_) | ast::Stmt::ClassDef(_) => {
                    available.insert(definition);
                    continue;
                }
                ast::Stmt::Import(_) | ast::Stmt::ImportFrom(_) => continue,
                _ => {}
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
        callables: &[DefinitionId],
        targets: &HashMap<Type, HashSet<usize>>,
    ) -> Result<(), String> {
        let module = &graph.modules[self.module];
        for checkpoint in &self.checkpoints {
            let mut reads = HashSet::new();
            let mut calls = Vec::new();
            let mut references = Vec::new();
            let mut visited = HashSet::new();
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
                collect(
                    &functions[id].body,
                    &mut reads,
                    &mut calls,
                    &mut references,
                    targets,
                );
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
        | ExprKind::Construct {
            initializer: id, ..
        } => calls.push(*id),
        ExprKind::Callable { function, .. } => references.push(*function),
        ExprKind::Invoke { callable, .. } => {
            if let Some(targets) = targets.get(&callable.ty) {
                calls.extend(targets);
            }
        }
        _ => {}
    });
}
