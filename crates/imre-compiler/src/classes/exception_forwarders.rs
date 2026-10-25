//! Recognize exception constructors that store their inputs and forward them unchanged.
use super::*;
use ruff_python_ast::visitor::transformer::{self, Transformer};

fn private_field(class: &str, name: &str) -> Option<String> {
    let owner = class.trim_start_matches('_');
    (name.starts_with("__") && !name.ends_with("__") && !owner.is_empty())
        .then(|| format!("_{owner}{name}"))
}

/// Resolve private attributes in the declaring method before subclasses inherit it.
pub(super) fn mangle_fields(class: &str, method: &mut ast::StmtFunctionDef) {
    struct PrivateFields<'a>(&'a str);

    impl Transformer for PrivateFields<'_> {
        fn visit_expr(&self, expr: &mut ast::Expr) {
            if let ast::Expr::Attribute(attr) = expr
                && let Some(name) = private_field(self.0, attr.attr.as_str())
            {
                attr.attr.id = name.into();
            }
            transformer::walk_expr(self, expr);
        }

        fn visit_stmt(&self, stmt: &mut ast::Stmt) {
            if !matches!(stmt, ast::Stmt::ClassDef(_)) {
                transformer::walk_stmt(self, stmt);
            }
        }
    }

    PrivateFields(class).visit_body(&mut method.body);
}

#[derive(Clone)]
pub(crate) struct ForwardingInitializer {
    pub owner: DefinitionId,
    pub receiver: String,
    pub parameters: Vec<(String, Option<crate::ir::Expr>)>,
    pub positional: usize,
    pub variadic: Option<String>,
    pub stores: Vec<(String, String)>,
    pub before_super: usize,
    /// A true flag expands this constructor's positional tail.
    pub arguments: Vec<(String, bool)>,
    pub keywords: Vec<(String, String)>,
}

impl ForwardingInitializer {
    /// Every source operation is either a parameter read, a field write or one lexical super call.
    pub(super) fn collect(
        definition: DefinitionId,
        node: &ast::StmtClassDef,
        parent: Option<&Class>,
        module: &crate::modules::SourceModule,
    ) -> Option<Self> {
        let inherited = parent.and_then(|c| c.forwarder.as_ref());
        let source = &module.source;
        if !node.decorator_list.is_empty()
            || module.bindings.contains_key("super")
            || node
                .body
                .iter()
                .filter_map(ast::Stmt::as_function_def_stmt)
                .any(|method| {
                    !method.decorator_list.is_empty()
                        || matches!(
                            method.name.as_str(),
                            "__new__" | "__setattr__" | "__delattr__" | "__getattribute__"
                        )
                })
            || node
                .body
                .iter()
                .any(|s| matches!(s, ast::Stmt::AnnAssign(_)))
        {
            return None;
        }
        let Some(init) = node
            .body
            .iter()
            .filter_map(ast::Stmt::as_function_def_stmt)
            .find(|f| f.name.as_str() == "__init__")
        else {
            return inherited.cloned();
        };
        let p = &init.parameters;
        if p.iter_non_variadic_params()
            .any(|p| p.name().as_str() == "super")
            || p.vararg
                .as_ref()
                .is_some_and(|p| p.name.as_str() == "super")
        {
            return None;
        }
        if (p.vararg.is_none() && inherited.is_none())
            || p.kwarg.is_some()
            || p.vararg.as_ref().is_some_and(|v| v.annotation.is_some())
            || p.iter_non_variadic_params()
                .next()
                .is_none_or(|p| p.default.is_some())
            || !p.posonlyargs.is_empty()
            || !init.decorator_list.is_empty()
            || init
                .returns
                .as_deref()
                .is_some_and(|r| !matches!(r, ast::Expr::NoneLiteral(_)))
            || parent.is_some() && inherited.is_none()
            || p.iter_non_variadic_params()
                .skip(1)
                .any(|param| param.annotation().is_some())
        {
            return None;
        }
        let receiver = p.iter_non_variadic_params().next()?.name().as_str();
        let positional = (p.args.len() + p.posonlyargs.len()).checked_sub(1)?;
        let parameters: Vec<_> = p
            .iter_non_variadic_params()
            .skip(1)
            .map(|p| {
                Some((
                    p.name().to_string(),
                    p.default
                        .as_deref()
                        .map(|d| crate::frontend::scalar_literal(d, source))
                        .transpose()
                        .ok()?,
                ))
            })
            .collect::<Option<_>>()?;
        let is_parameter = |name: &str| parameters.iter().any(|(n, _)| n == name);
        let variadic = p.vararg.as_ref().map(|p| p.name.to_string());
        let mut stores = Vec::new();
        let mut forwarded = None;
        for line in &init.body {
            match line {
                ast::Stmt::Pass(_) => {}
                ast::Stmt::Expr(e) if e.value.is_string_literal_expr() => {}
                ast::Stmt::Assign(a) => {
                    let [target] = a.targets.as_slice() else {
                        return None;
                    };
                    let field = target.as_attribute_expr()?;
                    let input = a.value.as_name_expr()?.id.as_str();
                    if field.value.as_name_expr()?.id != receiver || !is_parameter(input) {
                        return None;
                    }
                    let name = private_field(node.name.as_str(), field.attr.as_str())
                        .unwrap_or_else(|| field.attr.to_string());
                    stores.push((name, input.to_owned()));
                }
                ast::Stmt::Expr(e) => {
                    let call = e.value.as_call_expr()?;
                    if !super_init(call) || forwarded.is_some() {
                        return None;
                    }
                    let mut arguments = Vec::new();
                    for arg in &call.arguments.args {
                        let (value, spread) = if let ast::Expr::Starred(star) = arg {
                            (&*star.value, true)
                        } else {
                            (arg, false)
                        };
                        let name = value.as_name_expr()?.id.as_str();
                        if spread && variadic.as_deref() != Some(name)
                            || !spread && !is_parameter(name)
                        {
                            return None;
                        }
                        arguments.push((name.to_owned(), spread));
                    }
                    let mut keywords = Vec::new();
                    for keyword in &call.arguments.keywords {
                        let value = keyword.value.as_name_expr()?.id.as_str();
                        if !is_parameter(value) {
                            return None;
                        }
                        keywords.push((keyword.arg.as_ref()?.to_string(), value.to_owned()));
                    }
                    if inherited.is_none() && !keywords.is_empty() {
                        return None;
                    }
                    forwarded = Some((arguments, keywords, stores.len()));
                }
                _ => return None,
            }
        }
        let (arguments, keywords, before_super) = forwarded?;
        Some(Self {
            owner: definition,
            receiver: receiver.to_owned(),
            parameters,
            positional,
            variadic,
            stores,
            before_super,
            arguments,
            keywords,
        })
    }
}
