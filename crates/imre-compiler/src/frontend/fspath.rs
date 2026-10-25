//! POSIX normcase delegates to the same path protocol as os.fspath.

use super::*;

mod descriptor;
pub(super) use descriptor::protocol_call;

impl Checker<'_> {
    /// POSIX normcase(s) returns exactly os.fspath(s), with its own keyword signature.
    pub(super) fn public_fspath(
        &mut self,
        call: &ast::ExprCall,
        normcase: bool,
    ) -> Result<Expr, String> {
        calls::reject_unpacking(call, self.source)?;
        let args = call.arguments.args.as_ref();
        let keywords = call.arguments.keywords.as_ref();
        let parameter = if normcase { "s" } else { "path" };
        let path = match (args, keywords) {
            ([path], []) => path,
            ([], [kw]) if kw.arg.as_ref().is_some_and(|n| n.as_str() == parameter) => &kw.value,
            _ => {
                let count = args.len() + keywords.len();
                let message = if !normcase {
                    match (args.is_empty(), count) {
                        (true, 0 | 1) => {
                            "fspath() missing required argument 'path' (pos 1)".to_owned()
                        }
                        (true, _) => {
                            format!("fspath() takes at most 1 keyword argument ({count} given)")
                        }
                        _ => format!("fspath() takes at most 1 argument ({count} given)"),
                    }
                } else if let Some(name) = keywords
                    .iter()
                    .filter_map(|kw| kw.arg.as_ref())
                    .find(|n| n.as_str() != "s" || !args.is_empty())
                {
                    if name.as_str() == "s" {
                        "normcase() got multiple values for argument 's'".to_owned()
                    } else {
                        format!("normcase() got an unexpected keyword argument '{name}'")
                    }
                } else if args.is_empty() {
                    "normcase() missing 1 required positional argument: 's'".to_owned()
                } else {
                    format!(
                        "normcase() takes 1 positional argument but {} were given",
                        args.len()
                    )
                };
                return self.sys_type_error(call, Type::Str, message);
            }
        };
        let value = self.expr(path)?;
        let result = self.fspath(value, path)?;
        // The stubs omit invalid descriptors. The native operation above checks
        // their binding and return type, so its own outer overload is accounted for.
        self.graph.checked_overload(self.module, call.range());
        Ok(result)
    }

    /// Resolve one path-like value, preserving its return type and any user exception.
    pub(super) fn fspath(&mut self, value: Expr, at: &ast::Expr) -> Result<Expr, String> {
        self.fspath_protocol(value, at, None, None)
    }

    /// OS path converters name the operation and parameter if the protocol is missing.
    pub(super) fn syscall_fspath(
        &mut self,
        value: Expr,
        at: &ast::Expr,
        operation: &str,
        parameter: &str,
    ) -> Result<Expr, String> {
        self.fspath_protocol(value, at, None, Some((operation, parameter)))
    }

    /// A path protocol result must be a string or bytes; do not recursively call
    /// another object's protocol. Nullable and tagged inputs are evaluated once.
    pub(super) fn fspath_protocol(
        &mut self,
        value: Expr,
        at: &ast::Expr,
        returned_by: Option<usize>,
        syscall: Option<(&str, &str)>,
    ) -> Result<Expr, String> {
        if matches!(
            value.ty,
            Type::Str | Type::Bytes | Type::StrLiteral(_) | Type::StrEnum(_)
        ) || self
            .types
            .members(value.ty)
            .is_some_and(|members| members == [Type::Str, Type::Bytes])
        {
            // OS consumers need the string spelling; os.fspath itself preserves subclasses.
            return Ok(if syscall.is_some() {
                Self::string_value(value)
            } else {
                value
            });
        }
        if returned_by.is_none() && matches!(value.ty, Type::Path | Type::PurePath(_)) {
            return self.fspath_value(value, at);
        }
        if let Some(selected) = self.map_path_protocol_arms(&value, at, |checker, arm| {
            checker.fspath_protocol(arm, at, returned_by, syscall)
        })? {
            return Ok(selected);
        }
        if returned_by.is_none()
            && let Type::Class(id) = value.ty
        {
            if self.classes.has_concrete_subclasses(id) {
                return self.dispatch_record(id, value, at, |checker, class, receiver| {
                    checker.exact_fspath(class, receiver, at, syscall)
                });
            }
            return self.exact_fspath(id, value, at, syscall);
        }
        Ok(self.fspath_type_error(value, returned_by, syscall))
    }

    /// Check one actual protocol operand or descriptor result, including None.
    pub(super) fn map_path_protocol_arms(
        &mut self,
        value: &Expr,
        at: &ast::Expr,
        mut check: impl FnMut(&mut Self, Expr) -> Result<Expr, String>,
    ) -> Result<Option<Expr>, String> {
        let Some((slot, test, first, second)) = self.protocol_alternatives(value.ty) else {
            return Ok(None);
        };
        let mut left = check(self, first)?;
        let mut right = check(self, second)?;
        // A branch that only raises supplies no Python value to the join.
        if matches!(left.kind, ExprKind::Builtin(Builtin::RaiseException, _)) {
            left.ty = right.ty;
        } else if matches!(right.kind, ExprKind::Builtin(Builtin::RaiseException, _)) {
            right.ty = left.ty;
        }
        let ty = self.types.join(left.ty, right.ty).ok_or_else(|| {
            error(
                self.source,
                at,
                "path protocol branches require compatible native return types",
            )
        })?;
        Ok(Some(Expr {
            ty,
            kind: ExprKind::Scope {
                body: vec![Statement::Assign(slot, value.clone())],
                value: Box::new(Expr {
                    ty,
                    kind: ExprKind::Conditional(
                        Box::new(test),
                        Box::new(self.coerce(left, ty, at)?),
                        Box::new(self.coerce(right, ty, at)?),
                    ),
                }),
                clear: vec![slot],
            },
        }))
    }

    pub(super) fn exact_fspath(
        &mut self,
        id: usize,
        receiver: Expr,
        at: &ast::Expr,
        syscall: Option<(&str, &str)>,
    ) -> Result<Expr, String> {
        let Some(method) = self.classes.methods[id].get("__fspath__").cloned() else {
            return Ok(self.fspath_type_error(receiver, None, syscall));
        };
        let result = self.call_fspath_descriptor(id, receiver, &method, at, syscall)?;
        self.fspath_protocol(result, at, Some(id), syscall)
    }

    fn fspath_type_error(
        &self,
        evaluated: Expr,
        returned_by: Option<usize>,
        syscall: Option<(&str, &str)>,
    ) -> Expr {
        let actual = self.path_argument_type_name(evaluated.ty);
        let message = match (returned_by, syscall) {
            (Some(id), _) => format!(
                "expected {}.__fspath__() to return str or bytes, not {actual}",
                self.classes.declarations.definitions[id].name
            ),
            (None, Some((operation, parameter))) => format!(
                "{operation}: {parameter} should be string, bytes or os.PathLike, not {actual}"
            ),
            (None, None) => format!("expected str, bytes or os.PathLike object, not {actual}"),
        };
        Self::path_operand_type_error(evaluated, Type::Str, message)
    }

    /// Evaluate the operand before raising an error about its actual Python type.
    fn path_operand_type_error(evaluated: Expr, result: Type, message: String) -> Expr {
        let exception = Expr {
            ty: Type::Exception,
            kind: ExprKind::Exception {
                kind: exception_kind("TypeError").unwrap(),
                message: Some(Box::new(unpacking::string(message))),
            },
        };
        Expr {
            ty: result,
            kind: ExprKind::Builtin(Builtin::RaiseException, vec![evaluated, exception]),
        }
    }
    /// Use the Python object type name when reporting an invalid filesystem operand.
    pub(super) fn path_argument_type_name(&self, ty: Type) -> String {
        match ty {
            Type::None => "NoneType".to_owned(),
            Type::List(_) | Type::ListInt | Type::ByteOutput => "list".to_owned(),
            Type::Dict(_) => "dict".to_owned(),
            Type::Set(_) => "set".to_owned(),
            Type::Tuple(_) => "tuple".to_owned(),
            Type::Class(id) => self.classes.declarations.definitions[id].name.clone(),
            ty => self.type_name(ty),
        }
    }
}
