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
        row: Option<Row<'_>>,
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
    fn capture(&mut self, binder: &mut (impl PatternBinder + ?Sized), item: Expr) {
        let slot = binder.pattern_hidden(item.ty);
        self.body.push(Statement::Assign(slot, item));
        self.clear.push(slot);
    }
}

fn protocol_snapshot(
    binder: &mut (impl PatternBinder + ?Sized),
    target: TextRange,
    item_types: impl ExactSizeIterator<Item = Type>,
    star: Option<usize>,
    value: Expr,
    sequence_type: Type,
    row: Option<Row<'_>>,
) -> PatternSnapshot {
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
    let slot = binder.pattern_hidden(value.ty);
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
pub(super) enum Row<'a> {
    Projected(usize),
    Pulled {
        scope: &'a mut calls::ArgumentScope,
        pair: Option<(usize, usize)>,
    },
}

/// Only the expression consuming the pull can release its source temporaries.
fn consume_row(value: Expr, row: Option<Row<'_>>) -> Expr {
    match row {
        Some(Row::Pulled { scope, .. }) => std::mem::take(scope).finish(value),
        _ => value,
    }
}

/// Append one target's store or its scoped unpack to the receiving suite.
pub(super) fn bind_input<B: PatternBinder>(
    binder: &mut B,
    target: &ast::Expr,
    value: Expr,
    row: Option<Row<'_>>,
    output: &mut Vec<Statement>,
) -> StdResult<Option<usize>, B::Error> {
    let Some(targets) = pattern_elements(target) else {
        binder.pattern_store(target, consume_row(value, row), output)?;
        return Ok(None);
    };
    let mut stars = targets
        .iter()
        .enumerate()
        .filter(|(_, target)| matches!(target, ast::Expr::Starred(_)));
    let star = stars.next().map(|(index, _)| index);
    if stars.next().is_some() {
        return Err(binder.multiple_stars(target));
    }
    // A private tuple slot supplies fixed fields. A qualified Python cursor can
    // supply a pair directly. Other inputs snapshot the ordinary row.
    let pair_pull = match &row {
        Some(Row::Pulled { pair, .. }) => *pair,
        _ => None,
    };
    let PatternSnapshot { mut body, clear } =
        binder.pattern_snapshot(target, targets, star, value, row)?;
    let (temporaries, items) = clear.split_at(clear.len() - targets.len());
    if let Some(&root) = temporaries.first() {
        body.push(Statement::Clear(root));
    }
    let pair = if let Some((iterator, ready)) = pair_pull
        && star.is_none()
        && let &[first, second] = items
    {
        let ordinary = binder.pattern_hidden(Type::Bool);
        // The loop owns the cursor predicate across iterations.
        // A direct pull initializes both projected items before the common target stores.
        let from_row = Expr {
            ty: Type::Bool,
            kind: ExprKind::Compare(
                vec![binder.pattern_local(ordinary), Expr::boolean(true)],
                vec![ast::CmpOp::Is],
            ),
        };
        let direct = vec![
            Statement::IterNext {
                kind: ir::IterNextKind::Generic,
                iterator,
                key: first,
                value: Some(second),
                ready,
            },
            pipelines::Prepared::exhaust_if_empty(Expr::local(ready, Type::Bool)),
        ];
        body = vec![Statement::If(from_row, body, direct)];
        Some(ordinary)
    } else {
        None
    };
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
    Ok(pair)
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
            self.names
                .get(name.id.as_str())
                .map(|&slot| (self.locals[slot], self.declared_slots.get(&slot).copied()))
        } else if self.registered.is_some() && target.is_attribute_expr() {
            None
        } else {
            self.assignment_context(target, rhs)
        }
    }

    fn assignment_rhs(
        &mut self,
        target: &ast::Expr,
        value: &ast::Expr,
        (expected, declaration): (Type, Option<TypeDeclaration>),
    ) -> Result<Expr, String> {
        if self.inference.is_some() && expected == Type::JsonObject && target.is_attribute_expr() {
            // Keep a mutable owner until the resolved target decides its storage.
            self.document_annotation_value(value, expected, documents::MAP)
        } else {
            self.expr_in_context(
                value,
                expected,
                |_, _| false,
                Self::coerce_supplied_result,
                declaration,
            )
        }
    }

    /// Keep a terminating RHS and any fresh name declarations, without evaluating its targets.
    pub(super) fn ends_before_assignment(
        &mut self,
        source: &ast::Expr,
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
                self.declare(name.id.as_str(), value.ty, target)?;
            }
        }
        if let Some(sites) = &mut self.fixed_sites {
            sites.statement(source.range(), FixedNode::Effect);
        }
        Ok(true)
    }

    /// A chain shares one RHS object and visits targets in source order. A later
    /// annotated target can supply the type of an otherwise empty literal.
    pub(super) fn assignment_statement(
        &mut self,
        assignment: &ast::StmtAssign,
        output: &mut Vec<Statement>,
    ) -> Result<bool, String> {
        // The live namespace owns later lookups. An enclosing call may already
        // have loaded its callable before an argument mutates this attribute.
        for target in &assignment.targets {
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
        let fixed_field = match assignment.targets.as_slice() {
            [ast::Expr::Attribute(field)] if self.registered.is_none() => Some(field),
            _ => None,
        };
        if let Some(attribute) = fixed_field
            && let Some(field) = self.mime_attribute(attribute)
        {
            let mut state_assignment = assignment.clone();
            state_assignment.targets[0] = ast::Expr::Attribute(field);
            return self.assignment_statement(&state_assignment, output);
        }
        if let Some(attribute) = fixed_field
            && attribute.attr.as_str() == "default_factory"
            && (matches!(&*assignment.value, ast::Expr::NoneLiteral(_))
                || self.unshadowed_builtin(&assignment.value, "list"))
        {
            let receiver = self.expr(&attribute.value)?;
            if self.types.is_default_list_dict(receiver.ty) {
                let enabled = !matches!(&*assignment.value, ast::Expr::NoneLiteral(_));
                output.push(Statement::Expr(Expr {
                    ty: Type::None,
                    kind: ExprKind::Builtin(Builtin::DefaultFactorySet { enabled }, vec![receiver]),
                }));
                return Ok(true);
            }
        }
        if let Some(attribute) = fixed_field
            && attribute.attr.as_str() == "converter"
            && let Some(Binding::Intrinsic(Intrinsic::ClockConverter(utc))) =
                self.resolve(&assignment.value)
        {
            let receiver = self.expr(&attribute.value)?;
            if receiver.ty == Type::LogFormatter {
                output.push(self.formatter_set(receiver, attribute, Expr::boolean(utc))?);
                return Ok(true);
            }
        }
        if let [target] = assignment.targets.as_slice() {
            if pattern_elements(target).is_some() {
                let value = self.unpacking_value(target, &assignment.value)?;
                bind_assignment(self, &assignment.targets, value, output)?;
                return Ok(true);
            }
            let local = target.as_name_expr().filter(|_| {
                !self.module_scope && self.block_depth == 1 && self.local_functions.is_empty()
            });
            let previous = local.and_then(|name| {
                let slot = *self.names.get(name.id.as_str())?;
                self.inferred_containers
                    .contains(&slot)
                    .then_some((name.id.as_str(), slot))
            });
            let context = self.target_expectation(target, Some(&assignment.value));
            let scalar_item = context
                .filter(|(ty, _)| target.is_subscript_expr() && ty.builtin_scalar_name().is_some());
            let value = if let Some((_, old)) = previous {
                // An empty literal still needs the old context. Other container
                // expressions can determine their own concrete result type.
                self.expr_expected_if(
                    &assignment.value,
                    self.locals[old],
                    Self::rebindable_container,
                )?
            } else if let Some((expected, _)) = scalar_item {
                // Keep the scalar operand. Its RHS can change which container receives the store.
                self.expr_expected_if(&assignment.value, expected, |_, _| true)?
            } else if let Some(expected) = context {
                self.assignment_rhs(target, &assignment.value, expected)?
            } else {
                self.typed_expr(&assignment.value)?
            };
            if self.ends_before_assignment(&assignment.value, &value, &assignment.targets)? {
                output.push(Statement::Expr(value));
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
                self.consuming_lists.capture_split(target.range(), value)
            } else {
                value
            };
            let value_site = scalar_item.map(|_| assignment.value.as_ref());
            let start = output.len();
            self.store_value(target, value, value_site, Some(output))?;
            if let Some(sites) = &mut self.fixed_sites
                && let Some(name) = target.as_name_expr()
                && let [Statement::Assign(slot, _)] = &output[start..]
                && self.names.get(name.id.as_str()) == Some(slot)
            {
                sites.statement(assignment.value.range(), FixedNode::Store(*slot));
            } else if self.class.is_some()
                && let Some(sites) = &mut self.fixed_sites
                && let ast::Expr::Attribute(attribute) = target
                && let [Statement::SetField(_, field, _)] = &output[start..]
            {
                sites.store_field(attribute, assignment.value.range(), *field);
            }
            return Ok(true);
        }
        let context = assignment.targets.iter().find_map(|target| {
            self.target_expectation(target, Some(&assignment.value))
                .map(|ty| (target, ty))
        });
        let value = if let Some((target, expected)) = context {
            assignments::unpromoted(self.assignment_rhs(target, &assignment.value, expected)?)
        } else {
            let target = assignment
                .targets
                .iter()
                .find(|target| pattern_elements(target).is_some())
                .unwrap_or(&assignment.targets[0]);
            self.unpacking_value(target, &assignment.value)?
        };
        if self.ends_before_assignment(&assignment.value, &value, &assignment.targets)? {
            output.push(Statement::Expr(value));
            return Ok(false);
        }
        bind_assignment(self, &assignment.targets, value, output)?;
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
                        Builtin::Process(ir::ProcessOperation::EnvironSet { bytes }),
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
                let value = self
                    .coerce(value, element, site)
                    .map_err(|error| error.expected_from(declaration, site.range()));
                // A direct RHS owns its error before the key. An unpacked value is placed after its key.
                if value_site.is_some()
                    && let Err(error) = value
                {
                    return Err(error);
                }
                let index = if matches!(receiver.ty, Type::Deque(_)) {
                    self.integer_index(&subscript.slice)?
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
                        &receiver,
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
                if self.resolve(target) == Some(Binding::Intrinsic(Intrinsic::PopenSelector)) {
                    let value = self.coerce(value, Type::ProcessSelectorClass, target)?;
                    break 'place Statement::Expr(Expr {
                        ty: Type::None,
                        kind: ExprKind::Builtin(
                            Builtin::Process(ir::ProcessOperation::SetSelector),
                            vec![value],
                        ),
                    });
                }
                if self.cache_config(target) == Some(true) {
                    let value = self.coerce(value, self.types.optional(Type::Str), target)?;
                    break 'place Statement::Expr(cache_paths::op(
                        ir::CachePathOperation::Set(true),
                        Type::None,
                        vec![value],
                    ));
                }
                if let Some(Binding::Intrinsic(Intrinsic::LogRecordFlag(mask))) =
                    self.resolve(target)
                {
                    let value = self.coerce(value, Type::Bool, target)?;
                    break 'place Statement::Expr(logging_records::op(
                        ir::LogRecordOperation::SetFlag,
                        Type::None,
                        vec![Expr::integer(mask), value],
                    ));
                }
                if self.resolve(target) == Some(Binding::Intrinsic(Intrinsic::BuiltinsLen)) {
                    let value = self.coerce(value, self.builtin_len().ty, target)?;
                    break 'place Statement::Expr(Expr {
                        ty: Type::None,
                        kind: ExprKind::Builtin(Builtin::BuiltinsSetLen, vec![value]),
                    });
                }
                if self.resolve(target) == Some(Binding::Intrinsic(Intrinsic::Stdout)) {
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
                if self.resolve(target) == Some(Binding::Intrinsic(Intrinsic::BuiltinsLast)) {
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
                        Builtin::Process(ir::ProcessOperation::PopenSetStatus),
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
                        Builtin::Process(ir::ProcessOperation::PopenSetPoll {
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
                Statement::SetField(receiver, field, value)
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
                .assignment_rhs(target, value, expected)
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
        row: Option<Row<'_>>,
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
            let mut snapshot = PatternSnapshot {
                body: Vec::with_capacity(items.len()),
                clear: Vec::with_capacity(items.len()),
            };
            for item in items {
                snapshot.capture(self, item);
            }
            return Ok(snapshot);
        }
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
            if star.is_none() {
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
            start: None,
        };
        let (iteration, _) = self.lower_iteration(&item_target, input, &[], |checker, body| {
            let item = checker.local(checker.names[&name]);
            let item = checker.coerce(item, expected, target)?;
            body.push(Statement::Expr(builtin(
                Type::None,
                Builtin::Append,
                vec![checker.local(result), item],
            )));
            Ok(())
        })?;
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
