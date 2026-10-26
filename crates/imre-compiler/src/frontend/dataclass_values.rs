//! Validates generated value behavior without changing the fixed storage model.

use super::*;

impl Checker<'_> {
    /// An erased record keeps its concrete descriptor and owner. Admit only
    /// closed classes with generated value operations and supported slot types.
    /// Packed value structs cannot be given observable identity through erasure.
    pub(super) fn boxed_dataclass(&self, class: usize) -> bool {
        let all = &self.classes.declarations.definitions;
        let decl = &all[class];
        let special_name = |name: &str| name.starts_with("__") && name.ends_with("__");
        decl.generated_initializer
            && decl
                .dataclass
                .is_some_and(|d| !d.value_struct && d.slots && d.eq && d.repr)
            && !decl.fields.iter().any(|f| special_name(&f.name))
            && !decl.static_bindings.keys().any(|name| special_name(name))
            && decl.bases.is_empty()
            && !all.iter().any(|child| child.bases.contains(&class))
            && !decl.node.body.iter().any(|stmt| {
                matches!(stmt,
                ast::Stmt::FunctionDef(f) if special_name(&f.name))
            })
            && self.classes.fields[class]
                .iter()
                .all(|ty| self.boxable_value(*ty))
    }

    pub(super) fn object_value_supported(&self, ty: Type) -> bool {
        if self.boxable_value(ty) {
            return true;
        }
        match ty {
            Type::Float
            | Type::ListInt
            | Type::Path
            | Type::PurePath(_)
            | Type::Enum(_)
            | Type::StrEnum(_) => true,
            Type::List(_)
            | Type::Set(_)
            | Type::Dict(_)
            | Type::Tuple(_)
            | Type::Optional(_)
            | Type::Union(_) => self
                .types
                .children(ty)
                .into_iter()
                .all(|child| self.object_value_supported(child)),
            Type::Class(class) => {
                let decl = &self.classes.declarations.definitions[class];
                // A plain instance without protocol overrides keeps its concrete
                // descriptor and identity when stored in a Python dictionary.
                self.boxed_dataclass(class) || (decl.dataclass.is_none()
                    && decl.bases.is_empty()
                    && !self.classes.has_concrete_subclasses(class)
                    && decl.node.body.iter().all(|stmt| !matches!(stmt,
                        ast::Stmt::FunctionDef(f) if f.name.starts_with("__") && f.name.ends_with("__"))))
            }
            _ => false,
        }
    }
}

