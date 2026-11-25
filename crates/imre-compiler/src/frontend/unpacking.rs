//! Assignment targets consume one owned RHS, with a snapshot at each unpacking
//! level so target mutation cannot change values already removed from a sequence.

use super::*;

/// Calls an existing typed primitive from compiler-generated unpacking code.
fn builtin(ty: Type, kind: Builtin, args: Vec<Expr>) -> Expr {
    Expr {
        ty,
        kind: ExprKind::Builtin(kind, args),
    }
}

/// Tests sequence length before any user target can observe a partial unpack.
fn comparison(left: Expr, op: Compare, right: Expr) -> Expr {
    Expr {
        ty: Type::Bool,
        kind: ExprKind::Compare(vec![left, right], vec![op.operator()]),
    }
}

/// Materializes a constant fragment of Python's unpacking diagnostics.
pub(super) fn string(value: impl Into<String>) -> Expr {
    Expr {
        ty: Type::Str,
        kind: ExprKind::Str(value.into()),
    }
}

/// Joins diagnostic fragments only on the corresponding failure path.
pub(super) fn concat(left: Expr, right: Expr) -> Expr {
    Expr {
        ty: Type::Str,
        kind: ExprKind::Binary(Binary::Add, Box::new(left), Box::new(right)),
    }
}

/// Uses the sealed ValueError category so ordinary handlers catch arity errors.
fn unpack_error(message: Expr) -> Statement {
    Statement::raise(Expr {
        ty: Type::Exception,
        kind: ExprKind::Exception {
            kind: exception_kind("ValueError").unwrap(),
            message: Some(Box::new(message)),
        },
    })
}

/// Target traversal and operand lifetimes are shared by checked and namespace suites.
/// A checked sequence can specialize its arity checks and element projections.
pub(super) trait PatternBinder {
    type Error;

    fn pattern_hidden(&mut self, ty: Type) -> usize;
    fn pattern_local(&self, slot: usize) -> Expr;
    fn pattern_capture(&mut self, value: &Expr) -> usize {
        self.pattern_hidden(value.ty)
    }
    fn pattern_store(
        &mut self,
        target: &ast::Expr,
        value: Expr,
        output: &mut Vec<Statement>,
    ) -> StdResult<(), Self::Error>;
    fn multiple_stars(&self, target: &ast::Expr) -> Self::Error;

    fn pattern_snapshot(
        &mut self,
        target: &ast::Expr,
        elements: &[ast::Expr],
        star: Option<usize>,
        value: Expr,
        row: Option<Row>,
    ) -> StdResult<PatternSnapshot, Self::Error> {
        Ok(protocol_snapshot(
            self,
            target.range(),
            elements.iter().map(|_| Type::PythonObject),
            star,
            value,
            Type::PythonObject,
            row,
        ))
    }
}

pub(super) struct PatternSnapshot {
    body: Vec<Statement>,
    // Captured target slots follow the temporaries used to evaluate and check the input.
    clear: Vec<usize>,
}

impl PatternSnapshot {
    fn from_items(binder: &mut (impl PatternBinder + ?Sized), items: Vec<Expr>) -> Self {
        let mut snapshot = Self {
            body: Vec::with_capacity(items.len()),
            clear: Vec::with_capacity(items.len()),
        };
        for item in items {
            snapshot.capture(binder, item);
        }
        snapshot
    }

    fn capture(&mut self, binder: &mut (impl PatternBinder + ?Sized), item: Expr) {
        let slot = binder.pattern_capture(&item);
        self.body.push(Statement::Assign(slot, item));
        self.clear.push(slot);
    }
}

/// Take the original operands of a fixed tuple through its diagnostic source positions.
fn tuple_operands(mut value: &mut Expr, count: usize) -> Option<Vec<Expr>> {
    loop {
        match &mut value.kind {
            ExprKind::Builtin(Builtin::RegisteredLocation(..), args) => {
                let [inner] = args.as_mut_slice() else {
                    return None;
                };
                value = inner;
            }
            ExprKind::Tuple(items)
                if items.len() == count
                    && items.iter().all(|item| item.spread_operand().is_none())
                    && !items.iter().all(ir::Constant::is_constant) =>
            {
                return Some(std::mem::take(items));
            }
            _ => return None,
        }
    }
}

fn protocol_snapshot(
    binder: &mut (impl PatternBinder + ?Sized),
    target: TextRange,
    item_types: impl ExactSizeIterator<Item = Type>,
    star: Option<usize>,
    mut value: Expr,
    sequence_type: Type,
    mut row: Option<Row>,
) -> PatternSnapshot {
    if row.is_none()
        && star.is_none()
        && (1..=3).contains(&item_types.len())
        && let Some(items) = tuple_operands(&mut value, item_types.len())
    {
        return PatternSnapshot::from_items(binder, items);
    }
    // Project directly into the two output slots of the prepared pull. Keep its
    // cursor and its original exhaustion edge, including after generator startup.
    if item_types.len() == 2
        && star.is_none()
        && let Some(Row::Pulled(scope)) = &mut row
        && let [
            Statement::Clear(clear),
            Statement::IterNext {
                kind,
                iterator,
                key,
                value: second_slot @ None,
                ready,
            },
            exhausted,
        ] = scope.body.as_mut_slice()
        && *kind == ir::IterNextKind::Generic
        && *key == *clear
        && scope.clear.as_slice() == [*clear]
        && exhausted.branches_for_flag(*ready).is_some()
        && binder.pattern_local(*iterator).ty == Type::PythonObject
    {
        let mut slots = item_types.map(|ty| binder.pattern_hidden(ty));
        let first = slots.next().unwrap();
        let second = slots.next().unwrap();
        *kind = ir::IterNextKind::Pair(target);
        *key = first;
        *second_slot = Some(second);
        scope.body.remove(0);
        return PatternSnapshot {
            body: std::mem::take(&mut scope.body),
            clear: vec![first, second],
        };
    }
    let sequence = builtin(
        sequence_type,
        Builtin::RegisteredUnpack {
            count: item_types.len(),
            star,
            target: Some(target),
        },
        vec![value],
    );
    let root = binder.pattern_hidden(sequence.ty);
    let mut snapshot = PatternSnapshot {
        body: vec![Statement::Assign(root, consume_row(sequence, row))],
        clear: vec![root],
    };
    for (index, ty) in item_types.enumerate() {
        // Python packs the starred remainder into a list even when its items are opaque.
        let item = builtin(
            if star == Some(index) {
                sequence_type
            } else {
                ty
            },
            Builtin::ListGet(None),
            vec![binder.pattern_local(root), Expr::integer(index)],
        );
        snapshot.capture(binder, item);
    }
    snapshot
}

pub(super) fn take_pattern_temporary(value: Expr, slot: usize) -> Expr {
    Expr {
        ty: value.ty,
        kind: ExprKind::Scope {
            body: Vec::new(),
            value: Box::new(value),
            clear: vec![slot],
        },
    }
}

/// Evaluate a shared RHS once, then bind each target from left to right.
/// The last target takes the temporary before its receiver or index runs.
pub(super) fn bind_assignment<B: PatternBinder>(
    binder: &mut B,
    targets: &[ast::Expr],
    value: Expr,
    output: &mut Vec<Statement>,
) -> StdResult<(), B::Error> {
    if let [target] = targets {
        bind_input(binder, target, value, None, output)?;
        return Ok(());
    }
    let slot = binder.pattern_capture(&value);
    let mut body = vec![Statement::Assign(slot, value)];
    for (index, target) in targets.iter().enumerate() {
        let value = binder.pattern_local(slot);
        let value = if index + 1 == targets.len() {
            take_pattern_temporary(value, slot)
        } else {
            value
        };
        bind_input(binder, target, value, None, &mut body)?;
    }
    output.push(Statement::Scope {
        body,
        clear: vec![slot],
    });
    Ok(())
}

/// A projected fixed tuple or a checked Python pull that owns the source row.
pub(super) enum Row {
    Projected(usize),
    Pulled(calls::ArgumentScope),
}

