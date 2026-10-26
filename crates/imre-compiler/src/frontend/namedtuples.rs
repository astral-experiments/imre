//! Static collections.namedtuple declarations specialize ordinary immutable tuple layouts.

use super::*;
use crate::types::{CACHE_INFO, NamedTuple, TupleOrigin};
use std::rc::Rc;

pub(super) fn collect(graph: &SourceGraph, types: &Types) -> Result<(), String> {
    for (module_id, module) in graph.modules.iter().enumerate() {
        for (index, statement) in module.parsed.syntax().body.iter().enumerate() {
            let definition = DefinitionId {
                module: module_id,
                statement: index,
            };
            if !graph.includes(definition) {
                continue;
            }
            let ast::Stmt::Assign(assign) = statement else {
                continue;
            };
            let ast::Expr::Call(call) = &*assign.value else {
                continue;
            };
            if graph.resolve(module_id, &call.func)
                != Some(Binding::Intrinsic(Intrinsic::NamedTuple))
            {
                continue;
            }
            let fail = |message| module.error(call, message);
            let [ast::Expr::StringLiteral(name), names] = call.arguments.args.as_ref() else {
                return Err(fail(
                    "namedtuple requires a literal type name and field names",
                ));
            };
            if !call.arguments.keywords.is_empty() {
                return Err(fail("namedtuple declaration options are not supported"));
            }
            let name = name.value.to_string();
            let fields: Vec<String> = if let ast::Expr::StringLiteral(value) = names {
                value
                    .value
                    .to_str()
                    .split(|c: char| c == ',' || c.is_whitespace())
                    .filter(|field| !field.is_empty())
                    .map(str::to_owned)
                    .collect()
            } else if let Some(elements) = pattern_elements(names) {
                elements
                    .iter()
                    .map(|item| match item {
                        ast::Expr::StringLiteral(field) => Ok(field.value.to_string()),
                        _ => Err(fail("namedtuple field names must be literal strings")),
                    })
                    .collect::<Result<_, _>>()?
            } else {
                return Err(fail(
                    "namedtuple field names require a literal string, list, or tuple",
                ));
            };
            let mut seen = HashSet::new();
            if !ruff_python_stdlib::identifiers::is_identifier(&name)
                || fields.iter().any(|field| {
                    !ruff_python_stdlib::identifiers::is_identifier(field)
                        || field.starts_with('_')
                        || !seen.insert(field)
                })
            {
                return Err(fail("invalid or repeated namedtuple name or field"));
            }
            types.declare_namedtuple(definition, NamedTuple { name, fields });
        }
    }
    Ok(())
}

pub(super) fn annotation(
    graph: &SourceGraph,
    module: ModuleId,
    expression: &ast::Expr,
    types: &Types,
) -> Option<DefinitionId> {
    let Binding::Definition(definition) = graph.resolve_type(module, expression)? else {
        return None;
    };
    types.namedtuple(definition).map(|_| definition)
}

