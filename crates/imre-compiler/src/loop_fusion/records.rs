//! Scalar replacement of a private list's fresh records during field-only traversal.

use super::*;
use crate::reductions::count_writes;
use ruff_python_ast::CmpOp;

/// A closed initializer whose only operations store positional parameters into fields.
pub(super) struct Constructor {
    class: usize,
    fields: Vec<usize>,
}

pub(super) fn constructors(module: &Module) -> Vec<Option<Constructor>> {
    module
        .functions
        .iter()
        .map(|function| {
            let (&receiver, params) = function.params.split_first()?;
            let Type::Class(class) = function.locals[receiver] else {
                return None;
            };
            let fields = &module.class_fields[class];
            if function.returns != Type::None || fields.is_empty() || fields.len() != params.len() {
                return None;
            }
            let mut layout = vec![None; fields.len()];
            let body = match function.body.as_slice() {
                [
                    body @ ..,
                    Statement::Return(Expr {
                        kind: ExprKind::None,
                        ..
                    }),
                ] => body,
                body => body,
            };
            if body.len() != fields.len() {
                return None;
            }
            let mut seen = HashSet::new();
            for statement in body {
                let Statement::SetField(base, field, value) = statement else {
                    return None;
                };
                let ExprKind::Local(slot) = value.kind else {
                    return None;
                };
                if !is_local(base, receiver)
                    || value.ty != *fields.get(*field)?
                    || !seen.insert(slot)
                {
                    return None;
                }
                let position = params.iter().position(|param| *param == slot)?;
                if layout[*field].replace(position).is_some() {
                    return None;
                }
            }
            Some(Constructor {
                class,
                fields: layout.into_iter().collect::<Option<_>>()?,
            })
        })
        .collect()
}

fn is_local(expr: &Expr, slot: usize) -> bool {
    matches!(expr.kind, ExprKind::Local(id) if id==slot)
}

/// Keep scalar field values separate from the identities of their enclosing records.
pub(super) enum Row {
    Scalar(Expr),
    Record(Vec<Row>),
}

impl Row {
    pub(super) fn build(
        expr: &Expr,
        constructors: &[Option<Constructor>],
        inputs: &mut HashSet<usize>,
    ) -> Option<Self> {
        if let ExprKind::Construct { initializer, args } = &expr.kind {
            let init = constructors.get(*initializer)?.as_ref()?;
            if expr.ty != Type::Class(init.class)
                || args.values.len() != init.fields.len()
                || !args.order.iter().copied().eq(0..args.values.len())
            {
                return None;
            }
            let fields = init
                .fields
                .iter()
                .map(|&index| Self::build(&args.values[index], constructors, inputs))
                .collect::<Option<_>>()?;
            Some(Self::Record(fields))
        } else if matches!(
            expr.ty,
            Type::Int | Type::Bool | Type::Float | Type::Str | Type::Bytes
        ) && producer_value(expr, inputs)
        {
            Some(Self::Scalar(expr.clone()))
        } else {
            None
        }
    }

    /// Computed fields have one value per row, even if multiple projections use it.
    pub(super) fn stage(
        &mut self,
        old: usize,
        ordinal: usize,
        first: usize,
        locals: &mut Vec<Type>,
        code: &mut Vec<Statement>,
    ) {
        match self {
            Self::Record(fields) => {
                for field in fields {
                    field.stage(old, ordinal, first, locals, code);
                }
            }
            Self::Scalar(expr) => {
                remap_index(expr, old, ordinal);
                if !matches!(expr.kind, ExprKind::Local(_)) {
                    let slot = first + locals.len();
                    locals.push(expr.ty);
                    let value = std::mem::replace(
                        expr,
                        Expr {
                            ty: expr.ty,
                            kind: ExprKind::Local(slot),
                        },
                    );
                    code.push(Statement::Assign(slot, value));
                }
            }
        }
    }

