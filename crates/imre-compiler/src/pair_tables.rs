//! Pack append-only tables of integer pairs after checking every use of the
//! table and row types. Ordinary list and tuple identity remains observable in
//! the general lowering. An unsupported use abandons the whole transformation.
use crate::ir::{
    self, Builtin, ChildMut, Expr, ExprKind, Module, PairTableOp as Op, Statement, Type,
};

pub(crate) fn specialize(module: &mut Module) {
    let functions = || module.functions.iter().chain([&module.entry]);
    let Some((table, row)) = functions().flat_map(|f| &f.locals).find_map(|&table| {
        let row = module.types.element(table)?;
        let pair = module.types.element(row)?;
        (module.types.fields(pair)?.as_slice() == [Type::Int, Type::Int]).then_some((table, row))
    }) else {
        return;
    };
    let relevant = |ty| ty == table || ty == row;
    if module
        .globals
        .iter()
        .chain(module.class_fields.iter().flatten())
        .copied()
        .any(relevant)
    {
        return;
    }
    let signatures: Vec<_> = module
        .functions
        .iter()
        .map(|f| f.params.iter().any(|&s| relevant(f.locals[s])) || relevant(f.returns))
        .collect();
    let mut changes = Vec::new();
    let mut builds = 0;
    for (id, f) in functions().enumerate() {
        let mut touched = f.locals.iter().copied().any(relevant) || relevant(f.returns);
        let mut indirect = false;
        let mut readonly = true;
        ir::walk_expressions(&f.body, &mut |expr| {
            touched |= relevant(expr.ty);
            match &expr.kind {
                ExprKind::Builtin(Builtin::Append, args) if relevant(args[0].ty) => {
                    readonly = false
                }
                ExprKind::Call(_, args) if args.values.iter().any(|arg| relevant(arg.ty)) => {
                    readonly = false
                }
                ExprKind::Invoke { .. } => readonly = false,
                _ => {}
            }
            match expr.kind {
                ExprKind::Callable { function, .. }
                | ExprKind::Builtin(Builtin::StaticFunction(function), _) => {
                    indirect |= signatures[function];
                }
                _ => {}
            }
        });
        if indirect || f.returns == row || f.params.iter().any(|&s| f.locals[s] == row) {
            return;
        }
        if !touched {
            continue;
        }
        let mut f = f.clone();
        let mut pass = Rewrite {
            table,
            row,
            readonly,
            owners: vec![None; f.locals.len()],
            locals: &mut f.locals,
            builds: 0,
        };
        for slot in 0..pass.owners.len() {
            if pass.locals[slot] == row {
                pass.owners[slot] = Some(pass.temp(table));
                pass.locals[slot] = Type::Int;
            }
        }
        if pass.suite(&mut f.body, f.returns).is_none() {
            return;
        }
        builds += pass.builds;
        changes.push((id, f));
    }
    if builds == 0 {
        return;
    }
    for (id, f) in changes {
        if id == module.functions.len() {
            module.entry = f;
        } else {
            module.functions[id] = f;
        }
    }
}

struct Rewrite<'a> {
    // The table cannot escape through a global or indirect call. An index may
    // borrow its row buffer only if this function cannot mutate or pass it on.
    readonly: bool,
    table: Type,
    row: Type,
    owners: Vec<Option<usize>>,
    locals: &'a mut Vec<Type>,
    builds: usize,
}

fn call(op: Op, args: Vec<Expr>, ty: Type) -> Expr {
    Expr {
        ty,
        kind: ExprKind::Builtin(Builtin::PairTable(op), args),
    }
}

fn row_at(value: &Expr, table: Type) -> Option<(&Expr, &Expr)> {
    let ExprKind::Builtin(Builtin::ListGet | Builtin::SequenceGetInBounds, args) = &value.kind
    else {
        return None;
    };
    let [receiver, at] = args.as_slice() else {
        return None;
    };
    (receiver.ty == table && at.ty == Type::Int).then_some((receiver, at))
}

fn local(value: &Expr) -> Option<usize> {
    match value.kind {
        ExprKind::Local(s) => Some(s),
        _ => None,
    }
}

