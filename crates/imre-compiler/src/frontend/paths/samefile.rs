//! Path.samefile stats its receiver first, then compares inode and device in that order.

use super::*;

fn missing_attribute(ty: &str, name: &str) -> Expr {
    Expr {
        ty: Type::Exception,
        kind: ExprKind::Exception {
            kind: exception_kind("AttributeError").unwrap(),
            message: Some(Box::new(unpacking::string(format!(
                "'{ty}' object has no attribute '{name}'"
            )))),
        },
    }
}

impl Checker<'_> {
    pub(super) fn path_samefile(
        &mut self,
        receiver: Expr,
        call: &ast::ExprCall,
    ) -> Result<Expr, String> {
        calls::reject_unpacking(call, self.source)?;
        let args = &call.arguments.args;
        let mut other = args.first();
        let mut binding_error = None;
        for keyword in &call.arguments.keywords {
            let name = keyword.arg.as_deref().unwrap();
            if name == "self" || (name == "other_path" && other.is_some()) {
                self.graph
                    .checked_repeated_keyword(self.module, keyword.range());
                binding_error.get_or_insert_with(|| {
                    format!("Path.samefile() got multiple values for argument '{name}'")
                });
            } else if name == "other_path" {
                other = Some(&keyword.value);
            } else {
                self.graph
                    .checked_keywords(self.module, std::iter::once(keyword.range()));
                binding_error.get_or_insert_with(|| {
                    format!("Path.samefile() got an unexpected keyword argument '{name}'")
                });
            }
        }
        if args.len() > 1 {
            for extra in &args[1..] {
                self.graph
                    .checked_excess_argument(self.module, extra.range());
            }
            binding_error.get_or_insert_with(|| {
                format!(
                    "Path.samefile() takes 2 positional arguments but {} were given",
                    args.len() + 1
                )
            });
        } else if other.is_none() {
            self.graph
                .checked_missing_argument(self.module, call.range());
            binding_error.get_or_insert_with(|| {
                "Path.samefile() missing 1 required positional argument: 'other_path'".to_owned()
            });
        }
        let mut scope = calls::ArgumentScope::default();
        let receiver = scope.hold(self, receiver);
        if let Some(message) = binding_error {
            return Ok(scope.finish(self.sys_type_error(call, Type::Bool, message)?));
        }
        let at = other.unwrap();
        let right = self.expr(at)?;
        let right = scope.hold(self, right);
        let left = self.follow_stat(receiver);
        let left = scope.hold(self, left);
        let result = self.samefile_other(left, right, at)?;
        Ok(scope.finish(result))
    }

    fn follow_stat(&self, path: Expr) -> Expr {
        operation(
            PathOperation::Stat { lstat: false },
            self.types.stat_result(),
            vec![path, boolean(true)],
        )
    }

    fn samefile_other(&mut self, left: Expr, other: Expr, at: &ast::Expr) -> Result<Expr, String> {
        if other.ty == Type::Path {
            let mut scope = calls::ArgumentScope::default();
            let stat = self.follow_stat(other);
            let right = scope.hold(self, stat);
            return Ok(scope.finish(self.samefile_stats(left, right, at)?));
        }
        if let Some(selected) = self.map_path_protocol_arms(&other, at, |checker, arm| {
            checker.samefile_other(left.clone(), arm, at)
        })? {
            return Ok(selected);
        }
        if let Type::Class(id) = other.ty
            && self.classes.has_concrete_subclasses(id)
        {
            return self.dispatch_record(id, other, at, |checker, _, receiver| {
                checker.samefile_exact(left.clone(), receiver, at)
            });
        }
        self.samefile_exact(left, other, at)
    }

    fn samefile_exact(&mut self, left: Expr, other: Expr, at: &ast::Expr) -> Result<Expr, String> {
        let path = self.constructor_part(other.clone(), at, Type::Path)?;
        let path = self.concrete_from_part(path, at)?;
        let fallback = self.follow_stat(path);
        let fs_result = self.hidden(fallback.ty);
        let mut clear = vec![fs_result];
        let mut handler = vec![Statement::Assign(fs_result, fallback)];
        let call = self.samefile_stat_call(&other, at)?;
        let (body, value) = if let Some(call) = call {
            if call.ty == self.types.stat_result() {
                (
                    vec![Statement::Assign(fs_result, call)],
                    self.samefile_stats(left, self.local(fs_result), at)?,
                )
            } else {
                let selected = self.hidden(Type::Bool);
                let custom = self.hidden(call.ty);
                clear.extend([selected, custom]);
                handler.push(Statement::Assign(selected, boolean(false)));
                let a = self.samefile_stats(left.clone(), self.local(custom), at)?;
                let b = self.samefile_stats(left, self.local(fs_result), at)?;
                (
                    vec![
                        Statement::Assign(custom, call),
                        Statement::Assign(selected, boolean(true)),
                    ],
                    Expr {
                        ty: Type::Bool,
                        kind: ExprKind::Conditional(
                            Box::new(self.local(selected)),
                            Box::new(a),
                            Box::new(b),
                        ),
                    },
                )
            }
        } else {
            (
                vec![Statement::Raise(Some(missing_attribute(
                    &self.path_argument_type_name(other.ty),
                    "stat",
                )))],
                self.samefile_stats(left, self.local(fs_result), at)?,
            )
        };
        // Keep only the stat method call in the try: an AttributeError from a result's
        // st_ino/st_dev must escape, and a conversion failure belongs to the handler.
        Ok(Expr {
            ty: Type::Bool,
            kind: ExprKind::Scope {
                body: vec![Statement::Try {
                    body,
                    handlers: vec![ir::ExceptHandler {
                        kinds: vec![exception_kind("AttributeError").unwrap()],
                        name: None,
                        body: handler,
                    }],
                    orelse: vec![],
                    finalbody: vec![],
                }],
                value: Box::new(value),
                clear,
            },
        })
    }

    /// Ordinary attribute lookup may yield a bound method, a property, or an
    /// instance field. Only AttributeError while looking it up or calling it
    /// triggers Path's conversion fallback.
    fn samefile_stat_call(&mut self, other: &Expr, at: &ast::Expr) -> Result<Option<Expr>, String> {
        let Type::Class(class) = other.ty else {
            return Ok(None);
        };
        let call = fspath::protocol_call(at, "stat");
        let attribute = call.func.as_attribute_expr().unwrap();
        let result = self.types.stat_result();
        let record = &self.classes.declarations.definitions[class];
        let value = if let Some(method) = self.classes.methods[class].get("stat").cloned() {
            if method.method.unwrap().kind(&self.classes.declarations)
                != crate::classes::MethodKind::Property
            {
                return self
                    .call_path_method(&call, other.clone(), &method, result)
                    .map(Some);
            }
            self.direct_class_attribute(attribute, class, Some(other.clone()), None)?
        } else if let Some(index) = record.fields.iter().position(|f| f.name == "stat") {
            Expr {
                ty: self.classes.fields[class][index],
                kind: ExprKind::Field(Box::new(other.clone()), index),
            }
        } else if record.static_bindings.contains_key("stat") {
            self.class_attribute(attribute, class, Some(other.clone()), None)?
        } else {
            return Ok(None);
        };
        self.invoke_path_value(&call, value, None, result).map(Some)
    }

    fn samefile_stats(&mut self, left: Expr, right: Expr, at: &ast::Expr) -> Result<Expr, String> {
        if let Some(selected) = self.map_path_protocol_arms(&right, at, |checker, arm| {
            checker.samefile_stats(left.clone(), arm, at)
        })? {
            return Ok(selected);
        }
        if let Type::Class(class) = right.ty
            && self.classes.has_concrete_subclasses(class)
        {
            return self.dispatch_record(class, right, at, |checker, _, exact| {
                checker.samefile_exact_stats(left.clone(), exact, at)
            });
        }
        self.samefile_exact_stats(left, right, at)
    }

    fn samefile_exact_stats(
        &mut self,
        left: Expr,
        right: Expr,
        at: &ast::Expr,
    ) -> Result<Expr, String> {
        let mut equal_field = |index| -> Result<Expr, String> {
            let a = Expr {
                ty: Type::Int,
                kind: ExprKind::Field(Box::new(left.clone()), index),
            };
            let b = self.samefile_field(right.clone(), index, at)?;
            self.samefile_number_equal(a, b, at)
        };
        let ino = equal_field(1)?;
        let dev = equal_field(2)?;
        Ok(Expr {
            ty: Type::Bool,
            kind: ExprKind::Conditional(Box::new(ino), Box::new(dev), Box::new(boolean(false))),
        })
    }

    /// The filesystem result is integral, but a user-supplied stat object may
    /// return another type. Evaluate its attribute even when equality is false.
    fn samefile_number_equal(
        &mut self,
        integer: Expr,
        field: Expr,
        at: &ast::Expr,
    ) -> Result<Expr, String> {
        if let Some(selected) = self.map_path_protocol_arms(&field, at, |checker, arm| {
            checker.samefile_number_equal(integer.clone(), arm, at)
        })? {
            return Ok(selected);
        }
        let field = match field.ty {
            Type::Int | Type::Float => field,
            Type::Bool => Expr {
                ty: Type::Int,
                kind: ExprKind::Builtin(Builtin::Int, vec![field]),
            },
            Type::None
            | Type::Str
            | Type::StrLiteral(_)
            | Type::StrEnum(_)
            | Type::Enum(_)
            | Type::Bytes
            | Type::List(_)
            | Type::ListInt
            | Type::Dict(_)
            | Type::Set(_)
            | Type::Tuple(_)
            | Type::Callable(_)
            | Type::PlainObject
            | Type::Path
            | Type::PurePath(_) => {
                return Ok(Expr {
                    ty: Type::Bool,
                    kind: ExprKind::Scope {
                        body: vec![Statement::Expr(integer), Statement::Expr(field)],
                        value: Box::new(boolean(false)),
                        clear: vec![],
                    },
                });
            }
            _ => {
                return Err(error(
                    self.source,
                    at,
                    "cannot compare this stat attribute with a native inode or device number",
                ));
            }
        };
        Ok(pipelines::compare(Compare::Equal, integer, field))
    }

    fn samefile_field(&mut self, stat: Expr, index: usize, at: &ast::Expr) -> Result<Expr, String> {
        if stat.ty == self.types.stat_result() {
            return Ok(Expr {
                ty: Type::Int,
                kind: ExprKind::Field(Box::new(stat), index),
            });
        }
        let name = crate::types::STAT_SEQUENCE_FIELDS[index];
        if let Type::Class(id) = stat.ty {
            let item = &self.classes.declarations.definitions[id];
            let attribute = fspath::protocol_call(at, name);
            let attribute = attribute.func.as_attribute_expr().unwrap();
            let value = if self.classes.methods[id].contains_key(name) {
                Some(self.direct_class_attribute(attribute, id, Some(stat.clone()), None)?)
            } else if item.static_bindings.contains_key(name) {
                Some(self.class_attribute(attribute, id, Some(stat.clone()), None)?)
            } else {
                item.fields
                    .iter()
                    .position(|f| f.name == name)
                    .map(|i| Expr {
                        ty: self.classes.fields[id][i],
                        kind: ExprKind::Field(Box::new(stat.clone()), i),
                    })
            };
            if let Some(value) = value {
                return Ok(value);
            }
        }
        let exc = missing_attribute(&self.path_argument_type_name(stat.ty), name);
        Ok(Expr {
            ty: Type::Int,
            kind: ExprKind::Builtin(Builtin::RaiseException, vec![stat, exc]),
        })
    }
}