    /// Follow typed field projections from the caller's chosen virtual root.
    pub(super) fn projection(&self, expr: &Expr, root: &impl Fn(&Expr) -> bool) -> Option<&Self> {
        if root(expr) {
            return Some(self);
        }
        match &expr.kind {
            ExprKind::Field(base, field) => match self.projection(base, root)? {
                Self::Record(fields) => fields.get(*field),
                Self::Scalar(_) => None,
            },
            _ => None,
        }
    }
}

struct Traversal<'a> {
    alias: usize,
    cursor: usize,
    row: usize,
    clear: &'a [usize],
    body: &'a [Statement],
}

/// Match the checked iterator lowering, including its unit increment and exhaustion.
fn traversal(statement: &Statement, list: usize) -> Option<Traversal<'_>> {
    let Statement::Scope { body, clear } = statement else {
        return None;
    };
    let [
        Statement::Assign(alias, source),
        Statement::Assign(cursor, zero),
        Statement::While(test, body, orelse),
    ] = body.as_slice()
    else {
        return None;
    };
    let ExprKind::Compare(args, ops) = &test.kind else {
        return None;
    };
    let [index, length] = args.as_slice() else {
        return None;
    };
    let ExprKind::Builtin(Builtin::Len, collection) = &length.kind else {
        return None;
    };
    let [receiver] = collection.as_slice() else {
        return None;
    };
    if !is_local(source, list)
        || !integer(zero, "0")
        || !matches!(ops.as_slice(), [CmpOp::Lt])
        || !is_local(index, *cursor)
        || !is_local(receiver, *alias)
        || !orelse.is_empty()
    {
        return None;
    }
    let [
        Statement::Assign(row, read),
        Statement::Assign(next, increment),
        tail @ ..,
    ] = body.as_slice()
    else {
        return None;
    };
    let ExprKind::Builtin(Builtin::SequenceGetInBounds, args) = &read.kind else {
        return None;
    };
    let [receiver, index] = args.as_slice() else {
        return None;
    };
    let ExprKind::Binary(Binary::Add, left, one) = &increment.kind else {
        return None;
    };
    if !is_local(receiver, *alias)
        || !is_local(index, *cursor)
        || next != cursor
        || !is_local(left, *cursor)
        || !integer(one, "1")
        || clear
            .iter()
            .any(|slot| ![alias, cursor, row].contains(&slot))
    {
        return None;
    }
    Some(Traversal {
        alias: *alias,
        cursor: *cursor,
        row: *row,
        clear,
        body: tail,
    })
}

