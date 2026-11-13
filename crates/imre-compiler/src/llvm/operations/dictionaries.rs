use super::sequences::{self, STACK_USE_GUIDELINE};
use super::{CheckedBody, CheckedValue, Entry};
use crate::ir::{Builtin, DictEntry, Expr, ExprKind, Type};
use crate::llvm::{Value, representation_kind};

impl CheckedBody<'_, '_, '_, '_> {
    fn new_dictionary(&mut self, ty: Type) -> CheckedValue {
        match &mut self.0 {
            Entry::Native(code) => {
                let (key, item) = code
                    .module
                    .types
                    .mapping(ty)
                    .expect("checked dictionary layout");
                let value = Value {
                    ty,
                    operand: code.instruction(format!(
                        "call ptr @imre_dict_new(i32 {}, i32 {})",
                        representation_kind(key, &code.module.types),
                        representation_kind(item, &code.module.types),
                    )),
                    small_integer: false,
                };
                CheckedValue::owned(code.owned(value))
            }
            Entry::Registered(frame) => CheckedValue::handle(frame.op("dict", &[])),
        }
    }

    fn insert_dictionary_pair(
        &mut self,
        dict: &CheckedValue,
        (key, value): (CheckedValue, CheckedValue),
        roots: Option<(&str, &str)>,
    ) {
        self.write_item(dict, &key, &value, None, false);
        self.release_input(value);
        self.release_input(key);
        if let Some((mark, previous)) = roots {
            self.python()
                .keep_operands(mark, dict.rooted_handle(), previous);
        }
    }

    /// A general Python key can run user code during hashing. Small displays
    /// evaluate every pair first and keep their operands live through the last store.
    fn dictionary_pairs(
        &mut self,
        ty: Type,
        entries: &[DictEntry],
        roots: Option<(&str, &str)>,
    ) -> CheckedValue {
        let limit = if roots.is_some() {
            STACK_USE_GUIDELINE / 2
        } else {
            0
        };
        sequences::build(
            self,
            entries,
            limit,
            |_| None,
            |body, entry| {
                let key = entry.key.as_ref().expect("explicit dictionary entry");
                (body.rooted_operand(key), body.rooted_operand(&entry.value))
            },
            |body, pending, _| {
                let dict = body.new_dictionary(ty);
                let count = pending.len();
                for (index, pair) in pending.into_iter().enumerate() {
                    body.insert_dictionary_pair(&dict, pair, roots.filter(|_| index + 1 == count));
                }
                dict
            },
            |body, dict, _, pair, _| body.insert_dictionary_pair(dict, pair, roots),
        )
        .0
    }

    /// Flush explicit entries before each mapping expansion and after CPython's
    /// stack-use threshold. The root mark spans each potentially raising operation.
    pub(super) fn dictionary(&mut self, ty: Type, entries: &[DictEntry]) -> CheckedValue {
        if entries.is_empty() {
            return self.new_dictionary(ty);
        }
        if matches!(self.0, Entry::Native(_)) {
            return self.dictionary_pairs(ty, entries, None);
        }
        let frame = self.python();
        let mark = frame.temporary_mark();
        frame.unwind_operands(&mark, |frame| {
            let mut body = CheckedBody::registered(frame.reborrow());
            let text = |value: &Expr| {
                matches!(
                    value.without_location().kind,
                    ExprKind::Str(_) | ExprKind::Builtin(Builtin::RegisteredString(_), _)
                )
            };
            // Literal text pairs can be inserted as they are evaluated without calling Python.
            if entries.len() > STACK_USE_GUIDELINE / 2 + 2
                && entries
                    .iter()
                    .all(|e| e.key.as_ref().is_some_and(text) && text(&e.value))
            {
                return body.dictionary_pairs(ty, entries, Some((&mark, "0")));
            }
            let mut result: Option<CheckedValue> = None;
            let mut remaining = entries;
            while let Some((entry, tail)) = remaining.split_first() {
                if entry.key.is_none() {
                    let dict = result.get_or_insert_with(|| body.new_dictionary(ty));
                    let mapping = body.rooted_operand(&entry.value);
                    body.python()
                        .op("update", &[dict.rooted_handle(), mapping.rooted_handle()]);
                    body.python()
                        .keep_operands(&mark, dict.rooted_handle(), "0");
                    remaining = tail;
                    continue;
                }
                let count = remaining
                    .iter()
                    .take(STACK_USE_GUIDELINE / 2 + 2)
                    .take_while(|entry| entry.key.is_some())
                    .count();
                let (pairs, tail) = remaining.split_at(count);
                let previous = result.as_ref().map_or("0", CheckedValue::rooted_handle);
                let part = body.dictionary_pairs(ty, pairs, Some((&mark, previous)));
                if let Some(dict) = &result {
                    body.python()
                        .op("update", &[dict.rooted_handle(), part.rooted_handle()]);
                    body.python()
                        .keep_operands(&mark, dict.rooted_handle(), "0");
                } else {
                    result = Some(part);
                }
                remaining = tail;
            }
            result.expect("nonempty dictionary display")
        })
    }
}
