//! Text results use existing string, sequence, and fixed tuple ownership.

use super::*;
use crate::ir::{TextOperation, TextPredicate};

impl Emitter<'_, '_> {
    /// Elides a temporary split sequence only when its selected field is known.
    /// Ordinary argument evaluation and rooting still precede separator checks.
    pub(super) fn rsplit_last(&mut self, builtin: &Builtin, expressions: &[Expr]) -> Option<Value> {
        let (expressions, function) = match (builtin, expressions) {
            (Builtin::ListGet, args) => (args, "imre_str_rsplit_last"),
            (Builtin::Text(TextOperation::Unquote), [arg]) => match &arg.kind {
                ExprKind::Builtin(Builtin::ListGet, args) => {
                    (args.as_slice(), "imre_str_unquote_rsplit_last")
                }
                _ => return None,
            },
            _ => return None,
        };
        let [split, index] = expressions else {
            return None;
        };
        let args = split.last_rsplit_args(index)?;
        let args = args.iter().map(|arg| self.operand(arg)).collect::<Vec<_>>();
        let separator = self.instruction(format!("inttoptr i64 {} to ptr", args[1].value.operand));
        let operand = self.checked_instruction(format!(
            "call ptr @{function}(ptr {}, ptr {separator})",
            args[0].value.operand,
        ));
        let result = self.owned(Value {
            ty: Type::Str,
            operand,
            small_integer: false,
        });
        for arg in &args {
            self.release_operand(arg);
        }
        Some(result)
    }

