//! Bulk operations whose exact string elements cannot invoke user callbacks.

use imre_ast::comprehension::{Output, Plan};

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

/// Both bulk filters implement exactly one synchronous pull guarded by one
/// predicate. Inspect the executable plan before replacing that loop.
fn filtered_item<'a>(plan: &Plan<'a>) -> Option<(&'a str, &'a ast::Expr)> {
    let [clause] = plan.clauses else { return None };
    match (&clause.target, clause.ifs.as_slice(), clause.is_async) {
        (ast::Expr::Name(target), [test], false) => Some((target.id.as_str(), test)),
        _ => None,
    }
}

impl Checker<'_> {
    /// Recognize `{c for c in text if c in keys}`, optionally including `ord(c) < n`.
    pub(super) fn character_set_filter(
        &mut self,
        plan: &Plan<'_>,
        expected: Option<Type>,
    ) -> Result<Option<Expr>, String> {
        let Some((name, condition)) = filtered_item(plan) else {
            return Ok(None);
        };
        let ty = self.types.set(Type::Str);
        if !same_name(plan.output.value(), name)
            || expected.is_some_and(|t| t != ty)
            || !matches!(plan.outer, ast::Expr::Name(_))
        {
            return Ok(None);
        }
        let Some((keys, below)) = self.character_filter_operands(condition, name) else {
            return Ok(None);
        };
        let text = self.expr(plan.outer)?;
        if text.ty != Type::Str {
            return Ok(None);
        }
        let keys = self.expr(keys)?;
        if !matches!(keys.ty, Type::Set(_) | Type::FrozenStrings)
            || self.types.readable_set_element(keys.ty) != Some(Type::Str)
        {
            return Ok(None);
        }
        let source = self.hidden(Type::Str);
        let selected = Expr {
            ty,
            kind: ExprKind::Builtin(
                Builtin::FilterCharacterSet(crate::ir::CharacterFilter {
                    below,
                    ascii_keys: None,
                }),
                vec![self.local(source), keys],
            ),
        };
        // An empty input never evaluates the membership operand.
        let nonempty = self.truth_value(self.local(source));
        Ok(Some(Expr {
            ty,
            kind: ExprKind::Scope {
                body: vec![Statement::Assign(source, text)],
                value: Box::new(Expr {
                    ty,
                    kind: ExprKind::Conditional(
                        Box::new(nonempty),
                        Box::new(selected),
                        Box::new(Expr {
                            ty,
                            kind: ExprKind::Set {
                                element: Type::Str,
                                values: Vec::new(),
                            },
                        }),
                    ),
                }),
                clear: vec![source],
            },
        }))
    }

    /// The set expression must be invariant for this iteration target. An
    /// optional second operand may only call the unshadowed builtin `ord`.
    fn character_filter_operands<'a>(
        &self,
        condition: &'a ast::Expr,
        name: &str,
    ) -> Option<(&'a ast::Expr, u32)> {
        let (membership, below) = if let ast::Expr::BoolOp(or) = condition {
            let [membership, ast::Expr::Compare(bound)] = or.values.as_slice() else {
                return None;
            };
            let [ast::Expr::Call(call), ast::Expr::NumberLiteral(limit)] = bound.operands.as_ref()
            else {
                return None;
            };
            let ast::Number::Int(limit) = &limit.value else {
                return None;
            };
            let [arg] = call.arguments.args.as_slice() else {
                return None;
            };
            if or.op != ast::BoolOp::Or
                || bound.ops.as_ref() != [ast::CmpOp::Lt]
                || !same_name(&call.func, "ord")
                || name == "ord"
                || !same_name(arg, name)
                || !call.arguments.keywords.is_empty()
                || self.names.contains_key("ord")
                || self.local_functions.contains_key("ord")
                || self.resolve(&call.func).is_some()
            {
                return None;
            }
            (membership, limit.to_string().parse().ok()?)
        } else {
            (condition, 0)
        };
        let ast::Expr::Compare(test) = membership else {
            return None;
        };
        let [item, keys @ ast::Expr::Name(_)] = test.operands.as_ref() else {
            return None;
        };
        if test.ops.as_ref() != [ast::CmpOp::In] || !same_name(item, name) || same_name(keys, name)
        {
            return None;
        }
        Some((keys, below))
    }

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
        plan: &Plan<'_>,
        element: Option<Type>,
    ) -> Result<Option<Expr>, String> {
        let Some((name, condition)) = filtered_item(plan) else {
            return Ok(None);
        };
        let Some(strip) = suffix(plan.output.value(), name) else {
            return Ok(None);
        };
        if element.is_some_and(|ty| ty != Type::Str)
            || !matches!(plan.outer, ast::Expr::Name(_) | ast::Expr::Attribute(_))
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
        let list = self.expr(plan.outer)?;
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
                Ok(Expr::absent(optional))
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
        let mut body = vec![Statement::Assign(source, list)];
        let mut clear = vec![source];
        let value = if matches!(plan.output, Output::Generator(_)) {
            let result = self.hidden(ty);
            body.push(self.generator_frame(vec![Statement::Assign(result, value)]));
            clear.push(result);
            self.local(result)
        } else {
            value
        };
        Ok(Some(Expr {
            ty,
            kind: ExprKind::Scope {
                body,
                value: Box::new(value),
                clear,
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

#[cfg(test)]
mod tests {
    use crate::{CompileOptions, compile};

    #[test]
    fn character_set_filter_requires_exact_strings_and_a_pure_predicate() {
        for (element, condition, fused) in [
            ("c", "c in keys", true),
            ("c", "c in keys or ord(c) < 128", true),
            ("c", "c in keys and ord(c) < 128", false),
            ("c", "c in keys or ord(c) <= 128", false),
            ("c", "c in keys or ord(c) < limit", false),
            ("c.upper()", "c in keys", false),
        ] {
            let source = format!(
                "def select(text: str, keys: set[str], limit: int) -> set[str]:\n    return {{{element} for c in text if {condition}}}\n"
            );
            let llvm = compile(&source, &CompileOptions::default()).unwrap();
            assert_eq!(llvm.contains("call ptr @imre_filter_character_set("), fused);
        }

        let source = "def ord(c: str) -> int:\n    print(c)\n    return 0\ndef select(text: str, keys: set[str]) -> set[str]:\n    return {c for c in text if c in keys or ord(c) < 32}\n";
        let llvm = compile(source, &CompileOptions::default()).unwrap();
        assert!(!llvm.contains("call ptr @imre_filter_character_set("));
    }
}
