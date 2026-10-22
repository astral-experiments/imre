//! Structural patterns over closed storage, lowered to ordinary owned branches.

use ruff_python_ast::visitor::{Visitor, walk_expr};

use super::*;

#[derive(Clone)]
struct Flow {
    assigned: HashSet<usize>,
    narrowed: HashMap<usize, Type>,
}

impl Flow {
    fn read(checker: &Checker<'_>) -> Self {
        Self {
            assigned: checker.assigned.clone(),
            narrowed: checker.narrowed.clone(),
        }
    }

    fn restore(&self, checker: &mut Checker<'_>) {
        checker.assigned.clone_from(&self.assigned);
        checker.narrowed.clone_from(&self.narrowed);
    }

    fn intersect(&mut self, other: &Self) {
        self.assigned.retain(|slot| other.assigned.contains(slot));
        self.narrowed
            .retain(|slot, ty| other.narrowed.get(slot) == Some(ty));
    }
}

struct Capture {
    name: String,
    slot: usize,
    range: TextRange,
}

struct Pattern {
    body: Vec<Statement>,
    captures: Vec<Capture>,
    clear: Vec<usize>,
    /// A successful pattern can expose an optional payload or the None singleton.
    value: Expr,
    /// A failed None pattern proves that the captured subject is present.
    failure: Option<Type>,
}

impl Pattern {
    fn new(value: Expr) -> Self {
        Self {
            body: Vec::new(),
            captures: Vec::new(),
            clear: Vec::new(),
            value,
            failure: None,
        }
    }

    fn child(&mut self, success: Expr, child: Self) {
        self.body
            .push(Statement::If(success, child.body, Vec::new()));
        self.captures.extend(child.captures);
        self.clear.extend(child.clear);
    }
}

fn boolean(value: bool) -> Expr {
    Expr {
        ty: Type::Bool,
        kind: ExprKind::Bool(value),
    }
}

fn integer(value: impl ToString) -> Expr {
    Expr {
        ty: Type::Int,
        kind: ExprKind::Int(value.to_string()),
    }
}

fn none() -> Expr {
    Expr {
        ty: Type::None,
        kind: ExprKind::None,
    }
}

fn not(value: Expr) -> Expr {
    Expr {
        ty: Type::Bool,
        kind: ExprKind::Not(Box::new(value)),
    }
}

fn builtin(ty: Type, operation: Builtin, args: Vec<Expr>) -> Expr {
    Expr {
        ty,
        kind: ExprKind::Builtin(operation, args),
    }
}

fn compare(left: Expr, operation: Compare, right: Expr) -> Expr {
    Expr {
        ty: Type::Bool,
        kind: ExprKind::Compare(vec![left, right], vec![operation]),
    }
}