/// Only the expression consuming the pull can release its source temporaries.
fn consume_row(value: Expr, row: Option<Row>) -> Expr {
    match row {
        Some(Row::Pulled(scope)) => scope.finish(value),
        _ => value,
    }
}

/// Append one target's store or its scoped unpack to the receiving suite.
pub(super) fn bind_input<B: PatternBinder>(
    binder: &mut B,
    target: &ast::Expr,
    value: Expr,
    row: Option<Row>,
    output: &mut Vec<Statement>,
) -> StdResult<(), B::Error> {
    let Some(targets) = pattern_elements(target) else {
        binder.pattern_store(target, consume_row(value, row), output)?;
        return Ok(());
    };
    let mut stars = targets
        .iter()
        .enumerate()
        .filter(|(_, target)| matches!(target, ast::Expr::Starred(_)));
    let star = stars.next().map(|(index, _)| index);
    if stars.next().is_some() {
        return Err(binder.multiple_stars(target));
    }
    let PatternSnapshot { mut body, clear } =
        binder.pattern_snapshot(target, targets, star, value, row)?;
    let (temporaries, items) = clear.split_at(clear.len() - targets.len());
    if let Some(&root) = temporaries.first() {
        body.push(Statement::Clear(root));
    }
    for (target, &item) in targets.iter().zip(items) {
        let target = if let ast::Expr::Starred(starred) = target {
            &starred.value
        } else {
            target
        };
        let value = take_pattern_temporary(binder.pattern_local(item), item);
        bind_input(binder, target, value, None, &mut body)?;
    }
    output.push(Statement::Scope { body, clear });
    Ok(())
}

