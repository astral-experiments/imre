//! Bulk operations whose exact string elements cannot invoke user callbacks.

use super::*;

fn same_name(expr: &ast::Expr, name: &str) -> bool {
    matches!(expr, ast::Expr::Name(variable) if variable.id.as_str() == name)
}

/// The number of leading Unicode code points skipped by `item[n:]`.
fn suffix(expr: &ast::Expr, name: &str) -> Option<u32> {
    if same_name(expr, name) {
        return Some(0);
    }
    let ast::Expr::Subscript(index) = expr else {
        return None;
    };
    let ast::Expr::Slice(slice) = &*index.slice else {
        return None;
    };
    if !same_name(&index.value, name) || slice.step.is_some() || slice.upper.is_some() {
        return None;
    }
    let ast::Expr::NumberLiteral(number) = slice.lower.as_deref()? else {
        return None;
    };
    let ast::Number::Int(n) = &number.value else {
        return None;
    };
    n.to_string().parse().ok()
}

fn prefix_argument<'a>(expr: &'a ast::Expr, name: &str) -> Option<&'a ast::Expr> {
    let ast::Expr::Call(call) = expr else {
        return None;
    };
    let ast::Expr::Attribute(method) = &*call.func else {
        return None;
    };
    let [arg] = call.arguments.args.as_ref() else {
        return None;
    };
    (method.attr.as_str() == "startswith"
        && same_name(&method.value, name)
        && call.arguments.keywords.is_empty())
    .then_some(arg)
}

