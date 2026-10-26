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
    let named = module.types.tuple_metadata(ty);
    let (kind, flags, class_name, qualified_name, fields) = match ty {
        Type::Class(class) => {
            let Some(metadata) = module.class_metadata.get(class) else {
                return "null".to_owned();
            };
            let flags = u32::from(metadata.dataclass_eq)
                | (u32::from(metadata.dataclass_repr) << 1)
                | (u32::from(metadata.toml_hash) << 2)
                | (u32::from(metadata.value_struct) << 3)
                | (u32::from(metadata.fields.iter().any(|f| f.repr_text)) << 4);
            // The runtime walks the physical layout. Hidden cache and shadow fields
            // own their payloads, but have no public name, equality, or repr role.
            let layout = (0..module.class_fields[class].len()).map(|index| {
                metadata.fields.get(index).map_or(("", 0), |field| {
                    (
                        field.name.as_str(),
                        u32::from(field.compare)
                            | (u32::from(field.repr) << 1)
                            | (u32::from(field.repr_text) << 2),
                    )
                })
            });
            (
                0,
                flags,
                metadata.name.as_str(),
                metadata.qualified_name.as_str(),
                field_metadata(output, name, layout),
            )
        }
        Type::ModuleSpec(_) => {
            let layout = [
                ("name", 2),
                ("loader", 2),
                ("origin", 2),
                ("loader_state", 0),
                ("submodule_search_locations", 2),
                ("_uninitialized_submodules", 0),
                ("_set_fileattr", 0),
                ("_cached", 0),
            ];
            (
                2,
                2,
                "ModuleSpec",
                "_frozen_importlib.ModuleSpec",
                field_metadata(output, name, layout.into_iter()),
            )
        }
        Type::SysImplementation => (
            0,
            2,
            "namespace",
            "namespace",
            field_metadata(
                output,
                name,
                crate::version::IMPLEMENTATION
                    .iter()
                    .map(|field| (*field, 2)),
            ),
        ),
        Type::CompletedProcess(capture, _) => {
            let layout = [
                ("args", true),
                ("returncode", true),
                ("stdout", capture.stdout),
                ("stderr", capture.stderr),
            ]
            .into_iter()
            .map(|(field, repr)| (field, u32::from(repr) << 1));
            (
                0,
                2,
                "CompletedProcess",
                "subprocess.CompletedProcess",
                field_metadata(output, name, layout),
            )
        }
        Type::Tuple(_) if named.is_some() => {
            let declaration = named.as_ref().unwrap();
            // stat_result appends raw numeric attributes after the visible tuple.
            let flags = 3
                | (u32::from(module.types.is_stat_result(ty)) << 3)
                | (u32::from(module.types.tuple_origin(ty) == Some(crate::types::STRUCT_TIME))
                    << 4);
            (
                1,
                flags,
                declaration.name.as_str(),
                declaration.name.as_str(),
                field_metadata(
                    output,
                    name,
                    declaration.fields.iter().map(|field| (field.as_str(), 3)),
                ),
            )
        }
        Type::Tuple(_) => (1, 3, "", "", "null".to_owned()),
        _ => unreachable!("only public record layouts have value behavior"),
    };
    let names = |output: &mut String, prefix: &str, value: &str| {
        if value.is_empty() {
            return "ptr null, i64 0".to_owned();
        }
        let symbol = format!("repr_{prefix}_{name}");
        bytes(output, &symbol, value);
        format!("ptr @{symbol}, i64 {}", value.len())
    };
    let class_name = names(output, "name", class_name);
    let qualified_name = names(output, "qualified", qualified_name);
    writeln!(
        output,
        "@semantics_{name} = private constant {{ i32, i32, ptr, i64, ptr, i64, ptr }} {{ i32 {kind}, i32 {flags}, {class_name}, {qualified_name}, ptr {fields} }}"
    )
    .unwrap();
    format!("@semantics_{name}")
}

fn field_metadata<'a>(
    output: &mut String,
    name: &str,
    layout: impl Iterator<Item = (&'a str, u32)>,
) -> String {
    let mut fields = Vec::new();
    for (index, (field, flags)) in layout.enumerate() {
        if field.is_empty() {
            fields.push("{ ptr, i64, i32 } zeroinitializer".to_owned());
            continue;
        }
        let symbol = format!("repr_field_{name}_{index}");
        bytes(output, &symbol, field);
        fields.push(format!(
            "{{ ptr, i64, i32 }} {{ ptr @{symbol}, i64 {}, i32 {flags} }}",
            field.len()
        ));
    }
    writeln!(
        output,
        "@repr_fields_{name} = private constant [{} x {{ ptr, i64, i32 }}] [{}]",
        fields.len(),
        fields.join(", ")
    )
    .unwrap();
    format!("@repr_fields_{name}")
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
        let failed = self.instruction(format!("icmp eq ptr {operand}, null"));
        self.check_failed(&failed);
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
    fn repr_checks_for_failure_before_using_the_result() {
        let source = "from collections import Counter\ncounts: Counter[int] = Counter()\ntry:\n    print(len(repr(counts)))\nexcept MemoryError:\n    print(0)\n";
        let ir = compile(source, &CompileOptions::default()).unwrap();
        let mut lines = ir
            .lines()
            .skip_while(|line| !line.contains(" = call ptr @imre_value_repr("));
        let result = lines.next().unwrap().trim().split_once(" = ").unwrap().0;
        assert!(
            lines
                .next()
                .unwrap()
                .contains(&format!("icmp eq ptr {result}, null"))
        );
        assert!(lines.next().unwrap().trim_start().starts_with("br i1 "));
    }

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
