//! Module values with ordered initialization and typed storage.

use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;

use super::*;

type ClassLiteral = (Option<ModuleId>, Rc<[String]>);

#[derive(Default)]
pub(super) struct ModuleValues {
    /// A reached initializer may still be waiting for a callee during this inference round.
    waiting: RefCell<HashMap<DefinitionId, Failure>>,
    pub(super) registered_initializers: bool,
    /// A concrete source read supplies the registered function's ordinary Python binding.
    pub(super) missing_bindings: RefCell<std::collections::BTreeMap<usize, Diagnostic>>,
    pub(super) class_data: RefCell<class_data::Storage>,
    definitions: RefCell<HashMap<(DefinitionId, usize), (usize, Type)>>,
    list_origins: RefCell<HashMap<usize, DefinitionId>>,
    class_slots: RefCell<HashMap<(DefinitionId, usize), usize>>,
    pub(super) unittest_skips: RefCell<HashMap<(DefinitionId, usize), (usize, usize)>>,
    literal_tuples: RefCell<HashMap<ClassLiteral, usize>>,
    types: RefCell<Vec<Type>>,
    regex_globals: RefCell<HashMap<usize, regex::CaptureLayout>>,
    /// Physical functions published in a module slot, in execution order.
    slot_functions: RefCell<HashMap<usize, Vec<usize>>>,
}

