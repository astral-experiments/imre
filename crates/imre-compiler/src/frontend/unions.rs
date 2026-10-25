//! Closed type tests refine local values without refining aliased heap storage.

use super::*;
use crate::ir::InstanceType;

impl Checker<'_> {
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
        if matches!((value.ty, ty), (Type::Class(parent), Type::Class(child)) if self.classes.parsers.child_of(child, parent))
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
        let kind = if self
            .types
            .members(source)
            .is_some_and(|members| members.contains(&target))
        {
            ExprKind::Builtin(Builtin::UnionProject, vec![value])
        } else {
            debug_assert_eq!(self.types.payload(value.ty), Some(ty));
            ExprKind::Unwrap(Box::new(value))
        };
        Expr { ty, kind }
    }

    /// Class arguments are static builtin/nominal classes or tuples of them;
    /// annotations such as list[int] never become runtime class objects.
    fn instance_types(&self, expression: &ast::Expr) -> Result<Vec<InstanceType>, String> {
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
        if self.resolve(expression) == Some(Binding::Intrinsic(Intrinsic::MappingAbc)) {
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
            ))]);
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
                "bytes" => InstanceType::Exact(Type::Bytes),
                "int" => InstanceType::Exact(Type::Int),
                "bool" => InstanceType::Exact(Type::Bool),
                "float" => InstanceType::Exact(Type::Float),
                "list" => InstanceType::List,
                "dict" => InstanceType::Dict,
                "set" => InstanceType::Set,
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
        if matches!(value.ty, Type::Document(_)) {
            let op = match self.instance_types(classes)?.as_slice() {
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
            && matches!(
                self.instance_types(classes)?.as_slice(),
                [InstanceType::Exact(Type::Int)]
            )
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
        let classes = self.instance_types(classes)?;
        Ok(Expr {
            ty: Type::Bool,
            kind: ExprKind::Builtin(Builtin::IsInstance(classes), vec![value]),
        })
    }

    fn instance_members(&self, ty: Type) -> Vec<Type> {
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

    /// Exception categories below Exception need a runtime hierarchy test;
    /// other category checks are exact for every admitted concrete layout.
    /// A stored builtin exception can be either a SystemExit or an Exception.
    fn instance_member_matches(ty: Type, classes: &[InstanceType]) -> Option<bool> {
        if matches!(ty, Type::ExceptionField | Type::StringIterable) {
            return None;
        }
        let mut unknown = false;
        for &class in classes {
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
                    let current = Self::instance_member_matches(member, classes)?;
                    if result.is_some_and(|previous| previous != current) {
                        return None;
                    }
                    result = Some(current);
                }
                result
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
                    Self::instance_member_matches(ty, classes)
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
                        Self::instance_member_matches(ty, classes)
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
