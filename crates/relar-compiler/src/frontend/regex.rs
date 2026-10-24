//! Sealed text patterns are validated while checking the source, including dead code.

use super::*;

pub(super) type CaptureLayout = std::rc::Rc<[crate::fixed_regex::Capture]>;

impl Checker<'_> {
    /// A successful search with an unchanged local has the fixed pattern's
    /// group layout throughout this arm, including any nested control flow.
    pub(super) fn regex_guard(
        &self,
        condition: &Expr,
        body: &[ast::Stmt],
    ) -> Option<(usize, CaptureLayout)> {
        let ExprKind::Builtin(Builtin::Bool, args) = &condition.kind else {
            return None;
        };
        let [
            Expr {
                kind: ExprKind::Bind(slot, rhs),
                ..
            },
        ] = args.as_slice()
        else {
            return None;
        };
        let ExprKind::Builtin(Builtin::RegexSearch | Builtin::RegexMatchStart, search) = &rhs.kind
        else {
            return None;
        };
        let groups = self.capture_layout(&search[0])?;
        if narrowing::assigned_names(body)
            .iter()
            .any(|name| self.names.get(name) == Some(slot))
        {
            return None;
        }
        Some((*slot, groups))
    }

    /// Resolve only a selector whose membership is proved. Unknown names,
    /// negative indices and out-of-range requests still go through IndexError.
    fn capture_index(&self, receiver: &Expr, selector: &Expr) -> Option<usize> {
        if matches!(&selector.kind, ExprKind::Int(n) if n == "0") {
            return Some(0);
        }
        let groups = self.capture_layout(receiver)?;
        match &selector.kind {
            ExprKind::Str(name) => groups
                .iter()
                .position(|group| group.name.as_deref() == Some(name)),
            ExprKind::Int(index) => index.parse::<usize>().ok().filter(|n| *n < groups.len()),
            _ => None,
        }
    }

    /// Carries fixed capture names and participation through a successful search
    /// and through dominated, single-assignment local bindings.
    fn capture_layout(&self, value: &Expr) -> Option<CaptureLayout> {
        match &value.kind {
            ExprKind::Unwrap(inner) | ExprKind::Present(inner) => self.capture_layout(inner),
            ExprKind::Builtin(
                Builtin::RegexSearch | Builtin::RegexMatchStart | Builtin::RegexGroupDict,
                args,
            ) => self.capture_layout(&args[0]),
            ExprKind::Local(slot) => self
                .regex_guards
                .iter()
                .rev()
                .find(|(saved, _)| saved == slot)
                .map(|(_, groups)| groups.clone())
                .or_else(|| self.regex_locals.get(slot).cloned()),
            _ => self.globals.regex_groups(value),
        }
    }

    pub(super) fn remember_regex_assignment(
        &mut self,
        stmt: &ast::Stmt,
        lowered: &[Statement],
        later: &[ast::Stmt],
    ) {
        if self.block_depth != 1 || self.module_scope {
            return;
        }
        let [Statement::Assign(slot, value)] = lowered else {
            return;
        };
        let dictionary = matches!(value.kind, ExprKind::Builtin(Builtin::RegexGroupDict, _));
        if !dictionary
            && !matches!(
                value.kind,
                ExprKind::Builtin(
                    Builtin::RegexCompile(_) | Builtin::RegexSearch | Builtin::RegexMatchStart,
                    _
                )
            )
        {
            return;
        }
        let name = match stmt {
            ast::Stmt::Assign(assign)
                if let [ast::Expr::Name(name)] = assign.targets.as_slice() =>
            {
                name.id.as_str()
            }
            ast::Stmt::AnnAssign(assign) if let ast::Expr::Name(name) = &*assign.target => {
                name.id.as_str()
            }
            _ => return,
        };
        if let Some(groups) = self.capture_layout(value)
            && !narrowing::assigned_names(later).contains(name)
            && (!dictionary || readonly_groupdict(later, name))
        {
            self.regex_locals.insert(*slot, groups);
        }
    }

    pub(super) fn required_group_key(&self, receiver: &Expr, key: &Expr) -> bool {
        let ExprKind::Str(name) = &key.kind else {
            return false;
        };
        self.capture_layout(receiver).is_some_and(|groups| {
            groups
                .iter()
                .any(|group| group.name.as_deref() == Some(name) && group.required)
        })
    }

    /// Accepts the optional flags parameter in either of its source spellings.
    fn regex_verbose(&self, call: &ast::ExprCall, required: usize) -> Result<bool, String> {
        let args = &call.arguments;
        let flags = match (args.args.get(required..), args.keywords.as_ref()) {
            (Some([]), []) => return Ok(false),
            (Some([flags]), []) => flags,
            (Some([]), [keyword]) if keyword.arg.as_deref() == Some("flags") => &keyword.value,
            _ => {
                return Err(error(
                    self.source,
                    call,
                    "expected positional regex arguments and at most one flags argument",
                ));
            }
        };
        if self.resolve(flags) == Some(Binding::Intrinsic(Intrinsic::RegexVerbose)) {
            return Ok(true);
        }
        if matches!(flags, ast::Expr::NumberLiteral(number) if matches!(&number.value, ast::Number::Int(value) if value.as_i64() == Some(0)))
        {
            return Ok(false);
        }
        Err(error(
            self.source,
            flags,
            "regex flags must be literal 0 or re.VERBOSE",
        ))
    }

    fn fixed_regex(&self, pattern: &ast::Expr, verbose: bool) -> Result<Expr, String> {
        let ast::Expr::StringLiteral(literal) = pattern else {
            return Err(error(
                self.source,
                pattern,
                "re.compile requires a literal str pattern",
            ));
        };
        check_string(literal, self.source)?;
        let pattern = crate::fixed_regex::translate(literal.value.to_str(), verbose)
            .map_err(|message| error(self.source, literal, &message))?;
        self.globals
            .regex_literals
            .borrow_mut()
            .entry(pattern.source.clone())
            .or_insert_with(|| pattern.groups.into());
        Ok(Expr {
            ty: Type::RegexPattern,
            kind: ExprKind::Builtin(Builtin::RegexCompile(pattern.source), Vec::new()),
        })
    }

    pub(super) fn compile_regex(&self, call: &ast::ExprCall) -> Result<Expr, String> {
        let verbose = self.regex_verbose(call, 1)?;
        self.fixed_regex(&call.arguments.args[0], verbose)
    }

    pub(super) fn search_regex(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        let verbose = self.regex_verbose(call, 2)?;
        let pattern = &call.arguments.args[0];
        let pattern = if matches!(pattern, ast::Expr::StringLiteral(_)) {
            self.fixed_regex(pattern, verbose)?
        } else {
            if verbose {
                return Err(error(
                    self.source,
                    pattern,
                    "re.search cannot apply flags to a compiled pattern",
                ));
            }
            self.expr_expected(pattern, Type::RegexPattern)?
        };
        self.pattern_search(pattern, &call.arguments.args[1], Builtin::RegexSearch)
    }

    fn pattern_search(
        &mut self,
        pattern: Expr,
        text: &ast::Expr,
        operation: Builtin,
    ) -> Result<Expr, String> {
        Ok(Expr {
            ty: self.types.optional(Type::RegexMatch),
            kind: ExprKind::Builtin(
                operation,
                vec![pattern, self.expr_expected(text, Type::Str)?],
            ),
        })
    }

    pub(super) fn regex_method(
        &mut self,
        receiver: Expr,
        attribute: &ast::ExprAttribute,
        call: &ast::ExprCall,
    ) -> Result<Expr, String> {
        self.reject_builtin_keywords(call)?;
        let method = attribute.attr.as_str();
        let args = call.arguments.args.as_ref();
        if receiver.ty == Type::RegexPattern {
            return match (method, args) {
                ("search", [text]) => self.pattern_search(receiver, text, Builtin::RegexSearch),
                ("match", [text]) => self.pattern_search(receiver, text, Builtin::RegexMatchStart),
                _ => Err(error(
                    self.source,
                    call,
                    "a regex pattern supports search(text) and match(text)",
                )),
            };
        }
        if method == "groupdict" && args.is_empty() {
            return Ok(Expr {
                ty: self.types.dict(Type::Str, self.types.optional(Type::Str)),
                kind: ExprKind::Builtin(Builtin::RegexGroupDict, vec![receiver]),
            });
        }
        if method == "groups" && args.is_empty() {
            let groups = self.capture_layout(&receiver).ok_or_else(|| {
                error(
                    self.source,
                    call,
                    "groups() requires a statically known fixed pattern",
                )
            })?;
            let slot = self.hidden(Type::RegexMatch);
            let fields: Vec<_> = groups
                .iter()
                .enumerate()
                .skip(1)
                .map(|(index, group)| Expr {
                    ty: if group.required {
                        Type::Str
                    } else {
                        self.types.optional(Type::Str)
                    },
                    kind: ExprKind::Builtin(
                        Builtin::RegexCapture {
                            operation: RegexCapture::Group,
                            known: true,
                        },
                        vec![
                            self.local(slot),
                            Expr {
                                ty: Type::Int,
                                kind: ExprKind::Int(index.to_string()),
                            },
                        ],
                    ),
                })
                .collect();
            let ty = self
                .types
                .tuple(fields.iter().map(|field| field.ty).collect());
            return Ok(Expr {
                ty,
                kind: ExprKind::Scope {
                    body: vec![Statement::Assign(slot, receiver)],
                    value: Box::new(Expr {
                        ty,
                        kind: ExprKind::Tuple(fields),
                    }),
                    clear: vec![slot],
                },
            });
        }
        if !matches!(method, "group" | "start" | "end") || args.len() > 1 {
            return Err(error(
                self.source,
                call,
                "a regex match supports group(), start(), end(), groups(), and groupdict()",
            ));
        }
        let mut selector = if let [arg] = args {
            calls::scalar_literal(arg, self.source)
                .ok()
                .filter(|value| matches!(value.ty, Type::Int | Type::Str))
                .ok_or_else(|| {
                    error(
                        self.source,
                        arg,
                        "regex groups require a literal integer index or string name",
                    )
                })?
        } else {
            Expr {
                ty: Type::Int,
                kind: ExprKind::Int("0".into()),
            }
        };
        if let ExprKind::Negate(inner) = &selector.kind
            && matches!(&inner.kind, ExprKind::Int(value) if value == "0")
        {
            selector.kind = ExprKind::Int("0".into());
        }
        let index = self.capture_index(&receiver, &selector);
        if let Some(index) = index {
            selector = Expr {
                ty: Type::Int,
                kind: ExprKind::Int(index.to_string()),
            };
        }
        let (operation, ty) = if method == "group" {
            let ty = if index == Some(0)
                || index.is_some_and(|index| {
                    self.capture_layout(&receiver)
                        .is_some_and(|groups| groups[index].required)
                }) {
                Type::Str
            } else {
                self.types.optional(Type::Str)
            };
            (RegexCapture::Group, ty)
        } else {
            (
                if method == "end" {
                    RegexCapture::End
                } else {
                    RegexCapture::Start
                },
                Type::Int,
            )
        };
        Ok(Expr {
            ty,
            kind: ExprKind::Builtin(
                Builtin::RegexCapture {
                    operation,
                    known: index.is_some(),
                },
                vec![receiver, selector],
            ),
        })
    }
}

/// A fresh group dictionary keeps its required keys while its only uses are
/// reads. An alias or mutation makes later lookups ordinary optional values.
fn readonly_groupdict(body: &[ast::Stmt], name: &str) -> bool {
    use ruff_python_ast::visitor::{Visitor, walk_expr};
    struct Reads<'a> {
        name: &'a str,
        escape: bool,
    }
    impl<'ast> Visitor<'ast> for Reads<'_> {
        fn visit_expr(&mut self, value: &'ast ast::Expr) {
            let local = |expr: &ast::Expr| matches!(expr, ast::Expr::Name(n) if n.id == self.name);
            match value {
                ast::Expr::Subscript(sub)
                    if sub.ctx == ast::ExprContext::Load && local(&sub.value) =>
                {
                    self.visit_expr(&sub.slice)
                }
                ast::Expr::Call(call) if matches!(&*call.func, ast::Expr::Attribute(attr) if attr.attr.as_str() == "get" && local(&attr.value)) => {
                    self.visit_arguments(&call.arguments)
                }
                _ if local(value) => self.escape = true,
                _ => walk_expr(self, value),
            }
        }
    }
    let mut reads = Reads {
        name,
        escape: false,
    };
    reads.visit_body(body);
    !reads.escape
}