impl ModuleValues {
    pub(super) fn for_initializers(registered_initializers: bool) -> Self {
        Self {
            registered_initializers,
            ..Self::default()
        }
    }

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
                        self.class_slots
                            .borrow_mut()
                            .insert((definition, ordinal), existing);
                        return None;
                    }
                }
                slots.push(ty);
                assert!(
                    self.class_slots
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

    pub(super) fn class_value(&self, owner: DefinitionId, ordinal: usize) -> Option<Expr> {
        let slot = *self.class_slots.borrow().get(&(owner, ordinal))?;
        Some(self.slot_value(slot))
    }

    pub(super) fn slot_value(&self, slot: usize) -> Expr {
        Expr {
            ty: self.types.borrow()[slot],
            kind: ExprKind::Builtin(Builtin::GlobalGet(slot), vec![]),
        }
    }

    pub(super) fn list_origin(&self, value: &Expr) -> Option<DefinitionId> {
        match &value.kind {
            ExprKind::Builtin(Builtin::GlobalGet(slot), _) => {
                self.list_origins.borrow().get(slot).copied()
            }
            _ => None,
        }
    }

    fn publish_function(&self, slot: usize, function: usize) {
        self.slot_functions
            .borrow_mut()
            .entry(slot)
            .or_default()
            .push(function);
    }

    fn stored_functions(&self, value: &Expr) -> Option<std::cell::Ref<'_, Vec<usize>>> {
        let ExprKind::Builtin(Builtin::GlobalGet(slot), _) = &value.kind else {
            return None;
        };
        std::cell::Ref::filter_map(self.slot_functions.borrow(), |slots| slots.get(slot)).ok()
    }

    fn saved_function(&self, value: &Expr) -> Option<usize> {
        match &value.kind {
            ExprKind::Builtin(Builtin::StaticFunction(function), _) => Some(*function),
            _ => self.stored_functions(value)?.last().copied(),
        }
    }

    pub(super) fn invoked_functions(&self, value: &Expr) -> Option<std::cell::Ref<'_, Vec<usize>>> {
        let ExprKind::Invoke { callable, .. } = &value.kind else {
            return None;
        };
        self.stored_functions(callable)
    }

    pub(super) fn reserve(&self, ty: Type) -> usize {
        let mut slots = self.types.borrow_mut();
        let next = slots.len();
        slots.push(ty);
        next
    }

    /// Resolves storage only after the defining initializer has been checked.
    fn read(
        &self,
        definition: DefinitionId,
        member: usize,
    ) -> Result<Option<(usize, Type)>, String> {
        if let Some(wait) = self.waiting.borrow().get(&definition) {
            return Err(wait.clone());
        }
        Ok(self
            .definitions
            .borrow()
            .get(&(definition, member))
            .copied())
    }

    pub fn get(&self, definition: DefinitionId) -> Option<(usize, Type)> {
        self.definitions.borrow().get(&(definition, 0)).copied()
    }

    fn insert(&self, definition: DefinitionId, ty: Type) -> usize {
        self.insert_member(definition, 0, ty)
    }

    fn insert_member(&self, definition: DefinitionId, member: usize, ty: Type) -> usize {
        let mut types = self.types.borrow_mut();
        let slot = types.len();
        types.push(ty);
        assert!(
            self.definitions
                .borrow_mut()
                .insert((definition, member), (slot, ty))
                .is_none()
        );
        slot
    }

    pub(super) fn types(&self) -> std::cell::Ref<'_, Vec<Type>> {
        self.types.borrow()
    }

    pub fn into_types(self) -> Vec<Type> {
        self.types.into_inner()
    }

    pub(super) fn regex_groups(&self, pattern: &Expr) -> Option<regex::CaptureLayout> {
        match &pattern.kind {
            ExprKind::Builtin(Builtin::RegexCompile { pattern, .. }, _) => {
                Some(regex::CaptureLayout(pattern.clone()))
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
    binding_reads: HashSet<DefinitionId>,
}

impl Checkpoint {
    fn record(
        output: Option<&mut Vec<Self>>,
        statement: usize,
        body: Range<usize>,
        initialized: usize,
        available: &HashSet<DefinitionId>,
        reads: &mut HashSet<DefinitionId>,
    ) {
        let binding_reads = std::mem::take(reads);
        if let Some(checkpoints) = output {
            checkpoints.push(Self {
                statement,
                body,
                initialized,
                definitions: available.clone(),
                binding_reads,
            });
        }
    }
}

impl Checker<'_> {
    pub(super) fn read_module_value(
        &self,
        definition: DefinitionId,
        member: Option<usize>,
        at: &impl Ranged,
    ) -> Result<Expr, String> {
        let (slot, ty) = self
            .globals
            .read(definition, member.unwrap_or(0))?
            .ok_or_else(|| {
                error(
                    self.source,
                    at,
                    "module value is not initialized on this path",
                )
            })?;
        let storage = self.globals.slot_value(slot);
        if member.is_none()
            && !matches!(ty, Type::Callable(_))
            && self.graph.modules[definition.module]
                .rebound_values
                .contains(&definition.statement)
        {
            return Ok(Expr {
                ty: self.types.element(ty).expect("module cell"),
                kind: ExprKind::Builtin(Builtin::ListGet(None), vec![storage, Expr::integer(0)]),
            });
        }
        Ok(storage)
    }

    /// Execute assignments in the class namespace at the definition's source position.
    /// A class local is visible to later initializers, but inherited names are not.
    fn initialize_class_values(
        &mut self,
        definition: DefinitionId,
        class_id: usize,
        available: &HashSet<DefinitionId>,
        mut body: Option<&mut Vec<Statement>>,
        mut checkpoints: Option<&mut Vec<Checkpoint>>,
    ) -> Result<(), String> {
        use crate::classes::ClassValue;
        let class = &self.classes.declarations.definitions[class_id];
        for (index, skip) in class.skip_decorators.iter().enumerate() {
            if !self.unittest_case(class_id) {
                return Err(error(
                    self.source,
                    class.node.as_ref(),
                    "native unittest class skip decorators require TestCase",
                ));
            }
            let start = body.as_deref().map_or(0, Vec::len);
            let initialized = self.globals.types.borrow().len();
            let condition = self.class_skip_condition(&skip.condition)?;
            self.expect(Type::Bool, condition.ty, &skip.condition)?;
            let reason = self.expr(&skip.reason)?;
            self.expect(Type::Str, reason.ty, &skip.reason)?;
            let flag_slot = self.globals.reserve(Type::Bool);
            let reason_slot = self.globals.reserve(Type::Str);
            self.globals
                .unittest_skips
                .borrow_mut()
                .insert((definition, index), (flag_slot, reason_slot));
            if let Some(body) = body.as_deref_mut() {
                let flag = if skip.unless {
                    Expr::not(condition)
                } else {
                    condition
                };
                body.push(Statement::store_global(flag_slot, flag));
                body.push(Statement::store_global(reason_slot, reason));
            }
            Checkpoint::record(
                checkpoints.as_deref_mut(),
                definition.statement,
                start..body.as_deref().map_or(0, Vec::len),
                initialized,
                available,
                self.class_reads.get_mut(),
            );
        }
        if !class
            .static_bindings
            .values()
            .any(|v| matches!(v, ClassValue::Runtime { owner, .. } if *owner == definition))
        {
            return self.inherit_class_data(class_id, body);
        }
        let mut locals = Vec::new();
        for (ordinal, stmt) in class.node.body.iter().enumerate() {
            let ast::Stmt::Assign(assign) = stmt else {
                continue;
            };
            let [ast::Expr::Name(target)] = &*assign.targets else {
                continue;
            };
            let name = target.id.as_str();
            let start = body.as_deref().map_or(0, Vec::len);
            let initialized = self.globals.types.borrow().len();
            let binding = class.static_bindings.get(name);
            let value = match binding {
                Some(ClassValue::Runtime { owner, ordinal: at })
                    if *owner == definition && *at == ordinal =>
                {
                    if class_scoped_child(&assign.value)
                        && !(matches!(&*assign.value, ast::Expr::DictComp(_))
                            && !ast::helpers::any_over_expr(&assign.value, |expr| {
                                expr.as_name_expr().is_some_and(|n| {
                                    self.class_lexical.contains(n.id.as_str())
                                        || n.id == "__class__"
                                })
                            }))
                    {
                        return Err(error(
                            self.source,
                            assign.value.as_ref(),
                            "nested scopes in a class initializer are unsupported",
                        ));
                    }
                    let value = self.expr(&assign.value)?;
                    if !class_data_type(value.ty, self.types) {
                        return Err(error(
                            self.source,
                            assign.value.as_ref(),
                            "a shared class attribute requires plain scalar or container values",
                        ));
                    }
                    value
                }
                Some(ClassValue::String(text)) => unpacking::string(text),
                Some(ClassValue::Strings { owner, ordinal, .. }) => {
                    self.globals.class_value(*owner, *ordinal).unwrap()
                }
                _ => continue,
            };
            let temporary = self.hidden(value.ty);
            if let Some(body) = body.as_deref_mut() {
                body.push(Statement::Assign(temporary, value));
            }
            if matches!(binding, Some(ClassValue::Runtime { .. })) {
                let value =
                    self.class_storage_value(class_id, name, self.local(temporary), assign)?;
                let slot = self.globals.reserve(value.ty);
                self.globals
                    .class_slots
                    .borrow_mut()
                    .insert((definition, ordinal), slot);
                self.record_class_cell(class_id, name, slot, value.ty);
                if let Some(body) = body.as_deref_mut() {
                    body.push(Statement::store_global(slot, value));
                }
            }
            let previous = self.names.insert(name.to_owned(), temporary);
            self.class_lexical.insert(name.to_owned());
            self.assigned.insert(temporary);
            locals.push((name.to_owned(), temporary, previous));
            Checkpoint::record(
                checkpoints.as_deref_mut(),
                definition.statement,
                start..body.as_deref().map_or(0, Vec::len),
                initialized,
                available,
                self.class_reads.get_mut(),
            );
        }
        for (name, slot, previous) in locals.into_iter().rev() {
            self.class_lexical.remove(&name);
            if let Some(old) = previous {
                self.names.insert(name, old);
            } else {
                self.names.remove(&name);
            }
            self.assigned.remove(&slot);
            if let Some(body) = body.as_deref_mut() {
                body.push(Statement::Clear(slot));
            }
        }
        self.inherit_class_data(class_id, body)
    }

    pub(super) fn replace_global(
        &mut self,
        first: DefinitionId,
        statement: &ast::Stmt,
    ) -> Result<Statement, String> {
        let (slot, storage) = self.globals.get(first).ok_or_else(|| {
            error(
                self.source,
                statement,
                "module value is not initialized on this path",
            )
        })?;
        let ty = self.types.element(storage).expect("module cell");
        let value = match statement {
            ast::Stmt::Assign(assign) => self.expr_expected(&assign.value, ty)?,
            _ => {
                let value = self.module_assignment(statement)?;
                self.coerce(value, ty, statement)?
            }
        };
        Ok(Statement::assign_item(
            self.globals.slot_value(slot),
            Expr::integer(0),
            value,
        ))
    }

    fn module_assignment(&mut self, statement: &ast::Stmt) -> Result<Expr, String> {
        match statement {
            ast::Stmt::Assign(assignment) => self.expr(&assignment.value),
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
                    self.expr(value)
                } else {
                    let ty = annotation(
                        &assignment.annotation,
                        self.graph,
                        self.module,
                        self.classes.declarations,
                        self.types,
                    )?;
                    self.expr_expected(value, ty)
                }
            }
            _ => unreachable!(),
        }
    }

    fn initialize_global(
        &mut self,
        statement: &ast::Stmt,
        definition: DefinitionId,
        mut body: Option<&mut Vec<Statement>>,
    ) -> Result<(), String> {
        use crate::modules::ImportSnapshot;
        let snapshot = self.graph.modules[self.module]
            .snapshots
            .get(&definition.statement);
        if let Some(ImportSnapshot::Values(sources)) = snapshot {
            // Read every source before allocating slots: inference can retry a pending initializer.
            let values = sources
                .iter()
                .map(|&source| self.read_module_value(source, None, statement))
                .collect::<Result<Vec<_>, _>>()?;
            for (member, value) in values.into_iter().enumerate() {
                let slot = self.globals.insert_member(definition, member, value.ty);
                if let Some(origin) = self.globals.list_origin(&value) {
                    self.globals.list_origins.borrow_mut().insert(slot, origin);
                }
                if let Some(target) = self.globals.saved_function(&value) {
                    self.globals.publish_function(slot, target);
                }
                if let Some(body) = body.as_deref_mut() {
                    body.push(Statement::store_global(slot, value));
                }
            }
            return Ok(());
        }
        let rebound = self.graph.modules[self.module].rebound_target(self.module, statement);
        if let Some(first) = rebound
            && first != definition
        {
            let store = self.replace_global(first, statement)?;
            if let Some(body) = body {
                body.push(store);
            }
            return Ok(());
        }
        let parser = if rebound.is_some() {
            None
        } else {
            self.saved_datetime_parser(statement, calendar::ParserBinding::Global(definition))
        };
        let exact_parser = parser.is_some();
        let value = if let Some((_, value)) = parser {
            value
        } else if let Some(ImportSnapshot::Intrinsic(symbol)) = snapshot {
            self.import_sys_value(*symbol)
        } else if self.types.module_list_uses_strings(definition) == Some(true)
            && let ast::Stmt::Assign(assign) = statement
        {
            self.expr_expected(&assign.value, self.types.list(Type::Str))?
        } else {
            self.module_assignment(statement)?
        };
        // A native frozenset predicate captures only its already-initialized immutable set.
        let frozen_predicate = matches!(&value.kind, ExprKind::Callable { receiver: Some(set), .. }
            if set.ty == Type::FrozenStrings);
        let saved = self
            .globals
            .saved_function(&value)
            .filter(|_| rebound.is_none());
        if !exact_parser
            && !frozen_predicate
            && saved.is_none()
            && callables::contains_callable(value.ty, self.types)
        {
            return Err(error(
                self.source,
                statement,
                "imported module values cannot contain callable values",
            )
            .classify(Kind::UnsupportedRepresentation));
        }
        let value = if rebound.is_some() {
            if !class_data_type(value.ty, self.types)
                && !matches!(
                    self.types.payload(value.ty).unwrap_or(value.ty),
                    Type::Class(_)
                )
            {
                return Err(error(
                    self.source,
                    statement,
                    "a rebindable module value requires plain scalar or container storage",
                )
                .classify(Kind::UnsupportedRepresentation));
            }
            Expr {
                ty: self.types.list(value.ty),
                kind: ExprKind::List(vec![value]),
            }
        } else {
            value
        };
        let slot = self.globals.insert(definition, value.ty);
        if let Some(origin) = self
            .types
            .module_list_uses_strings(definition)
            .map(|_| definition)
            .or_else(|| match statement {
                ast::Stmt::Assign(_) => self.globals.list_origin(&value),
                _ => None,
            })
        {
            self.globals.list_origins.borrow_mut().insert(slot, origin);
        }
        if let Some(target) = saved {
            self.globals.publish_function(slot, target);
        }
        if let Some(groups) = self.globals.regex_groups(&value) {
            self.globals.regex_globals.borrow_mut().insert(slot, groups);
        }
        if let Some(body) = body {
            body.push(Statement::store_global(slot, value));
        }
        Ok(())
    }
}

