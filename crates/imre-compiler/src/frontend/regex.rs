//! Sealed text patterns are validated while checking the source, including dead code.

use super::*;

#[derive(Clone)]
pub(super) struct CaptureLayout {
    groups: std::rc::Rc<[crate::fixed_regex::Capture]>,
    nonempty_match: bool,
}

impl std::ops::Deref for CaptureLayout {
    type Target = [crate::fixed_regex::Capture];
    fn deref(&self) -> &Self::Target {
        &self.groups
    }
}

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
        lowered: &mut Vec<Statement>,
        later: &[ast::Stmt],
    ) {
        if self.block_depth != 1 || self.module_scope {
            return;
        }
        let [Statement::Assign(slot, value)] = lowered.as_slice() else {
            return;
        };
        let dictionary = matches!(value.kind, ExprKind::Builtin(Builtin::RegexGroupDict, _));
        if !dictionary
            && value.ty != Type::RegexPattern
            && !matches!(
                value.kind,
                ExprKind::Builtin(
                    Builtin::RegexCompile { .. } | Builtin::RegexSearch | Builtin::RegexMatchStart,
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
        // A previously defined local function may load this binding at its
        // next call even when the remaining source never mentions the name.
        if dictionary
            && self
                .local_functions
                .values()
                .any(|f| f.may_capture(*slot, name))
        {
            return;
        }
        let Some(groups) = self.capture_layout(value) else {
            return;
        };
        if narrowing::assigned_names(later).contains(name) {
            return;
        }
        let slot = *slot;
        let reads = dictionary.then(|| groupdict_reads(later, name));
        if reads.as_ref().is_some_and(|reads| reads.escapes) {
            return;
        }
        self.regex_locals.insert(slot, groups.clone());
        let Some(keys) = reads.and_then(|reads| reads.keys) else {
            return;
        };
        if keys.is_empty()
            || !keys
                .iter()
                .all(|key| groups.iter().any(|g| g.name.as_ref() == Some(key)))
        {
            return;
        }
        let ExprKind::Builtin(Builtin::RegexGroupDict, args) = &value.kind else {
            unreachable!()
        };
        let matched = self.hidden(Type::RegexMatch);
        let mut captures = vec![Statement::Assign(matched, args[0].clone())];
        let optional = self.types.optional(Type::Str);
        let mut fields = HashMap::new();
        for (index, group) in groups.iter().enumerate() {
            let Some(name) = group.name.as_ref().filter(|name| keys.contains(*name)) else {
                continue;
            };
            let field = self.hidden(optional);
            captures.push(Statement::Assign(
                field,
                Expr {
                    ty: optional,
                    kind: ExprKind::Builtin(
                        Builtin::RegexCapture {
                            operation: RegexCapture::Group,
                            known: true,
                        },
                        vec![
                            self.local(matched),
                            Expr {
                                ty: Type::Int,
                                kind: ExprKind::Int(index.to_string()),
                            },
                        ],
                    ),
                },
            ));
            fields.insert(name.clone(), field);
        }
        // Repeated reads observe the same captured string. The original match
        // local may be rebound, and this assignment may displace a previous dict.
        captures.extend([Statement::Clear(matched), Statement::Clear(slot)]);
        *lowered = captures;
        self.regex_fields.insert(slot, fields);
    }

    /// Resolves a static read only when the dictionary binding cannot escape.
    pub(super) fn projected_group(&self, dictionary: &Expr, key: &Expr) -> Option<Expr> {
        let (ExprKind::Local(slot), ExprKind::Str(name)) = (&dictionary.kind, &key.kind) else {
            return None;
        };
        Some(self.local(*self.regex_fields.get(slot)?.get(name)?))
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
    fn regex_flags(&self, call: &ast::ExprCall, required: usize) -> Result<u8, String> {
        let args = &call.arguments;
        let flags = match (args.args.get(required..), args.keywords.as_ref()) {
            (Some([]), []) => return Ok(0),
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
        self.fixed_flags(flags).ok_or_else(|| error(self.source, flags,
            "regex flags must be literal 0, re.VERBOSE or re.UNICODE, optionally combined with |"))
    }

    /// UNICODE is already the matching mode for a str pattern, but is still a
    /// nonzero flag when re.search receives an already compiled Pattern.
    fn fixed_flags(&self, expression: &ast::Expr) -> Option<u8> {
        match self.resolve(expression) {
            Some(Binding::Intrinsic(Intrinsic::RegexVerbose)) => return Some(64),
            Some(Binding::Intrinsic(Intrinsic::RegexUnicode)) => return Some(32),
            _ => {}
        }
        match expression {
            ast::Expr::BinOp(op) if op.op == ast::Operator::BitOr => {
                Some(self.fixed_flags(&op.left)? | self.fixed_flags(&op.right)?)
            }
            ast::Expr::NumberLiteral(n) if matches!(&n.value, ast::Number::Int(value) if value.as_i64() == Some(0)) => {
                Some(0)
            }
            _ => None,
        }
    }

    pub(super) fn fixed_regex(&self, pattern: &ast::Expr, verbose: bool) -> Result<Expr, String> {
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
            .or_insert_with(|| CaptureLayout {
                groups: pattern.groups.into(),
                nonempty_match: !pattern.may_match_empty,
            });
        Ok(Expr {
            ty: Type::RegexPattern,
            kind: ExprKind::Builtin(
                Builtin::RegexCompile {
                    source: pattern.source,
                    spelling: literal.value.to_str().to_owned(),
                },
                Vec::new(),
            ),
        })
    }

    pub(super) fn compile_regex(&self, call: &ast::ExprCall) -> Result<Expr, String> {
        let verbose = self.regex_flags(call, 1)? & 64 != 0;
        self.fixed_regex(&call.arguments.args[0], verbose)
    }

    pub(super) fn sub_regex(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        let (
            [
                pattern @ ast::Expr::StringLiteral(_),
                ast::Expr::StringLiteral(replacement),
                text,
            ],
            [],
        ) = (
            call.arguments.args.as_ref(),
            call.arguments.keywords.as_ref(),
        )
        else {
            return Err(error(
                self.source,
                call,
                "re.sub expects a literal str pattern, a literal str replacement, and a str input",
            ));
        };
        let pattern = self.fixed_regex(pattern, false)?;
        self.pattern_sub(pattern, replacement, text)
    }

    fn pattern_sub(
        &mut self,
        pattern: Expr,
        replacement: &ast::ExprStringLiteral,
        text: &ast::Expr,
    ) -> Result<Expr, String> {
        check_string(replacement, self.source)?;
        let groups = self
            .capture_layout(&pattern)
            .filter(|layout| layout.nonempty_match)
            .ok_or_else(|| {
                error(
                    self.source,
                    replacement,
                    "re.sub requires a known nonempty match",
                )
            })?;
        let template = crate::fixed_regex::replacement(replacement.value.to_str(), &groups)
            .map_err(|msg| error(self.source, replacement, msg))?;
        Ok(Expr {
            ty: Type::Str,
            kind: ExprKind::Builtin(
                Builtin::RegexSub(template),
                vec![pattern, self.expr_expected(text, Type::Str)?],
            ),
        })
    }

    pub(super) fn search_regex(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        let flags = self.regex_flags(call, 2)?;
        let pattern = &call.arguments.args[0];
        let pattern = if matches!(pattern, ast::Expr::StringLiteral(_)) {
            self.fixed_regex(pattern, flags & 64 != 0)?
        } else {
            if flags != 0 {
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
                ("finditer", [text]) => {
                    if !self
                        .capture_layout(&receiver)
                        .is_some_and(|layout| layout.nonempty_match)
                    {
                        return Err(error(
                            self.source,
                            call,
                            "finditer requires a known fixed pattern that cannot match an empty string",
                        ));
                    }
                    Ok(Expr {
                        ty: self.types.cursor(Type::RegexMatch),
                        kind: ExprKind::Builtin(
                            Builtin::RegexFindIter,
                            vec![receiver, self.expr_expected(text, Type::Str)?],
                        ),
                    })
                }
                ("sub", [ast::Expr::StringLiteral(replacement), text]) => {
                    self.pattern_sub(receiver, replacement, text)
                }
                ("search", [text]) => self.pattern_search(receiver, text, Builtin::RegexSearch),
                ("match", [text]) => self.pattern_search(receiver, text, Builtin::RegexMatchStart),
                _ => Err(error(
                    self.source,
                    call,
                    "a regex pattern supports search, match, finditer, and sub with a literal replacement",
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

/// An alias or mutation invalidates even required-key narrowing. Other reads
/// keep that narrowing but only literal, local lookups can replace the dictionary.
struct GroupdictReads {
    escapes: bool,
    keys: Option<HashSet<String>>,
}

fn groupdict_reads(body: &[ast::Stmt], name: &str) -> GroupdictReads {
    use ruff_python_ast::visitor::{Visitor, walk_expr, walk_stmt};
    struct Reads<'a> {
        name: &'a str,
        uses: GroupdictReads,
    }
    impl Reads<'_> {
        fn key(&mut self, expr: &ast::Expr) {
            if let ast::Expr::StringLiteral(text) = expr {
                if let Some(keys) = &mut self.uses.keys {
                    keys.insert(text.value.to_str().to_owned());
                }
            } else {
                self.uses.keys = None;
            }
        }
    }
    impl<'ast> Visitor<'ast> for Reads<'_> {
        fn visit_stmt(&mut self, stmt: &'ast ast::Stmt) {
            if matches!(stmt, ast::Stmt::FunctionDef(_) | ast::Stmt::ClassDef(_)) {
                self.uses.keys = None;
            }
            walk_stmt(self, stmt);
        }
        fn visit_expr(&mut self, value: &'ast ast::Expr) {
            let local = |expr: &ast::Expr| matches!(expr, ast::Expr::Name(n) if n.id == self.name);
            match value {
                ast::Expr::Subscript(sub)
                    if sub.ctx == ast::ExprContext::Load && local(&sub.value) =>
                {
                    self.key(&sub.slice);
                    self.visit_expr(&sub.slice)
                }
                ast::Expr::Call(call) if matches!(&*call.func, ast::Expr::Attribute(attr) if attr.attr.as_str() == "get" && local(&attr.value)) =>
                {
                    if let [key] = call.arguments.args.as_ref()
                        && call.arguments.keywords.is_empty()
                    {
                        self.key(key);
                    } else {
                        self.uses.keys = None;
                    }
                    self.visit_arguments(&call.arguments)
                }
                ast::Expr::ListComp(_)
                | ast::Expr::DictComp(_)
                | ast::Expr::SetComp(_)
                | ast::Expr::Generator(_)
                | ast::Expr::Lambda(_) => {
                    // Comprehensions and closures can move reads into another function.
                    self.uses.keys = None;
                    walk_expr(self, value);
                }
                _ if local(value) => self.uses.escapes = true,
                _ => walk_expr(self, value),
            }
        }
    }
    let mut reads = Reads {
        name,
        uses: GroupdictReads {
            escapes: false,
            keys: Some(HashSet::new()),
        },
    };
    reads.visit_body(body);
    reads.uses
}
