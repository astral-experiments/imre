//! Representation changes transfer the same owner across a union boundary.

use super::*;

fn tag(ty: Type) -> u8 {
    match ty {
        Type::Str => 0,
        Type::Bytes => 1,
        // Both are Object owners and occupy separate closed unions with str.
        Type::List(_) | Type::Dict(_) | Type::FrozenStrings | Type::Float => 2,
        // These owners share the always-true Object tag. No admitted union
        // contains both an exception and a LogRecord.
        Type::Exception | Type::LogRecord => 3,
        Type::Class(_) | Type::Tuple(_) => 4,
        Type::Enum(_) | Type::IntEnum(_) => 5,
        Type::Path => 6,
        Type::Int => 7,
        _ => unreachable!("closed native union member"),
    }
}

impl Emitter<'_, '_> {
    /// The low bits select the concrete member. A nullable
    /// source preserves its absent zero word when transitioning into a union.
    pub(super) fn union_inject(&mut self, value: Value, returns: Type) -> Value {
        let source = self.module.types.payload(value.ty).unwrap_or(value.ty);
        // Words 1 and 2 are not tagged live addresses. Reuse optional bool's
        // encoding so widening bool | None preserves absence as well.
        if source == Type::Bool {
            self.forget(&value);
            let operand = if value.ty == source {
                let word = self.word(&value);
                self.instruction(format!("add i64 {word}, 1"))
            } else {
                value.operand
            };
            return Value {
                ty: returns,
                operand,
                small_integer: false,
            };
        }
        let tag = tag(source);
        let word = self.word(&value);
        let operand = if source == Type::Float {
            self.instruction(format!(
                "call i64 @imre_union_from_float(i64 {word}, i1 {})",
                value.ty != source
            ))
        } else if source == Type::Int {
            self.instruction(format!("call i64 @imre_union_from_int(i64 {word})"))
        } else if tag != 0 {
            let bits = if matches!(source, Type::Enum(_) | Type::IntEnum(_)) {
                self.instruction(format!("shl i64 {word}, 3"))
            } else {
                word.clone()
            };
            let tagged = self.instruction(format!("or i64 {bits}, {tag}"));
            if value.ty != source {
                let present = self.instruction(format!("icmp ne i64 {word}, 0"));
                self.instruction(format!("select i1 {present}, i64 {tagged}, i64 0"))
            } else {
                tagged
            }
        } else {
            word
        };
        self.forget(&value);
        Value {
            ty: returns,
            operand,
            small_integer: false,
        }
    }

    /// Local flow proves the member before projection. An integer may need to
    /// recover its immediate representation; other owners transfer unchanged.
    pub(super) fn union_project(&mut self, value: Value, returns: Type) -> Value {
        if returns == Type::Bool || self.module.types.payload(returns) == Some(Type::Bool) {
            self.forget(&value);
            let operand = if returns == Type::Bool {
                self.instruction(format!("icmp eq i64 {}, 2", value.operand))
            } else {
                value.operand
            };
            return Value {
                ty: returns,
                operand,
                small_integer: false,
            };
        }
        if matches!(
            self.module.types.payload(returns).unwrap_or(returns),
            Type::Union(_)
        ) {
            // A narrowed union still uses the original member tags and optional zero.
            self.forget(&value);
            return self.decode_word(value.operand, returns);
        }
        let word =
            if returns == Type::Float || self.module.types.payload(returns) == Some(Type::Float) {
                self.instruction(format!(
                    "call i64 @imre_union_into_float(i64 {}, i1 {})",
                    value.operand,
                    returns != Type::Float
                ))
            } else if returns == Type::Int {
                self.instruction(format!(
                    "call i64 @imre_union_into_int(i64 {})",
                    value.operand
                ))
            } else if matches!(returns, Type::Enum(_) | Type::IntEnum(_)) {
                self.instruction(format!("lshr i64 {}, 3", value.operand))
            } else {
                self.instruction(format!("and i64 {}, -8", value.operand))
            };
        self.forget(&value);
        self.decode_word(word, returns)
    }

    /// A record's header retains its actual descriptor through base-class views.
    pub(super) fn exact_record_test(&mut self, value: &Value, class: usize) -> String {
        let layout = self.output.global_ref(format!("layout_class_{class}"));
        let matched = self.instruction(format!(
            "call i32 @imre_record_has_type(ptr {}, ptr @{layout})",
            value.operand
        ));
        self.instruction(format!("icmp ne i32 {matched}, 0"))
    }

