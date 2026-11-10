//! Physical sites for operations that can also consume Python frame arguments.

use super::*;

#[derive(Clone)]
pub(crate) struct FixedBodySource {
    pub(crate) positions: Vec<TextRange>,
    pub(crate) operations: Vec<Option<FixedNode>>,
    pub(crate) local_names: HashMap<usize, registered::LocalBinding>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum FixedNode {
    Return,
    Effect,
    Store(usize),
    Read(usize, Type),
    Binary(Binary, Type),
}

impl FixedNode {
    pub(crate) fn checked(node: &ir::Node<'_>) -> Option<Self> {
        Some(match node {
            ir::Node::Statement(Statement::Return(_)) => Self::Return,
            ir::Node::Statement(Statement::Expr(_)) => Self::Effect,
            ir::Node::Statement(Statement::Assign(slot, _)) => Self::Store(*slot),
            ir::Node::Expression(expr) => match expr.kind {
                ExprKind::Local(slot) => Self::Read(slot, expr.ty),
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
    positions: Vec<TextRange>,
    operations: Vec<Option<FixedNode>>,
    current: Option<PendingSite>,
    next_index: usize,
    statements: usize,
}

impl SourceSites {
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

    pub(super) fn expression(&mut self, range: TextRange, node: FixedNode) {
        if let Some(site) = self.current
            && site.range == range
        {
            self.commit(site, node);
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
                (
                    slot,
                    registered::LocalBinding {
                        name,
                        namespace: None,
                    },
                )
            })
            .collect();
        if local_names.len() != locals.len() {
            return None;
        }
        Some(FixedBodySource {
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
                kind: ExprKind::Binary(Binary::Concat, ..),
            })]
        ));
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
        let (_, source) = &sources[&0];
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