impl Checker<'_> {
    /// Keep the evaluated subject alive across failed guards, without introducing
    /// a synthetic loop that could intercept a user's break or continue.
    pub(super) fn match_statement(
        &mut self,
        branch: &ast::StmtMatch,
    ) -> Result<(Vec<Statement>, bool), String> {
        for (index, case) in branch.cases.iter().enumerate() {
            self.pattern_names(&case.pattern)?;
            if case.pattern.is_irrefutable()
                && case.guard.is_none()
                && index + 1 != branch.cases.len()
            {
                return Err(error(
                    self.source,
                    &case.pattern,
                    "an unguarded irrefutable pattern must be the last case",
                ));
            }
        }
        let mut value = self.expr(&branch.subject)?;
        let exhaustive = self.enum_exhaustive(value.ty, &branch.cases);
        if exhaustive && let ExprKind::Call(_, args) = &mut value.kind {
            args.exhaustive_enum_match = true;
        }
        // A named subject stores its target before yielding the same value.
        // Track that outer target with the same narrowing rules as a bare name.
        let visible = match &*branch.subject {
            ast::Expr::Named(named) => &*named.target,
            subject => subject,
        };
        let mut original = match visible {
            ast::Expr::Name(name) => self.names.get(name.id.as_str()).copied(),
            _ => None,
        };
        // Preserve an annotated optional shape even when its current value is
        // known: all pattern syntax and case bodies must still be checked.
        if let Some(slot) = original {
            value = self.coerce(value, self.locals[slot], &*branch.subject)?;
        }
        let subject = self.hidden(value.ty);
        let selected = self.hidden(Type::Bool);
        let mut body = vec![
            Statement::Assign(subject, value),
            Statement::Assign(selected, boolean(false)),
        ];
        let before = Flow::read(self);
        let mut pending = Some(before.clone());
        let mut exits = Vec::new();
        for case in &branch.cases {
            let entering = pending.take().expect("irrefutable cases are last");
            entering.restore(self);
            let success = self.hidden(Type::Bool);
            let value = self.match_subject(subject);
            let mut pattern = self.pattern(&case.pattern, value, success)?;

            let mut failure = entering.clone();
            if let Some(ty) = pattern.failure {
                self.match_fact(subject, original, ty);
                failure = Flow::read(self);
                entering.restore(self);
            }
            self.match_fact(subject, original, pattern.value.ty);
            let mut matched = Vec::new();
            for capture in &pattern.captures {
                let mut value = self.local(capture.slot);
                if let Some(&slot) = self.names.get(&capture.name) {
                    value = self.coerce(value, self.locals[slot], &capture.range)?;
                }
                let slot = self.declare(&capture.name, value.ty, &capture.range)?;
                self.assigned_value(slot, &value);
                matched.push(Statement::Assign(slot, value));
                if original == Some(slot) {
                    original = None;
                }
            }

            let guard = case
                .guard
                .as_deref()
                .map(|guard| self.condition(guard))
                .transpose()?;
            if let (Some(slot), Some(guard)) = (original, case.guard.as_deref()) {
                let name = self
                    .names
                    .iter()
                    .find_map(|(name, &local)| (local == slot).then_some(name.as_str()));
                if name.is_some_and(|name| expression_rebinds(guard, name)) {
                    original = None;
                }
            }
            let after_guard = Flow::read(self);
            if let Some(guard) = case.guard.as_deref() {
                self.apply_facts(guard, true);
            }
            let (case_body, falls) = self.block(&case.body)?;
            if falls {
                exits.push(Flow::read(self));
            }
            let mut chosen = vec![Statement::Assign(selected, boolean(true))];
            chosen.extend(case_body);
            if let Some(guard) = guard {
                matched.push(Statement::If(guard, chosen, Vec::new()));
                after_guard.restore(self);
                self.apply_facts(case.guard.as_deref().unwrap(), false);
                let mut next = Flow::read(self);
                if !case.pattern.is_irrefutable() {
                    next.intersect(&failure);
                }
                pending = Some(next);
            } else {
                matched.extend(chosen);
                if !case.pattern.is_irrefutable() {
                    pending = Some(failure);
                }
            }
            let mut tests = vec![Statement::Assign(success, boolean(true))];
            tests.append(&mut pattern.body);
            tests.push(Statement::If(self.local(success), matched, Vec::new()));
            pattern.clear.push(success);
            body.push(Statement::If(
                not(self.local(selected)),
                vec![Statement::Scope {
                    body: tests,
                    clear: pattern.clear,
                }],
                Vec::new(),
            ));
        }
        if !exhaustive {
            exits.extend(pending);
        }
        let falls = !exits.is_empty();
        if let Some(mut joined) = exits.pop() {
            for exit in exits {
                joined.intersect(&exit);
            }
            joined.restore(self);
        } else {
            before.restore(self);
        }
        self.narrowed.remove(&subject);
        Ok((
            vec![Statement::Scope {
                body,
                clear: vec![subject, selected],
            }],
            falls,
        ))
    }

    fn match_subject(&self, slot: usize) -> Expr {
        self.read_local(slot)
    }

    fn match_fact(&mut self, subject: usize, original: Option<usize>, ty: Type) {
        for slot in std::iter::once(subject).chain(original) {
            if let Some(payload) = self.types.payload(self.locals[slot])
                && (ty == Type::None || ty == payload)
            {
                self.narrowed.insert(slot, ty);
            }
        }
    }

    /// Capture temporaries are committed only after the entire pattern succeeds.
    /// Python leaves failed-pattern partial bindings unspecified; guard failure,
    /// in contrast, must preserve all successfully committed bindings.
    fn capture(&mut self, pattern: &mut Pattern, name: &ast::Identifier, value: Expr) {
        let slot = self.hidden(value.ty);
        pattern.body.push(Statement::Assign(slot, value));
        pattern.clear.push(slot);
        pattern.captures.push(Capture {
            name: name.to_string(),
            slot,
            range: name.range(),
        });
    }

    fn pattern(
        &mut self,
        node: &ast::Pattern,
        value: Expr,
        success: usize,
    ) -> Result<Pattern, String> {
        if let ast::Pattern::MatchAs(as_) = node {
            let mut pattern = if let Some(inner) = &as_.pattern {
                self.pattern(inner, value, success)?
            } else {
                Pattern::new(value)
            };
            if let Some(name) = &as_.name {
                let mut capture = Pattern::new(pattern.value.clone());
                self.capture(&mut capture, name, pattern.value.clone());
                pattern.child(self.local(success), capture);
            }
            return Ok(pattern);
        }
        if let ast::Pattern::MatchOr(or) = node {
            return self.or_pattern(or, value, success);
        }
        if let ast::Pattern::MatchSingleton(singleton) = node
            && singleton.value == ast::Singleton::None
        {
            let mut pattern = Pattern::new(none());
            pattern.failure = self.types.payload(value.ty);
            pattern.body.push(Statement::Assign(
                success,
                builtin(Type::Bool, Builtin::IsNone, vec![value]),
            ));
            return Ok(pattern);
        }
        if let Some(payload) = self.types.payload(value.ty) {
            let slot = self.hidden(payload);
            let mut pattern = self.pattern(node, self.local(slot), success)?;
            let mut present = vec![Statement::Assign(
                slot,
                Expr {
                    ty: payload,
                    kind: ExprKind::Unwrap(Box::new(value.clone())),
                },
            )];
            present.append(&mut pattern.body);
            pattern.body = vec![
                Statement::Assign(
                    success,
                    not(builtin(Type::Bool, Builtin::IsNone, vec![value])),
                ),
                Statement::If(self.local(success), present, Vec::new()),
            ];
            pattern.clear.push(slot);
            pattern.failure = None;
            return Ok(pattern);
        }
        let mut pattern = Pattern::new(value.clone());
        match node {
            ast::Pattern::MatchValue(literal) => {
                let literal = self.pattern_literal(&literal.value)?;
                pattern.body.push(Statement::Assign(
                    success,
                    self.pattern_equal(value, literal),
                ));
            }
            ast::Pattern::MatchSingleton(singleton) => {
                let test = if value.ty == Type::Bool {
                    compare(
                        value,
                        Compare::Equal,
                        boolean(singleton.value == ast::Singleton::True),
                    )
                } else {
                    boolean(false)
                };
                pattern.body.push(Statement::Assign(success, test));
            }
            ast::Pattern::MatchSequence(sequence) => {
                return self.sequence_pattern(sequence, value, success);
            }
            ast::Pattern::MatchMapping(mapping) => {
                return self.mapping_pattern(mapping, value, success);
            }
            ast::Pattern::MatchClass(class) => {
                return self.class_pattern(class, value, success);
            }
            ast::Pattern::MatchStar(_) => {
                return Err(error(
                    self.source,
                    node,
                    "a starred pattern requires a sequence",
                ));
            }
            ast::Pattern::MatchAs(_) | ast::Pattern::MatchOr(_) => unreachable!(),
        }
        Ok(pattern)
    }

    /// Numeric value patterns use equality: `case 1` matches True, whereas
    /// singleton `case True` only matches a bool. Literal lowering remains shared
    /// with expressions so new scalar types need no separate literal parser.
    fn pattern_equal(&self, mut value: Expr, mut literal: Expr) -> Expr {
        if value.ty != literal.ty {
            value = Self::string_value(value);
            literal = Self::string_value(literal);
        }
        if value.ty == Type::Bool && matches!(literal.ty, Type::Int | Type::Float) {
            value = builtin(Type::Int, Builtin::Int, vec![value]);
        }
        if value.ty == literal.ty
            || (matches!(value.ty, Type::Int | Type::Float)
                && matches!(literal.ty, Type::Int | Type::Float))
        {
            compare(value, Compare::Equal, literal)
        } else {
            boolean(false)
        }
    }

    fn pattern_literal(&mut self, literal: &ast::Expr) -> Result<Expr, String> {
        if let Some(member) = self.qualified_member(literal) {
            return Ok(member);
        }
        let allowed = match literal {
            ast::Expr::NumberLiteral(_)
            | ast::Expr::StringLiteral(_)
            | ast::Expr::BytesLiteral(_)
            | ast::Expr::BooleanLiteral(_)
            | ast::Expr::NoneLiteral(_) => true,
            ast::Expr::UnaryOp(unary) => {
                matches!(unary.op, ast::UnaryOp::UAdd | ast::UnaryOp::USub)
                    && matches!(&*unary.operand, ast::Expr::NumberLiteral(_))
            }
            _ => false,
        };
        if !allowed {
            return Err(error(
                self.source,
                literal,
                "patterns require scalar literals; dynamic value-pattern lookups are unsupported",
            ));
        }
        self.expr(literal)
    }

    fn pattern_join(&self, left: Type, right: Type, node: &impl Ranged) -> Result<Type, String> {
        if left == right || self.types.payload(left) == Some(right) {
            Ok(left)
        } else if self.types.payload(right) == Some(left) {
            Ok(right)
        } else if left == Type::None {
            Ok(self.types.optional(right))
        } else if right == Type::None {
            Ok(self.types.optional(left))
        } else {
            Err(error(
                self.source,
                node,
                "pattern captures require one concrete type or T | None",
            ))
        }
    }

    fn or_pattern(
        &mut self,
        or: &ast::PatternMatchOr,
        value: Expr,
        success: usize,
    ) -> Result<Pattern, String> {
        let mut pattern = Pattern::new(value.clone());
        pattern
            .body
            .push(Statement::Assign(success, boolean(false)));
        let mut alternatives = Vec::new();
        let mut captures: Vec<(String, Type, TextRange)> = Vec::new();
        let mut matched_type = None;
        for alternative in &or.patterns {
            let ready = self.hidden(Type::Bool);
            let alternative = self.pattern(alternative, value.clone(), ready)?;
            matched_type = Some(match matched_type {
                Some(ty) => self.pattern_join(ty, alternative.value.ty, or)?,
                None => alternative.value.ty,
            });
            pattern.failure = pattern.failure.or(alternative.failure);
            for capture in &alternative.captures {
                let ty = self.locals[capture.slot];
                if let Some((_, previous, _)) = captures
                    .iter_mut()
                    .find(|(name, _, _)| name == &capture.name)
                {
                    *previous = self.pattern_join(*previous, ty, &capture.range)?;
                } else {
                    captures.push((capture.name.clone(), ty, capture.range));
                }
            }
            pattern.clear.push(ready);
            alternatives.push((ready, alternative));
        }
        for (name, ty, range) in captures {
            let slot = self.hidden(ty);
            pattern.clear.push(slot);
            pattern.captures.push(Capture { name, slot, range });
        }
        for (ready, alternative) in alternatives {
            let mut body = vec![Statement::Assign(ready, boolean(true))];
            body.extend(alternative.body);
            let mut commit = Vec::new();
            for capture in &pattern.captures {
                let source = alternative
                    .captures
                    .iter()
                    .find(|source| source.name == capture.name)
                    .unwrap();
                commit.push(Statement::Assign(
                    capture.slot,
                    self.coerce(
                        self.local(source.slot),
                        self.locals[capture.slot],
                        &capture.range,
                    )?,
                ));
            }
            commit.push(Statement::Assign(success, boolean(true)));
            body.push(Statement::If(self.local(ready), commit, Vec::new()));
            pattern
                .body
                .push(Statement::If(not(self.local(success)), body, Vec::new()));
            pattern.clear.extend(alternative.clear);
        }
        let ty = matched_type.unwrap();
        pattern.value = if ty == Type::None {
            none()
        } else if self.types.payload(value.ty) == Some(ty) {
            Expr {
                ty,
                kind: ExprKind::Unwrap(Box::new(value)),
            }
        } else {
            self.coerce(value, ty, or)?
        };
        Ok(pattern)
    }

    fn sequence_pattern(
        &mut self,
        sequence: &ast::PatternMatchSequence,
        value: Expr,
        success: usize,
    ) -> Result<Pattern, String> {
        let element = self.types.element(value.ty);
        let fields = self.types.fields(value.ty);
        if element.is_none() && fields.is_none() {
            return Err(error(
                self.source,
                sequence,
                "sequence patterns require a concrete list or fixed tuple",
            ));
        }
        let star = sequence
            .patterns
            .iter()
            .position(|pattern| matches!(pattern, ast::Pattern::MatchStar(_)));
        if sequence
            .patterns
            .iter()
            .filter(|pattern| matches!(pattern, ast::Pattern::MatchStar(_)))
            .count()
            > 1
        {
            return Err(error(
                self.source,
                sequence,
                "a sequence pattern can contain only one starred rest",
            ));
        }
        let fixed = sequence.patterns.len() - usize::from(star.is_some());
        if fields.as_ref().is_some_and(|fields| fields.len() < fixed) {
            return Err(error(
                self.source,
                sequence,
                "sequence pattern exceeds the fixed tuple's declared shape",
            ));
        }
        let mut pattern = Pattern::new(value.clone());
        pattern.body.push(Statement::Assign(
            success,
            compare(
                builtin(Type::Int, Builtin::Len, vec![value.clone()]),
                if star.is_some() {
                    Compare::GreaterEqual
                } else {
                    Compare::Equal
                },
                integer(fixed),
            ),
        ));
        for (index, child) in sequence.patterns.iter().enumerate() {
            if let ast::Pattern::MatchStar(starred) = child {
                if let Some(name) = &starred.name {
                    let suffix = sequence.patterns.len() - index - 1;
                    let rest = if let Some(element) = element {
                        let bound = |value: Option<i64>| Expr {
                            ty: self.types.optional(Type::Int),
                            kind: match value {
                                Some(value) => ExprKind::Present(Box::new(integer(value))),
                                None => ExprKind::Absent(Box::new(none())),
                            },
                        };
                        builtin(
                            self.types.list(element),
                            Builtin::Slice,
                            vec![
                                value.clone(),
                                bound(Some(index as i64)),
                                bound((suffix > 0).then_some(-(suffix as i64))),
                                bound(None),
                            ],
                        )
                    } else {
                        let fields = fields.as_ref().unwrap();
                        let remaining = &fields[index..fields.len() - suffix];
                        let context = self.names.get(name.as_str()).and_then(|&slot| {
                            let ty = self.locals[slot];
                            self.types.element(self.types.payload(ty).unwrap_or(ty))
                        });
                        let mut element =
                            context.or(remaining.first().copied()).unwrap_or(Type::Int);
                        if context.is_none() {
                            for &ty in remaining {
                                element = self.pattern_join(element, ty, starred)?;
                            }
                        }
                        let elements = remaining
                            .iter()
                            .enumerate()
                            .map(|(offset, &ty)| {
                                self.coerce(
                                    Expr {
                                        ty,
                                        kind: ExprKind::Field(
                                            Box::new(value.clone()),
                                            index + offset,
                                        ),
                                    },
                                    element,
                                    starred,
                                )
                            })
                            .collect::<Result<Vec<_>, _>>()?;
                        Expr {
                            ty: self.types.list(element),
                            kind: ExprKind::List(elements),
                        }
                    };
                    let mut capture = Pattern::new(rest.clone());
                    self.capture(&mut capture, name, rest);
                    pattern.child(self.local(success), capture);
                }
                continue;
            }
            let position = if star.is_some_and(|star| index > star) {
                -(sequence.patterns.len() as i64 - index as i64)
            } else {
                index as i64
            };
            let item = if let Some(element) = element {
                builtin(
                    element,
                    Builtin::ListGet,
                    vec![value.clone(), integer(position)],
                )
            } else {
                let fields = fields.as_ref().unwrap();
                let position = if position < 0 {
                    (fields.len() as i64 + position) as usize
                } else {
                    position as usize
                };
                Expr {
                    ty: fields[position],
                    kind: ExprKind::Field(Box::new(value.clone()), position),
                }
            };
            let slot = self.hidden(item.ty);
            let mut child = self.pattern(child, self.local(slot), success)?;
            child.body.insert(0, Statement::Assign(slot, item));
            child.clear.push(slot);
            pattern.child(self.local(success), child);
        }
        Ok(pattern)
    }

    fn mapping_pattern(
        &mut self,
        mapping: &ast::PatternMatchMapping,
        value: Expr,
        success: usize,
    ) -> Result<Pattern, String> {
        let (key_type, value_type) = self.types.mapping(value.ty).ok_or_else(|| {
            error(
                self.source,
                mapping,
                "mapping patterns require a concrete dictionary",
            )
        })?;
        let mut pattern = Pattern::new(value.clone());
        let mut keys = Vec::new();
        let mut seen = HashSet::new();
        for (key, child) in mapping.keys.iter().zip(&mapping.patterns) {
            let key = mapping_key(self.pattern_literal(key)?);
            if let Some(key) = literal_key(&key)
                && !seen.insert(key)
            {
                return Err(error(
                    self.source,
                    mapping,
                    "duplicate literal key in mapping pattern",
                ));
            }
            let slot = self.hidden(value_type);
            let mut child = self.pattern(child, self.local(slot), success)?;
            child.body.insert(
                0,
                Statement::Assign(
                    slot,
                    builtin(
                        value_type,
                        Builtin::DictGet,
                        vec![value.clone(), key.clone()],
                    ),
                ),
            );
            child.clear.push(slot);
            if key.ty == key_type {
                pattern.body.push(Statement::If(
                    self.local(success),
                    vec![Statement::Assign(
                        success,
                        builtin(
                            Type::Bool,
                            Builtin::DictContains,
                            vec![key.clone(), value.clone()],
                        ),
                    )],
                    Vec::new(),
                ));
                pattern.child(self.local(success), child);
                keys.push(key);
            } else {
                // A different sealed key domain can never be present. Still
                // check the nested pattern and keep its statically typed binds.
                child.body.clear();
                child.body.push(Statement::Assign(success, boolean(false)));
                pattern.child(self.local(success), child);
            }
        }
        if let Some(name) = &mapping.rest {
            let mut rest = self.mapping_rest(value, keys, key_type, value_type);
            let value = rest.value.clone();
            self.capture(&mut rest, name, value);
            pattern.child(self.local(success), rest);
        }
        Ok(pattern)
    }

    /// Materialize **rest with the existing typed iterator and writes: the copy
    /// preserves insertion order and retains each value before any guard mutates
    /// the source, while contained objects keep their original identities.
    fn mapping_rest(
        &mut self,
        value: Expr,
        keys: Vec<Expr>,
        key_type: Type,
        value_type: Type,
    ) -> Pattern {
        let result = self.hidden(value.ty);
        let iterator = self.hidden(Type::Iterator);
        let key = self.hidden(key_type);
        let item = self.hidden(value_type);
        let ready = self.hidden(Type::Bool);
        let mut pattern = Pattern::new(self.local(result));
        pattern.clear.extend([result, iterator, key, item, ready]);
        pattern.body = vec![
            Statement::Assign(
                result,
                Expr {
                    ty: value.ty,
                    kind: ExprKind::Dict {
                        key: key_type,
                        value: value_type,
                        entries: Vec::new(),
                    },
                },
            ),
            Statement::Assign(
                iterator,
                builtin(Type::Iterator, Builtin::DictIter(true), vec![value]),
            ),
            Statement::While(
                boolean(true),
                vec![
                    Statement::IterNext {
                        iterator,
                        key,
                        value: Some(item),
                        ready,
                    },
                    Statement::If(not(self.local(ready)), vec![Statement::Break], Vec::new()),
                    Statement::SetItem(self.local(result), self.local(key), self.local(item)),
                    Statement::Clear(key),
                    Statement::Clear(item),
                ],
                Vec::new(),
            ),
        ];
        pattern.body.extend(keys.into_iter().map(|key| {
            Statement::Expr(builtin(
                Type::None,
                Builtin::DictDelete,
                vec![self.local(result), key],
            ))
        }));
        pattern
    }

    fn class_pattern(
        &mut self,
        class: &ast::PatternMatchClass,
        value: Expr,
        success: usize,
    ) -> Result<Pattern, String> {
        if !class.arguments.patterns.is_empty() {
            return Err(error(
                self.source,
                class,
                "positional class patterns and __match_args__ are unsupported; use declared field keywords",
            ));
        }
        let id = match self.resolve(&class.cls) {
            Some(Binding::Definition(definition)) => {
                self.classes.declarations.ids.get(&definition).copied()
            }
            _ => None,
        }
        .ok_or_else(|| {
            error(
                self.source,
                class,
                "class patterns require a statically resolved declared class",
            )
        })?;
        if self.classes.declarations.definitions[id]
            .enum_members
            .is_some()
        {
            return Err(error(
                self.source,
                class,
                "StrEnum patterns require qualified member names",
            ));
        }
        let mut pattern = Pattern::new(value.clone());
        let value = if value.ty == Type::Class(id) {
            value
        } else {
            // The nominal identities cannot match. A cleanup-safe, never-read
            // root still lets the unreachable fields and captures keep their
            // declared types; all accesses are gated by this false result.
            let unreachable = self.hidden(Type::Class(id));
            pattern.clear.push(unreachable);
            pattern
                .body
                .push(Statement::Assign(success, boolean(false)));
            self.local(unreachable)
        };
        let mut seen = HashSet::new();
        for keyword in &class.arguments.keywords {
            if !seen.insert(keyword.attr.as_str()) {
                return Err(error(
                    self.source,
                    keyword,
                    "duplicate field in class pattern",
                ));
            }
            let field = self.classes.declarations.definitions[id]
                .fields
                .iter()
                .position(|field| field.name == keyword.attr.as_str())
                .ok_or_else(|| {
                    error(
                        self.source,
                        keyword,
                        "class patterns require a declared field",
                    )
                })?;
            let ty = self.classes.fields[id][field];
            let slot = self.hidden(ty);
            let mut child = self.pattern(&keyword.pattern, self.local(slot), success)?;
            child.body.insert(
                0,
                Statement::Assign(
                    slot,
                    Expr {
                        ty,
                        kind: ExprKind::Field(Box::new(value.clone()), field),
                    },
                ),
            );
            child.clear.push(slot);
            pattern.child(self.local(success), child);
        }
        Ok(pattern)
    }

    /// Validate binding structure independently of runtime success, including
    /// alternatives and dead cases whose values will never be extracted.
    fn pattern_names(&self, pattern: &ast::Pattern) -> Result<HashSet<String>, String> {
        let mut names = HashSet::new();
        let mut add = |name: &ast::Identifier| {
            if names.insert(name.to_string()) {
                Ok(())
            } else {
                Err(error(
                    self.source,
                    name,
                    "a pattern cannot capture the same name twice",
                ))
            }
        };
        let children: Vec<&ast::Pattern> = match pattern {
            ast::Pattern::MatchAs(pattern) => {
                if let Some(name) = &pattern.name {
                    add(name)?;
                }
                pattern.pattern.iter().map(|pattern| &**pattern).collect()
            }
            ast::Pattern::MatchStar(pattern) => {
                if let Some(name) = &pattern.name {
                    add(name)?;
                }
                Vec::new()
            }
            ast::Pattern::MatchSequence(pattern) => pattern.patterns.iter().collect(),
            ast::Pattern::MatchMapping(pattern) => {
                if let Some(name) = &pattern.rest {
                    add(name)?;
                }
                pattern.patterns.iter().collect()
            }
            ast::Pattern::MatchClass(pattern) => pattern
                .arguments
                .patterns
                .iter()
                .chain(
                    pattern
                        .arguments
                        .keywords
                        .iter()
                        .map(|keyword| &keyword.pattern),
                )
                .collect(),
            ast::Pattern::MatchOr(or) => {
                let mut expected = None;
                for (index, pattern) in or.patterns.iter().enumerate() {
                    let names = self.pattern_names(pattern)?;
                    if expected.as_ref().is_some_and(|expected| expected != &names) {
                        return Err(error(
                            self.source,
                            pattern,
                            "OR patterns must bind the same names in every alternative",
                        ));
                    }
                    if pattern.is_irrefutable() && index + 1 != or.patterns.len() {
                        return Err(error(
                            self.source,
                            pattern,
                            "an irrefutable OR alternative must be last",
                        ));
                    }
                    expected = Some(names);
                }
                return Ok(expected.unwrap_or_default());
            }
            ast::Pattern::MatchValue(_) | ast::Pattern::MatchSingleton(_) => Vec::new(),
        };
        for child in children {
            for name in self.pattern_names(child)? {
                if !names.insert(name) {
                    return Err(error(
                        self.source,
                        child,
                        "a pattern cannot capture the same name twice",
                    ));
                }
            }
        }
        Ok(names)
    }
}

