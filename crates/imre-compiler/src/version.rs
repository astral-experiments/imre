//! Language and implementation releases are independently versioned.
use crate::ir::{Expr, ExprKind, Type};
use ruff_python_ast::PythonVersion;

pub(crate) const LANGUAGE: PythonVersion = PythonVersion::PY314;
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

pub(crate) fn integer(value: u32) -> Expr {
    Expr {
        ty: Type::Int,
        kind: ExprKind::Int(value.to_string()),
    }
}

pub(crate) fn hex(components: [u32; 3]) -> u32 {
    let [major, minor, micro] = components;
    (major << 24) | (minor << 16) | (micro << 8) | 0xf0
}

pub(crate) fn tuple_items(implementation: bool) -> Vec<Expr> {
    let mut values = components(implementation).map(integer).to_vec();
    values.extend([
        Expr {
            ty: Type::Str,
            kind: ExprKind::Str("final".into()),
        },
        integer(0),
    ]);
    values
}