    /// Borrow the payload only if this erased value retains the exact record.
    pub(super) fn boxed_record(&mut self, value: &Value, class: usize) -> String {
        let layout = self.output.global_ref(format!("layout_class_{class}"));
        self.instruction(format!(
            "call ptr @imre_object_record_payload(ptr {}, ptr @{layout})",
            value.operand
        ))
    }

    /// Inspect exception and record layouts only after proving the payload is present
    /// and selecting its tag if it is part of a finite union.
    pub(super) fn instance_test(&mut self, value: &Value, classes: &[InstanceType]) -> String {
        if matches!(value.ty, Type::AnnotationValue(_)) {
            let kind = self.instruction(format!(
                "call i32 @imre_annotation_kind(ptr {})",
                value.operand
            ));
            let mut result = "false".to_string();
            for member in self.module.types.children(value.ty) {
                if classes
                    .iter()
                    .any(|class| class.accepts(member, &self.module.types))
                {
                    let tag = super::representation_kind(member, &self.module.types);
                    let is_member = self.instruction(format!("icmp eq i32 {kind}, {tag}"));
                    result = self.instruction(format!("or i1 {result}, {is_member}"));
                }
            }
            return result;
        }
        if let Some(payload) = self.module.types.payload(value.ty) {
            let present = self.instruction(format!("icmp ne i64 {}, 0", value.operand));
            let (inspect, end) = (self.label(), self.label());
            let absent = self.current.clone();
            self.branch(&present, &inspect, &end);
            self.start(&inspect);
            let payload = self.decode_word(value.operand.clone(), payload);
            let result = self.instance_test(&payload, classes);
            let inspected = self.current.clone();
            self.jump(&end);
            self.start(&end);
            return self.instruction(format!(
                "phi i1 [ false, %{absent} ], [ {result}, %{inspected} ]"
            ));
        }
        if value.ty == Type::Calendar(ir::CalendarType::Date) {
            if classes
                .iter()
                .any(|class| class.accepts(value.ty, &self.module.types))
            {
                return "true".to_owned();
            }
            if classes.iter().any(|class| {
                class.accepts(
                    Type::Calendar(ir::CalendarType::DateTime),
                    &self.module.types,
                )
            }) {
                return self.instruction(format!(
                    "call i1 @imre_calendar_is_datetime(ptr {})",
                    value.operand
                ));
            }
        }
        // Frontend class tests include every descendant. If they include the
        // declared base, all layouts which can reach this view already match.
        if matches!(value.ty, Type::Class(_))
            && classes
                .iter()
                .any(|class| class.accepts(value.ty, &self.module.types))
        {
            return "true".to_owned();
        }
        if value.ty == Type::ObjectValue {
            // The view retains its heterogeneous payload. None is not a class
            // accepted by this subset, and bool is also an instance of int.
            let mask = [
                (0, Type::None),
                (1, Type::Bool),
                (2, Type::Int),
                (3, Type::Str),
                (4, Type::ListInt),
                (5, Type::Dict(0)),
                (6, Type::Tuple(0)),
                (7, Type::Float),
                (8, Type::Bytes),
                (12, Type::Set(0)),
            ]
            .iter()
            .filter(|(_, ty)| {
                classes
                    .iter()
                    .any(|class| class.accepts(*ty, &self.module.types))
            })
            .fold(0, |mask, (kind, _)| mask | 1 << *kind);
            let mut matched = (mask != 0).then(|| {
                let kind = self.instruction(format!(
                    "call i32 @imre_system_exit_code_kind(ptr {})",
                    value.operand
                ));
                let bit = self.instruction(format!("shl i32 1, {kind}"));
                let found = self.instruction(format!("and i32 {bit}, {mask}"));
                self.instruction(format!("icmp ne i32 {found}, 0"))
            });
            for class in classes {
                let found = match class {
                    InstanceType::Exact(Type::Class(class)) => {
                        let ptr = self.boxed_record(value, *class);
                        self.instruction(format!("icmp ne ptr {ptr}, null"))
                    }
                    InstanceType::Exception(kind) => {
                        let child = self.instruction(format!(
                            "call i32 @imre_exception_field_matches(ptr {}, i32 {kind})",
                            value.operand
                        ));
                        self.instruction(format!("icmp ne i32 {child}, 0"))
                    }
                    _ => continue,
                };
                matched = Some(match matched {
                    Some(previous) => self.instruction(format!("or i1 {previous}, {found}")),
                    None => found,
                });
            }
            return matched.unwrap_or_else(|| "false".into());
        }
        let mut result = "false".to_owned();
        for &class in classes {
            let current = if value.ty == Type::StringIterable {
                match class {
                    InstanceType::Tuple | InstanceType::List => {
                        let tag = if matches!(class, InstanceType::List) {
                            2
                        } else {
                            4
                        };
                        let bits = self.instruction(format!("and i64 {}, 7", value.operand));
                        self.instruction(format!("icmp eq i64 {bits}, {tag}"))
                    }
                    _ => "false".into(),
                }
            } else if let Some(members) = self.module.types.members(value.ty) {
                let accepts_tag = |ty| {
                    class.accepts(ty, &self.module.types)
                        || matches!(
                            (class, ty),
                            (InstanceType::Exact(Type::Class(_)), Type::Class(_))
                        )
                };
                let inspect_io =
                    |ty| matches!(class, InstanceType::IoBase(base) if base.matches(ty).is_none());
                if members.iter().copied().all(accepts_tag)
                    && !members.iter().copied().any(inspect_io)
                {
                    "true".to_owned()
                } else {
                    let mut matched = None;
                    let bits = self.instruction(format!("and i64 {}, 7", value.operand));
                    for selected in members.into_iter().filter(|&ty| accepts_tag(ty)) {
                        let found = if selected == Type::Bool {
                            // Exclude zero (None) and every tagged integer owner.
                            let offset = self.instruction(format!("sub i64 {}, 1", value.operand));
                            self.instruction(format!("icmp ult i64 {offset}, 2"))
                        } else {
                            self.instruction(format!("icmp eq i64 {bits}, {}", tag(selected)))
                        };
                        let current = if matches!(
                            (class, selected),
                            (InstanceType::Exception(1..), Type::Exception)
                                | (InstanceType::Exact(Type::Class(_)), Type::Class(_))
                        ) || inspect_io(selected)
                        {
                            let (inspect, end) = (self.label(), self.label());
                            let other = self.current.clone();
                            self.branch(&found, &inspect, &end);
                            self.start(&inspect);
                            let word = self.instruction(format!("and i64 {}, -8", value.operand));
                            let payload = self.decode_word(word, selected);
                            let matches = self.instance_test(&payload, &[class]);
                            let inspected = self.current.clone();
                            self.jump(&end);
                            self.start(&end);
                            self.instruction(format!(
                                "phi i1 [ false, %{other} ], [ {matches}, %{inspected} ]"
                            ))
                        } else {
                            found
                        };
                        matched = Some(match matched {
                            None => current,
                            Some(prior) => self.instruction(format!("or i1 {prior}, {current}")),
                        });
                    }
                    matched.unwrap_or_else(|| "false".to_owned())
                }
            } else if let InstanceType::IoBase(base) = class
                && base.matches(value.ty).is_none()
            {
                let raw =
                    self.instruction(format!("call i32 @imre_file_is_raw(ptr {})", value.operand));
                let comparison = if base == crate::ir::IoBase::Raw {
                    "ne"
                } else {
                    "eq"
                };
                self.instruction(format!("icmp {comparison} i32 {raw}, 0"))
            } else if let (InstanceType::Exact(Type::Class(class)), Type::Class(_)) =
                (class, value.ty)
            {
                self.exact_record_test(value, class)
            } else if let InstanceType::Exception(kind) = class
                && matches!(
                    value.ty,
                    Type::Exception
                        | Type::ImportException
                        | Type::TimeoutExpired
                        | Type::NamedException(_)
                )
            {
                let matched = self.instruction(format!(
                    "call i32 @imre_exception_matches(ptr {}, i32 {kind})",
                    value.operand
                ));
                self.instruction(format!("icmp ne i32 {matched}, 0"))
            } else {
                u8::from(class.accepts(value.ty, &self.module.types)).to_string()
            };
            result = self.instruction(format!("or i1 {result}, {current}"));
        }
        result
    }
}
