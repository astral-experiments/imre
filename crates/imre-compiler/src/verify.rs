//! Structural checks at IR pass boundaries. These validate recorded types and
//! operation shapes, not the alias, dominance or range proofs that produced them.

use crate::ir::{
    self, Builtin, CallArgs, Expr, ExprKind, Function, InstanceType, Module, Statement, Type,
};
use ruff_python_ast::UnaryOp;

/// Checks identities before layouts, then validates statements and expression shapes.
pub(crate) fn module(module: &Module) -> Result<(), String> {
    require(
        module.source_frames.is_empty()
            || (module.source_frames.len() == module.functions.len() + 1
                && module.source_frames.last().is_some_and(Option::is_some)),
        "source frame metadata must cover the functions and module entry",
    )?;
    let classes = module.class_fields.len();
    module.types.verify_shapes(classes)?;
    require(
        module.class_metadata.len() == classes,
        "class metadata/layout count mismatch",
    )?;
    for &ty in module
        .globals
        .iter()
        .chain(module.class_fields.iter().flatten())
    {
        module.types.verify_type(ty, classes)?;
    }
    for schema in &module.json_records {
        let fields = module
            .class_fields
            .get(schema.class)
            .ok_or("invalid JSON record class")?;
        require(
            schema.fields.len() == fields.len(),
            "JSON record field count mismatch",
        )?;
    }
    // Validate every signature before checking references from other functions.
    for function in module.functions.iter().chain([&module.entry]) {
        let verifier = Verifier { module, function };
        for &ty in function.locals.iter().chain([&function.returns]) {
            verifier.ty(ty)?;
        }
        verifier.distinct_slots(&function.params)?;
    }
    for (index, function) in module.functions.iter().chain([&module.entry]).enumerate() {
        let verifier = Verifier { module, function };
        let result = (|| {
            // Shape accessors assume valid identities. Check expression and
            // embedded metadata types before inspecting any operand's shape.
            expressions(&function.body, |value| verifier.expression_types(value))?;
            verifier.block(&function.body)?;
            expressions(&function.body, |value| verifier.expression(value))
        })();
        result.map_err(|error| {
            let name = if index == module.functions.len() {
                "entry".into()
            } else {
                format!("function #{index}")
            };
            format!("{name}: {error}")
        })?;
    }
    Ok(())
}

/// Registered frames resolve slots by their original Python names and can carry either a
/// checked native scalar or a Python handle as a display operand. They have no native
/// container layout table. Validate this handoff before the emitter indexes its operands.
pub(crate) fn registered(frame: crate::frontend::RegisteredRegion<'_>) -> Result<(), String> {
    let private = frame.frame_layout(|_| true).unwrap();
    let local = |slot: usize| {
        require(
            slot < frame.locals.len()
                && (frame.local_names.contains_key(&slot) || private.index(&slot).is_some()),
            "invalid registered frame slot or missing Python name",
        )
    };
    for &slot in frame.params {
        local(slot)?;
    }
    for &slot in frame.local_names.keys() {
        local(slot)?;
    }
    let mut result = Ok(());
    frame.walk(&mut |node, parent| {
        if result.is_err() {
            return;
        }
        result = match node {
            ir::Node::Statement(Statement::Assign(slot, _)) => local(*slot),
            ir::Node::Statement(Statement::With {
                binding: Some(binding),
                ..
            }) => require(
                frame.locals.get(binding.entered) == Some(&Type::PythonObject)
                    && private.index(&binding.entered).is_some(),
                "context binding needs a private entered-value slot",
            ),
            ir::Node::Statement(Statement::IterNext {
                kind: kind @ (ir::IterNextKind::Generic | ir::IterNextKind::Async(_)),
                iterator,
                key,
                ready,
                value,
            }) => require(
                value.is_none_or(|slot| {
                    *kind == ir::IterNextKind::Generic
                        && slot != *iterator
                        && slot != *key
                        && slot != *ready
                        && private.index(&slot).is_some()
                        && frame.locals.get(slot) == Some(&Type::PythonObject)
                }) && iterator != key
                    && key != ready
                    && iterator != ready
                    && [iterator, key, ready]
                        .iter()
                        .all(|slot| private.index(slot).is_some())
                    && frame.locals.get(*iterator) == Some(&Type::PythonObject)
                    && frame.locals.get(*key) == Some(&Type::PythonObject)
                    && frame.locals.get(*ready) == Some(&Type::Bool),
                "registered pull needs distinct private cursor, item and readiness slots",
            ),
            ir::Node::Expression(value) => match &value.kind {
                ExprKind::Local(slot) => local(*slot),
                ExprKind::List(_) => require(
                    matches!(value.ty, Type::PythonObject | Type::ListInt | Type::List(_)),
                    "invalid checked list type",
                ),
                ExprKind::Tuple(_) => require(
                    matches!(value.ty, Type::PythonObject | Type::Tuple(_)),
                    "invalid checked tuple type",
                ),
                ExprKind::Set { element, values } => require(
                    match value.ty {
                        Type::PythonObject => *element == Type::PythonObject,
                        Type::Set(_) => {
                            *element != Type::PythonObject
                                && values.iter().all(|item| {
                                    matches!(
                                        item.kind,
                                        ExprKind::Builtin(Builtin::RegisteredSpread, _)
                                    ) || item.ty == *element
                                })
                        }
                        _ => false,
                    },
                    "inconsistent checked set element type",
                ),
                ExprKind::Builtin(Builtin::GlobalSet(slot), args) => local(*slot).and_then(|()| {
                    require(
                        args.len() == 1,
                        "registered namespace write needs one operand",
                    )
                }),
                ExprKind::Builtin(Builtin::RegisteredAssertionFailure { .. }, args) => require(
                    value.ty == Type::None && args.len() <= 1,
                    "registered assertion failure takes at most one message",
                ),
                ExprKind::Invoke { .. } => require(
                    value.is_registered_invoke(),
                    "registered invocation needs a Python callable, call position and positional arguments",
                ),
                ExprKind::Builtin(Builtin::RegisteredLocation(..), args) => match args.as_slice() {
                    [inner] => same(value.ty, inner.ty),
                    _ => Err("registered location needs one operand".into()),
                },
                ExprKind::Builtin(Builtin::RegisteredSpread, args) => {
                    let collection = parent.is_some_and(|parent| match &parent.kind {
                        ExprKind::List(_) | ExprKind::Tuple(_) | ExprKind::Set { .. } => true,
                        ExprKind::Builtin(Builtin::Append | Builtin::SetAdd, args) => {
                            matches!(args.as_slice(), [out, item] if out.ty == Type::PythonObject && std::ptr::eq(item, value))
                        }
                        _ => false,
                    });
                    require(
                        collection && value.ty == Type::PythonObject && args.len() == 1,
                        "registered spread needs one operand in a Python collection",
                    )
                }
                ExprKind::Builtin(Builtin::RegisteredGenerator(_, scoped, _), args) => {
                    ir::generator_operands(*scoped, args)
                        .map(|_| ())
                        .map_err(str::to_owned)
                }
                ExprKind::Builtin(op, args) => op
                    .iteration_operands(args)
                    .transpose()
                    .map(|_| ())
                    .map_err(str::to_owned),
                _ => Ok(()),
            },
            _ => Ok(()),
        };
    });
    result?;
    // Replacing an operation can leave an allocated local unused.
    require(
        private.slots().iter().all(|slot| {
            frame.frame_slots.index(slot).is_some()
                && frame.frame_slots.cursor(slot) == private.cursor(slot)
        }),
        "registered frame storage disagrees with its scoped operations",
    )
}

