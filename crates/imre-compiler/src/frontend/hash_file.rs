//! Bind the native file_digest call and evaluate its operands before constructing the hash.
use super::*;

impl Checker<'_> {
    pub(super) fn hash_file_digest(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        let [stream, digest] = call.arguments.args.as_ref() else {
            return Err(error(
                self.source,
                call,
                "file_digest requires a file and digest as positional arguments",
            ));
        };
        let stream = self.expr(stream)?;
        let stream_ty = stream.ty;
        if !matches!(stream_ty, Type::BytesIO | Type::BinaryIO | Type::None) {
            return Err(error(
                self.source,
                call,
                "file_digest requires an io.BytesIO or native binary file",
            ));
        }
        let file = self.hidden(stream_ty);
        let hash = self.hidden(Type::Hash);
        let mut body = vec![Statement::Assign(file, stream)];
        let mut clear = vec![file, hash];

        // Only a direct, unshadowed import can supply a constructor without evaluating a receiver.
        let base = digest
            .as_attribute_expr()
            .map_or(digest, |attr| &attr.value);
        let binding = base
            .as_name_expr()
            .filter(|n| !self.names.contains_key(n.id.as_str()))
            .and_then(|_| self.resolve(digest));
        let empty = Expr {
            ty: Type::Bytes,
            kind: ExprKind::Bytes(Vec::new()),
        };
        let constructor = match binding {
            Some(Binding::Intrinsic(Intrinsic::Sha1)) => {
                ExprKind::Builtin(Builtin::Sha1New, vec![empty])
            }
            Some(Binding::Intrinsic(Intrinsic::Sha256)) => {
                ExprKind::Builtin(Builtin::Sha256New, vec![empty])
            }
            Some(Binding::Intrinsic(Intrinsic::NamedHash(algorithm))) => ExprKind::Builtin(
                Builtin::HashNew,
                vec![unpacking::string(algorithm), empty, Expr::boolean(true)],
            ),
            _ => {
                let name = self.expr(digest)?;
                if !matches!(name.ty, Type::Str | Type::StrLiteral(_)) {
                    return Err(error(
                        self.source,
                        digest,
                        "file_digest requires a str algorithm name or a supported hashlib constructor",
                    ));
                }
                let slot = self.hidden(name.ty);
                body.push(Statement::Assign(slot, name));
                clear.push(slot);
                ExprKind::Builtin(
                    Builtin::HashNew,
                    vec![self.local(slot), empty, Expr::boolean(true)],
                )
            }
        };
        let size = match call.arguments.keywords.as_ref() {
            [] => Expr::integer(1 << 18),
            [size] if size.arg.as_deref() == Some("_bufsize") => {
                let value = self.evaluated_argument(&size.value)?;
                if stream_ty == Type::BinaryIO && !matches!(value.ty, Type::Int | Type::Bool) {
                    return Err(error(
                        self.source,
                        &size.value,
                        "native binary file_digest requires an integer _bufsize",
                    ));
                }
                let slot = self.hidden(value.ty);
                body.push(Statement::Assign(slot, value));
                clear.push(slot);
                self.local(slot)
            }
            _ => {
                return Err(error(
                    self.source,
                    call,
                    "file_digest accepts only the _bufsize keyword",
                ));
            }
        };

        // Constructor errors precede stream checks and buffer allocation.
        body.push(Statement::Assign(
            hash,
            Expr {
                ty: Type::Hash,
                kind: constructor,
            },
        ));
        let update = match stream_ty {
            Type::BytesIO => {
                // BytesIO's getbuffer path hashes the complete contents without moving the cursor.
                let bytes = Expr {
                    ty: Type::Bytes,
                    kind: ExprKind::Builtin(Builtin::BytesIoValue, vec![self.local(file)]),
                };
                ExprKind::Builtin(Builtin::HashUpdate, vec![self.local(hash), bytes])
            }
            Type::BinaryIO => {
                let count = if size.ty == Type::Bool {
                    Expr {
                        ty: Type::Int,
                        kind: ExprKind::Builtin(Builtin::Int, vec![size]),
                    }
                } else {
                    size
                };
                ExprKind::Builtin(
                    Builtin::HashUpdateFile,
                    vec![self.local(hash), self.local(file), count],
                )
            }
            Type::None => ExprKind::Builtin(
                Builtin::RaiseException,
                vec![Expr {
                    ty: Type::Exception,
                    kind: ExprKind::Exception {
                        kind: exception_kind("ValueError").unwrap(),
                        message: Some(Box::new(unpacking::string(
                            "'None' is not a file-like object in binary reading mode.",
                        ))),
                    },
                }],
            ),
            _ => unreachable!("stream type was checked above"),
        };
        body.push(Statement::Expr(Expr {
            ty: Type::None,
            kind: update,
        }));
        Ok(Expr {
            ty: Type::Hash,
            kind: ExprKind::Scope {
                body,
                value: Box::new(self.local(hash)),
                clear,
            },
        })
    }
}
