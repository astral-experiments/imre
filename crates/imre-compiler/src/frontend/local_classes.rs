//! Name inspection for unconditional local classes whose creation runs no user code.
use super::*;
use std::collections::BTreeMap;

pub(super) type Attributes = BTreeMap<String, Option<bool>>;
pub(super) struct Binding {
    pub(super) site: TextRange,
    pub(super) value: Option<LocalClass>,
}
pub(super) struct LocalClass {
    pub(super) name: String,
    pub(super) qualname: String,
    pub(super) attributes: Attributes,
    pub(super) base: Option<usize>,
    pub(super) abstract_methods: HashSet<String>,
}

/// Recover the lexical owner from source even when an inherited method is called on a subclass.
fn qualified_name(body: &[ast::Stmt], site: TextRange) -> Option<String> {
    for stmt in body {
        if !stmt.range().contains_range(site) {
            continue;
        }
        return match stmt {
            ast::Stmt::ClassDef(c) if c.range == site => Some(c.name.to_string()),
            ast::Stmt::ClassDef(c) => {
                Some(format!("{}.{}", c.name, qualified_name(&c.body, site)?))
            }
            ast::Stmt::FunctionDef(f) => Some(format!(
                "{}.<locals>.{}",
                f.name,
                qualified_name(&f.body, site)?
            )),
            _ => None,
        };
    }
    None
}

impl Checker<'_> {
    pub(super) fn local_class(&self, value: &ast::Expr) -> Result<Option<&LocalClass>, String> {
        let Some(binding) = value
            .as_name_expr()
            .and_then(|n| self.local_classes.get(n.id.as_str()))
        else {
            return Ok(None);
        };
        binding.value.as_ref().map(Some).ok_or_else(|| {
            error(
                self.source,
                value,
                "local class is referenced before its definition",
            )
        })
    }

    pub(super) fn runtime_module_name(&self, module: ModuleId) -> &str {
        if module == self.graph.entry {
            "__main__"
        } else {
            &self.graph.modules[module].name
        }
    }

    /// Record method bindings without calling them. Eager defaults, decorators,
    /// arbitrary bases and executable class statements must not be silently elided.
    pub(super) fn define_local_class(&mut self, node: &ast::StmtClassDef) -> Result<(), String> {
        if crate::classes::defines_pickle_reducer(node) {
            return Err(self.object_runtime_export("pickle reducers", node));
        }
        if node.type_params.is_some() {
            self.graph.needs_object_runtime.set(true);
            return Err(error(
                self.source,
                node,
                "class type parameters require the object runtime",
            ));
        }
        let name = node.name.as_str();
        for decorator in &node.decorator_list {
            if let Some(crate::modules::Binding::Intrinsic(Intrinsic::ObjectRuntimeExport(name))) =
                self.resolve(&decorator.expression)
            {
                return Err(self.object_runtime_export(name, decorator));
            }
        }
        if !self
            .local_classes
            .get(name)
            .is_some_and(|b| b.site == node.range && b.value.is_none())
        {
            return Err(error(
                self.source,
                node,
                "local class declarations must appear directly in the enclosing function's suite",
            ));
        }
        if !node.decorator_list.is_empty()
            || node
                .arguments
                .as_ref()
                .is_some_and(|a| !a.keywords.is_empty() || a.args.len() > 1)
        {
            return Err(error(
                self.source,
                node,
                "local class metadata requires one fixed base and no decorators, metaclass or type parameters",
            ));
        }
        let mut fixed_base = None;
        let mut abstract_methods = HashSet::new();
        let mut attributes =
            if let Some(base) = node.arguments.as_ref().and_then(|a| a.args.first()) {
                if let Some(parent) = self.local_class(base)? {
                    fixed_base = parent.base;
                    abstract_methods.clone_from(&parent.abstract_methods);
                    parent.attributes.clone()
                } else if self.unshadowed_builtin(base, "int") {
                    inspect::INT_ATTRIBUTES
                        .iter()
                        .map(|&(name, callable)| (name.to_owned(), Some(callable)))
                        .collect()
                } else if let Some(id) = self.class_reference(base)
                    && let class = &self.classes.declarations.definitions[id]
                    && let module = &self.graph.modules[class.definition.module]
                    && ((matches!(class.name.as_str(), "TestSuite" | "TestCase")
                        && module.is_bundled("unittest"))
                        || (class.abc && !class.abstract_methods.is_empty()))
                {
                    fixed_base = Some(id);
                    if !class.abstract_methods.is_empty() {
                        abstract_methods.clone_from(&class.abstract_methods);
                    }
                    self.fixed_class_attributes(id)
                } else {
                    if self.graph.is_protocol_base(self.module, base) {
                        return Err(self.object_runtime_export("typing.Protocol", base));
                    }
                    if let Some(crate::modules::Binding::Intrinsic(kind)) = self.resolve(base)
                        && kind.is_subclassable_calendar()
                    {
                        return Err(self.object_runtime_export("calendar subclasses", base));
                    }
                    return Err(error(
                        self.source,
                        base,
                        "local class base requires supported native class metadata",
                    ));
                }
            } else {
                Attributes::new()
            };
        for stmt in &node.body {
            match stmt {
                ast::Stmt::Pass(_) => {}
                ast::Stmt::Expr(e) if matches!(&*e.value, ast::Expr::StringLiteral(_)) => {}
                ast::Stmt::FunctionDef(f)
                    if f.decorator_list.is_empty()
                        && f.type_params.is_none()
                        && (!f.name.starts_with("__")
                            || matches!(f.name.as_str(), "__enter__" | "__exit__"))
                        && f.parameters
                            .iter_non_variadic_params()
                            .all(|p| p.default.is_none()) =>
                {
                    abstract_methods.remove(f.name.as_str());
                    attributes.insert(f.name.to_string(), Some(true));
                }
                ast::Stmt::Assign(a)
                    if let [ast::Expr::Name(n)] = a.targets.as_slice()
                        && ((matches!(n.id.as_str(), "__enter__" | "__exit__")
                            && matches!(&*a.value, ast::Expr::NoneLiteral(_)))
                            || (n.id == "__slots__"
                                && matches!(&*a.value, ast::Expr::Tuple(t) if t.elts.is_empty()))) =>
                {
                    abstract_methods.remove(n.id.as_str());
                    attributes.insert(n.id.to_string(), Some(false));
                }
                ast::Stmt::Assign(a)
                    if let [ast::Expr::Name(n)] = a.targets.as_slice()
                        && !n.id.starts_with("__")
                        && matches!(
                            &*a.value,
                            ast::Expr::StringLiteral(_)
                                | ast::Expr::NumberLiteral(_)
                                | ast::Expr::BooleanLiteral(_)
                                | ast::Expr::NoneLiteral(_)
                        ) =>
                {
                    abstract_methods.remove(n.id.as_str());
                    attributes.insert(n.id.to_string(), Some(false));
                }
                _ => {
                    return Err(error(
                        self.source,
                        stmt,
                        "local class body requires plain methods without defaults or inert class attributes",
                    ));
                }
            }
        }
        let qualname = qualified_name(
            &self.graph.modules[self.module].parsed.syntax().body,
            node.range,
        )
        .ok_or_else(|| {
            error(
                self.source,
                node,
                "cannot determine the lexical name of this local class",
            )
        })?;
        self.local_classes.get_mut(name).unwrap().value = Some(LocalClass {
            name: name.to_owned(),
            qualname,
            attributes,
            base: fixed_base,
            abstract_methods,
        });
        Ok(())
    }
}
