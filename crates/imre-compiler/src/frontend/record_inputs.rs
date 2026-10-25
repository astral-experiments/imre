//! Recognize a frozen dataclass which normalizes a literal mapping in __post_init__.

use super::*;

fn self_field<'a>(expr: &'a ast::Expr, this: &str) -> Option<&'a str> {
    if let ast::Expr::Attribute(field) = expr
        && matches!(&*field.value, ast::Expr::Name(n) if n.id.as_str() == this)
    {
        Some(field.attr.as_str())
    } else {
        None
    }
}

impl Checker<'_> {
    /// A Python constructor can accept a literal mapping when its first and
    /// only post-init action wraps that field in a generated, side-effect-free
    /// dataclass. Store the normalized value directly in the fixed record.
    pub(super) fn normalized_record_input(
        &self,
        class: usize,
    ) -> Option<(String, usize, ast::ExprCall)> {
        let owner = &self.classes.declarations.definitions[class];
        if !owner.generated_initializer || !owner.dataclass?.frozen {
            return None;
        }
        let (i, post) = owner
            .methods
            .iter()
            .enumerate()
            .find(|(_, m)| m.name.as_str() == "__post_init__")?;
        let this = post
            .parameters
            .iter_non_variadic_params()
            .next()?
            .name()
            .as_str();
        let [ast::Stmt::If(branch)] = post.body.as_slice() else {
            return None;
        };
        if !branch.elif_else_clauses.is_empty() {
            return None;
        }
        let ast::Expr::Call(test) = &*branch.test else {
            return None;
        };
        let module = owner.method_origins[i].module;
        let builtin = |expr: &ast::Expr, name: &str| {
            this != name
                && matches!(expr,ast::Expr::Name(n) if n.id.as_str()==name)
                && self.graph.resolve(module, expr).is_none()
        };
        let [value, class_spec] = test.arguments.args.as_ref() else {
            return None;
        };
        let field = self_field(value, this)?;
        if !builtin(&test.func, "isinstance")
            || !test.arguments.keywords.is_empty()
            || !builtin(class_spec, "dict")
        {
            return None;
        }
        let [ast::Stmt::Expr(stmt)] = branch.body.as_slice() else {
            return None;
        };
        let ast::Expr::Call(set) = &*stmt.value else {
            return None;
        };
        let ast::Expr::Attribute(setter) = &*set.func else {
            return None;
        };
        if setter.attr.as_str() != "__setattr__"
            || !builtin(&setter.value, "object")
            || !set.arguments.keywords.is_empty()
        {
            return None;
        }
        let [
            ast::Expr::Name(receiver),
            ast::Expr::StringLiteral(name),
            ast::Expr::Call(wrap),
        ] = set.arguments.args.as_ref()
        else {
            return None;
        };
        if receiver.id.as_str() != this
            || name.value.to_str() != field
            || !wrap.arguments.args.is_empty()
        {
            return None;
        }
        let [kw] = wrap.arguments.keywords.as_ref() else {
            return None;
        };
        if kw.arg.is_none() || self_field(&kw.value, this) != Some(field) {
            return None;
        }
        let Binding::Definition(target) = self.graph.resolve(module, &wrap.func)? else {
            return None;
        };
        let &index = self.classes.declarations.ids.get(&target)?;
        let wrapper = &self.classes.declarations.definitions[index];
        if !wrapper.generated_initializer
            || self.classes.methods[index].contains_key("__post_init__")
            || wrapper
                .fields
                .iter()
                .any(|f| matches!(f.default, Some(crate::classes::FieldDefault::Record(_))))
        {
            return None;
        }
        let offset = owner.fields.iter().position(|f| f.name == field)?;
        (self.classes.fields[class][offset] == Type::Class(index))
            .then(|| (field.to_owned(), index, wrap.clone()))
    }
}

