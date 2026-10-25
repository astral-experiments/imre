//! Sealed string and byte paths share the same ASCII separator rules.

use super::*;
use crate::ir::PosixPathOperation;

impl Emitter<'_, '_> {
    pub(super) fn posix_path_builtin(
        &mut self,
        operation: PosixPathOperation,
        args: &[Operand],
        returns: Type,
    ) -> Value {
        let first = &args[0].value;
        let bytes = u8::from(first.ty == Type::Bytes);
        let operand = match operation {
            PosixPathOperation::Commonpath => {
                let kind = match self.module.types.element(first.ty) {
                    Some(Type::Str) => 0,
                    Some(Type::Bytes) => 1,
                    Some(Type::Union(_)) => 2,
                    Some(Type::Path) => 3,
                    _ => 4,
                };
                let handle = if first.ty == Type::None {
                    "null"
                } else {
                    &first.operand
                };
                let raw = self.checked_instruction(format!(
                    "call i64 @imre_posix_commonpath(ptr {handle}, i32 {kind})"
                ));
                if matches!(returns, Type::Union(_)) {
                    raw
                } else {
                    self.instruction(format!("inttoptr i64 {raw} to ptr"))
                }
            }
            PosixPathOperation::Join => {
                let handles = self.entry_alloca(format!("[{} x ptr]", args.len()));
                for (index, arg) in args.iter().enumerate() {
                    let slot =
                        self.instruction(format!("getelementptr ptr, ptr {handles}, i64 {index}"));
                    self.line(format!("store ptr {}, ptr {slot}", arg.value.operand));
                }
                self.checked_pointer(format!(
                    "call ptr @imre_posix_join(ptr {handles}, i64 {}, i32 {bytes})",
                    args.len()
                ))
            }
            PosixPathOperation::MixedJoin { path_in_tail } => {
                let mut unions = args[1..]
                    .iter()
                    .filter(|arg| matches!(arg.value.ty, Type::Union(_)));
                let flag = if path_in_tail {
                    "1".to_owned()
                } else if let Some(first) = unions.next() {
                    let path = [InstanceType::Exact(Type::Path)];
                    let mut found = self.instance_test(&first.value, &path);
                    for arg in unions {
                        let next = self.instance_test(&arg.value, &path);
                        found = self.instruction(format!("or i1 {found}, {next}"));
                    }
                    self.instruction(format!("zext i1 {found} to i32"))
                } else {
                    "0".to_owned()
                };
                self.checked_instruction(format!("call ptr @imre_posix_join_error(i32 {flag})"))
            }
            PosixPathOperation::Basename | PosixPathOperation::Dirname => {
                let head = u8::from(operation == PosixPathOperation::Dirname);
                self.checked_pointer(format!(
                    "call ptr @imre_posix_component(ptr {}, i32 {bytes}, i32 {head})",
                    first.operand
                ))
            }
            PosixPathOperation::Splitext => {
                let tuple = self.record(returns);
                self.checked_line(format!(
                    "call void @imre_posix_splitext(ptr {}, i32 {bytes}, ptr {})",
                    first.operand, tuple.operand
                ));
                return tuple;
            }
            PosixPathOperation::Normpath | PosixPathOperation::Abspath => {
                let absolute = u8::from(operation == PosixPathOperation::Abspath);
                self.checked_instruction(format!(
                    "call ptr @imre_posix_normalize(ptr {}, i32 {bytes}, i32 {absolute})",
                    first.operand
                ))
            }
        };
        self.owned(Value {
            ty: returns,
            operand,
            small_integer: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::{CompileOptions, compile};

    #[test]
    fn check_path_allocation_errors_before_using_outputs() {
        let llvm = compile(
            r#"import os.path
def transform(p: bytes) -> bytes:
    base, ext = os.path.splitext(p)
    return os.path.join(os.path.dirname(base), os.path.basename(ext))
try:
    print(transform(b"dir/name.x"))
except MemoryError:
    print("caught")
"#,
            &CompileOptions::default(),
        )
        .unwrap();
        let mut checked = 0;
        let mut lines = llvm.lines();
        while let Some(line) = lines.next() {
            if line.contains("call ptr @imre_posix_join(")
                || line.contains("call ptr @imre_posix_component(")
            {
                assert!(lines.next().unwrap().contains("icmp eq ptr "));
                checked += 1;
            } else if line.contains("call void @imre_posix_splitext(") {
                assert!(
                    lines
                        .next()
                        .unwrap()
                        .contains("call i32 @imre_error_pending()")
                );
                checked += 1;
            }
        }
        assert_eq!(checked, 4);
    }
}
