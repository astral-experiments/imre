//! Pass timeout representations without rounding arbitrary Python integers through a double.
use super::*;
use crate::ir::Readiness;

impl Emitter<'_, '_> {
    pub(super) fn readiness(&mut self, op: Readiness, args: &[Operand], returns: Type) -> Value {
        match op {
            Readiness::New => self.call("imre_poll_new", returns, &[]),
            Readiness::Register | Readiness::Modify | Readiness::Unregister => {
                let mode = match op {
                    Readiness::Register => 0,
                    Readiness::Modify => 1,
                    _ => 2,
                };
                let mask = args.get(2).map_or("0", |a| a.value.operand.as_str());
                self.checked_line(format!(
                    "call void @imre_poll_update(ptr {}, i32 {mode}, i64 {}, i64 {mask})",
                    args[0].value.operand, args[1].value.operand
                ));
                none()
            }
            Readiness::Poll | Readiness::Select => {
                let timeout = &args.last().unwrap().value;
                let tag = representation_kind(timeout.ty, &self.module.types);
                let word = self.word(timeout);
                if op == Readiness::Select {
                    let out = self.record(returns);
                    self.checked_line(format!("call void @imre_select_wait(ptr {}, ptr {}, ptr {}, i32 {tag}, i64 {word}, ptr {})", args[0].value.operand, args[1].value.operand, args[2].value.operand, out.operand));
                    return out;
                }
                let pair = self.module.types.element(returns).unwrap();
                let layout = self
                    .output
                    .global_ref(format!("layout_{}", record_name(pair)));
                let operand = self.checked_pointer(format!(
                    "call ptr @imre_poll_wait(ptr {}, i32 {tag}, i64 {word}, ptr @{layout})",
                    args[0].value.operand
                ));
                self.owned(Value {
                    ty: returns,
                    operand,
                    small_integer: false,
                })
            }
        }
    }
}
