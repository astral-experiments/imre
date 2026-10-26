//! Assignment targets consume one owned RHS, with a snapshot at each unpacking
//! level so target mutation cannot change values already removed from a sequence.

use super::*;

/// Builds a checked-count constant without introducing a source-level binding.
fn integer(value: impl ToString) -> Expr {
    Expr {
        ty: Type::Int,
        kind: ExprKind::Int(value.to_string()),
    }
}

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
        kind: ExprKind::Compare(vec![left, right], vec![op]),
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
        kind: ExprKind::Binary(Binary::Concat, Box::new(left), Box::new(right)),
    }
}

/// Uses the sealed ValueError category so ordinary handlers catch arity errors.
fn unpack_error(message: Expr) -> Statement {
    Statement::Raise(Some(Expr {
        ty: Type::Exception,
        kind: ExprKind::Exception {
            kind: exception_kind("ValueError").unwrap(),
            message: Some(Box::new(message)),
        },
    }))
}

impl Checker<'_> {
    /// Finds a target's storage without executing its receiver or index.
    fn target_context(&self, target: &ast::Expr) -> Option<Type> {
        if let ast::Expr::Name(name) = target {
            self.names
                .get(name.id.as_str())
                .map(|&slot| self.locals[slot])
        } else {
            self.assignment_context(target)
        }
    }

    /// Yields the captured value and clears its slot before evaluating the next target.
    fn take_temporary(&self, slot: usize) -> Expr {
        Expr {
            ty: self.locals[slot],
            kind: ExprKind::Scope {
                body: Vec::new(),
                value: Box::new(self.local(slot)),
                clear: vec![slot],
            },
        }
    }

    /// A chain shares one RHS object and visits targets in source order. A later
    /// annotated target can supply the type of an otherwise empty literal.
    pub(super) fn assignment_statement(
        &mut self,
        assignment: &ast::StmtAssign,
    ) -> Result<Vec<Statement>, String> {
        if let [ast::Expr::Attribute(attribute)] = assignment.targets.as_slice()
            && let Some(field) = self.mime_attribute(attribute)
        {
            let mut state_assignment = assignment.clone();
            state_assignment.targets[0] = ast::Expr::Attribute(field);
            return self.assignment_statement(&state_assignment);
        }
        if let [ast::Expr::Attribute(attribute)] = assignment.targets.as_slice()
            && attribute.attr.as_str() == "default_factory"
            && (matches!(&*assignment.value, ast::Expr::NoneLiteral(_))
                || self.unshadowed_builtin(&assignment.value, "list"))
        {
            let receiver = self.expr(&attribute.value)?;
            if self.types.is_default_list_dict(receiver.ty) {
                let enabled = !matches!(&*assignment.value, ast::Expr::NoneLiteral(_));
                return Ok(vec![Statement::Expr(Expr {
                    ty: Type::None,
                    kind: ExprKind::Builtin(Builtin::DefaultFactorySet { enabled }, vec![receiver]),
                })]);
            }
        }
        if let [ast::Expr::Attribute(attribute)] = assignment.targets.as_slice()
            && attribute.attr.as_str() == "converter"
            && let Some(Binding::Intrinsic(Intrinsic::ClockConverter(utc))) =
                self.resolve(&assignment.value)
        {
            let receiver = self.expr(&attribute.value)?;
            if receiver.ty == Type::LogFormatter {
                return Ok(vec![self.formatter_set(
                    receiver,
                    attribute,
                    Expr {
                        ty: Type::Bool,
                        kind: ExprKind::Bool(utc),
                    },
                )?]);
            }
        }
        // A literal tuple assigned to plain locals can stage its elements in native
        // slots. This also permits unpacking a typed callable without storing it
        // in a heap object. All RHS expressions still precede all target writes.
        if let [ast::Expr::Tuple(targets)] = assignment.targets.as_slice()
            && let ast::Expr::Tuple(values) = &*assignment.value
            && targets.elts.len() == values.elts.len()
            && targets
                .elts
                .iter()
                .all(|target| matches!(target, ast::Expr::Name(_)))
            && !values
                .elts
                .iter()
                .any(|value| matches!(value, ast::Expr::Starred(_)))
        {
            let mut body = Vec::new();
            let mut slots = Vec::new();
            for (target, source) in targets.elts.iter().zip(&values.elts) {
                let value = self.unpacking_value(target, source)?;
                let slot = self.hidden(value.ty);
                body.push(Statement::Assign(slot, value));
                slots.push(slot);
            }
            for (target, &slot) in targets.elts.iter().zip(&slots) {
                let value = self.take_temporary(slot);
                body.push(self.assign_value(target, value)?);
            }
            return Ok(vec![Statement::Scope { body, clear: slots }]);
        }
        if let [target] = assignment.targets.as_slice() {
            if pattern_elements(target).is_some() {
                let value = self.unpacking_value(target, &assignment.value)?;
                return self.bind_pattern(target, value);
            }
            let value = if let Some(expected) = self.target_context(target) {
                self.expr_expected(&assignment.value, expected)?
            } else {
                self.typed_expr(&assignment.value)?
            };
            return Ok(vec![self.assign_value(target, value)?]);
        }
        let context = assignment
            .targets
            .iter()
            .find_map(|target| self.target_context(target));
        let value = if let Some(expected) = context {
            assignments::unpromoted(self.expr_expected(&assignment.value, expected)?)
        } else {
            let target = assignment
                .targets
                .iter()
                .find(|target| pattern_elements(target).is_some())
                .unwrap_or(&assignment.targets[0]);
            self.unpacking_value(target, &assignment.value)?
        };
        let captured = self.hidden(value.ty);
        let mut body = vec![Statement::Assign(captured, value)];
        for (index, target) in assignment.targets.iter().enumerate() {
            let value = if index + 1 == assignment.targets.len() {
                self.take_temporary(captured)
            } else {
                self.local(captured)
            };
            body.extend(self.bind_pattern(target, value)?);
        }
        Ok(vec![Statement::Scope {
            body,
            clear: vec![captured],
        }])
    }

    /// Receivers and indices are checked after the RHS and preceding targets.
    /// SetItem/SetField preserve this same RHS-first order in generated code.
    pub(super) fn assign_value(
        &mut self,
        target: &ast::Expr,
        value: Expr,
    ) -> Result<Statement, String> {
        match target {
            ast::Expr::Name(_) => self.assign(target, value),
            ast::Expr::Subscript(subscript) => {
                let receiver = self.expr(&subscript.value)?;
                if let Some((key, element)) = self.types.chainmap_entries(receiver.ty) {
                    let value = self.coerce(value, element, target)?;
                    let key = self.expr_expected(&subscript.slice, key)?;
                    return Ok(Statement::Expr(Expr {
                        ty: Type::None,
                        kind: ExprKind::Builtin(
                            Builtin::ChainMapSetItem,
                            vec![value, receiver, key],
                        ),
                    }));
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
                    return Ok(Statement::Expr(scope.finish(assigned)));
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
                        return Ok(Statement::Expr(documents::op(
                            ir::DocumentOp::ReplaceItems,
                            Type::None,
                            vec![value, receiver],
                        )));
                    }
                    let value = self.coerce(value, documents::ANY, target)?;
                    let key = self.expr_expected(&subscript.slice, Type::Str)?;
                    return Ok(Statement::Expr(documents::op(
                        ir::DocumentOp::SetItem,
                        Type::None,
                        vec![value, receiver, key],
                    )));
                }
                let receiver = self.json_mutation_receiver(receiver, subscript)?;
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
                let index = if matches!(receiver.ty, Type::Deque(_)) {
                    self.integer_index(&subscript.slice)?
                } else {
                    self.expr_expected(&subscript.slice, key)?
                };
                let value = self.coerce(value, element, target)?;
                Ok(Statement::SetItem(receiver, index, value))
            }
            ast::Expr::Attribute(attribute) => {
                if self.resolve(target) == Some(Binding::Intrinsic(Intrinsic::PopenSelector)) {
                    let value = self.coerce(value, Type::ProcessSelectorClass, target)?;
                    return Ok(Statement::Expr(Expr {
                        ty: Type::None,
                        kind: ExprKind::Builtin(
                            Builtin::Process(ir::ProcessOperation::SetSelector),
                            vec![value],
                        ),
                    }));
                }
                if self.cache_config(target) == Some(true) {
                    let value = self.coerce(value, self.types.optional(Type::Str), target)?;
                    return Ok(Statement::Expr(cache_paths::op(
                        ir::CachePathOperation::Set(true),
                        Type::None,
                        vec![value],
                    )));
                }
                if let Some(Binding::Intrinsic(Intrinsic::LogRecordFlag(mask))) =
                    self.resolve(target)
                {
                    let value = self.coerce(value, Type::Bool, target)?;
                    return Ok(Statement::Expr(logging_records::op(
                        ir::LogRecordOperation::SetFlag,
                        Type::None,
                        vec![logging::number(mask), value],
                    )));
                }
                if self.resolve(target) == Some(Binding::Intrinsic(Intrinsic::BuiltinsLen)) {
                    let value = self.coerce(value, self.builtin_len().ty, target)?;
                    return Ok(Statement::Expr(Expr {
                        ty: Type::None,
                        kind: ExprKind::Builtin(Builtin::BuiltinsSetLen, vec![value]),
                    }));
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
                    return Ok(Statement::Expr(Expr {
                        ty: Type::None,
                        kind: ExprKind::Builtin(Builtin::SysStdoutSet, vec![value]),
                    }));
                }
                if self.resolve(target) == Some(Binding::Intrinsic(Intrinsic::BuiltinsLast)) {
                    if !sys::displayable(value.ty) {
                        return Err(error(
                            self.source,
                            target,
                            "builtins._ requires a supported display value",
                        ));
                    }
                    return Ok(Statement::Expr(Expr {
                        ty: Type::None,
                        kind: ExprKind::Builtin(Builtin::BuiltinsSetLast, vec![value]),
                    }));
                }
                if let Some(class) = self.class_reference(&attribute.value)
                    && matches!(
                        self.classes.declarations.definitions[class]
                            .static_bindings
                            .get(attribute.attr.as_str()),
                        Some(crate::classes::ClassValue::Runtime { .. })
                    )
                {
                    return self.store_class_data(class, attribute, value);
                }
                let receiver = self.expr(&attribute.value)?;
                if receiver.ty == Type::SysImplementation && attribute.attr.as_str() == "cache_tag"
                {
                    let value = self.coerce(value, self.types.optional(Type::Str), target)?;
                    return Ok(Statement::Expr(cache_paths::op(
                        ir::CachePathOperation::Set(false),
                        Type::None,
                        vec![value, receiver],
                    )));
                }
                if matches!(receiver.ty, Type::Popen(..)) && attribute.attr.as_str() == "returncode"
                {
                    let value = self.coerce(value, self.types.optional(Type::Int), target)?;
                    return Ok(Statement::Expr(builtin(
                        Type::None,
                        Builtin::Process(ir::ProcessOperation::PopenSetStatus),
                        vec![value, receiver],
                    )));
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
                    return Ok(Statement::Expr(builtin(
                        Type::None,
                        Builtin::Process(ir::ProcessOperation::PopenSetPoll {
                            returns_none: returns == Type::None,
                        }),
                        vec![value, receiver],
                    )));
                }
                if let Some(class) =
                    self.mutable_instance_data(receiver.ty, attribute.attr.as_str())
                {
                    return self.store_instance_data(class, attribute, receiver, value);
                }
                if self.types.has_reflected_functions()
                    && matches!(receiver.ty, Type::FunctionRef | Type::Callable(_))
                    && let Some(index) = typing::attribute_index(attribute.attr.as_str())
                {
                    let value = self.coerce(value, Type::Str, target)?;
                    return Ok(Statement::Expr(Expr {
                        ty: Type::None,
                        kind: ExprKind::Builtin(
                            Builtin::Typing(ir::TypingOperation::SetAttr(index)),
                            vec![value, receiver],
                        ),
                    }));
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
                    return Ok(Statement::Expr(codecs::writer(
                        op,
                        Type::None,
                        vec![value, receiver],
                    )));
                }
                if receiver.ty == Type::LogManager && attribute.attr.as_str() == "disable" {
                    return self.set_manager_disable(receiver, value, target);
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
                    return Ok(Statement::Expr(Expr {
                        ty: Type::None,
                        kind: ExprKind::Builtin(Builtin::Logging(op), vec![value, receiver]),
                    }));
                }
                if receiver.ty == Type::LogFormatter {
                    return self.formatter_set(receiver, attribute, value);
                }
                if let Type::CmpKey(frame, true) = receiver.ty
                    && attribute.attr.as_str() == "obj"
                {
                    let cell = self.cmp_field(receiver, frame, 1);
                    let value = self.coerce(value, self.types.element(cell.ty).unwrap(), target)?;
                    return Ok(Statement::SetItem(cell, pipelines::integer(0), value));
                }
                if let Type::SimpleNamespace(id) = receiver.ty {
                    let name = self.namespace_attr_name(attribute)?;
                    let item = self.types.mapping(Type::Dict(id)).unwrap().1;
                    let value = self.coerce(value, item, target)?;
                    return Ok(Statement::SetItem(
                        super::namespaces::backing(receiver, id),
                        name,
                        value,
                    ));
                }
                if matches!(receiver.ty, Type::ModuleSpec(_))
                    && attribute.attr.as_str() == "has_location"
                {
                    return self.set_module_spec_location(attribute, receiver, value);
                }
                if receiver.ty == Type::LogRecord {
                    let value = self.check_record_value(value, target)?;
                    return Ok(Statement::Expr(logging_records::op(
                        ir::LogRecordOperation::Put,
                        Type::None,
                        vec![receiver, string(attribute.attr.as_str()), value],
                    )));
                }

                if attribute.attr.as_str() == "MIN_READ_SIZE" && receiver.ty == Type::BinaryIO {
                    let value = self.coerce(value, Type::Int, target)?;
                    // Assignment evaluates the RHS before the receiver.
                    return Ok(Statement::Expr(Expr {
                        ty: Type::None,
                        kind: ExprKind::Builtin(Builtin::ZipMinReadSize, vec![value, receiver]),
                    }));
                }
                if let Type::ChainMap(id) = receiver.ty
                    && attribute.attr.as_str() == "maps"
                {
                    let maps = self.coerce(value, self.types.list(Type::Dict(id)), target)?;
                    return Ok(Statement::Expr(Expr {
                        ty: Type::None,
                        kind: ExprKind::Builtin(Builtin::ChainMapSetMaps, vec![maps, receiver]),
                    }));
                }
                if let Some(class) = self.writable_property(receiver.ty, attribute) {
                    return self.set_property(class, attribute, receiver, value);
                }
                if let Some(field) = self.setup_field(receiver.ty, attribute.attr.as_str()) {
                    return self.store_setup_field(receiver, value, attribute, field);
                }
                let (receiver, field, ty) = self.writable_field_value(attribute, receiver)?;
                let value = self.coerce(value, ty, target)?;
                Ok(Statement::SetField(receiver, field, value))
            }
            _ => Err(error(self.source, target, "unsupported assignment target")),
        }
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
                        && let Some(context) = self.target_context(&starred.value)
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
                        .collect::<Result<Vec<_>, _>>()?;
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
                            let context = self.target_context(&starred.value)?;
                            self.types
                                .element(self.types.payload(context).unwrap_or(context))
                        }
                        _ => self.target_context(target),
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
                            .collect::<Result<Vec<_>, _>>()?;
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
                            .collect::<Result<Vec<_>, _>>()?;
                        return Ok(Expr {
                            ty: self.types.list(actual),
                            kind: ExprKind::List(values),
                        });
                    }
                    return self.expr_expected(value, self.types.list(element));
                }
            }
        }
        if let Some(expected) = self.target_context(target) {
            return self
                .expr_expected(value, expected)
                .map(assignments::unpromoted);
        }
        self.expr(value)
    }

    /// Each sequence level is checked and captured before its target stores;
    /// nested levels are entered only when their target is reached.
    pub(super) fn bind_pattern(
        &mut self,
        target: &ast::Expr,
        value: Expr,
    ) -> Result<Vec<Statement>, String> {
        self.bind_pattern_with_root(target, value, None)
    }

    /// An iterator's private fixed tuple cannot be rebound by source targets.
    /// Reuse that root while snapshotting all fields before user stores, making
    /// the aggregate projection-only and eligible for scalar replacement.
    pub(super) fn bind_iterator_pattern(
        &mut self,
        target: &ast::Expr,
        source: usize,
    ) -> Result<Vec<Statement>, String> {
        let stable = matches!(self.locals[source], Type::Tuple(_)).then_some(source);
        self.bind_pattern_with_root(target, self.local(source), stable)
    }

    /// A supplied root belongs exclusively to compiler iteration state; ordinary
    /// source assignments retain a separate snapshot through target effects.
    fn bind_pattern_with_root(
        &mut self,
        target: &ast::Expr,
        value: Expr,
        stable: Option<usize>,
    ) -> Result<Vec<Statement>, String> {
        let Some(targets) = pattern_elements(target) else {
            return Ok(vec![self.assign_value(target, value)?]);
        };
        let mut stars = targets
            .iter()
            .enumerate()
            .filter(|(_, target)| matches!(target, ast::Expr::Starred(_)));
        let star = stars.next().map(|(index, _)| index);
        if stars.next().is_some() {
            return Err(error(
                self.source,
                target,
                "an unpacking target may contain only one starred target",
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
        let (captured, mut body) = if let Some(captured) = stable {
            (captured, Vec::new())
        } else {
            let captured = self.hidden(value.ty);
            (captured, vec![Statement::Assign(captured, value)])
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
                    comparison(self.local(length), Compare::Less, integer(fixed)),
                    vec![unpack_error(message)],
                    Vec::new(),
                ));
            }
            if star.is_none() {
                body.push(Statement::If(
                    comparison(self.local(length), Compare::Greater, integer(fixed)),
                    vec![unpack_error(string(format!(
                        "too many values to unpack (expected {fixed})"
                    )))],
                    Vec::new(),
                ));
            }
        }
        let mut items = Vec::with_capacity(targets.len());
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
                    _ => Builtin::ListGet,
                };
                builtin(
                    element.unwrap(),
                    kind,
                    vec![self.local(captured), integer(position)],
                )
            };
            let slot = self.hidden(item.ty);
            body.push(Statement::Assign(slot, item));
            clear.push(slot);
            items.push(slot);
        }
        body.push(Statement::Clear(captured));
        for (target, item) in targets.iter().zip(items) {
            let target = if let ast::Expr::Starred(starred) = target {
                &starred.value
            } else {
                target
            };
            body.extend(self.bind_pattern(target, self.take_temporary(item))?);
        }
        Ok(vec![Statement::Scope { body, clear }])
    }

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
            .target_context(target)
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
            .collect::<Result<Vec<_>, _>>()?;
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
                Some(value) => ExprKind::Present(Box::new(integer(value))),
                None => ExprKind::Absent(Box::new(Expr {
                    ty: Type::None,
                    kind: ExprKind::None,
                })),
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
            .target_context(target)
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
        let (iteration, _) = self.lower_iteration(&item_target, input, &[], |checker| {
            let item = checker.local(checker.names[&name]);
            let item = checker.coerce(item, expected, target)?;
            Ok(vec![Statement::Expr(builtin(
                Type::None,
                Builtin::Append,
                vec![checker.local(result), item],
            ))])
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
