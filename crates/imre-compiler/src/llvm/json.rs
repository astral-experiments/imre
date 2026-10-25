//! Immutable JSON/TOML schemas share the ordinary native record ownership layouts.

use std::collections::HashSet;

use crate::ir::{self, JsonDefault};

use super::*;

impl Emitter<'_, '_> {
    fn json_mode(&self, result: Type, compare_string: bool) -> u32 {
        let payload = self.module.types.payload(result);
        let concrete = payload.unwrap_or(result);
        let tag = match concrete {
            Type::JsonValue => 0,
            Type::JsonObject => 1,
            Type::Str => 2,
            Type::Int => 3,
            Type::Bool if compare_string => 4,
            Type::Bool => 7,
            Type::Float => 8,
            _ if self.module.types.element(concrete) == Some(Type::JsonObject) => 5,
            _ if self.module.types.element(concrete) == Some(Type::JsonValue) => 6,
            _ => unreachable!("checked JSON selection"),
        };
        tag | if payload.is_some() { 256 } else { 0 }
    }

    fn json_key(&mut self, key: &str) -> String {
        let name = self.byte_array(key.as_bytes().to_vec());
        format!("ptr @{name}, i64 {}", key.len())
    }

    pub(super) fn json_add_fields(&mut self, a: &str, b: &str, args: &[Operand]) -> Value {
        let a = self.json_key(a);
        let b = self.json_key(b);
        let operand = self.checked_instruction(format!(
            "call ptr @imre_json_add_fields(ptr {}, {a}, i64 {}, {b}, i64 {})",
            args[0].value.operand, args[1].value.operand, args[2].value.operand
        ));
        Value {
            ty: Type::JsonValue,
            operand,
            small_integer: false,
        }
    }

    pub(super) fn json_get(
        &mut self,
        key: &str,
        empty_list: bool,
        result: Type,
        args: &[Operand],
    ) -> Value {
        let key = self.json_key(key);
        let mode = self.json_mode(result, false);
        let (kind, fallback) = if empty_list {
            (1, "0".to_owned())
        } else {
            let value = &args[1].value;
            let kind = match value.ty {
                Type::None => 0,
                Type::Int => 2,
                Type::Str | Type::StrLiteral(_) => 3,
                Type::Bool => 4,
                Type::Float => 5,
                _ => unreachable!("checked JSON default"),
            };
            (kind, self.word(value))
        };
        let word = self.checked_instruction(format!(
            "call i64 @imre_json_get(ptr {}, {key}, i32 {mode}, i32 {kind}, i64 {fallback})",
            args[0].value.operand
        ));
        self.decode_word(word, result)
    }

    pub(super) fn json_select(&mut self, path: &[String], result: Type, args: &[Operand]) -> Value {
        // Each literal has its UTF-8 byte length followed by its bytes, so
        // empty names and embedded NULs need no escaping or runtime allocation.
        let mut bytes = Vec::new();
        for name in path {
            bytes.extend(
                u32::try_from(name.len())
                    .expect("a literal fits Ruff's source range")
                    .to_le_bytes(),
            );
            bytes.extend(name.as_bytes());
        }
        let size = bytes.len();
        let name = self.byte_array(bytes);
        let mode = self.json_mode(result, args.len() > 1);
        let literal = args.get(1).map_or("null", |arg| arg.value.operand.as_str());
        let word = self.checked_instruction(format!("call i64 @imre_json_select(ptr {}, ptr @{name}, i64 {size}, i32 {mode}, ptr {literal})", args[0].value.operand));
        self.decode_word(word, result)
    }
}

