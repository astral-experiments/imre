//! Immutable JSON/TOML schemas share the ordinary native record ownership layouts.

use std::collections::HashSet;

use crate::ir::{self, JsonDefault};

use super::*;

pub(super) fn schemas(module: &Module) -> String {
    let mut roots = Vec::new();
    for function in module.functions.iter().chain([&module.entry]) {
        ir::walk_expressions(&function.body, &mut |value| {
            if let ExprKind::Builtin(
                Builtin::JsonDecode(ty) | Builtin::TomlDecode(ty) | Builtin::JsonEncode(ty),
                _,
            ) = value.kind
            {
                roots.push(ty);
            }
        });
    }
    if roots.is_empty() {
        return String::new();
    }
    let mut builder = Schemas {
        module,
        seen: HashSet::new(),
        output: String::from(
            "%json_schema = type { i32, i32, ptr, ptr, i64, ptr }\n\
             %json_field = type { ptr, i64, ptr, i32, ptr, i64 }\n",
        ),
    };
    for ty in roots {
        builder.schema(ty);
    }
    builder.output
}

pub(super) fn schema_name(ty: Type) -> String {
    match ty {
        Type::Int => "json_int".to_owned(),
        Type::Str | Type::StrLiteral(_) => "json_str".to_owned(),
        Type::Bool => "json_bool".to_owned(),
        Type::Float => "json_float".to_owned(),
        Type::None => "json_none".to_owned(),
        Type::Class(id) => format!("json_class_{id}"),
        Type::ListInt => "json_list_int".to_owned(),
        Type::List(id) => format!("json_list_{id}"),
        Type::Dict(id) => format!("json_dict_{id}"),
        Type::Optional(id) => format!("json_optional_{id}"),
        _ => unreachable!("JSON schemas contain only validated concrete types"),
    }
}

struct Schemas<'a> {
    module: &'a Module,
    seen: HashSet<Type>,
    output: String,
}

impl Schemas<'_> {
    fn bytes(&mut self, name: &str, value: &str) {
        write!(
            self.output,
            "@{name} = private unnamed_addr constant [{} x i8] c\"",
            value.len()
        )
        .unwrap();
        for byte in value.bytes() {
            write!(self.output, "\\{byte:02X}").unwrap();
        }
        self.output.push_str("\"\n");
    }

    fn schema(&mut self, ty: Type) -> String {
        let ty = if matches!(ty, Type::StrLiteral(_)) {
            Type::Str
        } else {
            ty
        };
        let name = schema_name(ty);
        if !self.seen.insert(ty) {
            return name;
        }
        let (kind, child, layout, count, fields) = match ty {
            Type::Int => (1, None, None, 0, None),
            Type::Str => (2, None, None, 0, None),
            Type::Bool => (3, None, None, 0, None),
            Type::Float => (4, None, None, 0, None),
            Type::None => (5, None, None, 0, None),
            Type::ListInt | Type::List(_) => {
                let child = self.schema(self.module.types.element(ty).unwrap());
                (6, Some(child), None, 0, None)
            }
            Type::Optional(_) => {
                let child = self.schema(self.module.types.payload(ty).unwrap());
                (8, Some(child), None, 0, None)
            }
            Type::Dict(_) => {
                let (_, value) = self.module.types.mapping(ty).unwrap();
                let child = self.schema(value);
                (9, Some(child), None, 0, None)
            }
            Type::Class(class) => {
                let record = self
                    .module
                    .json_records
                    .iter()
                    .find(|schema| schema.class == class)
                    .unwrap();
                let mut fields = Vec::new();
                for (index, field) in record.fields.iter().enumerate() {
                    let child = self.schema(self.module.class_fields[class][index]);
                    let prefix = format!("json_field_{class}_{index}");
                    self.bytes(&format!("{prefix}_name"), &field.name);
                    let (default_kind, default_pointer, default_len) = match &field.default {
                        None => (0, "null".to_owned(), 0),
                        Some(JsonDefault::EmptyList) => (2, "null".to_owned(), 0),
                        Some(value) => {
                            let value = literal(value);
                            self.bytes(&format!("{prefix}_default"), &value);
                            (1, format!("@{prefix}_default"), value.len())
                        }
                    };
                    fields.push(format!(
                        "%json_field {{ ptr @{prefix}_name, i64 {}, ptr @{child}, i32 {default_kind}, ptr {default_pointer}, i64 {default_len} }}",
                        field.name.len(),
                    ));
                }
                let count = fields.len();
                let fields_name = format!("json_fields_{class}");
                writeln!(
                    self.output,
                    "@{fields_name} = private constant [{count} x %json_field] [{}]",
                    fields.join(", ")
                )
                .unwrap();
                (
                    7,
                    None,
                    Some(format!("layout_class_{class}")),
                    count,
                    Some(fields_name),
                )
            }
            _ => unreachable!(),
        };
        let pointer = |value: Option<String>| {
            value.map_or_else(|| "null".to_owned(), |value| format!("@{value}"))
        };
        writeln!(
            self.output,
            "@{name} = private constant %json_schema {{ i32 {kind}, i32 {}, ptr {}, ptr {}, i64 {count}, ptr {} }}",
            representation_kind(ty, &self.module.types), pointer(child), pointer(layout), pointer(fields),
        ).unwrap();
        name
    }
}

/// Defaults use complete JSON fragments; floats keep their lexical category,
/// including negative zero, when the runtime decodes an absent field.
fn literal(value: &JsonDefault) -> String {
    match value {
        JsonDefault::Int(value) => value.clone(),
        JsonDefault::Float(value) => {
            let mut text = value.to_string();
            if !text.contains(['.', 'e', 'E']) {
                text.push_str(".0");
            }
            text
        }
        JsonDefault::Bool(value) => value.to_string(),
        JsonDefault::None => "null".to_owned(),
        JsonDefault::Str(value) => {
            let mut text = String::from("\"");
            for character in value.chars() {
                match character {
                    '"' => text.push_str("\\\""),
                    '\\' => text.push_str("\\\\"),
                    '\0'..='\u{1f}' => write!(text, "\\u{:04x}", u32::from(character)).unwrap(),
                    _ => text.push(character),
                }
            }
            text.push('"');
            text
        }
        JsonDefault::EmptyList => {
            unreachable!("empty lists use their own fresh-allocation default")
        }
    }
}
