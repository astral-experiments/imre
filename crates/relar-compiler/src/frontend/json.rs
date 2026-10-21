//! Extracts JSON schemas only from constructors proved to perform structural writes.

use super::*;
use crate::ir::{JsonDefault, JsonField, JsonRecordSchema};

impl Checker<'_> {
    /// The schema argument names a source class; no runtime type object is created.
    pub(super) fn json_decode(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        self.reject_builtin_keywords(call)?;
        let [text, schema] = call.arguments.args.as_ref() else {
            return Err(error(
                self.source,
                call,
                "decode expects text and one record class",
            ));
        };
        let class = self.class_reference(schema).ok_or_else(|| {
            error(
                self.source,
                schema,
                "decode schema must name a fixed record class",
            )
        })?;
        Ok(Expr {
            ty: Type::Class(class),
            kind: ExprKind::Builtin(
                Builtin::JsonDecode(class),
                vec![self.expr_expected(text, Type::Str)?],
            ),
        })
    }
}

/// Unused ordinary classes remain unrestricted by this explicit decoding boundary.
pub(super) fn schemas(
    graph: &SourceGraph,
    classes: &ClassInfo,
    types: &Types,
    functions: &[ir::Function],
    entry: &ir::Function,
) -> Result<Vec<JsonRecordSchema>, String> {
    let mut pending = Vec::new();
    for function in functions.iter().chain([entry]) {
        ir::walk_expressions(&function.body, &mut |value| {
            if let ExprKind::Builtin(Builtin::JsonDecode(class), _) = value.kind {
                pending.push(class);
            }
        });
    }
    let mut visited = HashSet::new();
    let mut schemas = Vec::new();
    while let Some(class) = pending.pop() {
        if !visited.insert(class) {
            continue;
        }
        let declaration = &classes.declarations.definitions[class];
        let module = &graph.modules[declaration.definition.module];
        let initializer = &declaration.methods[declaration.initializer];
        let parameters = &classes.methods[class]["__init__"].params[1..];
        if parameters.len() != declaration.fields.len()
            || parameters.iter().any(|parameter| {
                !declaration
                    .fields
                    .iter()
                    .any(|field| field.name == parameter.name)
            })
        {
            return Err(module.error(
                initializer,
                "JSON record parameters must match every declared field exactly",
            ));
        }
        let mut values = HashMap::new();
        for statement in &initializer.body {
            match statement {
                ast::Stmt::Assign(assignment) if assignment.targets.len() == 1 => {
                    let ast::Expr::Attribute(target) = &assignment.targets[0] else {
                        return Err(module.error(
                            statement,
                            "JSON constructors require individual self.field assignments",
                        ));
                    };
                    values.insert(target.attr.as_str(), &*assignment.value);
                }
                ast::Stmt::Pass(_) => {}
                ast::Stmt::Expr(value) if matches!(&*value.value, ast::Expr::StringLiteral(_)) => {}
                _ => {
                    return Err(module.error(
                        statement,
                        "JSON constructors require one structural field assignment per statement",
                    ));
                }
            }
        }
        let mut fields = Vec::new();
        for (field, &ty) in declaration.fields.iter().zip(&classes.fields[class]) {
            check_type(ty, types, &mut pending)
                .map_err(|message| module.error(&field.annotation, message))?;
            let parameter = parameters
                .iter()
                .find(|parameter| parameter.name == field.name)
                .unwrap();
            let value = values[field.name.as_str()];
            let default = if is_name(value, &field.name) && parameter.ty == ty {
                parameter
                    .default
                    .as_ref()
                    .map(scalar_default)
                    .transpose()
                    .map_err(|message| module.error(value, message))?
            } else if types.element(ty).is_some()
                && types.payload(parameter.ty) == Some(ty)
                && parameter.default.as_ref().is_some_and(is_none_default)
                && fresh_list(value, &field.name)
            {
                Some(JsonDefault::EmptyList)
            } else {
                return Err(module.error(value, "JSON constructors may only copy matching fields or replace a missing optional list with []"));
            };
            fields.push(JsonField {
                name: field.name.clone(),
                default,
            });
        }
        schemas.push(JsonRecordSchema { class, fields });
    }
    schemas.sort_by_key(|schema| schema.class);
    Ok(schemas)
}

