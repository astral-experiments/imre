//! Static generated record behavior shares the ordinary destruction descriptor.

use super::*;

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
        Type::Class(_) | Type::CompletedProcess => {
            let process;
            let metadata = if let Type::Class(class) = ty {
                let Some(metadata) = module.class_metadata.get(class) else {
                    return "null".to_owned();
                };
                metadata
            } else {
                process = crate::ir::ClassMetadata {
                    enum_members: Vec::new(),
                    name: "CompletedProcess".to_owned(),
                    qualified_name: "subprocess.CompletedProcess".to_owned(),
                    dataclass_eq: false,
                    dataclass_repr: true,
                    toml_hash: false,
                    fields: ["args", "returncode", "stdout", "stderr"]
                        .into_iter()
                        .map(|name| crate::ir::ClassFieldMetadata {
                            name: name.to_owned(),
                            compare: false,
                            repr: true,
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
            "call ptr @relar_value_repr(i64 {word}, i32 {})",
            representation_kind(value.ty, &self.module.types)
        ));
        self.owned(Value {
            ty: Type::Str,
            operand,
            small_integer: false,
        })
    }
}