fn class_data_type(ty: Type, types: &Types) -> bool {
    match ty {
        Type::None
        | Type::Bool
        | Type::Int
        | Type::Float
        | Type::Str
        | Type::Bytes
        | Type::IntEnum(_)
        | Type::ListInt => true,
        Type::List(_) => types.element(ty).is_some_and(|t| class_data_type(t, types)),
        Type::Set(_) => types
            .set_element(ty)
            .is_some_and(|t| class_data_type(t, types)),
        Type::Tuple(_) if types.tuple_origin(ty).is_none() => types
            .fields(ty)
            .is_some_and(|fields| fields.into_iter().all(|t| class_data_type(t, types))),
        Type::Optional(_) => types.payload(ty).is_some_and(|t| class_data_type(t, types)),
        Type::Dict(_) => types
            .mapping(ty)
            .is_some_and(|(k, v)| class_data_type(k, types) && class_data_type(v, types)),
        _ => false,
    }
}

fn class_scoped_child(value: &ast::Expr) -> bool {
    use ruff_python_ast::visitor::{Visitor, walk_expr};
    struct Nested(bool);
    impl<'a> Visitor<'a> for Nested {
        fn visit_expr(&mut self, node: &'a ast::Expr) {
            if matches!(
                node,
                ast::Expr::Lambda(_)
                    | ast::Expr::ListComp(_)
                    | ast::Expr::SetComp(_)
                    | ast::Expr::DictComp(_)
                    | ast::Expr::Generator(_)
                    | ast::Expr::Named(_)
            ) {
                self.0 = true;
            } else if !self.0 {
                walk_expr(self, node);
            }
        }
    }
    let mut nested = Nested(false);
    nested.visit_expr(value);
    nested.0
}