fn check_type(ty: Type, types: &Types, pending: &mut Vec<usize>) -> Result<(), &'static str> {
    if let Some(inner) = types.payload(ty).or_else(|| types.element(ty)) {
        return check_type(inner, types, pending);
    }
    match ty {
        Type::Int | Type::Float | Type::Bool | Type::Str | Type::None => Ok(()),
        Type::Class(class) => {
            pending.push(class);
            Ok(())
        }
        _ => Err(
            "JSON fields require int, float, bool, str, None, optional, list, or structural record types",
        ),
    }
}

fn is_name(value: &ast::Expr, expected: &str) -> bool {
    matches!(value, ast::Expr::Name(name) if name.id == expected)
}

fn fresh_list(value: &ast::Expr, name: &str) -> bool {
    let ast::Expr::If(value) = value else {
        return false;
    };
    let ast::Expr::Compare(test) = &*value.test else {
        return false;
    };
    let [left, right] = test.operands.as_ref() else {
        return false;
    };
    test.ops.as_ref() == [ast::CmpOp::Is]
        && is_name(left, name)
        && matches!(right, ast::Expr::NoneLiteral(_))
        && matches!(&*value.body, ast::Expr::List(list) if list.elts.is_empty())
        && is_name(&value.orelse, name)
}

fn is_none_default(value: &Expr) -> bool {
    match &value.kind {
        ExprKind::None => true,
        ExprKind::Absent(value) => is_none_default(value),
        _ => false,
    }
}

fn scalar_default(value: &Expr) -> Result<JsonDefault, &'static str> {
    Ok(match &value.kind {
        ExprKind::Int(value) => JsonDefault::Int(value.clone()),
        ExprKind::Float(value) if value.is_finite() => JsonDefault::Float(*value),
        ExprKind::Bool(value) => JsonDefault::Bool(*value),
        ExprKind::Str(value) => JsonDefault::Str(value.clone()),
        ExprKind::None => JsonDefault::None,
        ExprKind::List(values) if values.is_empty() => JsonDefault::EmptyList,
        ExprKind::Present(value) | ExprKind::Absent(value) => return scalar_default(value),
        ExprKind::Negate(value) => match &value.kind {
            ExprKind::Int(value) => JsonDefault::Int(format!("-{value}")),
            ExprKind::Float(value) if value.is_finite() => JsonDefault::Float(-value),
            _ => return Err("JSON defaults require finite scalar literals"),
        },
        _ => return Err("JSON defaults require finite scalar literals"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lower(source: &str) -> Result<ir::Module, String> {
        super::super::lower(&SourceGraph::single(source)?)
    }

    #[test]
    fn json_schemas_keep_required_fields_and_fresh_defaults_distinct() {
        let module = lower(
            r#"
import relar.json
class Item:
    name: str
    amount: int
    note: str | None
    def __init__(self, name: str, amount: int = -7, note: str | None = None) -> None:
        self.note = note
        self.amount = amount
        self.name = name
class Report:
    items: list[Item]
    def __init__(self, items: list[Item] | None = None) -> None:
        self.items = [] if items is None else items
report = relar.json.decode("{}", Report)
"#,
        )
        .unwrap();
        assert_eq!(module.json_records.len(), 2);
        let item = &module.json_records[0];
        assert_eq!(item.class, 0);
        assert_eq!(
            item.fields
                .iter()
                .map(|field| field.name.as_str())
                .collect::<Vec<_>>(),
            ["name", "amount", "note"]
        );
        assert!(item.fields[0].default.is_none());
        assert!(matches!(&item.fields[1].default, Some(JsonDefault::Int(value)) if value == "-7"));
        assert!(matches!(item.fields[2].default, Some(JsonDefault::None)));
        assert!(matches!(
            module.json_records[1].fields[0].default,
            Some(JsonDefault::EmptyList)
        ));
    }

    #[test]
    fn json_rejects_computed_constructor_writes_without_restricting_other_classes() {
        for assignment in ["value + 1", "int(value)", "abs(value)"] {
            let class = format!(
                "class Item:\n    value: int\n    def __init__(self, value: int) -> None:\n        self.value = {assignment}\n"
            );
            assert!(lower(&format!("{class}print(Item(1).value)\n")).is_ok());
            let error = lower(&format!(
                "from relar.json import decode\n{class}item = decode(\"{{}}\", Item)\n"
            ))
            .err()
            .unwrap();
            assert!(
                error.contains("JSON constructors may only copy matching fields"),
                "{error}"
            );
        }
        let error = lower(
            r#"
from relar.json import decode
class Item:
    value: float
    def __init__(self, value: float = 1e999) -> None:
        self.value = value
item = decode("{}", Item)
"#,
        )
        .err()
        .unwrap();
        assert!(error.contains("finite scalar literals"), "{error}");
    }
}
