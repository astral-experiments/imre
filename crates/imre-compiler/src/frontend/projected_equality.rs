//! Compare sorted, purely projected lists directly at cardinality zero or one.

use super::*;

struct Mapped<'a> {
    target: Option<&'a str>,
    mapper: &'a ast::Expr,
    source: &'a str,
}

fn name(value: &ast::Expr) -> Option<&str> {
    Some(value.as_name_expr()?.id.as_str())
}

fn arguments<'a>(value: &'a ast::Expr, builtin: &str) -> Option<&'a [ast::Expr]> {
    let call = value.as_call_expr()?;
    (name(&call.func) == Some(builtin) && call.arguments.keywords.is_empty())
        .then_some(call.arguments.args.as_ref())
}

fn mapped(statement: &ast::Stmt) -> Option<Mapped<'_>> {
    let assign = statement.as_assign_stmt()?;
    let [target] = assign.targets.as_slice() else {
        return None;
    };
    let mut projection = mapped_value(&assign.value)?;
    projection.target = Some(name(target)?);
    Some(projection)
}

fn mapped_value(value: &ast::Expr) -> Option<Mapped<'_>> {
    let [inner] = arguments(value, "sorted")? else {
        return None;
    };
    let [mapper, source] = arguments(inner, "map")? else {
        return None;
    };
    Some(Mapped {
        target: None,
        mapper,
        source: name(source)?,
    })
}

fn comparison(value: &ast::Expr, left: &str, right: &str) -> Option<ast::CmpOp> {
    let compare = value.as_compare_expr()?;
    let [op @ (ast::CmpOp::Eq | ast::CmpOp::NotEq)] = compare.ops.as_ref() else {
        return None;
    };
    matches!(compare.operands.as_ref(), [l,r] if
        (name(l) == Some(left) && name(r) == Some(right))
        || (name(l) == Some(right) && name(r) == Some(left)))
    .then_some(*op)
}

fn return_bool(stmt: &ast::Stmt) -> Option<bool> {
    let value = stmt.as_return_stmt()?.value.as_deref()?;
    Some(value.as_boolean_literal_expr()?.value)
}

struct Answers {
    equal: bool,
    empty: bool,
    different_lengths: Option<bool>,
}

fn zipped_comparison(
    iter: &ast::Expr,
    target: &ast::Expr,
    predicate: &ast::Expr,
    left: &str,
    right: &str,
) -> Option<ast::CmpOp> {
    let [a, b] = arguments(iter, "zip")? else {
        return None;
    };
    let [x, y] = target.as_tuple_expr()?.elts.as_slice() else {
        return None;
    };
    let (x, y) = (name(x)?, name(y)?);
    if name(a)? != left || name(b)? != right || x == y {
        return None;
    }
    comparison(predicate, x, y)
}

/// The result is terminal, so neither sorted list nor either zip target escapes.
/// Other lengths retain the complete reduction, including zip truncation.
fn tail(stmts: &[ast::Stmt]) -> Option<(usize, Mapped<'_>, Mapped<'_>, Answers)> {
    if let [.., ast::Stmt::Return(ret)] = stmts
        && let Some(compare) = ret.value.as_deref()?.as_compare_expr()
        && let [op @ (ast::CmpOp::Eq | ast::CmpOp::NotEq)] = compare.ops.as_ref()
        && let [left, right] = compare.operands.as_ref()
        && let (Some(a), Some(b)) = (mapped_value(left), mapped_value(right))
    {
        let equal = *op == ast::CmpOp::Eq;
        return Some((
            stmts.len() - 1,
            a,
            b,
            Answers {
                equal,
                empty: equal,
                different_lengths: Some(!equal),
            },
        ));
    }
    if let [.., a, b, ast::Stmt::Return(ret)] = stmts
        && let (Some(a), Some(b), Some(value)) = (mapped(a), mapped(b), ret.value.as_deref())
        && let Some(call) = value.as_call_expr()
        && let Some(reducer @ ("any" | "all")) = name(&call.func)
        && call.arguments.keywords.is_empty()
        && let [ast::Expr::Generator(generator)] = call.arguments.args.as_ref()
        && let [clause] = generator.generators.as_slice()
        && !clause.is_async
        && clause.ifs.is_empty()
        && let Some(op) = zipped_comparison(
            &clause.iter,
            &clause.target,
            &generator.elt,
            a.target?,
            b.target?,
        )
    {
        return Some((
            stmts.len() - 3,
            a,
            b,
            Answers {
                equal: op == ast::CmpOp::Eq,
                empty: reducer == "all",
                different_lengths: None,
            },
        ));
    }
    let [.., a, b, ast::Stmt::For(loop_), result] = stmts else {
        return None;
    };
    let (a, b) = (mapped(a)?, mapped(b)?);
    let [ast::Stmt::If(test)] = loop_.body.as_slice() else {
        return None;
    };
    let [found] = test.body.as_slice() else {
        return None;
    };
    if loop_.is_async || !loop_.orelse.is_empty() || !test.elif_else_clauses.is_empty() {
        return None;
    }
    let op = zipped_comparison(&loop_.iter, &loop_.target, &test.test, a.target?, b.target?)?;
    let (hit, empty) = (return_bool(found)?, return_bool(result)?);
    (hit != empty).then_some((
        stmts.len() - 4,
        a,
        b,
        Answers {
            equal: if op == ast::CmpOp::Eq { hit } else { empty },
            empty,
            different_lengths: None,
        },
    ))
}
struct Input {
    mapper: usize,
    list: Expr,
    item: Type,
    // Each path ends in a scalar, in Python tuple field order.
    fields: Vec<(Type, Vec<(usize, Type)>)>,
    result: Type,
}

