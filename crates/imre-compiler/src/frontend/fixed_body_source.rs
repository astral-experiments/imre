//! Physical sites for operations that can also consume Python frame arguments.

use super::*;

#[derive(Clone)]
pub(crate) struct FixedBodySource {
    pub(crate) method: Option<Arc<ast::StmtFunctionDef>>,
    pub(crate) attributes: HashMap<TextRange, FixedAttribute>,
    sites: Vec<(TextRange, Option<FixedNode>)>,
    pub(crate) local_names: HashMap<usize, registered::LocalBinding>,
}

impl FixedBodySource {
    /// Project the compact source positions for the placed function.
    pub(crate) fn positions(&self) -> Vec<TextRange> {
        self.sites.iter().map(|(range, _)| *range).collect()
    }

    pub(crate) fn matches(&self, function: &ir::Function) -> bool {
        if self.local_names.len() != function.locals.len() {
            return false;
        }
        let mut remaining = Some(self.sites.as_slice());
        ir::walk_nodes(&function.body, &mut |node, _| {
            remaining = FixedNode::consume(remaining, &node);
        });
        matches!(remaining, Some([]))
    }
}

#[derive(Clone)]
pub(crate) struct FixedAttribute {
    pub(crate) operation: Box<Builtin>,
    pub(crate) position: TextSize,
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) enum FixedNode {
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
    Branch(usize, usize),
    /// The operation keeps its checked slot or operand count, and its native type.
    Indexed(std::mem::Discriminant<ExprKind>, usize, Type),
    NamedCall(usize, usize, Type),
    ModuleRead(usize, Type),
    Integer(i64),
    Constant(ir::Constant),
    Binary(Binary, Type),
    Builtin(std::mem::Discriminant<Builtin>, usize, Type),
}

