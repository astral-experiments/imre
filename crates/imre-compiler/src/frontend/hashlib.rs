//! A sealed hash object owns its state. Reading a digest leaves it usable.
use super::calls::parameter;
use super::*;

impl Checker<'_> {
    pub(super) fn hmac_compare_digest(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        self.reject_builtin_keywords(call)?;
        let [left, right] = call.arguments.args.as_ref() else {
            return Err(error(
                self.source,
                call,
                "compare_digest requires two positional arguments",
            ));
        };
        let left = self.evaluated_argument(left)?;
        let right = self.evaluated_argument(right)?;
        let text = match (left.ty, right.ty) {
            (Type::Bytes, Type::Bytes) => false,
            (Type::Str | Type::StrLiteral(_), Type::Str | Type::StrLiteral(_)) => true,
            (a, b) => {
                let (Some(a_name), Some(b_name)) =
                    (self.known_nonbuffer_type(a), self.known_nonbuffer_type(b))
                else {
                    return Err(error(
                        self.source,
                        call,
                        "compare_digest requires concrete bytes or ASCII string operands",
                    ));
                };
                let message = if a == Type::Bytes || b == Type::Bytes {
                    let other = if a == Type::Bytes { b_name } else { a_name };
                    format!("a bytes-like object is required, not '{other}'")
                } else {
                    format!(
                        "unsupported operand types(s) or combination of types: '{a_name}' and '{b_name}'"
                    )
                };
                self.graph.checked_overload(self.module, call.range());
                return Ok(calls::type_error(Type::Bool, vec![left, right], message));
            }
        };
        Ok(Expr {
            ty: Type::Bool,
            kind: ExprKind::Builtin(Builtin::HmacCompareDigest { text }, vec![left, right]),
        })
    }

    pub(super) fn hash_constructor(
        &mut self,
        call: &ast::ExprCall,
        constructor: Intrinsic,
    ) -> Result<Expr, String> {
        if constructor == Intrinsic::HashNew {
            let name = call
                .arguments
                .args
                .first()
                .or_else(|| call.arguments.find_keyword("name").map(|kw| &kw.value));
            // Keep object dispatch available for mutable provider modules and buffer
            // types the typed constructor cannot lower.
            if name.is_some_and(|expr| match expr {
                ast::Expr::StringLiteral(lit) => {
                    let name = lit.value.to_str();
                    name.eq_ignore_ascii_case("md5")
                        || name.eq_ignore_ascii_case("ssl3-md5")
                        || name == "1.2.840.113549.2.5"
                }
                _ => true,
            }) {
                self.graph.needs_object_runtime.set(true);
            }
        }
        if matches!(
            constructor,
            Intrinsic::NamedHash("blake2b" | "blake2s") | Intrinsic::HashNew
        ) && call.arguments.keywords.iter().any(|kw| {
            matches!(
                kw.arg.as_deref(),
                Some(
                    "digest_size"
                        | "key"
                        | "salt"
                        | "person"
                        | "fanout"
                        | "depth"
                        | "leaf_size"
                        | "node_offset"
                        | "node_depth"
                        | "inner_size"
                        | "last_node"
                )
            )
        }) {
            self.graph.needs_object_runtime.set(true);
            return Err(self.classified_error(
                Kind::Object,
                call,
                "BLAKE2 parameter conversion requires native object dispatch",
            ));
        }
        let mut params = vec![
            parameter(
                "data",
                Type::Bytes,
                Some(Expr {
                    ty: Type::Bytes,
                    kind: ExprKind::Bytes(Vec::new()),
                }),
            ),
            ParameterKind::KeywordOnly.parameter(
                "usedforsecurity",
                Type::Bool,
                Some(Expr::boolean(true)),
            ),
        ];
        let named = constructor == Intrinsic::HashNew;
        let supplied = |key: &str| call.arguments.find_keyword(key).is_some();
        let string = supplied("string");
        let data = call.arguments.args.len() > usize::from(named) || supplied("data");
        if string {
            if data {
                params.push(Parameter {
                    name: "string".into(),
                    kind: ParameterKind::KeywordOnly,
                    ..params[0].clone()
                });
            } else {
                params[0].name = "string".into();
                params[0].kind = ParameterKind::KeywordOnly;
            }
        }
        let mut name = None;
        let op = if named {
            params.insert(0, parameter("name", Type::Str, None));
            Builtin::HashNew
        } else if let Intrinsic::NamedHash(algorithm) = constructor {
            name = Some(unpacking::string(algorithm));
            Builtin::HashNew
        } else if constructor == Intrinsic::Sha1 {
            Builtin::Sha1New
        } else {
            Builtin::Sha256New
        };
        let first = usize::from(matches!(op, Builtin::HashNew));
        let mut args = self.arguments_with(call, &params, name, |checker, index, node| {
            if params[index].name == "name" {
                return checker.direct_argument(node, Type::Str, false);
            }
            let value = checker.evaluated_argument(node)?;
            if value.ty != params[index].ty {
                checker
                    .graph
                    .checked_argument_type(checker.module, node.range());
            }
            Ok(value)
        })?;
        let input = args.order[first];
        let flag = args.order[first + 1];
        let conflict = string && data;
        let invalid = if conflict {
            Some(
                "'data' and 'string' are mutually exclusive and support for 'string' keyword parameter is slated for removal in a future version.",
            )
        } else {
            self.hash_input_error(args.values[input].ty, call)?
        };
        if invalid.is_some() || args.values[flag].ty != Type::Bool {
            // All caller expressions run before the flag's truth method or a buffer error.
            let mut scope = calls::ArgumentScope::default();
            args.values = args
                .values
                .into_iter()
                .map(|v| scope.hold(self, v))
                .collect();
            let flag_value = self.truth_value(args.values[flag].clone());
            if let Some(message) = invalid {
                return Ok(scope.finish(calls::type_error(
                    Type::Hash,
                    vec![flag_value],
                    message.into(),
                )));
            }
            args.values[flag] = flag_value;
            return Ok(scope.finish(self.ordered_builtin(op, Type::Hash, args)));
        }
        Ok(self.ordered_builtin(op, Type::Hash, args))
    }

    fn hash_input_error(
        &self,
        ty: Type,
        call: &ast::ExprCall,
    ) -> Result<Option<&'static str>, String> {
        match ty {
            Type::Bytes => Ok(None),
            Type::Str | Type::StrLiteral(_) => Ok(Some("Strings must be encoded before hashing")),
            _ if self.known_nonbuffer_type(ty).is_some() => {
                Ok(Some("object supporting the buffer API required"))
            }
            _ => Err(error(
                self.source,
                call,
                "hashing requires a concrete bytes-like input",
            )),
        }
    }

    pub(super) fn hash_method(
        &mut self,
        receiver: Expr,
        attr: &ast::ExprAttribute,
        call: &ast::ExprCall,
    ) -> Result<Expr, String> {
        let method = attr.attr.as_str();
        if matches!(method, "digest" | "hexdigest") {
            if !call.arguments.args.is_empty() || !call.arguments.keywords.is_empty() {
                return self.hash_digest_arguments(receiver, method, call);
            }
            // A runtime-selected XOF reports its missing length at the call.
            self.graph
                .checked_missing_argument(self.module, call.range());
        }
        self.reject_builtin_keywords(call)?;
        if attr.attr.as_str() == "hexdigest"
            && call.arguments.args.is_empty()
            && let ExprKind::Builtin(constructor @ (Builtin::Sha1New | Builtin::Sha256New), inputs) =
                receiver.kind
        {
            // An immediately consumed constructor needs no mutable state object.
            return Ok(Expr {
                ty: Type::Str,
                kind: ExprKind::Builtin(
                    if matches!(constructor, Builtin::Sha1New) {
                        Builtin::Sha1Hex
                    } else {
                        Builtin::Sha256Hex
                    },
                    inputs,
                ),
            });
        }
        let mut args = vec![receiver];
        let (op, ty) = match (attr.attr.as_str(), call.arguments.args.as_ref()) {
            ("update", [data]) => {
                let value = self.evaluated_argument(data)?;
                let invalid = self.hash_input_error(value.ty, call)?;
                args.push(value);
                if let Some(message) = invalid {
                    self.graph.checked_argument_type(self.module, data.range());
                    return Ok(calls::type_error(Type::None, args, message.into()));
                }
                (Builtin::HashUpdate, Type::None)
            }
            ("hexdigest", []) => (Builtin::HashDigest { hex: true }, Type::Str),
            ("digest", []) => (Builtin::HashDigest { hex: false }, Type::Bytes),
            ("copy", []) => (Builtin::HashCopy, Type::Hash),
            _ => {
                return Err(error(
                    self.source,
                    call,
                    "hash objects support update(bytes), digest(), hexdigest(), and copy()",
                ));
            }
        };
        Ok(Expr {
            ty,
            kind: ExprKind::Builtin(op, args),
        })
    }

    fn hash_digest_arguments(
        &mut self,
        receiver: Expr,
        method: &str,
        call: &ast::ExprCall,
    ) -> Result<Expr, String> {
        calls::reject_unpacking(call, self.source)?;
        let arguments = &call.arguments;
        let mut scope = calls::ArgumentScope::default();
        let hash = scope.hold(self, receiver);
        let mut length = None;
        let mut count = 0;
        for node in arguments
            .args
            .iter()
            .chain(arguments.keywords.iter().map(|kw| &kw.value))
        {
            let value = self.evaluated_argument(node)?;
            length = Some((scope.hold(self, value), node));
            count += 1;
            self.graph.checked_argument_type(self.module, node.range());
        }
        for node in &arguments.args {
            self.graph
                .checked_excess_argument(self.module, node.range());
        }
        self.graph
            .checked_keywords(self.module, arguments.keywords.iter().map(Ranged::range));
        self.graph
            .checked_missing_argument(self.module, call.range());
        let hex = method == "hexdigest";
        let ty = if hex { Type::Str } else { Type::Bytes };
        let fixed_error = if arguments.keywords.is_empty() {
            format!("HASH.{method}() takes no arguments ({count} given)")
        } else {
            format!("HASH.{method}() takes no keyword arguments")
        };
        let xof = if count > 1 {
            let kind = if arguments.args.is_empty() {
                "keyword "
            } else {
                ""
            };
            calls::type_error(
                ty,
                vec![],
                format!("{method}() takes at most 1 {kind}argument ({count} given)"),
            )
        } else if !arguments.keywords.is_empty() && arguments.find_keyword("length").is_none() {
            calls::type_error(
                ty,
                vec![],
                format!("{method}() missing required argument 'length' (pos 1)"),
            )
        } else {
            let (value, node) = length.unwrap();
            let length = self.index_word(value, node, false)?;
            Expr {
                ty,
                kind: ExprKind::Builtin(Builtin::HashDigest { hex }, vec![hash.clone(), length]),
            }
        };
        // Both operands evaluate before binding. Only an XOF converts the length through __index__.
        let size = Expr {
            ty: Type::Int,
            kind: ExprKind::Builtin(Builtin::HashDigestSize, vec![hash]),
        };
        Ok(scope.finish(Expr {
            ty,
            kind: ExprKind::Conditional(
                Box::new(pipelines::compare(Compare::Equal, size, Expr::integer(0))),
                Box::new(xof),
                Box::new(calls::type_error(ty, vec![], fixed_error)),
            ),
        }))
    }

    pub(super) fn hash_attribute(
        &self,
        receiver: Expr,
        attr: &ast::ExprAttribute,
    ) -> Result<Expr, String> {
        let (op, ty) = match attr.attr.as_str() {
            "__class__" => (Builtin::HashType, Type::BuiltinClass),
            "name" => (Builtin::HashName, Type::Str),
            "digest_size" => (Builtin::HashDigestSize, Type::Int),
            "block_size" => (Builtin::HashBlockSize, Type::Int),
            _ => {
                return Err(error(
                    self.source,
                    attr,
                    "hash objects expose name, digest_size, and block_size",
                ));
            }
        };
        Ok(Expr {
            ty,
            kind: ExprKind::Builtin(op, vec![receiver]),
        })
    }
}
