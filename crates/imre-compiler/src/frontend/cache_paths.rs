//! Cache filename operations use the same sealed path, truth, and string protocols as ordinary calls.
use super::*;
use ir::CachePathOperation as Op;

pub(super) fn op(operation: Op, ty: Type, args: Vec<Expr>) -> Expr {
    Expr {
        ty,
        kind: ExprKind::Builtin(Builtin::CachePath(operation), args),
    }
}

fn text(s: &str) -> Expr {
    unpacking::string(s)
}
fn none() -> Expr {
    Expr {
        ty: Type::None,
        kind: ExprKind::None,
    }
}
fn boolean(b: bool) -> Expr {
    Expr {
        ty: Type::Bool,
        kind: ExprKind::Bool(b),
    }
}
fn choose(test: Expr, yes: Expr, no: Expr) -> Expr {
    Expr {
        ty: yes.ty,
        kind: ExprKind::Conditional(Box::new(test), Box::new(yes), Box::new(no)),
    }
}

impl Checker<'_> {
    pub(super) fn cache_config(&self, node: &ast::Expr) -> Option<bool> {
        match node {
            ast::Expr::Attribute(a)
                if a.attr.as_str() == "cache_tag"
                    && self.resolve(&a.value)
                        == Some(Binding::Intrinsic(Intrinsic::SysImplementation)) =>
            {
                Some(false)
            }
            ast::Expr::Attribute(_)
                if self.resolve(node) == Some(Binding::Intrinsic(Intrinsic::SysPycachePrefix)) =>
            {
                Some(true)
            }
            _ => None,
        }
    }

    fn cache_is_none(&self, value: Expr) -> Expr {
        if self.types.payload(value.ty).is_some() {
            Expr {
                ty: Type::Bool,
                kind: ExprKind::Builtin(Builtin::IsNone, vec![value]),
            }
        } else {
            boolean(value.ty == Type::None)
        }
    }

    pub(super) fn default_cache_optimization(&self) -> Expr {
        match self.graph.python_optimization() {
            0 => text(""),
            level => text(&level.to_string()),
        }
    }

    fn optimization_text(&mut self, value: Expr, at: &ast::Expr) -> Result<Expr, String> {
        if let Some(ty) = self.types.payload(value.ty) {
            let test = self.cache_is_none(value.clone());
            let unwrapped = Expr {
                ty,
                kind: ExprKind::Unwrap(Box::new(value)),
            };
            return Ok(choose(
                test,
                self.default_cache_optimization(),
                self.optimization_text(unwrapped, at)?,
            ));
        }
        Ok(match value.ty {
            Type::None => self.default_cache_optimization(),
            Type::Str | Type::StrLiteral(_) | Type::StrEnum(_) => Self::string_value(value),
            Type::Class(_) => self.record_text(value, at)?,
            Type::Int
            | Type::Bool
            | Type::Float
            | Type::Bytes
            | Type::Exception
            | Type::NamedException(_) => Expr {
                ty: Type::Str,
                kind: ExprKind::Builtin(Builtin::Str, vec![value]),
            },
            _ if dataclass_values::can_format(value.ty, self.types, self.classes) => Expr {
                ty: Type::Str,
                kind: ExprKind::Builtin(Builtin::Repr, vec![value]),
            },
            _ => {
                return Err(error(
                    self.source,
                    at,
                    "optimization must support a native string conversion",
                ));
            }
        })
    }

    /// _path_split uses a text separator: bytes fail after __fspath__, even with no tag.
    fn cache_path_text(&mut self, value: Expr, at: &ast::Expr) -> Result<Expr, String> {
        if matches!(value.ty, Type::Str | Type::StrLiteral(_) | Type::StrEnum(_)) {
            return Ok(Self::string_value(value));
        }
        if value.ty == Type::Bytes {
            return Ok(Expr {
                ty: Type::Str,
                kind: ExprKind::Builtin(
                    Builtin::RaiseException,
                    vec![
                        value,
                        Expr {
                            ty: Type::Exception,
                            kind: ExprKind::Exception {
                                kind: exception_kind("TypeError").unwrap(),
                                message: Some(Box::new(text(
                                    "argument should be integer or bytes-like object, not 'str'",
                                ))),
                            },
                        },
                    ],
                ),
            });
        }
        if let Some((slot, test, first, second)) = self.protocol_alternatives(value.ty) {
            let left = self.cache_path_text(first, at)?;
            let right = self.cache_path_text(second, at)?;
            return Ok(Expr {
                ty: Type::Str,
                kind: ExprKind::Scope {
                    body: vec![Statement::Assign(slot, value)],
                    clear: vec![slot],
                    value: Box::new(choose(test, left, right)),
                },
            });
        }
        Err(error(
            self.source,
            at,
            "cache path requires a native str or bytes spelling",
        ))
    }

    pub(super) fn cache_path_call(
        &mut self,
        call: &ast::ExprCall,
        reverse: bool,
    ) -> Result<Expr, String> {
        calls::reject_unpacking(call, self.source)?;
        let names: &[&str] = if reverse {
            &["path"]
        } else {
            &["path", "debug_override", "optimization"]
        };
        let params: Vec<_> = names
            .iter()
            .enumerate()
            .map(|(i, &n)| Parameter {
                name: n.into(),
                ty: Type::None,
                kind: if i == 2 {
                    ParameterKind::KeywordOnly
                } else {
                    ParameterKind::PositionalOrKeyword
                },
                default: (i != 0).then(none),
                sign_only: false,
            })
            .collect();
        let args =
            self.arguments_with(call, &params, None, |checker, _, node| checker.expr(node))?;
        let mut roots = calls::ArgumentScope::default();
        let values: Vec<_> = args
            .values
            .into_iter()
            .map(|v| roots.hold(self, v))
            .collect();
        let field = |i: usize| values[args.order[i]].clone();
        let path = field(0);
        let at = &*call.func;
        let tag = op(Op::Tag, Type::Str, vec![]);
        let path = self.fspath_protocol(path, at, None, None)?;
        let path = self.cache_path_text(path, at)?;
        if reverse {
            return Ok(roots.finish(op(Op::Reverse, Type::Str, vec![tag, path])));
        }
        let debug = field(1);
        let optimization = field(2);
        // Warn, reject conflicts, and run __bool__ before attempting the filesystem protocol.
        let override_text = if debug.ty == Type::None {
            None
        } else {
            let debug_absent = self.cache_is_none(debug.clone());
            let opt_absent = self.cache_is_none(optimization.clone());
            roots.hold(
                self,
                op(
                    Op::Debug,
                    Type::None,
                    vec![debug_absent.clone(), opt_absent],
                ),
            );
            let truth = self.truth_value(debug);
            let chosen = choose(debug_absent.clone(), boolean(false), truth);
            let active = roots.hold(self, chosen);
            Some((debug_absent, choose(active, text(""), text("1"))))
        };
        let normal = self.optimization_text(optimization, at)?;
        let opt = match override_text {
            Some((absent, override_)) => choose(absent, normal, override_),
            None => normal,
        };
        Ok(roots.finish(op(Op::Forward, Type::Str, vec![path, tag, opt])))
    }
}
