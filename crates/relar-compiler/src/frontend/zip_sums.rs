//! Fuses adjacent sums over the same strict zip after their scalar callees have been checked.

use super::pipelines::integer;
use super::*;

mod terms;

pub(super) struct Sum {
    pub lists: [usize; 2],
    pub items: [usize; 2],
    pub term: Expr,
    pub begin: Vec<Statement>,
    pub next: Vec<Statement>,
    pub clear: Vec<usize>,
}

/// Both inputs are stable rooted lists. The generator binds two fresh locals and pulls every row.
pub(super) fn lists<'a>(
    checker: &Checker<'_>,
    generator: &'a ast::ExprGenerator,
) -> Option<([usize; 2], [&'a str; 2])> {
    let [clause] = generator.generators.as_slice() else {
        return None;
    };
    let ast::Expr::Call(zip) = &clause.iter else {
        return None;
    };
    let [strict] = zip.arguments.keywords.as_slice() else {
        return None;
    };
    let [ast::Expr::Name(left), ast::Expr::Name(right)] = zip.arguments.args.as_slice() else {
        return None;
    };
    let [ast::Expr::Name(a), ast::Expr::Name(b)] = pattern_elements(&clause.target)? else {
        return None;
    };
    if !clause.ifs.is_empty()
        || a.id == b.id
        || !matches!(&*zip.func, ast::Expr::Name(name) if name.id == "zip")
        || checker.resolve(&zip.func).is_some()
        || checker.names.contains_key("zip")
        || checker.local_functions.contains_key("zip")
        || strict.arg.as_deref() != Some("strict")
        || !matches!(&strict.value, ast::Expr::BooleanLiteral(value) if value.value)
    {
        return None;
    }
    let lists = [
        *checker.names.get(left.id.as_str())?,
        *checker.names.get(right.id.as_str())?,
    ];
    lists
        .iter()
        .all(|&slot| matches!(checker.locals[slot], Type::ListInt | Type::List(_)))
        .then_some((lists, [a.id.as_str(), b.id.as_str()]))
}

pub(super) fn fuse(functions: &mut [ir::Function], id: usize, sums: &HashMap<usize, Sum>) {
    let mut body = std::mem::take(&mut functions[id].body);
    let mut locals = std::mem::take(&mut functions[id].locals);
    fuse_block(&mut body, &mut locals, functions, sums);
    functions[id].body = body;
    functions[id].locals = locals;
}

fn fuse_block(
    body: &mut Vec<Statement>,
    locals: &mut Vec<Type>,
    functions: &[ir::Function],
    sums: &HashMap<usize, Sum>,
) {
    let mut index = 0;
    while index < body.len() {
        if let Some((count, fused)) = group(&body[index..], locals, functions, sums) {
            body.splice(index..index + count, [fused]);
        } else {
            let mut descend =
                |block: &mut Vec<Statement>| fuse_block(block, locals, functions, sums);
            match &mut body[index] {
                Statement::Scope { body, .. } => descend(body),
                Statement::If(_, body, orelse)
                | Statement::While(_, body, orelse)
                | Statement::For { body, orelse, .. } => {
                    descend(body);
                    descend(orelse);
                }
                Statement::Try {
                    body,
                    handlers,
                    orelse,
                    finalbody,
                } => {
                    descend(body);
                    for handler in handlers {
                        descend(&mut handler.body);
                    }
                    descend(orelse);
                    descend(finalbody);
                }
                _ => {}
            }
        }
        index += 1;
    }
}

/// Match only a direct assignment of the original sum. Its result local is unique to that fold.
fn assignment<'a>(
    stmt: &Statement,
    sums: &'a HashMap<usize, Sum>,
) -> Option<(usize, usize, &'a Sum)> {
    let Statement::Assign(
        target,
        Expr {
            kind: ExprKind::Scope { value, .. },
            ..
        },
    ) = stmt
    else {
        return None;
    };
    let ExprKind::Local(result) = value.kind else {
        return None;
    };
    Some((*target, result, sums.get(&result)?))
}

fn group(
    statements: &[Statement],
    locals: &mut Vec<Type>,
    functions: &[ir::Function],
    sums: &HashMap<usize, Sum>,
) -> Option<(usize, Statement)> {
    let (_, _, first) = assignment(statements.first()?, sums)?;
    if assignment(statements.get(1)?, sums)?.2.lists != first.lists {
        return None;
    }
    let start = locals.len();
    let mut row = terms::Row::new(first.items);
    let mut terms = Vec::new();
    let mut outputs = Vec::new();
    for statement in statements {
        let Some((target, result, sum)) = assignment(statement, sums) else {
            break;
        };
        if sum.lists != first.lists {
            break;
        }
        let Some(term) = row.project(&sum.term, sum.items, functions, locals, &mut terms) else {
            break;
        };
        terms.push(Statement::Assign(
            result,
            Expr {
                ty: Type::Int,
                kind: ExprKind::Binary(
                    Binary::Add,
                    Box::new(Expr {
                        ty: Type::Int,
                        kind: ExprKind::Local(result),
                    }),
                    Box::new(term),
                ),
            },
        ));
        outputs.push((target, result));
    }
    if outputs.len() < 2 {
        locals.truncate(start);
        return None;
    }
    let mut body = first.begin.clone();
    let mut clear = first.clear.clone();
    clear.extend(start..locals.len());
    for &(_, result) in &outputs {
        body.push(Statement::Assign(result, integer(0)));
        clear.push(result);
    }
    let mut next = first.next.clone();
    next.extend(terms);
    body.push(Statement::While(
        Expr {
            ty: Type::Bool,
            kind: ExprKind::Bool(true),
        },
        next,
        Vec::new(),
    ));
    // Every term is pure and total. A strict-length error therefore belongs to the first
    // sum, before any user-visible assignment. Commit the private totals only on exhaustion.
    let count = outputs.len();
    body.extend(
        outputs
            .into_iter()
            .map(|(target, result)| Statement::Move(target, result)),
    );
    Some((count, Statement::Scope { body, clear }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn three_sums_share_one_zip_and_one_difference() {
        let source = r#"
def distance(a: int, b: int) -> int:
    d = b - a
    return -d if d < 0 else d

def square(a: int, b: int) -> int:
    d = b - a
    return d * d

def total(left: list[int], right: list[int]) -> int:
    signed = sum(b - a for a, b in zip(left, right, strict=True))
    absolute = sum(distance(b=y, a=x) for x, y in zip(left, right, strict=True))
    squared = sum(square(x, y) for x, y in zip(left, right, strict=True))
    return signed + absolute + squared
"#;
        let module = super::super::lower(&SourceGraph::single(source).unwrap()).unwrap();
        let [Statement::Scope { body, .. }, Statement::Return(_)] =
            module.functions[2].body.as_slice()
        else {
            panic!("expected one fused sum scope")
        };
        assert_eq!(
            body.iter()
                .filter(|stmt| matches!(stmt, Statement::While(..)))
                .count(),
            1
        );
        let mut differences = 0;
        ir::walk_expressions(body, &mut |expr| {
            assert!(!matches!(expr.kind, ExprKind::Call(..)));
            differences += usize::from(matches!(expr.kind, ExprKind::Binary(Binary::Subtract, ..)));
        });
        assert_eq!(differences, 1);
    }
}
