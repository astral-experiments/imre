//! Language and implementation releases are independently versioned.
use crate::ir::{Expr, ExprKind, Type};
pub(crate) use imre_ast::PYTHON_VERSION as LANGUAGE;

pub(crate) const FIELDS: &[&str] = &["major", "minor", "micro", "releaselevel", "serial"];

pub(crate) fn components(implementation: bool) -> [u32; 3] {
    if implementation {
        [
            env!("CARGO_PKG_VERSION_MAJOR"),
            env!("CARGO_PKG_VERSION_MINOR"),
            env!("CARGO_PKG_VERSION_PATCH"),
        ]
        .map(|part| part.parse().unwrap())
    } else {
        [LANGUAGE.major.into(), LANGUAGE.minor.into(), 0]
    }
}

pub(crate) fn hex(components: [u32; 3]) -> u32 {
    let [major, minor, micro] = components;
    (major << 24) | (minor << 16) | (micro << 8) | 0xf0
}

pub(crate) fn tuple_items(implementation: bool) -> Vec<Expr> {
    let mut values = components(implementation).map(Expr::integer).to_vec();
    values.extend([
        Expr {
            ty: Type::Str,
            kind: ExprKind::Str("final".into()),
        },
        Expr::integer(0),
    ]);
    values
}

pub(crate) const IMPLEMENTATION: &[&str] = &[
    "name",
    "cache_tag",
    "version",
    "hexversion",
    "supports_isolated_interpreters",
];
pub(crate) const CACHE_TAG_FIELD: usize = 1;

pub(crate) fn info_type(types: &crate::types::Types) -> Type {
    types.struct_sequence(
        "sys.version_info",
        FIELDS,
        vec![Type::Int, Type::Int, Type::Int, Type::Str, Type::Int],
    )
}

pub(crate) fn implementation_fields(types: &crate::types::Types) -> Vec<Type> {
    vec![
        Type::Str,
        types.optional(Type::Str),
        info_type(types),
        Type::Int,
        Type::Bool,
    ]
}

pub(crate) fn implementation_values(types: &crate::types::Types) -> Vec<Expr> {
    vec![
        Expr {
            ty: Type::Str,
            kind: ExprKind::Str("imre".into()),
        },
        Expr {
            ty: types.optional(Type::Str),
            kind: ExprKind::Builtin(
                crate::ir::Builtin::CachePath(crate::ir::CachePathOperation::Get(false)),
                vec![],
            ),
        },
        Expr {
            ty: info_type(types),
            kind: ExprKind::Builtin(
                crate::ir::Builtin::SysVersionInfo {
                    implementation: true,
                },
                vec![],
            ),
        },
        Expr::integer(hex(components(true))),
        Expr {
            ty: Type::Bool,
            kind: ExprKind::Bool(false),
        },
    ]
}