    /// Emits text calls with rooted inputs and existing result ownership layouts.
    pub(super) fn text_builtin(
        &mut self,
        operation: TextOperation,
        args: &[Operand],
        returns: Type,
    ) -> Value {
        let source = &args[0].value.operand;
        let operand = match operation {
            TextOperation::Predicate(predicate) => {
                let symbol = match predicate {
                    TextPredicate::Alnum => "imre_str_isalnum",
                    TextPredicate::Space => "imre_str_isspace",
                    TextPredicate::Upper => "imre_str_is_upper",
                };
                let bit = self.instruction(format!("call i32 @{symbol}(ptr {source})"));
                self.instruction(format!("icmp ne i32 {bit}, 0"))
            }
            TextOperation::SplitLineEnds => {
                let keep = self.instruction(format!("zext i1 {} to i32", args[1].value.operand));
                self.checked_instruction(format!(
                    "call ptr @imre_str_splitlines_options(ptr {source}, i32 {keep})"
                ))
            }
            TextOperation::ExpandTabs => self.checked_instruction(format!(
                "call ptr @imre_str_expandtabs(ptr {source}, i64 {})",
                args[1].value.operand,
            )),
            TextOperation::Lower => {
                self.checked_instruction(format!("call ptr @imre_str_lower(ptr {source})"))
            }
            TextOperation::Upper => {
                self.checked_instruction(format!("call ptr @imre_str_upper(ptr {source})"))
            }
            TextOperation::Capitalize => {
                self.checked_instruction(format!("call ptr @imre_str_capitalize(ptr {source})"))
            }
            TextOperation::Split(reverse) => {
                let separator =
                    self.instruction(format!("inttoptr i64 {} to ptr", args[1].value.operand));
                self.checked_instruction(format!(
                    "call ptr @imre_str_split(ptr {source}, ptr {separator}, i64 {}, i32 {})",
                    args[2].value.operand,
                    u8::from(reverse),
                ))
            }
            TextOperation::Strip(mode) => {
                let chars =
                    self.instruction(format!("inttoptr i64 {} to ptr", args[1].value.operand));
                self.instruction(format!(
                    "call ptr @imre_str_strip(ptr {source}, ptr {chars}, i32 {mode})"
                ))
            }
            TextOperation::StartsWith | TextOperation::EndsWith => {
                let byte = self.instruction(format!(
                    "call i32 @imre_str_affix(ptr {source}, ptr {}, i32 {})",
                    args[1].value.operand,
                    u8::from(matches!(operation, TextOperation::EndsWith)),
                ));
                self.instruction(format!("icmp ne i32 {byte}, 0"))
            }
            TextOperation::Find => self.instruction(format!(
                "call i64 @imre_str_find(ptr {source}, ptr {}, i64 {})",
                args[1].value.operand, args[2].value.operand,
            )),
            TextOperation::Partition => {
                // A failed partition leaves zeroed fields, so ordinary pending
                // error cleanup can safely release the already rooted record.
                let result = self.record(returns);
                self.checked_line(format!(
                    "call void @imre_str_partition(ptr {source}, ptr {}, ptr {})",
                    args[1].value.operand, result.operand,
                ));
                return result;
            }
            TextOperation::Translate => self.checked_instruction(format!(
                "call ptr @imre_str_translate(ptr {source}, ptr {})",
                args[1].value.operand
            )),
            TextOperation::Dedent => {
                self.instruction(format!("call ptr @imre_str_dedent(ptr {source})"))
            }
            TextOperation::UrlSplit => {
                let bytes = i32::from(args[0].value.ty == Type::Bytes);
                let scheme_bytes = i32::from(args[1].value.ty == Type::Bytes);
                let fragments =
                    self.instruction(format!("zext i1 {} to i32", args[2].value.operand));
                let result = self.record(returns);
                self.checked_line(format!("call void @imre_url_split(ptr {source}, i32 {bytes}, ptr {}, i32 {scheme_bytes}, i32 {fragments}, ptr {})", args[1].value.operand, result.operand));
                return result;
            }
            TextOperation::UrlProperty(property) => {
                let bytes = i32::from(args[0].value.ty == Type::Bytes);
                self.checked_instruction(format!(
                    "call i64 @imre_url_property(ptr {source}, i32 {bytes}, i32 {})",
                    property as u32
                ))
            }
            TextOperation::UrlTranscode { encode } => {
                let bytes = i32::from(encode);
                let result = self.record(returns);
                self.checked_line(format!("call void @imre_url_transcode(ptr {source}, i32 {bytes}, ptr {}, ptr {}, ptr {})", args[1].value.operand, args[2].value.operand, result.operand));
                return result;
            }
            TextOperation::UrlGetUrl => {
                let bytes = i32::from(returns == Type::Bytes);
                self.checked_pointer(format!(
                    "call ptr @imre_url_geturl(ptr {source}, i32 {bytes})"
                ))
            }
            TextOperation::Unquote => {
                self.checked_pointer(format!("call ptr @imre_str_unquote(ptr {source})"))
            }
            TextOperation::ShlexSplit => {
                let source = self.instruction(format!("inttoptr i64 {source} to ptr"));
                let comments =
                    self.instruction(format!("zext i1 {} to i32", args[1].value.operand));
                let posix = self.instruction(format!("zext i1 {} to i32", args[2].value.operand));
                self.checked_instruction(format!(
                    "call ptr @imre_shlex_split_options(ptr {source}, i32 {comments}, i32 {posix})"
                ))
            }
            TextOperation::ShlexQuote => {
                self.checked_instruction(format!("call ptr @imre_shlex_quote(ptr {source})"))
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
    fn unquote_borrows_only_an_immediately_selected_tail() {
        for (body, fused) in [
            ("return unquote(url.rsplit(sep, 1)[-1])", true),
            (
                "tail = url.rsplit(sep, 1)[-1]\n    return unquote(tail)",
                false,
            ),
            ("return unquote(url.rsplit(sep, limit)[-1])", false),
        ] {
            let source = format!(
                "from urllib.parse import unquote\ndef name(url: str, sep: str, limit: int) -> str:\n    {body}\n"
            );
            let llvm = compile(&source, &CompileOptions::default()).unwrap();
            assert_eq!(
                llvm.contains("call ptr @imre_str_unquote_rsplit_last("),
                fused
            );
            assert_eq!(llvm.contains("call ptr @imre_str_unquote("), !fused);
        }
    }

    #[test]
    fn only_ephemeral_right_splits_with_literal_selection_are_fused() {
        for (parameters, body, fused) in [
            (
                "source: str, sep: str",
                "return source.rsplit(sep, 1)[-1]",
                true,
            ),
            (
                "source: str, sep: str",
                "parts = source.rsplit(sep, 1)\n    return parts[-1]",
                false,
            ),
            (
                "source: str, sep: str, limit: int",
                "return source.rsplit(sep, limit)[-1]",
                false,
            ),
            (
                "source: str, sep: str | None",
                "return source.rsplit(sep, 1)[-1]",
                false,
            ),
            (
                "source: str, sep: str, index: int",
                "return source.rsplit(sep, 1)[index]",
                false,
            ),
            (
                "source: str, sep: str",
                "return source.rsplit(maxsplit=1, sep=sep)[-1]",
                false,
            ),
            (
                "source: str, sep: str",
                "return source.split(sep, 1)[-1]",
                false,
            ),
        ] {
            let source = format!("def tail({parameters}) -> str:\n    {body}\n");
            let llvm = compile(&source, &CompileOptions::default()).unwrap();
            assert_eq!(
                llvm.contains("call ptr @imre_str_rsplit_last("),
                fused,
                "{source}",
            );
            assert_eq!(
                llvm.contains("call ptr @imre_str_split("),
                !fused,
                "{source}"
            );
        }
    }
}
