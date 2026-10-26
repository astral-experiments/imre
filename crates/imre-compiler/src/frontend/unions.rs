//! Closed type tests refine local values without refining aliased heap storage.

use super::*;
use crate::ir::InstanceType;

impl Checker<'_> {
    /// int() on a closed string/integer value preserves the integer or parses the
    /// string. Keep the original value alive and evaluate it only once.
    pub(super) fn integer_from_text_or_int(&mut self, value: Expr) -> Expr {
        let (slot, test, text, integer) = self.protocol_alternatives(value.ty).unwrap();
        debug_assert_eq!((text.ty, integer.ty), (Type::Str, Type::Int));
        let parsed = Expr {
            ty: Type::Int,
            kind: ExprKind::Builtin(Builtin::Int, vec![text]),
        };
        Expr {
            ty: Type::Int,
            kind: ExprKind::Scope {
                body: vec![Statement::Assign(slot, value)],
                value: Box::new(Expr {
                    ty: Type::Int,
                    kind: ExprKind::Conditional(
                        Box::new(test),
                        Box::new(parsed),
                        Box::new(integer),
                    ),
                }),
                clear: vec![slot],
            },
        }
    }

    /// Store a protocol operand once, then provide the test and exact values for
    /// its two possible layouts. The caller assigns the operand to the returned slot.
    pub(super) fn protocol_alternatives(&mut self, ty: Type) -> Option<(usize, Expr, Expr, Expr)> {
        if self.types.payload(ty).is_none() && self.types.members(ty).is_none() {
            return None;
        }
        let slot = self.hidden(ty);
        let (test, first, second) = if let Some(payload) = self.types.payload(ty) {
            (
                Expr {
                    ty: Type::Bool,
                    kind: ExprKind::Builtin(Builtin::IsNone, vec![self.local(slot)]),
                },
                Expr {
                    ty: Type::None,
                    kind: ExprKind::None,
                },
                Expr {
                    ty: payload,
                    kind: ExprKind::Unwrap(Box::new(self.local(slot))),
                },
            )
        } else {
            let members = self.types.members(ty).unwrap();
            let project = |ty| Expr {
                ty,
                kind: ExprKind::Builtin(Builtin::UnionProject, vec![self.local(slot)]),
            };
            (
                Expr {
                    ty: Type::Bool,
                    kind: ExprKind::Builtin(
                        Builtin::IsInstance(vec![ir::InstanceType::Exact(members[0])]),
                        vec![self.local(slot)],
                    ),
                },
                project(members[0]),
                project(members[1]),
            )
        };
        Some((slot, test, first, second))
    }

    /// Reads a proved member through explicit ownership-preserving projection.
    /// Optional members keep zero intact until a separate non-None proof exists.
    pub(super) fn read_local(&self, slot: usize) -> Expr {
        let value = self.local(slot);
        let Some(&ty) = self.narrowed.get(&slot) else {
            return value;
        };
        if ty == Type::None {
            return Expr {
                ty,
                kind: ExprKind::None,
            };
        }
        if ty == value.ty {
            return value;
        }
        if matches!(value.ty, Type::AnnotationValue(_)) {
            let Type::Tuple(layout) = self.types.tuple(vec![ty]) else {
                unreachable!()
            };
            return Expr {
                ty,
                kind: ExprKind::Builtin(Builtin::AnnotationMember(layout), vec![value]),
            };
        }
        if value.ty == Type::ObjectValue
            && let Type::Class(class) = ty
        {
            return Expr {
                ty,
                kind: ExprKind::Builtin(Builtin::ObjectRecordProject(class), vec![value]),
            };
        }
        if matches!(value.ty, Type::Document(_)) {
            return if ty == Type::Str {
                documents::op(ir::DocumentOp::AsStr, Type::Str, vec![value])
            } else {
                Expr {
                    ty,
                    kind: ExprKind::Builtin(Builtin::RecordView, vec![value]),
                }
            };
        }
        if matches!((value.ty, ty), (Type::Class(parent), Type::Class(child))
            if self.classes.parsers.child_of(child, parent)
                || self.classes.declarations.definitions[child].bases.contains(&parent))
        {
            return Expr {
                ty,
                kind: ExprKind::Builtin(Builtin::RecordView, vec![value]),
            };
        }
        if matches!(ty, Type::StrLiteral(_)) && matches!(value.ty, Type::Str | Type::StrLiteral(_))
        {
            return literals::view(value, ty);
        }
        if self.types.payload(value.ty).unwrap_or(value.ty) == Type::Exception
            && matches!(
                ty,
                Type::ImportException | Type::TimeoutExpired | Type::NamedException(_)
            )
        {
            let value = if value.ty == Type::Exception {
                value
            } else {
                Expr {
                    ty: Type::Exception,
                    kind: ExprKind::Unwrap(Box::new(value)),
                }
            };
            return Expr {
                ty,
                kind: ExprKind::Builtin(
                    Builtin::Process(ir::ProcessOperation::ExceptionView),
                    vec![value],
                ),
            };
        }
        let source = self.types.payload(value.ty).unwrap_or(value.ty);
        let target = self.types.payload(ty).unwrap_or(ty);
        let kind = if self.types.members(source).is_some_and(|members| {
            members.contains(&target)
                || self
                    .types
                    .members(target)
                    .is_some_and(|subset| subset.iter().all(|member| members.contains(member)))
        }) {
            ExprKind::Builtin(Builtin::UnionProject, vec![value])
        } else {
            debug_assert_eq!(self.types.payload(value.ty), Some(ty));
            ExprKind::Unwrap(Box::new(value))
        };
        Expr { ty, kind }
    }

    /// Compare numeric values without changing the stored member used by repr or identity.
    pub(super) fn numeric_union_comparison(&self, values: Vec<Expr>) -> Vec<Expr> {
        if !values.iter().any(|v| self.types.is_bool_or_int(v.ty))
            || !values
                .iter()
                .all(|v| matches!(v.ty, Type::Int | Type::Bool) || self.types.is_bool_or_int(v.ty))
        {
            return values;
        }
        values
            .into_iter()
            .map(|value| {
                let cast = match value.ty {
                    Type::Int => return value,
                    Type::Bool => Builtin::Int,
                    _ => Builtin::UnionProject,
                };
                Expr {
                    ty: Type::Int,
                    kind: ExprKind::Builtin(cast, vec![value]),
                }
            })
            .collect()
    }

    /// Class arguments are static builtin/nominal classes or tuples of them;
    /// annotations such as list[int] never become runtime class objects.
    fn instance_types(&self, expression: &ast::Expr) -> Result<Vec<InstanceType>, String> {
        use InstanceType::{Deque, Exact, List, Tuple};
        match self.collection_class_name(expression) {
            Some("collections.deque") => return Ok(vec![Deque]),
            Some("collections.abc.MutableSequence") => return Ok(vec![List, Deque]),
            Some("collections.abc.Sequence") => {
                return Ok(vec![
                    List,
                    Deque,
                    Tuple,
                    Exact(Type::Str),
                    Exact(Type::Bytes),
                ]);
            }
            _ => {}
        }
        if self.resolve(expression) == Some(Binding::Intrinsic(Intrinsic::StatResult)) {
            return Ok(vec![InstanceType::Exact(self.types.stat_result())]);
        }
        match self.resolve(expression) {
            Some(Binding::Intrinsic(Intrinsic::Context)) => {
                return Ok(vec![InstanceType::Exact(Type::Context)]);
            }
            Some(Binding::Intrinsic(Intrinsic::ContextVar)) => {
                return Ok(vec![InstanceType::ContextVar]);
            }
            Some(Binding::Intrinsic(Intrinsic::ContextToken)) => {
                return Ok(vec![InstanceType::ContextToken]);
            }
            Some(Binding::Intrinsic(Intrinsic::ModuleSpec)) => {
                return Ok(vec![InstanceType::ModuleSpec]);
            }
            Some(Binding::Intrinsic(Intrinsic::ForwardRef)) => {
                return Ok(vec![InstanceType::ForwardRef]);
            }
            Some(Binding::Intrinsic(Intrinsic::SimpleNamespace)) => {
                return Ok(vec![InstanceType::SimpleNamespace]);
            }
            _ => {}
        }
        if let ast::Expr::Tuple(tuple) = expression {
            let mut result = Vec::new();
            for value in &tuple.elts {
                result.extend(self.instance_types(value)?);
            }
            return Ok(result);
        }
        if let Some(kind) = self.exception_category(expression) {
            return Ok(vec![InstanceType::Exception(kind)]);
        }
        if matches!(
            self.resolve(expression),
            Some(Binding::Intrinsic(Intrinsic::Mapping { .. }))
        ) {
            let mut classes = vec![InstanceType::Dict];
            classes.extend(
                self.classes
                    .declarations
                    .definitions
                    .iter()
                    .enumerate()
                    .filter(|(_, class)| class.mapping)
                    .map(|(id, _)| InstanceType::Exact(Type::Class(id))),
            );
            return Ok(classes);
        }
        if self.resolve(expression) == Some(Binding::Intrinsic(Intrinsic::CodecInfoClass)) {
            return Ok(vec![InstanceType::Exact(Type::CodecInfo)]);
        }
        if let Some(Binding::Intrinsic(Intrinsic::IoBase(base))) = self.resolve(expression) {
            return Ok(vec![InstanceType::IoBase(base)]);
        }
        if self.resolve(expression) == Some(Binding::Intrinsic(Intrinsic::StrEnum)) {
            return Ok(vec![InstanceType::StrEnum]);
        }
        if matches!(
            self.resolve(expression),
            Some(Binding::Intrinsic(Intrinsic::Path))
        ) {
            return Ok(vec![InstanceType::Exact(Type::Path)]);
        }
        if matches!(
            self.resolve(expression),
            Some(Binding::Intrinsic(Intrinsic::CompletedProcess))
        ) {
            return Ok(vec![InstanceType::Exact(Type::CompletedProcess(
                crate::ir::ProcessCapture::TEXT,
                None,
            ))]);
        }
        if let Some(Binding::Definition(definition)) = self.resolve(expression)
            && self.types.namedtuple(definition).is_some()
        {
            return Ok(self
                .types
                .tuples()
                .into_iter()
                .filter(|(ty, _)| self.types.namedtuple_definition(*ty) == Some(definition))
                .map(|(ty, _)| InstanceType::Exact(ty))
                .collect());
        }
        if let Some(Binding::Definition(definition)) = self.resolve(expression)
            && let Some(&class) = self.classes.declarations.ids.get(&definition)
        {
            let records = &self.classes.declarations;
            return Ok(records
                .definitions
                .iter()
                .enumerate()
                .filter(|(id, record)| *id == class || record.bases.contains(&class))
                .map(|(id, _)| InstanceType::Exact(records.instance(id)))
                .collect());
        }
        if let ast::Expr::Name(name) = expression {
            let ty = match name.id.as_str() {
                "str" => InstanceType::Exact(Type::Str),
                "type" => InstanceType::Exact(Type::BuiltinClass),
                "bytes" => InstanceType::Exact(Type::Bytes),
                "int" => InstanceType::Exact(Type::Int),
                "bool" => InstanceType::Exact(Type::Bool),
                "float" => InstanceType::Exact(Type::Float),
                "list" => InstanceType::List,
                "dict" => InstanceType::Dict,
                "set" => InstanceType::Set,
                "frozenset" => InstanceType::Exact(Type::FrozenStrings),
                "tuple" => InstanceType::Tuple,
                _ => {
                    return Err(error(
                        self.source,
                        expression,
                        "isinstance requires supported static classes",
                    ));
                }
            };
            return Ok(vec![ty]);
        }
        Err(error(
            self.source,
            expression,
            "isinstance requires supported static classes",
        ))
    }

    pub(super) fn isinstance(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        self.reject_builtin_keywords(call)?;
        let [value, classes] = call.arguments.args.as_ref() else {
            return Err(error(
                self.source,
                call,
                "isinstance requires a value and static class or tuple of classes",
            ));
        };
        let value = self.expr(value)?;
        let classes = match self.instance_types(classes) {
            Ok(classes) => classes,
            Err(diagnostic) => {
                if let Ok(target) = self.expr(classes) {
                    match target.ty {
                        Type::Placeholder(ir::PlaceholderKind::Class) => {
                            // Looking up either operand can run Python code.
                            let mut scope = calls::ArgumentScope::default();
                            let value = scope.hold(self, value);
                            scope.hold(self, target);
                            let class =
                                InstanceType::Exact(Type::Placeholder(ir::PlaceholderKind::Value));
                            return Ok(scope.finish(Expr {
                                ty: Type::Bool,
                                kind: ExprKind::Builtin(
                                    Builtin::IsInstance(vec![class]),
                                    vec![value],
                                ),
                            }));
                        }
                        Type::ForwardRef(_) | Type::Placeholder(ir::PlaceholderKind::Value) => {
                            return Ok(Self::class_check_error(
                                value,
                                target,
                                "isinstance() arg 2 must be a type, a tuple of types, or a union",
                            ));
                        }
                        _ => {}
                    }
                }
                return Err(diagnostic);
            }
        };
        if matches!(value.ty, Type::Document(_)) {
            let op = match classes.as_slice() {
                [InstanceType::Dict] => ir::DocumentOp::IsMap,
                [InstanceType::List] => ir::DocumentOp::IsList,
                [InstanceType::Exact(Type::Str)] => ir::DocumentOp::IsStr,
                _ => {
                    return Err(error(
                        self.source,
                        call,
                        "document isinstance requires a dict, list, or str class",
                    ));
                }
            };
            return Ok(documents::op(op, Type::Bool, vec![value]));
        }
        if matches!(value.ty, Type::JsonRoot | Type::JsonValue)
            && matches!(classes.as_slice(), [InstanceType::Exact(Type::Int)])
        {
            return Ok(Expr {
                ty: Type::Bool,
                kind: ExprKind::Builtin(Builtin::JsonIsInt, vec![value]),
            });
        }
        if matches!(
            self.types.payload(value.ty).unwrap_or(value.ty),
            Type::JsonRoot | Type::JsonValue | Type::JsonObject
        ) {
            return Err(error(
                self.source,
                call,
                "isinstance on readonly JSON is unsupported; select into an exact type",
            ));
        }
        Ok(Expr {
            ty: Type::Bool,
            kind: ExprKind::Builtin(Builtin::IsInstance(classes), vec![value]),
        })
    }

    /// Class checks evaluate both operands before reporting an invalid class.
    fn class_check_error(first: Expr, second: Expr, message: &str) -> Expr {
        let exception = Expr {
            ty: Type::Exception,
            kind: ExprKind::Exception {
                kind: exception_kind("TypeError").unwrap(),
                message: Some(Box::new(unpacking::string(message))),
            },
        };
        Expr {
            ty: Type::Bool,
            kind: ExprKind::Builtin(Builtin::RaiseException, vec![first, second, exception]),
        }
    }

    pub(super) fn issubclass(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        self.reject_builtin_keywords(call)?;
        let [first, second] = call.arguments.args.as_ref() else {
            return Err(error(
                self.source,
                call,
                "issubclass requires two positional operands",
            ));
        };
        let first = self.expr(first)?;
        let second = self.expr(second)?;
        match (first.ty, second.ty) {
            (Type::BuiltinClass, Type::BuiltinClass) => Ok(Expr {
                ty: Type::Bool,
                kind: ExprKind::Builtin(Builtin::IsBuiltinSubclass, vec![first, second]),
            }),
            (Type::BuiltinClass, Type::ForwardRef(_)) => Ok(Self::class_check_error(
                first,
                second,
                "issubclass() arg 2 must be a class, a tuple of classes, or a union",
            )),
            (
                Type::Bool | Type::Int | Type::Float | Type::Str | Type::None | Type::ForwardRef(_),
                Type::BuiltinClass | Type::ForwardRef(_),
            ) => Ok(Self::class_check_error(
                first,
                second,
                "issubclass() arg 1 must be a class",
            )),
            _ => Err(error(
                self.source,
                call,
                "issubclass requires supported native class objects",
            )),
        }
    }

    fn instance_members(&self, ty: Type) -> Vec<Type> {
        if matches!(ty, Type::AnnotationValue(_)) {
            return self.types.children(ty);
        }
        let concrete = self.types.payload(ty).unwrap_or(ty);
        let mut members = self
            .types
            .members(concrete)
            .unwrap_or_else(|| vec![concrete]);
        if concrete != ty {
            members.push(Type::None);
        }
        members
    }

    /// Tests for derived records or exception kinds can require runtime inspection.
    /// A stored builtin exception can be either a SystemExit or an Exception.
    fn instance_member_matches(&self, ty: Type, classes: &[InstanceType]) -> Option<bool> {
        if matches!(ty, Type::ObjectValue | Type::StringIterable) {
            return None;
        }
        let mut unknown = false;
        for &class in classes {
            if let InstanceType::IoBase(base) = class {
                match base.matches(ty) {
                    Some(true) => return Some(true),
                    Some(false) => {}
                    None => unknown = true,
                }
                continue;
            }
            if let (Type::Class(base), InstanceType::Exact(Type::Class(derived))) = (ty, class)
                && self.classes.declarations.definitions[derived]
                    .bases
                    .contains(&base)
            {
                // A base view can contain either this derived layout or another one.
                unknown = true;
            }
            if let InstanceType::Exception(kind) = class
                && ty == Type::ImportException
            {
                if matches!(kind, 0 | 1 | 41) {
                    return Some(true);
                }
                if kind == 42 || kind >= 256 {
                    unknown = true;
                }
                continue;
            }
            if let InstanceType::Exception(kind) = class
                && ty == Type::TimeoutExpired
            {
                if matches!(kind, 0 | 1 | 25 | 27) {
                    return Some(true);
                }
                continue;
            }
            if let InstanceType::Exception(kind) = class
                && matches!(ty, Type::Exception | Type::NamedException(_))
                && kind != 0
            {
                unknown = true;
            } else if class.accepts(ty) {
                return Some(true);
            }
        }
        (!unknown).then_some(false)
    }

    /// Keeps exhaustive elif chains definite while still checking every branch.
    /// Evaluating the original condition remains in IR even when its result is known.
    pub(super) fn instance_condition(&self, expression: &Expr) -> Option<bool> {
        match &expression.kind {
            ExprKind::Builtin(Builtin::IsInstance(classes), args) => {
                let mut result = None;
                for member in self.instance_members(args[0].ty) {
                    let current = self.instance_member_matches(member, classes)?;
                    if result.is_some_and(|previous| previous != current) {
                        return None;
                    }
                    result = Some(current);
                }
                result
            }
            ExprKind::Compare(values, ops) => {
                let [left, right] = values.as_slice() else {
                    return None;
                };
                let (ExprKind::Str(left), ExprKind::Str(right)) = (&left.kind, &right.kind) else {
                    return None;
                };
                match ops.as_slice() {
                    [Compare::Equal] => Some(left == right),
                    [Compare::NotEqual] => Some(left != right),
                    _ => None,
                }
            }
            ExprKind::Not(value) => self.instance_condition(value).map(|value| !value),
            _ => None,
        }
    }

    /// A later walrus write invalidates earlier tests even inside the same and/or.
    /// Attribute/subscript tests deliberately contribute no local storage facts.
    fn instance_facts(
        &self,
        test: &ast::Expr,
        truth: bool,
    ) -> HashMap<String, Vec<(Vec<InstanceType>, bool)>> {
        match test {
            ast::Expr::Call(call)
                if matches!(&*call.func, ast::Expr::Name(name) if name.id == "isinstance")
                    && call.arguments.args.len() == 2
                    && call.arguments.keywords.is_empty() =>
            {
                let name = match &call.arguments.args[0] {
                    ast::Expr::Name(name) => Some(name),
                    ast::Expr::Named(value) => match &*value.target {
                        ast::Expr::Name(name) => Some(name),
                        _ => None,
                    },
                    _ => None,
                };
                if let Some(name) = name
                    && let Ok(classes) = self.instance_types(&call.arguments.args[1])
                {
                    HashMap::from([(name.id.to_string(), vec![(classes, truth)])])
                } else {
                    HashMap::new()
                }
            }
            ast::Expr::Call(call)
                if matches!(&*call.func, ast::Expr::Name(name) if name.id == "bool")
                    && call.arguments.args.len() == 1
                    && call.arguments.keywords.is_empty() =>
            {
                self.instance_facts(&call.arguments.args[0], truth)
            }
            ast::Expr::UnaryOp(value) if value.op == ast::UnaryOp::Not => {
                self.instance_facts(&value.operand, !truth)
            }
            ast::Expr::BoolOp(value) if truth == (value.op == ast::BoolOp::And) => {
                let mut facts: HashMap<String, Vec<(Vec<InstanceType>, bool)>> = HashMap::new();
                for operand in &value.values {
                    for name in narrowing::expression_assignments(operand) {
                        facts.remove(&name);
                    }
                    for (name, constraints) in self.instance_facts(operand, truth) {
                        facts.entry(name).or_default().extend(constraints);
                    }
                }
                facts
            }
            _ => HashMap::new(),
        }
    }

    /// A successful test proves a derived layout only if every matching class
    /// has that layout. A tuple of siblings cannot expose either's fields.
    fn tested_subclass(&self, base: usize, classes: &[InstanceType]) -> Option<usize> {
        let definitions = &self.classes.declarations.definitions;
        classes.iter().find_map(|class| {
            let InstanceType::Exact(Type::Class(child)) = *class else {
                return None;
            };
            (definitions[child].bases.contains(&base)
                && classes.iter().all(|class| {
                    matches!(*class, InstanceType::Exact(Type::Class(id))
                    if id == child || definitions[id].bases.contains(&child))
                }))
            .then_some(child)
        })
    }

    pub(super) fn apply_instance_facts(&mut self, test: &ast::Expr, truth: bool) {
        for (name, constraints) in self.instance_facts(test, truth) {
            let Some(&slot) = self.names.get(&name) else {
                continue;
            };
            let current = self
                .narrowed
                .get(&slot)
                .copied()
                .unwrap_or(self.locals[slot]);
            if current == Type::ObjectValue {
                for (classes, truth) in &constraints {
                    if *truth
                        && let [InstanceType::Exact(Type::Class(class))] = classes.as_slice()
                        && self.boxed_dataclass(*class)
                    {
                        self.narrowed.insert_mut(slot, Type::Class(*class));
                    }
                }
                continue;
            }
            if let (Type::Class(_), Type::Class(mut base)) = (self.locals[slot], current) {
                for (classes, positive) in &constraints {
                    if *positive && let Some(child) = self.tested_subclass(base, classes) {
                        base = child;
                    }
                }
                self.narrowed.insert_mut(slot, Type::Class(base));
                continue;
            }
            if matches!(current, Type::Document(_)) {
                for (classes, truth) in &constraints {
                    if !*truth {
                        continue;
                    }
                    let ty = match classes.as_slice() {
                        [InstanceType::List] => documents::LIST,
                        [InstanceType::Dict] => documents::MAP,
                        [InstanceType::Exact(Type::Str)] => Type::Str,
                        _ => continue,
                    };
                    self.narrowed.insert_mut(slot, ty);
                }
                continue;
            }
            let mut members = self.instance_members(current);
            for (classes, truth) in &constraints {
                members.retain(|&ty| {
                    self.instance_member_matches(ty, classes)
                        .is_none_or(|matches| matches == *truth)
                });
                if *truth && !classes.is_empty() && classes.iter().all(|c| matches!(c, InstanceType::Exception(kind) if self.is_import_error(*kind))) {
                    for member in &mut members { if *member==Type::Exception { *member=Type::ImportException; } }
                }
                // A positive exact timeout category test establishes the byte
                // payload contract even when the value was stored as Exception.
                if *truth
                    && !classes.is_empty()
                    && classes
                        .iter()
                        .all(|class| matches!(class, InstanceType::Exception(27)))
                {
                    for member in &mut members {
                        if *member == Type::Exception {
                            *member = Type::TimeoutExpired;
                        }
                    }
                }
            }
            if members.is_empty() {
                // A known-dead branch is still checked against a valid member
                // of the declared union; its facts never join a live successor.
                members = self.instance_members(self.locals[slot]);
                for (classes, truth) in &constraints {
                    members.retain(|&ty| {
                        self.instance_member_matches(ty, classes)
                            .is_none_or(|matches| matches == *truth)
                    });
                }
            }
            if let Some(ty) = self.types.union(&members) {
                self.narrowed.insert_mut(slot, ty);
            }
        }
    }
}