impl FixedNode {
    pub(crate) fn consume<'a>(
        sites: Option<&'a [(TextRange, Option<Self>)]>,
        node: &ir::Node<'_>,
    ) -> Option<&'a [(TextRange, Option<Self>)]> {
        let ((_, expected), rest) = sites?.split_first()?;
        (Self::checked(node).as_ref() == Some(expected.as_ref()?)).then_some(rest)
    }

    pub(super) fn indexed(kind: &ExprKind, index: usize, ty: Type) -> Self {
        Self::Indexed(std::mem::discriminant(kind), index, ty)
    }

    pub(crate) fn checked(node: &ir::Node<'_>) -> Option<Self> {
        Some(match node {
            ir::Node::Statement(Statement::If(_, yes, no)) => Self::Branch(yes.len(), no.len()),
            ir::Node::Statement(Statement::Return(_)) => Self::Return,
            ir::Node::Statement(Statement::Expr(_)) => Self::Effect,
            ir::Node::Statement(Statement::Raise(Some(raised))) => Self::Raise {
                cause: raised.cause.is_some(),
            },
            ir::Node::Statement(Statement::Assign(slot, _)) => Self::Store(*slot),
            ir::Node::Expression(expr) => match expr.kind {
                ExprKind::Str(_) | ExprKind::Bool(_) => {
                    Self::Constant(ir::Constant::from_expr(expr)?)
                }
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
                    if !args.exhaustive_enum_match
                        && args.order.iter().copied().eq(0..args.values.len()) =>
                {
                    Self::NamedCall(id, args.values.len(), expr.ty)
                }
                ExprKind::Local(slot) | ExprKind::Field(_, slot) => {
                    Self::indexed(&expr.kind, slot, expr.ty)
                }
                ExprKind::Builtin(Builtin::GlobalGet(slot), ref args) if args.is_empty() => {
                    Self::ModuleRead(slot, expr.ty)
                }
                ExprKind::Binary(op, _, _) => Self::Binary(op, expr.ty),
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
    wrapper_fields: bool,
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

    pub(super) fn expression(&mut self, range: TextRange, node: FixedNode) {
        if let Some(site) = self.site_at(range) {
            self.commit(site, node);
        }
    }

    pub(super) fn checked_expression(&mut self, range: TextRange, value: &Expr) {
        if let Some(node) = FixedNode::checked(&ir::Node::Expression(value)) {
            self.expression(range, node);
        }
    }

    pub(super) fn module_read(&mut self, name: &ast::ExprName, slot: usize, ty: Type) {
        if let Some(site) = self.site_at(name.range()) {
            self.python_operation(
                site,
                name.start(),
                Builtin::RegisteredName(name.id.to_string()),
                FixedNode::ModuleRead(slot, ty),
            );
        }
    }

    /// A Python caller loads the live name and supplies only the written arguments.
    pub(super) fn named_call(&mut self, call: &ast::ExprCall, name: &ast::ExprName, value: &Expr) {
        if let Some(site) = self.site_at(call.range())
            && call.start() == name.start()
            && !ruff_python_stdlib::identifiers::is_mangled_private(name.id.as_str())
            && let Some(args) = value.named_call_arguments()
            && args.len() == call.arguments.args.len()
            && registered::values::fixed_positional_arguments(call)
            && let Some(node) = FixedNode::checked(&ir::Node::Expression(value))
        {
            self.python_operation(
                site,
                name.end(),
                Builtin::RegisteredName(name.id.to_string()),
                node,
            );
        }
    }

    pub(super) fn field(
        &mut self,
        field: &ast::ExprAttribute,
        kind: &ExprKind,
        slot: usize,
        result: Type,
    ) {
        // Private names require the registered class's lexical mangling.
        if self.wrapper_fields
            && !ruff_python_stdlib::identifiers::is_mangled_private(field.attr.as_str())
            && let Some(site) = self.site_at(field.range())
        {
            self.python_operation(
                site,
                field.attr.start(),
                registered::values::attribute_load(field),
                FixedNode::indexed(kind, slot, result),
            );
        }
    }

    /// A source method binds a Python callable before its arguments even when
    /// the fixed entry can use a collection operation directly.
    pub(super) fn method(
        &mut self,
        call: &ast::ExprCall,
        field: &ast::ExprAttribute,
        op: &Builtin,
        result: Type,
    ) {
        if let Some(site) = self.site_at(call.range()) {
            self.python_operation(
                site,
                field.attr.start(),
                registered::values::bound_method_operation(call, field),
                FixedNode::Builtin(
                    std::mem::discriminant(op),
                    1 + call.arguments.args.len(),
                    result,
                ),
            );
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
            FixedAttribute {
                operation: Box::new(operation),
                position,
            },
        );
        self.commit(site, native);
    }

    pub(super) fn statement(&mut self, operand: TextRange, node: FixedNode) {
        if let Some(site) = self.current
            && let Some((range, _)) = self.sites.get(site.index + 1)
            && *range == operand
        {
            self.commit(site, node);
        }
    }

    pub(super) fn finish(
        self,
        source_statements: usize,
        body: &[Statement],
        names: HashMap<String, usize>,
        locals: &[Type],
        method: Option<&Arc<ast::StmtFunctionDef>>,
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
    fn changing_a_literal_invalidates_the_fixed_operation() {
        for (value, changed) in [
            (unpacking::string("same"), unpacking::string("some")),
            (ir::Expr::boolean(false), ir::Expr::boolean(true)),
        ] {
            let node = ir::Node::Expression(&value);
            let expected = [(TextRange::default(), FixedNode::checked(&node))];
            assert!(FixedNode::consume(Some(&expected), &node).is_some());
            assert!(FixedNode::consume(Some(&expected), &ir::Node::Expression(&changed)).is_none());
        }
    }

    #[test]
    fn display_elements_cannot_change_owners() {
        let list = |items| Expr {
            ty: Type::ListInt,
            kind: ExprKind::List(items),
        };
        // Both trees visit four identical list operations in the same preorder.
        let empty = || list(Vec::new());
        let original = list(vec![empty(), list(vec![empty()])]);
        let changed = list(vec![list(vec![empty()]), empty()]);
        let body = |value| vec![Statement::Return(value)];
        let original = body(original);
        let mut sites = Vec::new();
        ir::walk_nodes(&original, &mut |node, _| {
            sites.push((TextRange::default(), FixedNode::checked(&node)));
        });
        let matches = |body: &[Statement]| {
            let mut remaining = Some(sites.as_slice());
            ir::walk_nodes(body, &mut |node, _| {
                remaining = FixedNode::consume(remaining, &node);
            });
            matches!(remaining, Some([]))
        };
        assert!(matches(&original));
        assert!(!matches(&body(changed)));
    }

    #[test]
    fn moving_a_checked_statement_to_the_other_arm_invalidates_its_site() {
        let test = ir::Expr::boolean(true);
        let first = Statement::Return(ir::Expr::boolean(false));
        let second = Statement::Return(ir::Expr::boolean(true));
        let original = Statement::If(test.clone(), vec![first.clone()], vec![second.clone()]);
        let moved = Statement::If(test, Vec::new(), vec![first, second]);
        let original = ir::Node::Statement(&original);
        let sites = [(TextRange::default(), FixedNode::checked(&original))];
        assert!(FixedNode::consume(Some(&sites), &original).is_some());
        assert!(FixedNode::consume(Some(&sites), &ir::Node::Statement(&moved)).is_none());
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
                        && source.matches(&module.functions[*id])
                )
        );
        let both = text.find("def both").unwrap();
        let &(id, _, ref source) = sources
            .iter()
            .find(|(_, definition, _)| usize::from(definition.start()) == both)
            .unwrap();
        assert!(source.matches(&module.functions[id]));
        assert_eq!(source.attributes.len(), 2);
        for (range, field) in &source.attributes {
            assert!(
                matches!(&*field.operation, Builtin::RegisteredAttribute { action: ir::AttributeAction::Load, name, range: target } if name == "text" && target == range)
            );
            assert_eq!(&text[TextRange::new(field.position, range.end())], "text");
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
            assert!(!source.matches(&module.functions[id]));
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
                        source.positions(),
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
                assert_eq!(placed.fixed_sites, positions);
                assert_eq!(placed.fixed_sites.capacity(), positions.len());
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
