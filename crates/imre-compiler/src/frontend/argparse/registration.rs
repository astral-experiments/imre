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

/// CPython formats the choices to validate an action's metavar during add_argument.
pub(super) fn choice_digits(range: Option<&[String; 3]>, choices: &[String]) -> Option<usize> {
    const MIN_LIMIT: usize = 640;
    // At most four bits per source digit, even for a hexadecimal literal.
    // Inputs this short are below every permissible runtime limit.
    let small = |value: &str| value.len() <= MIN_LIMIT / 2;
    fn number(literal: &str) -> num_bigint::BigInt {
        let literal = literal.replace('_', "");
        let (digits, negative) = literal
            .strip_prefix('-')
            .map_or((literal.as_str(), false), |digits| (digits, true));
        let (radix, digits) = match digits.as_bytes().get(..2) {
            Some(b"0x" | b"0X") => (16, &digits[2..]),
            Some(b"0o" | b"0O") => (8, &digits[2..]),
            Some(b"0b" | b"0B") => (2, &digits[2..]),
            _ => (10, digits),
        };
        let value = num_bigint::BigInt::parse_bytes(digits.as_bytes(), radix)
            .expect("validated source integer");
        if negative { -value } else { value }
    }
    let width = |n: &num_bigint::BigInt| n.magnitude().to_str_radix(10).len();
    let count = if let Some([first, stop, step]) = range {
        if small(first) && small(stop) {
            return None;
        }
        // Only the first and last members can have the largest magnitude. The
        // exclusive bound and the step can have more digits than either member.
        let first = number(first);
        let stop = number(stop);
        let step = number(step);
        let span = stop - &first;
        if span.sign() != step.sign() {
            return None;
        }
        let adjustment = if step.sign() == num_bigint::Sign::Minus {
            -1i32
        } else {
            1
        };
        let last = &first + ((span - adjustment) / &step) * step;
        width(&first).max(width(&last))
    } else {
        choices
            .iter()
            .filter(|text| !small(text))
            .map(|text| width(&number(text)))
            .max()
            .unwrap_or(0)
    };
    (count > MIN_LIMIT).then_some(count)
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

#[cfg(test)]
mod choice_tests {
    use super::choice_digits;

    #[test]
    fn count_only_actual_members_in_their_decimal_spelling() {
        let hex = format!("0X_1{}", "0".repeat(550));
        let stop = format!("0x1{}1", "0".repeat(549));
        assert_eq!(choice_digits(None, &[format!("-{hex}")]), Some(663));
        assert_eq!(
            choice_digits(Some(&["0".into(), hex.clone(), hex.clone()]), &[]),
            None
        );
        assert_eq!(
            choice_digits(Some(&["0".into(), stop, hex.clone()]), &[]),
            Some(663)
        );
        assert_eq!(
            choice_digits(Some(&[hex, "-1".into(), "1".into()]), &[]),
            None
        );
    }
}
