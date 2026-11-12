//! Physical sites for operations that can also consume Python frame arguments.

use super::*;

#[derive(Clone)]
pub(crate) struct FixedBodySource {
    pub(crate) fields: HashMap<TextRange, FixedField>,
    pub(crate) positions: Vec<TextRange>,
    pub(crate) operations: Vec<Option<FixedNode>>,
    pub(crate) local_names: HashMap<usize, registered::LocalBinding>,
}

impl FixedBodySource {
    pub(crate) fn matches(&self, function: &ir::Function) -> bool {
        if self.local_names.len() != function.locals.len() {
            return false;
        }
        let mut remaining = Some(self.operations.as_slice());
        ir::walk_nodes(&function.body, &mut |node, _| {
            remaining = FixedNode::consume(remaining, &node);
        });
        matches!(remaining, Some([]))
    }
}

#[derive(Clone, Debug)]
pub(crate) struct FixedField {
    pub(crate) name: String,
    pub(crate) position: TextSize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum FixedNode {
    Return,
    Effect,
    Store(usize),
    Read(usize, Type),
    Integer(i64),
    Field(usize, Type),
    Binary(Binary, Type),
}

impl FixedNode {
    pub(crate) fn consume<'a>(
        operations: Option<&'a [Option<Self>]>,
        node: &ir::Node<'_>,
    ) -> Option<&'a [Option<Self>]> {
        let (expected, rest) = operations?.split_first()?;
        (Self::checked(node) == Some((*expected)?)).then_some(rest)
    }

    pub(crate) fn checked(node: &ir::Node<'_>) -> Option<Self> {
        Some(match node {
            ir::Node::Statement(Statement::Return(_)) => Self::Return,
            ir::Node::Statement(Statement::Expr(_)) => Self::Effect,
            ir::Node::Statement(Statement::Assign(slot, _)) => Self::Store(*slot),
            ir::Node::Expression(expr) => match expr.kind {
                ExprKind::Int(ref text) => Self::Integer(text.parse().ok()?),
                ExprKind::Local(slot) => Self::Read(slot, expr.ty),
                ExprKind::Field(_, slot) => Self::Field(slot, expr.ty),
                ExprKind::Binary(op, _, _) => Self::Binary(op, expr.ty),
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
    fields: Option<HashMap<TextRange, FixedField>>,
    positions: Vec<TextRange>,
    operations: Vec<Option<FixedNode>>,
    current: Option<PendingSite>,
    next_index: usize,
    statements: usize,
}

impl SourceSites {
    pub(super) fn new(has_wrapper_link: bool) -> Self {
        Self {
            fields: has_wrapper_link.then(HashMap::new),
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

    pub(super) fn leave(&mut self, parent: Option<PendingSite>) -> bool {
        let current = std::mem::replace(&mut self.current, parent);
        current.is_some_and(|site| self.operations.get(site.index).is_some_and(Option::is_some))
    }

    fn commit(&mut self, site: PendingSite, node: FixedNode) {
        if self.positions.len() <= site.index {
            self.positions.resize(site.index + 1, TextRange::default());
            self.operations.resize(site.index + 1, None);
        }
        self.positions[site.index] = site.range;
        self.operations[site.index] = Some(node);
    }

    fn site_at(&self, range: TextRange) -> Option<PendingSite> {
        self.current.filter(|site| site.range == range)
    }

    pub(super) fn expression(&mut self, range: TextRange, node: FixedNode) {
        if let Some(site) = self.site_at(range) {
            self.commit(site, node);
        }
    }

    pub(super) fn field(
        &mut self,
        range: TextRange,
        member: &ast::Identifier,
        slot: usize,
        result: Type,
    ) {
        // Private names require the registered class's lexical mangling.
        if let Some(site) = self.site_at(range)
            && let Some(fields) = &mut self.fields
            && !ruff_python_stdlib::identifiers::is_mangled_private(member.as_str())
        {
            fields.insert(
                range,
                FixedField {
                    name: member.to_string(),
                    position: member.start(),
                },
            );
            self.commit(site, FixedNode::Field(slot, result));
        }
    }

    pub(super) fn statement(&mut self, operand: TextRange, node: FixedNode) {
        if let Some(site) = self.current
            && self.positions.get(site.index + 1) == Some(&operand)
        {
            self.commit(site, node);
            self.statements += 1;
        }
    }

    pub(super) fn finish(
        self,
        source_statements: usize,
        body: &[Statement],
        names: HashMap<String, usize>,
        locals: &[Type],
        method: bool,
    ) -> Option<FixedBodySource> {
        if self.current.is_some()
            || self.statements != source_statements
            || self.statements != body.len()
            || names.len() != locals.len()
            || !matches!(body.last(), Some(Statement::Return(_)))
        {
            return None;
        }
        let local_names: HashMap<_, _> = names
            .into_iter()
            .map(|(name, slot)| {
                // The Python binder spells private method locals with their class prefix.
                if method && ruff_python_stdlib::identifiers::is_mangled_private(&name) {
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
            fields: self.fields.unwrap_or_default(),
            positions: self.positions,
            operations: self.operations,
            local_names,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn abandoning_unselected_sites_leaves_the_tables_unallocated() {
        let mut sites = SourceSites::default();
        let outer = sites.enter(TextRange::new(0.into(), 8.into()));
        let inner = sites.enter(TextRange::new(2.into(), 5.into()));
        assert!(!sites.leave(inner));
        assert!(!sites.leave(outer));
        assert_eq!(sites.positions.capacity(), 0);
        assert_eq!(sites.operations.capacity(), 0);
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
        assert!(fixed.frame_sources.is_empty());
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
                .any(|(id, start, source)| usize::from(*start) == native
                    && source.matches(&module.functions[*id]))
        );
        let both = text.find("def both").unwrap();
        let &(id, _, ref source) = sources
            .iter()
            .find(|(_, start, _)| usize::from(*start) == both)
            .unwrap();
        assert!(source.matches(&module.functions[id]));
        assert_eq!(source.fields.len(), 2);
        for (range, field) in &source.fields {
            assert_eq!(field.name, "text");
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
        let mut graph = SourceGraph::load(&entry, Some(project.path()), &[]).unwrap();
        let crate::CheckedProgram::Native(fixed) = crate::checked_lower(&mut graph).unwrap() else {
            panic!("expected a fixed wrapper graph")
        };
        let mut owners: Vec<_> = fixed
            .frame_sources
            .iter()
            .filter(|(_, _, body)| body.local_names.contains_key(&1))
            .map(|(_, start, _)| usize::from(*start))
            .collect();
        owners.sort_unstable();
        assert_eq!(
            owners,
            text.match_indices("def send(")
                .map(|(at, _)| at)
                .collect::<Vec<_>>()
        );
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
        let spellings: Vec<_> = source.positions.iter().map(|range| &text[*range]).collect();
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
        assert!(matches!(source.operations[0], Some(FixedNode::Store(2))));
        assert!(matches!(source.operations[10], Some(FixedNode::Store(2))));
        assert!(matches!(
            source.operations[13],
            Some(FixedNode::Read(2, Type::Int))
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
