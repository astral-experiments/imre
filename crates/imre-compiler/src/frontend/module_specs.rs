//! Closed ModuleSpec records retain the concrete loader and loader state.
use super::*;

const NAME: usize = 0;
const LOADER: usize = 1;
const ORIGIN: usize = 2;
const STATE: usize = 3;
const LOCATIONS: usize = 4;
const UNINITIALIZED: usize = 5;
const LOCATION: usize = 6;
const CACHED: usize = 7;

fn none() -> Expr {
    Expr {
        ty: Type::None,
        kind: ExprKind::None,
    }
}

fn is_none(value: Expr) -> Expr {
    Expr {
        ty: Type::Bool,
        kind: ExprKind::Builtin(Builtin::IsNone, vec![value]),
    }
}

fn field(object: &Expr, index: usize, types: &[Type]) -> Expr {
    Expr {
        ty: types[index],
        kind: ExprKind::Field(Box::new(object.clone()), index),
    }
}

impl Checker<'_> {
    /// Evaluate the RHS and target before the has_location descriptor invokes bool().
    pub(super) fn set_module_spec_location(
        &mut self,
        attr: &ast::ExprAttribute,
        receiver: Expr,
        value: Expr,
    ) -> Result<Statement, String> {
        let (rhs, object) = (self.hidden(value.ty), self.hidden(receiver.ty));
        let (owner, field, _) = self.writable_field_value(attr, self.local(object))?;
        let truth = self.truth_value(self.local(rhs));
        Ok(Statement::Scope {
            body: vec![
                Statement::Assign(rhs, value),
                Statement::Assign(object, receiver),
                Statement::SetField(owner, field, truth),
            ],
            clear: vec![rhs, object],
        })
    }

    pub(super) fn module_spec(&mut self, call: &ast::ExprCall) -> Result<Expr, String> {
        let params = ["name", "loader", "origin", "loader_state", "is_package"].map(|name| {
            let required = matches!(name, "name" | "loader");
            Parameter {
                name: name.into(),
                ty: Type::None,
                kind: if required {
                    ParameterKind::PositionalOrKeyword
                } else {
                    ParameterKind::KeywordOnly
                },
                default: (!required).then(none),
                sign_only: false,
            }
        });
        let args = self.arguments_with(call, &params, None, |checker, index, arg| {
            if index == NAME {
                checker.expr_expected(arg, Type::Str)
            } else {
                checker.expr(arg)
            }
        })?;
        // Bind before evaluating the truth protocol or allocating either fresh list.
        let mut roots = calls::ArgumentScope::default();
        let values: Vec<_> = args
            .values
            .into_iter()
            .map(|v| roots.hold(self, v))
            .collect();
        let get = |i: usize| values[args.order[i]].clone();
        let optional_str = self.types.optional(Type::Str);
        let list = self.types.list(Type::Str);
        let optional_list = self.types.optional(list);
        let empty = || Expr {
            ty: list,
            kind: ExprKind::List(vec![]),
        };
        let package = self.truth_value(get(4));
        let locations = Expr {
            ty: optional_list,
            kind: ExprKind::Conditional(
                Box::new(package),
                Box::new(self.coerce(empty(), optional_list, call)?),
                Box::new(self.coerce(none(), optional_list, call)?),
            ),
        };
        let fields = vec![
            get(NAME),
            get(LOADER),
            self.coerce(get(ORIGIN), optional_str, call)?,
            get(STATE),
            locations,
            empty(),
            Expr {
                ty: Type::Bool,
                kind: ExprKind::Bool(false),
            },
            self.coerce(none(), optional_str, call)?,
        ];
        Ok(roots.finish(Expr {
            ty: self
                .types
                .module_spec(fields.iter().map(|v| v.ty).collect()),
            kind: ExprKind::Tuple(fields),
        }))
    }

    pub(super) fn module_spec_field(
        &self,
        id: usize,
        attr: &ast::ExprAttribute,
    ) -> Result<(usize, Type), String> {
        let index = match attr.attr.as_str() {
            "name" => NAME,
            "loader" => LOADER,
            "origin" => ORIGIN,
            "loader_state" => STATE,
            "submodule_search_locations" => LOCATIONS,
            "_uninitialized_submodules" => UNINITIALIZED,
            "has_location" | "_set_fileattr" => LOCATION,
            "cached" | "_cached" => CACHED,
            "parent" => return Err(error(self.source, attr, "ModuleSpec.parent is read-only")),
            _ => return Err(error(self.source, attr, "unsupported ModuleSpec attribute")),
        };
        Ok((index, self.types.module_spec_fields(id)[index]))
    }

    pub(super) fn module_spec_attribute(
        &mut self,
        receiver: Expr,
        id: usize,
        attr: &ast::ExprAttribute,
    ) -> Result<Expr, String> {
        let types = self.types.module_spec_fields(id);
        let mut roots = calls::ArgumentScope::default();
        let obj = roots.hold(self, receiver);
        let value = match attr.attr.as_str() {
            "parent" => {
                let name = field(&obj, NAME, &types);
                let prefix = Expr {
                    ty: Type::Str,
                    kind: ExprKind::Builtin(Builtin::ModuleParentPrefix, vec![name.clone()]),
                };
                Expr {
                    ty: Type::Str,
                    kind: ExprKind::Conditional(
                        Box::new(is_none(field(&obj, LOCATIONS, &types))),
                        Box::new(prefix),
                        Box::new(name),
                    ),
                }
            }
            "cached" => {
                let cached = field(&obj, CACHED, &types);
                let origin = field(&obj, ORIGIN, &types);
                let loc = field(&obj, LOCATION, &types);
                let from_origin = Expr {
                    ty: cached.ty,
                    kind: ExprKind::Builtin(
                        Builtin::ModuleCacheFromOrigin,
                        vec![origin, self.default_cache_optimization()],
                    ),
                };
                let populate = Statement::If(
                    Expr {
                        ty: Type::Bool,
                        kind: ExprKind::Boolean(true, vec![is_none(cached.clone()), loc]),
                    },
                    vec![Statement::SetField(obj, CACHED, from_origin)],
                    vec![],
                );
                Expr {
                    ty: cached.ty,
                    kind: ExprKind::Scope {
                        body: vec![populate],
                        value: Box::new(cached),
                        clear: vec![],
                    },
                }
            }
            _ => {
                let (index, _) = self.module_spec_field(id, attr)?;
                field(&obj, index, &types)
            }
        };
        Ok(roots.finish(value))
    }
}
