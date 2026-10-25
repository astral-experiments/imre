//! Contexts and keys stay rooted while selecting or replacing a bound value.
use super::*;
use crate::ir::ContextOperation as Op;
impl Emitter<'_, '_> {
    pub(super) fn context_operation(&mut self, op: Op, args: &[Operand], returns: Type) -> Value {
        let value = |i: usize| &args[i].value;
        let ptr = |i: usize| value(i).operand.as_str();
        let operand = match op {
            Op::New | Op::CopyCurrent => self.checked_pointer(format!(
                "call ptr @imre_contextvars_new(i1 zeroext {})",
                u8::from(op == Op::CopyCurrent)
            )),
            Op::Var | Op::Set => {
                let (kind, word) = args.get(1).map_or((u32::MAX, "0".to_string()), |v| {
                    (
                        representation_kind(v.value.ty, &self.module.types),
                        self.word(&v.value),
                    )
                });
                let symbol = if op == Op::Var {
                    "imre_contextvars_var"
                } else {
                    "imre_contextvars_set"
                };
                self.checked_pointer(format!(
                    "call ptr @{symbol}(ptr {}, i32 {kind}, i64 {word})",
                    ptr(0)
                ))
            }
            Op::Read | Op::ContextRead | Op::Contains | Op::ContextContains => {
                let ctx = if args.len() == 2 { ptr(1) } else { "null" };
                if matches!(op, Op::Read | Op::ContextRead) {
                    let word = self.checked_instruction(format!(
                        "call i64 @imre_contextvars_read(ptr {ctx}, ptr {})",
                        ptr(0)
                    ));
                    return self.unpack_word(word, returns);
                }
                self.instruction(format!(
                    "call zeroext i1 @imre_contextvars_contains(ptr {ctx}, ptr {})",
                    ptr(0)
                ))
            }
            Op::WasMissing => self.instruction(format!(
                "call zeroext i1 @imre_contextvars_was_missing(ptr {})",
                ptr(0)
            )),
            Op::Old | Op::Missing => {
                let tok = if op == Op::Old { ptr(0) } else { "null" };
                self.checked_pointer(format!("call ptr @imre_contextvars_old(ptr {tok})"))
            }
            Op::Enter => {
                self.checked_pointer(format!("call ptr @imre_contextvars_enter(ptr {})", ptr(0)))
            }
            Op::Copy | Op::Name | Op::TokenVar | Op::Len => {
                let symbol = match op {
                    Op::Copy => "imre_contextvars_copy",
                    Op::Name => "imre_contextvars_name",
                    Op::TokenVar => "imre_contextvars_token_var",
                    _ => "imre_contextvars_len",
                };
                return self.call(symbol, returns, &[value(0)]);
            }
            Op::Reset | Op::ExitToken => {
                let (key, tok) = if op == Op::Reset {
                    (ptr(0), ptr(1))
                } else {
                    ("null", ptr(0))
                };
                self.checked_line(format!(
                    "call void @imre_contextvars_reset(ptr {key}, ptr {tok})"
                ));
                return none();
            }
            Op::Leave => {
                self.line(format!(
                    "call void @imre_contextvars_leave(ptr {}, ptr {})",
                    ptr(0),
                    ptr(1)
                ));
                return none();
            }
        };
        Value {
            ty: returns,
            operand,
            small_integer: false,
        }
    }
}
