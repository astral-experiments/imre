//! Fuse a sorted key union with a checked list build of generated records.

use crate::ir::{Builtin, Expr, ExprKind, Function, Module, Statement, Type};

pub(crate) fn specialize(module: &mut Module) {
    for id in 0..module.functions.len() {
        if let Some(rows) = sorted_rows(module, &module.functions[id]) {
            let body = &mut module.functions[id].body;
            body.truncate(body.len() - 2);
            body.push(Statement::Return(rows));
        }
    }
}

fn sorted_rows(module: &Module, function: &Function) -> Option<Expr> {
    // A handler or finally can observe the keys binding. Only the terminal
    // assignment and return at function scope can be replaced together.
    let [
        ..,
        Statement::Assign(keys, sorted),
        Statement::Return(result),
    ] = function.body.as_slice()
    else {
        return None;
    };
    let ExprKind::Builtin(Builtin::SortedStringKeys, inputs) = &sorted.kind else {
        return None;
    };
    if !inputs
        .iter()
        .all(|dict| module.types.is_default_list_dict(dict.ty))
    {
        return None;
    }
    let (item, element) = list_projection(result, *keys)?;
    let (Type::Class(class), ExprKind::Construct { args, .. }) = (element.ty, &element.kind) else {
        return None;
    };
    // Source evaluation must already be in field order. Keyword-only fields
    // may appear earlier in the record than in the initializer's parameters.
    if module.types.element(result.ty) != Some(element.ty)
        || module.class_metadata[class].initializer_fields.as_ref()? != &args.order
    {
        return None;
    }
    let mut columns = Vec::with_capacity(args.values.len());
    for (value, field) in args.values.iter().zip(&module.class_fields[class]) {
        if value.ty != *field {
            return None;
        }
        let column = match &value.kind {
            ExprKind::Local(slot) if *slot == item => 0,
            ExprKind::Builtin(Builtin::DictGet, operands) => {
                let [dict, key] = operands.as_slice() else {
                    return None;
                };
                let (ExprKind::Local(dict), ExprKind::Local(key)) = (&dict.kind, &key.kind) else {
                    return None;
                };
                if *key != item {
                    return None;
                }
                let index = inputs.iter().position(
                    |input| matches!(input.kind, ExprKind::Local(slot) if slot == *dict),
                )?;
                u8::try_from(index + 1).ok()?
            }
            _ => return None,
        };
        columns.push(column);
    }
    if columns.iter().all(|column| *column == 0) {
        return None;
    }
    Some(Expr {
        ty: result.ty,
        kind: ExprKind::Builtin(Builtin::SortedRecordRows { class, columns }, inputs.clone()),
    })
}

/// Prove that the private list receives one projection for every key in order.
fn list_projection(result: &Expr, keys: usize) -> Option<(usize, &Expr)> {
    let ExprKind::Scope { body, value, clear } = &result.kind else {
        return None;
    };
    let [
        Statement::Assign(list, empty),
        Statement::Scope {
            body,
            clear: loop_roots,
        },
    ] = body.as_slice()
    else {
        return None;
    };
    let [
        Statement::Assign(source, input),
        Statement::Assign(cursor, initial),
        Statement::While(test, steps, otherwise),
    ] = body.as_slice()
    else {
        return None;
    };
    let [
        Statement::Assign(item, row),
        Statement::Assign(advance, next),
        Statement::Expr(append),
    ] = steps.as_slice()
    else {
        return None;
    };
    let local = |expr: &Expr, expected: usize| matches!(expr.kind, ExprKind::Local(slot) if slot == expected);
    if !matches!(&empty.kind, ExprKind::List(items) if items.is_empty())
        || !local(value, *list)
        || !local(input, keys)
        || initial.tagged_integer_literal() != Some(0)
        || !test.tests_index_before_len(*source, *cursor)
        || !otherwise.is_empty()
        || advance != cursor
        || next.local_integer_offset() != Some((*cursor, 1))
        || loop_roots != &[*source, *cursor]
        || clear != &[*list, *source, *cursor, *item]
    {
        return None;
    }
    let ExprKind::Builtin(Builtin::SequenceGetInBounds | Builtin::ListGet(None), index) = &row.kind
    else {
        return None;
    };
    let ExprKind::Builtin(Builtin::Append, args) = &append.kind else {
        return None;
    };
    let [output, element] = args.as_slice() else {
        return None;
    };
    (matches!(index.as_slice(), [container, i] if local(container, *source) && local(i, *cursor))
        && local(output, *list))
    .then_some((*item, element))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{frontend, modules::SourceGraph};

    #[test]
    fn only_field_ordered_list_builds_are_fused_after_checking() {
        let graph = SourceGraph::single(
            r#"
from collections import defaultdict
from dataclasses import dataclass, field

@dataclass
class Row:
    first: list[int] = field(kw_only=True)
    second: list[int]
    key: str
    rest: list[int] = field(default_factory=list)

def ordered(a: defaultdict[str, list[int]], b: defaultdict[str, list[int]]) -> list[Row]:
    keys = sorted(a.keys() | b.keys())
    return [Row(first=b[k], second=a[k], key=k, rest=a[k]) for k in keys]

def filtered(a: defaultdict[str, list[int]], b: defaultdict[str, list[int]]) -> list[Row]:
    keys = sorted(a.keys() | b.keys())
    return [Row(first=b[k], second=a[k], key=k, rest=a[k]) for k in keys if k]

def reordered(a: defaultdict[str, list[int]], b: defaultdict[str, list[int]]) -> list[Row]:
    keys = sorted(a.keys() | b.keys())
    return [Row(second=a[k], first=b[k], key=k, rest=a[k]) for k in keys]

def defaulted(a: defaultdict[str, list[int]], b: defaultdict[str, list[int]]) -> list[Row]:
    keys = sorted(a.keys() | b.keys())
    return [Row(first=b[k], second=a[k], key=k) for k in keys]

@dataclass
class Hook:
    value: list[int]
    def __post_init__(self) -> None:
        self.value.append(7)

def hook(a: defaultdict[str, list[int]], b: defaultdict[str, list[int]]) -> list[Hook]:
    keys = sorted(a.keys() | b.keys())
    return [Hook(a[k]) for k in keys]
"#,
        )
        .unwrap();
        let mut module = frontend::lower(&graph).unwrap();
        let columns = |module: &Module| {
            module
                .functions
                .iter()
                .filter_map(|function| {
                    let Statement::Return(Expr {
                        kind: ExprKind::Builtin(Builtin::SortedRecordRows { columns, .. }, _),
                        ..
                    }) = function.body.last()?
                    else {
                        return None;
                    };
                    Some(columns.clone())
                })
                .collect::<Vec<_>>()
        };
        assert!(columns(&module).is_empty());
        specialize(&mut module);
        assert_eq!(columns(&module), [vec![2, 1, 0, 1]]);
    }
}