/// Canonicalize equal numeric keys without introducing a runtime conversion.
/// Fixed precision renders every digit of an integral binary64 value, including
/// values beyond machine integers; shortest decimal formatting would lose that.
fn mapping_key(value: Expr) -> Expr {
    if let ExprKind::Bool(value) = value.kind {
        return integer(usize::from(value));
    }
    let number = match &value.kind {
        ExprKind::Float(value) => *value,
        ExprKind::Negate(inner) => match &inner.kind {
            ExprKind::Float(value) => -*value,
            _ => return value,
        },
        _ => return value,
    };
    if number.is_finite() && number.fract() == 0.0 {
        integer(if number == 0.0 {
            "0".to_owned()
        } else {
            format!("{number:.0}")
        })
    } else {
        Expr {
            ty: Type::Float,
            kind: ExprKind::Float(number),
        }
    }
}

fn literal_key(value: &Expr) -> Option<String> {
    match &value.kind {
        ExprKind::Int(value) => Some(format!("int:{value}")),
        ExprKind::Float(value) => Some(format!("float:{:016x}", value.to_bits())),
        ExprKind::Str(value) => Some(format!("str:{value}")),
        ExprKind::Bytes(value) => Some(format!("bytes:{value:?}")),
        ExprKind::Negate(value) => match &value.kind {
            ExprKind::Int(value) if value == "0" => Some("int:0".to_owned()),
            ExprKind::Int(value) => Some(format!("int:-{value}")),
            _ => None,
        },
        ExprKind::None => Some("None".to_owned()),
        _ => None,
    }
}

/// A guard can rebind the visible subject while later cases still inspect the
/// original hidden owner. Never transfer its optional facts onto that new value.
/// Comprehension iteration targets belong to a private scope; only assignment
/// expressions can write the enclosing function's locals from an expression.
fn expression_rebinds(expression: &ast::Expr, name: &str) -> bool {
    struct Stores<'a> {
        name: &'a str,
        found: bool,
    }
    impl<'a> Visitor<'a> for Stores<'_> {
        fn visit_expr(&mut self, expression: &'a ast::Expr) {
            if let ast::Expr::Named(named) = expression
                && let ast::Expr::Name(name) = &*named.target
                && name.id.as_str() == self.name
            {
                self.found = true;
            }
            walk_expr(self, expression);
        }
    }
    let mut stores = Stores { name, found: false };
    stores.visit_expr(expression);
    stores.found
}
