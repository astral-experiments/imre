//! Transferable operands for an ordinary compiled callable started on a native worker.

use ruff_python_ast as ast;

/// A static admission proof. The runtime validates the actual objects before starting the worker.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ThreadEntry {
    Parameters { names: Vec<String>, lock_only: bool },
    Captured(Vec<String>),
    Bound(String),
}

pub fn thread_entry(def: &ast::StmtFunctionDef) -> Option<ThreadEntry> {
    // Every admitted suite also excludes yield and yield from.
    if def.is_async || def.returns.is_some() {
        return None;
    }
    let (stmts, params) = (&def.body, &def.parameters);
    if let Some((names, lock_only)) = parameters(stmts, params) {
        Some(ThreadEntry::Parameters {
            names: names.into_iter().map(str::to_owned).collect(),
            lock_only,
        })
    } else if let Some(names) = captured_thread_locks(stmts, params) {
        Some(ThreadEntry::Captured(
            names.into_iter().map(str::to_owned).collect(),
        ))
    } else {
        bound_lock_field(stmts, params).map(|name| ThreadEntry::Bound(name.to_owned()))
    }
}

struct Proof<'a> {
    params: &'a [&'a str],
    // Joinable workers cannot dispatch a Python exception hook across heaps.
    lock_only: bool,
}

impl Proof<'_> {
    fn expression(&mut self, expr: &ast::Expr) -> bool {
        match expr {
            ast::Expr::BooleanLiteral(_) | ast::Expr::NoneLiteral(_) => true,
            ast::Expr::NumberLiteral(_) => {
                self.lock_only = false;
                true
            }
            ast::Expr::Compare(expr) => {
                self.lock_only = false;
                expr.operands.iter().all(|e| self.expression(e))
            }
            ast::Expr::UnaryOp(expr) if expr.op == ast::UnaryOp::Not => {
                self.expression(&expr.operand)
            }
            ast::Expr::BoolOp(expr) => expr.values.iter().all(|operand| self.expression(operand)),
            ast::Expr::Call(call) if call.arguments.keywords.is_empty() => {
                if let ast::Expr::Name(name) = &*call.func {
                    self.lock_only = false;
                    return self.params.contains(&name.id.as_str())
                        && call.arguments.args.iter().all(|arg| self.expression(arg));
                }
                let ast::Expr::Attribute(attr) = &*call.func else {
                    return false;
                };
                matches!(&*attr.value, ast::Expr::Name(name) if self.params.contains(&name.id.as_str()))
                    && match attr.attr.as_str() {
                        "locked" => call.arguments.args.is_empty(),
                        "acquire" => matches!(
                            call.arguments.args.as_ref(),
                            [] | [ast::Expr::BooleanLiteral(_)]
                        ),
                        "release" => {
                            self.lock_only = false;
                            call.arguments.args.is_empty()
                        }
                        _ => false,
                    }
            }
            _ => false,
        }
    }

    fn suite(&mut self, stmts: &[ast::Stmt]) -> bool {
        stmts.iter().all(|stmt| match stmt {
            ast::Stmt::Pass(_) | ast::Stmt::Break(_) | ast::Stmt::Continue(_) => true,
            ast::Stmt::Expr(stmt) => self.expression(&stmt.value),
            ast::Stmt::Return(stmt) => stmt.value.as_deref().is_none_or(|v| self.expression(v)),
            ast::Stmt::If(stmt) => {
                self.expression(&stmt.test)
                    && self.suite(&stmt.body)
                    && stmt.elif_else_clauses.iter().all(|clause| {
                        clause.test.as_ref().is_none_or(|v| self.expression(v))
                            && self.suite(&clause.body)
                    })
            }
            ast::Stmt::While(stmt) => {
                self.expression(&stmt.test) && self.suite(&stmt.body) && self.suite(&stmt.orelse)
            }
            ast::Stmt::Try(stmt) if !stmt.is_star => {
                let [ast::ExceptHandler::ExceptHandler(handler)] = stmt.handlers.as_slice() else {
                    return false;
                };
                handler.type_.is_none()
                    && handler.name.is_none()
                    && self.suite(&stmt.body)
                    && self.suite(&handler.body)
                    && self.suite(&stmt.orelse)
                    && self.suite(&stmt.finalbody)
            }
            _ => false,
        })
    }
}

fn parameters<'a>(
    stmts: &[ast::Stmt],
    params: &'a ast::Parameters,
) -> Option<(Vec<&'a str>, bool)> {
    if params.vararg.is_some() || params.kwarg.is_some() || !params.kwonlyargs.is_empty() {
        return None;
    }
    let mut names = Vec::new();
    for param in params.posonlyargs.iter().chain(&params.args) {
        let name = param.name().as_str();
        if param.parameter.annotation.is_some() || name.starts_with("__") {
            return None;
        }
        names.push(name);
    }
    let mut proof = Proof {
        params: &names,
        lock_only: true,
    };
    if !proof.suite(stmts) {
        return None;
    }
    let lock_only = proof.lock_only;
    Some((names, lock_only))
}

