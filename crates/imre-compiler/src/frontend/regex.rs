//! Sealed literal patterns are validated while checking the source, including dead code.

use super::*;
use imre_regex::flags::{
    ASCII as ASCII_FLAG, MULTILINE as MULTILINE_FLAG, UNICODE as UNICODE_FLAG,
    VERBOSE as VERBOSE_FLAG,
};
use ruff_python_ast::UnaryOp;

/// A sealed literal must still be valid when checked in unreachable code.
fn boxed_lookahead(spelling: &str, verbose: bool, ascii: bool) -> bool {
    let alphabet = if ascii {
        imre_regex::Alphabet::Ascii
    } else {
        imre_regex::Alphabet::Unicode
    };
    let Ok((source, names)) = imre_regex::lookahead_source(spelling, verbose, alphabet, false)
    else {
        return false;
    };
    let mut seen = HashSet::new();
    let valid_names = names.iter().flatten().all(|name| {
        (ruff_python_stdlib::identifiers::is_identifier(name)
            || ruff_python_stdlib::keyword::is_keyword(name))
            && seen.insert(name)
    });
    valid_names && fancy_regex::Regex::new(&source).is_ok_and(|re| re.captures_len() == names.len())
}

#[derive(Clone)]
pub(super) struct CaptureLayout(pub(super) std::sync::Arc<crate::fixed_regex::FixedPattern>);