/// A closed `**kwargs` dataclass copy consumes precisely the caller's fixed
/// keyword names. No general variadic dictionary escapes into native code.
pub(super) fn copies_keywords(
    method: &ast::StmtFunctionDef,
    graph: &SourceGraph,
    module: usize,
) -> bool {
    let parameters = &method.parameters;
    let Some(kwargs) = &parameters.kwarg else {
        return false;
    };
    if !method.decorator_list.is_empty()
        || method.is_async
        || parameters.vararg.is_some()
        || parameters.iter_non_variadic_params().count() != 1
        || method
            .returns
            .as_deref()
            .and_then(|r| graph.resolve_type(module, r))
            != Some(Binding::Intrinsic(Intrinsic::SelfType))
    {
        return false;
    }
    let receiver = parameters
        .iter_non_variadic_params()
        .next()
        .unwrap()
        .name()
        .as_str();
    let is_name =
        |expr: &ast::Expr, name: &str| matches!(expr, ast::Expr::Name(n) if n.id.as_str() == name);
    let body = match method.body.as_slice() {
        [ast::Stmt::Expr(s), rest @ ..] if matches!(&*s.value, ast::Expr::StringLiteral(_)) => rest,
        body => body,
    };
    let [ast::Stmt::Return(ret)] = body else {
        return false;
    };
    let Some(ast::Expr::Call(call)) = ret.value.as_deref() else {
        return false;
    };
    let ast::Expr::Call(ty) = &*call.func else {
        return false;
    };
    let [v] = ty.arguments.args.as_ref() else {
        return false;
    };
    if !is_name(&ty.func, "type")
        || graph.resolve(module, &ty.func).is_some()
        || !is_name(v, receiver)
        || !ty.arguments.keywords.is_empty()
        || !call.arguments.args.is_empty()
    {
        return false;
    }
    let [kw] = call.arguments.keywords.as_ref() else {
        return false;
    };
    let ast::Expr::Dict(d) = &kw.value else {
        return false;
    };
    let [base, updates] = d.items.as_slice() else {
        return false;
    };
    if kw.arg.is_some()
        || base.key.is_some()
        || updates.key.is_some()
        || !is_name(&updates.value, kwargs.name.as_str())
    {
        return false;
    }
    let ast::Expr::Call(asdict) = &base.value else {
        return false;
    };
    graph.resolve(module, &asdict.func) == Some(Binding::Intrinsic(Intrinsic::AsDict))
        && matches!(asdict.arguments.args.as_ref(), [arg] if is_name(arg, receiver))
        && asdict.arguments.keywords.is_empty()
}

impl Checker<'_> {
    /// Retain receiver and each supplied override in Python call order before
    /// reading fields, then call the concrete dataclass's normal initializer.
    pub(super) fn copy_with_keywords(
        &mut self,
        class: usize,
        receiver: Expr,
        keywords: &[ast::Keyword],
        site: &impl Ranged,
    ) -> Result<Expr, String> {
        let declaration = &self.classes.declarations.definitions[class];
        if !declaration.generated_initializer
            || !declaration.dataclass.is_some_and(|d| d.frozen)
            || !self.classes.fields[class].iter().all(|&ty| {
                matches!(
                    self.types.payload(ty).unwrap_or(ty),
                    Type::None
                        | Type::Int
                        | Type::Bool
                        | Type::Float
                        | Type::Str
                        | Type::Bytes
                        | Type::StrLiteral(_)
                )
            })
        {
            return Err(error(
                self.source,
                site,
                "variadic asdict copies require a frozen scalar dataclass",
            ));
        }
        let source = self.hidden(receiver.ty);
        let mut body = vec![Statement::Assign(source, receiver)];
        let mut clear = vec![source];
        let signature = &self.classes.methods[class]["__init__"];
        let mut values = signature.params[1..]
            .iter()
            .map(|p| {
                let index = declaration
                    .fields
                    .iter()
                    .position(|f| f.name == p.name)
                    .unwrap();
                Expr {
                    ty: p.ty,
                    kind: ExprKind::Field(Box::new(self.local(source)), index),
                }
            })
            .collect::<Vec<_>>();
        for keyword in keywords {
            let Some(name) = &keyword.arg else {
                return Err(error(
                    self.source,
                    keyword,
                    "dataclass copies require static keywords",
                ));
            };
            let i = signature.params[1..]
                .iter()
                .position(|p| p.name == name.as_str())
                .ok_or_else(|| {
                    error(
                        self.source,
                        keyword,
                        "unknown field in asdict constructor expansion",
                    )
                })?;
            let arg = self.expr_expected(&keyword.value, values[i].ty)?;
            let slot = self.hidden(arg.ty);
            body.push(Statement::Assign(slot, arg));
            clear.push(slot);
            values[i] = self.local(slot);
        }
        let ty = Type::Class(class);
        self.calls.insert(signature.id);
        let value = Expr {
            ty,
            kind: ExprKind::Construct {
                initializer: signature.id,
                args: ir::CallArgs::positional(values),
            },
        };
        clear.reverse();
        Ok(Expr {
            ty,
            kind: ExprKind::Scope {
                body,
                value: Box::new(value),
                clear,
            },
        })
    }
}