pub(super) fn schemas(module: &Module, output: &mut Output) {
    let mut roots = Vec::new();
    for function in module.functions.iter().chain([&module.entry]) {
        ir::walk_expressions(&function.body, &mut |value| {
            if let ExprKind::Builtin(Builtin::JsonDumps(_) | Builtin::JsonDump(_), args) =
                &value.kind
            {
                roots.push(args[5].ty);
            }
            if let ExprKind::Builtin(
                Builtin::JsonDecode(ty)
                | Builtin::TomlDecode(ty)
                | Builtin::JsonEncode(ty)
                | Builtin::JsonDumps(ty)
                | Builtin::JsonDump(ty),
                _,
            ) = value.kind
            {
                roots.push(ty);
            }
        });
    }
    if roots.is_empty() {
        return;
    }
    output.common().push_str("%json_schema = type { i32, i32, ptr, ptr, i64, ptr }\n%json_field = type { ptr, i64, ptr, i32, ptr, i64 }\n");
    let mut builder = Schemas {
        module,
        seen: HashSet::new(),
        output,
    };
    for ty in roots {
        builder.schema(ty);
    }
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
        Type::Tuple(id) => format!("json_tuple_{id}"),
        Type::Document(_) => "json_document".to_owned(),
        Type::JsonRoot => "json_root".to_owned(),
        Type::JsonObject => "json_object".to_owned(),
        Type::JsonValue => "json_value".to_owned(),
        Type::Bytes => "json_bytes".to_owned(),
        Type::Set(id) => format!("json_set_{id}"),
        Type::Path => "json_path".to_owned(),
        Type::Optional(id) => format!("json_optional_{id}"),
        _ => unreachable!("JSON schemas contain only validated concrete types"),
    }
}

struct Schemas<'a> {
    module: &'a Module,
    seen: HashSet<Type>,
    output: &'a mut Output,
}

impl Schemas<'_> {
    fn bytes(&mut self, name: &str, value: &str) {
        let output = self.output.entry_buffer();
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

    fn schema(&mut self, ty: Type) -> String {
        let ty = match ty {
            Type::StrLiteral(_) => Type::Str,
            Type::Document(_) => Type::Document(crate::ir::DocumentShape::Any),
            _ => ty,
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
            Type::JsonRoot | Type::JsonValue | Type::JsonObject => (15, None, None, 0, None),
            Type::Document(_) => (16, None, None, 0, None),
            Type::Bytes => (12, None, None, 0, None),
            Type::Set(_) => (13, None, None, 0, None),
            Type::Path => (14, None, None, 0, None),
            Type::ListInt | Type::List(_) => {
                let child = self.schema(self.module.types.element(ty).unwrap());
                (6, Some(child), None, 0, None)
            }
            Type::Optional(_) => {
                let child = self.schema(self.module.types.payload(ty).unwrap());
                (8, Some(child), None, 0, None)
            }
            Type::Dict(_) => {
                let (key, value) = self.module.types.mapping(ty).unwrap();
                let child = self.schema(value);
                (
                    if key == Type::Int { 11 } else { 9 },
                    Some(child),
                    None,
                    0,
                    None,
                )
            }
            Type::Tuple(_) => {
                let parts = self.module.types.fields(ty).unwrap();
                let fields_name = format!("{name}_fields");
                let fields: Vec<_> = parts.iter().map(|&part| {
                    let child = self.schema(part);
                    format!("%json_field {{ ptr null, i64 0, ptr @{child}, i32 0, ptr null, i64 0 }}")
                }).collect();
                let count = fields.len();
                writeln!(
                    self.output.entry_buffer(),
                    "@{fields_name} = private constant [{count} x %json_field] [{}]",
                    fields.join(", ")
                )
                .unwrap();
                (10, None, None, count, Some(fields_name))
            }
            Type::Class(class) => {
                let record = self
                    .module
                    .json_records
                    .iter()
                    .find(|schema| schema.class == class);
                let mut fields = Vec::new();
                for (index, field) in record
                    .into_iter()
                    .flat_map(|record| &record.fields)
                    .enumerate()
                {
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
                    self.output.entry_buffer(),
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
            self.output
                .global(self.output.entry, &name, "constant", "%json_schema"),
            "{{ i32 {kind}, i32 {}, ptr {}, ptr {}, i64 {count}, ptr {} }}",
            representation_kind(ty, &self.module.types),
            pointer(child),
            pointer(layout),
            pointer(fields),
        )
        .unwrap();
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
