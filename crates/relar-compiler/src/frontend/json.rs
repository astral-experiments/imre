//! Checks concrete JSON/TOML shapes and proves constructors used during decoding.

use super::*;
use crate::ir::{JsonDefault, JsonField, JsonRecordSchema};

/// Folds successive literal subscripts into one traversal. The keys have no
/// evaluation effects; the first failing lookup still determines the exception.
pub(super) fn select(value: Expr, ty: Type, key: Option<String>, literal: Option<Expr>) -> Expr {
    if key.is_none()
        && literal.is_none()
        && matches!(value.kind, ExprKind::Builtin(Builtin::JsonGet { .. }, _))
    {
        return Expr { ty, ..value };
    }
    let (mut path, mut args) = match value {
        Expr {
            ty: Type::JsonValue,
            kind: ExprKind::Builtin(Builtin::JsonSelect(path), args),
        } => (path, args),
        value => (Vec::new(), vec![value]),
    };
    path.extend(key);
    args.extend(literal);
    Expr {
        ty,
        kind: ExprKind::Builtin(Builtin::JsonSelect(path), args),
    }
}

/// Literal defaults and reads of the same rooted local have no Python side
/// effects. The runtime can add the selected values without boxing either one.
pub(super) fn add(left: Expr, right: Expr) -> Expr {
    fn field(expr: &Expr) -> Option<(&String, &Expr, &Expr, usize)> {
        let ExprKind::Builtin(
            Builtin::JsonGet {
                key,
                empty_list: false,
            },
            args,
        ) = &expr.kind
        else {
            return None;
        };
        let [receiver, default] = args.as_slice() else {
            return None;
        };
        let ExprKind::Local(slot) = receiver.kind else {
            return None;
        };
        matches!(default.kind, ExprKind::Int(_)).then_some((key, receiver, default, slot))
    }
    let kind = if let (Some((a, receiver, left_default, x)), Some((b, _, right_default, y))) =
        (field(&left), field(&right))
        && x == y
    {
        ExprKind::Builtin(
            Builtin::JsonAddFields(a.clone(), b.clone()),
            vec![
                receiver.clone(),
                left_default.clone(),
                right_default.clone(),
            ],
        )
    } else {
        ExprKind::Builtin(Builtin::JsonAdd, vec![left, right])
    };
    Expr {
        ty: Type::JsonValue,
        kind,
    }
}

pub(super) fn compare(
    values: Vec<Expr>,
    ops: &[ast::CmpOp],
    source: &str,
    node: &ast::Expr,
) -> Result<Expr, String> {
    let invalid = || {
        error(
            source,
            node,
            "readonly JSON comparisons require == or != with a literal string",
        )
    };
    let [left, right]: [Expr; 2] = values.try_into().map_err(|_| invalid())?;
    let [op @ (ast::CmpOp::Eq | ast::CmpOp::NotEq)] = ops else {
        return Err(invalid());
    };
    let (value, literal) = if matches!(left.kind, ExprKind::Str(_)) {
        (right, left)
    } else if matches!(right.kind, ExprKind::Str(_)) {
        (left, right)
    } else {
        return Err(invalid());
    };
    let result = select(value, Type::Bool, None, Some(literal));
    Ok(if *op == ast::CmpOp::Eq {
        result
    } else {
        Expr {
            ty: Type::Bool,
            kind: ExprKind::Not(Box::new(result)),
        }
    })
}