impl Checker<'_> {
    /// Store a row from a Python cursor using the ordinary two-target unpack.
    pub(super) fn store_object_pair(
        &mut self,
        target: usize,
        item: Expr,
        site: TextRange,
    ) -> Statement {
        let sequence = self.types.list(Type::PythonObject);
        let PatternSnapshot { mut body, clear } = protocol_snapshot(
            self,
            site,
            [Type::PythonObject; 2].into_iter(),
            None,
            item,
            sequence,
            None,
        );
        let &[key, value] = clear.last_chunk().expect("captured both pair items");
        body.push(Statement::assign_item(
            self.local(target),
            self.local(key),
            self.local(value),
        ));
        Statement::Scope { body, clear }
    }

    /// Finds a target's storage without executing its receiver or index.
    pub(super) fn target_context(
        &self,
        target: &ast::Expr,
        rhs: Option<&ast::Expr>,
    ) -> Option<Type> {
        self.target_expectation(target, rhs).map(|(ty, _)| ty)
    }

    fn target_expectation(
        &self,
        target: &ast::Expr,
        rhs: Option<&ast::Expr>,
    ) -> Option<(Type, Option<TypeDeclaration>)> {
        if let ast::Expr::Name(name) = target {
            self.stored_name(name.id.as_str())
                .map(|slot| (self.locals[slot], self.declared_slots.get(&slot).copied()))
        } else if target.is_attribute_expr() && (self.registered.is_some() || self.initializing) {
            // A generated field's parameter already has a type. Its store checks the destination.
            None
        } else {
            self.assignment_context(target, rhs)
        }
    }

    /// Query the actual destination after argument evaluation, which can bind the name.
    pub(super) fn stores_python_result(&self, name: &str) -> bool {
        match self.stored_name(name) {
            Some(slot) => self.locals[slot] == Type::PythonObject,
            None => !self.nullable_assignments.contains_key(name),
        }
    }

    /// Only the final conversion into a proven inferred binding may select
    /// object storage. Failures inside its operand keep their own diagnostics.
    pub(super) fn inferred_store_conflict(
        &self,
        target: &ast::Expr,
        site: &impl Ranged,
        failure: &Failure,
    ) -> bool {
        failure.is_storage_mismatch(site)
            && self.graph.modules[self.module].is_program_source()
            && self.graph.session.inferred_name_store(self.module, target)
            && target
                .as_name_expr()
                .and_then(|name| self.stored_name(&name.id))
                .is_none_or(|slot| {
                    !self.declared_slots.contains_key(&slot) && !self.fixed_formals.contains(&slot)
                })
    }

    /// Keep checking subsequent reads with object storage. A fixed body
    /// containing this placeholder is discarded before emission.
    pub(super) fn widen_inferred_store(
        &mut self,
        target: &ast::Expr,
        site: &impl Ranged,
        failure: Failure,
    ) -> Result<Expr> {
        if !self.inferred_store_conflict(target, site, &failure) {
            return Err(failure);
        }
        self.require_registered_operation(site)?;
        if let Some(slot) = target
            .as_name_expr()
            .and_then(|name| self.stored_name(&name.id))
        {
            self.locals[slot] = Type::PythonObject;
        }
        Ok(Expr::unrepresented())
    }

    pub(super) fn assignment_rhs(
        &mut self,
        target: &ast::Expr,
        value: &ast::Expr,
        (expected, declaration): (Type, Option<TypeDeclaration>),
        result: ResultUse<'_>,
    ) -> Result<Expr, String> {
        if expected == Type::TextIO
            && let Some(stream) = self.stream_rhs(value)?
        {
            return Ok(stream);
        }
        if self.inference.is_some() && expected == Type::JsonObject && target.is_attribute_expr() {
            // Keep a mutable owner until the resolved target decides its storage.
            self.document_annotation_value(value, expected, documents::MAP)
        } else {
            self.expr_in_context_for(
                value,
                expected,
                |checker, ty| {
                    ty == Type::PythonObject
                        && target
                            .as_name_expr()
                            .and_then(|name| checker.names.get(name.id.as_str()))
                            .is_some_and(|&slot| checker.can_widen_unbound_python_local(slot))
                },
                |checker, operand, expected, site| {
                    if expected == Type::None
                        && declaration.is_none()
                        && matches!(operand.ty, Type::JsonValue | Type::JsonRoot)
                        && !checker.terminating_call(&operand)
                        && checker.graph.modules[checker.module].is_program_source()
                        && let Some(slot) = target
                            .as_name_expr()
                            .and_then(|name| checker.stored_name(&name.id))
                        && checker.locals[slot] == expected
                        && checker
                            .graph
                            .session
                            .inferred_local_definitions(checker.module, target)
                            .is_some()
                    {
                        checker.require_registered_operation(site)?;
                        checker.locals[slot] = Type::PythonObject;
                        return Ok(operand);
                    }
                    match checker.coerce_supplied_result(operand, expected, site) {
                        Ok(value) if checker.native_type_proof(&value, expected) => {
                            if let Some(owner) = declaration {
                                checker.checked_native_operand(site.range(), owner);
                            }
                            Ok(value)
                        }
                        Err(failure) if declaration.is_none() => {
                            checker.widen_inferred_store(target, site, failure)
                        }
                        result => result,
                    }
                },
                declaration,
                result,
            )
        }
    }

    /// Keep a terminating RHS and any fresh name declarations, without evaluating its targets.
    pub(super) fn ends_before_assignment(
        &mut self,
        value: &Expr,
        targets: &[ast::Expr],
    ) -> Result<bool, String> {
        if !self.terminating_call(value)
            || targets
                .iter()
                .any(|target| pattern_elements(target).is_some())
        {
            return Ok(false);
        }
        for target in targets {
            if let Some(name) = target.as_name_expr()
                && !self.names.contains_key(name.id.as_str())
            {
                let slot = self.declare(name.id.as_str(), value.ty, target)?;
                // A checked unbound read can already refer to its original slot and flag.
                if !self.bound_flags.contains_key(name.id.as_str()) {
                    self.unstored_targets.insert(slot);
                }
            }
        }
        Ok(true)
    }

    /// Check the source declaration itself so an inferred ABI formal is distinct
    /// from an explicitly annotated parameter or a later annotated local.
    fn direct_unannotated_parameter_store(
        &self,
        name: &str,
        target: &ast::Expr,
        rhs: &ast::Expr,
    ) -> bool {
        self.graph.modules[self.module]
            .parsed
            .syntax()
            .body
            .iter()
            .any(|statement| {
                let ast::Stmt::FunctionDef(function) = statement else {
                    return false;
                };
                function.parameters.iter_non_variadic_params().any(|parameter| {
                    parameter.name().as_str() == name && parameter.annotation().is_none()
                }) && function.body.iter().any(|statement| {
                    matches!(statement, ast::Stmt::Assign(assign)
                        if matches!(assign.targets.as_slice(), [stored] if stored.range() == target.range())
                            && assign.value.range() == rhs.range())
                })
            })
    }

    /// Keep this initial fresh-storage subset to two fixed, alias-safe shapes:
    /// a builtin string conversion comprehension and a literal string join.
    fn simple_parameter_conversion(&self, name: &str, slot: usize, rhs: &ast::Expr) -> bool {
        match rhs {
            ast::Expr::ListComp(comp) => {
                let [clause] = comp.generators.as_slice() else {
                    return false;
                };
                let (ast::Expr::Name(input), ast::Expr::Name(item), ast::Expr::Call(call)) =
                    (&clause.iter, &clause.target, comp.elt.as_ref())
                else {
                    return false;
                };
                input.id == name
                    && item.id != name
                    && !clause.is_async
                    && clause.ifs.is_empty()
                    && self.unshadowed_builtin(&call.func, "str")
                    && call.arguments.keywords.is_empty()
                    && matches!(call.arguments.args.as_ref(), [ast::Expr::Name(value)] if value.id == item.id)
            }
            ast::Expr::Call(call) => {
                self.types.element(self.locals[slot]) == Some(Type::Str)
                    && matches!(&*call.func, ast::Expr::Attribute(attribute)
                        if attribute.attr.as_str() == "join" && attribute.value.is_string_literal_expr())
                    && call.arguments.keywords.is_empty()
                    && matches!(call.arguments.args.as_ref(), [ast::Expr::Name(value)] if value.id == name)
            }
            _ => false,
        }
    }

    /// A chain shares one RHS object and visits targets in source order. A later
    /// annotated target can supply the type of an otherwise empty literal.
    pub(super) fn assignment_statement(
        &mut self,
        targets: &[ast::Expr],
        rhs: &ast::Expr,
        output: &mut Vec<Statement>,
    ) -> Result<bool, String> {
        // The live namespace owns later lookups. An enclosing call may already
        // have loaded its callable before an argument mutates this attribute.
        for target in targets {
            if self.registered.is_none()
                && let ast::Expr::Attribute(attribute) = target
                && matches!(attribute.attr.as_str(), "compile" | "MULTILINE" | "M")
                && self
                    .graph
                    .session
                    .is_stdlib_module(self.module, &attribute.value, "re")
                && match self.resolve(&attribute.value) {
                    Some(Binding::IntrinsicModule(crate::modules::IntrinsicModule::Re)) => true,
                    Some(Binding::Module(module)) => self.graph.modules[module].is_bundled("re"),
                    _ => false,
                }
            {
                return Err(self.scoped_builtin_error("the mutable re module binding", attribute));
            }
        }
        let fixed_field = match targets {
            [ast::Expr::Attribute(field)] if self.registered.is_none() => Some(field),
            _ => None,
        };
        if let Some(attribute) = fixed_field
            && let Some(field) = self.mime_attribute(attribute, self.resolve(&attribute.value))
        {
            return self.assignment_statement(&[ast::Expr::Attribute(field)], rhs, output);
        }
        if let Some(attribute) = fixed_field
            && attribute.attr.as_str() == "default_factory"
            && (matches!(rhs, ast::Expr::NoneLiteral(_)) || self.unshadowed_builtin(rhs, "list"))
        {
            let receiver = self.expr(&attribute.value)?;
            if self.types.is_default_list_dict(receiver.ty) {
                let enabled = !matches!(rhs, ast::Expr::NoneLiteral(_));
                output.push(Statement::Expr(Expr {
                    ty: Type::None,
                    kind: ExprKind::Builtin(Builtin::DefaultFactorySet { enabled }, vec![receiver]),
                }));
                return Ok(true);
            }
        }
        if let Some(attribute) = fixed_field
            && attribute.attr.as_str() == "converter"
            && let Some(Binding::Intrinsic(Intrinsic::ClockConverter(utc))) = self.resolve(rhs)
        {
            let receiver = self.expr(&attribute.value)?;
            if receiver.ty == Type::LogFormatter {
                output.push(self.formatter_set(receiver, attribute, Expr::boolean(utc))?);
                return Ok(true);
            }
        }
        if let [target] = targets {
            if let Some(elements) = pattern_elements(target) {
                let limit = (!elements
                    .iter()
                    .any(|element| matches!(element, ast::Expr::Starred(_))))
                .then_some(elements.len());
                let value = match self.mapped_unpack(rhs, limit)? {
                    Some(value) => value,
                    None => self.unpacking_value(target, rhs)?,
                };
                bind_assignment(self, targets, value, output)?;
                return Ok(true);
            }
            let local = target
                .as_name_expr()
                .filter(|_| {
                    !self.module_scope && self.block_depth == 1 && self.local_functions.is_empty()
                })
                .and_then(|name| Some((name.id.as_str(), *self.names.get(name.id.as_str())?)));
            // A signature's input type describes the incoming argument, not the
            // later value of its unannotated Python parameter. Prove the exact
            // direct function-suite assignment before splitting its storage.
            let inferred_parameter = local.is_some_and(|(name, slot)| {
                self.graph.modules[self.module].is_program_source()
                    && matches!(self.locals[slot], Type::ListInt | Type::List(_))
                    && !self.declared_slots.contains_key(&slot)
                    && self.direct_unannotated_parameter_store(name, target, rhs)
                    && self.simple_parameter_conversion(name, slot, rhs)
                    && !narrowing::expression_assignments(rhs).contains(name)
            });
            let previous = local
                .filter(|(_, slot)| self.inferred_containers.contains(slot) || inferred_parameter);
            let mut context = self.target_expectation(target, Some(rhs));
            if matches!(context, Some((Type::Callable(_), None)))
                && let Some((_, slot)) = local
                && let Some(ValueSource::Function(prior)) = self.assigned.value_source(slot)
                && let Some(Binding::Definition(owner)) = self.resolve(rhs)
                && self
                    .functions
                    .get(&owner)
                    .is_some_and(|next| next.id != prior)
            {
                // A previous function in an unannotated slot does not declare
                // the next function's parameter types.
                context = None;
            }
            let result = target
                .as_name_expr()
                .filter(|_| context.is_none_or(|(ty, _)| ty == Type::PythonObject))
                .map_or(ResultUse::Fixed, |name| ResultUse::Store(name.id.as_str()));
            let scalar_item = context
                .filter(|(ty, _)| target.is_subscript_expr() && ty.builtin_scalar_name().is_some());
            let value = if context.is_none()
                && let Some(stream) = self.stream_rhs(rhs)?
            {
                stream
            } else if inferred_parameter {
                // Both recognized conversions have a concrete RHS; the previous
                // list element type must not constrain the new local's result.
                self.typed_expr_for(rhs, ResultUse::Fixed)?
            } else if let Some((_, old)) = previous {
                // An empty literal still needs the old context. Other container
                // expressions can determine their own concrete result type.
                self.expr_expected_if(rhs, self.locals[old], Self::rebindable_container)?
            } else if let Some((expected, _)) = scalar_item {
                // Keep the scalar operand. Its RHS can change which container receives the store.
                self.expr_expected_if(rhs, expected, |_, _| true)?
            } else if let Some(expected) = context {
                self.assignment_rhs(target, rhs, expected, result)?
            } else {
                self.typed_expr_for(rhs, result)?
            };
            if self.ends_before_assignment(&value, targets)? {
                output.push(self.statement_at(rhs, Statement::Expr(value)));
                return Ok(false);
            }
            if let Some((name, old)) = previous
                && value.ty != self.locals[old]
            {
                statements::append_block(output, self.replace_local(name, old, value.ty, value));
                return Ok(true);
            }
            let value = if context.is_none()
                && let ast::Expr::Name(name) = target
                && !self.names.contains_key(name.id.as_str())
            {
                let value = self.consuming_lists.capture_split(target.range(), value);
                // Later statements still need the binding after this body lost its output.
                if value.ty == Type::PythonObject
                    && matches!(
                        value.kind,
                        ExprKind::Builtin(Builtin::Unrepresented | Builtin::UnrepresentedCall, _)
                    )
                    && self.discarded_function_body()
                {
                    self.assign(target, value, None)?;
                    return Ok(true);
                }
                value
            } else {
                value
            };
            let value_site = (scalar_item.is_some()
                || context.is_none() && target.is_subscript_expr())
            .then_some(rhs);
            let start = output.len();
            self.store_value(target, value, value_site, Some(output))?;
            if let Some(sites) = &mut self.fixed_sites
                && let Some(name) = target.as_name_expr()
                && let [statement @ Statement::Assign(slot, _)] = &output[start..]
                && self.names.get(name.id.as_str()) == Some(slot)
            {
                sites.statement(rhs.range(), statement);
            } else if let Some(sites) = &mut self.fixed_sites
                && let ast::Expr::Attribute(attribute) = target
                && let [Statement::SetField(_, field, _)] = &mut output[start..]
            {
                sites.store_field(attribute, rhs.range(), field, None);
            }
            return Ok(true);
        }
        let context = targets.iter().find_map(|target| {
            self.target_expectation(target, Some(rhs))
                .map(|ty| (target, ty))
        });
        let value = if let Some((target, expected)) = context {
            assignments::unpromoted(self.assignment_rhs(target, rhs, expected, ResultUse::Fixed)?)
        } else {
            let target = targets
                .iter()
                .find(|target| pattern_elements(target).is_some())
                .unwrap_or(&targets[0]);
            self.unpacking_value(target, rhs)?
        };
        if self.ends_before_assignment(&value, targets)? {
            output.push(self.statement_at(rhs, Statement::Expr(value)));
            return Ok(false);
        }
        bind_assignment(self, targets, value, output)?;
        Ok(true)
    }

    pub(super) fn store_value(
        &mut self,
        target: &ast::Expr,
        value: Expr,
        value_site: Option<&ast::Expr>,
        output: Option<&mut Vec<Statement>>,
    ) -> Result<(), String> {
        if target.is_name_expr() {
            self.assign(target, value, output)
        } else {
            self.store_place(target, value, value_site, output)
        }
    }

    /// Receivers and indices are checked after the RHS and preceding targets.
    fn store_place(
        &mut self,
        target: &ast::Expr,
        value: Expr,
        value_site: Option<&ast::Expr>,
        output: Option<&mut Vec<Statement>>,
    ) -> Result<(), String> {
        let statement = 'place: {
            if let ast::Expr::Subscript(subscript) = target {
                if let Some(ty) = self.environ_item_type(&subscript.value) {
                    let bytes = ty == Type::Bytes;
                    let value = self.coerce(value, ty, target)?;
                    let key = self.expr_expected(&subscript.slice, ty)?;
                    break 'place Statement::Expr(builtin(
                        Type::None,
                        Builtin::process(ir::ProcessOperation::EnvironSet { bytes }),
                        vec![value, key],
                    ));
                }
                let receiver = self.expr(&subscript.value)?;
                self.keep_module_list_layout(&receiver);
                if receiver.ty == Type::PythonObject {
                    let index = if self.registered.is_some() {
                        registered::values::index_key(&subscript.slice, &mut |expr| {
                            self.expr(expr)
                        })?
                    } else {
                        self.expr(&subscript.slice)?
                    };
                    if self.module_scope && self.is_module_namespace(&receiver) {
                        self.namespace_item_store(&index, &value);
                    } else if !self.module_scope
                        && self.reachable
                        && let Some(key) = subscript.slice.as_string_literal_expr()
                        && let Some(ValueSource::FrameLocals { site, caller }) = self
                            .value_source(&receiver)
                            .or_else(|| self.frame_locals(&subscript.value))
                    {
                        let name = key.value.to_str();
                        let slot = if caller {
                            Some(assignments::SourceSlot::Caller(name.to_owned()))
                        } else {
                            self.names
                                .get(name)
                                .copied()
                                .filter(|slot| !self.declared_slots.contains_key(slot))
                                .map(assignments::SourceSlot::Local)
                        };
                        if let Some(slot) = slot {
                            self.assigned.write_frame_local(slot, site, [value.ty]);
                        }
                    }
                    break 'place Statement::assign_item(receiver, index, value);
                }
                if let Some((key, element)) = self.types.chainmap_entries(receiver.ty) {
                    let value = self.coerce(value, element, target)?;
                    let key = self.expr_expected(&subscript.slice, key)?;
                    break 'place Statement::Expr(Expr {
                        ty: Type::None,
                        kind: ExprKind::Builtin(
                            Builtin::ChainMapSetItem,
                            vec![value, receiver, key],
                        ),
                    });
                }
                if let Type::Class(class) = receiver.ty
                    && self.classes.methods[class].contains_key("__setitem__")
                {
                    // Python evaluates the RHS, the receiver, then the index.
                    let mut scope = calls::ArgumentScope::default();
                    let value = scope.hold(self, value);
                    let receiver = scope.hold(self, receiver);
                    let key = self.expr(&subscript.slice)?;
                    let key = scope.hold(self, key);
                    let assigned =
                        self.record_primitive(receiver, "__setitem__", &[key, value], target)?;
                    break 'place Statement::Expr(scope.finish(assigned));
                }
                if matches!(receiver.ty, Type::Document(_)) {
                    if let ast::Expr::Slice(slice) = &*subscript.slice {
                        if slice.lower.is_some() || slice.upper.is_some() || slice.step.is_some() {
                            return Err(error(
                                self.source,
                                target,
                                "document list assignment requires a full slice",
                            ));
                        }
                        let value = if value.ty == documents::LIST {
                            self.document_list(value)
                        } else {
                            value
                        };
                        let value = self.coerce(value, self.types.list(documents::ANY), target)?;
                        break 'place Statement::Expr(documents::op(
                            ir::DocumentOp::ReplaceItems,
                            Type::None,
                            vec![value, receiver],
                        ));
                    }
                    let value = self.coerce(value, documents::ANY, target)?;
                    let key = self.expr_expected(&subscript.slice, Type::Str)?;
                    break 'place Statement::Expr(documents::op(
                        ir::DocumentOp::SetItem,
                        Type::None,
                        vec![value, receiver, key],
                    ));
                }
                let receiver = self.json_mutation_receiver(receiver, subscript)?;
                if matches!(receiver.ty, Type::ListInt | Type::List(_))
                    && let ast::Expr::Slice(slice) = &*subscript.slice
                {
                    self.slice(receiver, slice, subscript.range())?;
                    return Err(self.scoped_builtin_error("a list slice assignment", target));
                }
                let declaration = self.value_declaration(&receiver);
                let (key, element) = self
                    .types
                    .mapping(receiver.ty)
                    .or_else(|| {
                        self.types
                            .indexed_element(receiver.ty)
                            .map(|element| (Type::Int, element))
                    })
                    .ok_or_else(|| {
                        error(
                            self.source,
                            target,
                            "indexed assignment requires a mutable typed list or dictionary",
                        )
                    })?;
                let site = value_site.unwrap_or(target);
                let owner = self.inferred_storage(&receiver);
                let value = self
                    .container_store(owner.as_deref(), false, value, element, site)
                    .map_err(|error| error.expected_from(declaration, site.range()));
                // A direct RHS owns its error before the key. An unpacked value is placed after its key.
                if value_site.is_some()
                    && let Err(error) = value
                {
                    return Err(error);
                }
                let index = if matches!(receiver.ty, Type::Deque(_)) {
                    self.integer_index(&subscript.slice)?
                } else if let Some(owner) = owner
                    && self.types.mapping(receiver.ty).is_some()
                {
                    self.inserted_operand(&owner, true, &subscript.slice, key)?
                } else {
                    self.expr_expected_from(&subscript.slice, key, declaration)?
                };
                Statement::assign_item(receiver, index, value?)
            } else if let ast::Expr::Attribute(attribute) = target {
                if self.registered.is_some() {
                    let receiver = self.expr(&attribute.value)?;
                    let method_receiver = registered::values::method_receiver(
                        self.registered_local_start,
                        self.class,
                        &attribute.value,
                        registered::values::receiver_facts(&receiver),
                    );
                    if receiver.ty != Type::PythonObject && !method_receiver {
                        return Err(error(
                            self.source,
                            attribute,
                            "expected a Python object target",
                        ));
                    }
                    if method_receiver {
                        self.frame_arguments = FrameArguments::Python;
                    }
                    break 'place registered::attribute_statement(
                        ir::AttributeAction::Store,
                        attribute,
                        vec![value, receiver],
                    );
                }
                let binding = self.resolve(target);
                if binding == Some(Binding::Intrinsic(Intrinsic::PopenSelector)) {
                    let value = self.coerce(value, Type::ProcessSelectorClass, target)?;
                    break 'place Statement::Expr(Expr {
                        ty: Type::None,
                        kind: ExprKind::Builtin(
                            Builtin::process(ir::ProcessOperation::SetSelector),
                            vec![value],
                        ),
                    });
                }
                if cache_paths::config(attribute, binding, || self.resolve(&attribute.value))
                    == Some(true)
                {
                    let value = self.coerce(value, self.types.optional(Type::Str), target)?;
                    break 'place Statement::Expr(cache_paths::op(
                        ir::CachePathOperation::Set(true),
                        Type::None,
                        vec![value],
                    ));
                }
                if let Some(Binding::Intrinsic(Intrinsic::LogRecordFlag(mask))) = binding {
                    let value = self.coerce(value, Type::Bool, target)?;
                    break 'place Statement::Expr(logging_records::op(
                        ir::LogRecordOperation::SetFlag,
                        Type::None,
                        vec![Expr::integer(mask), value],
                    ));
                }
                if binding == Some(Binding::Intrinsic(Intrinsic::BuiltinsLen)) {
                    let value = self.coerce(value, self.builtin_len().ty, target)?;
                    break 'place Statement::Expr(Expr {
                        ty: Type::None,
                        kind: ExprKind::Builtin(Builtin::BuiltinsSetLen, vec![value]),
                    });
                }
                if binding == Some(Binding::Intrinsic(Intrinsic::Stdout)) {
                    if !matches!(
                        self.types.payload(value.ty).unwrap_or(value.ty),
                        Type::TextIO | Type::StringIO | Type::None
                    ) {
                        return Err(error(
                            self.source,
                            target,
                            "sys.stdout requires None or a native text stream",
                        ));
                    }
                    break 'place Statement::Expr(Expr {
                        ty: Type::None,
                        kind: ExprKind::Builtin(Builtin::SysStdoutSet, vec![value]),
                    });
                }
                if binding == Some(Binding::Intrinsic(Intrinsic::BuiltinsLast)) {
                    if !sys::displayable(value.ty) {
                        return Err(error(
                            self.source,
                            target,
                            "builtins._ requires a supported display value",
                        ));
                    }
                    break 'place Statement::Expr(Expr {
                        ty: Type::None,
                        kind: ExprKind::Builtin(Builtin::BuiltinsSetLast, vec![value]),
                    });
                }
                if let Some(class) = self.class_reference(&attribute.value)
                    && matches!(
                        self.classes.declarations.definitions[class]
                            .static_bindings
                            .get(attribute.attr.as_str()),
                        Some(crate::classes::ClassValue::Runtime { .. })
                    )
                {
                    break 'place self.store_class_data(class, attribute, value)?;
                }
                let receiver = Self::weak_target(self.expr(&attribute.value)?);
                if self.types.payload(receiver.ty).unwrap_or(receiver.ty) == Type::PythonObject {
                    break 'place registered::attribute_statement(
                        ir::AttributeAction::Store,
                        attribute,
                        vec![value, receiver],
                    );
                }
                if attribute.attr.as_str() == "__wrapped__"
                    && (self.types.cache_declaration(receiver.ty).is_some()
                        || self.types.cached_function(receiver.ty).is_some())
                    && self.types.wrapped_link_type(value.ty)
                        == self.types.wrapped_link_type(receiver.ty)
                {
                    // Assignment evaluates the RHS before the receiver. The public link
                    // owns this reference independently of the cache's invocation target.
                    let mut scope = calls::ArgumentScope::default();
                    let value = scope.hold(self, value);
                    let store = self.attach_wrapped(receiver, value, attribute)?;
                    break 'place Statement::Expr(scope.finish(store));
                }
                if matches!(receiver.ty, Type::Callable(_))
                    && attribute.attr.as_str() == "__defaults__"
                {
                    self.require_registered_operation(attribute)?;
                    break 'place registered::attribute_statement(
                        ir::AttributeAction::Store,
                        attribute,
                        vec![value, receiver],
                    );
                }
                if receiver.ty == Type::SysImplementation && attribute.attr.as_str() == "cache_tag"
                {
                    let value = self.coerce(value, self.types.optional(Type::Str), target)?;
                    break 'place Statement::Expr(cache_paths::op(
                        ir::CachePathOperation::Set(false),
                        Type::None,
                        vec![value, receiver],
                    ));
                }
                if matches!(receiver.ty, Type::Popen(..) | Type::ActivePopen)
                    && attribute.attr.as_str() == "returncode"
                {
                    let value = self.coerce(value, self.types.optional(Type::Int), target)?;
                    break 'place Statement::Expr(builtin(
                        Type::None,
                        Builtin::process(ir::ProcessOperation::PopenSetStatus),
                        vec![value, receiver],
                    ));
                }
                if matches!(receiver.ty, Type::Popen(..)) && attribute.attr.as_str() == "poll" {
                    let Some((params, returns)) = self.types.signature(value.ty) else {
                        return Err(error(
                            self.source,
                            target,
                            "Popen.poll needs a zero-argument callable",
                        ));
                    };
                    if !params.is_empty()
                        || !(matches!(returns, Type::None | Type::Int)
                            || returns == self.types.optional(Type::Int))
                    {
                        return Err(error(
                            self.source,
                            target,
                            "Popen.poll needs a zero-argument callable returning int or None",
                        ));
                    }
                    break 'place Statement::Expr(builtin(
                        Type::None,
                        Builtin::process(ir::ProcessOperation::PopenSetPoll {
                            returns_none: returns == Type::None,
                        }),
                        vec![value, receiver],
                    ));
                }
                if let Some(class) =
                    self.mutable_instance_data(receiver.ty, attribute.attr.as_str())
                {
                    break 'place self.store_instance_data(class, attribute, receiver, value)?;
                }
                if self.types.has_reflected_functions()
                    && matches!(receiver.ty, Type::FunctionRef | Type::Callable(_))
                    && let Some(index) = typing::attribute_index(attribute.attr.as_str())
                {
                    let value = self.coerce(value, Type::Str, target)?;
                    break 'place Statement::Expr(Expr {
                        ty: Type::None,
                        kind: ExprKind::Builtin(
                            Builtin::Typing(ir::TypingOperation::SetAttr(index)),
                            vec![value, receiver],
                        ),
                    });
                }
                if matches!(receiver.ty, Type::CodecWriterClass | Type::CodecWriter) {
                    let op = match (receiver.ty, attribute.attr.as_str()) {
                        (_, "encoding") => ir::WriterOperation::SetEncoding,
                        (Type::CodecWriter, "errors") => ir::WriterOperation::SetErrors,
                        _ => {
                            return Err(error(
                                self.source,
                                target,
                                "unsupported stream writer assignment",
                            ));
                        }
                    };
                    let value = self.coerce(value, Type::Str, target)?;
                    break 'place Statement::Expr(codecs::writer(
                        op,
                        Type::None,
                        vec![value, receiver],
                    ));
                }
                if receiver.ty == Type::LogManager && attribute.attr.as_str() == "disable" {
                    break 'place self.set_manager_disable(receiver, value, target)?;
                }
                if receiver.ty == Type::Logger
                    && let Some(op) = match attribute.attr.as_str() {
                        "disabled" => Some(ir::LogOperation::SetDisabled),
                        "propagate" => Some(ir::LogOperation::SetPropagate),
                        _ => None,
                    }
                {
                    let value = self.coerce(value, Type::Bool, target)?;
                    // The RHS precedes evaluation of the assignment target.
                    break 'place Statement::Expr(Expr {
                        ty: Type::None,
                        kind: ExprKind::Builtin(Builtin::Logging(op), vec![value, receiver]),
                    });
                }
                if receiver.ty == Type::LogFormatter {
                    break 'place self.formatter_set(receiver, attribute, value)?;
                }
                if let Type::CmpKey(frame, true) = receiver.ty
                    && attribute.attr.as_str() == "obj"
                {
                    let cell = self.cmp_field(receiver, frame, 1);
                    let value = self.coerce(value, self.types.element(cell.ty).unwrap(), target)?;
                    break 'place Statement::assign_item(cell, Expr::integer(0), value);
                }
                if let Type::SimpleNamespace(id) = receiver.ty {
                    let name = self.namespace_attr_name(attribute)?;
                    let item = self.types.mapping(Type::Dict(id)).unwrap().1;
                    let value = self.coerce(value, item, target)?;
                    break 'place Statement::assign_item(
                        super::namespaces::backing(receiver, id),
                        name,
                        value,
                    );
                }
                if matches!(receiver.ty, Type::ModuleSpec(_))
                    && attribute.attr.as_str() == "has_location"
                {
                    break 'place self.set_module_spec_location(attribute, receiver, value)?;
                }
                if receiver.ty == Type::LogRecord {
                    let value = self.check_record_value(value, target)?;
                    break 'place Statement::Expr(logging_records::op(
                        ir::LogRecordOperation::Put,
                        Type::None,
                        vec![receiver, string(attribute.attr.as_str()), value],
                    ));
                }

                if attribute.attr.as_str() == "MIN_READ_SIZE" && receiver.ty == Type::BinaryIO {
                    let value = self.coerce(value, Type::Int, target)?;
                    // Assignment evaluates the RHS before the receiver.
                    break 'place Statement::Expr(Expr {
                        ty: Type::None,
                        kind: ExprKind::Builtin(Builtin::ZipMinReadSize, vec![value, receiver]),
                    });
                }
                if let Type::ChainMap(id) = receiver.ty
                    && attribute.attr.as_str() == "maps"
                {
                    let maps = self.coerce(value, self.types.list(Type::Dict(id)), target)?;
                    break 'place Statement::Expr(Expr {
                        ty: Type::None,
                        kind: ExprKind::Builtin(Builtin::ChainMapSetMaps, vec![maps, receiver]),
                    });
                }
                if let Some(property) = self.writable_property(receiver.ty, attribute) {
                    break 'place self.set_property(property, attribute, receiver, value)?;
                }
                if let Some(field) = self.setup_field(receiver.ty, attribute.attr.as_str()) {
                    break 'place self.store_setup_field(receiver, value, attribute, field)?;
                }
                if let Type::Class(id) = receiver.ty
                    && let class = &self.classes.declarations.definitions[id]
                    && self.classes.declarations.has_empty_slots(id)
                    && !self.classes.has_concrete_subclasses(id)
                    && !class
                        .fields
                        .iter()
                        .any(|f| f.name == attribute.attr.as_str())
                    && !class.static_bindings.contains_key(attribute.attr.as_str())
                    && !self.classes.methods[id].contains_key(attribute.attr.as_str())
                {
                    let message = format!(
                        "'{}' object has no attribute '{}' and no __dict__ for setting new attributes",
                        class.name, attribute.attr
                    );
                    break 'place Statement::Scope {
                        body: vec![
                            Statement::Expr(value),
                            Statement::Expr(receiver),
                            Statement::raise(Expr {
                                ty: Type::Exception,
                                kind: ExprKind::Exception {
                                    kind: exception_kind("AttributeError").unwrap(),
                                    message: Some(Box::new(string(message))),
                                },
                            }),
                        ],
                        clear: vec![],
                    };
                }
                let mock_effect = self.native_mock_instance(receiver.ty)
                    && attribute.attr.as_str() == "side_effect";
                let (receiver, field, mut ty) = self.writable_field_value(attribute, receiver)?;
                if let Some(inference) = self.inference
                    && let Type::Class(class) = receiver.ty
                    && ty == Type::JsonObject
                    && matches!(value.ty, documents::ANY | documents::MAP)
                {
                    inference.document_field(class, field);
                    ty = documents::MAP;
                }
                let value = self.coerce(value, ty, target).map_err(|error| {
                    if mock_effect {
                        self.object_runtime_export(
                            "unittest.mock side effect assignment",
                            attribute,
                        )
                    } else {
                        error
                    }
                })?;
                Statement::set_field(receiver, field, value)
            } else {
                return Err(error(self.source, target, "unsupported assignment target"));
            }
        };
        if let Some(output) = output {
            output.push(statement);
        }
        Ok(())
    }

    /// Literal context is discovered before any target binding is published.
    /// Optional promotions belong to stores rather than the shared RHS object.
    fn unpacking_value(&mut self, target: &ast::Expr, value: &ast::Expr) -> Result<Expr, String> {
        if let Some(targets) = pattern_elements(target) {
            let star = targets
                .iter()
                .position(|target| matches!(target, ast::Expr::Starred(_)));
            let fixed = targets.len() - usize::from(star.is_some());
            let literal = match value {
                ast::Expr::Tuple(tuple) => Some((&tuple.elts, false)),
                ast::Expr::List(list)
                    if targets
                        .iter()
                        .any(|target| pattern_elements(target).is_some()) =>
                {
                    Some((&list.elts, true))
                }
                _ => None,
            };
            if let Some((elements, is_list)) = literal
                && !elements
                    .iter()
                    .any(|value| matches!(value, ast::Expr::Starred(_)))
                && (elements.len() == fixed || star.is_some() && elements.len() >= fixed)
            {
                let mut values = Vec::with_capacity(elements.len());
                for (index, value) in elements.iter().enumerate() {
                    let target_index = star.map_or(Some(index), |star| {
                        let suffix = targets.len() - star - 1;
                        if index < star {
                            Some(index)
                        } else if index >= elements.len() - suffix {
                            Some(targets.len() - (elements.len() - index))
                        } else {
                            None
                        }
                    });
                    values.push(if let Some(index) = target_index {
                        self.unpacking_value(&targets[index], value)?
                    } else if let ast::Expr::Starred(starred) = &targets[star.unwrap()]
                        && let Some(context) = self.target_context(&starred.value, None)
                        && let Some(element) = self
                            .types
                            .element(self.types.payload(context).unwrap_or(context))
                    {
                        assignments::unpromoted(self.expr_expected(value, element)?)
                    } else {
                        self.expr(value)?
                    });
                }
                if is_list {
                    let mut element = values[0].ty;
                    for item in &values {
                        element = self.types.join(element, item.ty).ok_or_else(|| {
                            error(
                                self.source,
                                value,
                                "list unpacking requires one concrete element type",
                            )
                        })?;
                    }
                    let values = values
                        .into_iter()
                        .map(|item| self.coerce(item, element, value))
                        .collect::<std::result::Result<Vec<_>, _>>()?;
                    return Ok(Expr {
                        ty: self.types.list(element),
                        kind: ExprKind::List(values),
                    });
                }
                return Ok(Expr {
                    ty: self
                        .types
                        .tuple(values.iter().map(|value| value.ty).collect()),
                    kind: ExprKind::Tuple(values),
                });
            }
            if let ast::Expr::List(list) = value {
                let context = targets
                    .iter()
                    .filter_map(|target| match target {
                        ast::Expr::Starred(starred) => {
                            let context = self.target_context(&starred.value, None)?;
                            self.types
                                .element(self.types.payload(context).unwrap_or(context))
                        }
                        _ => self.target_context(target, None),
                    })
                    .reduce(|left, right| {
                        // Every destination must accept the shared element
                        // layout, so nullable targets cannot widen exact ones.
                        if self.types.join(left, right) == Some(left) {
                            right
                        } else {
                            left
                        }
                    });
                if let Some(element) = context {
                    if !list
                        .elts
                        .iter()
                        .any(|value| matches!(value, ast::Expr::Starred(_)))
                    {
                        let values = list
                            .elts
                            .iter()
                            .map(|value| {
                                self.expr_expected(value, element)
                                    .map(assignments::unpromoted)
                            })
                            .collect::<std::result::Result<Vec<_>, _>>()?;
                        let mut actual = values.first().map_or(element, |value| value.ty);
                        for value in &values {
                            actual = self.types.join(actual, value.ty).ok_or_else(|| {
                                error(
                                    self.source,
                                    list,
                                    "list unpacking requires one concrete element type",
                                )
                            })?;
                        }
                        let values = values
                            .into_iter()
                            .map(|value| self.coerce(value, actual, list))
                            .collect::<std::result::Result<Vec<_>, _>>()?;
                        return Ok(Expr {
                            ty: self.types.list(actual),
                            kind: ExprKind::List(values),
                        });
                    }
                    return self.expr_expected(value, self.types.list(element));
                }
            }
        }
        if let Some(expected) = self.target_expectation(target, Some(value)) {
            return self
                .assignment_rhs(target, value, expected, ResultUse::Fixed)
                .map(assignments::unpromoted);
        }
        self.expr(value)
    }
}

