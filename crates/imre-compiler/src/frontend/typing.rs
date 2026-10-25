//! Preserve Python function identities in typing's run-time overload registry.
use super::*;
use crate::modules::IntrinsicModule;
use crate::types::FunctionSite;
use ruff_python_ast::visitor::{Visitor, walk_expr, walk_stmt};

struct UsesOverload<'a> {
    graph: &'a SourceGraph,
    module: ModuleId,
    found: bool,
}
impl<'a> Visitor<'a> for UsesOverload<'_> {
    fn visit_expr(&mut self, expression: &'a ast::Expr) {
        if self.found {
            return;
        }
        if matches!(
            self.graph.resolve(self.module, expression),
            Some(Binding::Intrinsic(
                Intrinsic::Overload | Intrinsic::GetOverloads
            ))
        ) {
            self.found = true;
        } else {
            walk_expr(self, expression);
        }
    }
}

struct Sites<'a> {
    types: &'a Types,
    module: ModuleId,
    name: &'a str,
    line_starts: Vec<usize>,
    scope: Vec<String>,
}
impl<'a> Visitor<'a> for Sites<'_> {
    fn visit_stmt(&mut self, stmt: &'a ast::Stmt) {
        match stmt {
            ast::Stmt::FunctionDef(node) => {
                self.scope.push(node.name.to_string());
                let start = node
                    .decorator_list
                    .first()
                    .map_or(node.range().start(), |d| d.range().start());
                let line = self
                    .line_starts
                    .partition_point(|&offset| offset <= start.to_usize())
                    as u32;
                self.types.add_function_site(
                    self.module,
                    node.name.range().start().to_u32(),
                    FunctionSite {
                        module: self.name.into(),
                        qualname: self.scope.join("."),
                        line,
                    },
                );
                self.scope.push("<locals>".into());
                self.visit_body(&node.body);
                self.scope.pop();
                self.scope.pop();
            }
            ast::Stmt::ClassDef(node) => {
                self.scope.push(node.name.to_string());
                self.visit_body(&node.body);
                self.scope.pop();
            }
            _ => walk_stmt(self, stmt),
        }
    }
}

pub(super) fn collect_sites(graph: &SourceGraph, types: &Types) {
    let used = graph.modules.iter().enumerate().any(|(id, m)| {
        if !m.bindings.values().any(|b| {
            matches!(
                b,
                Binding::IntrinsicModule(IntrinsicModule::Typing)
                    | Binding::Intrinsic(Intrinsic::Overload | Intrinsic::GetOverloads)
            )
        }) {
            return false;
        }
        let mut visitor = UsesOverload {
            graph,
            module: id,
            found: false,
        };
        visitor.visit_body(&m.parsed.syntax().body);
        visitor.found
    });
    if !used {
        return;
    }
    for (id, m) in graph.modules.iter().enumerate() {
        Sites {
            types,
            module: id,
            name: if id == graph.entry {
                "__main__"
            } else {
                &m.name
            },
            line_starts: std::iter::once(0)
                .chain(m.source.match_indices('\n').map(|(i, _)| i + 1))
                .collect(),
            scope: vec![],
        }
        .visit_body(&m.parsed.syntax().body);
    }
}

/// An inert annotated stub evaluates to None if the original function itself
/// is retrieved. The decorator changes the public binding to a raising dummy.
pub(super) fn strip_overload(def: &mut ast::StmtFunctionDef) {
    def.decorator_list.clear();
    if annotations::inert_body(def) {
        def.returns = None;
        def.body.retain(|s| !matches!(s, ast::Stmt::Expr(e) if matches!(&*e.value,ast::Expr::EllipsisLiteral(_))));
    }
}

pub(super) fn is_dummy(graph: &SourceGraph, binding: Option<Binding>) -> bool {
    matches!(binding, Some(Binding::Intrinsic(Intrinsic::OverloadDummy)))
        || matches!(binding, Some(Binding::Definition(d)) if graph.modules[d.module].overloads.contains(&d.statement))
}

/// Both representations own the same callable record. A whole-program check
/// below proves every possible registered source has the requested signature.
pub(super) fn typed_function(value: Expr, signature: Type) -> Expr {
    Expr {
        ty: signature,
        kind: ExprKind::Builtin(Builtin::RecordView, vec![value]),
    }
}

pub(super) fn dummy() -> Expr {
    Expr {
        ty: Type::OverloadStub,
        kind: ExprKind::Builtin(Builtin::Typing(ir::TypingOperation::Dummy), vec![]),
    }
}

