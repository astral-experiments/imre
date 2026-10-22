//! Sealed text patterns are validated while checking the source, including dead code.

use super::*;

pub(super) type CaptureNames = std::rc::Rc<[Option<String>]>;

impl Checker<'_> {
    /// A successful search with an unchanged local has the fixed pattern's
    /// group layout throughout this arm, including any nested control flow.
    pub(super) fn regex_guard(
        &self,
        condition: &Expr,
        body: &[ast::Stmt],
    ) -> Option<(usize, CaptureNames)> {
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
        let ExprKind::Builtin(Builtin::RegexSearch, search) = &rhs.kind else {
            return None;
        };
        let groups = self.globals.regex_groups(&search[0])?;
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
        let ExprKind::Unwrap(inner) = &receiver.kind else {
            return None;
        };
        let ExprKind::Local(slot) = inner.kind else {
            return None;
        };
        let (_, groups) = self
            .regex_guards
            .iter()
            .rev()
            .find(|(saved, _)| *saved == slot)?;
        match &selector.kind {
            ExprKind::Str(name) => groups
                .iter()
                .position(|group| group.as_deref() == Some(name)),
            ExprKind::Int(index) => index.parse::<usize>().ok().filter(|n| *n < groups.len()),
            _ => None,
        }
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
        self.pattern_search(pattern, &call.arguments.args[1])
    }

    fn pattern_search(&mut self, pattern: Expr, text: &ast::Expr) -> Result<Expr, String> {
        Ok(Expr {
            ty: self.types.optional(Type::RegexMatch),
            kind: ExprKind::Builtin(
                Builtin::RegexSearch,
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
                ("search", [text]) => self.pattern_search(receiver, text),
                _ => Err(error(
                    self.source,
                    call,
                    "a regex pattern supports search(text)",
                )),
            };
        }
        if !matches!(method, "group" | "start" | "end") || args.len() > 1 {
            return Err(error(
                self.source,
                call,
                "a regex match supports group(), start(), or end() with one optional literal group",
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
            // Group zero always participates, even when the entire match is empty.
            let ty = if matches!(&selector.kind, ExprKind::Int(value) if value == "0") {
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
