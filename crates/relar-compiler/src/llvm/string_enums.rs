//! One-based nominal tags share a process-constant table of pooled text roots.

use super::*;

impl Emitter<'_, '_> {
    pub(super) fn prepare_enum_members(&mut self) {
        let module = self.module;
        for class in &module.class_metadata {
            for (name, value) in &class.enum_members {
                let fields = [&class.name, name, value]
                    .map(|text| self.intern_literal(text.as_bytes(), Type::Str));
                self.enum_members.push(fields);
            }
        }
    }

    pub(super) fn enum_tag(&self, class: usize, member: usize) -> usize {
        1 + member
            + self.module.class_metadata[..class]
                .iter()
                .map(|class| class.enum_members.len())
                .sum::<usize>()
    }

    pub(super) fn enum_table(&self) -> String {
        if self.enum_members.is_empty() {
            return String::new();
        }
        let rows = self
            .enum_members
            .iter()
            .map(|[class, name, value]| {
                format!(
                    "[3 x ptr] [ptr @literal_{class}, ptr @literal_{name}, ptr @literal_{value}]"
                )
            })
            .collect::<Vec<_>>();
        format!(
            "@str_enum_members = private constant [{} x [3 x ptr]] [{}]\n",
            rows.len(),
            rows.join(", ")
        )
    }

    pub(super) fn enum_text(&mut self, tag: &Value, name: bool) -> Value {
        let index = self.instruction(format!("sub nuw i64 {}, 1", tag.operand));
        let field = if name { 1 } else { 2 };
        let ptr = self.instruction(format!("getelementptr inbounds [{} x [3 x ptr]], ptr @str_enum_members, i64 0, i64 {index}, i64 {field}", self.enum_members.len()));
        let root = self.instruction(format!("load ptr, ptr {ptr}"));
        let result = Value {
            ty: Type::Str,
            operand: self.instruction(format!("load ptr, ptr {root}")),
            small_integer: false,
        };
        self.retain(&result);
        result
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn tags_and_nullable_tags_pass_as_scalar_words() {
        let llvm = crate::compile(
            r#"
from enum import StrEnum
class S(StrEnum):
    ON = "on"
    OFF = "off"
def same(a: S, b: S | None) -> bool:
    return a is b
def label(s: S) -> str:
    return s.name
print(same(S.ON, S.OFF))
print(label(S.ON))
"#,
            &crate::CompileOptions::default(),
        )
        .unwrap();
        assert!(llvm.contains("@str_enum_members = private constant [2 x [3 x ptr]]"));
        assert!(llvm.contains("icmp eq i64"));
        assert!(llvm.contains("getelementptr inbounds [2 x [3 x ptr]]"));
        assert!(
            !llvm
                .lines()
                .any(|line| line.contains("call ptr @relar_record_new("))
        );
    }
}
