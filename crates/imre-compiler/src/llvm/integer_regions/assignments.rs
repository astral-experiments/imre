//! Lower a run of pure integer assignments under one range and ownership guard.

use super::*;

struct Input {
    slot: usize,
    width: u32,
    node: usize,
}

struct Assignments {
    summary: Summary,
    inputs: Vec<Input>,
    writes: Vec<(usize, usize)>,
}

impl Assignments {
    /// Build a small expression graph and stop before an effect or an unproven result.
    fn analyze(statements: &[Statement], owns: impl Fn(usize) -> bool) -> Option<Self> {
        let [
            Statement::Assign(_, first),
            Statement::Assign(..),
            Statement::Assign(..),
            ..,
        ] = statements
        else {
            return None;
        };
        if first.ty != Type::Int
            || !matches!(
                first.kind,
                ExprKind::Binary(..)
                    | ExprKind::Negate(_)
                    | ExprKind::Conditional(..)
                    | ExprKind::Builtin(Builtin::Abs, _)
            )
        {
            return None;
        }
        let mut summary = Summary {
            nodes: Vec::new(),
            result: 0,
            parameters: 0,
        };
        let mut inputs = Vec::<Input>::new();
        let mut writes = Vec::new();
        for stmt in statements.iter().take(MAX_STATEMENTS) {
            let Statement::Assign(target, value) = stmt else {
                break;
            };
            if value.ty != Type::Int || !owns(*target) {
                break;
            }
            let accumulating = matches!(&value.kind,
                ExprKind::Binary(Binary::Add, left, _) if matches!(left.kind, ExprKind::Local(slot) if slot == *target));
            let (old_nodes, old_inputs) = (summary.nodes.len(), inputs.len());
            let result = summary.expression(value, &mut |slot, graph| {
                if let Some(&(_, node)) = writes.iter().rev().find(|&&(target, _)| target == slot) {
                    return Some(node);
                }
                if let Some(input) = inputs.iter().find(|input| input.slot == slot) {
                    return Some(input.node);
                }
                if inputs.len() == MAX_PARAMETERS {
                    return None;
                }
                // Two 30-bit inputs can be differenced and squared as small integers.
                // A 61-bit running total leaves room for that square.
                let width = if accumulating && slot == *target {
                    61
                } else {
                    30
                };
                let limit = 1_i128 << (width - 1);
                let node = graph.push(
                    Operation::Parameter(inputs.len()),
                    Interval {
                        min: -limit,
                        max: limit - 1,
                    },
                )?;
                inputs.push(Input { slot, width, node });
                Some(node)
            });
            let Some(node) = result.filter(|_| {
                summary.nodes[old_nodes..]
                    .iter()
                    .all(|n| n.interval.min >= MIN_SMALL && n.interval.max <= MAX_SMALL)
            }) else {
                summary.nodes.truncate(old_nodes);
                inputs.truncate(old_inputs);
                break;
            };
            writes.push((*target, node));
            summary.result = node;
        }
        if writes.len() < 3
            || inputs.is_empty()
            || summary
                .nodes
                .iter()
                .filter(|n| {
                    !matches!(
                        n.operation,
                        Operation::Parameter(_) | Operation::Constant(_)
                    )
                })
                .count()
                < 4
        {
            return None;
        }
        summary.parameters = inputs.len();
        Some(Self {
            summary,
            inputs,
            writes,
        })
    }
}

impl Emitter<'_, '_> {
    /// Evaluate adjacent assignments in native registers when all input tags and ranges match.
    pub(in crate::llvm) fn integer_assignments(&mut self, statements: &[Statement]) -> usize {
        let Some(group) = Assignments::analyze(statements, |slot| self.owns_slot(slot)) else {
            return 0;
        };
        let mut parameters = Vec::with_capacity(group.inputs.len());
        let mut tags: Option<String> = None;
        let mut ranges: Option<String> = None;
        for input in &group.inputs {
            let value = self.borrow_slot(input.slot).operand;
            let limit = 1_u64 << input.width;
            let shifted = self.instruction(format!("add i64 {value}, {limit}"));
            let fits = self.instruction(format!("icmp ult i64 {shifted}, {}", limit * 2));
            ranges = Some(match ranges {
                Some(previous) => self.instruction(format!("and i1 {previous}, {fits}")),
                None => fits,
            });
            tags = Some(match tags {
                Some(previous) => self.instruction(format!("and i64 {previous}, {value}")),
                None => value.clone(),
            });
            parameters.push(value);
        }
        let tag = self.instruction(format!("trunc i64 {} to i1", tags.unwrap()));
        let valid = self.instruction(format!("and i1 {tag}, {}", ranges.unwrap()));
        let (fast, slow, end) = (self.label(), self.label(), self.label());
        self.line(format!(
            "br i1 {valid}, label %{fast}, label %{slow}, !prof !1"
        ));
        self.terminated = true;

        self.start(&fast);
        let values =
            self.integer_region_values(&group.summary, |index| parameters[index].as_str(), true);
        for (index, &(slot, node)) in group.writes.iter().enumerate() {
            if group.writes[index + 1..]
                .iter()
                .any(|&(later, _)| later == slot)
            {
                continue;
            }
            let tagged = self.instruction(format!("or i64 {}, 1", values[node]));
            let value = Value {
                ty: Type::Int,
                operand: tagged,
                small_integer: true,
            };
            let previous_small = group.inputs.iter().any(|input| input.slot == slot);
            self.store_with_previous_small(slot, &value, previous_small);
        }
        self.jump(&end);

        self.start(&slow);
        for stmt in &statements[..group.writes.len()] {
            self.statement(stmt);
        }
        self.jump(&end);
        self.start(&end);
        group.writes.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stop_before_a_product_can_overflow_or_a_call_can_observe_locals() {
        for next in ["total * total", "notify(total)"] {
            let source = format!(
                "def series(x: int, y: int, total: int) -> int:\n    delta = y - x\n    total = total + delta\n    magnitude = abs(delta)\n    square = delta * delta\n    total = total + square\n    end = {next}\n    return end + magnitude\n\ndef notify(x: int) -> int:\n    print(x)\n    return x\n"
            );
            let graph = crate::modules::SourceGraph::single(&source).unwrap();
            let module = crate::frontend::lower(&graph).unwrap();
            let group = Assignments::analyze(&module.functions[0].body, |_| true).unwrap();
            assert_eq!(group.writes.len(), 5);
            assert_eq!(
                group
                    .inputs
                    .iter()
                    .map(|input| input.width)
                    .collect::<Vec<_>>(),
                [30, 30, 61]
            );
        }
    }
}
