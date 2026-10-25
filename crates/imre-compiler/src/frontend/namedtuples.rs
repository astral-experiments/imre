//! Static collections.namedtuple declarations specialize ordinary immutable tuple layouts.

use super::*;
use crate::types::NamedTuple;

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
    pub(super) fn namedtuple_fields(&self, declaration: &NamedTuple) -> Expr {
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
        definition: DefinitionId,
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
        let ty = self
            .types
            .namedtuple_layout(definition, fields.iter().map(|field| field.ty).collect());
        Ok(scope.finish(Expr {
            ty,
            kind: ExprKind::Tuple(fields),
        }))
    }
}