impl Checker<'_> {
    /// The same document array may be projected at multiple typed boundaries.
    /// Its source type remains read-only across aliases and function calls.
    pub(super) fn check_mutable_json_rows(
        &self,
        ty: Type,
        node: &impl Ranged,
    ) -> Result<(), String> {
        if self.types.element(ty) == Some(Type::JsonObject) {
            return Err(error(
                self.source,
                node,
                "lists of readonly JSON object views cannot be mutated",
            ));
        }
        Ok(())
    }

    pub(super) fn json_load(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        self.reject_builtin_keywords(call)?;
        let [stream] = call.arguments.args.as_ref() else {
            return Err(error(
                self.source,
                call,
                "json.load requires one open text stream",
            ));
        };
        let stream = self.expr_expected(stream, Type::TextIO)?;
        let text = Expr {
            ty: Type::Str,
            kind: ExprKind::Builtin(Builtin::TextRead, vec![stream]),
        };
        Ok(Expr {
            ty: Type::JsonValue,
            kind: ExprKind::Builtin(Builtin::JsonLoad, vec![text]),
        })
    }

    pub(super) fn json_get(
        &mut self,
        receiver: Expr,
        call: &ast::ExprCall,
    ) -> Result<Expr, String> {
        self.reject_builtin_keywords(call)?;
        let [ast::Expr::StringLiteral(key), default] = call.arguments.args.as_ref() else {
            return Err(error(
                self.source,
                call,
                "readonly JSON get requires a literal string key and an integer or empty-list default",
            ));
        };
        check_string(key, self.source)?;
        let empty_list = matches!(default, ast::Expr::List(list) if list.elts.is_empty());
        let args = if empty_list {
            vec![receiver]
        } else {
            let default = self.expr_expected(default, Type::Int)?;
            // Python binds the method before evaluating the default. Literal and
            // local reads cannot mask a missing attribute on a malformed row.
            let receiver = if matches!(default.kind, ExprKind::Int(_) | ExprKind::Local(_)) {
                receiver
            } else {
                Expr {
                    ty: receiver.ty,
                    kind: ExprKind::Builtin(Builtin::JsonGetReceiver, vec![receiver]),
                }
            };
            vec![receiver, default]
        };
        Ok(Expr {
            ty: Type::JsonValue,
            kind: ExprKind::Builtin(
                Builtin::JsonGet {
                    key: key.value.to_string(),
                    empty_list,
                },
                args,
            ),
        })
    }

    pub(super) fn json_loads(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        self.reject_builtin_keywords(call)?;
        let [text] = call.arguments.args.as_ref() else {
            return Err(error(
                self.source,
                call,
                "json.loads requires one UTF-8 string containing an array",
            ));
        };
        let text = self.expr_expected(text, Type::Str)?;
        Ok(Expr {
            ty: self.types.list(Type::JsonValue),
            kind: ExprKind::Builtin(Builtin::JsonLoads, vec![text]),
        })
    }

    /// TOML shares concrete field layouts and defaults, but its document root
    /// is always a table rather than an arbitrary JSON value.
    pub(super) fn toml_decode(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        self.typed_decode(call, true)
            .map_err(|message| message.replace("JSON", "TOML"))
    }

    /// The schema is a static type expression; no runtime type object is created.
    pub(super) fn json_decode(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        self.typed_decode(call, false)
    }

    fn typed_decode(&mut self, call: &ast::ExprCall, toml: bool) -> Result<Expr, String> {
        self.reject_builtin_keywords(call)?;
        let [text, schema] = call.arguments.args.as_ref() else {
            return Err(error(
                self.source,
                call,
                "decode expects text and one record, list, or dictionary schema",
            ));
        };
        let ty = self.json_schema(schema)?;
        if !matches!(
            ty,
            Type::Class(_) | Type::ListInt | Type::List(_) | Type::Dict(_)
        ) {
            return Err(error(
                self.source,
                schema,
                "decode schema must be a fixed record, list, or string-key dictionary",
            ));
        }
        if toml && matches!(ty, Type::ListInt | Type::List(_)) {
            return Err(error(
                self.source,
                schema,
                "TOML decode schema must be a fixed record or string-key dictionary",
            ));
        }
        check_type(ty, self.types, &mut Vec::new())
            .map_err(|message| error(self.source, schema, message))?;
        Ok(Expr {
            ty,
            kind: ExprKind::Builtin(
                if toml {
                    Builtin::TomlDecode(ty)
                } else {
                    Builtin::JsonDecode(ty)
                },
                vec![self.expr_expected(text, Type::Str)?],
            ),
        })
    }

    /// Runtime schema expressions cannot use quoted annotations or resolve a
    /// namespace through a local value. Erased class receivers still name their
    /// exact owner where the type checker permits that expression.
    fn json_schema(&self, expression: &ast::Expr) -> Result<Type, String> {
        let invalid = || {
            error(
                self.source,
                expression,
                "JSON schemas require unshadowed static type expressions",
            )
        };
        if matches!(expression, ast::Expr::Name(_) | ast::Expr::Attribute(_)) {
            if !self.static_schema_name(expression) {
                return Err(invalid());
            }
            if let Some(class) = self.class_reference(expression) {
                return Ok(self.classes.declarations.instance(class));
            }
        }
        match expression {
            ast::Expr::Name(name) if self.resolve(expression).is_none() => match name.id.as_str() {
                "int" => Ok(Type::Int),
                "float" => Ok(Type::Float),
                "bool" => Ok(Type::Bool),
                "str" => Ok(Type::Str),
                _ => Err(invalid()),
            },
            ast::Expr::NoneLiteral(_) => Ok(Type::None),
            ast::Expr::Subscript(value)
                if self.static_schema_name(&value.value)
                    && self.class_reference(&value.value).is_none() =>
            {
                match &*value.value {
                    ast::Expr::Name(name)
                        if name.id == "list" && self.resolve(&value.value).is_none() =>
                    {
                        Ok(self.types.list(self.json_schema(&value.slice)?))
                    }
                    ast::Expr::Name(name)
                        if name.id == "dict" && self.resolve(&value.value).is_none() =>
                    {
                        let ast::Expr::Tuple(arguments) = &*value.slice else {
                            return Err(invalid());
                        };
                        let [key, value] = arguments.elts.as_slice() else {
                            return Err(invalid());
                        };
                        Ok(self
                            .types
                            .dict(self.json_schema(key)?, self.json_schema(value)?))
                    }
                    _ if matches!(
                        self.resolve(&value.value),
                        Some(Binding::Intrinsic(Intrinsic::Union))
                    ) =>
                    {
                        let members = if let ast::Expr::Tuple(arguments) = &*value.slice {
                            arguments
                                .elts
                                .iter()
                                .map(|value| self.json_schema(value))
                                .collect::<Result<Vec<_>, _>>()?
                        } else {
                            vec![self.json_schema(&value.slice)?]
                        };
                        self.types.union(&members).ok_or_else(invalid)
                    }
                    _ => Err(invalid()),
                }
            }
            ast::Expr::BinOp(union) if union.op == ast::Operator::BitOr => {
                let left = self.json_schema(&union.left)?;
                let right = self.json_schema(&union.right)?;
                if left == Type::None && right == Type::None {
                    return Err(invalid());
                }
                self.types.union(&[left, right]).ok_or_else(invalid)
            }
            _ => Err(invalid()),
        }
    }

    /// Checks the lexical root before resolving a static namespace path.
    fn static_schema_name(&self, expression: &ast::Expr) -> bool {
        let mut root = expression;
        while let ast::Expr::Attribute(attribute) = root {
            root = &attribute.value;
        }
        matches!(root, ast::Expr::Name(name)
            if !self.names.contains_key(name.id.as_str())
                && !self.comprehension_targets.contains(name.id.as_str()))
    }

    /// Keep source keyword order while passing a fixed native encoding ABI.
    pub(super) fn json_encode(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        let [source] = call.arguments.args.as_ref() else {
            return Err(error(
                self.source,
                call,
                "encode expects one positional value",
            ));
        };
        let mut names = HashSet::new();
        for keyword in &call.arguments.keywords {
            let name = keyword.arg.as_ref().map(|name| name.as_str());
            if !matches!(name, Some("indent" | "ensure_ascii")) || !names.insert(name) {
                return Err(error(
                    self.source,
                    keyword,
                    "encode accepts indent and ensure_ascii once each",
                ));
            }
        }
        let source = self.expr(source)?;
        let schema = source.ty;
        check_type(schema, self.types, &mut Vec::new())
            .map_err(|message| error(self.source, call, message))?;
        let slot = self.hidden(schema);
        let mut body = vec![Statement::Assign(slot, source)];
        let mut clear = vec![slot];
        let optional_int = self.types.optional(Type::Int);
        let mut indent = Expr {
            ty: optional_int,
            kind: ExprKind::Absent(Box::new(Expr {
                ty: Type::None,
                kind: ExprKind::None,
            })),
        };
        let mut ensure_ascii = Expr {
            ty: Type::Bool,
            kind: ExprKind::Bool(true),
        };
        for keyword in &call.arguments.keywords {
            let is_indent = keyword.arg.as_ref().unwrap() == "indent";
            let value = self.expr_expected(
                &keyword.value,
                if is_indent { optional_int } else { Type::Bool },
            )?;
            let slot = self.hidden(value.ty);
            body.push(Statement::Assign(slot, value));
            clear.push(slot);
            if is_indent {
                indent = self.local(slot);
            } else {
                ensure_ascii = self.local(slot);
            }
        }
        Ok(Expr {
            ty: Type::Str,
            kind: ExprKind::Scope {
                body,
                value: Box::new(Expr {
                    ty: Type::Str,
                    kind: ExprKind::Builtin(
                        Builtin::JsonEncode(schema),
                        vec![self.local(slot), indent, ensure_ascii],
                    ),
                }),
                clear,
            },
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
    let mut decoded = Vec::new();
    let mut encoded = Vec::new();
    for function in functions.iter().chain([entry]) {
        ir::walk_expressions(&function.body, &mut |value| match value.kind {
            ExprKind::Builtin(Builtin::JsonDecode(ty) | Builtin::TomlDecode(ty), _) => {
                decoded.push(ty);
            }
            ExprKind::Builtin(Builtin::JsonEncode(ty), _) => encoded.push(ty),
            _ => {}
        });
    }
    let mut decode_pending = Vec::new();
    let mut encode_pending = Vec::new();
    for (roots, pending) in [
        (&decoded, &mut decode_pending),
        (&encoded, &mut encode_pending),
    ] {
        for &ty in roots {
            check_type(ty, types, pending).map_err(str::to_owned)?;
        }
    }
    let mut visited = HashSet::new();
    let mut schemas = Vec::new();
    // Finish all decoding roots first: encoding alone never needs constructor
    // restrictions, but a type used in both directions must satisfy decoding.
    while let Some((class, decoding)) = decode_pending
        .pop()
        .map(|class| (class, true))
        .or_else(|| encode_pending.pop().map(|class| (class, false)))
    {
        if !visited.insert(class) {
            continue;
        }
        let declaration = &classes.declarations.definitions[class];
        let module = &graph.modules[declaration.definition.module];
        let pending = if decoding {
            &mut decode_pending
        } else {
            &mut encode_pending
        };
        for (field, &ty) in declaration.fields.iter().zip(&classes.fields[class]) {
            check_type(ty, types, pending)
                .map_err(|message| module.error(&field.annotation, message))?;
        }
        if !decoding {
            schemas.push(JsonRecordSchema {
                class,
                fields: declaration
                    .fields
                    .iter()
                    .map(|field| JsonField {
                        name: field.name.clone(),
                        default: None,
                    })
                    .collect(),
            });
            continue;
        }
        if declaration.generated_initializer && classes.methods[class].contains_key("__post_init__")
        {
            return Err(module.error(
                &declaration.node,
                "typed record decoding requires an initializer without __post_init__",
            ));
        }
        let initializer =
            &declaration.methods[declaration.initializer.expect("validated record schema")];
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
    if types.is_default_list_dict(ty) {
        return Err("JSON schemas require ordinary dictionaries");
    }
    if let Some(inner) = types.payload(ty).or_else(|| types.element(ty)) {
        return check_type(inner, types, pending);
    }
    if let Some((key, value)) = types.mapping(ty) {
        return if key == Type::Str {
            check_type(value, types, pending)
        } else {
            Err("JSON dictionaries require exact str keys")
        };
    }
    match ty {
        Type::Int | Type::Float | Type::Bool | Type::Str | Type::None => Ok(()),
        Type::Class(class) => {
            pending.push(class);
            Ok(())
        }
        _ => Err(
            "JSON values require int, float, bool, str, None, optional, list, dict[str, T], or fixed record types",
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
    fn json_schemas_resolve_class_receivers_instead_of_same_named_definitions() {
        let module = lower(include_str!(
            "../../../../tests/fixtures/json_records/class_schemas.py"
        ))
        .unwrap();
        assert_eq!(module.json_records.len(), 1);
        assert_eq!(module.json_records[0].class, 1);
    }

    #[test]
    fn json_schema_expressions_do_not_use_annotation_only_names() {
        for schema in [
            "\"Record\"",
            "list[\"Record\"]",
            "dict[\"str\", Record]",
            "list[cls.Union[int, None]]",
        ] {
            let source = format!(
                "import typing as cls\nfrom relar.json import decode\nclass Record:\n    value: int\n    def __init__(self, value: int) -> None:\n        self.value = value\n    @classmethod\n    def parse(cls, text: str) -> None:\n        decode(text, {schema})\n"
            );
            let message = lower(&source).err().expect("runtime schema was accepted");
            assert!(
                message.contains("unshadowed static type expressions"),
                "{message}"
            );
        }
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