impl Checker<'_> {
    /// Validates the complete ordinary implementation even when a fast exit
    /// applies. Earlier statements (including any length checks) still run first.
    pub(super) fn function_body(
        &mut self,
        stmts: &[ast::Stmt],
    ) -> Result<(Vec<Statement>, bool), String> {
        self.collect_local_definitions(stmts)?;
        if self.returns != Type::Bool {
            return self.block(stmts);
        }
        let Some((at, a, b, answers)) = tail(stmts) else {
            return self.block(stmts);
        };
        let (mut body, prefix_falls) = self.block(&stmts[..at])?;
        let reachable = self.reachable;
        self.reachable &= prefix_falls;
        if (a.target.is_none() || a.target != b.target)
            && a.target.is_none_or(|name| !self.names.contains_key(name))
            && b.target.is_none_or(|name| !self.names.contains_key(name))
            && let (Some(a), Some(b)) = (self.projected_input(a), self.projected_input(b))
            && a.result == b.result
            && a.fields.len() == b.fields.len()
        {
            // Reading the scalar fields directly also skips the two Python mapper calls.
            let untracked = Expr {
                ty: Type::Bool,
                kind: ExprKind::Builtin(
                    Builtin::CanElideMapperFrames([a.mapper, b.mapper]),
                    Vec::new(),
                ),
            };
            body.push(Statement::If(
                untracked,
                self.short_projected_comparison(a, b, answers),
                Vec::new(),
            ));
        }
        let (suffix, suffix_falls) = self.block(&stmts[at..])?;
        body.extend(suffix);
        self.reachable = reachable;
        Ok((body, prefix_falls && suffix_falls))
    }

    fn projected_input(&self, mapped: Mapped<'_>) -> Option<Input> {
        let &list = self.names.get(mapped.source)?;
        if !self.assigned.contains(&list) {
            return None;
        }
        let list = self.local(list);
        let item = self.types.element(list.ty)?;
        if self.local_cached_value(mapped.mapper).is_some() {
            return None;
        }
        let (signature, def) = if let Some(local) = self.local_function_binding(mapped.mapper) {
            let function = local.target(self.source, mapped.mapper).ok()?;
            (&function.signature, function.definition.as_ref())
        } else {
            let Binding::Definition(id) = self.resolve(mapped.mapper)? else {
                return None;
            };
            (
                self.functions.get(&id)?,
                self.graph.modules[id.module].parsed.syntax().body[id.statement]
                    .as_function_def_stmt()?,
            )
        };
        let [param] = signature.params.as_slice() else {
            return None;
        };
        let [ast::Stmt::Return(ret)] = def.body.as_slice() else {
            return None;
        };
        if param.ty != item {
            return None;
        }
        let expression = ret.value.as_deref()?;
        let values = match expression {
            ast::Expr::Tuple(t) => t.elts.as_slice(),
            _ => std::slice::from_ref(expression),
        };
        let mut fields = Vec::new();
        for value in values {
            let mut chain = Vec::new();
            let text = arguments(value, "str");
            let mut current = match text {
                Some([arg]) => arg,
                Some(_) => return None,
                None => value,
            };
            while let ast::Expr::Attribute(attr) = current {
                chain.push(attr.attr.as_str());
                current = &attr.value;
            }
            if name(current) != Some(param.name.as_str()) {
                return None;
            }
            let mut ty = item;
            let mut path = Vec::new();
            for attr in chain.into_iter().rev() {
                let Type::Class(id) = ty else { return None };
                let index = self.classes.declarations.definitions[id]
                    .fields
                    .iter()
                    .position(|f| f.name == attr)?;
                ty = self.classes.fields[id][index];
                path.push((index, ty));
            }
            // Exclude projections with callbacks, nullable access, coercions,
            // floats (boxed NaN identity), or structural comparisons.
            if text.is_some() {
                if !matches!(ty, Type::Path | Type::Str) {
                    return None;
                }
                ty = Type::Str;
            } else if !self.types.ordered_key(ty) || matches!(ty, Type::Tuple(_)) {
                return None;
            }
            fields.push((ty, path));
        }
        let field_types = fields.iter().map(|(ty, _)| *ty).collect::<Vec<_>>();
        let result = signature.returns;
        let compatible = match expression {
            ast::Expr::Tuple(_) => self.types.fields(result).as_ref() == Some(&field_types),
            _ => field_types == [result],
        };
        (compatible && !fields.is_empty()).then_some(Input {
            mapper: signature.id,
            list,
            item,
            fields,
            result,
        })
    }

    fn short_projected_comparison(
        &mut self,
        a: Input,
        b: Input,
        answers: Answers,
    ) -> Vec<Statement> {
        let length = |list: &Expr| Expr {
            ty: Type::Int,
            kind: ExprKind::Builtin(Builtin::Len, vec![list.clone()]),
        };
        let mut branches = Vec::new();
        if let Some(result) = answers.different_lengths {
            branches.push(Statement::If(
                pipelines::compare(Compare::NotEqual, length(&a.list), length(&b.list)),
                vec![Statement::Return(Expr {
                    ty: Type::Bool,
                    kind: ExprKind::Bool(result),
                })],
                vec![],
            ));
        }
        let both_len = |n| {
            let checks = [&a, &b]
                .into_iter()
                .map(|v| {
                    let len = length(&v.list);
                    pipelines::compare(Compare::Equal, len, pipelines::integer(n))
                })
                .collect();
            Expr {
                ty: Type::Bool,
                kind: ExprKind::Boolean(true, checks),
            }
        };
        let both_one = both_len(1);
        let both_empty = both_len(0);
        let mut setup = Vec::new();
        let mut clear = Vec::new();
        let mut project = |input: Input| {
            let root = self.hidden(input.item);
            let get = if input.list.ty == Type::ListInt {
                Builtin::ListGet
            } else {
                Builtin::SequenceGetInBounds
            };
            setup.push(Statement::Assign(
                root,
                Expr {
                    ty: input.item,
                    kind: ExprKind::Builtin(get, vec![input.list, pipelines::integer(0)]),
                },
            ));
            clear.push(root);
            input
                .fields
                .into_iter()
                .map(|(_, path)| {
                    path.into_iter()
                        .fold(self.local(root), |expr, (i, ty)| Expr {
                            ty,
                            kind: ExprKind::Field(Box::new(expr), i),
                        })
                })
                .collect::<Vec<_>>()
        };
        let left = project(a);
        let right = project(b);
        let equal = left
            .into_iter()
            .zip(right)
            .map(|(l, r)| {
                // Native POSIX paths compare by the exact text returned by str().
                // Only materialize a projection when the opposite leaf is a str.
                let text = |v: Expr| {
                    if v.ty == Type::Path {
                        paths::operation(ir::PathOperation::Text, Type::Str, vec![v])
                    } else {
                        v
                    }
                };
                if l.ty == r.ty {
                    pipelines::compare(Compare::Equal, l, r)
                } else {
                    pipelines::compare(Compare::Equal, text(l), text(r))
                }
            })
            .collect();
        let mut value = Expr {
            ty: Type::Bool,
            kind: ExprKind::Scope {
                body: setup,
                clear,
                value: Box::new(Expr {
                    ty: Type::Bool,
                    kind: ExprKind::Boolean(true, equal),
                }),
            },
        };
        if !answers.equal {
            value = Expr {
                ty: Type::Bool,
                kind: ExprKind::Not(Box::new(value)),
            };
        }
        branches.extend([
            Statement::If(both_one, vec![Statement::Return(value)], vec![]),
            Statement::If(
                both_empty,
                vec![Statement::Return(Expr {
                    ty: Type::Bool,
                    kind: ExprKind::Bool(answers.empty),
                })],
                vec![],
            ),
        ]);
        branches
    }
}
