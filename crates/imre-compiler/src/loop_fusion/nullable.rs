//! Fuse aligned optional columns whose consumers skip every all-None boundary row.
use super::records::{Constructor, Row};
use super::*;
use crate::reductions::count_writes;
use ruff_python_ast::UnaryOp;

fn local(expr: &Expr) -> Option<usize> {
    match expr.kind {
        ExprKind::Local(slot) => Some(slot),
        _ => None,
    }
}

/// Return the receiver and value of a standalone append.
fn append(statement: &Statement) -> Option<(usize, &Expr)> {
    let Statement::Expr(Expr {
        kind: ExprKind::Builtin(Builtin::Append, args),
        ..
    }) = statement
    else {
        return None;
    };
    let [receiver, value] = args.as_slice() else {
        return None;
    };
    Some((local(receiver)?, value))
}

fn none_test(expr: &Expr) -> Option<usize> {
    let ExprKind::Builtin(Builtin::IsNone, args) = &expr.kind else {
        return None;
    };
    let [value] = args.as_slice() else {
        return None;
    };
    local(value)
}

fn unwrapped(expr: &Expr) -> Option<usize> {
    let ExprKind::Unwrap(inner) = &expr.kind else {
        return None;
    };
    local(inner)
}

fn plus(value: Expr, offset: i128) -> Expr {
    if offset == 0 {
        return value;
    }
    Expr {
        ty: Type::Int,
        kind: ExprKind::Binary(
            Binary::Add,
            Box::new(value),
            Box::new(Expr::integer(offset)),
        ),
    }
}

struct Column {
    list: usize,
    row: Row,
    suffix: usize,
}

/// Read a range producer and its trailing absent values. Each column must have
/// one present value per ordinal and the same number of absent values at each end.
fn columns(
    function: &Function,
    first: usize,
    producer: &[Statement],
    constructors: &[Option<Constructor>],
    inputs: &mut HashSet<usize>,
) -> Option<(Vec<Column>, usize, usize)> {
    let mut columns: Vec<Column> = Vec::new();
    for statement in producer {
        let (list, value) = append(statement)?;
        let ExprKind::Present(inner) = &value.kind else {
            return None;
        };
        if columns.iter().any(|col| col.list == list) {
            return None;
        }
        columns.push(Column {
            list,
            row: Row::build(inner, constructors, inputs)?,
            suffix: 0,
        });
    }
    let prefix = seed(function, first, columns.first()?.list)?.len();
    let mut end = first;
    for statement in &function.body[first + 1..] {
        let Some((list, value)) = append(statement) else {
            break;
        };
        let column = columns.iter_mut().find(|c| c.list == list)?;
        if !absent(value) {
            return None;
        }
        column.suffix += 1;
        end += 1;
    }
    let suffix = columns[0].suffix;
    columns
        .iter()
        .all(|col| col.suffix == suffix)
        .then_some((columns, prefix, end))
}

struct Guarded {
    bindings: HashMap<usize, usize>,
    active: Vec<Statement>,
    uses: HashMap<usize, usize>,
}

/// Each list read must bind a distinct optional. Only presence-guarded blocks
/// may follow, so the consumer has no observable work on an all-None row.
fn guarded_body(body: &[Statement], columns: &[Column], index: usize) -> Option<Guarded> {
    let (reads, guards) = body.split_at_checked(columns.len())?;
    let mut bindings = HashMap::new();
    let mut lists = HashSet::new();
    for statement in reads {
        let Statement::Assign(slot, read) = statement else {
            return None;
        };
        let list = row_read(read, index)?;
        let column = columns.iter().position(|c| c.list == list)?;
        if !lists.insert(list) || bindings.insert(*slot, column).is_some() {
            return None;
        }
    }
    let mut uses = HashMap::new();
    let mut active = Vec::new();
    for guard in guards {
        let Statement::If(test, yes, no) = guard else {
            return None;
        };
        let ExprKind::Unary(UnaryOp::Not, test) = &test.kind else {
            return None;
        };
        let slot = none_test(test)?;
        if !no.is_empty() || !bindings.contains_key(&slot) {
            return None;
        }
        *uses.entry(slot).or_insert(0) += 1;
        active.extend_from_slice(yes);
    }
    (!active.is_empty()).then_some(Guarded {
        bindings,
        active,
        uses,
    })
}

/// Substitute proven-present scalar projections. Reading an optional or a
/// nested record as an object would expose its identity and rejects the plan.
fn forward(
    body: &mut [Statement],
    bindings: &HashMap<usize, usize>,
    columns: &[Column],
    uses: &mut HashMap<usize, usize>,
) -> bool {
    let mut safe = true;
    ir::walk_expressions_mut(body, &mut |expr| {
        if let Some(slot) = none_test(expr).filter(|slot| bindings.contains_key(slot)) {
            *uses.entry(slot).or_default() += 1;
            *expr = Expr::boolean(false);
            return;
        }
        let mut root = &*expr;
        while let ExprKind::Field(base, _) = &root.kind {
            root = base;
        }
        if let Some(slot) = unwrapped(root)
            && let Some(column) = bindings.get(&slot)
            && let Some(Row::Scalar(value)) = columns[*column]
                .row
                .projection(expr, &|base| unwrapped(base) == Some(slot))
        {
            *uses.entry(slot).or_default() += 1;
            *expr = value.clone();
        } else if local(expr).is_some_and(|slot| bindings.contains_key(&slot))
            || observable_consumer(expr)
        {
            safe = false;
        }
    });
    safe
}

