//! Keep the callable's declared parameter names for archive dispatch. The
//! runtime owns the registration; a generated function unpacks its typed frame.
use super::*;

impl Checker<'_> {
    pub(in super::super) fn shutil_register_format(
        &mut self,
        call: &ast::ExprCall,
        unpack: bool,
    ) -> Result<Expr, String> {
        calls::reject_unpacking(call, self.source)?;
        let none = Expr {
            ty: Type::None,
            kind: ExprKind::None,
        };
        let mut params = [
            ("name", Type::Str, None),
            ("extensions", self.types.list(Type::Str), None),
            ("function", Type::None, None),
            ("extra_args", Type::None, Some(none.clone())),
            ("description", Type::Str, Some(unpacking::string(""))),
        ]
        .map(|(name, ty, default)| Parameter {
            name: name.into(),
            ty,
            default,
            kind: ParameterKind::PositionalOrKeyword,
            sign_only: false,
        })
        .to_vec();
        if !unpack {
            params.remove(1);
        }
        let func = if unpack { 2 } else { 1 };
        let mut signature = None;
        let mut root_flag = None;
        let mut name = String::new();
        let mut module = self.module;
        let mut args = self.arguments_with(call, &params, None, |c, i, expr| {
            if i == func {
                if let Some(Binding::Definition(id)) = c.resolve(expr)
                    && let Some(mut definition) = c.functions.get(&id).cloned()
                {
                    name = c.graph.modules[id.module].parsed.syntax().body[id.statement]
                        .as_function_def_stmt()
                        .unwrap()
                        .name
                        .to_string();
                    module = id.module;
                    if !unpack {
                        root_flag = Some((id.module, id.statement));
                    }
                    signature = Some(definition.params.clone());
                    // The runtime binds keyword-only values before calling this closed ABI.
                    for p in &mut definition.params {
                        p.kind = ParameterKind::PositionalOrKeyword;
                    }
                    return c.callable_value(&definition, None, None, expr);
                }
                if let Some(binding) = c.local_function_binding(expr) {
                    let local = binding.target(c.source, expr)?;
                    name = local.definition.name.to_string();
                    signature = Some(local.signature.params.clone());
                    return local.callback(c.types, c.source, expr);
                }
                return c.expr(expr);
            }
            if i == func + 1 {
                if matches!(expr, ast::Expr::List(list) if list.elts.is_empty()) {
                    let pair = c.types.tuple(vec![Type::Str, Type::Str]);
                    return c.expr_expected(expr, c.types.list(pair));
                }
                return c.expr(expr);
            }
            c.expr_expected(expr, params[i].ty)
        })?;
        // Packing needs no extensions. Keep common native positions after evaluating the arguments.
        if !unpack {
            args.order.insert(1, args.values.len());
            args.values.push(none.clone());
        }
        let callback = args.values[args.order[2]].ty;
        let result_ty = if unpack { Type::None } else { Type::Str };
        let mut keyword_value = None;
        let mut variadic_value = None;
        let (layout, adapter, metadata) =
            if let Some((fields, returns)) = self.types.signature(callback) {
                let declared = signature.ok_or_else(|| error(self.source, call,
                "registered archive callbacks need a known signature to bind keyword arguments"))?;
                // Two unrelated container/class types can have the same storage kind. Never
                // bind an erased object to a differently typed slot in the callback frame.
                for (p, &ty) in declared.iter().zip(&fields) {
                    let value = if p.kind == ParameterKind::KeywordRest {
                        let (Type::Str, element) =
                            self.types.mapping(ty).expect("typed keyword rest")
                        else {
                            unreachable!("keyword names are strings")
                        };
                        keyword_value = Some(element);
                        element
                    } else if p.kind == ParameterKind::Variadic {
                        let element = self.types.element(ty).expect("typed positional rest");
                        variadic_value = Some(element);
                        element
                    } else {
                        ty
                    };
                    if !matches!(
                        self.types.payload(value).unwrap_or(value),
                        Type::None | Type::Bool | Type::Int | Type::Float | Type::Str | Type::Bytes
                    ) {
                        return Err(error(
                            self.source,
                            call,
                            "registered archive callback parameters require scalar types",
                        ));
                    }
                }
                // Keep the callable separate from the frame for the bound arguments.
                if !unpack && returns != Type::Str {
                    return Err(error(
                        self.source,
                        call,
                        "registered packing callbacks require a string return type",
                    ));
                }
                let frame = self.types.tuple(fields.clone());
                let field = |ty, index| Expr {
                    ty,
                    kind: ExprKind::Field(
                        Box::new(Expr {
                            ty: frame,
                            kind: ExprKind::Local(1),
                        }),
                        index,
                    ),
                };
                let invoke = Expr {
                    ty: returns,
                    kind: ExprKind::Invoke {
                        callable: Box::new(Expr {
                            ty: callback,
                            kind: ExprKind::Local(0),
                        }),
                        args: ir::CallArgs::positional(
                            fields
                                .into_iter()
                                .enumerate()
                                .map(|(i, ty)| field(ty, i))
                                .collect(),
                        ),
                    },
                };
                let id = self.helpers.reserve();
                self.helpers.finish(
                    id,
                    comprehensions::Helper {
                        module: self.module,
                        calls: HashSet::new(),
                        class_reads: HashSet::new(),
                        recursion_error: self.graph.modules[self.module].error(
                            call,
                            "recursive calls through registered archive formats are unsupported",
                        ),
                        function: ir::Function {
                            params: vec![0, 1],
                            locals: vec![callback, frame],
                            returns: result_ty,
                            body: if unpack {
                                vec![Statement::Expr(invoke), Statement::Return(none.clone())]
                            } else {
                                vec![Statement::Return(invoke)]
                            },
                        },
                    },
                );
                let tuple = |values: Vec<Expr>| Expr {
                    ty: self.types.tuple(values.iter().map(|v| v.ty).collect()),
                    kind: ExprKind::Tuple(values),
                };
                let origin = if module == self.graph.entry {
                    "__main__"
                } else {
                    &self.graph.modules[module].name
                };
                let qualified = format!("{origin}.{name}");
                let metadata = tuple(
                    [unpacking::string(qualified), unpacking::string(name)]
                        .into_iter()
                        .chain(declared.into_iter().map(|p| {
                            let mode = match p.kind {
                                ParameterKind::PositionalOnly => "/",
                                ParameterKind::PositionalOrKeyword => "",
                                ParameterKind::KeywordOnly => "*",
                                ParameterKind::Variadic => "*args",
                                ParameterKind::KeywordRest => "**",
                            };
                            tuple(vec![
                                unpacking::string(p.name),
                                unpacking::string(mode),
                                Expr {
                                    ty: Type::Bool,
                                    kind: ExprKind::Bool(p.default.is_some()),
                                },
                                p.default.unwrap_or_else(|| none.clone()),
                            ])
                        }))
                        .collect(),
                );
                (
                    Some(frame),
                    Expr {
                        ty: self.types.callable(vec![callback, frame], result_ty),
                        kind: ExprKind::Callable {
                            function: id,
                            receiver: None,
                        },
                    },
                    metadata,
                )
            } else {
                (None, none.clone(), none)
            };
        for value in [adapter, metadata] {
            args.order.push(args.values.len());
            args.values.push(value);
        }
        Ok(self.ordered_builtin(
            Builtin::Process(ir::ProcessOperation::RegisterArchive {
                unpack,
                frame: layout,
                keyword_value,
                variadic_value,
                root_flag,
            }),
            Type::None,
            args,
        ))
    }
}
