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
            "JSON comparisons require == or != with another parsed value or a literal string",
        )
    };
    let [left, right]: [Expr; 2] = values.try_into().map_err(|_| invalid())?;
    let [op @ (ast::CmpOp::Eq | ast::CmpOp::NotEq)] = ops else {
        return Err(invalid());
    };
    let result = if matches!(left.kind, ExprKind::Str(_)) {
        select(right, Type::Bool, None, Some(left))
    } else if matches!(right.kind, ExprKind::Str(_)) {
        select(left, Type::Bool, None, Some(right))
    } else {
        let parsed = |ty| matches!(ty, Type::JsonRoot | Type::JsonObject | Type::JsonValue);
        if !parsed(left.ty) || !parsed(right.ty) {
            return Err(invalid());
        }
        Expr {
            ty: Type::Bool,
            kind: ExprKind::Builtin(Builtin::JsonEqual, vec![left, right]),
        }
    };
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
    /// A checked object yields its actual string keys; arrays and untyped
    /// roots expose JSON values until an element is projected to a Python type.
    pub(super) fn json_iteration(&self, value: Expr) -> Expr {
        let element = match value.ty {
            Type::JsonObject => Type::Str,
            Type::JsonRoot | Type::JsonValue => Type::JsonValue,
            _ => return value,
        };
        Expr {
            ty: self.types.list(element),
            kind: ExprKind::Builtin(Builtin::JsonIter, vec![value]),
        }
    }

    /// Expose loads or a saved decoder's decode method through the ordinary
    /// callable ABI, with the same input and result checks as a direct call.
    pub(super) fn json_load_callable(
        &mut self,
        receiver: Option<Expr>,
        expected: Option<Type>,
        site: &impl Ranged,
    ) -> Result<Expr, String> {
        let (params, returns) = expected
            .and_then(|ty| self.types.signature(ty))
            .unwrap_or_else(|| (vec![Type::Str], Type::JsonRoot));
        let [input] = params.as_slice() else {
            return Err(error(
                self.source,
                site,
                "a JSON decoder callable requires one positional input",
            ));
        };
        if *input != Type::Str && (receiver.is_some() || *input != Type::Bytes) {
            return Err(error(
                self.source,
                site,
                "a JSON decoder callable needs a supported text input",
            ));
        }
        // A callable owns a compiler record as its context. The tuple holds
        // the decoder using its own runtime object's retain/release ABI.
        let context = receiver.map(|value| Expr {
            ty: self.types.tuple(vec![Type::JsonDecoder]),
            kind: ExprKind::Tuple(vec![value]),
        });
        let mut locals = Vec::new();
        let mut operands = Vec::new();
        let builtin = if let Some(context) = &context {
            locals.push(context.ty);
            operands.push(Expr {
                ty: Type::JsonDecoder,
                kind: ExprKind::Field(
                    Box::new(Expr {
                        ty: context.ty,
                        kind: ExprKind::Local(0),
                    }),
                    0,
                ),
            });
            Builtin::JsonDecoderDecode
        } else {
            Builtin::JsonLoads
        };
        operands.push(Expr {
            ty: *input,
            kind: ExprKind::Local(locals.len()),
        });
        locals.push(*input);
        let value = self.coerce(
            Expr {
                ty: Type::JsonRoot,
                kind: ExprKind::Builtin(builtin, operands),
            },
            returns,
            site,
        )?;
        let id = self.helpers.reserve();
        self.helpers.finish(
            id,
            comprehensions::Helper {
                module: self.module,
                calls: HashSet::new(),
                class_reads: HashSet::new(),
                recursion_error: self.graph.modules[self.module].error(
                    site,
                    "recursive calls through a JSON decoder are unsupported",
                ),
                function: ir::Function {
                    params: (0..locals.len()).collect(),
                    locals,
                    returns,
                    body: vec![Statement::Return(value)],
                },
            },
        );
        Ok(Expr {
            ty: self.types.callable(params, returns),
            kind: ExprKind::Callable {
                function: id,
                receiver: context.map(Box::new),
            },
        })
    }

    /// Nested array projections may be shared by independent document views.
    /// Only the original root list exposes the existing mutable Python list.
    pub(super) fn json_mutation_receiver(
        &self,
        value: Expr,
        node: &impl Ranged,
    ) -> Result<Expr, String> {
        if self.types.element(value.ty) == Some(Type::JsonObject) {
            return Err(error(
                self.source,
                node,
                "lists of readonly JSON object views cannot be mutated",
            ));
        }
        if value.ty == Type::JsonRoot {
            return Ok(select(value, self.types.list(Type::JsonValue), None, None));
        }
        Ok(value)
    }

    pub(super) fn json_get(
        &mut self,
        receiver: Expr,
        call: &ast::ExprCall,
    ) -> Result<Expr, String> {
        self.reject_builtin_keywords(call)?;
        let args = call.arguments.args.as_ref();
        let [ast::Expr::StringLiteral(key), rest @ ..] = args else {
            return Err(error(
                self.source,
                call,
                "readonly JSON get requires a literal string key",
            ));
        };
        check_string(key, self.source)?;
        let (empty_list, values) = match rest {
            [] => (
                false,
                vec![
                    receiver,
                    Expr {
                        ty: Type::None,
                        kind: ExprKind::None,
                    },
                ],
            ),
            [ast::Expr::List(list)] if list.elts.is_empty() => (true, vec![receiver]),
            [default] => {
                let default = self.expr(default)?;
                if !matches!(
                    default.ty,
                    Type::None
                        | Type::Bool
                        | Type::Int
                        | Type::Float
                        | Type::Str
                        | Type::StrLiteral(_)
                ) {
                    return Err(error(
                        self.source,
                        call,
                        "readonly JSON get requires a scalar or empty list default",
                    ));
                }
                // Bind the receiver before an effectful default. Literal and local
                // integer defaults retain the fast path for adding two fields.
                let receiver = if matches!(default.kind, ExprKind::Int(_) | ExprKind::Local(_)) {
                    receiver
                } else {
                    Expr {
                        ty: receiver.ty,
                        kind: ExprKind::Builtin(Builtin::JsonGetReceiver, vec![receiver]),
                    }
                };
                (false, vec![receiver, default])
            }
            _ => {
                return Err(error(
                    self.source,
                    call,
                    "readonly JSON get takes a key and optional default",
                ));
            }
        };
        Ok(Expr {
            ty: Type::JsonValue,
            kind: ExprKind::Builtin(
                Builtin::JsonGet {
                    key: key.value.to_string(),
                    empty_list,
                },
                values,
            ),
        })
    }

    pub(super) fn json_loads(&mut self, call: &ast::ExprCall, file: bool) -> Result<Expr, String> {
        self.reject_builtin_keywords(call)?;
        let [text] = call.arguments.args.as_ref() else {
            return Err(error(
                self.source,
                call,
                if file {
                    "json.load requires one open stream"
                } else {
                    "json.loads requires one str or bytes argument"
                },
            ));
        };
        let value = self.expr(text)?;
        let text = if file && value.ty == Type::BinaryIO {
            Expr {
                ty: Type::Bytes,
                kind: ExprKind::Builtin(Builtin::BinaryRead, vec![value]),
            }
        } else if file {
            let stream = self.coerce(value, Type::TextIO, text)?;
            Expr {
                ty: Type::Str,
                kind: ExprKind::Builtin(Builtin::TextRead, vec![stream]),
            }
        } else if value.ty == Type::Bytes {
            value
        } else {
            self.coerce(value, Type::Str, text)?
        };
        Ok(Expr {
            ty: Type::JsonRoot,
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
    pub(super) fn json_encode(
        &mut self,
        call: &ast::ExprCall,
        op: Intrinsic,
    ) -> Result<Expr, String> {
        let standard = op != Intrinsic::JsonEncode;
        let (source, stream) = match (op, call.arguments.args.as_ref()) {
            (Intrinsic::JsonDump, [value, stream]) => (value, Some(stream)),
            (Intrinsic::JsonEncode | Intrinsic::JsonDumps, [value]) => (value, None),
            _ => {
                return Err(error(
                    self.source,
                    call,
                    if op == Intrinsic::JsonDump {
                        "json.dump requires a value and a text stream"
                    } else {
                        "JSON encoding requires one positional value"
                    },
                ));
            }
        };
        let mut names = HashSet::new();
        for keyword in &call.arguments.keywords {
            let name = keyword.arg.as_ref().map(|name| name.as_str());
            if !(matches!(name, Some("indent" | "ensure_ascii"))
                || standard && matches!(name, Some("allow_nan" | "sort_keys" | "separators")))
                || !names.insert(name)
            {
                return Err(error(
                    self.source,
                    keyword,
                    "unsupported or repeated JSON encoding option",
                ));
            }
        }
        let source = match source {
            display if standard && documents::constant(display) && needs_document(display) => {
                self.expr_expected(display, documents::ANY)?
            }
            ast::Expr::List(items) if standard && items.elts.is_empty() => {
                self.expr_expected(source, Type::ListInt)?
            }
            ast::Expr::Dict(items) if standard && items.items.is_empty() => {
                self.expr_expected(source, self.types.dict(Type::Str, Type::Int))?
            }
            _ => self.expr(source)?,
        };
        let schema = source.ty;
        if standard {
            check_standard_encoding(schema, self.types)
        } else {
            check_type(schema, self.types, &mut Vec::new())
        }
        .map_err(|message| error(self.source, call, message))?;
        let slot = self.hidden(schema);
        let mut body = vec![Statement::Assign(slot, source)];
        let mut clear = vec![slot];
        let file = if let Some(stream) = stream {
            let stream = self.expr_expected(stream, Type::TextIO)?;
            let file = self.hidden(Type::TextIO);
            body.push(Statement::Assign(file, stream));
            clear.push(file);
            Some(self.local(file))
        } else {
            None
        };
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
        let mut allow_nan = ensure_ascii.clone();
        let mut sort_keys = Expr {
            ty: Type::Bool,
            kind: ExprKind::Bool(false),
        };
        let mut separators = Expr {
            ty: Type::None,
            kind: ExprKind::None,
        };
        for keyword in &call.arguments.keywords {
            let name = keyword.arg.as_ref().unwrap().as_str();
            let value = if name == "separators" {
                let value = match &keyword.value {
                    // The encoder only unpacks this fresh display; its members can
                    // have different types, and each must be evaluated in order.
                    ast::Expr::List(items)
                        if !items.elts.iter().any(ast::Expr::is_starred_expr) =>
                    {
                        let values = items
                            .elts
                            .iter()
                            .map(|item| self.expr(item))
                            .collect::<Result<Vec<_>, _>>()?;
                        Expr {
                            ty: self.types.tuple(values.iter().map(|v| v.ty).collect()),
                            kind: ExprKind::Tuple(values),
                        }
                    }
                    ast::Expr::Dict(items) if items.items.is_empty() => {
                        self.expr_expected(&keyword.value, self.types.dict(Type::Str, Type::Int))?
                    }
                    _ => self.expr(&keyword.value)?,
                };
                check_standard_encoding(value.ty, self.types)
                    .map_err(|message| error(self.source, keyword, message))?;
                value
            } else if name == "sort_keys" {
                self.condition(&keyword.value)?
            } else if name == "indent" && standard {
                let value = self.expr(&keyword.value)?;
                match self.types.payload(value.ty).unwrap_or(value.ty) {
                    Type::Str | Type::StrLiteral(_) => Self::string_value(value),
                    Type::Bool if value.ty == Type::Bool => Expr {
                        ty: Type::Int,
                        kind: ExprKind::Builtin(Builtin::Int, vec![value]),
                    },
                    _ => self.coerce(value, optional_int, &keyword.value)?,
                }
            } else {
                self.expr_expected(
                    &keyword.value,
                    if name == "indent" {
                        optional_int
                    } else {
                        Type::Bool
                    },
                )?
            };
            let slot = self.hidden(value.ty);
            body.push(Statement::Assign(slot, value));
            clear.push(slot);
            match name {
                "indent" => indent = self.local(slot),
                "ensure_ascii" => ensure_ascii = self.local(slot),
                "allow_nan" => allow_nan = self.local(slot),
                "sort_keys" => sort_keys = self.local(slot),
                _ => separators = self.local(slot),
            }
        }
        let mut args = vec![self.local(slot), indent, ensure_ascii];
        if standard {
            args.extend([allow_nan, sort_keys, separators]);
        }
        let (builtin, ty) = if let Some(stream) = file {
            args.push(stream);
            (Builtin::JsonDump(schema), Type::None)
        } else if standard {
            (Builtin::JsonDumps(schema), Type::Str)
        } else {
            (Builtin::JsonEncode(schema), Type::Str)
        };
        Ok(Expr {
            ty,
            kind: ExprKind::Scope {
                body,
                value: Box::new(Expr {
                    ty,
                    kind: ExprKind::Builtin(builtin, args),
                }),
                clear,
            },
        })
    }
}

/// A homogeneous scalar display already has ordinary typed storage. A fresh
/// mixed or nested JSON display can use the existing native document containers.
fn needs_document(display: &ast::Expr) -> bool {
    fn mixed<'a>(mut values: impl Iterator<Item = &'a ast::Expr>) -> bool {
        let Some(first) = values.next() else {
            return false;
        };
        let kind = |x: &ast::Expr| match x {
            ast::Expr::StringLiteral(_) => 1,
            ast::Expr::BooleanLiteral(_) => 2,
            ast::Expr::NoneLiteral(_) => 3,
            ast::Expr::NumberLiteral(n) => match n.value {
                ast::Number::Int(_) => 4,
                ast::Number::Float(_) => 5,
                _ => 6,
            },
            _ => 0,
        };
        let first = kind(first);
        first == 0 || values.any(|v| kind(v) != first)
    }
    match display {
        ast::Expr::List(l) => mixed(l.elts.iter()),
        ast::Expr::Dict(d) => mixed(d.items.iter().map(|i| &i.value)),
        _ => false,
    }
}

fn check_standard_encoding(ty: Type, types: &Types) -> Result<(), &'static str> {
    if let Some(child) = types.payload(ty).or_else(|| types.element(ty)) {
        return check_standard_encoding(child, types);
    }
    if let Some((key, child)) = types.mapping(ty) {
        if !matches!(key, Type::Str | Type::Int) {
            return Err("json.dumps supports dictionaries with string or integer keys");
        }
        return check_standard_encoding(child, types);
    }
    if let Some(fields) = types.fields(ty) {
        for field in fields {
            check_standard_encoding(field, types)?;
        }
        return Ok(());
    }
    match ty {
        Type::None
        | Type::Int
        | Type::Bool
        | Type::Float
        | Type::Str
        | Type::StrLiteral(_)
        | Type::Document(_)
        | Type::JsonRoot
        | Type::JsonValue
        | Type::JsonObject
        | Type::Bytes
        | Type::Set(_)
        | Type::Class(_)
        | Type::Path => Ok(()),
        _ => Err("json.dumps requires concrete builtin values"),
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
    if types.is_default_list_dict(ty) || types.is_counter(ty) || types.is_ordered_dict(ty) {
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
                "import typing as cls\nfrom imre.json import decode\nclass Record:\n    value: int\n    def __init__(self, value: int) -> None:\n        self.value = value\n    @classmethod\n    def parse(cls, text: str) -> None:\n        decode(text, {schema})\n"
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
import imre.json
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
report = imre.json.decode("{}", Report)
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
                "from imre.json import decode\n{class}item = decode(\"{{}}\", Item)\n"
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
from imre.json import decode
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