impl Checker<'_> {
    /// Resolve the same declaration for a namedtuple constructor and its class attributes.
    pub(super) fn namedtuple_factory(
        &self,
        expr: &ast::Expr,
    ) -> Option<(TupleOrigin, Rc<NamedTuple>)> {
        let origin = match self.resolve(expr)? {
            Binding::Definition(id) => TupleOrigin::Declaration(id),
            Binding::Intrinsic(Intrinsic::CacheInfo) => CACHE_INFO,
            Binding::Intrinsic(Intrinsic::SplitResult { bytes: false }) => {
                crate::types::SPLIT_RESULT
            }
            Binding::Intrinsic(Intrinsic::SplitResult { bytes: true }) => {
                crate::types::SPLIT_RESULT_BYTES
            }
            _ => return None,
        };
        Some((origin, self.types.namedtuple_metadata(origin)?))
    }

    /// Read immutable attributes from a static namedtuple factory.
    pub(super) fn namedtuple_class_attribute(
        &self,
        attribute: &ast::ExprAttribute,
        declaration: &NamedTuple,
    ) -> Result<Expr, String> {
        match attribute.attr.as_str() {
            "_fields" | "__match_args__" => Ok(self.namedtuple_fields(declaration)),
            "__name__" => Ok(Expr {
                ty: Type::Str,
                kind: ExprKind::Str(declaration.name.clone()),
            }),
            _ => Err(error(
                self.source,
                attribute,
                "unsupported namedtuple class attribute",
            )),
        }
    }

    /// Evaluate an instance once before projecting a field or reading its metadata.
    pub(super) fn namedtuple_instance_attribute(
        &self,
        attribute: &ast::ExprAttribute,
        receiver: Expr,
        declaration: &NamedTuple,
    ) -> Result<Expr, String> {
        if attribute.attr.as_str() == "__match_args__"
            || (attribute.attr.as_str() == "_fields"
                && !matches!(
                    self.types.tuple_origin(receiver.ty),
                    Some(TupleOrigin::StructSequence(..) | TupleOrigin::StructTime(..))
                ))
        {
            let fields = self.namedtuple_fields(declaration);
            return Ok(Expr {
                ty: fields.ty,
                kind: ExprKind::Scope {
                    body: vec![Statement::Expr(receiver)],
                    clear: Vec::new(),
                    value: Box::new(fields),
                },
            });
        }
        let index = declaration
            .fields
            .iter()
            .position(|name| name == attribute.attr.as_str())
            .ok_or_else(|| error(self.source, attribute, "unknown namedtuple field"))?;
        Ok(Expr {
            ty: self.types.fields(receiver.ty).unwrap()[index],
            kind: ExprKind::Field(Box::new(receiver), index),
        })
    }

    fn namedtuple_fields(&self, declaration: &NamedTuple) -> Expr {
        let fields: Vec<_> = declaration
            .fields
            .iter()
            .map(|field| Expr {
                ty: Type::Str,
                kind: ExprKind::Str(field.clone()),
            })
            .collect();
        Expr {
            ty: self.types.tuple(vec![Type::Str; fields.len()]),
            kind: ExprKind::Tuple(fields),
        }
    }

    pub(super) fn namedtuple_call(
        &mut self,
        origin: TupleOrigin,
        declaration: &NamedTuple,
        call: &ast::ExprCall,
    ) -> Result<Expr, String> {
        let mut fields = Vec::with_capacity(declaration.fields.len());
        let mut scope = calls::ArgumentScope::default();
        for arg in &call.arguments.args {
            if let ast::Expr::Starred(star) = arg {
                fields.extend(scope.spread(self, star)?.into_iter().map(Some));
            } else {
                let value = self.expr(arg)?;
                fields.push(Some(scope.hold(self, value)));
            }
        }
        if fields.len() > declaration.fields.len() {
            return Err(error(self.source, call, "too many namedtuple arguments"));
        }
        fields.resize(declaration.fields.len(), None);
        for keyword in &call.arguments.keywords {
            let index = keyword
                .arg
                .as_ref()
                .and_then(|name| {
                    declaration
                        .fields
                        .iter()
                        .position(|field| field == name.as_str())
                })
                .ok_or_else(|| {
                    error(self.source, keyword, "unknown namedtuple keyword argument")
                })?;
            if fields[index].is_some() {
                return Err(error(
                    self.source,
                    keyword,
                    "multiple values for namedtuple field",
                ));
            }
            let value = self.expr(&keyword.value)?;
            fields[index] = Some(scope.hold(self, value));
        }
        let fields = fields
            .into_iter()
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| error(self.source, call, "missing namedtuple field argument"))?;
        if matches!(
            origin,
            crate::types::SPLIT_RESULT | crate::types::SPLIT_RESULT_BYTES
        ) && fields.iter().any(|value| match origin {
            crate::types::SPLIT_RESULT => !matches!(value.ty, Type::Str | Type::StrLiteral(_)),
            _ => value.ty != Type::Bytes,
        }) {
            return Err(error(
                self.source,
                call,
                "URL result fields must match their string or bytes layout",
            ));
        }
        let ty = self
            .types
            .namedtuple_layout(origin, fields.iter().map(|field| field.ty).collect());
        Ok(scope.finish(Expr {
            ty,
            kind: ExprKind::Tuple(fields),
        }))
    }
}
