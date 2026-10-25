//! Typed operations over a TOML tree. Only document boundaries use a tagged
//! value; its ordered dicts and lists retain their ordinary native storage.

use super::*;
use crate::ir::{DocumentOp as Op, DocumentShape as Shape};

pub(super) const ANY: Type = Type::Document(Shape::Any);
pub(super) const SCALAR: Type = Type::Document(Shape::Scalar);
pub(super) const MAP: Type = Type::Document(Shape::Map);
pub(super) const LIST: Type = Type::Document(Shape::List);

/// A string-keyed display with different literal value types needs a tag on each
/// value. Homogeneous displays keep their ordinary unboxed storage.
pub(super) fn mixed_scalar_fields(dict: &ast::ExprDict) -> bool {
    let mut first = None;
    let mut mixed = false;
    for item in &dict.items {
        if !matches!(item.key, Some(ast::Expr::StringLiteral(_))) {
            return false;
        }
        let kind = match &item.value {
            ast::Expr::BooleanLiteral(_) => Type::Bool,
            ast::Expr::NoneLiteral(_) => Type::None,
            ast::Expr::StringLiteral(_) => Type::Str,
            ast::Expr::NumberLiteral(n) if matches!(n.value, ast::Number::Int(_)) => Type::Int,
            ast::Expr::UnaryOp(n)
                if matches!(n.op, ast::UnaryOp::UAdd | ast::UnaryOp::USub)
                    && matches!(&*n.operand, ast::Expr::NumberLiteral(n) if matches!(n.value, ast::Number::Int(_))) =>
            {
                Type::Int
            }
            _ => return false,
        };
        mixed |= *first.get_or_insert(kind) != kind;
    }
    mixed
}

/// A read-only object can own these fresh literals without severing an external alias.
pub(super) fn constant(expr: &ast::Expr) -> bool {
    match expr {
        ast::Expr::StringLiteral(_)
        | ast::Expr::BooleanLiteral(_)
        | ast::Expr::NumberLiteral(_)
        | ast::Expr::NoneLiteral(_) => true,
        ast::Expr::List(list) => list.elts.iter().all(constant),
        ast::Expr::Dict(dict) => dict.items.iter().all(|entry| {
            matches!(entry.key, Some(ast::Expr::StringLiteral(_))) && constant(&entry.value)
        }),
        ast::Expr::UnaryOp(u) => {
            matches!(u.op, ast::UnaryOp::USub | ast::UnaryOp::UAdd)
                && matches!(&*u.operand, ast::Expr::NumberLiteral(_))
        }
        _ => false,
    }
}

pub(super) fn op(op: Op, ty: Type, args: Vec<Expr>) -> Expr {
    Expr {
        ty,
        kind: ExprKind::Builtin(Builtin::Document(op), args),
    }
}