/// Keep the original loops on the list-length overflow path. The fast loop
/// visits only its present interval and restores both Python induction variables.
pub(super) fn fuse(
    function: &Function,
    first: usize,
    constructors: &[Option<Constructor>],
) -> Option<(usize, Vec<Statement>, Vec<Type>)> {
    let (bound, source, producer) = counted(&function.body[first])?;
    if bound.ty != Type::Int || function.locals[source] != Type::Int || local(bound) == Some(source)
    {
        return None;
    }
    let mut inputs = HashSet::from([source]);
    inputs.extend(local(bound));
    let (mut columns, prefix, end) = columns(function, first, producer, constructors, &mut inputs)?;
    let last = next_loop(function, end)?;
    let (length, index, consumer) = ordinal_loop(&function.body[last])?;
    let ExprKind::Builtin(Builtin::Len, args) = &length.kind else {
        return None;
    };
    let [receiver] = args.as_slice() else {
        return None;
    };
    let anchor = local(receiver)?;
    if function.locals[index] != Type::Int
        || !columns.iter().any(|c| c.list == anchor)
        || (source != index && inputs.contains(&index))
    {
        return None;
    }
    let Guarded {
        bindings,
        mut active,
        mut uses,
    } = guarded_body(consumer, &columns, index)?;
    let gap = &function.body[end + 1..last];
    let rebound = function.expression_rebound();
    if !preserves(gap, &inputs)
        || !preserves(consumer, &inputs)
        || !preserves(consumer, &HashSet::from([index]))
        || inputs.iter().any(|&slot| {
            rebound[slot]
                || (slot != source
                    && !function.params.contains(&slot)
                    && !function.body[..first]
                        .iter()
                        .any(|s| matches!(s,Statement::Assign(to,_) if *to==slot)))
        })
        || columns.iter().any(|c| rebound[c.list])
        || bindings
            .keys()
            .any(|slot| inputs.contains(slot) || *slot == index || function.params.contains(slot))
    {
        return None;
    }
    let reads = columns
        .iter()
        .map(|c| (c.list, 1 + c.suffix + usize::from(c.list == anchor)))
        .collect();
    if !private_lists(function, first, &reads, &inputs, prefix) {
        return None;
    }

    let ordinal = function.locals.len();
    let mut locals = vec![Type::Int];
    let mut body = vec![Statement::Assign(
        index,
        plus(Expr::local(ordinal, Type::Int), prefix as i128),
    )];
    for col in &mut columns {
        col.row
            .stage(source, ordinal, ordinal, &mut locals, &mut body);
    }
    if !forward(&mut active, &bindings, &columns, &mut uses) {
        return None;
    }
    let mut all_reads = vec![0usize; function.locals.len()];
    ir::walk_expressions(&function.body, &mut |expr| {
        if let Some(slot) = local(expr) {
            all_reads[slot] += 1;
        }
    });
    let mut writes = vec![0; function.locals.len()];
    count_writes(&function.body, &mut writes);
    if bindings
        .keys()
        .any(|slot| all_reads[*slot] != uses.get(slot).copied().unwrap_or(0) || writes[*slot] != 1)
    {
        return None;
    }
    body.extend(active);
    let mut loop_ = function.body[first].clone();
    let Statement::Scope { body: scope, .. } = &mut loop_ else {
        unreachable!()
    };
    let Statement::For {
        target,
        body: inner,
        ..
    } = &mut scope[0]
    else {
        unreachable!()
    };
    *target = ordinal;
    *inner = body;
    let mut fast = Vec::new();
    if source != index {
        fast.push(Statement::If(
            greater(bound, 0),
            vec![Statement::Assign(source, plus(bound.clone(), -1))],
            vec![],
        ));
    }
    fast.extend_from_slice(gap);
    fast.push(loop_);
    let padding = prefix as i128 + columns[0].suffix as i128;
    if padding > 0 {
        let present = Expr {
            ty: Type::Int,
            kind: ExprKind::Conditional(
                Box::new(greater(bound, 0)),
                Box::new(bound.clone()),
                Box::new(Expr::integer(0)),
            ),
        };
        fast.push(Statement::Assign(index, plus(present, padding - 1)));
    }
    let original = function.body[first..=last].to_vec();
    Some((
        last,
        vec![Statement::If(
            greater(bound, crate::ranges::MAX_LIST_LEN - padding),
            original,
            fast,
        )],
        locals,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_skip_proven_absent_and_unobserved_rows() {
        let source = r#"
class Entry:
    amount: int
    def __init__(self, amount: int) -> None:
        self.amount = amount

def total(n: int) -> int:
    rows: list[Entry | None] = [None]
    i = -3
    j = -5
    for i in range(n):
        rows.append(Entry(i))
    result = 0
    for j in range(len(rows)):
        row = rows[j]
        if row is not None:
            result += row.amount + j
    return result + i + j
print(total(int(input())))
"#;
        let eligible = |source: &str| {
            let graph = crate::modules::SourceGraph::single(source).unwrap();
            let module = crate::frontend::lower(&graph).unwrap();
            let constructors = records::constructors(&module);
            module.functions.iter().any(|function| {
                (0..function.body.len()).any(|first| fuse(function, first, &constructors).is_some())
            })
        };
        assert!(eligible(source));
        for rejected in [
            source.replace("return result + i + j", "return result + i + j + len(rows)"),
            source.replace("= [None]", "= [Entry(0)]"),
            source.replace(
                "result += row.amount + j",
                "result += row.amount + j\n            j = 23",
            ),
            source.replace(
                "result += row.amount + j",
                "result += row.amount + j\n        else:\n            result += 1",
            ),
            source.replace(
                "self.amount = amount",
                "assert amount != 4\n        self.amount = amount",
            ),
        ] {
            assert!(!eligible(&rejected), "{rejected}");
        }
    }
}
