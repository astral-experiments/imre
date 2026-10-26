//! Concrete truth tests and ownership-preserving operand selection.

use super::*;

impl Emitter<'_, '_> {
    /// Inspects a borrowed value without invoking user code or consuming an owner.
    /// Nullable pointer/float payloads are read only on the present branch.
    pub(super) fn truth(&mut self, value: &Value) -> Value {
        let operand = if let Some(truth) = self.module.types.truthiness(value.ty) {
            u8::from(truth).to_string()
        } else if let Some(payload) = self.module.types.payload(value.ty) {
            match payload {
                // Zero is absent, one is tagged integer zero, and every other
                // canonical integer word is nonzero (including negative values).
                Type::Int => self.instruction(format!("icmp ugt i64 {}, 1", value.operand)),
                Type::Bool => self.instruction(format!("icmp eq i64 {}, 2", value.operand)),
                _ => {
                    let present = self.instruction(format!("icmp ne i64 {}, 0", value.operand));
                    if self.module.types.truthiness(payload) == Some(true) {
                        present
                    } else {
                        let (inspect, end) = (self.label(), self.label());
                        let absent = self.current.clone();
                        self.branch(&present, &inspect, &end);
                        self.start(&inspect);
                        let payload = if payload == Type::Float {
                            self.call("imre_float_unbox", Type::Float, &[value])
                        } else {
                            self.decode_word(value.operand.clone(), payload)
                        };
                        let truth = self.truth(&payload);
                        let inspected = self.current.clone();
                        self.jump(&end);
                        self.start(&end);
                        self.instruction(format!(
                            "phi i1 [ false, %{absent} ], [ {}, %{inspected} ]",
                            truth.operand
                        ))
                    }
                }
            }
        } else {
            match value.ty {
                Type::JsonRoot | Type::JsonValue | Type::JsonObject => {
                    let byte = self
                        .instruction(format!("call i32 @imre_json_truth(ptr {})", value.operand));
                    self.instruction(format!("icmp ne i32 {byte}, 0"))
                }
                Type::Enum(_) => "true".into(),
                Type::StrEnum(class) => {
                    match self.module.class_metadata[class]
                        .enum_members
                        .iter()
                        .position(|(_, value)| value.is_empty())
                    {
                        Some(member) => self.instruction(format!(
                            "icmp ne i64 {}, {}",
                            value.operand,
                            self.enum_tag(class, member)
                        )),
                        None => "true".to_owned(),
                    }
                }
                Type::Bool => value.operand.clone(),
                Type::VarTuple(_) => {
                    let len = self
                        .instruction(format!("call i64 @imre_tuple_len(ptr {})", value.operand));
                    self.instruction(format!("icmp ne i64 {len}, 1"))
                }
                Type::Int => self.instruction(format!("icmp ne i64 {}, 1", value.operand)),
                // Unordered-or-not-equal keeps NaN true and both signed zeros false.
                Type::Float => self.instruction(format!("fcmp une double {}, 0.0", value.operand)),
                Type::Str | Type::StrLiteral(_) | Type::Bytes => {
                    let family = if value.ty != Type::Bytes {
                        "str"
                    } else {
                        "bytes"
                    };
                    let byte = self.instruction(format!(
                        "call i32 @imre_{family}_truthy(ptr {})",
                        value.operand
                    ));
                    self.instruction(format!("icmp ne i32 {byte}, 0"))
                }
                Type::Context => {
                    let count = self.instruction(format!(
                        "call i64 @imre_contextvars_len(ptr {})",
                        value.operand
                    ));
                    self.instruction(format!("icmp ne i64 {count}, 1"))
                }
                Type::Calendar(crate::ir::CalendarType::Delta) => self.instruction(format!(
                    "call i1 @imre_calendar_truth(ptr {})",
                    value.operand
                )),
                Type::ObjectValue => {
                    let bit = self.checked_instruction(format!(
                        "call i32 @imre_system_exit_code_truth(ptr {})",
                        value.operand
                    ));
                    self.instruction(format!("icmp ne i32 {bit}, 0"))
                }
                Type::StringIterable => {
                    let length = self.instruction(format!(
                        "call i64 @imre_string_iterable_len(i64 {})",
                        value.operand
                    ));
                    self.instruction(format!("icmp ne i64 {length}, 1"))
                }
                Type::Union(_) => {
                    let byte = self.instruction(format!(
                        "call i32 @imre_union_truthy(i64 {})",
                        value.operand
                    ));
                    self.instruction(format!("icmp ne i32 {byte}, 0"))
                }
                Type::ChainMap(_) => {
                    let byte = self.instruction(format!(
                        "call i32 @imre_chainmap_truth(ptr {})",
                        value.operand
                    ));
                    self.instruction(format!("icmp ne i32 {byte}, 0"))
                }
                Type::Patch(_) => {
                    let length = self
                        .instruction(format!("call i64 @imre_patch_len(ptr {})", value.operand));
                    self.instruction(format!("icmp ne i64 {length}, 1"))
                }
                Type::List(_) if self.module.types.element(value.ty) == Some(Type::Str) => {
                    let bit = self.checked_instruction(format!(
                        "call i32 @imre_str_list_nonempty(ptr {})",
                        value.operand
                    ));
                    self.instruction(format!("icmp ne i32 {bit}, 0"))
                }
                Type::List(_) => {
                    let length = self
                        .instruction(format!("call i32 @sequence_header(ptr {})", value.operand));
                    self.instruction(format!("icmp ugt i32 {length}, 1"))
                }
                Type::ListInt
                | Type::Deque(_)
                | Type::Dict(_)
                | Type::ReadOnlyObjectMapping
                | Type::Set(_)
                | Type::StringKeys => {
                    let family = match value.ty {
                        Type::ListInt => "list",
                        Type::Dict(_) | Type::ReadOnlyObjectMapping => "dict",
                        Type::Set(_) | Type::StringKeys => "set",
                        Type::Deque(_) => "deque",
                        _ => unreachable!(),
                    };
                    // These containers have at least one word per entry, so
                    // their lengths always fit the tagged-small representation.
                    let length = self.instruction(format!(
                        "call i64 @imre_{family}_len(ptr {})",
                        value.operand
                    ));
                    self.instruction(format!("icmp ne i64 {length}, 1"))
                }
                _ => unreachable!("fixed truth values and optionals were handled above"),
            }
        };
        Value {
            ty: Type::Bool,
            operand,
            small_integer: false,
        }
    }

