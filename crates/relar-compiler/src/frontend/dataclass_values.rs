//! Validates generated value behavior without changing the fixed storage model.

use super::*;

/// Container comparisons in CPython preserve the identity of boxed NaNs. Bare
/// native floats intentionally have no such identity, so generated equality may
/// compare them directly but cannot silently introduce value-only container
/// equality. A record boundary preserves identity and is validated independently.
fn has_container_float(ty: Type, inside: bool, types: &Types) -> bool {
    match ty {
        Type::Float => inside,
        Type::Optional(_) => has_container_float(types.payload(ty).unwrap(), inside, types),
        Type::ListInt | Type::List(_) | Type::Dict(_) | Type::Set(_) | Type::Tuple(_) => types
            .children(ty)
            .into_iter()
            .any(|child| has_container_float(child, true, types)),
        _ => false,
    }
}

/// Resource representations expose interpreter state absent from the native
/// subset. Excluding a field from generated repr keeps those records usable.
pub(super) fn repr_supported(ty: Type, types: &Types) -> bool {
    match ty {
        Type::Exception
        | Type::JsonValue
        | Type::JsonObject
        | Type::StringKeys
        | Type::TimeoutExpired
        | Type::RegexPattern
        | Type::RegexMatch
        | Type::TextIO
        | Type::BinaryIO
        | Type::Iterator
        | Type::Callable(_) => false,
        Type::Class(_) => true,
        _ => types
            .children(ty)
            .into_iter()
            .all(|child| repr_supported(child, types)),
    }
}

/// Rejects only generated operations whose supported field representations cannot
/// retain Python semantics; ignored fields and disabled operations remain valid.
pub(super) fn validate(
    classes: &ClassInfo,
    graph: &SourceGraph,
    types: &Types,
) -> Result<(), String> {
    for (class, declaration) in classes.declarations.definitions.iter().enumerate() {
        let Some(options) = &declaration.dataclass else {
            continue;
        };
        let module = &graph.modules[declaration.definition.module];
        for (field, &ty) in declaration.fields.iter().zip(&classes.fields[class]) {
            if options.eq && field.compare && has_container_float(ty, false, types) {
                return Err(module.error(
                    &field.annotation,
                    "generated dataclass equality cannot compare float-containing containers; use field(compare=False) or dataclass(eq=False)",
                ));
            }
            if options.eq && field.compare && contains_opaque(ty, types) {
                return Err(module.error(
                    &field.annotation,
                    "generated dataclass equality cannot compare regex, readonly JSON objects, or TOML key views; use field(compare=False) or dataclass(eq=False)",
                ));
            }
            if options.repr && field.repr && !repr_supported(ty, types) {
                return Err(module.error(
                    &field.annotation,
                    "generated dataclass repr does not support resource or callable fields; use field(repr=False) or dataclass(repr=False)",
                ));
            }
        }
    }
    Ok(())
}

fn contains_opaque(ty: Type, types: &Types) -> bool {
    matches!(
        ty,
        Type::RegexPattern
            | Type::RegexMatch
            | Type::JsonValue
            | Type::JsonObject
            | Type::StringKeys
    ) || types
        .children(ty)
        .into_iter()
        .any(|child| contains_opaque(child, types))
}

/// Names the entry module like CPython while keeping imported class names
/// qualified; the same metadata drives generated equality and representations.
pub(super) fn metadata(classes: &ClassInfo, graph: &SourceGraph) -> Vec<ir::ClassMetadata> {
    classes
        .declarations
        .definitions
        .iter()
        .enumerate()
        .map(|(id, class)| ir::ClassMetadata {
            name: class.name.clone(),
            enum_members: class
                .enum_members
                .as_ref()
                .map(|members| members.values.clone())
                .unwrap_or_default(),
            qualified_name: format!(
                "{}.{}",
                if class.definition.module == graph.entry {
                    "__main__"
                } else {
                    &graph.modules[class.definition.module].name
                },
                class.name
            ),
            dataclass_eq: class.dataclass.as_ref().is_some_and(|options| options.eq),
            dataclass_repr: class.dataclass.as_ref().is_some_and(|options| options.repr),
            initializer_fields: (class.generated_initializer
                && !classes.methods[id].contains_key("__post_init__"))
            .then(|| {
                let fields = class.fields.iter().enumerate();
                fields
                    .clone()
                    .filter(|(_, f)| !f.kw_only)
                    .chain(fields.filter(|(_, f)| f.kw_only))
                    .map(|(index, _)| index)
                    .collect()
            }),
            fields: class
                .fields
                .iter()
                .map(|field| ir::ClassFieldMetadata {
                    name: field.name.clone(),
                    compare: field.compare,
                    repr: field.repr,
                })
                .collect(),
        })
        .collect()
}