impl std::ops::Deref for CaptureLayout {
    type Target = [crate::fixed_regex::Capture];
    fn deref(&self) -> &Self::Target {
        &self.0.groups
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
    fn capture_index(layout: Option<&CaptureLayout>, selector: &mut Expr) -> Option<usize> {
        if matches!(&selector.kind, ExprKind::Int(n) if n == "0") {
            return Some(0);
        }
        let groups = layout?;
        match &selector.kind {
            ExprKind::Str(name) => {
                let index = groups
                    .iter()
                    .position(|group| group.name.as_deref() == Some(name))?;
                *selector = Expr::integer(index);
                Some(index)
            }
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
            && !matches!(value.ty, Type::RegexPattern | Type::RegexBytesPattern)
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
        let matched = self.hidden(args[0].ty);
        let mut captures = vec![Statement::Assign(matched, args[0].clone())];
        let optional = self.types.optional(if args[0].ty == Type::RegexBytesMatch {
            Type::Bytes
        } else {
            Type::Str
        });
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
                        vec![self.local(matched), Expr::integer(index)],
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
    fn regex_flags(&self, call: &ast::ExprCall, required: usize) -> Result<i64, String> {
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
        if self.debug_flags(flags) {
            // Emitting a fresh diagnostic uses the current Python stdout on each compile.
            return Err(self.scoped_builtin_error("re.DEBUG", flags));
        }
        let bits = self.fixed_flags(flags).ok_or_else(|| {
            // A runtime operand must be evaluated at the call site, with the real
            // Python callable already loaded. Do not use another Pattern's methods
            // as evidence about this call.
            if self.resolve(&call.func) == Some(Binding::Intrinsic(Intrinsic::RegexCompile))
                && ast::helpers::any_over_expr(flags, |operand| match operand {
                    ast::Expr::Call(_) => true,
                    ast::Expr::Name(_) => !matches!(self.resolve(operand),
                        Some(Binding::Intrinsic(_) | Binding::IntrinsicModule(_))),
                    _ => false,
                })
            {
                return self.scoped_builtin_error("re.compile", flags);
            }
            error(self.source, flags, "fixed regex flags must be literal integers or re.ASCII, re.MULTILINE, re.VERBOSE or re.UNICODE, optionally combined with |")
        })?;
        if bits & ASCII_FLAG != 0 && bits & UNICODE_FLAG != 0 {
            return Err(error(
                self.source,
                flags,
                "ASCII and UNICODE flags are incompatible",
            ));
        }
        Ok(bits)
    }

    fn debug_flags(&self, expression: &ast::Expr) -> bool {
        match expression {
            ast::Expr::BinOp(op) if op.op == ast::Operator::BitOr => {
                self.debug_flags(&op.left) || self.debug_flags(&op.right)
            }
            ast::Expr::NumberLiteral(n) => {
                matches!(&n.value, ast::Number::Int(value) if value.as_i64().is_some_and(|bits| bits & imre_regex::flags::DEBUG != 0))
            }
            _ => self.resolve(expression) == Some(Binding::Intrinsic(Intrinsic::REGEX_DEBUG)),
        }
    }

    /// UNICODE is already the matching mode for a str pattern, but is still a
    /// nonzero flag when re.search receives an already compiled Pattern.
    fn fixed_flags(&self, expression: &ast::Expr) -> Option<i64> {
        match self.resolve(expression) {
            Some(Binding::Intrinsic(Intrinsic::RegexVerbose)) => return Some(VERBOSE_FLAG),
            Some(Binding::Intrinsic(Intrinsic::RegexUnicode)) => return Some(UNICODE_FLAG),
            Some(Binding::Intrinsic(Intrinsic::RegexMultiline)) => return Some(MULTILINE_FLAG),
            Some(Binding::Intrinsic(Intrinsic::RegexAscii)) => return Some(ASCII_FLAG),
            _ => {}
        }
        match expression {
            ast::Expr::Call(call)
                if self.unshadowed_builtin(&call.func, "int")
                    && call.arguments.keywords.is_empty()
                    && let [flag] = call.arguments.args.as_ref() =>
            {
                self.fixed_flags(flag)
            }
            ast::Expr::BinOp(op) if op.op == ast::Operator::BitOr => {
                Some(self.fixed_flags(&op.left)? | self.fixed_flags(&op.right)?)
            }
            ast::Expr::NumberLiteral(n) => {
                let ast::Number::Int(value) = &n.value else {
                    return None;
                };
                let bits = value.as_i64()?;
                (bits & !(ASCII_FLAG | MULTILINE_FLAG | UNICODE_FLAG | VERBOSE_FLAG) == 0)
                    .then_some(bits)
            }
            _ => None,
        }
    }

    pub(super) fn fixed_regex(&self, pattern: &ast::Expr, flags: i64) -> Result<Expr, String> {
        let ascii = flags & ASCII_FLAG != 0;
        let verbose = flags & VERBOSE_FLAG != 0;
        let multiline = flags & MULTILINE_FLAG != 0;
        let (spelling, literal, alphabet, ty) = match pattern {
            ast::Expr::StringLiteral(literal) => {
                check_string(literal, self.source)?;
                (
                    std::borrow::Cow::Borrowed(literal.value.to_str()),
                    ir::Constant::Text(literal.value.to_str().as_bytes().to_vec()),
                    if ascii {
                        imre_regex::Alphabet::Ascii
                    } else {
                        imre_regex::Alphabet::Unicode
                    },
                    Type::RegexPattern,
                )
            }
            ast::Expr::BytesLiteral(literal) => {
                if flags & UNICODE_FLAG != 0 {
                    return Err(error(
                        self.source,
                        pattern,
                        "cannot use UNICODE flag with a bytes pattern",
                    ));
                }
                let bytes = literal.value.bytes().collect::<Vec<_>>();
                (
                    std::borrow::Cow::Owned(imre_regex::byte_pattern(&bytes)),
                    ir::Constant::Bytes(bytes),
                    imre_regex::Alphabet::Bytes,
                    Type::RegexBytesPattern,
                )
            }
            _ => {
                return Err(error(
                    self.source,
                    pattern,
                    "re.compile requires a literal str pattern",
                ));
            }
        };
        let bytes_pattern = ty == Type::RegexBytesPattern;
        let parsed = imre_regex::translate_pattern_with_line_mode(
            &spelling,
            verbose,
            alphabet,
            false,
            !bytes_pattern,
            multiline,
        )
        .map_err(|message| {
            if message == crate::fixed_regex::NEEDS_CLASS_WARNING {
                return self.object_builtin_error("regex class warning", pattern);
            }
            if alphabet == imre_regex::Alphabet::Bytes
                && imre_regex::validation_source(&spelling, verbose, alphabet, false).is_ok()
            {
                return self.scoped_builtin_error("re.compile", pattern);
            }
            if !bytes_pattern && !multiline && boxed_lookahead(&spelling, verbose, ascii) {
                return self.object_builtin_error("regex lookahead", pattern);
            }
            error(self.source, pattern, &message)
        })?;
        if ascii && !bytes_pattern && spelling.contains("(?u:") {
            return Err(self.scoped_builtin_error("re.compile", pattern));
        }
        Ok(Expr {
            ty,
            kind: ExprKind::Builtin(
                Builtin::RegexCompile {
                    spelling: literal,
                    supplied_flags: u16::try_from(flags).expect("fixed regex flag bits"),
                    pattern: std::sync::Arc::new(parsed),
                },
                Vec::new(),
            ),
        })
    }

    pub(super) fn compile_regex(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        let bits = self.regex_flags(call, 1)?;
        let Some(pattern) = call.arguments.args.first() else {
            return Err(error(self.source, call, "re.compile requires a pattern"));
        };
        if matches!(
            pattern,
            ast::Expr::StringLiteral(_) | ast::Expr::BytesLiteral(_)
        ) {
            return self.fixed_regex(pattern, bits);
        }
        if self.graph.session.is_regex_pattern(self.module, pattern) {
            let value = self.expr(pattern)?;
            if matches!(value.ty, Type::RegexPattern | Type::RegexBytesPattern) {
                if bits == 0 {
                    return Ok(value);
                }
                let exception = Expr {
                    ty: Type::Exception,
                    kind: ExprKind::Exception {
                        kind: exception_kind("ValueError").unwrap(),
                        message: Some(Box::new(Expr {
                            ty: Type::Str,
                            kind: ExprKind::Str(
                                "cannot process flags argument with a compiled pattern".into(),
                            ),
                        })),
                    },
                };
                return Ok(Expr {
                    ty: value.ty,
                    kind: ExprKind::Builtin(Builtin::RaiseException, vec![value, exception]),
                });
            }
        }
        if ast::helpers::is_constant(pattern) {
            return self.fixed_regex(pattern, bits);
        }
        Err(self.scoped_builtin_error("re.compile", pattern))
    }

    /// A greedy class with a known output shape has a single-pass collector. Other Python
    /// patterns and call signatures use the ordinary object implementation.
    pub(super) fn regex_runs(&mut self, call: &ast::ExprCall, split: bool) -> Result<Expr, String> {
        let name = if split { "re.split" } else { "re.findall" };
        let ([pattern, text], []) = (
            call.arguments.args.as_ref(),
            call.arguments.keywords.as_ref(),
        ) else {
            return Err(self.scoped_builtin_error(name, call));
        };
        let (pattern, kind) = match pattern {
            ast::Expr::StringLiteral(pattern) => {
                check_string(pattern, self.source)?;
                (
                    crate::fixed_regex::translate(pattern.value.to_str(), false),
                    Type::Str,
                )
            }
            ast::Expr::BytesLiteral(pattern) => {
                let bytes: Vec<_> = pattern.value.bytes().collect();
                let source = crate::fixed_regex::byte_pattern(&bytes);
                (
                    crate::fixed_regex::translate_pattern(
                        &source,
                        false,
                        crate::fixed_regex::Alphabet::Bytes,
                        false,
                        true,
                    ),
                    Type::Bytes,
                )
            }
            _ => return Err(self.scoped_builtin_error(name, call)),
        };
        let program = pattern
            .ok()
            .and_then(|pattern| {
                crate::fixed_regex::collection(&pattern, !split, kind == Type::Bytes)
            })
            .ok_or_else(|| self.scoped_builtin_error(name, call))?;
        let text = self.expr(text)?;
        if text.ty != kind {
            return Err(self.scoped_builtin_error(name, call));
        }
        Ok(self.regex_collection_expr(program, split, kind, text))
    }

    /// A bound findall/split with a proven literal layout can use exactly the
    /// collector already used by the corresponding module function. Evaluate
    /// the receiver before the subject, even when the caller discards the list.
    pub(super) fn compiled_regex_collection(
        &mut self,
        call: &ast::ExprCall,
        split: bool,
        method: &'static str,
    ) -> Result<Expr, String> {
        let attribute = call
            .func
            .as_attribute_expr()
            .expect("resolved Pattern method");
        let ([subject], []) = (
            call.arguments.args.as_ref(),
            call.arguments.keywords.as_ref(),
        ) else {
            return Err(self.scoped_builtin_error(method, call));
        };
        let receiver = self
            .expr(&attribute.value)
            .map_err(|_| self.scoped_builtin_error(method, call))?;
        let kind = match receiver.ty {
            Type::RegexPattern => Type::Str,
            Type::RegexBytesPattern => Type::Bytes,
            _ => return Err(self.scoped_builtin_error(method, call)),
        };
        let program = self
            .capture_layout(&receiver)
            .and_then(|layout| {
                crate::fixed_regex::collection(&layout.0, !split, kind == Type::Bytes)
            })
            .ok_or_else(|| self.scoped_builtin_error(method, call))?;
        let text = Self::string_value(
            self.expr(subject)
                .map_err(|_| self.scoped_builtin_error(method, call))?,
        );
        if text.ty != kind {
            return Err(self.scoped_builtin_error(method, call));
        }
        let value = self.regex_collection_expr(program, split, kind, text);
        Ok(Expr {
            ty: value.ty,
            kind: ExprKind::Scope {
                body: vec![Statement::Expr(receiver)],
                value: Box::new(value),
                clear: vec![],
            },
        })
    }

    fn regex_collection_expr(
        &mut self,
        program: imre_regex::Collection,
        split: bool,
        kind: Type,
        text: Expr,
    ) -> Expr {
        let element = match &program {
            imre_regex::Collection::Rows { fields, .. } => self.types.tuple(vec![kind; *fields]),
            _ => kind,
        };
        Expr {
            ty: self.types.list(element),
            kind: ExprKind::Builtin(Builtin::RegexCollect { program, split }, vec![text]),
        }
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
        let pattern = self.fixed_regex(pattern, 0)?;
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
            .filter(|layout| !layout.0.may_match_empty)
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

    pub(super) fn search_regex(
        &mut self,
        call: &ast::ExprCall,
        anchored: bool,
    ) -> Result<Expr, String> {
        let flags = self.regex_flags(call, 2)?;
        let pattern = &call.arguments.args[0];
        let pattern = if matches!(
            pattern,
            ast::Expr::StringLiteral(_) | ast::Expr::BytesLiteral(_)
        ) {
            self.fixed_regex(pattern, flags)?
        } else {
            let value = self.expr(pattern)?;
            if value.ty == Type::Str {
                if flags & MULTILINE_FLAG != 0 {
                    return Err(error(
                        self.source,
                        pattern,
                        "a runtime MULTILINE search requires a compiled pattern",
                    ));
                }
                if flags & ASCII_FLAG != 0 {
                    return Err(self.scoped_builtin_error(
                        if anchored { "re.match" } else { "re.search" },
                        call,
                    ));
                }
                // Evaluate both arguments before compiling the runtime pattern.
                let text = self.expr_expected(&call.arguments.args[1], Type::Str)?;
                return Ok(Expr {
                    ty: self.types.optional(Type::RegexMatch),
                    kind: ExprKind::Builtin(
                        Builtin::RegexSearchText {
                            anchored,
                            verbose: flags & VERBOSE_FLAG != 0,
                        },
                        vec![value, text],
                    ),
                });
            }
            if flags != 0 {
                return Err(error(
                    self.source,
                    pattern,
                    "cannot apply flags to a compiled pattern",
                ));
            }
            if value.ty != Type::RegexBytesPattern {
                self.expect(Type::RegexPattern, value.ty, pattern)?;
            }
            value
        };
        self.pattern_search(
            pattern,
            &call.arguments.args[1],
            if anchored {
                Builtin::RegexMatchStart
            } else {
                Builtin::RegexSearch
            },
        )
    }

    fn pattern_search(
        &mut self,
        pattern: Expr,
        text: &ast::Expr,
        operation: Builtin,
    ) -> Result<Expr, String> {
        let bytes_pattern = pattern.ty == Type::RegexBytesPattern;
        let subject_type = if bytes_pattern {
            Type::Bytes
        } else {
            Type::Str
        };
        let value = Self::string_value(self.expr(text)?);
        // Preserve the supplied argument's kind. The pattern's matcher owns
        // Python's wrong-subject exception after both operands have run.
        let value = if matches!(value.ty, Type::Str | Type::Bytes) {
            value
        } else {
            self.coerce(value, subject_type, text)?
        };
        Ok(Expr {
            ty: self.types.optional(if bytes_pattern {
                Type::RegexBytesMatch
            } else {
                Type::RegexMatch
            }),
            kind: ExprKind::Builtin(operation, vec![pattern, value]),
        })
    }

    pub(super) fn regex_method(
        &mut self,
        receiver: Expr,
        attribute: &ast::ExprAttribute,
        call: &ast::ExprCall,
    ) -> Result<Expr, String> {
        if receiver.ty == Type::RegexPattern && attribute.attr.as_str() == "findall" {
            return Err(self.object_builtin_error("re.Pattern.findall", call));
        }
        self.reject_builtin_keywords(call)?;
        let method = attribute.attr.as_str();
        let args = call.arguments.args.as_ref();
        if matches!(receiver.ty, Type::RegexPattern | Type::RegexBytesPattern) {
            return match (method, args) {
                ("finditer", [text]) if receiver.ty == Type::RegexPattern => {
                    if !self
                        .capture_layout(&receiver)
                        .is_some_and(|layout| !layout.0.may_match_empty)
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
                ("sub", [ast::Expr::StringLiteral(replacement), text])
                    if receiver.ty == Type::RegexPattern =>
                {
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
        // The implicit group zero exists for every match, independently of the pattern.
        let layout = match (method, args) {
            ("groups", []) | (_, [_]) => self.capture_layout(&receiver),
            _ => None,
        };
        let match_type = self.types.payload(receiver.ty).unwrap_or(receiver.ty);
        let group_type = if match_type == Type::RegexBytesMatch {
            Type::Bytes
        } else {
            Type::Str
        };
        let receiver = if self.types.payload(receiver.ty).is_some() {
            records::require_object_receiver(receiver, match_type, method)
        } else {
            receiver
        };
        if method == "groupdict" && args.is_empty() {
            return Ok(Expr {
                ty: self.types.dict(Type::Str, self.types.optional(group_type)),
                kind: ExprKind::Builtin(Builtin::RegexGroupDict, vec![receiver]),
            });
        }
        if method == "groups" && args.is_empty() {
            let groups = layout.ok_or_else(|| {
                error(
                    self.source,
                    call,
                    "groups() requires a statically known fixed pattern",
                )
            })?;
            let slot = self.hidden(match_type);
            let fields: Vec<_> = groups
                .iter()
                .enumerate()
                .skip(1)
                .map(|(index, group)| Expr {
                    ty: if group.required {
                        group_type
                    } else {
                        self.types.optional(group_type)
                    },
                    kind: ExprKind::Builtin(
                        Builtin::RegexCapture {
                            operation: RegexCapture::Group,
                            known: true,
                        },
                        vec![self.local(slot), Expr::integer(index)],
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
        if !matches!(method, "group" | "start" | "end" | "span") || args.len() > 1 {
            return Err(error(
                self.source,
                call,
                "a regex match supports group(), start(), end(), span(), groups(), and groupdict()",
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
            Expr::integer(0)
        };
        if let ExprKind::Unary(UnaryOp::USub, inner) = &selector.kind
            && matches!(&inner.kind, ExprKind::Int(value) if value == "0")
        {
            selector.kind = ExprKind::Int("0".into());
        }
        let index = Self::capture_index(layout.as_ref(), &mut selector);
        if method == "span" {
            let slot = self.hidden(match_type);
            let bounds = [RegexCapture::Start, RegexCapture::End]
                .into_iter()
                .map(|operation| Expr {
                    ty: Type::Int,
                    kind: ExprKind::Builtin(
                        Builtin::RegexCapture {
                            operation,
                            known: index.is_some(),
                        },
                        vec![self.local(slot), selector.clone()],
                    ),
                })
                .collect();
            let ty = self.types.tuple(vec![Type::Int, Type::Int]);
            return Ok(Expr {
                ty,
                kind: ExprKind::Scope {
                    body: vec![Statement::Assign(slot, receiver)],
                    value: Box::new(Expr {
                        ty,
                        kind: ExprKind::Tuple(bounds),
                    }),
                    clear: vec![slot],
                },
            });
        }
        let (operation, ty) = if method == "group" {
            let ty = if index == Some(0)
                || index.is_some_and(|index| {
                    layout.as_ref().is_some_and(|groups| groups[index].required)
                }) {
                group_type
            } else {
                self.types.optional(group_type)
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
