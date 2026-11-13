//! A directly consumed group shares one live source cursor and keyed lookahead.

use super::*;
use crate::frontend::phase::Result;

/// Recognize an immediate list assignment that consumes the private grouper.
fn materialization<'a>(statement: &'a ast::Stmt, group: &str) -> Option<&'a ast::ExprName> {
    let ast::Stmt::Assign(assignment) = statement else {
        return None;
    };
    let [ast::Expr::Name(name)] = assignment.targets.as_slice() else {
        return None;
    };
    let ast::Expr::Call(call) = &*assignment.value else {
        return None;
    };
    if matches!(&*call.func, ast::Expr::Name(f) if f.id == "list")
        && call.arguments.keywords.is_empty()
        && matches!(call.arguments.args.as_ref(), [ast::Expr::Name(value)] if value.id == group)
    {
        Some(name)
    } else {
        None
    }
}

impl Checker<'_> {
    pub(in crate::frontend) fn groupby_iteration(
        &mut self,
        loop_: &ast::StmtFor,
        call: &ast::ExprCall,
    ) -> Result<(Vec<Statement>, bool), String> {
        let Some([key_target @ ast::Expr::Name(key), ast::Expr::Name(group)]) =
            pattern_elements(&loop_.target)
        else {
            return Err(error(
                self.source,
                loop_.target.as_ref(),
                "groupby requires two distinct name targets",
            ));
        };
        let Some((first, rest)) = loop_.body.split_first() else {
            unreachable!("a for body is nonempty")
        };
        let members = materialization(first, group.id.as_str());
        let nested = match first {
            ast::Stmt::For(inner)
                if !inner.is_async
                    && matches!(inner.iter.as_ref(), ast::Expr::Name(name) if name.id == group.id) =>
            {
                Some(inner)
            }
            _ => None,
        };
        if members.is_none() && nested.is_none() {
            return Err(error(
                self.source,
                first,
                "a groupby loop must first iterate over its group or assign list(group) to a name",
            ));
        }
        if key.id == group.id
            || members.is_some_and(|name| name.id == key.id || name.id == group.id)
        {
            return Err(error(
                self.source,
                first,
                "groupby requires distinct key, group and materialized list names",
            ));
        }

        let args = call.arguments.args.as_ref();
        let (source, callback) = match (args, call.arguments.keywords.as_ref()) {
            ([source], []) => (source, None),
            ([source, callback], []) => (source, Some(callback)),
            ([source], [keyword]) if keyword.arg.as_ref().is_some_and(|arg| arg == "key") => {
                (source, Some(&keyword.value))
            }
            _ => {
                return Err(error(
                    self.source,
                    call,
                    "groupby requires one positional iterable and an optional key",
                ));
            }
        };
        let source = self.expansion_input(source, None)?;
        let mut input = self.prepare_iteration(source, call, false)?;
        // A raw Python grouper keeps its parent cursor alive after break or
        // failure. Elide the cursor only when releasing its owners is unobservable.
        if input
            .clear
            .iter()
            .map(|&slot| self.locals[slot])
            .any(|ty| self.may_own_file(ty, true))
        {
            return Err(error(
                self.source,
                call,
                "groupby does not support source iterators that can own files or dynamic callbacks",
            ));
        }
        let item = input.value;
        let item_type = self.locals[item];
        let (key_value, retained_callback) = if let Some(callback) = callback {
            let callback = self.item_callback(callback, item_type, None, None)?;
            if callback.ty == Type::None {
                input.evaluate.push(Statement::Expr(callback));
                (self.local(item), None)
            } else {
                let retain = match &callback.kind {
                    ExprKind::Callable { receiver, .. } => receiver
                        .as_ref()
                        .is_some_and(|receiver| self.may_own_file(receiver.ty, true)),
                    _ => true, // An indirect callable may own a bound receiver.
                };
                let position = input.evaluate.len();
                let (operation, captured) =
                    self.apply_item_callback(&mut input, callback, position, "groupby", call)?;
                (operation, captured.filter(|_| retain))
            }
        } else {
            (self.local(item), None)
        };
        let key_type = key_value.ty;
        if !self.types.hashable_key(key_type) && !matches!(key_type, Type::Bool | Type::None) {
            return Err(error(
                self.source,
                call,
                "groupby keys require exact immutable scalar equality",
            ));
        }
        if self.names.contains_key(group.id.as_str()) {
            return Err(error(
                self.source,
                group,
                "groupby requires a fresh grouper name",
            ));
        }
        // The grouper remains inaccessible to source expressions. Binding it
        // nevertheless extends the lifetime of a possibly file-owning callback
        // until the next successful binding, even after breaking or raising.
        let grouper_type = retained_callback.map_or(Type::None, |slot| self.locals[slot]);
        let grouper = self.declare(group.id.as_str(), grouper_type, group)?;
        let key = self.declare(key.id.as_str(), key_type, key)?;
        let next_key = self.hidden(key_type);
        let group_key = self.hidden(key_type);
        let have_group = self.hidden(Type::Bool);
        let ready = input.ready;
        let mut pull = std::mem::take(&mut input.next);
        pull.push(Statement::If(
            self.local(ready),
            vec![Statement::Assign(next_key, key_value)],
            Vec::new(),
        ));
        // Both next(groupby) and list(group) interpret StopIteration from a
        // pull as exhaustion. A later call can still advance the same source.
        let mut pull = vec![Statement::Try {
            body: pull,
            handlers: vec![ir::ExceptHandler {
                matcher: crate::ir::ExceptionMatcher::Categories(vec![
                    exception_kind("StopIteration").unwrap(),
                ]),
                name: None,
                body: vec![Statement::Assign(ready, Expr::boolean(false))],
            }],
            orelse: Vec::new(),
            finalbody: Vec::new(),
        }];
        input
            .acquire
            .push(Statement::Assign(ready, Expr::boolean(false)));
        input
            .acquire
            .push(Statement::Assign(have_group, Expr::boolean(false)));
        let boundary = Expr {
            ty: Type::Bool,
            kind: ExprKind::Boolean(
                false,
                vec![
                    not(self.local(ready)),
                    compare(
                        Compare::NotEqual,
                        self.local(group_key),
                        self.local(next_key),
                    ),
                ],
            ),
        };
        // When a group ended without lookahead, its remaining equal keys
        // belong to the previous group and must be skipped by the outer pull.
        let mut seek = pull.clone();
        seek.push(Statement::If(
            Expr {
                ty: Type::Bool,
                kind: ExprKind::Boolean(false, vec![not(self.local(have_group)), boundary.clone()]),
            },
            vec![Statement::Break(None)],
            Vec::new(),
        ));
        input.next = vec![Statement::If(
            not(self.local(ready)),
            vec![Statement::While(Expr::boolean(true), seek, Vec::new())],
            Vec::new(),
        )];
        input.value = next_key;
        input.clear.extend([next_key, group_key, have_group]);
        self.invalidate_loop_facts(&loop_.body, Some((&input, &loop_.iter)))?;
        let writes = input.writes.clone();
        let orelse = &loop_.orelse;
        self.lower_prepared_iteration(key_target, input, orelse, Vec::new(), |checker, body| {
            if let Some(callback) = retained_callback {
                body.push(Statement::Assign(grouper, checker.local(callback)));
            }
            body.extend([
                Statement::Assign(have_group, Expr::boolean(true)),
                Statement::Assign(group_key, checker.local(key)),
            ]);
            if let Some(inner) = nested {
                // The outer pull buffered the first member. Subsequent pulls stop at
                // the boundary, leaving that item and its key for the outer iterator.
                pull.push(Statement::If(
                    boundary,
                    vec![Statement::Exhaust],
                    Vec::new(),
                ));
                let group = Prepared {
                    evaluate: Vec::new(),
                    acquire: Vec::new(),
                    next: vec![Statement::If(not(checker.local(ready)), pull, Vec::new())],
                    value: item,
                    ready,
                    elementless: false,
                    eager_list: None,
                    writes,
                    clear: Vec::new(),
                };
                checker.invalidate_loop_facts(&inner.body, Some((&group, &inner.iter)))?;
                let (statements, _) = checker.lower_prepared_iteration(
                    &inner.target,
                    group,
                    &inner.orelse,
                    Vec::new(),
                    |checker, each| {
                        // A break leaves the outer iterator to skip the rest of this group.
                        each.push(Statement::Assign(ready, Expr::boolean(false)));
                        checker.lower_block(&inner.body, Some(each))?;
                        Ok(())
                    },
                )?;
                body.extend(statements);
            } else {
                let members = members.expect("validated group consumer");
                let list_type = checker.types.list(item_type);
                let accumulating = checker.hidden(list_type);
                let output = checker.declare(members.id.as_str(), list_type, members)?;
                checker.assigned.insert(output);
                checker.narrowed.remove_mut(&output);
                let mut drain = vec![Statement::Expr(Expr {
                    ty: Type::None,
                    kind: ExprKind::Builtin(
                        Builtin::Append,
                        vec![checker.local(accumulating), checker.local(item)],
                    ),
                })];
                drain.append(&mut pull);
                drain.push(Statement::If(
                    boundary,
                    vec![Statement::Break(None)],
                    Vec::new(),
                ));
                body.push(Statement::Scope {
                    body: vec![
                        Statement::Assign(
                            accumulating,
                            Expr {
                                ty: list_type,
                                kind: ExprKind::List(Vec::new()),
                            },
                        ),
                        Statement::While(Expr::boolean(true), drain, Vec::new()),
                        // If a pull raises, the old materialized list is still visible.
                        Statement::Move(output, accumulating),
                    ],
                    clear: vec![accumulating],
                });
            }
            checker.lower_block(rest, Some(body))?;
            Ok(())
        })
    }
}
