//! Representation changes transfer the same exact Rc owner across a union boundary.

use super::*;

impl Emitter<'_, '_> {
    /// The payload alignment bit selects str (zero) or bytes (one). Optional
    /// bytes preserve the absent zero word instead of manufacturing tag-only 1.
    pub(super) fn union_inject(&mut self, value: Value, returns: Type) -> Value {
        let source = self.module.types.payload(value.ty).unwrap_or(value.ty);
        debug_assert!(matches!(source, Type::Str | Type::Bytes));
        let word = self.word(&value);
        let operand = if source == Type::Bytes {
            let tagged = self.instruction(format!("or i64 {word}, 1"));
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
        let word = self.instruction(format!("and i64 {}, -2", value.operand));
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
        let mut result = "false".to_owned();
        for &class in classes {
            let current = if matches!(value.ty, Type::Union(_)) {
                match (class.accepts(Type::Str), class.accepts(Type::Bytes)) {
                    (true, true) => "true".to_owned(),
                    (false, false) => "false".to_owned(),
                    (text, _) => {
                        let tag = self.instruction(format!("and i64 {}, 1", value.operand));
                        self.instruction(format!("icmp eq i64 {tag}, {}", u8::from(!text)))
                    }
                }
            } else if let InstanceType::Exception(kind) = class
                && matches!(value.ty, Type::Exception | Type::TimeoutExpired)
            {
                let matched = self.instruction(format!(
                    "call i8 @relar_exception_matches(ptr {}, i32 {kind})",
                    value.operand
                ));
                self.instruction(format!("icmp ne i8 {matched}, 0"))
            } else {
                u8::from(class.accepts(value.ty)).to_string()
            };
            result = self.instruction(format!("or i1 {result}, {current}"));
        }
        result
    }
}