impl Checker<'_> {
    /// A spread's declared type decides the representation of its fresh outer table.
    /// Inspect only field/local bindings here; evaluation happens once inside the display.
    pub(super) fn probe_document(&self, expr: &ast::Expr) -> bool {
        match expr {
            ast::Expr::Name(n) => self
                .names
                .get(n.id.as_str())
                .is_some_and(|&slot| matches!(self.locals[slot], Type::Document(_))),
            ast::Expr::Attribute(a) => {
                let ast::Expr::Name(n) = &*a.value else {
                    return false;
                };
                let Some(&slot) = self.names.get(n.id.as_str()) else {
                    return false;
                };
                let Type::Class(class) = self.locals[slot] else {
                    return false;
                };
                self.classes.declarations.definitions[class]
                    .fields
                    .iter()
                    .position(|field| field.name == a.attr.as_str())
                    .is_some_and(|index| {
                        matches!(self.classes.fields[class][index], Type::Document(_))
                    })
            }
            ast::Expr::If(branch) => {
                self.probe_document(&branch.body) || self.probe_document(&branch.orelse)
            }
            _ => false,
        }
    }

    pub(super) fn document_coerce(
        &self,
        value: Expr,
        expected: Type,
        site: &impl Ranged,
    ) -> Result<Expr, String> {
        if value.ty == expected {
            return Ok(value);
        }
        let actual = value.ty;
        if expected == SCALAR {
            if !matches!(
                actual,
                Type::None | Type::Bool | Type::Int | Type::Str | Type::StrLiteral(_)
            ) {
                return Err(error(
                    self.source,
                    site,
                    "this dictionary holds None, bool, int or str values",
                ));
            }
            let tagged = self.document_coerce(value, ANY, site)?;
            return Ok(Expr {
                ty: SCALAR,
                kind: ExprKind::Builtin(Builtin::RecordView, vec![tagged]),
            });
        }
        match (actual, expected) {
            (Type::Document(_), ANY) => Ok(Expr {
                ty: ANY,
                kind: ExprKind::Builtin(Builtin::RecordView, vec![value]),
            }),
            (Type::Document(_), MAP) => Ok(op(Op::RequireMap, MAP, vec![value])),
            (Type::Document(_), LIST) => Ok(op(Op::RequireList, LIST, vec![value])),
            (Type::Document(_), Type::Str) => Ok(op(Op::AsStr, Type::Str, vec![value])),
            (_, Type::Document(shape)) => {
                let (kind, ty) = match actual {
                    Type::None => (Op::Null, ANY),
                    Type::Bool => (Op::FromBool, ANY),
                    Type::Int => (Op::FromInt, ANY),
                    Type::Float => (Op::FromFloat, ANY),
                    Type::Str | Type::StrLiteral(_) => (Op::FromStr, ANY),
                    Type::JsonRoot | Type::JsonValue | Type::JsonObject => (Op::FromJson, ANY),
                    t if self.types.mapping(t) == Some((Type::Str, Type::None)) => {
                        (Op::BoxNullMap, MAP)
                    }
                    t if self.types.element(t) == Some(ANY) => (Op::WrapList, LIST),
                    t if self.types.mapping(t) == Some((Type::Str, ANY)) => (Op::WrapMap, MAP),
                    _ => return Err(error(self.source, site, "unsupported document value")),
                };
                let value = op(kind, ty, vec![value]);
                if shape == Shape::Any || value.ty == expected {
                    Ok(Expr {
                        ty: expected,
                        ..value
                    })
                } else {
                    self.document_coerce(value, expected, site)
                }
            }
            _ => Err(error(self.source, site, "unsupported document conversion")),
        }
    }

    pub(super) fn document_literal(
        &mut self,
        expr: &ast::Expr,
        expected: Type,
    ) -> Result<Option<Expr>, String> {
        let value = match expr {
            ast::Expr::Dict(dict) => {
                let slot = self.hidden(MAP);
                let mut body = vec![Statement::Assign(slot, op(Op::EmptyMap, MAP, vec![]))];
                for item in &dict.items {
                    if let Some(key) = &item.key {
                        let key = self.expr_expected(key, Type::Str)?;
                        let value = self.expr_expected(&item.value, ANY)?;
                        body.push(Statement::SetItem(
                            self.document_map(self.local(slot)),
                            key,
                            value,
                        ));
                    } else {
                        let value = self.expr_expected(&item.value, MAP)?;
                        body.push(Statement::Expr(op(
                            Op::UpdateMap,
                            MAP,
                            vec![self.local(slot), value],
                        )));
                    }
                }
                Expr {
                    ty: MAP,
                    kind: ExprKind::Scope {
                        body,
                        value: Box::new(self.local(slot)),
                        clear: vec![slot],
                    },
                }
            }
            ast::Expr::List(list) if list.elts.is_empty() => op(Op::EmptyList, LIST, vec![]),
            ast::Expr::List(list) => {
                let values = list
                    .elts
                    .iter()
                    .map(|v| self.expr_expected(v, ANY))
                    .collect::<Result<Vec<_>, _>>()?;
                op(
                    Op::WrapList,
                    LIST,
                    vec![Expr {
                        ty: self.types.list(ANY),
                        kind: ExprKind::List(values),
                    }],
                )
            }
            _ => return Ok(None),
        };
        self.document_coerce(value, expected, expr).map(Some)
    }

    pub(super) fn document_map(&self, value: Expr) -> Expr {
        op(Op::MapView, self.types.dict(Type::Str, ANY), vec![value])
    }
    pub(super) fn document_list(&self, value: Expr) -> Expr {
        op(Op::ListView, self.types.list(ANY), vec![value])
    }

    pub(super) fn document_iteration(&self, value: Expr) -> Expr {
        match value.ty {
            MAP => self.document_map(value),
            LIST => self.document_list(value),
            _ => value,
        }
    }

    pub(super) fn document_method(
        &mut self,
        call: &ast::ExprCall,
        attribute: &ast::ExprAttribute,
        receiver: Expr,
    ) -> Result<Expr, String> {
        self.reject_builtin_keywords(call)?;
        match (attribute.attr.as_str(), call.arguments.args.as_ref()) {
            ("get" | "pop", [key] | [key, _]) => {
                let key = self.expr_expected(key, Type::Str)?;
                let default = match call.arguments.args.get(1) {
                    Some(value) => self.expr_expected(value, ANY)?,
                    None if attribute.attr.as_str() == "get" => op(Op::Null, ANY, vec![]),
                    None => {
                        return Err(error(
                            self.source,
                            call,
                            "document pop requires an explicit default",
                        ));
                    }
                };
                if attribute.attr.as_str() == "pop" {
                    return Ok(op(Op::PopDefault, ANY, vec![receiver, key, default]));
                }
                Ok(Expr {
                    ty: ANY,
                    kind: ExprKind::Builtin(
                        Builtin::DictGetDefault,
                        vec![self.document_map(receiver), key, default],
                    ),
                })
            }
            _ => Err(error(self.source, call, "unsupported document method")),
        }
    }

    pub(super) fn document_call(
        &mut self,
        kind: Intrinsic,
        call: &ast::ExprCall,
    ) -> Result<Expr, String> {
        self.reject_builtin_keywords(call)?;
        match (kind, call.arguments.args.as_slice()) {
            (Intrinsic::TomliLoads, [text]) => Ok(op(
                Op::ParseToml,
                MAP,
                vec![self.expr_expected(text, Type::Str)?],
            )),
            (Intrinsic::TomliLoad, [file]) => {
                let file = self.expr_expected(file, Type::BinaryIO)?;
                let bytes = Expr {
                    ty: Type::Bytes,
                    kind: ExprKind::Builtin(Builtin::BinaryRead, vec![file]),
                };
                let text = Expr {
                    ty: Type::Str,
                    kind: ExprKind::Builtin(Builtin::Decode, vec![bytes]),
                };
                Ok(op(Op::ParseToml, MAP, vec![text]))
            }
            (Intrinsic::TomlDumps, [source]) => Ok(op(
                Op::DumpToml,
                Type::Str,
                vec![self.expr_expected(source, MAP)?],
            )),
            (Intrinsic::TomliDump, [source, file]) => Ok(op(
                Op::WriteToml,
                Type::None,
                vec![
                    self.expr_expected(source, MAP)?,
                    self.expr_expected(file, Type::BinaryIO)?,
                ],
            )),
            _ => Err(error(
                self.source,
                call,
                "TOML I/O requires a document and a binary file or string",
            )),
        }
    }
}
