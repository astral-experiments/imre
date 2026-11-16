//! Physical sites for operations that can also consume Python frame arguments.

use super::*;

#[derive(Clone)]
pub(crate) struct FixedBodySource {
    pub(crate) method: Option<crate::classes::MethodSource>,
    pub(super) arguments: FrameArguments,
    pub(crate) attributes: HashMap<TextRange, FixedAttribute>,
    sites: Vec<(TextRange, Option<FixedNode>)>,
    pub(crate) local_names: HashMap<usize, registered::LocalBinding>,
}

pub(super) type PlacedBody = StdResult<RegisteredFunction, HashMap<TextRange, FixedAttribute>>;

impl FixedBodySource {
    pub(super) fn has_method_store(&self) -> bool {
        self.method.is_some()
            && self
                .sites
                .iter()
                .any(|(_, node)| matches!(node, Some(FixedNode::StoreField(..))))
    }

    /// Layout-only tests have no source owner. A deferred call requires `place_in`.
    #[cfg(test)]
    pub(crate) fn place(self, function: ir::Function) -> PlacedBody {
        self.place_with(function, |_| false)
    }

    pub(crate) fn place_in(
        self,
        function: ir::Function,
        graph: &SourceGraph,
        module: ModuleId,
    ) -> PlacedBody {
        let source = std::cell::LazyCell::new(|| registered::function_source(graph, module));
        self.place_with(function, |attribute| attribute.bind_original(*source))
    }

    /// Place the owned operations only if every physical site still belongs to them.
    /// The ordinary Python binder can still use the resolved attributes if placement fails.
    fn place_with(
        mut self,
        mut function: ir::Function,
        bind_original: impl Fn(&mut FixedAttribute) -> bool,
    ) -> PlacedBody {
        let mut sites = self.sites.into_iter();
        let mut fixed_sites = Vec::with_capacity(sites.len());
        let mut python_arguments = self.arguments == FrameArguments::Python;
        let mut condition = None;
        let complete = ir::all_nodes(&function.body, &mut |node, parent| {
            if let ir::Node::Statement(Statement::If(test, ..)) = node {
                condition = Some(test);
            }
            if !node.has_fixed_site_in(parent) {
                return true;
            }
            let Some((range, Some(expected))) = sites.next() else {
                return false;
            };
            python_arguments |= matches!(
                expected,
                FixedNode::StoreField(..) | FixedNode::Container(..)
            );
            if !expected.matches_in(&node, condition, parent, |expr| {
                let attr = self.attributes.get(&range)?;
                let next = sites.as_slice().first().map(|site| site.0);
                let (_, args) = attr.call_operands(range, next, expr)?;
                Some(FixedNode::indexed(&expr.kind, args.len(), expr.ty))
            }) {
                return false;
            }
            if let ir::Node::Statement(Statement::ContainerFor(input)) = node {
                for slot in input.slots {
                    let Some(ty) = function.locals.get_mut(slot) else {
                        return false;
                    };
                    *ty = Type::PythonObject;
                }
            }
            fixed_sites.push(range);
            true
        });
        if !complete || sites.len() != 0 {
            return Err(self.attributes);
        }
        drop(sites);
        if self
            .attributes
            .values_mut()
            .any(|attr| matches!(attr, FixedAttribute::OriginalCall(_)) && !bind_original(attr))
        {
            return Err(self.attributes);
        }
        let mut frame = RegisteredFunction {
            placed_from: 0,
            fixed_sites: fixed_sites.into_boxed_slice(),
            inputs: match self.method {
                Some(method) => FrameInputs::Protocol {
                    sites: Box::new([FrameSite::Method(method)]),
                },
                None => FrameInputs::default(),
            },
            function,
            local_names: self.local_names,
            frame_slots: Default::default(),
            fixed_attributes: (!self.attributes.is_empty()).then(|| Box::new(self.attributes)),
        };
        if (matches!(frame.inputs, FrameInputs::Checked { .. })
            && python_arguments
            && !frame.bind_python_arguments())
            || frame.local_names.len() + frame.frame_slots.slots().len()
                != frame.function.locals.len()
        {
            return Err(frame
                .fixed_attributes
                .map_or_else(HashMap::new, |attrs| *attrs));
        }
        Ok(frame)
    }
}

#[derive(Clone)]
pub(crate) enum FixedAttribute {
    /// The statement owns this whole call's traceback display.
    UnhighlightedCall(TextRange),
    CallValue {
        checked: Box<registered::values::CheckedCall>,
        placed: bool,
    },
    /// A placed module read retains its native slot in the IR and its Python name here.
    ModuleName(ast::name::Name),
    Load(ast::Identifier),
    /// The original indexed call supplies the live imported callable if a Python frame is selected.
    OriginalCall(ast::NodeIndex),
    /// The fixed builtin already carries its method receiver. A standalone callee is saved here.
    Call(Option<Box<Expr>>, Box<ir::RegisteredCall>),
}

impl FixedAttribute {
    pub(super) fn bound_call(
        call: &ast::ExprCall,
        target: Expr,
        layout: Option<ir::RegisteredArguments>,
    ) -> Self {
        let (site, target) = registered::values::callable_target(call).bind(call, target, layout);
        Self::Call(Some(target), site)
    }

    pub(super) fn is_standalone_call(&self) -> bool {
        matches!(self, Self::Call(Some(_), _) | Self::OriginalCall(_))
    }

    /// A statement can retain its call position before the Python entry binds the target.
    pub(super) fn has_call_binding(&self) -> bool {
        matches!(self, Self::OriginalCall(_)) || self.call_binding().is_some()
    }

    pub(crate) fn call_binding(&self) -> Option<(Option<&Expr>, &ir::RegisteredCall)> {
        match self {
            Self::Call(target, site) => Some((target.as_deref(), site)),
            Self::CallValue {
                checked,
                placed: true,
            } => Some((None, checked.call.python_call()?)),
            _ => None,
        }
    }