/// Container comparisons in CPython preserve the identity of boxed NaNs. Bare
/// native floats intentionally have no such identity, so generated equality may
/// compare them directly but cannot silently introduce value-only container
/// equality. A record boundary preserves identity and is validated independently.
fn has_container_float(ty: Type, inside: bool, types: &Types) -> bool {
    match ty {
        Type::Float => inside,
        Type::Optional(_) => has_container_float(types.payload(ty).unwrap(), inside, types),
        Type::ListInt
        | Type::List(_)
        | Type::Dict(_)
        | Type::Set(_)
        | Type::Tuple(_)
        | Type::VarTuple(_) => types
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
        Type::Context | Type::ContextMissing | Type::ContextVar(_) | Type::ContextToken(_) => true,
        Type::ModuleSpec(id) => {
            // A loader's custom __repr__ must run rather than being replaced by its storage.
            fn loader(ty: Type, types: &Types) -> bool {
                match ty {
                    Type::None
                    | Type::Bool
                    | Type::Int
                    | Type::Float
                    | Type::Str
                    | Type::StrLiteral(_)
                    | Type::Bytes
                    | Type::ListInt => true,
                    Type::List(_)
                    | Type::Tuple(_)
                    | Type::Set(_)
                    | Type::Dict(_)
                    | Type::Optional(_) => {
                        types.children(ty).into_iter().all(|ty| loader(ty, types))
                    }
                    _ => false,
                }
            }
            loader(types.module_spec_fields(id)[1], types)
        }
        Type::SysImplementation => true,
        Type::Exception
        | Type::CmpKey(..)
        | Type::NamedException(_)
        | Type::StringKeys
        | Type::ImportException
        | Type::TimeoutExpired
        | Type::EventLoop
        | Type::Coroutine(_)
        | Type::Task(_)
        | Type::TaskGroup
        | Type::GeneratorContext
        | Type::DirectoryContext(_)
        | Type::Semaphore
        | Type::Popen(..)
        | Type::ActivePopen
        | Type::AsyncProcess(_)
        | Type::NativeModule
        | Type::ModuleNamespace
        | Type::LogFormatter
        | Type::LogRecord
        | Type::Logger
        | Type::TimedFileHandler
        | Type::ProcessSelectorClass
        | Type::ClockTime
        | Type::CodecInfo
        | Type::Poll
        | Type::Socket
        | Type::CodecWriterClass
        | Type::CodecWriter
        | Type::TextOutput
        | Type::Sha256
        | Type::JsonDecoder
        | Type::CsvReader
        | Type::RegexPattern
        | Type::RegexMatch
        | Type::ObjectValue
        | Type::TextIO
        | Type::StringIO
        | Type::HandlerLock
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
/// An unused generated repr does not constrain a closed entry. Check its full
/// field tree on each operation that can actually format the value.
pub(super) fn can_format(ty: Type, types: &Types, classes: &ClassInfo) -> bool {
    check_format(ty, types, classes, &mut HashSet::new(), false)
}

/// The dispatch branch already proved the exact outer record layout.
pub(super) fn can_format_concrete(ty: Type, types: &Types, classes: &ClassInfo) -> bool {
    check_format(ty, types, classes, &mut HashSet::new(), true)
}

fn check_format(
    ty: Type,
    types: &Types,
    classes: &ClassInfo,
    seen: &mut HashSet<usize>,
    exact: bool,
) -> bool {
    if let Type::Class(id) = ty {
        if !exact && classes.has_concrete_subclasses(id) {
            return classes
                .concrete_layouts(id)
                .into_iter()
                .all(|class| check_format(Type::Class(class), types, classes, seen, true));
        }
        if classes.has_repr_override(id) {
            // The descriptor walk borrows contents and cannot run a mutating callback.
            return false;
        }
        if !seen.insert(id) {
            return true;
        }
        let class = &classes.declarations.definitions[id];
        return class.dataclass.is_none_or(|d| !d.repr)
            || class
                .fields
                .iter()
                .zip(&classes.fields[id])
                .all(|(field, &ty)| !field.repr || check_format(ty, types, classes, seen, false));
    }
    // Context variables can hold the supported boxed scalar or tuple defaults.
    if types.context_value(ty) == Some(Type::ObjectValue) {
        return true;
    }
    if matches!(ty, Type::ModuleSpec(_)) {
        return repr_supported(ty, types);
    }
    repr_supported(ty, types)
        && types
            .children(ty)
            .into_iter()
            .all(|ty| check_format(ty, types, classes, seen, false))
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
        for (field, &ty) in declaration.fields.iter().zip(&classes.fields[class]) {
            let module = &graph.modules[field.module];
            if options.eq && field.compare && types.contains_module_spec(ty) {
                return Err(module.error(
                    &field.annotation,
                    "generated equality for ModuleSpec requires dynamic attribute protocol support",
                ));
            }
            if options.eq && field.compare && has_container_float(ty, false, types) {
                return Err(module.error(
                    &field.annotation,
                    "generated dataclass equality cannot compare float-containing containers; use field(compare=False) or dataclass(eq=False)",
                ));
            }
            if options.eq && field.compare && contains_opaque(ty, types) {
                return Err(module.error(
                    &field.annotation,
                    "generated dataclass equality cannot compare opaque standard-library objects; use field(compare=False) or dataclass(eq=False)",
                ));
            }
            if (!graph.reachable_only()
                || classes
                    .declarations
                    .exceptions
                    .contains_key(&declaration.definition))
                && options.repr
                && field.repr
                && !repr_supported(ty, types)
            {
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
        Type::Logger
            | Type::TimedFileHandler
            | Type::ProcessSelectorClass
            | Type::ClockTime
            | Type::LogFormatter
            | Type::LogRecord
            | Type::CodecInfo
            | Type::Poll
            | Type::Socket
            | Type::CodecWriterClass
            | Type::CodecWriter
            | Type::TextOutput
            | Type::Sha256
            | Type::JsonDecoder
            | Type::RegexPattern
            | Type::RegexMatch
            | Type::StringKeys
            | Type::CsvReader
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
            plain_enum: class
                .enum_members
                .as_ref()
                .is_some_and(|members| members.plain),
            enum_members: class
                .enum_members
                .as_ref()
                .map(|members| members.values.clone())
                .unwrap_or_default(),
            http_members: class
                .enum_members
                .as_ref()
                .map(|m| m.http_members.clone())
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
            frozen_initializer: (class.generated_initializer
                && class.dataclass.is_some_and(|options| options.frozen)
                && !classes.methods[id].contains_key("__post_init__")
                && !classes.has_concrete_subclasses(id))
            .then(|| classes.methods[id]["__init__"].id),
            value_struct: class.dataclass.is_some_and(|options| options.value_struct),
            dataclass_eq: class.dataclass.as_ref().is_some_and(|options| options.eq),
            dataclass_repr: class.dataclass.as_ref().is_some_and(|options| options.repr),
            toml_hash: class.toml_hash,
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
                    repr_text: graph.modules[class.definition.module].bundled
                        && graph.modules[class.definition.module].name == "unittest.mock"
                        && class.name == "_Call"
                        && field.name == "_text",
                })
                .collect(),
        })
        .collect()
}