pub(super) struct Initialization {
    pub function: ir::Function,
    pub calls: HashSet<usize>,
    pub order: InitializationOrder,
}

/// Source-order checks index the original statements of the module initializer.
pub(super) struct InitializationOrder {
    pub module: ModuleId,
    checkpoints: Vec<Checkpoint>,
}

impl Initialization {
    /// Infers values in execution order while deferring callable body checks.
    pub fn lower(
        mut checker: Checker<'_>,
        available: &mut HashSet<DefinitionId>,
        range: Range<usize>,
        output: Option<&mut Vec<Self>>,
    ) -> Result<(), String> {
        checker.module_scope = true;
        let module = &checker.graph.modules[checker.module];
        let mut body = Vec::new();
        let mut checkpoints = output.as_ref().map(|_| Vec::new());
        let mut pending = None;
        let complete = range.end == module.parsed.syntax().body.len();
        for index in range {
            // Inference and registered modules need the effects on bindings and types.
            // Keep only the current statement's work until it has supplied those facts.
            if output.is_none() {
                body.clear();
            }
            let statement = &module.parsed.syntax().body[index];
            let definition = DefinitionId {
                module: checker.module,
                statement: index,
            };
            if module.import_config_slot(statement).is_some()
                || checker.graph.unused_literal_global(definition)
            {
                continue;
            }
            if let Some(checkpoints) = &mut checkpoints {
                let mut imports = HashSet::new();
                checker.graph.ordered_reads(
                    checker.module,
                    std::slice::from_ref(statement),
                    false,
                    &mut imports,
                );
                if !imports.is_empty() {
                    checkpoints.push(Checkpoint {
                        statement: index,
                        body: body.len()..body.len(),
                        initialized: checker.globals.types.borrow().len(),
                        definitions: available.clone(),
                        binding_reads: imports,
                    });
                }
            }
            if module.type_guards.binary_search(&index).is_ok()
                || local_functions::type_variable(checker.graph, checker.module, statement)
            {
                continue;
            }
            if checker.graph.has_ordered_imports()
                && matches!(statement, ast::Stmt::Import(_) | ast::Stmt::ImportFrom(_))
            {
                available.insert(definition);
            }
            if let Some(site) = module.registered_imports.get(&index) {
                checker.require_registered_operation(site)?;
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
                        checker.initialize_class_values(
                            definition,
                            class,
                            available,
                            output.is_some().then_some(&mut body),
                            checkpoints.as_mut(),
                        )?;
                    }
                    available.insert(definition);
                    continue;
                }
                ast::Stmt::FunctionDef(function) => {
                    available.insert(definition);
                    if let Some(first) = module.function_binding(checker.module, function) {
                        let Some(signature) = checker.functions.get(&definition) else {
                            continue;
                        };
                        let value = checker.callable_value(signature, None, None, statement)?;
                        let slot = if first == definition {
                            checker.globals.insert(first, value.ty)
                        } else {
                            let (slot, ty) = checker.globals.get(first).ok_or_else(|| {
                                error(
                                    checker.source,
                                    statement,
                                    "the preceding declaration is unavailable",
                                )
                            })?;
                            if ty != value.ty {
                                return Err(error(
                                    checker.source,
                                    statement,
                                    "repeated definitions require a common fixed callable signature",
                                ));
                            }
                            slot
                        };
                        checker.globals.publish_function(slot, signature.id);
                        if output.is_some() {
                            body.push(Statement::store_global(slot, value));
                        }
                    }
                    if inspect::marks_function(checker.graph, checker.module, function)
                        && let Some(signature) = checker.functions.get(&definition)
                    {
                        let original = checker.callable_value(signature, None, None, statement)?;
                        body.push(Statement::Expr(Expr {
                            ty: original.ty,
                            kind: ExprKind::Builtin(
                                Builtin::MarkCoroutine(signature.id),
                                vec![original],
                            ),
                        }));
                    }
                    if module.overloads.contains(&index) {
                        let signature = &checker.functions[&definition];
                        let original = checker.callable_value(signature, None, None, statement)?;
                        body.push(Statement::Expr(Expr {
                            ty: Type::OverloadStub,
                            kind: ExprKind::Builtin(
                                Builtin::Typing(ir::TypingOperation::Register),
                                vec![original],
                            ),
                        }));
                    }
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
                    Checkpoint::record(
                        checkpoints.as_mut(),
                        index,
                        start..body.len(),
                        checker.globals.types.borrow().len(),
                        available,
                        checker.class_reads.get_mut(),
                    );
                    continue;
                }
                ast::Stmt::Import(_) | ast::Stmt::ImportFrom(_)
                    if !module.snapshots.contains_key(&index) =>
                {
                    continue;
                }
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
                && !module.snapshots.contains_key(&index)
                && module.entry_globals.binary_search(&index).is_err()
            {
                continue;
            }
            let initialized = checker.globals.types.borrow().len();
            let start = body.len();
            if module.snapshots.contains_key(&index)
                || crate::modules::value_name(statement).is_some()
            {
                let captured = match module.snapshots.get(&index) {
                    Some(crate::modules::ImportSnapshot::Values(sources)) => Some(sources),
                    _ => None,
                };
                match checker.initialize_global(
                    statement,
                    definition,
                    output.is_some().then_some(&mut body),
                ) {
                    Ok(()) => {}
                    Err(wait @ Failure::PendingSignature { .. }) => {
                        checker
                            .globals
                            .waiting
                            .borrow_mut()
                            .insert(definition, wait.clone());
                        pending.get_or_insert(wait);
                    }
                    // The binding exists even when its initializer needs Python storage.
                    // Preserve a real use-before-initialization error for unreached stores.
                    Err(error) if checker.missing_operation(&error) => {
                        if checker.globals.get(definition).is_none() {
                            let ty = if module.rebound_values.contains(&index) {
                                checker.types.list(Type::PythonObject)
                            } else {
                                Type::PythonObject
                            };
                            for member in 0..captured.map_or(1, Vec::len) {
                                checker.globals.insert_member(definition, member, ty);
                            }
                        }
                    }
                    Err(error) if checker.defer_object_error(&error) => continue,
                    Err(error) => {
                        return Err(
                            error.in_entry_statement(checker.ordinary_entry(), statement.start())
                        );
                    }
                }
            } else {
                let falls_through = checker.statement(statement, &mut body)?;
                // An imported module's __main__ block must type-check, but it
                // cannot read definitions that are still being initialized.
                if checker.module != checker.graph.entry
                    && !module.bindings.contains_key("__name__")
                    && inactive_main_guard(statement)
                {
                    checker.class_reads.get_mut().clear();
                    body.truncate(start);
                    continue;
                }
                checker.reachable &= falls_through;
            }
            if checker.fault_enabled() && body.len() > start {
                body.insert(start, checker.fault_line(statement));
            }
            Checkpoint::record(
                checkpoints.as_mut(),
                index,
                start..body.len(),
                initialized,
                available,
                checker.class_reads.get_mut(),
            );
        }
        if complete && checker.graph.has_ordered_imports() {
            available.insert(DefinitionId {
                module: checker.module,
                statement: module.parsed.syntax().body.len(),
            });
        }
        checker.finish_region(pending.map_or(Ok(()), Err))?;
        if let (Some(output), Some(checkpoints)) = (output, checkpoints) {
            output.push(Self {
                order: InitializationOrder {
                    module: checker.module,
                    checkpoints,
                },
                function: ir::Function {
                    params: Vec::new(),
                    returns: Type::None,
                    locals: checker.locals,
                    body,
                },
                calls: checker.calls,
            });
        }
        Ok(())
    }
}