    /// Borrow the written target and arguments from the checked operation in evaluation order.
    pub(crate) fn call_operands<'a>(
        &'a self,
        range: TextRange,
        next: Option<TextRange>,
        expr: &'a Expr,
    ) -> Option<(&'a Expr, &'a [Expr])> {
        let (target, call) = self.call_binding()?;
        if matches!(expr.kind, ExprKind::Invoke { .. }) {
            let callee = next?;
            if target.is_some()
                || call.range != range
                || callee.is_empty()
                || !call.contains_target(Some(callee))
                || !matches!(call.target, ir::RegisteredCallTarget::Callable { suspended_name, .. }
                    if suspended_name.is_none_or(|name| name == callee))
                || !matches!(call.arguments, ir::RegisteredArguments::Positional)
                || call.partition_field.is_some()
            {
                return None;
            }
            return expr.fixed_positional_invoke_operands();
        }
        match target {
            Some(target) => Some((
                target,
                expr.named_call_arguments()
                    .or_else(|| expr.coroutine_arguments())?,
            )),
            None => expr.named_call_arguments()?.split_first(),
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub(super) enum FixedNode {
    Position(TextRange),
    Return,
    Container(usize, TextRange),
    Effect,
    Reraise,
    Raise {
        cause: bool,
    },
    Exception {
        kind: u32,
        message: bool,
    },
    Store(usize),
    StoreField(ir::FieldUse, Option<Binary>),
    NativeCall(
        TextRange,
        std::mem::Discriminant<ir::ProcessOperation>,
        usize,
        Type,
    ),
    Branch(usize, usize),
    TryFinally(usize, usize),
    TryExcept(usize, u32, TextRange, usize),
    /// The operation keeps its checked slot or operand count, and its native type.
    Indexed(std::mem::Discriminant<ExprKind>, usize, Type),
    NamedCall(std::mem::Discriminant<ExprKind>, usize, usize, Type),
    Invocation(Box<ir::RegisteredCall>, Type),
    Located(TextRange, TextSize, Option<Type>, Type),
    Name(ruff_python_ast::name::Name, Type),
    Attribute(
        ruff_python_ast::name::Name,
        TextRange,
        TextSize,
        Option<Type>,
        Type,
    ),
    Integer(i64),
    Scalar(ir::Constant),
    String(Box<str>),
    Unary(ast::UnaryOp, Type),
    Binary(Binary, Type),
    Comparison(ast::CmpOp),
    Logical(bool, usize),
    Copy(ir::CopyOperation, usize, Type),
    Text(ir::TextOperation, usize, Type),
    Path(ir::PathOperation, usize, Type),
    Builtin(std::mem::Discriminant<Builtin>, usize, Type),
}

impl FixedNode {
    fn indexed(kind: &ExprKind, index: usize, ty: Type) -> Self {
        Self::Indexed(std::mem::discriminant(kind), index, ty)
    }

    fn positional_invoke(expr: &Expr) -> Option<Self> {
        let (_, args) = expr.fixed_positional_invoke_operands()?;
        Some(Self::indexed(&expr.kind, args.len(), expr.ty))
    }

    /// Compare owned payloads against the final instruction without copying them.
    fn matches(&self, node: &ir::Node<'_>) -> bool {
        self.matches_in(node, None, None, Self::positional_invoke)
    }

    fn matches_in(
        &self,
        node: &ir::Node<'_>,
        condition: Option<&Expr>,
        parent: Option<&Expr>,
        positional: impl FnOnce(&Expr) -> Option<Self>,
    ) -> bool {
        if let ir::Node::Expression(expr) = node {
            match &expr.kind {
                ExprKind::Str(value) => {
                    return matches!(self, Self::String(expected) if expected.as_ref() == value);
                }
                ExprKind::Bytes(value) => {
                    return matches!(self, Self::Scalar(ir::Constant::Bytes(expected)) if expected == value);
                }
                // The branch writer tests a rich comparison on the Python frame.
                // A fixed bool used as a value still needs its own layout.
                ExprKind::Boolean(..)
                    if !condition.is_some_and(|test| std::ptr::eq(test, *expr)) =>
                {
                    return false;
                }
                ExprKind::Compare(values, ops)
                    if expr.ty != Type::PythonObject
                        && ir::Compare::tuple_membership(values, ops).is_none()
                        && !condition.is_some_and(|test| {
                            std::ptr::eq(test, *expr)
                                || matches!(test.kind, ExprKind::Boolean(..))
                                    && parent.is_some_and(|owner| std::ptr::eq(test, owner))
                        }) =>
                {
                    return false;
                }
                ExprKind::Invoke { .. } => {
                    return if let Self::Invocation(call, ty) = self {
                        *ty == expr.ty && expr.python_call() == Some(call.as_ref())
                    } else {
                        positional(expr).as_ref() == Some(self)
                    };
                }
                _ => {}
            }
        }
        Self::checked(node).as_ref() == Some(self)
    }

    pub(crate) fn checked(node: &ir::Node<'_>) -> Option<Self> {
        Some(match node {
            ir::Node::Statement(Statement::Position(range, None)) => Self::Position(*range),
            ir::Node::Statement(Statement::If(_, yes, no)) => Self::Branch(yes.len(), no.len()),
            ir::Node::Statement(Statement::Try {
                body,
                handlers,
                orelse,
                finalbody,
            }) if handlers.is_empty() && orelse.is_empty() && !finalbody.is_empty() => {
                Self::TryFinally(body.len(), finalbody.len())
            }
            ir::Node::Statement(Statement::Try {
                body,
                handlers,
                orelse,
                finalbody,
            }) if orelse.is_empty()
                && finalbody.is_empty()
                && let [handler] = handlers.as_slice()
                && handler.name.is_none()
                && let ir::ExceptionMatcher::WrittenCategory(kind, range, _) = &handler.matcher =>
            {
                Self::TryExcept(body.len(), *kind, *range, handler.body.len())
            }
            ir::Node::Statement(Statement::ContainerFor(loop_)) => {
                Self::Container(loop_.target, loop_.range)
            }
            ir::Node::Statement(Statement::Return(_)) => Self::Return,
            ir::Node::Statement(Statement::Expr(_)) => Self::Effect,
            ir::Node::Statement(Statement::Raise(None)) => Self::Reraise,
            ir::Node::Statement(Statement::Raise(Some(raised))) => Self::Raise {
                cause: raised.cause.is_some(),
            },
            ir::Node::Statement(Statement::Assign(slot, _)) => Self::Store(*slot),
            ir::Node::Statement(Statement::SetField(_, ir::StoreField::Written(field, _), _)) => {
                Self::StoreField(*field, None)
            }
            ir::Node::Statement(Statement::AugmentField(
                _,
                ir::StoreField::Written(field, _),
                op,
                _,
            )) => Self::StoreField(*field, Some(*op)),
            ir::Node::Expression(expr) => match expr.kind {
                ExprKind::None if expr.ty != Type::None => return None,
                ExprKind::None | ExprKind::Bool(_) | ExprKind::Float(_) | ExprKind::Bytes(_) => {
                    Self::Scalar(ir::Constant::from_expr(expr)?)
                }
                ExprKind::Str(ref value) => Self::String(value.as_str().into()),
                ExprKind::List(ref items) if matches!(expr.ty, Type::List(_) | Type::ListInt) => {
                    Self::indexed(&expr.kind, items.len(), expr.ty)
                }
                ExprKind::Tuple(ref items) if matches!(expr.ty, Type::Tuple(_)) => {
                    Self::indexed(&expr.kind, items.len(), expr.ty)
                }
                ExprKind::Dict(ref entries) if matches!(expr.ty, Type::Dict(_)) => {
                    Self::indexed(&expr.kind, entries.len(), expr.ty)
                }
                ExprKind::Int(ref text) => Self::Integer(text.parse().ok()?),
                ExprKind::Unary(ast::UnaryOp::USub, _) => {
                    Self::Integer(expr.tagged_integer_literal()?)
                }
                ExprKind::Exception { kind, ref message } => Self::Exception {
                    kind,
                    message: message.is_some(),
                },
                ExprKind::Call(id, ref args)
                | ExprKind::Construct {
                    initializer: id,
                    ref args,
                } if !args.exhaustive_enum_match
                    && args.order.iter().copied().eq(0..args.values.len()) =>
                {
                    Self::NamedCall(
                        std::mem::discriminant(&expr.kind),
                        id,
                        args.values.len(),
                        expr.ty,
                    )
                }
                ExprKind::Builtin(
                    Builtin::RegisteredLocation(range, position, hint),
                    ref operands,
                ) if operands.len() == 1 => Self::Located(range, position, hint, expr.ty),
                ExprKind::Builtin(Builtin::RegisteredName(ref name), ref operands)
                    if operands.is_empty() =>
                {
                    Self::Name(name.clone(), expr.ty)
                }
                ExprKind::Builtin(
                    Builtin::RegisteredAttribute {
                        action: ir::AttributeAction::Load,
                        ref name,
                        range,
                        hint,
                    },
                    ref operands,
                ) if operands.len() == 1 => {
                    Self::Attribute(name.id.clone(), range, name.start(), hint, expr.ty)
                }
                ExprKind::Invoke { .. } => {
                    Self::Invocation(Box::new(expr.python_call()?.clone()), expr.ty)
                }
                ExprKind::Local(slot) | ExprKind::Field(_, slot) | ExprKind::Bind(slot, _) => {
                    Self::indexed(&expr.kind, slot, expr.ty)
                }
                ExprKind::Builtin(
                    Builtin::Process(ir::ProcessOperation::Coroutine { function, .. }, _),
                    _,
                ) => Self::NamedCall(
                    std::mem::discriminant(&expr.kind),
                    function,
                    expr.coroutine_arguments()?.len(),
                    expr.ty,
                ),
                ExprKind::Builtin(Builtin::GlobalGet(slot), ref args) if args.is_empty() => {
                    Self::indexed(&expr.kind, slot, expr.ty)
                }
                ExprKind::Unary(op @ (ast::UnaryOp::UAdd | ast::UnaryOp::Not), _) => {
                    Self::Unary(op, expr.ty)
                }
                ExprKind::Binary(op, _, _) => Self::Binary(op, expr.ty),
                ExprKind::Boolean(and, ref values) => Self::Logical(and, values.len()),
                ExprKind::Compare(ref values, ref ops)
                    if let [op] = ops.as_slice()
                        && (ir::Compare::tuple_membership(values, ops).is_some()
                            || ir::Compare::from_operator(*op).is_some()
                                && ir::Compare::integer_pair(values)) =>
                {
                    Self::Comparison(*op)
                }
                ExprKind::Builtin(
                    Builtin::Process(ir::ProcessOperation::Copy(op), _),
                    ref operands,
                ) => Self::Copy(op, operands.len(), expr.ty),
                ExprKind::Builtin(
                    Builtin::Process(op, ir::ProcessSite::Written(ref binding)),
                    _,
                ) => Self::NativeCall(
                    binding.range,
                    std::mem::discriminant(&op),
                    expr.named_call_arguments()?.len(),
                    expr.ty,
                ),
                ExprKind::Builtin(Builtin::Path(op), ref operands) => {
                    Self::Path(op, operands.len(), expr.ty)
                }
                ExprKind::Builtin(Builtin::Text(op), ref operands) => {
                    Self::Text(op, operands.len(), expr.ty)
                }
                ExprKind::Builtin(ref op, ref operands) => {
                    Self::Builtin(std::mem::discriminant(op), operands.len(), expr.ty)
                }
                _ => return None,
            },
            _ => return None,
        })
    }
}

#[derive(Clone, Copy)]
pub(super) struct PendingSite {
    index: usize,
    range: TextRange,
}

#[derive(Default)]
pub(super) struct SourceSites {
    pub(super) attributes: HashMap<TextRange, FixedAttribute>,
    pub(super) wrapper_fields: bool,
    sites: Vec<(TextRange, Option<FixedNode>)>,
    current: Option<PendingSite>,
    next_index: usize,
    private_slots: usize,
}

impl SourceSites {
    pub(super) fn new(has_wrapper_link: bool) -> Self {
        Self {
            wrapper_fields: has_wrapper_link,
            ..Self::default()
        }
    }

    /// Keep the source range on the lowering stack until an operation commits a
    /// site. A body that declines before selecting any operation needs no tables.
    pub(super) fn enter(&mut self, range: TextRange) -> Option<PendingSite> {
        let index = self.next_index;
        self.next_index += 1;
        self.current.replace(PendingSite { index, range })
    }

    pub(super) fn leaf(&mut self, range: TextRange, value: &Expr) {
        let parent = self.enter(range);
        self.checked_expression(range, value);
        self.leave(parent);
    }

    pub(super) fn leave(&mut self, parent: Option<PendingSite>) -> bool {
        let current = std::mem::replace(&mut self.current, parent);
        current.is_some_and(|site| matches!(self.sites.get(site.index), Some((_, Some(_)))))
    }

    fn commit(&mut self, site: PendingSite, node: FixedNode) {
        if self.sites.len() <= site.index {
            self.sites.resize_with(site.index + 1, Default::default);
        }
        self.sites[site.index] = (site.range, Some(node));
    }

    pub(super) fn inside_site(&self, range: TextRange) -> bool {
        self.current
            .is_some_and(|site| site.range != range && site.range.contains_range(range))
    }

    /// Take a literal from the expression just checked, before the fixed call adds its defaults.
    pub(super) fn written_literal(&self, source: &ast::Expr, fixed: &mut Expr) -> Option<Expr> {
        let (range, Some(key)) = self.sites.last()? else {
            return None;
        };
        if source.range() != *range || !source.is_literal_expr() {
            return None;
        }
        let scalar = match &mut fixed.kind {
            ExprKind::Present(inner) => inner.as_mut(),
            _ => fixed,
        };
        let ty = scalar.kind.scalar_literal_type()?;
        if !key.matches(&ir::Node::Expression(scalar)) {
            return None;
        }
        let mut original = std::mem::replace(scalar, scalar.clone());
        original.ty = ty;
        Some(registered::values::located(
            *range,
            source.start(),
            original,
        ))
    }

    pub(super) fn site_at(&self, range: TextRange) -> Option<PendingSite> {
        self.current.filter(|site| site.range == range)
    }

    pub(super) fn record(&mut self, range: TextRange, value: ir::Node<'_>) {
        if let Some(site) = self.site_at(range)
            && let Some(node) = FixedNode::checked(&value)
        {
            self.commit(site, node);
        }
    }

    pub(super) fn checked_expression(&mut self, range: TextRange, value: &Expr) {
        self.record(range, ir::Node::Expression(value));
    }

    pub(super) fn module_read(&mut self, name: &ast::ExprName, value: &Expr) {
        if let Some(site) = self.site_at(name.range())
            && let ExprKind::Builtin(Builtin::GlobalGet(slot), ref args) = value.kind
            && args.is_empty()
        {
            self.attributes
                .insert(site.range, FixedAttribute::ModuleName(name.id.clone()));
            self.commit(site, FixedNode::indexed(&value.kind, slot, value.ty));
        }
    }

    /// A fused operation can supply the written arguments only when its source sites
    /// are exactly those operands. Nested or synthesized input uses the ordinary binder.
    pub(super) fn has_written_operands(&self, call: &ast::ExprCall) -> bool {
        self.site_at(call.range())
            .and_then(|site| self.sites.get(site.index + 1..))
            .is_some_and(|sites| {
                sites.iter().map(|(range, _)| *range).eq(call
                    .arguments
                    .args
                    .iter()
                    .map(Ranged::range))
            })
    }

    /// Confirm that binding the declared types left each supplied operand at its original site.
    pub(super) fn has_written_arguments(&self, call: &ast::ExprCall, values: &[Expr]) -> bool {
        let Some(site) = self.site_at(call.range()) else {
            return false;
        };
        if call.arguments.is_empty() {
            // The pending call site may not have been allocated yet.
            return self.sites.len() <= site.index + 1;
        }
        let Some(sites) = self.sites.get(site.index + 1..) else {
            return false;
        };
        let flat = sites.len() == call.arguments.len();
        let mut sites = sites.iter();
        values.len() >= call.arguments.len()
            && imre_ast::argument_values(&call.arguments)
                .zip(values)
                .all(|(arg, value)| {
                    if !sites
                        .as_slice()
                        .first()
                        .is_some_and(|(range, _)| *range == arg.range())
                    {
                        return false;
                    }
                    let mut matches = |node: ir::Node<'_>| {
                        sites.next().is_some_and(|(range, key)| {
                            arg.range().contains_range(*range)
                                && key.as_ref().is_some_and(|key| key.matches(&node))
                        })
                    };
                    if flat {
                        return matches(ir::Node::Expression(value));
                    }
                    ir::all_operand_nodes(value, None, &mut |node, parent| {
                        !node.has_fixed_site_in(parent) || matches(node)
                    })
                })
            && sites.len() == 0
    }

    /// Visit composed operation sites, retaining each operand's own source range where available.
    fn operand_sites(
        value: &Expr,
        fallback: TextRange,
        mut record: impl FnMut(&ir::Node<'_>, Option<TextRange>, TextRange),
    ) {
        ir::all_operand_nodes(value, None, &mut |node, parent| {
            if node.has_fixed_site() {
                let own = match node {
                    ir::Node::Expression(expr) => expr.written_range(),
                    ir::Node::Statement(_) => None,
                };
                let range = own
                    .or_else(|| parent.and_then(Expr::written_range))
                    .unwrap_or(fallback);
                record(&node, own, range);
            }
            true
        });
    }

    /// First source site after the operands already checked.
    pub(super) fn next_operand(&self) -> usize {
        self.sites.len()
    }

    /// The receiver already owns its complete sites. Only the key gained Python operations.
    pub(super) fn subscript(&mut self, index: &ast::ExprSubscript, begin: usize, value: &Expr) {
        let Some(site) = self.site_at(index.range()) else {
            return;
        };
        if begin <= site.index || begin > self.sites.len() {
            return;
        }
        let read = value.without_location();
        let ExprKind::Builtin(Builtin::RegisteredSubscript(..), operands) = &read.kind else {
            return;
        };
        let mut next = begin;
        let mut added = Vec::new();
        Self::operand_sites(&operands[1], index.slice.range(), |node, own, range| {
            if let Some((written, Some(expected))) = self.sites.get(next)
                && own.is_none_or(|owned| *written == owned)
                && expected.matches(node)
            {
                next += 1;
            } else {
                added.push((next, (range, FixedNode::checked(node))));
            }
        });
        if next != self.sites.len() {
            return;
        }
        // Insert in reverse so the indices of the original sites remain valid.
        for (at, operand) in added.into_iter().rev() {
            self.sites.insert(at, operand);
        }
        self.sites.insert(
            site.index + 1,
            (
                index.range(),
                FixedNode::checked(&ir::Node::Expression(read)),
            ),
        );
        self.checked_expression(index.range(), value);
        self.next_index = self.sites.len();
    }

    /// Insert the ordinary callee before the already-checked arguments at this source position.
    pub(super) fn invoked(&mut self, call: &ast::ExprCall, value: &Expr) {
        let site = self.site_at(call.range()).unwrap();
        let ExprKind::Invoke { callable, .. } = &value.kind else {
            unreachable!()
        };
        self.checked_expression(call.range(), value);
        let end = self.sites.len();
        Self::operand_sites(callable, call.func.range(), |node, _, range| {
            self.sites.push((range, FixedNode::checked(node)));
        });
        // The callable is evaluated before the written arguments that were checked first.
        let count = self.sites.len() - end;
        self.sites[site.index + 1..].rotate_right(count);
        self.next_index += count;
    }

    /// Place the Python callee beside the native operation. Both entries use its written operands.
    pub(super) fn positional_call(
        &mut self,
        call: &ast::ExprCall,
        value: &Expr,
        target: impl FnOnce() -> Option<FixedAttribute>,
    ) {
        if let Some(args) = value.named_call_arguments().or_else(|| {
            value
                .coroutine_arguments()
                .filter(|fields| self.has_written_arguments(call, fields))
        }) && args.len() == call.arguments.args.len()
            && registered::values::fixed_positional_arguments(call)
        {
            self.written_call(call, value, target);
        }
    }

    /// Retain the callable at its original occurrence before evaluating its checked arguments.
    pub(super) fn written_call(
        &mut self,
        call: &ast::ExprCall,
        value: &Expr,
        target: impl FnOnce() -> Option<FixedAttribute>,
    ) {
        if let Some(site) = self.site_at(call.range())
            && let Some(node) = FixedNode::checked(&ir::Node::Expression(value))
            && let Some(target) = target()
        {
            self.attributes.insert(site.range, target);
            self.commit(site, node);
        }
    }

    /// A checked positional invocation already owns the callable and every written argument.
    pub(super) fn positional_invoke(&mut self, call: &ast::ExprCall, value: &Expr, local: bool) {
        let Some(site) = self.site_at(call.range()) else {
            return;
        };
        let checked_callee = self
            .sites
            .get(site.index + 1)
            .is_some_and(|(range, _)| *range == call.func.range());
        if !local
            && !(checked_callee
                && self
                    .attributes
                    .get(&call.func.range())
                    .is_some_and(FixedAttribute::has_call_binding))
        {
            return;
        }
        let Some(node) = FixedNode::positional_invoke(value) else {
            return;
        };
        self.commit(site, node);
        if !checked_callee {
            self.invoked(call, value);
        }
        let invoke = ir::RegisteredCall::new(
            call.range(),
            registered::values::callable_target(call),
            ir::RegisteredArguments::Positional,
        );
        self.attributes
            .insert(site.range, FixedAttribute::Call(None, invoke));
    }

    pub(super) fn field(&mut self, field: &ast::ExprAttribute, value: &Expr) {
        if let Some(site) = self.site_at(field.range())
            && !ruff_python_stdlib::identifiers::is_mangled_private(field.attr.as_str())
            && let Some(node) = FixedNode::checked(&ir::Node::Expression(value))
        {
            self.attributes
                .insert(site.range, FixedAttribute::Load(field.attr.clone()));
            self.commit(site, node);
        }
    }

    /// Bind a method with its already-checked written arguments while the fixed
    /// entry keeps its specialized call.
    pub(super) fn method(
        &mut self,
        call: &ast::ExprCall,
        field: &ast::ExprAttribute,
        value: &Expr,
    ) -> bool {
        let Some(operands) = value.named_call_arguments() else {
            return false;
        };
        if operands.len() != 1 + call.arguments.len() {
            return false;
        }
        let Some(node) = FixedNode::checked(&ir::Node::Expression(value)) else {
            return false;
        };
        let range = call.range();
        if let Some(site) = self.site_at(range) {
            let mut invoke = ir::RegisteredCall::new(
                range,
                registered::values::method_target(field),
                match &value.kind {
                    ExprKind::Builtin(Builtin::StringFormat(layout), _) => layout.clone(),
                    _ => ir::RegisteredArguments::Positional.bind_arguments(&call.arguments, false),
                },
            );
            invoke.checked_nullable_receiver = matches!(operands[0].ty, Type::Optional(_))
                && matches!(
                    value.kind,
                    ExprKind::Builtin(Builtin::RegexCapture { .. }, _)
                );
            self.attributes
                .insert(range, FixedAttribute::Call(None, invoke));
            self.commit(site, node);
        } else if self.inside_site(range)
            && let Some(index) = self.first_operand(field.value.range(), &operands[0])
            && let Some(FixedAttribute::CallValue { checked, placed }) =
                self.attributes.get_mut(&range)
            && matches!(operands[0].kind, ExprKind::Local(slot) if slot == checked.slot && operands[0].ty == checked.ty)
        {
            // The binder saved this parameter method at the same physical call, before adding defaults.
            *placed = true;
            self.sites.insert(index, (range, Some(node)));
            self.next_index += 1;
        } else {
            return false;
        }
        true
    }

    pub(super) fn first_operand(&self, range: TextRange, value: &Expr) -> Option<usize> {
        let index = self.current?.index + 1;
        let (written, original) = self.sites.get(index)?;
        (*written == range && original.as_ref()?.matches(&ir::Node::Expression(value)))
            .then_some(index)
    }

    /// Keep an implicit unary operation at the site that owns it, around the written operand.
    pub(super) fn wrapped_operand(&mut self, source: &ast::Expr, range: TextRange, test: &Expr) {
        if let ExprKind::Builtin(Builtin::Bool | Builtin::FormatConversion(_), args) = &test.kind
            && let [value] = args.as_slice()
            && let Some(index) = self.first_operand(source.range(), value)
        {
            let node = FixedNode::checked(&ir::Node::Expression(test));
            self.sites.insert(index, (range, node));
            self.next_index += 1;
        }
    }

    pub(super) fn statement(&mut self, operand: TextRange, statement: &Statement) {
        if let Some(site) = self.statement_site(operand)
            && let Some(node) = FixedNode::checked(&ir::Node::Statement(statement))
        {
            self.commit(site, node);
        }
    }

    pub(super) fn container_loop(&mut self, input: ir::ContainerFor) -> Statement {
        let range = input.range;
        self.private_slots += input.slots.len();
        let statement = Statement::ContainerFor(input);
        self.statement(range, &statement);
        statement
    }

    /// The Python frame stores through the actual receiver at the original attribute range.
    pub(super) fn store_field(
        &mut self,
        field: &ast::ExprAttribute,
        first_operand: TextRange,
        destination: &mut ir::StoreField,
        op: Option<Binary>,
    ) {
        if self.wrapper_fields
            && !ruff_python_stdlib::identifiers::is_mangled_private(field.attr.as_str())
            && let Some(site) = self.statement_site(first_operand)
            && let ir::StoreField::Native(slot) = *destination
        {
            let key = ir::FieldUse {
                slot,
                target: field.range(),
            };
            *destination = ir::StoreField::Written(key, field.attr.clone());
            self.commit(site, FixedNode::StoreField(key, op));
        }
    }

    fn statement_site(&self, operand: TextRange) -> Option<PendingSite> {
        let site = self.current?;
        let (range, _) = self.sites.get(site.index + 1)?;
        (*range == operand).then_some(site)
    }

    pub(super) fn finish(
        self,
        source_statements: usize,
        body: &[Statement],
        names: HashMap<String, usize>,
        locals: &[Type],
        method: Option<&crate::classes::MethodSource>,
        arguments: FrameArguments,
    ) -> Option<FixedBodySource> {
        let named_slots = locals.len().checked_sub(self.private_slots)?;
        if body.is_empty()
            || self.current.is_some()
            || source_statements != body.len()
            || names.len() != named_slots
        {
            return None;
        }
        let local_names: HashMap<_, _> = names
            .into_iter()
            .map(|(name, slot)| {
                // The Python binder spells private method locals with their class prefix.
                if method.is_some() && ruff_python_stdlib::identifiers::is_mangled_private(&name) {
                    return None;
                }
                Some((
                    slot,
                    registered::LocalBinding {
                        name,
                        namespace: None,
                    },
                ))
            })
            .collect::<Option<_>>()?;
        if local_names.len() != named_slots {
            return None;
        }
        Some(FixedBodySource {
            method: method.cloned(),
            arguments,
            attributes: self.attributes,
            sites: self.sites,
            local_names,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refused_key_keeps_its_written_sites_and_the_next_operand_index() {
        let text = "items[key], later()";
        let parsed = ruff_python_parser::parse_expression(text).unwrap();
        let pair = parsed.expr().as_tuple_expr().unwrap();
        let index = pair.elts[0].as_subscript_expr().unwrap();
        let later = pair.elts[1].range();
        let value =
            registered::values::subscript(index, Expr::none(), unpacking::string("changed"), text);
        for missing in [false, true] {
            let mut sites = SourceSites::default();
            sites.enter(index.range());
            sites.leaf(index.value.range(), &Expr::none());
            let key = sites.next_operand();
            sites.leaf(index.slice.range(), &unpacking::string("written"));
            if missing {
                sites.sites[key].1 = None;
            }
            let before = sites.sites.clone();
            let next = sites.next_index;
            sites.subscript(index, key, &value);
            assert!(sites.sites == before);
            assert_eq!(sites.next_index, next);
            sites.leaf(later, &Expr::none());
            assert_eq!(sites.sites[next].0, later);
        }
    }

    #[test]
    fn rewriting_the_same_checked_if_declines_its_original_sites() {
        let text = concat!(
            "class Receiver:\n    def read(self, value: int) -> int: return value\n",
            "def pick(flag: bool) -> str:\n",
            "    if flag:\n        return 'café'\n    else:\n        return ''\n",
            "print(pick(True))\n",
        );
        let mut graph = SourceGraph::single(text).unwrap();
        let crate::CheckedProgram::Native(fixed) = crate::checked_lower(&mut graph).unwrap() else {
            panic!("expected a checked function");
        };
        let (slot, _, source) = fixed
            .frame_sources
            .into_iter()
            .find(|(id, ..)| *id == 0)
            .unwrap();
        let mut function = fixed.ir.0.functions.into_iter().nth(slot).unwrap();
        let body = function.body.as_ptr();
        let placed = source.clone().place(function).ok().unwrap();
        assert_eq!(placed.function.body.as_ptr(), body);
        assert_eq!(&text[placed.fixed_sites[3]], "'café'");
        function = placed.function;
        let [Statement::If(_, yes, _)] = function.body.as_mut_slice() else {
            panic!()
        };
        let [Statement::Return(value)] = yes.as_mut_slice() else {
            panic!()
        };
        *value = unpacking::string("cafè");
        assert!(source.clone().place(function.clone()).is_err());
        let [Statement::If(_, yes, no)] = function.body.as_mut_slice() else {
            panic!()
        };
        let [Statement::Return(value)] = yes.as_mut_slice() else {
            panic!()
        };
        *value = unpacking::string("café");
        no.insert(0, yes.remove(0));
        assert_eq!(function.body.as_ptr(), body);
        assert!(source.place(function).is_err());
    }

    #[test]
    fn a_signed_float_keeps_its_value_and_the_whole_expression_range() {
        let text = concat!(
            "class Receiver:\n    def read(self, value: int) -> int: return value\n",
            "def signed() -> float: return -0.0\nprint(signed())\n",
        );
        let mut graph = SourceGraph::single(text).unwrap();
        let crate::CheckedProgram::Native(fixed) = crate::checked_lower(&mut graph).unwrap() else {
            panic!("expected a fixed float return");
        };
        let (_, _, source) = fixed
            .frame_sources
            .into_iter()
            .find(|(id, ..)| *id == 0)
            .unwrap();
        let function = fixed.ir.0.functions.into_iter().next().unwrap();
        let mut placed = source.clone().place(function).ok().unwrap();
        let [Statement::Return(value)] = placed.function.body.as_slice() else {
            panic!("expected a float return");
        };
        assert!(
            matches!(value.without_location().kind, ExprKind::Float(value) if value.to_bits() == (-0.0_f64).to_bits())
        );
        assert_eq!(&text[*placed.fixed_sites.last().unwrap()], "-0.0");
        // A native caller may subsequently wrap this value for an optional destination.
        let [Statement::Return(operand)] = placed.function.body.as_mut_slice() else {
            unreachable!();
        };
        let payload = std::mem::replace(operand, Expr::none());
        operand.ty = fixed.ir.0.types.optional(payload.ty);
        operand.kind = ExprKind::Present(Box::new(payload));
        placed.function.returns = operand.ty;
        let mut optional = source.clone().place(placed.function).ok().unwrap();
        assert_eq!(optional.fixed_sites, placed.fixed_sites);
        let [
            Statement::Return(Expr {
                kind: ExprKind::Present(value),
                ..
            }),
        ] = optional.function.body.as_mut_slice()
        else {
            unreachable!();
        };
        value.kind = ExprKind::Float(0.0);
        assert!(source.place(optional.function).is_err());
    }

    #[test]
    fn list_method_keeps_its_receiver_and_declines_a_rewritten_operation() {
        let text = concat!(
            "class Receiver:\n    def read(self, value: int) -> int: return value\n",
            "events = [37]\ndef reset(): return events.clear()\nreset()\n",
        );
        let mut graph = SourceGraph::single(text).unwrap();
        let crate::CheckedProgram::Native(fixed) = crate::checked_lower(&mut graph).unwrap() else {
            panic!("expected a fixed list operation");
        };
        let (id, _, source) = fixed
            .frame_sources
            .into_iter()
            .find(|(_, range, _)| text[*range].starts_with("def reset"))
            .unwrap();
        let function = fixed.ir.0.functions.into_iter().nth(id).unwrap();
        let body = function.body.as_ptr();
        let mut placed = source.clone().place(function).ok().unwrap();
        assert_eq!(placed.function.body.as_ptr(), body);
        assert_eq!(&text[*placed.fixed_sites.last().unwrap()], "events");
        let [
            Statement::Return(Expr {
                kind: ExprKind::Builtin(operation, _),
                ..
            }),
        ] = placed.function.body.as_mut_slice()
        else {
            panic!("expected a list method return");
        };
        assert!(matches!(operation, Builtin::ListClear));
        *operation = Builtin::DictClear;
        assert!(source.place(placed.function).is_err());
    }

    #[test]
    fn a_named_constructor_site_requires_the_original_initializer() {
        let args = ir::CallArgs::positional(Vec::new());
        let checked = |kind| {
            FixedNode::checked(&ir::Node::Expression(&Expr {
                ty: Type::Class(0),
                kind,
            }))
        };
        let constructor = |initializer| ExprKind::Construct {
            initializer,
            args: args.clone(),
        };
        let original = checked(constructor(0)).unwrap();
        assert!(checked(constructor(1)).as_ref() != Some(&original));
        assert!(checked(ExprKind::Call(0, args)).as_ref() != Some(&original));
    }

    #[test]
    fn a_text_site_keeps_its_selected_operation() {
        use ir::{TextOperation, TextPredicate};
        for (original, changed, ty) in [
            (
                TextOperation::Predicate(TextPredicate::Upper),
                TextOperation::Predicate(TextPredicate::Lower),
                Type::Bool,
            ),
            (
                TextOperation::Split(false),
                TextOperation::Split(true),
                Type::List(0),
            ),
        ] {
            let text = |operation| Expr {
                kind: ExprKind::Builtin(Builtin::Text(operation), {
                    let mut operands = vec![Expr {
                        ty: Type::Str,
                        kind: ExprKind::Local(0),
                    }];
                    if matches!(operation, TextOperation::Split(_)) {
                        operands.extend([Expr::absent(Type::Optional(0)), Expr::integer(-1)]);
                    }
                    operands
                }),
                ty,
            };
            let original = text(original);
            let changed = text(changed);
            let original = FixedNode::checked(&ir::Node::Expression(&original)).unwrap();
            let changed = FixedNode::checked(&ir::Node::Expression(&changed)).unwrap();
            assert!(original != changed);
        }
    }

    #[test]
    fn a_bytes_operand_keeps_its_written_range_and_value() {
        let text = concat!(
            "class Receiver:\n    def read(self, value: int) -> int: return value\n",
            "def data() -> bytes: return b'\\0\\x80'\nprint(data())\n",
        );
        let mut graph = SourceGraph::single(text).unwrap();
        let crate::CheckedProgram::Native(fixed) = crate::checked_lower(&mut graph).unwrap() else {
            panic!("expected a fixed bytes return");
        };
        let (slot, _, source) = fixed.frame_sources.into_iter().next().unwrap();
        let function = fixed.ir.0.functions.into_iter().nth(slot).unwrap();
        let mut placed = source.clone().place(function).ok().unwrap();
        assert_eq!(&text[*placed.fixed_sites.last().unwrap()], r"b'\0\x80'");
        let [
            Statement::Return(Expr {
                kind: ExprKind::Bytes(value),
                ..
            }),
        ] = placed.function.body.as_mut_slice()
        else {
            panic!("expected the original bytes");
        };
        assert_eq!(value.as_slice(), [0, 0x80]);
        value[1] = 0x81;
        assert!(source.place(placed.function).is_err());
    }

    #[test]
    fn display_elements_cannot_change_owners() {
        let list = |items| Expr {
            ty: Type::ListInt,
            kind: ExprKind::List(items),
        };
        // Both trees have the same types and operation kinds in preorder.
        let empty = || list(Vec::new());
        let function = |value| ir::Function {
            params: vec![],
            locals: vec![],
            returns: Type::ListInt,
            body: vec![Statement::Return(value)],
        };
        let original = function(list(vec![empty(), list(vec![empty()])]));
        let changed = function(list(vec![list(vec![empty()]), empty()]));
        let mut sites = Vec::new();
        ir::walk_nodes(&original.body, &mut |node, _| {
            sites.push((TextRange::default(), FixedNode::checked(&node)));
        });
        let source = FixedBodySource {
            method: None,
            arguments: FrameArguments::Checked,
            attributes: Default::default(),
            sites,
            local_names: Default::default(),
        };
        assert!(source.clone().place(original).is_ok());
        assert!(source.place(changed).is_err());
    }

    #[test]
    fn abandoning_unselected_sites_leaves_the_tables_unallocated() {
        let mut sites = SourceSites::default();
        let outer = sites.enter(TextRange::new(0.into(), 8.into()));
        let inner = sites.enter(TextRange::new(2.into(), 5.into()));
        assert!(!sites.leave(inner));
        assert!(!sites.leave(outer));
        assert_eq!(sites.sites.capacity(), 0);
    }

    #[test]
    fn class_free_program_keeps_exact_operations_without_frame_sites() {
        let mut graph = SourceGraph::single(concat!(
            "def repeat(text: str) -> str:\n",
            "    return text + text\n",
            "print(repeat('x'))\n",
        ))
        .unwrap();
        let crate::CheckedProgram::Native(fixed) = crate::checked_lower(&mut graph).unwrap() else {
            panic!("expected fixed checked operations")
        };
        assert_eq!(fixed.frame_sources.capacity(), 0);
        assert!(matches!(
            fixed.ir.0.functions[0].body.as_slice(),
            [Statement::Return(Expr {
                ty: Type::Str,
                kind: ExprKind::Binary(Binary::Add, ..),
            })]
        ));
    }

    #[test]
    fn field_reads_keep_the_bound_slot_and_the_attribute_position() {
        let text = concat!(
            "from functools import update_wrapper\n",
            "from typing import Callable\n",
            "class Entry:\n",
            "    text: str\n",
            "    other: str\n",
            "    def __init__(self, text: str) -> None:\n",
            "        self.text = text\n",
            "        self.other = text\n",
            "    def both(self, suffix: str) -> str:\n",
            "        self.other = suffix\n",
            "        self.other = suffix\n",
            "        return (self.\n                text) + self.text\n",
            "class Other(Entry): pass\n",
            "def native(entry: Entry) -> str: return entry.text\n",
            "def sink(text: str) -> str: return text\n",
            "def main() -> None:\n",
            "    method: Callable[[str], str] = Entry('x').both\n",
            "    print(update_wrapper(sink, method, (), ())('x'), native(Entry('y')))\n",
            "main()\n",
        );
        let project = tempfile::tempdir().unwrap();
        let entry = project.path().join("app.py");
        std::fs::write(&entry, text).unwrap();
        let mut graph = SourceGraph::load(&entry, Some(project.path()), &[]).unwrap();
        let crate::CheckedProgram::Native(fixed) = crate::checked_lower(&mut graph).unwrap() else {
            panic!("expected a fixed field read")
        };
        let mut module = fixed.ir.0;
        let sources = fixed.frame_sources;
        // The free function read was checked before any wrapper link was seen.
        let native = text.find("def native").unwrap();
        assert!(
            !sources
                .iter()
                .any(
                    |(id, definition, source)| usize::from(definition.start()) == native
                        && source.clone().place(module.functions[*id].clone()).is_ok()
                )
        );
        let both = text.find("def both").unwrap();
        let &(id, _, ref source) = sources
            .iter()
            .find(|(_, definition, _)| usize::from(definition.start()) == both)
            .unwrap();
        assert!(source.clone().place(module.functions[id].clone()).is_ok());
        let function = &module.functions[id];
        assert!(source.clone().place(function.clone()).is_ok());
        let mut changed = function.clone();
        changed.body.swap(0, 1);
        assert!(source.clone().place(changed).is_err());
        let mut changed = function.clone();
        let Statement::SetField(_, ir::StoreField::Written(key, _), _) = &mut changed.body[0]
        else {
            panic!("expected checked field store");
        };
        key.slot += 1;
        assert!(source.clone().place(changed).is_err());
        let mut changed = function.clone();
        let Statement::SetField(_, _, rhs) = &mut changed.body[0] else {
            panic!("expected checked field store");
        };
        rhs.kind = ExprKind::Str("replaced".into());
        assert!(source.clone().place(changed).is_err());

        let stores = source.sites.iter().filter_map(|(range, node)| match node {
            Some(FixedNode::StoreField(field, None)) => Some((&text[*range], &text[field.target])),
            _ => None,
        });
        assert!(stores.eq([("self.other = suffix", "self.other"); 2]));
        assert_eq!(source.attributes.len(), 2);
        for (range, field) in &source.attributes {
            let FixedAttribute::Load(name) = field else {
                panic!("expected attribute load")
            };
            assert_eq!(name.as_str(), "text");
            assert_eq!(&text[name.range()], "text");
            assert_eq!(name.end(), range.end());
            assert!(text[*range].starts_with("self."));
        }
        let class = |name| {
            Type::Class(
                module
                    .class_metadata
                    .iter()
                    .position(|c| c.name == name)
                    .unwrap(),
            )
        };
        // The next node in the transcript checks the receiver's type.
        for (slot, owner) in [(1, class("Entry")), (0, class("Other"))] {
            let Statement::Return(Expr {
                kind: ExprKind::Binary(_, first, _),
                ..
            }) = module.functions[id].body.last_mut().unwrap()
            else {
                panic!("expected the two checked field operands")
            };
            let ExprKind::Field(receiver, field) = &mut first.kind else {
                panic!("expected a fixed class field")
            };
            *field = slot;
            receiver.ty = owner;
            assert!(source.clone().place(module.functions[id].clone()).is_err());
        }
    }

    #[test]
    fn wrapper_links_retain_only_the_owning_instance_method_sites() {
        let text = concat!(
            "from functools import update_wrapper\n",
            "from typing import Callable\n",
            "class First:\n",
            "    def send(self, text: str) -> str: return text + text\n",
            "class Child(First): pass\n",
            "class Second:\n",
            "    def send(self, text: str) -> str: return text\n",
            "    def private(self, __value: str) -> str: return __value\n",
            "def sink(text: str) -> str: return text\n",
            "def main() -> None:\n",
            "    method: Callable[[str], str] = First().send\n",
            "    print(update_wrapper(sink, method, (), ())('x'))\n",
            "    print(Child().send('a'), Second().send('b'), Second().private('c'))\n",
            "main()\n",
        );
        let project = tempfile::tempdir().unwrap();
        let entry = project.path().join("app.py");
        std::fs::write(&entry, text).unwrap();
        let declarations: Vec<_> = text
            .match_indices("def send(")
            .map(|(at, _)| TextSize::try_from(at).unwrap())
            .collect();
        for early in [true, false] {
            let mut graph = SourceGraph::load(&entry, Some(project.path()), &[]).unwrap();
            let crate::CheckedProgram::Native(fixed) = crate::checked_lower(&mut graph).unwrap()
            else {
                panic!("expected a fixed wrapper graph")
            };
            let methods: Vec<_> = fixed
                .frame_sources
                .iter()
                .filter_map(|(slot, range, source)| {
                    let method = source.method.as_ref()?;
                    assert_eq!(method.range(), *range);
                    Some((
                        method.start(),
                        std::ptr::from_ref(&**method),
                        fixed.ir.0.functions[*slot].body.as_ptr(),
                        source
                            .sites
                            .iter()
                            .map(|(range, _)| *range)
                            .collect::<Vec<_>>(),
                    ))
                })
                .collect();
            assert!(methods.iter().map(|m| m.0).eq(declarations.iter().copied()));
            let refused = methods[0].3[0];
            graph.registered_source_errors.borrow_mut().insert(
                graph.entry,
                vec![crate::analysis::RegisteredSourceError::Local {
                    range: refused,
                    attribute: false,
                }],
            );
            if early {
                let (module, owners) = fixed.ir;
                let checks = registered::SourceChecks::default();
                FixedFunctions {
                    functions: module.functions,
                    frames: fixed.frame_sources,
                }
                .register(&graph, &owners, |module, range| {
                    checks.accepts(&graph, module, range)
                });
            } else {
                fixed.register_definitions(&graph);
            }
            let registered = graph.registered_functions();
            for (start, declaration, operations, positions) in methods {
                if early && positions.contains(&refused) {
                    assert!(!registered[&graph.entry].contains_key(&start));
                    continue;
                }
                let placed = &registered[&graph.entry][&start];
                assert_eq!(placed.function.body.as_ptr(), operations);
                assert_eq!(&*placed.fixed_sites, positions.as_slice());
                assert!(matches!(&placed.inputs, FrameInputs::Protocol { sites }
                    if matches!(sites.as_ref(), [FrameSite::Method(method)] if std::ptr::from_ref(&**method) == declaration)));
            }
        }
    }

    #[test]
    fn original_source_sites_cover_rebinding_and_unary_positive() {
        let text = concat!(
            "class Receiver:\n    def read(self, value: int) -> int:\n        return value\n",
            "def combine(left: int, right: int) -> int:\n",
            "    total = left + right + left\n",
            "    total + left\n",
            "    total = right\n",
            "    return total\n",
            "def positive(value: int) -> int:\n",
            "    return +value\n",
            "print(positive(combine(2, 3)))\n",
        );
        let mut graph = SourceGraph::single(text).unwrap();
        let crate::CheckedProgram::Native(program) = crate::checked_lower(&mut graph).unwrap()
        else {
            panic!("expected native program")
        };
        let sources = &program.frame_sources;
        assert_eq!(sources.len(), 2);
        let (0, _, source) = &sources[0] else {
            panic!("expected the first function")
        };
        let spellings: Vec<_> = source
            .sites
            .iter()
            .map(|(range, _)| &text[*range])
            .collect();
        assert_eq!(
            spellings,
            [
                "total = left + right + left",
                "left + right + left",
                "left + right",
                "left",
                "right",
                "left",
                "total + left",
                "total + left",
                "total",
                "left",
                "total = right",
                "right",
                "return total",
                "total"
            ]
        );
        assert_eq!(source.local_names[&2].name, "total");
        assert!(matches!(source.sites[0].1, Some(FixedNode::Store(2))));
        assert!(matches!(source.sites[10].1, Some(FixedNode::Store(2))));
        assert!(matches!(
            source.sites[13].1,
            Some(FixedNode::Indexed(kind, 2, Type::Int)) if kind == std::mem::discriminant(&ExprKind::Local(2))
        ));
        let (1, _, positive) = &sources[1] else {
            panic!("expected the unary function")
        };
        assert!(positive.sites.iter().map(|(range, _)| &text[*range]).eq([
            "return +value",
            "+value",
            "value"
        ]));
        assert!(matches!(
            positive.sites[1].1,
            Some(FixedNode::Unary(ast::UnaryOp::UAdd, Type::Int))
        ));
    }
}