pub(super) fn attribute_index(name: &str) -> Option<u32> {
    Some(match name {
        "__module__" => 0,
        "__qualname__" => 1,
        "__name__" => 2,
        "__deprecated__" => 3,
        _ => return None,
    })
}

impl Checker<'_> {
    pub(super) fn typing_overload(
        &mut self,
        call: &ast::ExprCall,
        intrinsic: Intrinsic,
    ) -> Result<Expr, String> {
        use ir::TypingOperation as Op;
        let result = |ty, op, args| Expr {
            ty,
            kind: ExprKind::Builtin(Builtin::Typing(op), args),
        };
        if intrinsic == Intrinsic::ClearOverloads {
            if !call.arguments.args.is_empty() || !call.arguments.keywords.is_empty() {
                return Err(error(
                    self.source,
                    call,
                    "clear_overloads() takes no arguments",
                ));
            }
            return Ok(result(Type::None, Op::Clear, vec![]));
        }
        let argument = match (&*call.arguments.args, &*call.arguments.keywords) {
            ([value], []) => value,
            ([], [kw]) if kw.arg.as_ref().is_some_and(|s| s.as_str() == "func") => &kw.value,
            _ => {
                return Err(error(
                    self.source,
                    call,
                    "typing overload operations require one function",
                ));
            }
        };
        let register = intrinsic == Intrinsic::Overload;
        let ret = if register {
            Type::OverloadStub
        } else {
            self.types.list(Type::FunctionRef)
        };
        if let ast::Expr::Name(n) = argument
            && matches!(n.id.as_str(), "sum" | "print")
            && !self.names.contains_key(n.id.as_str())
            && self.resolve(argument).is_none()
        {
            let text = |v: &str| Expr {
                ty: Type::Str,
                kind: ExprKind::Str(v.into()),
            };
            return Ok(result(
                ret,
                if register {
                    Op::RegisterBuiltin
                } else {
                    Op::GetBuiltin
                },
                vec![text("builtins"), text(n.id.as_str())],
            ));
        }
        let f = self.expr(argument)?;
        if !matches!(f.ty, Type::Callable(_) | Type::FunctionRef) {
            return Err(error(
                self.source,
                argument,
                "typing overload operations require a known plain source function",
            ));
        }
        Ok(result(
            ret,
            if register { Op::Register } else { Op::Get },
            vec![f],
        ))
    }
}

/// Only unbound source functions with a known adapter can enter an overload result.
pub(super) fn validate_functions(
    functions: &[ir::Function],
    entry: &ir::Function,
    types: &Types,
) -> Result<(), String> {
    if !types.has_reflected_functions() {
        return Ok(());
    }
    let mut signatures = HashMap::new();
    let mut reflected = HashSet::new();
    let mut registered = HashSet::new();
    let mut recovered = HashSet::new();
    for function in functions.iter().chain([entry]) {
        ir::walk_expressions(&function.body, &mut |e| match &e.kind {
            ExprKind::Callable { function, receiver } => {
                let good = receiver.is_none()
                    && types.dispatcher(e.ty).is_none()
                    && types.function_site(*function).is_some()
                    && types.cache_adapter(*function).is_none();
                *signatures.entry(e.ty).or_insert(true) &= good;
            }
            ExprKind::Builtin(
                Builtin::Typing(
                    op @ (ir::TypingOperation::Register
                    | ir::TypingOperation::Get
                    | ir::TypingOperation::Attr(_)),
                ),
                args,
            ) if matches!(args[0].ty, Type::Callable(_)) => {
                reflected.insert(args[0].ty);
                if *op == ir::TypingOperation::Register {
                    registered.insert(args[0].ty);
                }
            }
            ExprKind::Builtin(Builtin::RecordView, args)
                if args[0].ty == Type::FunctionRef && matches!(e.ty, Type::Callable(_)) =>
            {
                recovered.insert(e.ty);
            }
            ExprKind::Builtin(Builtin::Typing(ir::TypingOperation::SetAttr(_)), args)
                if matches!(args[1].ty, Type::Callable(_)) =>
            {
                reflected.insert(args[1].ty);
            }
            _ => {}
        });
    }
    let safe: HashSet<_> = signatures
        .into_iter()
        .filter_map(|(t, v)| v.then_some(t))
        .collect();
    if !reflected.is_subset(&safe) {
        return Err(
            "typing overloads require plain source functions for every possible argument".into(),
        );
    }
    if !recovered.is_empty() && (registered.len() != 1 || recovered != registered) {
        return Err(
            "calling a retrieved overload requires one proven registered source signature".into(),
        );
    }
    // Leave unrelated callback signatures on the existing representation.
    types.allow_plain_callbacks(reflected);
    Ok(())
}