    /// Only the selected predecessor transfers ownership to the result phi.
    /// The discarded operand is released before the RHS, including its errors.
    pub(super) fn boolean_value(
        &mut self,
        and: bool,
        left: &Expr,
        right: &Expr,
        returns: Type,
    ) -> Value {
        let left = self.operand(left);
        if let Some(truth) = self.module.types.truthiness(left.value.ty) {
            if truth == and {
                self.release_operand(&left);
                return self.expr(right);
            }
            let left = self.own_operand(left);
            return self.selected_operand(left, returns, !and);
        }
        let test = self.truth(&left.value);
        let (selected, next, end) = (self.label(), self.label(), self.label());
        if and {
            self.branch(&test.operand, &next, &selected);
        } else {
            self.branch(&test.operand, &selected, &next);
        }
        self.forget(&left.value);
        self.start(&selected);
        let selected_value = self.own_operand(Operand {
            value: left.value.clone(),
            ownership: left.ownership,
        });
        let selected_value = self.selected_operand(selected_value, returns, !and);
        self.forget(&selected_value);
        let selected_end = self.current.clone();
        self.jump(&end);
        self.start(&next);
        self.release_operand(&left);
        let right = self.expr(right);
        self.forget(&right);
        let right_end = self.current.clone();
        self.jump(&end);
        self.start(&end);
        Value {
            ty: returns,
            operand: self.instruction(format!(
                "phi {} [ {}, %{selected_end} ], [ {}, %{right_end} ]",
                returns.llvm(),
                selected_value.operand,
                right.operand
            )),
            small_integer: selected_value.small_integer && right.small_integer,
        }
    }

    /// Converts the already-selected value using the checker's truth-branch
    /// proof. Nullable floats need an unbox; other payloads transfer their word.
    fn selected_operand(&mut self, value: Value, returns: Type, truth: bool) -> Value {
        if value.ty == returns {
            return value;
        }
        let target = self.module.types.payload(returns).unwrap_or(returns);
        let source = self.module.types.payload(value.ty).unwrap_or(value.ty);
        if self
            .module
            .types
            .members(target)
            .is_some_and(|members| members.contains(&source))
        {
            return self.union_inject(value, returns);
        }
        let selected = self.module.types.truth_branch(value.ty, truth);
        if selected == Type::None {
            // A false optional with an always-true payload is necessarily absent.
            self.forget(&value);
            return Value {
                ty: returns,
                operand: "0".into(),
                small_integer: false,
            };
        }
        if self.module.types.payload(returns) == Some(value.ty) {
            return self.present(value, returns);
        }
        debug_assert_eq!(self.module.types.payload(value.ty), Some(returns));
        debug_assert!(truth);
        if returns == Type::Float {
            let payload = self.call("imre_float_unbox", Type::Float, &[&value]);
            self.release(&value);
            payload
        } else {
            self.forget(&value);
            let word = if returns == Type::Bool {
                self.instruction(format!("sub i64 {}, 1", value.operand))
            } else {
                value.operand
            };
            self.decode_word(word, returns)
        }
    }
}