impl PatternBinder for Checker<'_> {
    type Error = Failure<String>;

    fn pattern_hidden(&mut self, ty: Type) -> usize {
        self.hidden(ty)
    }
    fn pattern_capture(&mut self, value: &Expr) -> usize {
        let slot = self.hidden(value.ty);
        if self.types.is_display_container(value.ty) {
            let source = self
                .value_source(value)
                .or_else(|| ValueSource::allocation(value));
            self.assigned.set_value_source(slot, source);
        }
        slot
    }
    fn pattern_local(&self, slot: usize) -> Expr {
        if self.registered.is_some()
            && self.locals[slot] == Type::PythonObject
            && let Some(&element) = self.narrowed.get(&slot)
        {
            return Expr::local(slot, element);
        }
        self.local(slot)
    }
    fn pattern_store(
        &mut self,
        target: &ast::Expr,
        value: Expr,
        output: &mut Vec<Statement>,
    ) -> StdResult<(), Self::Error> {
        self.store_value(target, value, None, Some(output))
    }
    fn multiple_stars(&self, target: &ast::Expr) -> Self::Error {
        error(
            self.source,
            target,
            "an unpacking target may contain only one starred target",
        )
    }

    fn pattern_snapshot(
        &mut self,
        target: &ast::Expr,
        targets: &[ast::Expr],
        star: Option<usize>,
        value: Expr,
        row: Option<Row>,
    ) -> StdResult<PatternSnapshot, Self::Error> {
        if value.ty == Type::PythonObject
            && (self.registered.is_some() || self.graph.modules[self.module].is_program_source())
        {
            if self.registered.is_none() {
                self.require_registered_operation(target)?;
            }
            let sequence = self.types.list(Type::PythonObject);
            return Ok(protocol_snapshot(
                self,
                target.range(),
                targets.iter().map(|_| Type::PythonObject),
                star,
                value,
                sequence,
                row,
            ));
        }

        let fixed = targets.len() - usize::from(star.is_some());
        let fields = self.types.fields(value.ty);
        let element = self.types.element(value.ty).or(match value.ty {
            Type::Str => Some(Type::Str),
            Type::Bytes => Some(Type::Int),
            _ => None,
        });
        if fields.is_none() && element.is_none() {
            return Err(error(
                self.source,
                target,
                "unpacking requires a fixed tuple, list, string, or bytes value",
            ));
        }
        if let Some(fields) = &fields
            && (fields.len() < fixed || star.is_none() && fields.len() != fixed)
        {
            return Err(error(
                self.source,
                target,
                "unpacking target has the wrong number of fields",
            ));
        }
        // A Python cursor yields a handle even when its element contract is a tuple.
        // Unpack that handle once, then check each target with its declared field type.
        if let Some(Row::Projected(slot)) = row
            && self.registered.is_some()
            && self.locals[slot] == Type::PythonObject
            && star.is_none()
            && let Some(fields) = fields.as_ref()
        {
            return Ok(protocol_snapshot(
                self,
                target.range(),
                fields.iter().copied(),
                None,
                self.local(slot),
                self.types.list(Type::PythonObject),
                None,
            ));
        }
        // A tuple display already evaluates these operands in snapshot order.
        if star.is_none()
            && row.is_none()
            && matches!(value.ty, Type::Tuple(_))
            && let ExprKind::Tuple(items) = value.kind
        {
            return Ok(PatternSnapshot::from_items(self, items));
        }
        // A successful builtin split returns at most maxsplit + 1 fields.
        let split_fits = row.is_none()
            && value
                .split_operands()
                .and_then(|(_, [_, _, limit])| limit.tagged_integer_literal())
                .and_then(|limit| usize::try_from(limit).ok())
                .is_some_and(|limit| limit < fixed);
        let (captured, mut body) = if let Some(Row::Projected(captured)) = row {
            (captured, Vec::new())
        } else {
            let captured = self.hidden(value.ty);
            (
                captured,
                vec![Statement::Assign(captured, consume_row(value, row))],
            )
        };
        let mut clear = vec![captured];
        if fields.is_none() && (fixed != 0 || star.is_none()) {
            let length = self.hidden(Type::Int);
            clear.push(length);
            body.push(Statement::Assign(
                length,
                builtin(Type::Int, Builtin::Len, vec![self.local(captured)]),
            ));
            if fixed != 0 {
                let expected = if star.is_some() {
                    format!("at least {fixed}")
                } else {
                    fixed.to_string()
                };
                let message = concat(
                    concat(
                        string(format!(
                            "not enough values to unpack (expected {expected}, got "
                        )),
                        builtin(Type::Str, Builtin::Str, vec![self.local(length)]),
                    ),
                    string(")"),
                );
                body.push(Statement::If(
                    comparison(self.local(length), Compare::Less, Expr::integer(fixed)),
                    vec![unpack_error(message)],
                    Vec::new(),
                ));
            }
            if star.is_none() && !split_fits {
                body.push(Statement::If(
                    comparison(self.local(length), Compare::Greater, Expr::integer(fixed)),
                    vec![unpack_error(string(format!(
                        "too many values to unpack (expected {fixed})"
                    )))],
                    Vec::new(),
                ));
            }
        }
        let mut snapshot = PatternSnapshot { body, clear };
        for (index, target) in targets.iter().enumerate() {
            let item = if let ast::Expr::Starred(starred) = target {
                let suffix = targets.len() - index - 1;
                if let Some(fields) = &fields {
                    self.tuple_rest(self.local(captured), fields, index, suffix, &starred.value)?
                } else {
                    self.sequence_rest(
                        self.local(captured),
                        element.unwrap(),
                        index,
                        suffix,
                        &starred.value,
                    )?
                }
            } else if let Some(fields) = &fields {
                let position = if star.is_some_and(|star| index > star) {
                    fields.len() - (targets.len() - index)
                } else {
                    index
                };
                Expr {
                    ty: fields[position],
                    kind: ExprKind::Field(Box::new(self.local(captured)), position),
                }
            } else {
                let position = if star.is_some_and(|star| index > star) {
                    -(targets.len() as i64 - index as i64)
                } else {
                    index as i64
                };
                let kind = match self.locals[captured] {
                    Type::Str => Builtin::StringGet,
                    Type::Bytes => Builtin::BytesGet,
                    _ => Builtin::ListGet(None),
                };
                builtin(
                    element.unwrap(),
                    kind,
                    vec![self.local(captured), Expr::integer(position)],
                )
            };
            snapshot.capture(self, item);
        }
        Ok(snapshot)
    }
}

