//! Preserve catchable errors from a literal add_argument call at the call site.

use super::*;
use ruff_python_ast::visitor::walk_stmt;

pub(super) fn boolean_action(
    expr: &ast::Expr,
    graph: &SourceGraph,
    id: ModuleId,
    locals: &HashSet<String>,
) -> bool {
    let mut root = expr;
    while let ast::Expr::Attribute(attr) = root {
        root = &attr.value;
    }
    root.as_name_expr()
        .is_some_and(|name| !locals.contains(name.id.as_str()))
        && graph.resolve(id, expr) == Some(Binding::Intrinsic(Intrinsic::BooleanOptionalAction))
}

fn raise(kind: &str, message: Expr) -> Expr {
    Expr {
        ty: Type::None,
        kind: ExprKind::Builtin(
            Builtin::RaiseException,
            vec![Expr {
                ty: Type::Exception,
                kind: ExprKind::Exception {
                    kind: exception_kind(kind).unwrap(),
                    message: Some(Box::new(message)),
                },
            }],
        ),
    }
}

struct Calls<'a> {
    receiver: &'a str,
    graph: &'a SourceGraph,
    module: ModuleId,
    locals: &'a HashSet<String>,
    prefixes: &'a str,
    failures: HashMap<TextSize, Expr>,
}

impl Calls<'_> {
    fn failure(&self, call: &ast::ExprCall) -> Option<Expr> {
        let flags = call
            .arguments
            .args
            .iter()
            .map(|arg| {
                let flag = arg.as_string_literal_expr()?.value.to_str();
                // Complete the earlier optional spelling and destination checks before
                // using the action constructor's error.
                (flag.starts_with(|c| self.prefixes.contains(c))
                    && !flag
                        .trim_start_matches(|c| self.prefixes.contains(c))
                        .is_empty())
                .then_some(flag)
            })
            .collect::<Option<Vec<_>>>()?;
        if flags.is_empty() {
            return None;
        }
        let mut action = None;
        let mut keywords = Vec::new();
        for kw in &call.arguments.keywords {
            let name = kw.arg.as_ref()?.as_str();
            if name == "action" {
                action = Some(&kw.value);
            } else {
                // A dynamic destination can fail the parser's default lookup before
                // the constructor is called. Other keywords are evaluated at the call site.
                if name == "dest"
                    && !matches!(
                        kw.value,
                        ast::Expr::NoneLiteral(_)
                            | ast::Expr::BooleanLiteral(_)
                            | ast::Expr::StringLiteral(_)
                            | ast::Expr::NumberLiteral(_)
                    )
                    && builtin_constant(&kw.value, &self.graph.modules[self.module], self.locals)
                        .is_none()
                {
                    return None;
                }
                keywords.push(name);
            }
        }
        let action = action?;
        let boolean = boolean_action(action, self.graph, self.module, self.locals);
        let class = if boolean {
            "BooleanOptionalAction"
        } else {
            match action.as_string_literal_expr()?.value.to_str() {
                "store_true" => "_StoreTrueAction",
                "store_false" => "_StoreFalseAction",
                "count" => "_CountAction",
                "store_const" => "_StoreConstAction",
                "append_const" => "_AppendConstAction",
                _ => return None,
            }
        };
        for name in keywords {
            if !matches!(
                name,
                "dest" | "default" | "help" | "required" | "deprecated" | "option_strings"
            ) && !(matches!(class, "_StoreConstAction" | "_AppendConstAction")
                && matches!(name, "const" | "metavar"))
            {
                return Some(raise(
                    "TypeError",
                    unpacking::string(format!(
                        "{class}.__init__() got an unexpected keyword argument '{name}'"
                    )),
                ));
            }
        }
        if boolean && let Some(flag) = flags.into_iter().find(|flag| flag.starts_with("--no-")) {
            let quoted = Expr {
                ty: Type::Str,
                kind: ExprKind::Builtin(Builtin::Repr, vec![unpacking::string(flag)]),
            };
            return Some(raise(
                "ValueError",
                unpacking::concat(
                    unpacking::concat(unpacking::string("invalid option name "), quoted),
                    unpacking::string(" for BooleanOptionalAction"),
                ),
            ));
        }
        None
    }
}

/// Evaluate the supplied keyword expressions before the inevitable exception.
/// The statically resolved action and literal flag strings are effect free.
pub(super) fn at_call(
    checker: &mut Checker<'_>,
    call: &ast::ExprCall,
    mut failure: Expr,
) -> Result<Expr, String> {
    let ExprKind::Builtin(Builtin::RaiseException, args) = &mut failure.kind else {
        unreachable!()
    };
    let mut evaluated = Vec::new();
    for kw in &call.arguments.keywords {
        if kw
            .arg
            .as_ref()
            .is_some_and(|name| name.as_str() != "action")
        {
            evaluated.push(checker.expr(&kw.value)?);
        }
    }
    evaluated.append(args);
    *args = evaluated;
    Ok(failure)
}

impl<'ast> Visitor<'ast> for Calls<'_> {
    fn visit_stmt(&mut self, stmt: &'ast ast::Stmt) {
        if !matches!(stmt, ast::Stmt::FunctionDef(_) | ast::Stmt::ClassDef(_)) {
            walk_stmt(self, stmt);
        }
    }

    fn visit_expr(&mut self, expr: &'ast ast::Expr) {
        if let Some(call) = method(expr, self.receiver, "add_argument")
            && let Some(failure) = self.failure(call)
        {
            self.failures.insert(call.start(), failure);
        }
        if !matches!(expr, ast::Expr::Lambda(_)) {
            walk_expr(self, expr);
        }
    }
}

pub(super) fn failures(
    body: &[ast::Stmt],
    receiver: &str,
    graph: &SourceGraph,
    module: ModuleId,
    locals: &HashSet<String>,
    prefixes: &str,
) -> HashMap<TextSize, Expr> {
    let mut calls = Calls {
        receiver,
        graph,
        module,
        locals,
        prefixes,
        failures: HashMap::new(),
    };
    calls.visit_body(body);
    calls.failures
}
