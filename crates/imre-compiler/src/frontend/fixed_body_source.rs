//! Physical sites for operations that can also consume Python frame arguments.

use super::*;

#[derive(Clone)]
pub(crate) struct FixedBodySource {
    pub(crate) method: Option<Arc<ast::StmtFunctionDef>>,
    arguments: FrameArguments,
    pub(crate) attributes: HashMap<TextRange, FixedAttribute>,
    sites: Vec<(TextRange, Option<FixedNode>)>,
    pub(crate) local_names: HashMap<usize, registered::LocalBinding>,
}

pub(super) type PlacedBody = StdResult<RegisteredFunction, HashMap<TextRange, FixedAttribute>>;

impl FixedBodySource {
    /// Place the owned operations only if every physical site still belongs to them.
    /// The ordinary Python binder can still use the resolved attributes if placement fails.
    pub(crate) fn place(self, function: ir::Function) -> PlacedBody {
        if self.local_names.len() != function.locals.len() {
            return Err(self.attributes);
        }
        let mut remaining = Some(self.sites.as_slice());
        ir::walk_nodes(&function.body, &mut |node, _| {
            if !node.has_fixed_site() {
                return;
            }
            remaining = remaining.and_then(|sites| {
                let ((_, expected), rest) = sites.split_first()?;
                expected.as_ref()?.matches(&node).then_some(rest)
            });
        });
        if !matches!(remaining, Some([])) {
            return Err(self.attributes);
        }
        let mut frame = RegisteredFunction {
            fixed_sites: self.sites.into_iter().map(|(range, _)| range).collect(),
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
        if matches!(frame.inputs, FrameInputs::Checked { .. })
            && self.arguments == FrameArguments::Python
            && frame
                .function
                .params
                .iter()
                .any(|&slot| frame.function.locals[slot].builtin_scalar_name().is_some())
        {
            let Some((inputs, slots)) =
                crate::llvm::registered::frame_inputs(&frame, self.arguments)
            else {
                return Err(frame
                    .fixed_attributes
                    .map_or_else(HashMap::new, |attrs| *attrs));
            };
            frame.inputs = inputs;
            frame.frame_slots = slots;
        }
        Ok(frame)
    }
}

#[derive(Clone)]
pub(crate) enum FixedAttribute {
    /// A placed module read retains its native slot in the IR and its Python name here.
    ModuleName(ast::name::Name),
    Operation {
        operation: Box<Builtin>,
        position: TextSize,
    },
    /// The fixed builtin already carries its method receiver. A standalone callee is saved here.
    Call(Option<Box<Expr>>, Box<ir::RegisteredCall>),
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) enum FixedNode {
    Position(TextRange),
    Return,
    Effect,
    Raise {
        cause: bool,
    },
    Exception {
        kind: u32,
        message: bool,
    },
    Store(usize),
    StoreField(usize),
    Branch(usize, usize),
    /// The operation keeps its checked slot or operand count, and its native type.
    Indexed(std::mem::Discriminant<ExprKind>, usize, Type),
    NamedCall(std::mem::Discriminant<ExprKind>, usize, usize, Type),
    Invocation(Box<ir::RegisteredCall>, Type),
    Located(TextRange, TextSize, Option<Type>, Type),
    Name(ruff_python_ast::name::Name, Option<TextRange>, Type),
    Integer(i64),
    Scalar(ir::Constant),
    String(Box<str>),
    Binary(Binary, Type),
    Copy(ir::CopyOperation, usize, Type),
    Text(ir::TextOperation, usize, Type),
    Path(ir::PathOperation, usize, Type),
    Builtin(std::mem::Discriminant<Builtin>, usize, Type),
}

impl FixedNode {
    fn indexed(kind: &ExprKind, index: usize, ty: Type) -> Self {
        Self::Indexed(std::mem::discriminant(kind), index, ty)
    }