impl Checker<'_> {
    /// A tuple remainder has one concrete element layout, including optional
    /// promotion. Empty remainders inherit a declared destination list type.
    fn tuple_rest(
        &mut self,
        value: Expr,
        fields: &[Type],
        start: usize,
        suffix: usize,
        target: &ast::Expr,
    ) -> Result<Expr, String> {
        let remaining = &fields[start..fields.len() - suffix];
        let context = self
            .target_context(target, None)
            .and_then(|ty| self.types.element(self.types.payload(ty).unwrap_or(ty)));
        let mut element = context.or(remaining.first().copied()).unwrap_or(Type::Int);
        if context.is_none() {
            for &ty in remaining {
                element = self.types.join(element, ty).ok_or_else(|| {
                    error(
                        self.source,
                        target,
                        "starred unpacking requires one concrete remainder element type",
                    )
                })?;
            }
        }
        let values = remaining
            .iter()
            .enumerate()
            .map(|(offset, &ty)| {
                self.coerce(
                    Expr {
                        ty,
                        kind: ExprKind::Field(Box::new(value.clone()), start + offset),
                    },
                    element,
                    target,
                )
            })
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(Expr {
            ty: self.types.list(element),
            kind: ExprKind::List(values),
        })
    }

    /// List slicing supplies a fresh shallow remainder. Other sequence types
    /// and optional promotions copy through the shared linear iteration lowering.
    fn sequence_rest(
        &mut self,
        value: Expr,
        element: Type,
        start: usize,
        suffix: usize,
        target: &ast::Expr,
    ) -> Result<Expr, String> {
        let bound = |value: Option<i64>| Expr {
            ty: self.types.optional(Type::Int),
            kind: match value {
                Some(value) => ExprKind::Present(Box::new(Expr::integer(value))),
                None => ExprKind::Absent(Box::new(Expr::none())),
            },
        };
        let slice = builtin(
            value.ty,
            Builtin::Slice,
            vec![
                value,
                bound(Some(start as i64)),
                bound((suffix > 0).then_some(-(suffix as i64))),
                bound(None),
            ],
        );
        let expected = self
            .target_context(target, None)
            .and_then(|ty| self.types.element(self.types.payload(ty).unwrap_or(ty)))
            .unwrap_or(element);
        if self.types.element(slice.ty) == Some(expected) {
            return Ok(slice);
        }
        let list = self.types.list(expected);
        let result = self.hidden(list);
        let name = format!("@unpacking{}", self.locals.len());
        let item_target = ast::Expr::Name(ast::ExprName {
            node_index: ast::AtomicNodeIndex::NONE,
            range: target.range(),
            id: name.clone().into(),
            ctx: ast::ExprContext::Store,
        });
        let input = Iteration::Container {
            iterable: slice,
            items: false,
        };
        let (iteration, _) = self.lower_iteration_after(
            (&item_target, None),
            input,
            &[],
            Vec::new(),
            None,
            |checker, body| {
                let item = checker.local(checker.names[&name]);
                let item = checker.coerce(item, expected, target)?;
                body.push(Statement::Expr(builtin(
                    Type::None,
                    Builtin::Append,
                    vec![checker.local(result), item],
                )));
                Ok(true)
            },
        )?;
        let item = self.names.remove(&name).unwrap();
        let mut body = vec![Statement::Assign(
            result,
            Expr {
                ty: list,
                kind: ExprKind::List(Vec::new()),
            },
        )];
        body.extend(iteration);
        Ok(Expr {
            ty: list,
            kind: ExprKind::Scope {
                body,
                value: Box::new(self.local(result)),
                clear: vec![result, item],
            },
        })
    }
}

