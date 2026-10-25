//! Bind batched arguments before converting the index, testing strictness, and acquiring the input.

use super::*;

fn argument_exception(kind: &str, message: impl Into<String>) -> Expr {
    Expr {
        ty: Type::Exception,
        kind: ExprKind::Exception {
            kind: exception_kind(kind).unwrap(),
            message: Some(Box::new(unpacking::string(message.into()))),
        },
    }
}

/// Binding errors happen after argument expressions run and are catchable at the call.
fn batched_argument_error(call: &ast::ExprCall) -> Option<String> {
    let args = &call.arguments;
    if args
        .args
        .iter()
        .any(|arg| matches!(arg, ast::Expr::Starred(_)))
        || args.keywords.iter().any(|kw| kw.arg.is_none())
    {
        return None; // The generic call binder validates argument expansions.
    }
    let positional = args.args.len();
    let total = positional + args.keywords.len();
    if total > 3 {
        return Some(format!(
            "batched() takes at most 3 arguments ({total} given)"
        ));
    }
    if positional > 2 {
        return Some(format!(
            "batched() takes exactly 2 positional arguments ({positional} given)"
        ));
    }
    let names = ["iterable", "n", "strict"];
    let mut keywords = args
        .keywords
        .iter()
        .filter_map(|kw| kw.arg.as_ref().map(|name| name.as_str()));
    for (i, name) in names[..2].iter().enumerate().skip(positional) {
        if !keywords.clone().any(|kw| kw == *name) {
            return Some(format!(
                "batched() missing required argument '{name}' (pos {})",
                i + 1
            ));
        }
    }
    for (i, name) in names.iter().enumerate().take(positional) {
        if keywords.clone().any(|kw| kw == *name) {
            return Some(format!(
                "argument for batched() given by name ('{name}') and position ({})",
                i + 1
            ));
        }
    }
    let kw = keywords.find(|kw| !names.contains(kw))?;
    let mut message = format!("batched() got an unexpected keyword argument '{kw}'");
    if let Some(name) = keyword_suggestion(kw, &names) {
        message.push_str(&format!(". Did you mean '{name}'?"));
    }
    Some(message)
}

/// Pick the closest parameter within Python's weighted edit distance. A case
/// change costs one; insertion, deletion, and other substitutions cost two.
fn keyword_suggestion<'a>(keyword: &str, names: &[&'a str]) -> Option<&'a str> {
    names
        .iter()
        .copied()
        .filter_map(|name| {
            let limit = (keyword.len() + name.len() + 3) / 3;
            if 2 * keyword.len().abs_diff(name.len()) > limit {
                return None;
            }
            let mut row: Vec<_> = (0..=name.len()).map(|i| i * 2).collect();
            for (i, a) in keyword.bytes().enumerate() {
                let mut diagonal = row[0];
                row[0] = 2 * (i + 1);
                for (j, b) in name.bytes().enumerate() {
                    let cost = if a == b {
                        0
                    } else if a.eq_ignore_ascii_case(&b) {
                        1
                    } else {
                        2
                    };
                    let distance = (diagonal + cost).min(2 + row[j].min(row[j + 1]));
                    diagonal = std::mem::replace(&mut row[j + 1], distance);
                }
            }
            let distance = row[name.len()];
            (distance <= limit).then_some((distance, name))
        })
        .min_by_key(|(distance, _)| *distance)
        .map(|(_, name)| name)
}

impl Checker<'_> {
    pub(in crate::frontend) fn batched_cursor(
        &mut self,
        call: &ast::ExprCall,
    ) -> Result<Expr, String> {
        // The literal overloads in typeshed do not model catchable constructor
        // errors or the index/truth protocols implemented below.
        self.graph.checked_overload(self.module, call.range());
        if let Some(message) = batched_argument_error(call) {
            let mut args = Vec::new();
            for arg in &call.arguments.args {
                args.push(self.cursor_source_value(arg)?);
            }
            for kw in &call.arguments.keywords {
                args.push(self.cursor_source_value(&kw.value)?);
            }
            args.push(argument_exception("TypeError", message));
            return Ok(Expr {
                ty: self.types.cursor(self.types.var_tuple(Type::Int)),
                kind: ExprKind::Builtin(Builtin::RaiseException, args),
            });
        }
        let params = [
            (
                "iterable",
                Type::None,
                ParameterKind::PositionalOrKeyword,
                None,
            ),
            ("n", Type::Int, ParameterKind::PositionalOrKeyword, None),
            (
                "strict",
                Type::Bool,
                ParameterKind::KeywordOnly,
                Some(Expr {
                    ty: Type::Bool,
                    kind: ExprKind::Bool(false),
                }),
            ),
        ]
        .map(|(name, ty, kind, default)| Parameter {
            name: name.to_owned(),
            ty,
            kind,
            default,
            sign_only: false,
        });
        let mut element = Type::Int;
        let args = self.arguments_with(call, &params, None, |checker, i, node| match i {
            0 => {
                let source = checker.cursor_source_value(node)?;
                element = checker
                    .iterable_element(source.ty)
                    .or_else(|| {
                        matches!(source.ty, Type::None | Type::Int | Type::Bool | Type::Float)
                            .then_some(Type::Int)
                    })
                    .ok_or_else(|| {
                        error(
                            checker.source,
                            node,
                            "batched requires an iterable of known element type",
                        )
                    })?;
                Ok(source)
            }
            _ => checker.expr(node),
        })?;
        let ty = self.types.cursor(self.types.var_tuple(element));
        let order = args.order;
        let mut body = Vec::new();
        let mut slots = Vec::new();
        for value in args.values {
            let slot = self.hidden(value.ty);
            body.push(Statement::Assign(slot, value));
            slots.push(slot);
        }
        // Evaluate all arguments first, including keyword expressions. Argument
        // Clinic converts n to Py_ssize_t before it tests strict for truth.
        let raw = self.local(slots[order[1]]);
        let size = self.hidden(Type::Int);
        let size_node = call
            .arguments
            .args
            .get(1)
            .or_else(|| {
                call.arguments
                    .keywords
                    .iter()
                    .find(|key| key.arg.as_deref() == Some("n"))
                    .map(|key| &key.value)
            })
            .unwrap_or(&call.func);
        let index = self.index_value(raw, size_node)?;
        body.push(Statement::Assign(size, index));
        slots.push(size);
        let fits = Expr {
            ty: Type::Bool,
            kind: ExprKind::Compare(
                vec![
                    pipelines::integer(i64::MIN),
                    self.local(size),
                    pipelines::integer(i64::MAX),
                ],
                vec![Compare::LessEqual, Compare::LessEqual],
            ),
        };
        body.push(Statement::If(
            fits,
            Vec::new(),
            vec![Statement::Raise(Some(argument_exception(
                "OverflowError",
                "Python int too large to convert to C ssize_t",
            )))],
        ));
        let strict = self.truth_value(self.local(slots[order[2]]));
        let arguments = vec![self.local(slots[order[0]]), self.local(size), strict];
        Ok(Expr {
            ty,
            kind: ExprKind::Scope {
                body,
                value: Box::new(Expr {
                    ty,
                    kind: ExprKind::Builtin(Builtin::Batched, arguments),
                }),
                clear: slots,
            },
        })
    }
}
