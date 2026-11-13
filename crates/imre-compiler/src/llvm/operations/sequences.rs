//! Evaluate display operands before packing a small prefix, then consume each starred
//! or streamed operand before evaluating the following expression.
use crate::llvm::{
    Emitter, Expr, LIST_APPEND_PAIR, Operand, Ownership, Type, Value, representation_kind,
    structs::PackedRow,
};

pub(in crate::llvm) const STACK_USE_GUIDELINE: usize = 30;

pub(crate) fn build<C, T, V, R>(
    context: &mut C,
    items: &[T],
    max_buffered: usize,
    spread: impl Fn(&T) -> Option<&T>,
    mut evaluate: impl FnMut(&mut C, &T) -> V,
    start: impl FnOnce(&mut C, Vec<V>, bool) -> R,
    mut push: impl FnMut(&mut C, &R, usize, V, bool),
) -> (R, bool) {
    // CPython's starunpack_helper bounds the evaluation stack for large displays.
    let prefix = if items.len() > max_buffered {
        0
    } else {
        items
            .iter()
            .position(|item| spread(item).is_some())
            .unwrap_or(items.len())
    };
    let initial = items[..prefix]
        .iter()
        .map(|item| evaluate(context, item))
        .collect();
    let complete = prefix == items.len();
    let container = start(context, initial, complete);
    for (offset, item) in items[prefix..].iter().enumerate() {
        let iterable = spread(item);
        let value = evaluate(context, iterable.unwrap_or(item));
        push(
            context,
            &container,
            prefix + offset,
            value,
            iterable.is_some(),
        );
    }
    (container, complete)
}

enum Element {
    Value(Operand),
    Packed(PackedRow),
}

impl Emitter<'_, '_> {
    pub(in crate::llvm) fn native_sequence(
        &mut self,
        ty: Type,
        items: &[Expr],
        tuple: bool,
    ) -> Value {
        let packed = !tuple
            && self
                .module
                .types
                .element(ty)
                .is_some_and(|element| self.module.types.is_value_struct(element));
        build(
            self,
            items,
            STACK_USE_GUIDELINE,
            |_| None,
            |emitter, expr| {
                if packed && let Some(row) = emitter.packed_constructor(expr) {
                    return Element::Packed(row);
                }
                Element::Value(emitter.operand(expr))
            },
            |emitter, values, _| {
                let container = if tuple {
                    emitter.record(ty)
                } else {
                    emitter.display_mutable(ty)
                };
                let mut elements = values.into_iter().enumerate();
                while let Some((index, value)) = elements.next() {
                    if ty == Type::ListInt
                        && let Some((_, second)) = elements.next()
                    {
                        let (Element::Value(first), Element::Value(second)) = (value, second)
                        else {
                            unreachable!("integer list operands")
                        };
                        emitter.runtime_call(
                            &LIST_APPEND_PAIR,
                            [&container, &first.value, &second.value],
                        );
                        emitter.release_operand(&second);
                        emitter.release_operand(&first);
                    } else {
                        emitter.store_display_element(&container, tuple, index, value);
                    }
                }
                container
            },
            |emitter, container, index, value, _| {
                emitter.store_display_element(container, tuple, index, value)
            },
        )
        .0
    }

    fn store_display_element(
        &mut self,
        container: &Value,
        tuple: bool,
        index: usize,
        item: Element,
    ) {
        match item {
            Element::Packed(row) => self.append_packed(container, row),
            Element::Value(value) => {
                if matches!(container.ty, Type::Set(_)) {
                    self.set_member(container, &value.value, true);
                } else if tuple {
                    self.set_field(container, index, &value.value);
                } else {
                    self.append(container, &value);
                    return;
                }
                self.release_operand(&value);
            }
        }
    }

    /// Both operands have been evaluated and are rooted across the copy. Each
    /// loop polls before its length check and owns only the current element.
    pub(in crate::llvm) fn concat_lists(
        &mut self,
        left: &Value,
        right: &Value,
        element: Type,
    ) -> Value {
        let result = self.display_mutable(left.ty);
        let cursor = self.entry_alloca("i64");
        for source in [left, right] {
            // Zero and each following index use the small tagged integer representation.
            self.line(format!("store i64 1, ptr {cursor}"));
            super::iteration::loop_region(
                self,
                |e| &mut e.body,
                Self::label,
                |e, edges| e.jump(&edges.step),
                |e, yes, no| {
                    e.check_interrupt();
                    let index = Value {
                        ty: Type::Int,
                        operand: e.instruction(format!("load i64, ptr {cursor}")),
                        small_integer: true,
                    };
                    let length = e.container_length(source);
                    let more = e.instruction(format!(
                        "icmp ult i64 {}, {}",
                        index.operand, length.operand
                    ));
                    e.branch(&more, yes, no);
                    index
                },
                |e, _, index| {
                    let item = Operand {
                        value: e.get_item(source, &index, element, true),
                        ownership: Ownership::Owned,
                    };
                    e.append(&result, &item);
                    let next = e.instruction(format!("add nuw nsw i64 {}, 2", index.operand));
                    e.line(format!("store i64 {next}, ptr {cursor}"));
                },
                None::<fn(&mut Self)>,
            );
        }
        result
    }

    fn display_mutable(&mut self, ty: Type) -> Value {
        if ty == Type::ListInt {
            self.call("imre_list_new", ty, &[])
        } else {
            let op = if let Some(element) = self.module.types.set_element(ty) {
                format!(
                    "call ptr @imre_set_new(i32 {})",
                    representation_kind(element, &self.module.types)
                )
            } else {
                let element = self.module.types.element(ty).unwrap();
                if self.module.types.is_value_struct(element) {
                    let layout = self.output.record_layout(element);
                    format!("call ptr @imre_struct_list_new(ptr @{layout})")
                } else {
                    format!(
                        "call ptr @imre_seq_new(i32 {})",
                        representation_kind(element, &self.module.types)
                    )
                }
            };
            let value = Value {
                ty,
                operand: self.instruction(op),
                small_integer: false,
            };
            self.owned(value)
        }
    }
}