pub(super) fn fuse(
    function: &Function,
    first: usize,
    constructors: &[Option<Constructor>],
) -> Option<(usize, Vec<Statement>, Vec<Type>)> {
    let (bound, source_index, producer) = counted(&function.body[first])?;
    let [
        Statement::Expr(Expr {
            kind: ExprKind::Builtin(Builtin::Append, args),
            ..
        }),
    ] = producer
    else {
        return None;
    };
    let [collection, value] = args.as_slice() else {
        return None;
    };
    let ExprKind::Local(list) = collection.kind else {
        return None;
    };
    if !matches!(value.kind, ExprKind::Construct { .. })
        || matches!(bound.kind,ExprKind::Local(slot) if slot==source_index)
        || bound.ty != Type::Int
        || function.locals[source_index] != Type::Int
    {
        return None;
    }
    let last = next_loop(function, first)?;
    let visit = traversal(&function.body[last], list)?;
    let mut inputs = HashSet::new();
    let mut row = Row::build(value, constructors, &mut inputs)?;
    if let ExprKind::Local(slot) = bound.kind {
        inputs.insert(slot);
    }
    inputs.insert(source_index);
    if [visit.alias, visit.cursor, visit.row]
        .iter()
        .any(|slot| inputs.contains(slot) || function.params.contains(slot))
        || !preserves(&function.body[first + 1..last], &inputs)
        || !preserves(visit.body, &inputs)
    {
        return None;
    }
    let rebound = function.expression_rebound();
    if inputs.iter().chain([&list]).any(|&slot| rebound[slot])
        || inputs.iter().any(|slot| {
            *slot != source_index
                && !function.params.contains(slot)
                && !function.body[..first]
                    .iter()
                    .any(|s| matches!(s,Statement::Assign(id,_) if id==slot))
        })
        || !private_lists(function, first, &HashMap::from([(list, 1)]), &inputs, 0)
    {
        return None;
    }

    let ordinal = function.locals.len();
    let mut locals = vec![Type::Int];
    let mut staged = Vec::new();
    row.stage(source_index, ordinal, ordinal, &mut locals, &mut staged);
    let mut body = visit.body.to_vec();
    let mut projections = 0;
    let mut observable = false;
    ir::walk_expressions_mut(&mut body, &mut |expr| {
        if matches!(expr.kind, ExprKind::Field(..))
            && let Some(Row::Scalar(value)) = row.projection(expr, &|e| is_local(e, visit.row))
        {
            *expr = value.clone();
            projections += 1;
        }
        observable |= is_local(expr, visit.row) || observable_consumer(expr);
    });
    if observable || projections == 0 {
        return None;
    }

    // The iterator's scratch slots and its current row cannot be used elsewhere.
    // Count hidden moves, iterator outputs and scope cleanup as well as assignments.
    let mut reads = vec![0; function.locals.len()];
    let mut writes = vec![0; function.locals.len()];
    ir::walk_expressions(&function.body, &mut |expr| {
        if let ExprKind::Local(slot) = expr.kind {
            reads[slot] += 1;
        }
    });
    count_writes(&function.body, &mut writes);
    for (slot, uses, assignments) in [
        (visit.alias, 2, 1),
        (visit.cursor, 3, 2),
        (visit.row, projections, 1),
    ] {
        if reads[slot] != uses
            || writes[slot] != assignments + usize::from(visit.clear.contains(&slot))
        {
            return None;
        }
    }

    staged.extend(body);
    let mut combined = function.body[first].clone();
    let Statement::Scope { body, .. } = &mut combined else {
        unreachable!()
    };
    let Statement::For { target, body, .. } = &mut body[0] else {
        unreachable!()
    };
    *target = ordinal;
    *body = staged;
    let mut result = loop_prefix(function, first, last, bound, source_index, ordinal);
    result.push(combined);
    Some((last, result, locals))
}

#[cfg(test)]
mod tests {
    use crate::{CompileOptions, compile};

    #[test]
    fn virtual_rows_require_a_pure_initializer_and_field_only_uses() {
        let source = r#"
class Leaf:
    value: int
    def __init__(self, value: int) -> None:
        self.value = value

class Row:
    leaf: Leaf
    tag: int
    def __init__(self, leaf: Leaf, tag: int) -> None:
        self.tag = tag
        self.leaf = leaf

def total(n: int, tag: int) -> int:
    rows: list[Row] = []
    i = -7
    for i in range(n):
        rows.append(Row(Leaf(i), tag))
    result = 0
    for row in rows:
        result += row.leaf.value + row.tag
    return result + i
print(total(int(input()), 3))
"#;
        let traverses_records = |source: &str| {
            let llvm = compile(source, &CompileOptions::default()).unwrap();
            llvm.lines().any(|line| {
                line.contains("call ")
                    && (line.contains("@imre_seq_borrow_index") || line.contains("@imre_seq_get"))
            })
        };
        assert!(!traverses_records(source));
        for changed in [
            source.replace("return result + i", "return result + i + len(rows)"),
            source.replace(
                "self.value = value",
                "assert value != 2\n        self.value = value",
            ),
            source.replace(
                "result += row.leaf.value + row.tag",
                "row.leaf.value += 1\n        result += row.tag",
            ),
            source.replace(
                "result += row.leaf.value + row.tag",
                "tag += 1\n        result += row.tag",
            ),
        ] {
            assert!(traverses_records(&changed));
        }
    }
}