impl Checker<'_> {
    /// A fixed tuple's result shape is known for constant slice bounds. Capture
    /// the receiver once, even when the selected fields are empty or out of order.
    pub(super) fn fixed_tuple_slice(
        &mut self,
        receiver: Expr,
        fields: &[Type],
        slice: &ast::ExprSlice,
    ) -> Result<Expr, String> {
        let bound = |expr: &ast::Expr| -> Result<i64, String> {
            let (value, negative) = match expr {
                ast::Expr::UnaryOp(op) if op.op == ast::UnaryOp::USub => (&*op.operand, true),
                value => (value, false),
            };
            if let ast::Expr::NumberLiteral(n) = value
                && let ast::Number::Int(i) = &n.value
            {
                let i = i.as_i64().unwrap_or(i64::MAX);
                return Ok(if negative { -i } else { i });
            }
            Err(error(
                self.source,
                expr,
                "fixed tuple slice bounds must be integer literals",
            ))
        };
        let step = slice.step.as_deref().map(bound).transpose()?.unwrap_or(1);
        if step == 0 {
            let ty = self.types.tuple(vec![]);
            return Ok(Expr {
                ty,
                kind: ExprKind::Scope {
                    body: vec![Statement::Expr(receiver)],
                    clear: vec![],
                    value: Box::new(Expr {
                        ty,
                        kind: ExprKind::Builtin(
                            Builtin::RaiseException,
                            vec![Expr {
                                ty: Type::Exception,
                                kind: ExprKind::Exception {
                                    kind: exception_kind("ValueError").unwrap(),
                                    message: Some(Box::new(string("slice step cannot be zero"))),
                                },
                            }],
                        ),
                    }),
                },
            });
        }
        let len = fields.len() as i64;
        let (lo, hi) = if step > 0 { (0, len) } else { (-1, len - 1) };
        let normalized = |value: i64| (if value < 0 { value + len } else { value }).clamp(lo, hi);
        let mut i = slice
            .lower
            .as_deref()
            .map(bound)
            .transpose()?
            .map(normalized)
            .unwrap_or(if step > 0 { lo } else { hi });
        let stop = slice
            .upper
            .as_deref()
            .map(bound)
            .transpose()?
            .map(normalized)
            .unwrap_or(if step > 0 { hi } else { lo });
        // CPython returns an exact tuple itself for a full unit-stride slice.
        // A struct sequence or namedtuple instead yields a fresh plain tuple.
        if step == 1 && i == 0 && stop == len && self.types.tuple_origin(receiver.ty).is_none() {
            return Ok(receiver);
        }
        let slot = self.hidden(receiver.ty);
        let mut items = Vec::new();
        while if step > 0 { i < stop } else { i > stop } {
            items.push(Expr {
                ty: fields[i as usize],
                kind: ExprKind::Field(Box::new(self.local(slot)), i as usize),
            });
            i = i.saturating_add(step);
        }
        let ty = self.types.tuple(items.iter().map(|item| item.ty).collect());
        Ok(Expr {
            ty,
            kind: ExprKind::Scope {
                body: vec![Statement::Assign(slot, receiver)],
                clear: vec![slot],
                value: Box::new(Expr {
                    ty,
                    kind: ExprKind::Tuple(items),
                }),
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::CompileOptions;

    #[test]
    fn unannotated_list_parameter_can_be_rebound_after_its_comprehension() {
        let source = r#"def render(values):
    values = [str(value) for value in values]
    values = ", ".join(values)
    return values
print(render([1.5, 2.5]))
"#;
        let llvm = crate::compile(source, &CompileOptions::default()).unwrap();
        assert!(llvm.contains("call ptr @imre_str_join"));
        assert!(!llvm.lines().any(|line| {
            line.contains("call ") && (line.contains("@imre_dyn_") || line.contains("@imre_boxed_"))
        }));
    }

    #[test]
    fn annotated_list_parameter_keeps_its_element_contract() {
        let source = r#"def render(values: list[float]):
    values = [str(value) for value in values]
    return values
print(render([1.5, 2.5]))
"#;
        let error = crate::compile(source, &CompileOptions::default()).unwrap_err();
        assert!(error.contains("expected float, got str"), "{error}");
    }
}