impl ModuleValues {
    /// Rejects early direct and transitive reads, including hidden comprehensions.
    /// Requiring every potential callee to be available is deliberately conservative.
    pub(super) fn validate_initializers(
        &self,
        initializers: impl IntoIterator<Item = (usize, InitializationOrder)>,
        graph: &SourceGraph,
        functions: &[ir::Function],
        binding_reads: &[HashSet<DefinitionId>],
        callables: &[DefinitionId],
        targets: &HashMap<Type, HashSet<usize>>,
    ) -> std::result::Result<(), String> {
        for (initializer, order) in initializers {
            let module = &graph.modules[order.module];
            for checkpoint in &order.checkpoints {
                let mut reads = HashSet::new();
                let mut calls = Vec::new();
                let mut references = Vec::new();
                let mut visited = HashSet::new();
                let mut required_bindings = checkpoint.binding_reads.clone();
                // A deferred reader sees the most recently executed declaration. An immutable
                // snapshot has just the one physical target it copied at its own store.
                let current = |value: &Expr| {
                    self.invoked_functions(value)?
                        .iter()
                        .rev()
                        .copied()
                        .find(|&id| checkpoint.definitions.contains(&callables[id]))
                };
                collect(
                    &functions[initializer].body[checkpoint.body.clone()],
                    &mut reads,
                    &mut calls,
                    &mut references,
                    targets,
                    &current,
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
                    required_bindings.extend(&binding_reads[id]);
                    collect(
                        &functions[id].body,
                        &mut reads,
                        &mut calls,
                        &mut references,
                        targets,
                        &current,
                    );
                }
                if let Some(read) = required_bindings
                    .difference(&checkpoint.definitions)
                    .min_by_key(|read| (read.module, read.statement))
                {
                    let reason = match graph.modules[read.module]
                        .parsed
                        .syntax()
                        .body
                        .get(read.statement)
                    {
                        None | Some(ast::Stmt::Import(_) | ast::Stmt::ImportFrom(_)) => {
                            "module initialization references an import before its binding is available"
                        }
                        Some(ast::Stmt::FunctionDef(_)) => {
                            "module initialization references a function before it is available"
                        }
                        _ => "module initialization references a class before it is available",
                    };
                    return Err(
                        module.error(&module.parsed.syntax().body[checkpoint.statement], reason)
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
        }
        Ok(())
    }
}

fn inactive_main_guard(stmt: &ast::Stmt) -> bool {
    let ast::Stmt::If(branch) = stmt else {
        return false;
    };
    let ast::Expr::Compare(test) = branch.test.as_ref() else {
        return false;
    };
    branch.elif_else_clauses.is_empty()
        && matches!(test.ops.as_ref(), [ast::CmpOp::Eq])
        && matches!(test.operands.as_ref(),
            [ast::Expr::Name(name), ast::Expr::StringLiteral(text)]
            | [ast::Expr::StringLiteral(text), ast::Expr::Name(name)]
            if name.id == "__name__" && text.value.to_str() == "__main__")
}

fn collect(
    body: &[Statement],
    reads: &mut HashSet<usize>,
    calls: &mut Vec<usize>,
    references: &mut Vec<usize>,
    targets: &HashMap<Type, HashSet<usize>>,
    current: &impl Fn(&Expr) -> Option<usize>,
) {
    ir::walk_expressions(body, &mut |value| match &value.kind {
        ExprKind::Builtin(Builtin::GlobalGet(slot), _) => {
            reads.insert(*slot);
        }
        ExprKind::Builtin(Builtin::FormatField(field), _) => {
            calls.extend(field.repr_methods.iter().copied());
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
            if let Some(id) = current(value) {
                calls.push(id);
            } else if let Some(targets) =
                callables::invoked_signature(value).and_then(|ty| targets.get(&ty))
            {
                calls.extend(targets);
            }
        }
        _ => {}
    });
}