/// The only heap lookup in this method precedes the blocking native acquire.
fn bound_lock_field<'a>(stmts: &'a [ast::Stmt], params: &ast::Parameters) -> Option<&'a str> {
    if params.vararg.is_some() || params.kwarg.is_some() || !params.kwonlyargs.is_empty() {
        return None;
    }
    let mut positional = params.posonlyargs.iter().chain(&params.args);
    let receiver = positional.next()?.name();
    if positional.next().is_some() || receiver.starts_with("__") {
        return None;
    }
    let [ast::Stmt::Expr(statement)] = stmts else {
        return None;
    };
    let ast::Expr::Call(call) = &*statement.value else {
        return None;
    };
    let ast::Expr::Attribute(method) = &*call.func else {
        return None;
    };
    let ast::Expr::Attribute(field) = &*method.value else {
        return None;
    };
    let ast::Expr::Name(owner) = &*field.value else {
        return None;
    };
    (call.arguments.is_empty()
        && method.attr.as_str() == "acquire"
        && owner.id.as_str() == receiver.as_str()
        && !field.attr.starts_with("__"))
    .then(|| field.attr.as_str())
}

/// Certify a final lock acquire with at most one unconditional release before it.
/// No cell can be read after a potentially blocking operation.
pub fn captured_thread_locks<'a>(
    stmts: &'a [ast::Stmt],
    params: &ast::Parameters,
) -> Option<Vec<&'a str>> {
    fn receiver<'a>(stmt: &'a ast::Stmt, method: &str) -> Option<&'a str> {
        let ast::Stmt::Expr(expr) = stmt else {
            return None;
        };
        let ast::Expr::Call(call) = &*expr.value else {
            return None;
        };
        let ast::Expr::Attribute(attr) = &*call.func else {
            return None;
        };
        let ast::Expr::Name(name) = &*attr.value else {
            return None;
        };
        (attr.attr.as_str() == method && call.arguments.is_empty()).then(|| name.id.as_str())
    }
    if params.iter().next().is_some() {
        return None;
    }
    match stmts {
        [last] => Some(vec![receiver(last, "acquire")?]),
        [first, last] => {
            let first = receiver(first, "release")?;
            let last = receiver(last, "acquire")?;
            (first != last).then(|| vec![first, last])
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(source: &str) -> Option<ThreadEntry> {
        let parsed = ruff_python_parser::parse_module(source).unwrap();
        let ast::Stmt::FunctionDef(definition) = &parsed.syntax().body[0] else {
            unreachable!()
        };
        thread_entry(definition)
    }

    #[test]
    fn retains_joinable_and_detached_admission() {
        assert_eq!(
            entry("def wait(lock, /):\n while not lock.acquire(False):\n  continue"),
            Some(ThreadEntry::Parameters {
                names: vec!["lock".into()],
                lock_only: true,
            })
        );
        assert_eq!(
            entry("def work(op, lock):\n try:\n  op(1)\n except:\n  lock.release()"),
            Some(ThreadEntry::Parameters {
                names: vec!["op".into(), "lock".into()],
                lock_only: false,
            })
        );
        assert_eq!(
            entry("def wait():\n first.release()\n last.acquire()"),
            Some(ThreadEntry::Captured(vec!["first".into(), "last".into()]))
        );
        assert_eq!(
            entry("def wait(self: Receiver): self._lock.acquire()"),
            Some(ThreadEntry::Bound("_lock".into()))
        );
    }

    #[test]
    fn detached_operations_in_nested_suites_grade_the_whole_body() {
        for operation in ["42", "True == None", "op()", "op.release()"] {
            let source = format!(
                "def work(op):\n if op.locked():\n  pass\n else:\n  {operation}\n  op.acquire(False)"
            );
            assert_eq!(
                entry(&source),
                Some(ThreadEntry::Parameters {
                    names: vec!["op".into()],
                    lock_only: false,
                }),
                "{operation}"
            );
        }
        assert_eq!(entry("def work(op):\n op()\n unknown.acquire()"), None);
    }

    #[test]
    fn excludes_other_calls_and_suspended_entries() {
        for source in [
            "def wait(lock: Lock): lock.acquire()",
            "def wait(lock) -> bool: lock.acquire()",
            "def wait(*, lock): lock.acquire()",
            "def wait(lock): lock.acquire(blocking=False)",
            "def wait(lock): lock.acquire(other())",
            "def wait(self): self.__private.acquire()",
            "def wait():\n lock.release()\n lock.acquire()",
            "def wait(): yield True",
            "async def wait(lock): lock.acquire()",
        ] {
            assert_eq!(entry(source), None, "{source}");
        }
    }
}
