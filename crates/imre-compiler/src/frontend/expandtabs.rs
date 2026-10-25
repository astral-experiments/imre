//! Bind str.expandtabs and convert its tab width through Python's index protocol.

use super::*;

fn type_error_after(ty: Type, mut values: Vec<Expr>, message: String) -> Expr {
    values.push(Expr {
        ty: Type::Exception,
        kind: ExprKind::Exception {
            kind: exception_kind("TypeError").unwrap(),
            message: Some(Box::new(unpacking::string(message))),
        },
    });
    Expr {
        ty,
        kind: ExprKind::Builtin(Builtin::RaiseException, values),
    }
}

impl Checker<'_> {
    pub(super) fn expandtabs(
        &mut self,
        receiver: Expr,
        call: &ast::ExprCall,
    ) -> Result<Expr, String> {
        calls::reject_unpacking(call, self.source)?;
        let args = &call.arguments;
        let count = args.args.len() + args.keywords.len();
        if count != 0 {
            self.graph.checked_overload(self.module, call.range());
        }
        let flag = match (args.args.as_ref(), args.keywords.as_ref()) {
            ([], []) => Expr {
                ty: Type::Int,
                kind: ExprKind::Int("8".into()),
            },
            ([arg], []) => {
                let value = self.expr(arg)?;
                self.index_word(value, arg, false)?
            }
            ([], [kw]) if kw.arg.as_deref() == Some("tabsize") => {
                let value = self.expr(&kw.value)?;
                self.index_word(value, &kw.value, false)?
            }
            _ => {
                // Signature errors precede conversion, after all source operands ran.
                let mut values = vec![receiver];
                for node in &args.args {
                    values.push(self.expr(node)?);
                }
                for kw in &args.keywords {
                    values.push(self.expr(&kw.value)?);
                }
                let message = if count > 1 {
                    let qualifier = if args.args.is_empty() { "keyword " } else { "" };
                    format!("expandtabs() takes at most 1 {qualifier}argument ({count} given)")
                } else {
                    format!(
                        "expandtabs() got an unexpected keyword argument '{}'",
                        args.keywords[0].arg.as_deref().unwrap()
                    )
                };
                return Ok(type_error_after(Type::Str, values, message));
            }
        };
        Ok(Expr {
            ty: Type::Str,
            kind: ExprKind::Builtin(
                Builtin::Text(ir::TextOperation::ExpandTabs),
                vec![receiver, flag],
            ),
        })
    }

    /// Convert an integer or a concrete __index__ result exactly once. Python
    /// does not recursively index objects returned by the method.
    pub(super) fn index_word(
        &mut self,
        value: Expr,
        at: &ast::Expr,
        returned: bool,
    ) -> Result<Expr, String> {
        if value.ty == Type::Int {
            return Ok(value);
        }
        if value.ty == Type::Bool {
            if returned {
                return Err(error(
                    self.source,
                    at,
                    "__index__ returning bool requires Python's deprecation warning",
                ));
            }
            return Ok(Expr {
                ty: Type::Int,
                kind: ExprKind::Builtin(Builtin::Int, vec![value]),
            });
        }
        if let Some((slot, test, first, second)) = self.protocol_alternatives(value.ty) {
            let yes = self.index_word(first, at, returned)?;
            let no = self.index_word(second, at, returned)?;
            return Ok(Expr {
                ty: Type::Int,
                kind: ExprKind::Scope {
                    body: vec![Statement::Assign(slot, value)],
                    value: Box::new(Expr {
                        ty: Type::Int,
                        kind: ExprKind::Conditional(Box::new(test), Box::new(yes), Box::new(no)),
                    }),
                    clear: vec![slot],
                },
            });
        }
        if !returned && let Type::Class(id) = value.ty {
            if self.classes.has_concrete_subclasses(id) {
                return self.dispatch_record(id, value, at, |checker, class, receiver| {
                    checker.exact_index(class, receiver, at)
                });
            }
            return self.exact_index(id, value, at);
        }
        self.index_type_error(value, at, returned)
    }

    fn exact_index(&mut self, id: usize, receiver: Expr, at: &ast::Expr) -> Result<Expr, String> {
        let Some(method) = self.classes.methods[id].get("__index__") else {
            return self.index_type_error(receiver, at, false);
        };
        self.calls.insert(method.id);
        let result = Expr {
            ty: method.returns,
            kind: ExprKind::Call(method.id, ir::CallArgs::positional(vec![receiver])),
        };
        self.index_word(result, at, true)
    }

    fn index_type_error(
        &self,
        value: Expr,
        at: &ast::Expr,
        returned: bool,
    ) -> Result<Expr, String> {
        let record = self
            .types
            .namedtuple_definition(value.ty)
            .and_then(|id| self.types.namedtuple(id));
        let name = match value.ty {
            Type::None => "NoneType",
            Type::Float => "float",
            Type::Str | Type::StrLiteral(_) => "str",
            Type::Bytes => "bytes",
            Type::ListInt | Type::List(_) => "list",
            Type::Dict(_) if self.types.is_counter(value.ty) => "Counter",
            Type::Dict(_) if self.types.is_default_list_dict(value.ty) => "collections.defaultdict",
            Type::Dict(_) => "dict",
            Type::Set(_) => "set",
            Type::Tuple(_) if !self.types.is_partial_frame(value.ty) => {
                record.as_ref().map_or("tuple", |r| r.name.as_str())
            }
            Type::PlainObject => "object",
            Type::Class(id) | Type::StrEnum(id) | Type::Enum(id) => {
                self.classes.declarations.definitions[id].name.as_str()
            }
            _ => {
                return Err(error(
                    self.source,
                    at,
                    "integer indexing requires a concrete supported type",
                ));
            }
        };
        let message = if returned {
            format!("__index__ returned non-int (type {name})")
        } else {
            format!("'{name}' object cannot be interpreted as an integer")
        };
        Ok(type_error_after(Type::Int, vec![value], message))
    }
}
