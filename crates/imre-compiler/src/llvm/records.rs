//! Static generated record behavior shares the ordinary destruction descriptor.

use super::*;

/// Recognizes constructions whose only work after evaluating the arguments is
/// writing each field once in order. The previous record may release
/// its fields during those writes, so admit only fields with resource-free contents.
pub(super) fn replacement_fields<'a>(module: &Module, expression: &'a Expr) -> Option<&'a [Expr]> {
    if let ExprKind::Tuple(fields) = &expression.kind {
        return (!module
            .types
            .may_own_file(expression.ty, &module.class_fields, true))
        .then_some(fields);
    }
    let (Type::Class(class), ExprKind::Construct { initializer, args }) =
        (expression.ty, &expression.kind)
    else {
        return None;
    };
    let function = &module.functions[*initializer];
    let (receiver, params) = function.params.split_first()?;
    let fields = &module.class_fields[class];
    if fields.is_empty()
        || function.body.len() != fields.len()
        || args.order.iter().copied().ne(0..fields.len())
    {
        return None;
    }
    for (field, (statement, ty)) in function.body.iter().zip(fields).enumerate() {
        let Statement::SetField(object, index, value) = statement else {
            return None;
        };
        if *index != field
            || value.ty != *ty
            || args.values[field].ty != *ty
            || !matches!(object.kind, ExprKind::Local(slot) if slot == *receiver)
            || !matches!(value.kind, ExprKind::Local(slot) if params.get(field) == Some(&slot))
            || !matches!(
                ty,
                Type::Int | Type::Bool | Type::Float | Type::Str | Type::Bytes | Type::ListInt
            )
        {
            return None;
        }
    }
    Some(&args.values)
}

fn bytes(output: &mut String, name: &str, value: &str) {
    write!(
        output,
        "@{name} = private unnamed_addr constant [{} x i8] c\"",
        value.len()
    )
    .unwrap();
    for byte in value.bytes() {
        write!(output, "\\{byte:02X}").unwrap();
    }
    output.push_str("\"\n");
}

/// Names and field flags are shared constants. Record allocation size, field
/// offsets, and ownership operations do not depend on generated value behavior.
pub(super) fn semantics(module: &Module, ty: Type, name: &str, output: &mut String) -> String {
    let (kind, flags, class_name, qualified_name, fields) = match ty {
        Type::Class(_) | Type::CompletedProcess(_) => {
            let process;
            let metadata = if let Type::Class(class) = ty {
                let Some(metadata) = module.class_metadata.get(class) else {
                    return "null".to_owned();
                };
                metadata
            } else {
                let Type::CompletedProcess(capture) = ty else {
                    unreachable!()
                };
                process = crate::ir::ClassMetadata {
                    enum_members: Vec::new(),
                    name: "CompletedProcess".to_owned(),
                    qualified_name: "subprocess.CompletedProcess".to_owned(),
                    frozen_initializer: None,
                    dataclass_eq: false,
                    dataclass_repr: true,
                    toml_hash: false,
                    fields: ["args", "returncode", "stdout", "stderr"]
                        .into_iter()
                        .map(|name| crate::ir::ClassFieldMetadata {
                            name: name.to_owned(),
                            compare: false,
                            repr: match name {
                                "stdout" => capture.stdout,
                                "stderr" => capture.stderr,
                                _ => true,
                            },
                        })
                        .collect(),
                };
                &process
            };
            bytes(output, &format!("repr_name_{name}"), &metadata.name);
            bytes(
                output,
                &format!("repr_qualified_{name}"),
                &metadata.qualified_name,
            );
            let mut fields = Vec::new();
            for (index, field) in metadata.fields.iter().enumerate() {
                let field_name = format!("repr_field_{name}_{index}");
                bytes(output, &field_name, &field.name);
                let flags = u32::from(field.compare) | (u32::from(field.repr) << 1);
                fields.push(format!(
                    "{{ ptr, i64, i32 }} {{ ptr @{field_name}, i64 {}, i32 {flags} }}",
                    field.name.len()
                ));
            }
            writeln!(
                output,
                "@repr_fields_{name} = private constant [{} x {{ ptr, i64, i32 }}] [{}]",
                fields.len(),
                fields.join(", ")
            )
            .unwrap();
            (
                0,
                u32::from(metadata.dataclass_eq)
                    | (u32::from(metadata.dataclass_repr) << 1)
                    | (u32::from(metadata.toml_hash) << 2),
                format!("ptr @repr_name_{name}, i64 {}", metadata.name.len()),
                format!(
                    "ptr @repr_qualified_{name}, i64 {}",
                    metadata.qualified_name.len()
                ),
                format!("@repr_fields_{name}"),
            )
        }
        Type::Tuple(_) => (
            1,
            3,
            "ptr null, i64 0".to_owned(),
            "ptr null, i64 0".to_owned(),
            "null".to_owned(),
        ),
        _ => unreachable!("only public record layouts have value behavior"),
    };
    writeln!(
        output,
        "@semantics_{name} = private constant {{ i32, i32, ptr, i64, ptr, i64, ptr }} {{ i32 {kind}, i32 {flags}, {class_name}, {qualified_name}, ptr {fields} }}"
    )
    .unwrap();
    format!("@semantics_{name}")
}

impl Emitter<'_, '_> {
    /// Formatting borrows the exact typed root and returns a single owned string.
    /// The descriptor traversal never calls user code or mutates child storage.
    pub(super) fn value_repr(&mut self, value: &Value) -> Value {
        let word = self.word(value);
        let operand = self.instruction(format!(
            "call ptr @imre_value_repr(i64 {word}, i32 {})",
            representation_kind(value.ty, &self.module.types)
        ));
        self.owned(Value {
            ty: Type::Str,
            operand,
            small_integer: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::{CompileOptions, compile};

    #[test]
    fn loop_record_replacement_requires_only_resource_free_field_stores() {
        let compile = |source| compile(source, &CompileOptions::default()).unwrap();
        let eligible = compile(include_str!(
            "../../../../tests/fixtures/classes/record_reuse.py"
        ));
        assert!(eligible.contains("call ptr @imre_record_try_cached_result("));

        let list_fields = compile(include_str!(
            "../../../../tests/fixtures/classes/record_reuse_list.py"
        ));
        let cached = "call ptr @imre_record_try_cached_result(";
        assert!(list_fields.contains(cached));
        let streams = list_fields
            .split("\ndefine ")
            .find(|body| body.contains("call ptr @imre_text_open("))
            .unwrap();
        assert!(!streams.contains(cached));

        let ordinary = compile(
            r#"
from dataclasses import dataclass
from typing import TextIO

@dataclass(repr=False, eq=False)
class Streams:
    handles: list[TextIO]

@dataclass
class Logged:
    number: int

    def __post_init__(self) -> None:
        print(self.number)

def keep(streams: list[TextIO]) -> None:
    for i in range(2):
        holder = Streams(streams)
        logged = Logged(i)
        print(len(holder.handles) + logged.number)
"#,
        );
        assert!(!ordinary.contains("call ptr @imre_record_try_cached_result("));
    }
}