impl Checker<'_> {
    /// Pure string expressions may depend on fixed scalar locals but never on
    /// the iteration target. Evaluate them at the first item so even overflow
    /// in string repetition follows the empty-input behavior of the comprehension.
    fn invariant_prefix(&self, expr: &ast::Expr, target: &str) -> bool {
        match expr {
            ast::Expr::StringLiteral(_) => true,
            ast::Expr::NumberLiteral(n) => matches!(n.value, ast::Number::Int(_)),
            ast::Expr::Name(name) if name.id.as_str() != target => {
                self.names.get(name.id.as_str()).is_some_and(|&slot| {
                    self.assigned.contains(&slot)
                        && matches!(
                            self.locals[slot],
                            Type::Int | Type::Str | Type::StrLiteral(_)
                        )
                })
            }
            ast::Expr::BinOp(bin) if matches!(bin.op, ast::Operator::Add | ast::Operator::Mult) => {
                self.invariant_prefix(&bin.left, target)
                    && self.invariant_prefix(&bin.right, target)
            }
            _ => false,
        }
    }

    /// Lowers one eager, side-effect-free prefix filter on an exact string list.
    pub(super) fn string_list_filter(
        &mut self,
        clauses: &[ast::Comprehension],
        elt: &ast::Expr,
        element: Option<Type>,
    ) -> Result<Option<Expr>, String> {
        let [clause] = clauses else { return Ok(None) };
        let [condition] = clause.ifs.as_slice() else {
            return Ok(None);
        };
        let ast::Expr::Name(target) = &clause.target else {
            return Ok(None);
        };
        let name = target.id.as_str();
        let Some(strip) = suffix(elt, name) else {
            return Ok(None);
        };
        if clause.is_async
            || element.is_some_and(|ty| ty != Type::Str)
            || !matches!(clause.iter, ast::Expr::Name(_) | ast::Expr::Attribute(_))
        {
            return Ok(None);
        }
        let (include, exclude) = match condition {
            ast::Expr::BoolOp(test) if test.op == ast::BoolOp::And => {
                let [pos, ast::Expr::UnaryOp(neg)] = test.values.as_slice() else {
                    return Ok(None);
                };
                if neg.op != ast::UnaryOp::Not {
                    return Ok(None);
                }
                let Some(exclude @ ast::Expr::StringLiteral(_)) =
                    prefix_argument(&neg.operand, name)
                else {
                    return Ok(None);
                };
                (pos, Some(exclude))
            }
            _ => (condition, None),
        };
        let Some(arg) = prefix_argument(include, name) else {
            return Ok(None);
        };
        let (prefix, alternate) = match arg {
            ast::Expr::Tuple(pair) => {
                let [
                    a @ ast::Expr::StringLiteral(_),
                    b @ ast::Expr::StringLiteral(_),
                ] = pair.elts.as_slice()
                else {
                    return Ok(None);
                };
                (a, Some(b))
            }
            _ if self.invariant_prefix(arg, name) => (arg, None),
            _ => return Ok(None),
        };
        let list = self.expr(&clause.iter)?;
        if self.types.element(list.ty) != Some(Type::Str) {
            return Ok(None);
        }
        let ty = list.ty;
        let source = self.hidden(ty);
        let mut optional = |expr: Option<&ast::Expr>| {
            let optional = self.types.optional(Type::Str);
            if let Some(expr) = expr {
                self.expr_expected(expr, optional)
            } else {
                Ok(Expr {
                    ty: optional,
                    kind: ExprKind::Absent(Box::new(Expr {
                        ty: Type::None,
                        kind: ExprKind::None,
                    })),
                })
            }
        };
        let alt = optional(alternate)?;
        let exclude = optional(exclude)?;
        let prefix_value = self.expr_expected(prefix, Type::Str)?;
        let selected = Expr {
            ty,
            kind: ExprKind::Builtin(
                Builtin::StringListFilter(strip),
                vec![self.local(source), prefix_value, alt, exclude],
            ),
        };
        let value = if matches!(prefix, ast::Expr::StringLiteral(_)) {
            selected
        } else {
            let nonempty = Expr {
                ty: Type::Bool,
                kind: ExprKind::Builtin(Builtin::StringListNonempty, vec![self.local(source)]),
            };
            Expr {
                ty,
                kind: ExprKind::Conditional(
                    Box::new(nonempty),
                    Box::new(selected),
                    Box::new(Expr {
                        ty,
                        kind: ExprKind::List(Vec::new()),
                    }),
                ),
            }
        };
        Ok(Some(Expr {
            ty,
            kind: ExprKind::Scope {
                body: vec![Statement::Assign(source, list)],
                value: Box::new(value),
                clear: vec![source],
            },
        }))
    }

    /// A pure suffix key permits sorting directly over immutable string views.
    /// A delegated list iterator runs its original prelude once at first pull.
    pub(super) fn string_list_sorted_suffix(
        &mut self,
        call: &ast::ExprCall,
        element: Option<Type>,
    ) -> Result<Option<Expr>, String> {
        let [key] = call.arguments.keywords.as_ref() else {
            return Ok(None);
        };
        if key.arg.as_ref().is_none_or(|name| name.as_str() != "key")
            || element.is_some_and(|ty| ty != Type::Str)
            || !matches!(
                &call.arguments.args[0],
                ast::Expr::Name(_) | ast::Expr::Attribute(_)
            )
        {
            return Ok(None);
        }
        let ast::Expr::Lambda(lambda) = &key.value else {
            return Ok(None);
        };
        let Some(params) = &lambda.parameters else {
            return Ok(None);
        };
        if params.vararg.is_some() || params.kwarg.is_some() || !params.kwonlyargs.is_empty() {
            return Ok(None);
        }
        let mut positional = params.iter_non_variadic_params();
        let Some(arg) = positional.next() else {
            return Ok(None);
        };
        if arg.default.is_some() || positional.next().is_some() {
            return Ok(None);
        }
        let Some(skip) = suffix(&lambda.body, arg.name().as_str()) else {
            return Ok(None);
        };
        let mut source = self.expr(&call.arguments.args[0])?;
        if let Type::Class(class) = source.ty {
            if !self.classes.has_record_iterator(class) {
                return Ok(None);
            }
            source = self.record_primitive(source, "__iter__", &[], call)?;
        }
        if self.types.element(source.ty) != Some(Type::Str) {
            return Ok(None);
        }
        Ok(Some(Expr {
            ty: source.ty,
            kind: ExprKind::Builtin(Builtin::StringListSortSuffix(skip), vec![source]),
        }))
    }
}