impl Rewrite<'_> {
    fn temp(&mut self, ty: Type) -> usize {
        let slot = self.locals.len();
        self.locals.push(ty);
        slot
    }

    fn owner(&self, slot: usize) -> Option<usize> {
        self.owners.get(slot).copied().flatten()
    }

    fn clear(&self, slots: &mut Vec<usize>) {
        slots.extend(
            slots
                .iter()
                .filter_map(|&s| self.owner(s))
                .collect::<Vec<_>>(),
        );
    }

    fn suite(&mut self, body: &mut Vec<Statement>, returns: Type) -> Option<()> {
        let mut i = 0;
        while i < body.len() {
            if let Some(replacement) = self.unpack(&body[i..]) {
                body.splice(i..i + 4, replacement);
                i += 3;
                continue;
            }
            let statement = &mut body[i];
            match statement {
                Statement::Assign(slot, value) if self.owner(*slot).is_some() => {
                    let (receiver, at) = row_at(value, self.table)?;
                    let (mut receiver, mut at) = (receiver.clone(), at.clone());
                    self.expression(&mut receiver, true, returns)?;
                    self.expression(&mut at, false, returns)?;
                    let temp = self.temp(self.table);
                    let owner = self.owner(*slot)?;
                    // Keep the original receiver through index evaluation and commit both
                    // parts of the row binding only after the bounds check succeeds.
                    *statement = Statement::Scope {
                        body: vec![
                            Statement::Assign(temp, receiver),
                            Statement::Assign(
                                *slot,
                                call(
                                    if self.readonly {
                                        Op::Snapshot(*slot)
                                    } else {
                                        Op::Index
                                    },
                                    vec![Expr::local(temp, self.table), at],
                                    Type::Int,
                                ),
                            ),
                            Statement::Move(owner, temp),
                        ],
                        clear: vec![temp],
                    };
                }
                Statement::Assign(slot, value) => {
                    self.expression(value, self.locals[*slot] == self.table, returns)?
                }
                Statement::Return(value) => {
                    self.expression(value, returns == self.table, returns)?
                }
                Statement::Clear(slot) if self.owner(*slot).is_some() => {
                    *statement = Statement::Scope {
                        body: Vec::new(),
                        clear: vec![*slot, self.owner(*slot)?],
                    };
                }
                Statement::Move(dst, src)
                    if self.owner(*dst).is_some() || self.owner(*src).is_some() =>
                {
                    return None;
                }
                Statement::IterNext {
                    iterator,
                    key,
                    value,
                    ready,
                    ..
                } if [Some(*iterator), Some(*key), *value, Some(*ready)]
                    .into_iter()
                    .flatten()
                    .any(|s| self.owner(s).is_some() || self.locals[s] == self.table) =>
                {
                    return None;
                }
                _ => {
                    if let Statement::Scope { clear, .. } = statement {
                        self.clear(clear);
                    }
                    let mut ok = Some(());
                    ir::statement_children_mut(statement, |child| {
                        if ok.is_some() {
                            ok = match child {
                                ChildMut::Expression(v) => self.expression(v, false, returns),
                                ChildMut::Suite(b) => self.suite(b, returns),
                            }
                        }
                    });
                    ok?;
                }
            }
            i += 1;
        }
        Some(())
    }

    /// Only remove the compiler's immediate complete tuple unpack. No tuple can
    /// be stored, compared by identity, or passed to another function here.
    fn unpack(&self, body: &[Statement]) -> Option<[Statement; 3]> {
        let [
            Statement::Assign(tmp, element),
            Statement::Assign(a, first),
            Statement::Assign(b, second),
            Statement::Clear(done),
            ..,
        ] = body
        else {
            return None;
        };
        let ExprKind::Builtin(Builtin::SequenceGetInBounds, args) = &element.kind else {
            return None;
        };
        let [row, at] = args.as_slice() else {
            return None;
        };
        let r = local(row)?;
        let owner = self.owner(r)?;
        let cursor = local(at)?;
        if done != tmp || cursor == *a || cursor == *b || row.ty != self.row || at.ty != Type::Int {
            return None;
        }
        let field = |e: &Expr, i| matches!(&e.kind, ExprKind::Field(record,n) if *n == i && local(record) == Some(*tmp));
        if !field(first, 0) || !field(second, 1) {
            return None;
        }
        let get = |i| {
            call(
                if self.readonly {
                    Op::ViewField(r, i)
                } else {
                    Op::Field(i)
                },
                vec![
                    Expr::local(owner, self.table),
                    Expr::local(r, Type::Int),
                    at.clone(),
                ],
                Type::Int,
            )
        };
        Some([
            Statement::Assign(*a, get(0)),
            Statement::Assign(*b, get(1)),
            Statement::Clear(*tmp),
        ])
    }

    fn expression(&mut self, value: &mut Expr, allow_table: bool, returns: Type) -> Option<()> {
        if value.ty == self.row || (value.ty == self.table && !allow_table) {
            return None;
        }
        match &mut value.kind {
            ExprKind::Local(_) if value.ty == self.table => return Some(()),
            ExprKind::List(items) if value.ty == self.table && items.is_empty() => {
                self.builds += 1;
                *value = call(Op::New, Vec::new(), self.table);
                return Some(());
            }
            ExprKind::Call(_, args) => {
                // This is a direct call to checked IR in the same module. Both
                // caller and callee must pass this proof before either is changed.
                for arg in &mut args.values {
                    self.expression(arg, arg.ty == self.table, returns)?;
                }
                return Some(());
            }
            ExprKind::Scope {
                body,
                value: result,
                clear,
            } => {
                self.clear(clear);
                self.suite(body, returns)?;
                return self.expression(result, allow_table, returns);
            }
            ExprKind::Builtin(Builtin::Append, args) => {
                let [receiver, item] = args.as_mut_slice() else {
                    return None;
                };
                if receiver.ty == self.table {
                    if !matches!(&item.kind, ExprKind::List(v) if v.is_empty() && item.ty == self.row)
                    {
                        return None;
                    }
                    self.expression(receiver, true, returns)?;
                    *value = call(Op::AddRow, vec![receiver.clone()], Type::None);
                    return Some(());
                }
                if receiver.ty == self.row {
                    let ExprKind::Tuple(fields) = &mut item.kind else {
                        return None;
                    };
                    let [x, y] = fields.as_mut_slice() else {
                        return None;
                    };
                    let (source, index) = row_at(receiver, self.table)?;
                    let (mut source, mut index) = (source.clone(), index.clone());
                    self.expression(&mut source, true, returns)?;
                    self.expression(&mut index, false, returns)?;
                    self.expression(x, false, returns)?;
                    self.expression(y, false, returns)?;
                    let direct = local(&source).is_some_and(|slot| {
                        [&index, &*x, &*y]
                            .into_iter()
                            .all(|arg| ir::expression_preserves_local(arg, slot))
                    });
                    let temp = (!direct).then(|| self.temp(self.table));
                    let table = temp.map_or_else(|| source.clone(), |s| Expr::local(s, self.table));
                    let at = call(Op::Index, vec![table.clone(), index], Type::Int);
                    let push = call(Op::Push, vec![table, at, x.clone(), y.clone()], Type::None);
                    *value = if let Some(temp) = temp {
                        Expr {
                            ty: Type::None,
                            kind: ExprKind::Scope {
                                body: vec![Statement::Assign(temp, source)],
                                value: Box::new(push),
                                clear: vec![temp],
                            },
                        }
                    } else {
                        push
                    };
                    return Some(());
                }
            }
            ExprKind::Builtin(Builtin::Len, args) => {
                let [source] = args.as_mut_slice() else {
                    return None;
                };
                if source.ty == self.table {
                    self.expression(source, true, returns)?;
                    *value = call(Op::Len, vec![source.clone()], Type::Int);
                    return Some(());
                }
                if source.ty == self.row {
                    let r = local(source)?;
                    *value = call(
                        if self.readonly {
                            Op::ViewLen(r)
                        } else {
                            Op::RowLen
                        },
                        vec![
                            Expr::local(self.owner(r)?, self.table),
                            Expr::local(r, Type::Int),
                        ],
                        Type::Int,
                    );
                    return Some(());
                }
            }
            ExprKind::Bind(slot, _)
                if self.owner(*slot).is_some() || self.locals[*slot] == self.table =>
            {
                return None;
            }
            _ => {}
        }
        if value.ty == self.table {
            return None;
        }
        let mut ok = Some(());
        ir::children_mut(value, |child| {
            if ok.is_some() {
                ok = self.expression(child, false, returns);
            }
        });
        ok
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn escaping_rows_keep_normal_identity_and_readers_use_a_stable_view() {
        let compile =
            |source: &str| crate::compile(source, &crate::CompileOptions::default()).unwrap();
        let packed = compile(include_str!(
            "../../../tests/fixtures/containers/packed_pair_table.py"
        ));
        assert!(packed.contains("call ptr @imre_pair_table_new("));
        assert!(packed.contains("call i64 @imre_pair_table_view("));
        let escaped = compile(include_str!(
            "../../../tests/fixtures/containers/pair_table_identity.py"
        ));
        assert!(!escaped.contains("call ptr @imre_pair_table_new("));
    }
}
