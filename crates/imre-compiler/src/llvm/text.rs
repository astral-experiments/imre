//! Text results use existing string, sequence, and fixed tuple ownership.

use super::*;
use crate::ir::{TextOperation, TextPredicate};

impl Emitter<'_, '_> {
    /// Reject long ASCII inputs whose folded prefix is absent from a small literal set.
    /// Short inputs use the existing UTF-8 guard. Non-ASCII input keeps the ordinary case conversion.
    pub(super) fn excluded_literal_prefix(
        &mut self,
        text: &str,
        mask: u64,
        tags: &[u32],
    ) -> String {
        let end_slot = self.entry_alloca("ptr");
        let start = self.instruction(format!(
            "call ptr @imre_str_bounds(ptr {text}, ptr {end_slot})"
        ));
        let end = self.instruction(format!("load ptr, ptr {end_slot}"));
        let start_offset = self.instruction(format!("ptrtoint ptr {start} to i64"));
        let end_offset = self.instruction(format!("ptrtoint ptr {end} to i64"));
        let length = self.instruction(format!("sub i64 {end_offset}, {start_offset}"));
        let has_prefix = self.instruction(format!("icmp uge i64 {length}, 3"));
        let short = self.label();
        let full = self.label();
        let matched = self.label();
        let other = self.label();
        let done = self.label();
        self.branch(&has_prefix, &full, &short);

        self.start(&short);
        let short_rejected = self.instruction(format!(
            "call i32 @imre_str_excludes_ascii_prefix3(ptr {text}, i64 {mask})"
        ));
        self.jump(&done);

        self.start(&full);
        let [a, b, c] = std::array::from_fn(|index| {
            let address = self.instruction(format!("getelementptr i8, ptr {start}, i64 {index}"));
            let byte = self.instruction(format!("load i8, ptr {address}, align 1"));
            self.instruction(format!("zext i8 {byte} to i32"))
        });
        let high = self.instruction(format!("shl i32 {a}, 16"));
        let middle = self.instruction(format!("shl i32 {b}, 8"));
        let first_two = self.instruction(format!("or i32 {high}, {middle}"));
        let raw = self.instruction(format!("or i32 {first_two}, {c}"));
        let folded = self.instruction(format!("or i32 {raw}, {}", 0x20_20_20));
        self.line(format!("switch i32 {folded}, label %{other} ["));
        for tag in tags {
            self.line(format!("  i32 {tag}, label %{matched}"));
        }
        self.line("]");

        self.start(&other);
        let high_bits = self.instruction(format!("and i32 {raw}, {}", 0x80_80_80));
        let ascii = self.instruction(format!("icmp eq i32 {high_bits}, 0"));
        let rejected = self.instruction(format!("zext i1 {ascii} to i32"));
        self.jump(&done);
        self.start(&matched);
        self.jump(&done);
        self.start(&done);
        self.instruction(format!(
            "phi i32 [{short_rejected}, %{short}], [{rejected}, %{other}], [0, %{matched}]"
        ))
    }

    /// Tests alternatives on one local string by inspecting its first or last byte once.
    pub(super) fn ascii_affix_alternatives(&mut self, values: &[Expr]) -> Option<Value> {
        /// A one-byte UTF-8 literal is ASCII, so it fits in one of two masks.
        fn alternative(expr: &Expr) -> Option<(&Expr, usize, bool, u8)> {
            let ExprKind::Builtin(Builtin::Text(op), args) = &expr.kind else {
                return None;
            };
            let reverse = match op {
                TextOperation::StartsWith => false,
                TextOperation::EndsWith => true,
                _ => return None,
            };
            let [source, affix] = args.as_slice() else {
                return None;
            };
            let ExprKind::Local(slot) = source.kind else {
                return None;
            };
            let ExprKind::Str(text) = &affix.kind else {
                return None;
            };
            let &[byte] = text.as_bytes() else {
                return None;
            };
            Some((source, slot, reverse, byte))
        }
        let [first, _, ..] = values else { return None };
        let (source, slot, reverse, _) = alternative(first)?;
        // Short file lines already compare literals directly against their inline data.
        if self.line_slots[slot] {
            return None;
        }
        let mut masks = [0_u64; 2];
        for expr in values {
            let (_, other_slot, other_reverse, byte) = alternative(expr)?;
            if slot != other_slot || reverse != other_reverse {
                return None;
            }
            masks[usize::from(byte / 64)] |= 1_u64 << (byte % 64);
        }
        let source = self.operand(source);
        let result = self.instruction(format!(
            "call i32 @imre_str_affix_ascii(ptr {}, i64 {}, i64 {}, i32 {})",
            source.value.operand,
            masks[0],
            masks[1],
            u8::from(reverse)
        ));
        self.release_operand(&source);
        Some(Value {
            ty: Type::Bool,
            operand: self.instruction(format!("icmp ne i32 {result}, 0")),
            small_integer: false,
        })
    }