fn require(condition: bool, message: &str) -> Result<(), String> {
    if condition {
        Ok(())
    } else {
        Err(message.into())
    }
}

fn same(actual: Type, expected: Type) -> Result<(), String> {
    if actual == expected {
        Ok(())
    } else {
        Err(format!("expected {expected}, got {actual}"))
    }
}

fn expressions(
    body: &[Statement],
    mut visit: impl FnMut(&Expr) -> Result<(), String>,
) -> Result<(), String> {
    let mut result = Ok(());
    ir::walk_expressions(body, &mut |value| {
        if result.is_ok() {
            result = visit(value);
        }
    });
    result
}

struct Verifier<'a> {
    module: &'a Module,
    function: &'a Function,
}

impl Verifier<'_> {
    fn ty(&self, ty: Type) -> Result<(), String> {
        self.module
            .types
            .verify_type(ty, self.module.class_fields.len())
    }

    fn local(&self, slot: usize) -> Result<Type, String> {
        self.function
            .locals
            .get(slot)
            .copied()
            .ok_or_else(|| format!("invalid local #{slot}"))
    }

    fn slots(&self, slots: &[usize]) -> Result<(), String> {
        for &slot in slots {
            self.local(slot)?;
        }
        Ok(())
    }

    fn distinct_slots(&self, slots: &[usize]) -> Result<(), String> {
        let mut seen = vec![false; self.function.locals.len()];
        for &slot in slots {
            self.local(slot)?;
            require(
                !seen[slot],
                "duplicate local in parameter or iterator slots",
            )?;
            seen[slot] = true;
        }
        Ok(())
    }

    fn callee(&self, id: usize) -> Result<&Function, String> {
        self.module
            .functions
            .get(id)
            .ok_or_else(|| format!("invalid function #{id}"))
    }

    fn arguments(&self, args: &CallArgs, params: &[Type]) -> Result<(), String> {
        require(
            args.values.len() == params.len() && args.order.len() == params.len(),
            "call argument count does not match signature",
        )?;
        let mut seen = vec![false; args.values.len()];
        for (&index, &expected) in args.order.iter().zip(params) {
            let value = args
                .values
                .get(index)
                .ok_or("call argument order has an invalid index")?;
            require(!seen[index], "call argument order is not a permutation")?;
            seen[index] = true;
            same(value.ty, expected)?;
        }
        Ok(())
    }

    fn field(&self, receiver: Type, field: usize, value: Type) -> Result<(), String> {
        let fields = match receiver {
            Type::Class(class) => self.module.class_fields[class].clone(),
            Type::Tuple(_) => self.module.types.fields(receiver).unwrap(),
            Type::ModuleSpec(id) => self.module.types.module_spec_fields(id),
            Type::Range => vec![Type::Int; 3],
            Type::SysImplementation => crate::version::implementation_fields(&self.module.types),
            Type::CompletedProcess(capture, cmd) => {
                return match field {
                    0 => same(value, self.module.types.completed_command(cmd)),
                    1 => same(value, Type::Int),
                    2 | 3 => same(value, capture.stream(field == 2)),
                    _ => Err(format!("invalid process field #{field}")),
                };
            }
            _ => return Err(format!("field access on {receiver}")),
        };
        same(
            value,
            *fields
                .get(field)
                .ok_or_else(|| format!("invalid field #{field} on {receiver}"))?,
        )
    }

    fn item(&self, receiver: Type, index: Type) -> Result<Type, String> {
        if let Some(element) = self.module.types.indexed_element(receiver) {
            same(index, Type::Int)?;
            Ok(element)
        } else if let Some((key, value)) = self.module.types.mapping(receiver) {
            same(index, key)?;
            Ok(value)
        } else {
            Err(format!("item assignment on {receiver}"))
        }
    }

    fn block(&self, body: &[Statement]) -> Result<(), String> {
        for (index, statement) in body.iter().enumerate() {
            self.statement(statement)
                .map_err(|error| format!("statement {index}: {error}"))?;
        }
        Ok(())
    }

    fn statement(&self, statement: &Statement) -> Result<(), String> {
        match statement {
            Statement::Assign(slot, value) => same(value.ty, self.local(*slot)?),
            Statement::Move(target, source) => same(self.local(*source)?, self.local(*target)?),
            Statement::Clear(slot) => self.local(*slot).map(|_| ()),
            Statement::With { binding, body, .. } => {
                if let Some(binding) = binding {
                    same(self.local(binding.entered)?, Type::PythonObject)?;
                    self.block(&binding.body)?;
                }
                self.block(body)
            }
            Statement::Scope { body, clear } => {
                self.slots(clear)?;
                self.block(body)
            }
            Statement::SetItem(_, receiver, index, value) => {
                same(value.ty, self.item(receiver.ty, index.ty)?)
            }
            Statement::AugmentItem(receiver, index, _, _) => {
                self.item(receiver.ty, index.ty).map(|_| ())
            }
            Statement::SetField(receiver, field, value) => {
                self.field(receiver.ty, *field, value.ty)
            }
            Statement::AugmentField(receiver, field, _, _) => {
                let Type::Class(class) = receiver.ty else {
                    return Err("augmented field assignment needs a class receiver".into());
                };
                require(
                    *field < self.module.class_fields[class].len(),
                    "invalid augmented field index",
                )
            }
            Statement::Match(_, cases) => {
                for case in cases {
                    if case.captures.len() != case.pattern.names.len() {
                        return Err("match capture names and slots have different lengths".into());
                    }
                    self.slots(&case.captures)?;
                    if let Some(guard) = &case.guard {
                        same(guard.ty, Type::Bool)?;
                    }
                    self.block(&case.body)?;
                }
                Ok(())
            }
            Statement::If(test, yes, no) | Statement::While(test, yes, no) => {
                same(test.ty, Type::Bool)?;
                self.block(yes)?;
                self.block(no)
            }
            Statement::For {
                target,
                cursor,
                stop,
                step,
                args,
                body,
                orelse,
            } => {
                for slot in [target, cursor, stop, step] {
                    same(self.local(*slot)?, Type::Int)?;
                }
                require(
                    (1..=3).contains(&args.len()),
                    "range loop needs one to three arguments",
                )?;
                for arg in args {
                    same(arg.ty, Type::Int)?;
                }
                self.block(body)?;
                self.block(orelse)
            }
            Statement::IterNext {
                iterator,
                key,
                value,
                ready,
                kind,
            } => {
                let item_type = match kind {
                    crate::ir::IterNextKind::String | crate::ir::IterNextKind::SplitField => {
                        Some(Type::Str)
                    }
                    crate::ir::IterNextKind::CodePoint | crate::ir::IterNextKind::Utf8Byte => {
                        Some(Type::Int)
                    }
                    crate::ir::IterNextKind::Async(_) => {
                        return Err("async cursor requires a registered frame".into());
                    }
                    crate::ir::IterNextKind::Generic => None,
                };
                if let Some(ty) = item_type {
                    same(self.local(*key)?, ty)?;
                    require(value.is_none(), "a string cursor yields one item")?;
                }
                let ty = self.local(*iterator)?;
                if let Some(element) = self.module.types.cursor_element(ty) {
                    same(self.local(*key)?, element)?;
                    require(value.is_none(), "a typed cursor yields one item")?;
                } else {
                    same(ty, Type::Iterator)?;
                }
                same(self.local(*ready)?, Type::Bool)?;
                let slots = [*iterator, *key, *ready]
                    .into_iter()
                    .chain(*value)
                    .collect::<Vec<_>>();
                self.distinct_slots(&slots)
            }
            Statement::Return(value) => same(value.ty, self.function.returns),
            Statement::Try {
                body,
                handlers,
                orelse,
                finalbody,
            } => {
                self.block(body)?;
                let subgroup = |h: &crate::ir::ExceptHandler| {
                    matches!(h.matcher, crate::ir::ExceptionMatcher::Subgroup(..))
                };
                if let Some(first) = handlers.first()
                    && handlers.iter().any(|h| subgroup(h) != subgroup(first))
                {
                    return Err("cannot mix whole-exception and subgroup handlers".into());
                }
                for handler in handlers {
                    if let Some(slot) = handler.name {
                        self.local(slot)?;
                    }
                    self.block(&handler.body)?;
                }
                self.block(orelse)?;
                self.block(finalbody)
            }
            Statement::Raise(Some(value)) => {
                require(
                    matches!(
                        value.exception.ty,
                        Type::Exception | Type::ImportException | Type::TimeoutExpired
                    ),
                    "raise needs an exception",
                )?;
                require(
                    value.cause.as_ref().is_none_or(|cause| {
                        self.module.types.payload(cause.ty) == Some(Type::Exception)
                    }),
                    "raise cause needs an optional exception",
                )
            }
            Statement::Raise(None)
            | Statement::Expr(_)
            | Statement::Position(..)
            | Statement::Break(_)
            | Statement::Continue(_)
            | Statement::Exhaust => Ok(()),
        }
    }

    fn expression_types(&self, value: &Expr) -> Result<(), String> {
        self.ty(value.ty)?;
        match &value.kind {
            ExprKind::Builtin(Builtin::Unrepresented, _) => {
                Err("unrepresented source operand in executable IR".into())
            }
            ExprKind::Set { element, .. } => self.ty(*element),
            ExprKind::Builtin(
                Builtin::JsonDecode(ty)
                | Builtin::JsonEncode(ty)
                | Builtin::JsonEncoderEncode(ty)
                | Builtin::JsonDumps(ty)
                | Builtin::JsonDump(ty)
                | Builtin::TomlDecode(ty)
                | Builtin::DictTryGet(ty)
                | Builtin::ChainMapGet(ty)
                | Builtin::DictTryPop(ty),
                _,
            ) => self.ty(*ty),
            ExprKind::Builtin(Builtin::IsInstance(kinds), _) => {
                for kind in kinds {
                    if let InstanceType::Exact(ty) = kind {
                        self.ty(*ty)?;
                    }
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn expression(&self, value: &Expr) -> Result<(), String> {
        let types = &self.module.types;
        match &value.kind {
            ExprKind::Int(_) => same(value.ty, Type::Int),
            ExprKind::Float(_) => same(value.ty, Type::Float),
            ExprKind::Bool(_) => same(value.ty, Type::Bool),
            ExprKind::Str(text) => {
                require(
                    matches!(value.ty, Type::Str | Type::StrLiteral(_)),
                    "string literal has non-string type",
                )?;
                require(
                    types
                        .literal_strings(value.ty)
                        .is_none_or(|choices| choices.contains(text)),
                    "string literal is outside its recorded domain",
                )
            }
            ExprKind::Bytes(_) => same(value.ty, Type::Bytes),
            ExprKind::None => same(value.ty, Type::None),
            ExprKind::Local(slot) => same(value.ty, self.local(*slot)?),
            ExprKind::Bind(slot, inner) => {
                same(value.ty, inner.ty)?;
                let storage = self.local(*slot)?;
                let payload = types.payload(storage).unwrap_or(storage);
                let source = types.payload(inner.ty).unwrap_or(inner.ty);
                require(
                    storage == inner.ty
                        || types.payload(storage) == Some(inner.ty)
                        || (types.payload(storage).is_some() && inner.ty == Type::None)
                        || (types
                            .members(payload)
                            .is_some_and(|members| members.contains(&source))
                            && (types.payload(inner.ty).is_none()
                                || types.payload(storage).is_some())),
                    "binding value cannot be promoted to its local storage",
                )
            }
            ExprKind::Scope {
                body,
                value: inner,
                clear,
            } => {
                self.slots(clear)?;
                same(value.ty, inner.ty)?;
                self.block(body)
            }
            ExprKind::List(values) => {
                let element = types
                    .element(value.ty)
                    .ok_or("list expression has non-list type")?;
                for value in values {
                    same(value.ty, element)?;
                }
                Ok(())
            }
            ExprKind::Dict(_) if value.ty == Type::PythonObject => Ok(()),
            ExprKind::Dict(entries) => {
                let (key, item) = types
                    .mapping(value.ty)
                    .ok_or("dictionary expression needs mapping storage")?;
                self.ty(key)?;
                self.ty(item)?;
                for entry in entries {
                    let k = entry
                        .key
                        .as_ref()
                        .ok_or("fixed dictionary display cannot expand a mapping")?;
                    same(k.ty, key)?;
                    same(entry.value.ty, item)?;
                }
                Ok(())
            }
            ExprKind::Set { element, values } => {
                require(
                    types.set_element(value.ty) == Some(*element),
                    "set expression has inconsistent element type",
                )?;
                for value in values {
                    same(value.ty, *element)?;
                }
                Ok(())
            }
            ExprKind::Tuple(values) => {
                let fields = if let Type::ModuleSpec(id) = value.ty {
                    types.module_spec_fields(id)
                } else {
                    types
                        .fields(value.ty)
                        .ok_or("tuple expression has non-tuple type")?
                };
                require(fields.len() == values.len(), "tuple field count mismatch")?;
                for (value, field) in values.iter().zip(fields) {
                    same(value.ty, field)?;
                }
                Ok(())
            }
            ExprKind::Present(inner) => require(
                types.payload(value.ty) == Some(inner.ty),
                "present value does not match optional payload",
            ),
            ExprKind::Absent(_) => require(
                types.payload(value.ty).is_some(),
                "absent value must have optional storage",
            ),
            ExprKind::Unwrap(inner) => require(
                types.payload(inner.ty) == Some(value.ty),
                "unwrap result does not match optional payload",
            ),
            ExprKind::Compare(values, ops) => {
                if value.ty != Type::PythonObject {
                    same(value.ty, Type::Bool)?;
                }
                require(
                    !ops.is_empty() && values.len() == ops.len() + 1,
                    "comparison operand/operator count mismatch",
                )
            }
            ExprKind::Boolean(_, values) => {
                same(value.ty, Type::Bool)?;
                require(!values.is_empty(), "boolean expression has no operands")?;
                for value in values {
                    same(value.ty, Type::Bool)?;
                }
                Ok(())
            }
            ExprKind::Conditional(test, yes, no) => {
                same(test.ty, Type::Bool)?;
                same(yes.ty, value.ty)?;
                same(no.ty, value.ty)
            }
            ExprKind::Unary(_, _) if value.ty == Type::PythonObject => Ok(()),
            ExprKind::Unary(UnaryOp::Not, _) => same(value.ty, Type::Bool),
            ExprKind::Unary(_, operand) => same(value.ty, operand.ty),
            ExprKind::Call(id, args) => {
                let callee = self.callee(*id)?;
                same(value.ty, callee.returns)?;
                self.arguments(
                    args,
                    &callee
                        .params
                        .iter()
                        .map(|&slot| callee.locals[slot])
                        .collect::<Vec<_>>(),
                )
            }
            ExprKind::Callable { function, receiver } => {
                let callee = self.callee(*function)?;
                let (params, returns) = types
                    .signature(value.ty)
                    .ok_or("callable expression has non-callable type")?;
                let mut expected = callee.params.iter().map(|&slot| callee.locals[slot]);
                if let Some(receiver) = receiver {
                    same(
                        receiver.ty,
                        expected
                            .next()
                            .ok_or("bound callable target has no receiver parameter")?,
                    )?;
                }
                require(
                    params.into_iter().eq(expected),
                    "callable parameter types do not match target",
                )?;
                // A cold callable returns the coroutine to Python. Its native
                // adapter runs the target body when awaited and returns its payload.
                let native_result = if matches!(returns, Type::Coroutine(_)) {
                    types.awaited(returns).unwrap()
                } else {
                    returns
                };
                same(native_result, callee.returns)
            }
            ExprKind::Invoke { callable, args } => {
                require(
                    args.site.python().is_none(),
                    "fixed invocation cannot use a Python call site",
                )?;
                let (params, returns) = types
                    .signature(callable.ty)
                    .ok_or("indirect call needs a callable")?;
                same(value.ty, returns)?;
                self.arguments(args, &params)
            }
            ExprKind::Construct { initializer, args } => {
                require(
                    matches!(value.ty, Type::Class(_)),
                    "constructor result must be a class",
                )?;
                let callee = self.callee(*initializer)?;
                same(callee.returns, Type::None)?;
                let mut params = callee.params.iter().map(|&slot| callee.locals[slot]);
                same(
                    value.ty,
                    params
                        .next()
                        .ok_or("initializer has no receiver parameter")?,
                )?;
                self.arguments(args, &params.collect::<Vec<_>>())
            }
            ExprKind::Exception { message, .. } => {
                same(value.ty, Type::Exception)?;
                if let Some(message) = message {
                    same(message.ty, Type::Str)?;
                }
                Ok(())
            }
            ExprKind::Field(receiver, field) => self.field(receiver.ty, *field, value.ty),
            ExprKind::Builtin(operation, args) => {
                match operation {
                    Builtin::RecordFromFields => {
                        let layout: &[Type] = match value.ty {
                            Type::Range => &[Type::Int; 3],
                            Type::Class(id) => self
                                .module
                                .class_fields
                                .get(id)
                                .ok_or("unknown class layout")?,
                            _ => return Err("nominal record layout required".into()),
                        };
                        require(
                            layout.len() == args.len(),
                            "record field count differs from its layout",
                        )?;
                        for (field, expected) in args.iter().zip(layout) {
                            same(field.ty, *expected)?;
                        }
                    }
                    Builtin::GlobalGet(slot) => {
                        let ty = *self
                            .module
                            .globals
                            .get(*slot)
                            .ok_or("invalid global slot")?;
                        require(args.is_empty(), "global read takes no arguments")?;
                        same(value.ty, ty)?;
                    }
                    Builtin::GlobalSet(slot) => {
                        let ty = *self
                            .module
                            .globals
                            .get(*slot)
                            .ok_or("invalid global slot")?;
                        let [arg] = args.as_slice() else {
                            return Err("global write needs one argument".into());
                        };
                        same(arg.ty, ty)?;
                        same(value.ty, Type::None)?;
                    }
                    Builtin::StrEnumMember { class, member } => {
                        let metadata = self
                            .module
                            .class_metadata
                            .get(*class)
                            .ok_or("invalid enum class")?;
                        require(*member < metadata.enum_members.len(), "invalid enum member")?;
                        require(
                            matches!(value.ty, Type::IntEnum(id) | Type::StrEnum(id) | Type::Enum(id) if id == *class),
                            "enum member result has the wrong class",
                        )?;
                    }
                    Builtin::SortedRecordRows { class, columns } => {
                        let fields = self
                            .module
                            .class_fields
                            .get(*class)
                            .ok_or("invalid sorted row class")?;
                        require(
                            fields.len() == columns.len()
                                && columns
                                    .iter()
                                    .all(|&column| usize::from(column) <= args.len()),
                            "invalid sorted row columns",
                        )?;
                    }
                    _ => {}
                }
                operation
                    .verify_specialization(args, value.ty, self.module)
                    .map_err(str::to_owned)
            }
            // Arithmetic and value-selecting boolean semantics remain the
            // frontend's responsibility; all child identities are still checked.
            ExprKind::BooleanValue(_, left, _, Some(predicate)) => {
                same(left.ty, self.local(predicate.receiver)?)?;
                same(predicate.condition.ty, Type::Bool)
            }
            ExprKind::Binary(..) | ExprKind::BooleanValue(..) => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty() -> Module {
        Module {
            types: Default::default(),
            class_fields: Vec::new(),
            list_builds: Default::default(),
            class_metadata: Vec::new(),
            command_flags: None,
            builtin_module_sources: None,
            json_records: Vec::new(),
            globals: Vec::new(),
            source_frames: Vec::new(),
            functions: Vec::new(),
            entry: Function {
                params: Vec::new(),
                returns: Type::None,
                locals: Vec::new(),
                body: Vec::new(),
            },
        }
    }

    fn reject(module: &Module, expected: &str) {
        let error = super::module(module).unwrap_err();
        assert!(error.contains(expected), "{error}");
    }

    #[test]
    fn registered_display_handoff_accepts_heterogeneous_operands_and_checks_frame_slots() {
        use crate::{frontend, modules::SourceGraph};
        let text = "out = [1, ('s', callback(arg)), *rest]\n";
        let graph = SourceGraph::single(text).unwrap();
        let source = frontend::RegisteredSource {
            python_optimization: 0,
            operand_types: None,
            template_fields: None,
            generators: None,
            lambdas: None,
            text,
            private_names: None,
            replacements: &Default::default(),
            method_imports: &Default::default(),
        };
        let operations = Default::default();
        let body = &graph.modules[graph.entry].parsed.syntax().body;
        for scope in [
            frontend::RegisteredScope::Module,
            frontend::RegisteredScope::Function { is_async: false },
        ] {
            let mut frame =
                frontend::RegisteredFunction::from_suite(body, scope, source, &operations, true)
                    .unwrap();
            registered(frame.region()).unwrap();
            frame.local_names.clear();
            assert!(
                registered(frame.region())
                    .unwrap_err()
                    .contains("frame slot")
            );
        }
    }

    #[test]
    fn registered_namespace_checks_the_optional_parent_before_emission() {
        let mut frame = crate::frontend::RegisteredFunction {
            inputs: Default::default(),
            frame_slots: Default::default(),
            fixed_sites: Box::default(),
            fixed_attributes: None,
            function: empty().entry,
            local_names: Default::default(),
        };
        for count in 0..=2 {
            let parent = Expr::none();
            frame.function.body = vec![Statement::Expr(Expr {
                ty: Type::PythonObject,
                kind: ExprKind::Builtin(
                    Builtin::RegisteredNamespace(Vec::new()),
                    vec![parent; count],
                ),
            })];
            if count < 2 {
                registered(frame.region()).unwrap();
            } else {
                assert_eq!(
                    registered(frame.region()).unwrap_err(),
                    "registered namespace takes at most one parent"
                );
            }
        }
    }

    #[test]
    fn registered_display_handoff_rejects_a_misplaced_or_malformed_spread() {
        let mut frame = crate::frontend::RegisteredFunction {
            inputs: Default::default(),
            frame_slots: Default::default(),
            fixed_sites: Box::default(),
            fixed_attributes: None,
            function: empty().entry,
            local_names: Default::default(),
        };
        let spread = |args| Expr {
            ty: Type::PythonObject,
            kind: ExprKind::Builtin(Builtin::RegisteredSpread, args),
        };
        frame.function.body = vec![Statement::Expr(spread(vec![Expr::integer(42)]))];
        assert!(registered(frame.region()).unwrap_err().contains("spread"));
        frame.function.body = vec![Statement::Expr(Expr {
            ty: Type::PythonObject,
            kind: ExprKind::Tuple(vec![Expr::integer(42), spread(vec![])]),
        })];
        assert!(registered(frame.region()).unwrap_err().contains("spread"));
    }

    #[test]
    fn checks_type_identities_before_using_shape_accessors() {
        for ty in [Type::List(usize::MAX), Type::Class(0), Type::Callable(0)] {
            let mut module = empty();
            module.entry.locals.push(ty);
            reject(&module, "invalid type identity");
        }
        let mut module = empty();
        let Type::List(id) = module.types.list(Type::Str) else {
            unreachable!()
        };
        module.entry.locals.push(Type::Dict(id));
        reject(&module, "invalid type identity");

        let module = empty();
        module.types.list(Type::List(0));
        reject(&module, "forward or cyclic reference");
    }

    #[test]
    fn checks_call_permutations_types_and_results() {
        let mut module = empty();
        module.functions.push(Function {
            params: vec![0, 1],
            returns: Type::None,
            locals: vec![Type::Int, Type::Float],
            body: Vec::new(),
        });
        let mut args = CallArgs {
            site: ir::CallSite::None,
            exhaustive_enum_match: false,
            values: vec![
                Expr {
                    ty: Type::Float,
                    kind: ExprKind::Float(2.0),
                },
                Expr::integer(1),
            ],
            order: vec![1, 0],
        };
        let call = |args| {
            Statement::Expr(Expr {
                ty: Type::None,
                kind: ExprKind::Call(0, args),
            })
        };
        module.entry.body = vec![call(args.clone())];
        super::module(&module).unwrap();
        for (order, error) in [
            (vec![1, 1], "not a permutation"),
            (vec![1, 2], "invalid index"),
            (vec![1], "argument count"),
            (vec![0, 1], "expected int, got float"),
        ] {
            args.order = order;
            module.entry.body = vec![call(args.clone())];
            reject(&module, error);
        }
        module.entry.body = vec![Statement::Expr(Expr {
            ty: Type::Int,
            kind: ExprKind::Call(
                0,
                CallArgs::positional(vec![Expr::integer(1), Expr::integer(1)]),
            ),
        })];
        reject(&module, "expected None, got int");
        module.entry.body = vec![Statement::Expr(Expr {
            ty: Type::None,
            kind: ExprKind::Call(1, CallArgs::positional(Vec::new())),
        })];
        reject(&module, "invalid function #1");
    }

    #[test]
    fn checks_bound_targets_and_indirect_signatures() {
        let mut module = empty();
        module.functions.push(Function {
            params: vec![0, 1],
            returns: Type::Int,
            locals: vec![Type::Str, Type::Int],
            body: Vec::new(),
        });
        let signature = module.types.callable(vec![Type::Int], Type::Int);
        let receiver = Expr {
            ty: Type::Str,
            kind: ExprKind::Str("receiver".into()),
        };
        let callable = Expr {
            ty: signature,
            kind: ExprKind::Callable {
                function: 0,
                receiver: Some(Box::new(receiver)),
            },
        };
        let invoke = |args| {
            Statement::Expr(Expr {
                ty: Type::Int,
                kind: ExprKind::Invoke {
                    callable: Box::new(callable.clone()),
                    args,
                },
            })
        };
        module.entry.body = vec![invoke(CallArgs::positional(vec![Expr::integer(1)]))];
        super::module(&module).unwrap();
        let mut frame_args = CallArgs::positional(vec![Expr::integer(1)]);
        frame_args.site = ir::CallSite::Python(ir::InvokeSite {
            range: ruff_text_size::TextRange::new(0.into(), 4.into()),
            callee_end: 1.into(),
            suspended_name: None,
            scope: None,
        });
        module.entry.body = vec![invoke(frame_args)];
        reject(&module, "fixed invocation cannot use a Python call site");
        module.entry.body = vec![invoke(CallArgs::positional(Vec::new()))];
        reject(&module, "argument count");
        module.functions[0].locals[0] = Type::Float;
        module.entry.body = vec![Statement::Expr(callable)];
        reject(&module, "expected float, got str");
    }

    #[test]
    fn checks_the_awaited_result_of_a_coroutine_callback() {
        let mut module = empty();
        module.functions.push(Function {
            params: Vec::new(),
            returns: Type::Int,
            locals: Vec::new(),
            body: vec![Statement::Return(Expr::integer(1))],
        });
        let make_callback = |result| {
            Statement::Expr(Expr {
                ty: module
                    .types
                    .callable(Vec::new(), module.types.coroutine(result)),
                kind: ExprKind::Callable {
                    function: 0,
                    receiver: None,
                },
            })
        };
        module.entry.body = vec![make_callback(Type::Int)];
        super::module(&module).unwrap();
        module.entry.body = vec![make_callback(Type::Float)];
        reject(&module, "expected int, got float");
    }

    #[test]
    fn weak_proxy_upgrade_returns_its_referent_class() {
        let module = empty();
        let check = |input, result| {
            let arg = Expr::local(0, input);
            Builtin::WeakProxyTarget.verify_specialization(&[arg], result, &module)
        };
        assert!(check(Type::WeakProxy(0), Type::Class(0)).is_ok());
        assert!(check(Type::WeakProxy(0), Type::Class(1)).is_err());
        assert!(check(Type::Class(0), Type::Class(0)).is_err());
    }

    #[test]
    fn callable_context_returns_the_bound_method_receiver_class() {
        let source = "\
from enum import Enum
class Mode(Enum):
    ON = 'on'
    def label(self, value: int = 1) -> int:
        return value
class Item:
    def label(self, value: int = 1) -> int:
        return value
";
        let graph = crate::modules::SourceGraph::single(source).unwrap();
        let module = crate::frontend::lower(&graph).unwrap();
        let class = |name| {
            module
                .class_metadata
                .iter()
                .position(|c| c.name == name)
                .unwrap()
        };
        let item = class("Item");
        let mode = class("Mode");
        let receiver = Type::Class(item);
        let signature = module.types.callable(vec![Type::Int], Type::Int);
        let method = |owner| {
            Expr::local(
                0,
                module
                    .types
                    .source_method(signature, owner, 0, Some(Type::Class(owner))),
            )
        };
        let check = |args: &[Expr], result| {
            Builtin::CallableContext.verify_specialization(args, result, &module)
        };
        assert!(check(&[method(item)], receiver).is_ok());
        assert!(check(&[method(item)], Type::Class(mode)).is_err());
        assert!(check(&[method(mode)], Type::Class(mode)).is_err());
        assert!(check(&[method(item)], Type::Int).is_err());
        assert!(check(&[Expr::local(0, signature)], receiver).is_err());
        assert!(check(&[Expr::integer(1)], receiver).is_err());
        assert!(check(&[], receiver).is_err());
        assert!(check(&[method(item), method(item)], receiver).is_err());
    }

    #[test]
    fn checks_sumprod_float_result_requires_a_nonempty_list_literal() {
        let mut module = empty();
        let floats = module.types.list(Type::Float);
        module.entry.locals = vec![floats, Type::Float];
        let unknown = Expr::local(0, floats);
        let literal = |items| Expr {
            ty: floats,
            kind: ExprKind::List(items),
        };
        let empty = literal(vec![]);
        let nonempty = literal(vec![Expr::local(1, Type::Float)]);
        let exposed = Expr {
            ty: floats,
            kind: ExprKind::Bind(0, Box::new(nonempty.clone())),
        };
        let mut verify = |left, right, result| {
            module.entry.body = vec![Statement::Expr(Expr {
                ty: result,
                kind: ExprKind::Builtin(Builtin::Sumprod, vec![left, right]),
            })];
            super::module(&module)
        };
        verify(nonempty.clone(), unknown.clone(), Type::Float).unwrap();
        verify(unknown.clone(), nonempty.clone(), Type::Float).unwrap();
        verify(unknown.clone(), unknown.clone(), Type::ObjectValue).unwrap();
        verify(empty.clone(), empty.clone(), Type::ObjectValue).unwrap();

        let ints = Expr {
            ty: Type::ListInt,
            kind: ExprKind::List(vec![Expr::integer(1)]),
        };
        for (left, right) in [
            (unknown.clone(), unknown.clone()),
            (empty.clone(), empty),
            (exposed, unknown),
            (ints, nonempty),
        ] {
            let error = verify(left, right, Type::Float).unwrap_err();
            assert!(error.contains("invalid specialized operation"), "{error}");
        }
    }

    #[test]
    fn checks_specialized_shapes_and_bounds_without_reproving_them() {
        let mut module = empty();
        module.entry.locals = vec![Type::ListInt, Type::Int];
        let read = |min, max, args| {
            Statement::Expr(Expr {
                ty: Type::Int,
                kind: ExprKind::Builtin(Builtin::ListGetBoundedInBounds { min, max }, args),
            })
        };
        let args = vec![Expr::local(0, Type::ListInt), Expr::local(1, Type::Int)];
        module.entry.body = vec![read(0, 10, args.clone())];
        super::module(&module).unwrap();
        for (min, max, args) in [
            (10, 0, args.clone()),
            (0, 1_i64 << 62, args.clone()),
            (0, 10, vec![]),
            (
                0,
                10,
                vec![
                    Expr {
                        ty: Type::ListInt,
                        kind: ExprKind::List(Vec::new()),
                    },
                    Expr::integer(1),
                ],
            ),
            (
                0,
                10,
                vec![Expr::local(0, Type::ListInt), Expr::integer(-1)],
            ),
        ] {
            module.entry.body = vec![read(min, max, args)];
            reject(&module, "invalid specialized operation");
        }
    }

    #[test]
    fn visits_hidden_scopes_and_reports_the_failing_stage() {
        let mut module = empty();
        module.entry.body = vec![Statement::If(
            Expr {
                ty: Type::Bool,
                kind: ExprKind::Bool(false),
            },
            vec![Statement::Expr(Expr {
                ty: Type::Int,
                kind: ExprKind::Scope {
                    body: vec![Statement::Clear(7)],
                    value: Box::new(Expr::integer(1)),
                    clear: Vec::new(),
                },
            })],
            Vec::new(),
        )];
        let error = crate::verify_after(&module, "list bounds specialization").unwrap_err();
        assert!(
            error.contains("after list bounds specialization: entry:"),
            "{error}"
        );
        assert!(error.contains("invalid local #7"), "{error}");
        module.entry.locals.push(Type::Int);
        module.entry.body = vec![Statement::Scope {
            body: Vec::new(),
            clear: vec![0, 0],
        }];
        // Repeated cleanup is harmless because Clear restores the empty value.
        super::module(&module).unwrap();
        module.entry.params = vec![0, 0];
        reject(&module, "duplicate local");
    }

    #[test]
    fn rejects_inconsistent_expression_storage() {
        let mut module = empty();
        module.entry.locals.push(Type::Bool);
        module.entry.body = vec![Statement::Assign(0, Expr::integer(1))];
        reject(&module, "expected bool, got int");
        module.entry.body = vec![Statement::Expr(Expr {
            ty: module.types.tuple(vec![Type::Int]),
            kind: ExprKind::Tuple(Vec::new()),
        })];
        reject(&module, "tuple field count");
        module.entry.body = vec![Statement::Expr(Expr {
            ty: module.types.optional(Type::Str),
            kind: ExprKind::Present(Box::new(Expr::integer(1))),
        })];
        reject(&module, "optional payload");
    }
}
