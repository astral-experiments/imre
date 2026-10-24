//! Representation changes transfer the same exact Rc owner across a union boundary.

use super::*;

fn tag(ty: Type) -> u8 {
    match ty {
        Type::Str => 0,
        Type::Bytes => 1,
        Type::List(_) => 2,
        Type::Exception => 3,
        Type::Class(_) => 4,
        Type::Enum(_) => 5,
        Type::Path => 6,
        _ => unreachable!("closed native union member"),
    }
}

impl Emitter<'_, '_> {
    /// The two alignment bits select str, bytes, or a string list. A nullable
    /// source preserves its absent zero word when transitioning into a union.
    pub(super) fn union_inject(&mut self, value: Value, returns: Type) -> Value {
        let source = self.module.types.payload(value.ty).unwrap_or(value.ty);
        let tag = tag(source);
        let word = self.word(&value);
        let operand = if tag != 0 {
            let bits = if matches!(source, Type::Enum(_)) {
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

    /// Local flow proves the member before projection; clearing the tag restores
    /// its address and transfers ownership without retaining or allocating.
    pub(super) fn union_project(&mut self, value: Value, returns: Type) -> Value {
        let word = if matches!(returns, Type::Enum(_)) {
            self.instruction(format!("lshr i64 {}, 3", value.operand))
        } else {
            self.instruction(format!("and i64 {}, -8", value.operand))
        };
        self.forget(&value);
        self.decode_word(word, returns)
    }

    /// Nullable exceptions are tested only on the present edge. Other exact
    /// categories need no object inspection; finite union members test their tag.
    pub(super) fn instance_test(&mut self, value: &Value, classes: &[InstanceType]) -> String {
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
        if value.ty == Type::ExitCode {
            // The view retains its heterogeneous payload. None is not a class
            // accepted by this subset, and bool is also an instance of int.
            let mask = [Type::None, Type::Bool, Type::Int, Type::Str]
                .iter()
                .enumerate()
                .filter(|(_, ty)| classes.iter().any(|class| class.accepts(**ty)))
                .fold(0, |mask, (kind, _)| mask | 1 << kind);
            let kind = self.instruction(format!(
                "call i32 @imre_system_exit_code_kind(ptr {})",
                value.operand
            ));
            let bit = self.instruction(format!("shl i32 1, {kind}"));
            let matched = self.instruction(format!("and i32 {bit}, {mask}"));
            return self.instruction(format!("icmp ne i32 {matched}, 0"));
        }
        let mut result = "false".to_owned();
        for &class in classes {
            let current = if let Some(members) = self.module.types.members(value.ty) {
                match (class.accepts(members[0]), class.accepts(members[1])) {
                    (true, true) => "true".to_owned(),
                    (false, false) => "false".to_owned(),
                    (first, _) => {
                        let selected = members[usize::from(!first)];
                        let bits = self.instruction(format!("and i64 {}, 7", value.operand));
                        let found =
                            self.instruction(format!("icmp eq i64 {bits}, {}", tag(selected)));
                        if let InstanceType::Exception(kind) = class
                            && kind != 0
                            && selected == Type::Exception
                        {
                            let (inspect, end) = (self.label(), self.label());
                            let other = self.current.clone();
                            self.branch(&found, &inspect, &end);
                            self.start(&inspect);
                            let word = self.instruction(format!("and i64 {}, -8", value.operand));
                            let payload = self.decode_word(word, Type::Exception);
                            let matches = self.instance_test(&payload, &[class]);
                            let inspected = self.current.clone();
                            self.jump(&end);
                            self.start(&end);
                            self.instruction(format!(
                                "phi i1 [ false, %{other} ], [ {matches}, %{inspected} ]"
                            ))
                        } else {
                            found
                        }
                    }
                }
            } else if let InstanceType::Exception(kind) = class
                && matches!(value.ty, Type::Exception | Type::TimeoutExpired)
            {
                let matched = self.instruction(format!(
                    "call i32 @imre_exception_matches(ptr {}, i32 {kind})",
                    value.operand
                ));
                self.instruction(format!("icmp ne i32 {matched}, 0"))
            } else {
                u8::from(class.accepts(value.ty)).to_string()
            };
            result = self.instruction(format!("or i1 {result}, {current}"));
        }
        result
    }
}
