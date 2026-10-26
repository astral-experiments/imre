//! Reflect docstrings from the closed module graph without evaluating module
//! attributes or importing a host Python interpreter.

use super::*;

use crate::modules::doctest::{sealed, source_docstrings};

impl Checker<'_> {
    pub(super) fn doctest_suite(&mut self, call: &ast::ExprCall) -> Result<Option<Expr>, String> {
        if self.resolve(&call.func) == Some(Binding::Intrinsic(Intrinsic::DocExecute)) {
            let def = self.graph.doctest_runner().ok_or_else(|| {
                error(
                    self.source,
                    call,
                    "doctest execution requires a compiled documentation suite",
                )
            })?;
            let sig = self.functions[&def].clone();
            let args = self.arguments(call, &sig.params, None)?;
            self.calls.insert(sig.id);
            return Ok(Some(Expr {
                ty: sig.returns,
                kind: ExprKind::Call(sig.id, args),
            }));
        }
        if let Some(Binding::Intrinsic(
            op @ (Intrinsic::DocCaptureStart
            | Intrinsic::DocCaptureTake
            | Intrinsic::DocAscii
            | Intrinsic::DocDisplay),
        )) = self.resolve(&call.func)
        {
            self.reject_builtin_keywords(call)?;
            let (ty, kind) = match (op, call.arguments.args.as_ref()) {
                (Intrinsic::DocCaptureStart, []) => (
                    Type::None,
                    ExprKind::Builtin(Builtin::DocCaptureStart, vec![]),
                ),
                (Intrinsic::DocCaptureTake, []) => (
                    Type::Str,
                    ExprKind::Builtin(Builtin::DocCaptureTake, vec![]),
                ),
                (Intrinsic::DocAscii, [value]) => (
                    Type::Str,
                    ExprKind::Builtin(
                        Builtin::AsciiEscape,
                        vec![self.expr_expected(value, Type::Str)?],
                    ),
                ),
                (Intrinsic::DocDisplay, [value]) => {
                    let value = self.expr(value)?;
                    if value.ty == Type::None {
                        return Ok(Some(value));
                    }
                    let slot = self.hidden(value.ty);
                    let mut shown = self.local(slot);
                    let optional = self.types.payload(shown.ty);
                    if let Some(ty) = optional {
                        shown = Expr {
                            ty,
                            kind: ExprKind::Unwrap(Box::new(shown)),
                        };
                    }
                    let text = if matches!(shown.ty, Type::Document(_)) {
                        documents::op(ir::DocumentOp::Repr, Type::Str, vec![shown])
                    } else if matches!(shown.ty, Type::Exception | Type::NamedException(_)) {
                        Expr {
                            ty: Type::Str,
                            kind: ExprKind::Builtin(Builtin::ExceptionRepr, vec![shown]),
                        }
                    } else {
                        if !dataclass_values::can_format(shown.ty, self.types, self.classes) {
                            return Err(error(
                                self.source,
                                call,
                                "doctest display requires a supported native repr",
                            ));
                        }
                        Expr {
                            ty: Type::Str,
                            kind: ExprKind::Builtin(Builtin::Repr, vec![shown]),
                        }
                    };
                    let print = Statement::Expr(Expr {
                        ty: Type::None,
                        kind: ExprKind::Builtin(Builtin::Print, vec![text]),
                    });
                    let display = if optional.is_some() {
                        Statement::If(
                            Expr {
                                ty: Type::Bool,
                                kind: ExprKind::Builtin(Builtin::IsNone, vec![self.local(slot)]),
                            },
                            vec![],
                            vec![print],
                        )
                    } else {
                        print
                    };
                    (
                        Type::None,
                        ExprKind::Scope {
                            body: vec![Statement::Assign(slot, value), display],
                            value: Box::new(Expr {
                                ty: Type::None,
                                kind: ExprKind::None,
                            }),
                            clear: vec![slot],
                        },
                    )
                }
                _ => return Err(error(self.source, call, "invalid doctest runtime call")),
            };
            return Ok(Some(Expr { ty, kind }));
        }
        let Some(Binding::Definition(def)) = self.resolve(&call.func) else {
            return Ok(None);
        };
        let owner = &self.graph.modules[def.module];
        if owner.name != "doctest"
            || owner.bindings.get("DocTestSuite") != Some(&Binding::Definition(def))
        {
            return Ok(None);
        }
        let module = match (
            call.arguments.args.as_ref(),
            call.arguments.keywords.as_ref(),
        ) {
            ([], []) => self
                .module_docs(Binding::Module(self.module), call)?
                .ok_or_else(|| {
                    error(
                        self.source,
                        call,
                        "DocTestSuite requires module documentation",
                    )
                })?,
            ([arg], []) => self.expr(arg)?,
            ([], [kw]) if kw.arg.as_ref().is_some_and(|arg| arg == "module") => {
                self.expr(&kw.value)?
            }
            _ => {
                return Err(error(
                    self.source,
                    call,
                    "DocTestSuite supports a module and its default local fixture",
                ));
            }
        };
        let sig = &self.functions[&def];
        let module = self.coerce(module, sig.params[0].ty, call)?;
        self.calls.insert(sig.id);
        Ok(Some(Expr {
            ty: sig.returns,
            kind: ExprKind::Call(sig.id, ir::CallArgs::positional(vec![module])),
        }))
    }

    /// Module references consumed by a doctest suite retain one identity when
    /// the program loops over a tuple of modules, as JSON's load_tests does.
    pub(super) fn module_docs(
        &mut self,
        target: Binding,
        node: &impl Ranged,
    ) -> Result<Option<Expr>, String> {
        if !matches!(target, Binding::Module(_) | Binding::IntrinsicModule(_)) {
            return Ok(None);
        }
        let Some(def) = self
            .graph
            .modules
            .iter()
            .find(|m| m.name == "doctest")
            .and_then(|m| m.bindings.get("_module_docs"))
        else {
            return Ok(None);
        };
        let Binding::Definition(def) = *def else {
            unreachable!()
        };
        let Some(sig) = self.functions.get(&def) else {
            return Ok(None);
        };
        let mut docs = Vec::new();
        let module_name = match target {
            Binding::Module(id) => {
                let source = &self.graph.modules[id];
                if source.bindings.contains_key("__test__") {
                    return Err(error(
                        self.source,
                        node,
                        "DocTestSuite __test__ additions require static documentation bindings",
                    ));
                }
                source_docstrings(&source.name, &source.parsed.syntax().body, true, &mut docs);
                source.name.as_str()
            }
            Binding::IntrinsicModule(module) if let Some(catalog) = sealed(module) => {
                catalog.append_to(&mut docs);
                catalog.module
            }
            _ => return Ok(None),
        };
        docs.sort_unstable_by(|a, b| a.0.cmp(&b.0));
        let pair = self.types.tuple(vec![Type::Str, Type::Str]);
        let values = docs
            .into_iter()
            .map(|(name, doc)| Expr {
                ty: pair,
                kind: ExprKind::Tuple(
                    [name, doc]
                        .into_iter()
                        .map(|s| Expr {
                            ty: Type::Str,
                            kind: ExprKind::Str(s),
                        })
                        .collect(),
                ),
            })
            .collect();
        let catalog = Expr {
            ty: self.types.list(pair),
            kind: ExprKind::List(values),
        };
        self.calls.insert(sig.id);
        Ok(Some(Expr {
            ty: sig.returns,
            kind: ExprKind::Call(
                sig.id,
                ir::CallArgs::positional(vec![
                    Expr {
                        ty: Type::Str,
                        kind: ExprKind::Str(module_name.to_owned()),
                    },
                    catalog,
                ]),
            ),
        }))
    }
}
