//! Bind public fnmatch calls in evaluation order while preserving str and bytes owners.
use super::*;
use crate::ir::FnmatchOperation;

impl Checker<'_> {
    pub(super) fn fnmatch_call(
        &mut self,
        op: FnmatchOperation,
        call: &ast::ExprCall,
    ) -> Result<Expr, String> {
        let names: &[&str] = match op {
            FnmatchOperation::Match { .. } => &["name", "pat"],
            FnmatchOperation::Filter { .. } => &["names", "pat"],
            FnmatchOperation::Translate => &["pat"],
            FnmatchOperation::Parts => &["pat", "star", "question_mark"],
        };
        let params: Vec<_> = names
            .iter()
            .map(|name| Parameter {
                name: (*name).into(),
                ty: Type::Str,
                kind: ParameterKind::PositionalOrKeyword,
                default: None,
                sign_only: false,
            })
            .collect();
        let mut input = None;
        let mut args = self.arguments_with(call, &params, None, |checker, i, source| {
            if i == 0 && matches!(op, FnmatchOperation::Filter { .. }) {
                input = Some(checker.fnmatch_input(source)?);
                return Ok(Expr {
                    ty: Type::None,
                    kind: ExprKind::None,
                });
            }
            let value = checker.expr(source)?;
            if matches!(
                op,
                FnmatchOperation::Match { normcase: true } | FnmatchOperation::Filter { .. }
            ) && matches!(value.ty, Type::Path | Type::PurePath(_))
            {
                return checker.fspath_value(value, source);
            }
            if op == FnmatchOperation::Parts {
                checker.expect(Type::Str, value.ty, source)?;
            } else if !matches!(value.ty, Type::Str | Type::Bytes) {
                return Err(error(
                    checker.source,
                    source,
                    "fnmatch requires str or bytes",
                ));
            }
            Ok(value)
        })?;
        if let FnmatchOperation::Filter { inverse } = op {
            let input = input.expect("the names argument was bound");
            if let Iteration::Container {
                iterable,
                items: false,
                start: None,
            } = &input
                && matches!(iterable.ty, Type::List(_))
                && let Some(element) = self.types.element(iterable.ty)
                && (matches!(element, Type::Str | Type::Bytes)
                    || self
                        .types
                        .members(element)
                        .is_some_and(|m| m == [Type::Str, Type::Bytes]))
            {
                args.values[args.order[0]] = iterable.clone();
                return Ok(self.ordered_builtin(Builtin::Fnmatch(op), iterable.ty, args));
            }
            let pattern = args.values[args.order[1]].clone();
            return self.fnmatch_items(input, pattern, args.order[0] == 0, inverse, call);
        }
        let returns = match op {
            FnmatchOperation::Match { .. } => Type::Bool,
            FnmatchOperation::Filter { .. } => unreachable!(),
            FnmatchOperation::Translate => Type::Str,
            FnmatchOperation::Parts => self
                .types
                .tuple(vec![self.types.list(Type::Str), Type::ListInt]),
        };
        Ok(self.ordered_builtin(Builtin::Fnmatch(op), returns, args))
    }

    fn fnmatch_input(&mut self, source: &ast::Expr) -> Result<Iteration, String> {
        if let ast::Expr::List(list) = source
            && !list.elts.iter().any(|e| matches!(e, ast::Expr::Starred(_)))
        {
            let values = list
                .elts
                .iter()
                .map(|n| self.expr(n))
                .collect::<Result<Vec<_>, _>>()?;
            if values
                .iter()
                .all(|v| matches!(v.ty, Type::Str | Type::Bytes))
            {
                let element = match values.first() {
                    None => Type::Str,
                    Some(first) if values.iter().all(|v| v.ty == first.ty) => first.ty,
                    _ => self
                        .types
                        .union(&[Type::Str, Type::Bytes])
                        .expect("closed text/bytes union"),
                };
                let values = values
                    .into_iter()
                    .zip(&list.elts)
                    .map(|(v, n)| self.coerce(v, element, n))
                    .collect::<Result<_, _>>()?;
                return Ok(Iteration::Container {
                    iterable: Expr {
                        ty: self.types.list(element),
                        kind: ExprKind::List(values),
                    },
                    items: false,
                    start: None,
                });
            }
        }
        self.expansion_input(source, None)
    }

    /// A generator or cursor is consumed in order: a mixed-type error must stop
    /// before requesting another item. Keep the pattern and source arguments in
    /// their original evaluation order, including when keywords are reversed.
    fn fnmatch_items(
        &mut self,
        input: Iteration,
        pattern: Expr,
        names_first: bool,
        inverse: bool,
        call: &ast::ExprCall,
    ) -> Result<Expr, String> {
        let elem = pattern.ty;
        let ty = self.types.list(elem);
        let (pat, result) = (self.hidden(elem), self.hidden(ty));
        let mut body = Vec::new();
        let mut clear = vec![pat, result];
        let assignment = Statement::Assign(pat, pattern);
        let input = if names_first {
            let input = self.capture_consumer_input(input, &mut body, &mut clear);
            body.push(assignment);
            input
        } else {
            body.push(assignment);
            self.capture_consumer_input(input, &mut body, &mut clear)
        };
        body.push(Statement::Assign(
            result,
            Expr {
                ty,
                kind: ExprKind::List(Vec::new()),
            },
        ));
        let match_name = |checker: &mut Self, value: Expr| {
            if !matches!(value.ty, Type::Str | Type::Bytes) {
                return Err(error(
                    checker.source,
                    call,
                    "fnmatch requires an iterable of str or bytes",
                ));
            }
            let matched = Expr {
                ty: Type::Bool,
                kind: ExprKind::Builtin(
                    Builtin::Fnmatch(FnmatchOperation::Match { normcase: false }),
                    vec![value.clone(), checker.local(pat)],
                ),
            };
            if value.ty != elem {
                return Ok(vec![Statement::Expr(matched)]);
            }
            let append = checker.expansion_insert(result, &mut Some(elem), value, false, call)?;
            let (yes, no) = if inverse {
                (Vec::new(), append)
            } else {
                (append, Vec::new())
            };
            Ok(vec![Statement::If(matched, yes, no)])
        };
        body.extend(self.expand_items(input, call, |checker, value| {
            if checker
                .types
                .members(value.ty)
                .is_some_and(|m| m == [Type::Str, Type::Bytes])
            {
                let (slot, test, text, bytes) = checker.protocol_alternatives(value.ty).unwrap();
                let yes = match_name(checker, text)?;
                let no = match_name(checker, bytes)?;
                return Ok(vec![Statement::Scope {
                    body: vec![Statement::Assign(slot, value), Statement::If(test, yes, no)],
                    clear: vec![slot],
                }]);
            }
            match_name(checker, value)
        })?);
        Ok(Expr {
            ty,
            kind: ExprKind::Scope {
                body,
                value: Box::new(self.local(result)),
                clear,
            },
        })
    }
}
