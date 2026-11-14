//! Closed type tests refine local values without refining aliased heap storage.

use super::*;
use crate::ir::{CalendarType, InstanceType};
use ruff_python_ast::CmpOp;
use ruff_python_ast::UnaryOp;
use ty_python_semantic::types::KnownClass;

impl Checker<'_> {
    /// int() on a closed string/integer value preserves the integer or parses the
    /// string. Keep the original value alive and evaluate it only once.
    pub(super) fn integer_from_text_or_int(&mut self, value: Expr, call: &ast::ExprCall) -> Expr {
        let (slot, test, text, integer) = self.protocol_alternatives(value.ty).unwrap();
        debug_assert_eq!((text.ty, integer.ty), (Type::Str, Type::Int));
        let parsed = Expr {
            ty: Type::Int,
            kind: ExprKind::Builtin(
                Builtin::Int(Some(Box::new(self.call_origin(call)))),
                vec![text],
            ),
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
        if self
            .types
            .members(ty)
            .is_some_and(|members| members.len() != 2)
        {
            return None;
        }
        self.split_protocol_union(ty)
    }

    /// Split off one possible layout, keeping the remaining alternatives as a typed union.
    pub(super) fn split_protocol_union(&mut self, ty: Type) -> Option<(usize, Expr, Expr, Expr)> {
        let payload = self.types.payload(ty);
        let members = self.types.members(ty);
        if payload.is_none() && members.is_none() {
            return None;
        }
        let slot = self.hidden(ty);
        let (test, first, second) = if let Some(payload) = payload {
            (
                Expr {
                    ty: Type::Bool,
                    kind: ExprKind::Builtin(Builtin::IsNone, vec![self.local(slot)]),
                },
                Expr::none(),
                Expr {
                    ty: payload,
                    kind: ExprKind::Unwrap(Box::new(self.local(slot))),
                },
            )
        } else {
            let members = members.unwrap();
            let rest = self.types.union(&members[1..])?;
            // A record stored in the base arm can have any of its concrete layouts.
            let alternatives = match members[0] {
                Type::Class(base) => self
                    .classes
                    .concrete_layouts(base)
                    .into_iter()
                    .map(|class| ir::InstanceType::Exact(Type::Class(class)))
                    .collect(),
                ty => vec![ir::InstanceType::Exact(ty)],
            };
            let project = |ty| Expr {
                ty,
                kind: ExprKind::Builtin(Builtin::UnionProject, vec![self.local(slot)]),
            };
            (
                Expr {
                    ty: Type::Bool,
                    kind: ExprKind::Builtin(
                        Builtin::IsInstance(alternatives),
                        vec![self.local(slot)],
                    ),
                },
                project(members[0]),
                project(rest),
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
        if self.types.payload(value.ty).unwrap_or(value.ty) == Type::Calendar(CalendarType::Date)
            && ty == Type::Calendar(CalendarType::DateTime)
        {
            let date = if self.types.payload(value.ty).is_some() {
                Expr {
                    ty: Type::Calendar(CalendarType::Date),
                    kind: ExprKind::Unwrap(Box::new(value)),
                }
            } else {
                value
            };
            return Expr {
                ty,
                kind: ExprKind::Builtin(Builtin::RecordView, vec![date]),
            };
        }
        if self
            .types
            .partial_function(ty)
            .is_some_and(|p| p.callable == value.ty)
            || self.types.method_signature(ty) == Some(value.ty)
        {
            return Expr {
                ty,
                kind: ExprKind::Builtin(Builtin::RecordView, vec![value]),
            };
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
        if value.ty == Type::BinaryIO && ty == Type::BytesIO {
            return Expr {
                ty,
                kind: ExprKind::Builtin(Builtin::RecordView, vec![value]),
            };
        }
        if value.ty == Type::ObjectValue && ty == Type::Str {
            return Expr {
                ty,
                kind: ExprKind::Builtin(Builtin::ObjectTextProject, vec![value]),
            };
        }
        if value.ty == Type::ObjectValue && matches!(ty, Type::BytesIO | Type::StringIO) {
            return Expr {
                ty,
                kind: ExprKind::Builtin(Builtin::ObjectStreamProject, vec![value]),
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
                    Builtin::process(ir::ProcessOperation::ExceptionView),
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

    /// Keep the nominal tag through isinstance; convert only the selected integer member.
    pub(super) fn int_enum_union_value(&mut self, value: Expr) -> Option<Expr> {
        let members = self.types.members(value.ty)?;
        let [Type::Int, member @ Type::IntEnum(_)] = *members.as_slice() else {
            return None;
        };
        let slot = self.hidden(value.ty);
        let project = |ty| Expr {
            ty,
            kind: ExprKind::Builtin(Builtin::UnionProject, vec![self.local(slot)]),
        };
        let test = Expr {
            ty: Type::Bool,
            kind: ExprKind::Builtin(
                Builtin::IsInstance(vec![InstanceType::Exact(member)]),
                vec![self.local(slot)],
            ),
        };
        Some(Expr {
            ty: Type::Int,
            kind: ExprKind::Scope {
                body: vec![Statement::Assign(slot, value)],
                value: Box::new(Expr {
                    ty: Type::Int,
                    kind: ExprKind::Conditional(
                        Box::new(test),
                        Box::new(Self::enum_integer(project(member))),
                        Box::new(project(Type::Int)),
                    ),
                }),
                clear: vec![slot],
            },
        })
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
                    Type::Bool => Builtin::Int(None),
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
        if matches!(self.resolve(expression), Some(Binding::RegisteredClass(_))) {
            return Err(self.scoped_builtin_error("isinstance with a Python class", expression));
        }
        let classinfo = expression
            .as_subscript_expr()
            .map_or(expression, |item| item.value.as_ref());
        if self.collection_binding(classinfo)
            == Some(Binding::Intrinsic(Intrinsic::TYPING_SEQUENCE))
        {
            return Err(self.object_runtime_export("typing.Sequence", expression));
        }
        let numeric = self.collection_binding(expression);
        let number = numeric == Some(Binding::Intrinsic(Intrinsic::Number));
        if number
            || numeric
                == Some(Binding::Intrinsic(Intrinsic::ScopedObjectExport(
                    "numbers.Rational",
                )))
        {
            // Int includes bool and IntEnum; Fraction inherits Rational and Number.
            let mut types = vec![Exact(Type::Int)];
            if number {
                types.push(Exact(Type::Float));
            }
            if let Some(fraction) = (0..self.classes.declarations.definitions.len())
                .find(|&class| self.fraction_class(class))
            {
                types.push(Exact(Type::Class(fraction)));
            }
            return Ok(types);
        }
        match Self::collection_class_name(numeric) {
            Some("collections.deque") => return Ok(vec![Deque]),
            Some("collections.abc.MutableMapping") => return Ok(vec![InstanceType::Dict]),
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
        if let Some(Binding::Intrinsic(Intrinsic::SplitResult { bytes })) = self.resolve(expression)
        {
            return Ok(vec![Exact(self.types.url_split_result(bytes))]);
        }
        if self.resolve(expression) == Some(Binding::Intrinsic(Intrinsic::StructTime)) {
            return Ok(vec![Exact(self.types.struct_time())]);
        }
        if self.resolve(expression) == Some(Binding::Intrinsic(Intrinsic::StatResult)) {
            return Ok(vec![InstanceType::Exact(self.types.stat_result())]);
        }
        match self.resolve(expression) {
            Some(Binding::Intrinsic(Intrinsic::BytesIo)) => return Ok(vec![Exact(Type::BytesIO)]),
            Some(Binding::Intrinsic(Intrinsic::StringIo)) => {
                return Ok(vec![Exact(Type::StringIO)]);
            }
            Some(Binding::Intrinsic(Intrinsic::Calendar(CalendarType::Date))) => {
                return Ok(vec![
                    Exact(Type::Calendar(CalendarType::Date)),
                    Exact(Type::Calendar(CalendarType::DateTime)),
                ]);
            }
            Some(Binding::Intrinsic(Intrinsic::Calendar(kind))) => {
                return Ok(vec![Exact(Type::Calendar(kind))]);
            }
            Some(Binding::Intrinsic(Intrinsic::ClockTime)) => {
                return Ok(vec![Exact(Type::ClockTime)]);
            }
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
            self.collection_binding(expression),
            Some(Binding::Intrinsic(
                Intrinsic::Mapping | Intrinsic::TYPING_MAPPING
            ))
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
        if self.resolve(expression)
            == Some(Binding::Intrinsic(Intrinsic::ObjectRuntimeExport(
                "codecs.CodecInfo",
            )))
        {
            return Ok(vec![InstanceType::Exact(Type::CodecInfo)]);
        }
        // Fixed I/O values can answer isinstance without materializing the public ABC.
        let io_base = match self.resolve(expression) {
            Some(Binding::Intrinsic(Intrinsic::IO_BASE)) => Some(ir::IoBase::Any),
            Some(Binding::Intrinsic(Intrinsic::RAW_IO_BASE)) => Some(ir::IoBase::Raw),
            Some(Binding::Intrinsic(Intrinsic::BUFFERED_IO_BASE)) => Some(ir::IoBase::Buffered),
            Some(Binding::Intrinsic(Intrinsic::TEXT_IO_BASE)) => Some(ir::IoBase::Text),
            _ => None,
        };
        if let Some(base) = io_base {
            return Ok(vec![InstanceType::IoBase(base)]);
        }
        if self.resolve(expression) == Some(Binding::Intrinsic(Intrinsic::IntEnum)) {
            return Ok(vec![InstanceType::IntEnum]);
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
                .filter(|(id, _)| records.subclass(self.graph, *id, class))
                .map(|(id, _)| InstanceType::Exact(records.instance(id)))
                .collect());
        }
        if let ast::Expr::Name(name) = expression
            && !self.is_class_receiver(expression)
            && !self.comprehension_targets.contains(name.id.as_str())
            && self.unshadowed_builtin(expression, name.id.as_str())
        {
            let ty = match name.id.as_str() {
                "str" => InstanceType::Exact(Type::Str),
                "type" => InstanceType::Exact(Type::BuiltinClass),
                "bytes" => InstanceType::Exact(Type::Bytes),
                "bytearray" => InstanceType::Exact(Type::ByteArray),
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
        if let Some(test) = self.unittest_failure_isinstance(&value, classes)? {
            return Ok(test);
        }
        if let Some(Binding::Intrinsic(Intrinsic::WeakProxyClass { callable })) =
            self.collection_binding(classes)
        {
            return Ok(self.weak_proxy_instance(value, callable));
        }
        let value = Self::weak_target(value);
        if classes
            .as_attribute_expr()
            .is_some_and(|a| a.attr.as_str() == "failureException")
            && matches!(value.ty, Type::Exception | Type::NamedException(_))
        {
            // Evaluate the candidate before the case, which may select an
            // overridden failure class through a base-typed reference.
            let mut scope = calls::ArgumentScope::default();
            let held = scope.hold(self, value.clone());
            if let Some(test) = self.unittest_failure_class(classes, |kind, _| Expr {
                ty: Type::Bool,
                kind: ExprKind::Builtin(
                    Builtin::IsInstance(vec![InstanceType::Exception(kind)]),
                    vec![held.clone()],
                ),
            })? {
                return Ok(scope.finish(test));
            }
        }
        let classes = match self.instance_types(classes) {
            Ok(classes) => classes,
            Err(diagnostic) => {
                if let Ok(target) = self.expr(classes) {
                    match target.ty {
                        Type::BuiltinClass if self.has_plain_factory_metaclass(value.ty) => {
                            let result = self.builtin_subclass_values(
                                annotations::class_value("type"),
                                target,
                                call,
                            )?;
                            return Ok(self.evaluated_receiver(Some(value), result));
                        }
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
        // Either operand can require a runtime class before the other has a static value.
        for operand in [first, second] {
            if let Some(Binding::Intrinsic(Intrinsic::ObjectRuntimeExport(name))) =
                self.resolve(operand)
            {
                return Err(self.object_runtime_export(name, call));
            }
        }
        let mut class_operand = |operand: &ast::Expr| {
            if self.collection_binding(operand) == Some(Binding::Intrinsic(Intrinsic::Number)) {
                Ok(annotations::class_value("numbers.Number"))
            } else {
                self.expr(operand)
            }
        };
        let first = class_operand(first)?;
        let second = class_operand(second)?;
        self.builtin_subclass_values(first, second, call)
    }

    /// Compare evaluated class operands or raise for an invalid Python argument.
    pub(super) fn builtin_subclass_values(
        &self,
        first: Expr,
        second: Expr,
        at: &impl Ranged,
    ) -> Result<Expr, String> {
        match (first.ty, second.ty) {
            (Type::PythonObject, _) | (_, Type::PythonObject) => {
                self.require_registered_operation(at)?;
                Ok(Expr {
                    ty: Type::Bool,
                    ..Expr::unrepresented()
                })
            }
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
                at,
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
            if ty == Type::Calendar(CalendarType::Date)
                && matches!(
                    class,
                    InstanceType::Exact(Type::Calendar(CalendarType::DateTime))
                )
            {
                unknown = true;
                continue;
            }
            if matches!(
                (ty, class),
                (Type::BinaryIO, InstanceType::Exact(Type::BytesIO))
                    | (Type::TextIO, InstanceType::Exact(Type::StringIO))
            ) {
                unknown = true;
                continue;
            }
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
            } else if class.accepts(ty, self.types) {
                return Some(true);
            }
        }
        (!unknown).then_some(false)
    }

    /// Keeps exhaustive elif chains definite while still checking every branch.
    /// Evaluating the original condition remains in IR even when its result is known.
    pub(super) fn instance_condition(&self, expression: &Expr) -> Option<bool> {
        match &expression.kind {
            ExprKind::Bool(value) => Some(*value),
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
                    [CmpOp::Eq] => Some(left == right),
                    [CmpOp::NotEq] => Some(left != right),
                    _ => None,
                }
            }
            ExprKind::Unary(UnaryOp::Not, value) => {
                self.instance_condition(value).map(|value| !value)
            }
            _ => None,
        }
    }

    /// Recognize exact builtin type comparisons on erased values and typed tuples.
    pub(super) fn exact_object_type<'n>(
        &self,
        left: &'n ast::Expr,
        right: &'n ast::Expr,
    ) -> Option<(&'n ast::Expr, KnownClass)> {
        let (call, name) = if let Some(call) = left.as_call_expr() {
            (call, right)
        } else {
            (right.as_call_expr()?, left)
        };
        let identifier = name.as_name_expr()?.id.as_str();
        let class = match identifier {
            "str" => KnownClass::Str,
            "tuple" => KnownClass::Tuple,
            _ => return None,
        };
        if !self.unshadowed_builtin(name, identifier) {
            return None;
        }
        let [value] = call.arguments.args.as_ref() else {
            return None;
        };
        if !call.arguments.keywords.is_empty() || !self.unshadowed_builtin(&call.func, "type") {
            return None;
        }
        let local_type = value
            .as_name_expr()
            .and_then(|name| self.names.get(name.id.as_str()))
            .map(|slot| {
                self.narrowed
                    .get(slot)
                    .copied()
                    .unwrap_or(self.locals[*slot])
            });
        let exact_tuple = class == KnownClass::Tuple
            && (matches!(local_type, Some(Type::VarTuple(_)))
                // Typeshed exposes the native inventory as Sequence[str].
                || (self.has_static_namespace_root(value)
                    && self.resolve(value) == Some(Binding::Intrinsic(Intrinsic::SysBuiltinModuleNames)))
                || self.graph.session.is_string_tuple(self.module, value));
        ((local_type == Some(Type::ObjectValue) || exact_tuple)
            && self
                .graph
                .session
                .is_known_class(self.module, &call.func, KnownClass::Type)
            && self.graph.session.is_known_class(self.module, name, class))
        .then_some((value, class))
    }

    /// A later walrus write invalidates earlier tests even inside the same and/or.
    /// Attribute/subscript tests deliberately contribute no local storage facts.
    fn instance_facts(
        &self,
        test: &ast::Expr,
        truth: bool,
    ) -> HashMap<String, Vec<(Vec<InstanceType>, bool)>> {
        if let Some((left, op, right)) = self.identity_test(test)
            && let Some((expr, KnownClass::Str)) = self.exact_object_type(left, right)
            && let Some(name) = expr.as_name_expr()
        {
            return HashMap::from([(
                name.id.to_string(),
                vec![(
                    vec![InstanceType::ExactText],
                    truth == (op == ast::CmpOp::Is),
                )],
            )]);
        }
        match test {
            ast::Expr::Call(call)
                if self.unshadowed_builtin(&call.func, "isinstance")
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
                if self.unshadowed_builtin(&call.func, "bool")
                    && call.arguments.args.len() == 1
                    && self.graph.session.is_known_class(
                        self.module,
                        &call.func,
                        KnownClass::Bool,
                    )
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
                    if *truth && matches!(classes.as_slice(), [InstanceType::ExactText]) {
                        self.narrowed.insert_mut(slot, Type::Str);
                    }
                    if *truth
                        && let [InstanceType::Exact(ty @ (Type::BytesIO | Type::StringIO))] =
                            classes.as_slice()
                    {
                        self.narrowed.insert_mut(slot, *ty);
                    }
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
                if *truth
                    && matches!(
                        classes.as_slice(),
                        [InstanceType::Exact(Type::Calendar(CalendarType::DateTime))]
                    )
                {
                    for member in &mut members {
                        if *member == Type::Calendar(CalendarType::Date) {
                            *member = Type::Calendar(CalendarType::DateTime);
                        }
                    }
                }
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