    /// Elides a temporary split sequence only when its selected field is known.
    /// Ordinary argument evaluation and rooting still precede separator checks.
    pub(super) fn selected_split(
        &mut self,
        builtin: &Builtin,
        expressions: &[Expr],
    ) -> Option<Value> {
        let (fields, unquote) = match (builtin, expressions) {
            (Builtin::ListGet(_), args) => (args, false),
            (Builtin::Text(TextOperation::Unquote), [arg]) => match &arg.kind {
                ExprKind::Builtin(Builtin::ListGet(_), args) => (args.as_slice(), true),
                _ => return None,
            },
            _ => return None,
        };
        let [split, index] = fields else { return None };
        let (reverse, args) = split.selected_split_args(index)?;
        let function = match (reverse, unquote) {
            (false, false) => "imre_str_split_first",
            (true, false) => "imre_str_rsplit_last",
            (true, true) => "imre_str_unquote_rsplit_last",
            (false, true) => return None,
        };
        let [source, separator] = args.map(|arg| self.operand(arg));
        let operand = self.checked_instruction(format!(
            "call ptr @{function}(ptr {}, ptr {})",
            source.value.operand, separator.value.operand,
        ));
        let result = self.owned(Value {
            ty: Type::Str,
            operand,
            small_integer: false,
        });
        self.release_operand(&source);
        self.release_operand(&separator);
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
                    TextPredicate::Ascii => "imre_str_is_ascii",
                    TextPredicate::Alpha => "imre_str_isalpha",
                    TextPredicate::Digit => "imre_str_isdigit",
                    TextPredicate::Numeric => "imre_str_isnumeric",
                    TextPredicate::Space => "imre_str_isspace",
                    TextPredicate::Lower => "imre_str_is_lower",
                    TextPredicate::Title => "imre_str_is_title",
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
            TextOperation::ZFill { copy } => self.checked_instruction(format!(
                "call ptr @imre_str_zfill(ptr {source}, i64 {}, i32 {})",
                args[1].value.operand,
                u32::from(copy),
            )),
            TextOperation::UnicodeProperty(property) => self.checked_instruction(format!(
                "call ptr @imre_unicode_{}(ptr {source})",
                property.name()
            )),
            TextOperation::Normalize => self.checked_instruction(format!(
                "call ptr @imre_unicode_normalize(ptr {source}, ptr {})",
                args[1].value.operand,
            )),
            TextOperation::CaseMap { title } => self.checked_instruction(format!(
                "call ptr @imre_str_case_map(ptr {source}, i32 {})",
                u32::from(title),
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
                args[1].value.operand,
                args.get(2).map_or("0", |start| &start.value.operand),
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
            TextOperation::UrlQuote | TextOperation::UrlQuotePlus => {
                let function = if matches!(operation, TextOperation::UrlQuotePlus) {
                    "imre_url_quote_plus"
                } else {
                    "imre_url_quote"
                };
                let bytes = i32::from(args[0].value.ty == Type::Bytes);
                let safe_bytes = i32::from(args[1].value.ty == Type::Bytes);
                let encoding = self.optional_object_pointer(&args[2].value);
                let errors = self.optional_object_pointer(&args[3].value);
                self.checked_instruction(format!("call ptr @{function}(ptr {source}, i32 {bytes}, ptr {}, i32 {safe_bytes}, ptr {encoding}, ptr {errors})", args[1].value.operand))
            }
            TextOperation::UrlUnquote => {
                let bytes = i32::from(args[0].value.ty == Type::Bytes);
                let encoding = self.optional_object_pointer(&args[1].value);
                let errors = self.optional_object_pointer(&args[2].value);
                self.checked_instruction(format!("call ptr @imre_url_unquote(ptr {source}, i32 {bytes}, ptr {encoding}, ptr {errors})"))
            }
            TextOperation::UrlJoin => {
                let base_bytes = i32::from(args[0].value.ty == Type::Bytes);
                let url_bytes = i32::from(args[1].value.ty == Type::Bytes);
                let fragments =
                    self.instruction(format!("zext i1 {} to i32", args[2].value.operand));
                let word = self.checked_instruction(format!("call i64 @imre_url_join(ptr {source}, i32 {base_bytes}, ptr {}, i32 {url_bytes}, i32 {fragments})", args[1].value.operand));
                if base_bytes == url_bytes {
                    self.instruction(format!("inttoptr i64 {word} to ptr"))
                } else {
                    word
                }
            }
            TextOperation::ParseQsl => {
                let query = self.optional_object_pointer(&args[0].value);
                let bytes = i32::from(
                    self.module
                        .types
                        .payload(args[0].value.ty)
                        .unwrap_or(args[0].value.ty)
                        == Type::Bytes,
                );
                let keep = self.instruction(format!("zext i1 {} to i32", args[1].value.operand));
                let strict = self.instruction(format!("zext i1 {} to i32", args[2].value.operand));
                let encoding = self.optional_object_pointer(&args[3].value);
                let errors = self.optional_object_pointer(&args[4].value);
                let max = if args[5].value.ty == Type::None {
                    "0"
                } else {
                    &args[5].value.operand
                };
                let sep = &args[6].value;
                let sep_ty = self.module.types.payload(sep.ty).unwrap_or(sep.ty);
                let separator_bytes = i32::from(sep_ty == Type::Bytes);
                let separator = if matches!(sep_ty, Type::Bytes | Type::Str | Type::StrLiteral(_)) {
                    self.optional_object_pointer(sep)
                } else {
                    "null".into()
                };
                let pair = self.module.types.element(returns).unwrap();
                let layout = self.output.record_layout(pair);
                self.checked_instruction(format!("call ptr @imre_parse_qsl(ptr {query}, i32 {bytes}, i32 {keep}, i32 {strict}, ptr {encoding}, ptr {errors}, i64 {max}, ptr {separator}, i32 {separator_bytes}, ptr @{layout})"))
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
    fn only_selects_the_first_field_of_an_ephemeral_explicit_split() {
        for (body, fused) in [
            ("s.split(sep, 1)[0]", true),
            ("s.split(sep, n)[0]", false),
            ("s.split(sep, 1)[n]", false),
            ("s.split(None, 1)[0]", false),
            ("s.rsplit(sep, 1)[0]", false),
        ] {
            let source = format!("def head(s: str, sep: str, n: int) -> str:\n    return {body}\n");
            let llvm = compile(&source, &CompileOptions::default()).unwrap();
            assert_eq!(
                llvm.contains("call ptr @imre_str_split_first("),
                fused,
                "{body}"
            );
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