    /// Borrow the checked spelling when a source literal is placed on its final instruction.
    fn matches(&self, node: &ir::Node<'_>) -> bool {
        if let ir::Node::Expression(expr) = node {
            match &expr.kind {
                ExprKind::Str(value) => {
                    return matches!(self, Self::String(expected) if expected.as_ref() == value);
                }
                ExprKind::Bytes(value) => {
                    return matches!(self, Self::Scalar(ir::Constant::Bytes(expected)) if expected == value);
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
            ir::Node::Statement(Statement::Return(_)) => Self::Return,
            ir::Node::Statement(Statement::Expr(_)) => Self::Effect,
            ir::Node::Statement(Statement::Raise(Some(raised))) => Self::Raise {
                cause: raised.cause.is_some(),
            },
            ir::Node::Statement(Statement::Assign(slot, _)) => Self::Store(*slot),
            ir::Node::Statement(Statement::SetField(_, field, _)) => Self::StoreField(*field),
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
                ExprKind::Int(ref text) => Self::Integer(text.parse().ok()?),
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
                    Self::Name(name.clone(), None, expr.ty)
                }
                ExprKind::Builtin(
                    Builtin::RegisteredAttribute {
                        action: ir::AttributeAction::Load,
                        ref name,
                        range,
                    },
                    ref operands,
                ) if operands.len() == 1 => {
                    Self::Name(ruff_python_ast::name::Name::new(name), Some(range), expr.ty)
                }
                ExprKind::Invoke { .. } => {
                    Self::Invocation(Box::new(expr.python_call()?.clone()), expr.ty)
                }
                ExprKind::Local(slot) | ExprKind::Field(_, slot) | ExprKind::Bind(slot, _) => {
                    Self::indexed(&expr.kind, slot, expr.ty)
                }
                ExprKind::Builtin(Builtin::GlobalGet(slot), ref args) if args.is_empty() => {
                    Self::indexed(&expr.kind, slot, expr.ty)
                }
                ExprKind::Binary(op, _, _) => Self::Binary(op, expr.ty),
                ExprKind::Builtin(
                    Builtin::Process(ir::ProcessOperation::Copy(op), _),
                    ref operands,
                ) => Self::Copy(op, operands.len(), expr.ty),
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

    fn site_at(&self, range: TextRange) -> Option<PendingSite> {
        self.current.filter(|site| site.range == range)
    }

    pub(super) fn record(&mut self, range: TextRange, node: FixedNode) {
        if let Some(site) = self.site_at(range) {
            self.commit(site, node);
        }
    }

    pub(super) fn checked_expression(&mut self, range: TextRange, value: &Expr) {
        if let Some(site) = self.site_at(range)
            && let Some(node) = FixedNode::checked(&ir::Node::Expression(value))
        {
            self.commit(site, node);
        }
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
        let Some(sites) = self.sites.get(site.index + 1..) else {
            return false;
        };
        sites.len() == call.arguments.len()
            && values.len() >= sites.len()
            && imre_ast::argument_values(&call.arguments)
                .zip(sites.iter().zip(values))
                .all(|(arg, ((range, node), value))| {
                    arg.range() == *range
                        && node
                            .as_ref()
                            .is_some_and(|node| node.matches(&ir::Node::Expression(value)))
                })
    }

    /// Insert the ordinary callee before the already-checked arguments at this source position.
    pub(super) fn invoked(&mut self, call: &ast::ExprCall, value: &Expr) {
        let site = self.site_at(call.range()).unwrap();
        let ExprKind::Invoke { callable, .. } = &value.kind else {
            unreachable!()
        };
        let end = self.sites.len();
        ir::all_operand_nodes(callable, None, &mut |node, parent| {
            if node.has_fixed_site() {
                let located = |expr: &Expr| match expr.kind {
                    ExprKind::Builtin(Builtin::RegisteredLocation(range, ..), _) => Some(range),
                    _ => None,
                };
                let range = match node {
                    ir::Node::Expression(expr) => located(expr),
                    ir::Node::Statement(_) => None,
                }
                .or_else(|| parent.and_then(located))
                .unwrap_or_else(|| call.func.range());
                self.sites.push((range, FixedNode::checked(&node)));
            }
            true
        });
        // The callable is evaluated before the written arguments that were checked first.
        let count = self.sites.len() - end;
        self.sites[site.index + 1..].rotate_right(count);
        self.next_index += count;
        self.checked_expression(call.range(), value);
    }

    /// Place the Python callee beside the native operation. Both entries use its written operands.
    pub(super) fn positional_call(
        &mut self,
        call: &ast::ExprCall,
        value: &Expr,
        target: impl FnOnce() -> Option<Expr>,
    ) {
        if let Some(site) = self.site_at(call.range())
            && let Some(args) = value.named_call_arguments()
            && args.len() == call.arguments.args.len()
            && registered::values::fixed_positional_arguments(call)
            && let Some(node) = FixedNode::checked(&ir::Node::Expression(value))
            && let Some(target) = target()
        {
            let invoke = ir::RegisteredCall::new(
                call.range(),
                registered::values::callable_target(call),
                ir::RegisteredArguments::Positional,
            );
            self.attributes.insert(
                site.range,
                FixedAttribute::Call(Some(Box::new(target)), invoke),
            );
            self.commit(site, node);
        }
    }

    pub(super) fn field(&mut self, field: &ast::ExprAttribute, value: &Expr) {
        if let Some(site) = self.site_at(field.range()) {
            self.attribute(site, field, ir::Node::Expression(value));
        }
    }

    fn attribute(&mut self, site: PendingSite, field: &ast::ExprAttribute, value: ir::Node<'_>) {
        // Private names require the registered class's lexical mangling.
        if !ruff_python_stdlib::identifiers::is_mangled_private(field.attr.as_str())
            && let Some(native) = FixedNode::checked(&value)
        {
            let action = match value {
                ir::Node::Expression(_) => ir::AttributeAction::Load,
                ir::Node::Statement(_) => ir::AttributeAction::Store,
            };
            self.python_operation(
                site,
                field.attr.start(),
                registered::attribute_opcode(action, field),
                native,
            );
        }
    }

    /// Bind a method with already-checked positional arguments while the fixed
    /// entry keeps the collection operation.
    pub(super) fn method(
        &mut self,
        call: &ast::ExprCall,
        field: &ast::ExprAttribute,
        value: &Expr,
    ) {
        if let Some(site) = self.site_at(call.range())
            && let ExprKind::Builtin(_, operands) = &value.kind
            && operands.len() == 1 + call.arguments.args.len()
            && let Some(node) = FixedNode::checked(&ir::Node::Expression(value))
        {
            let invoke = ir::RegisteredCall::new(
                call.range(),
                registered::values::method_target(field),
                ir::RegisteredArguments::Positional,
            );
            self.attributes
                .insert(site.range, FixedAttribute::Call(None, invoke));
            self.commit(site, node);
        }
    }

    fn python_operation(
        &mut self,
        site: PendingSite,
        position: TextSize,
        operation: Builtin,
        native: FixedNode,
    ) {
        self.attributes.insert(
            site.range,
            FixedAttribute::Operation {
                operation: Box::new(operation),
                position,
            },
        );
        self.commit(site, native);
    }

    /// The fixed branch converts the written local to bool. Its Python entry tests the same handle.
    pub(super) fn branch_test(&mut self, source: &ast::Expr, test: &Expr) {
        if source.is_name_expr()
            && let ExprKind::Builtin(Builtin::Bool, args) = &test.kind
            && let [value] = args.as_slice()
            && matches!(value.kind, ExprKind::Local(_))
            && let Some(site) = self.statement_site(source.range())
        {
            let node = FixedNode::checked(&ir::Node::Expression(test));
            self.sites.insert(site.index + 1, (source.range(), node));
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

    /// A physical method stores through the actual receiver at the original attribute range.
    pub(super) fn store_field(
        &mut self,
        field: &ast::ExprAttribute,
        rhs: TextRange,
        statement: &Statement,
    ) {
        if self.wrapper_fields
            && let Some(site) = self.statement_site(rhs)
        {
            self.attribute(site, field, ir::Node::Statement(statement));
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
        method: Option<&Arc<ast::StmtFunctionDef>>,
        arguments: FrameArguments,
    ) -> Option<FixedBodySource> {
        if body.is_empty()
            || self.current.is_some()
            || source_statements != body.len()
            || names.len() != locals.len()
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
        if local_names.len() != locals.len() {
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
        assert_eq!(source.attributes.len(), 2);
        for (range, field) in &source.attributes {
            let FixedAttribute::Operation {
                operation,
                position,
            } = field
            else {
                panic!("expected attribute load")
            };
            assert!(
                matches!(&**operation, Builtin::RegisteredAttribute { action: ir::AttributeAction::Load, name, range: target } if name == "text" && target == range)
            );
            assert_eq!(&text[TextRange::new(*position, range.end())], "text");
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
            }) = &mut module.functions[id].body[0]
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
                        Arc::as_ptr(method),
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
            graph
                .registered_source_errors
                .borrow_mut()
                .insert(graph.entry, vec![Some(refused)]);
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
                    if matches!(sites.as_ref(), [FrameSite::Method(method)] if Arc::as_ptr(method) == declaration)));
            }
        }
    }

    #[test]
    fn original_source_sites_cover_rebinding_and_decline_erased_syntax() {
        let text = concat!(
            "class Receiver:\n    def read(self, value: int) -> int:\n        return value\n",
            "def combine(left: int, right: int) -> int:\n",
            "    total = left + right + left\n",
            "    total + left\n",
            "    total = right\n",
            "    return total\n",
            "def erase(value: int) -> int:\n",
            "    return +value\n",
            "print(erase(combine(2, 3)))\n",
        );
        let mut graph = SourceGraph::single(text).unwrap();
        let crate::CheckedProgram::Native(program) = crate::checked_lower(&mut graph).unwrap()
        else {
            panic!("expected native program")
        };
        let sources = &program.frame_sources;
        assert_eq!(sources.len(), 1);
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
        assert!(matches!(
            program.ir.0.functions[1].body.as_slice(),
            [Statement::Return(Expr {
                kind: ExprKind::Local(_),
                ..
            })]
        ));
    }
}
