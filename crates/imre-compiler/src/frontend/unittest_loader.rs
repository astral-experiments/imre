//! TestLoader discovers method names without executing an uncalled test body.

use super::*;
use crate::classes::MethodKind;
use std::collections::{BTreeMap, HashSet};

impl Checker<'_> {
    /// Pass a resolved class's callable descriptors to the ordinary native loader.
    /// The loader instance applies its current prefix and name patterns at runtime.
    pub(super) fn unittest_loader_names(
        &mut self,
        call: &ast::ExprCall,
        loader: usize,
        receiver: &Expr,
    ) -> Result<Option<Expr>, String> {
        let classes = &self.classes.declarations.definitions;
        if self.classes.methods[loader].contains_key("getTestCaseNames")
            || !std::iter::once(&loader)
                .chain(&classes[loader].bases)
                .any(|&id| {
                    let c = &classes[id];
                    let m = &self.graph.modules[c.definition.module];
                    c.name == "TestLoader" && m.bundled && m.name == "unittest"
                })
        {
            return Ok(None);
        }
        let target = match (
            call.arguments.args.as_ref(),
            call.arguments.keywords.as_ref(),
        ) {
            ([arg], []) => arg,
            ([], [kw]) if kw.arg.as_deref() == Some("testCaseClass") => &kw.value,
            _ => {
                return Err(error(
                    self.source,
                    call,
                    "getTestCaseNames requires one testCaseClass",
                ));
            }
        };
        let (owner, attributes) = if let Some(class) = self.local_class(target)? {
            (
                format!(
                    "{}.{}",
                    self.runtime_module_name(self.module),
                    class.qualname
                ),
                class.attributes.clone(),
            )
        } else {
            let id = self.class_reference(target).ok_or_else(|| {
                error(
                    self.source,
                    target,
                    "native getTestCaseNames requires a resolved class",
                )
            })?;
            let class = &classes[id];
            (
                format!(
                    "{}.{}",
                    self.runtime_module_name(class.definition.module),
                    class.name
                ),
                self.fixed_class_attributes(id),
            )
        };
        self.unittest_select_names(
            call,
            loader,
            receiver,
            owner,
            attributes,
            "_getTestCaseNames",
        )
        .map(Some)
    }

    /// Filter a discovered module class with the actual loader before constructing cases.
    pub(super) fn unittest_fixed_names(
        &mut self,
        call: &ast::ExprCall,
        loader: &Expr,
        class: usize,
    ) -> Result<Expr, String> {
        let Type::Class(loader_class) = loader.ty else {
            unreachable!()
        };
        let decl = &self.classes.declarations.definitions[class];
        let owner = format!(
            "{}.{}",
            self.runtime_module_name(decl.definition.module),
            decl.name
        );
        self.unittest_select_names(
            call,
            loader_class,
            loader,
            owner,
            self.fixed_class_attributes(class),
            "_getModuleCaseNames",
        )
    }

    fn unittest_select_names(
        &mut self,
        call: &ast::ExprCall,
        loader: usize,
        receiver: &Expr,
        owner: String,
        attributes: local_classes::Attributes,
        method: &str,
    ) -> Result<Expr, String> {
        let signature = self.classes.methods[loader]
            .get(method)
            .cloned()
            .ok_or_else(|| {
                error(
                    self.source,
                    call,
                    "native TestLoader name filtering is unavailable",
                )
            })?;
        let items = |status| Expr {
            ty: signature.params[2].ty,
            kind: ExprKind::List(
                attributes
                    .iter()
                    .filter(|(_, v)| **v == status)
                    .map(|(n, _)| unpacking::string(n.as_str()))
                    .collect(),
            ),
        };
        let args = vec![
            receiver.clone(),
            unpacking::string(owner),
            items(Some(true)),
            items(None),
        ];
        self.calls.insert(signature.id);
        Ok(self.call_value(
            &signature,
            signature.returns,
            ir::CallArgs::positional(args),
        ))
    }

    /// Resolve the class namespace of an imported module while leaving the suite,
    /// loader settings, and optional load_tests hook as ordinary runtime values.
    pub(super) fn unittest_load_suite(
        &mut self,
        call: &ast::ExprCall,
        method: &str,
        class: usize,
        receiver: &Expr,
    ) -> Result<Option<Expr>, String> {
        let decl = &self.classes.declarations.definitions[class];
        let unittest = &self.graph.modules[decl.definition.module];
        if decl.name != "TestLoader" || !unittest.is_bundled("unittest") {
            return Ok(None);
        }
        if method == "loadTestsFromTestCase" {
            let suite_class = self.unittest_suite_class(call, decl.definition.module)?;
            let suite_ty = Type::Class(suite_class);
            let target = match (
                call.arguments.args.as_ref(),
                call.arguments.keywords.as_ref(),
            ) {
                ([target], []) => target,
                ([], [kw]) if kw.arg.as_deref() == Some("testCaseClass") => &kw.value,
                _ => {
                    return Err(error(
                        self.source,
                        call,
                        "loadTestsFromTestCase requires one testCaseClass",
                    ));
                }
            };
            let local = self.local_class(target)?;
            let base = local
                .and_then(|c| c.base)
                .or_else(|| self.class_reference(target))
                .ok_or_else(|| {
                    error(
                        self.source,
                        target,
                        "native loadTestsFromTestCase requires a resolved class",
                    )
                })?;
            if self
                .classes
                .declarations
                .subclass(self.graph, base, suite_class)
            {
                return Ok(Some(calls::type_error(suite_ty, vec![receiver.clone()],
                    "Test cases should not be derived from TestSuite. Maybe you meant to derive from TestCase?".into())));
            }
            if local.is_some() || !self.unittest_case(base) {
                return Err(error(
                    self.source,
                    target,
                    "native loadTestsFromTestCase requires a fixed TestCase layout",
                ));
            }
            let loader = self.hidden(receiver.ty);
            let suite = self.hidden(suite_ty);
            let mut body = vec![
                Statement::Assign(loader, receiver.clone()),
                Statement::Assign(suite, self.unittest_default_construct(call, suite_class)?),
            ];
            body.extend(self.unittest_add_class_cases(call, base, suite, loader)?);
            return Ok(Some(Expr {
                ty: suite_ty,
                kind: ExprKind::Scope {
                    body,
                    clear: vec![loader, suite],
                    value: Box::new(self.local(suite)),
                },
            }));
        }

        let mut target = match call.arguments.args.as_ref() {
            [] => None,
            [target] => Some(target),
            _ => {
                return Err(error(
                    self.source,
                    call,
                    "loadTestsFromModule requires a module and optional keyword-only pattern",
                ));
            }
        };
        let mut pattern = None;
        for kw in &call.arguments.keywords {
            match kw.arg.as_deref() {
                Some("module") if target.is_none() => target = Some(&kw.value),
                Some("pattern") if pattern.is_none() => pattern = Some(&kw.value),
                _ => {
                    return Err(error(
                        self.source,
                        kw,
                        "loadTestsFromModule requires a module and optional keyword-only pattern",
                    ));
                }
            }
        }
        let target = target
            .ok_or_else(|| error(self.source, call, "loadTestsFromModule requires a module"))?;
        // A parameter or local can shadow an imported module of the same name.
        let mut root = target;
        while let ast::Expr::Attribute(attr) = root {
            root = &attr.value;
        }
        let binding = if root
            .as_name_expr()
            .is_some_and(|n| self.names.contains_key(n.id.as_str()))
        {
            None
        } else {
            self.resolve(target)
        };
        let Some(Binding::Module(module)) = binding else {
            return Err(error(
                self.source,
                target,
                "native loadTestsFromModule requires a resolved source module",
            ));
        };
        self.unittest_suite_from_module(call, module, receiver.clone(), pattern)
            .map(Some)
    }

    fn unittest_suite_class(
        &self,
        call: &ast::ExprCall,
        unittest_module: ModuleId,
    ) -> Result<usize, String> {
        let unittest = &self.graph.modules[unittest_module];
        let Some(Binding::Definition(suite_def)) = unittest.bindings.get("TestSuite") else {
            return Err(error(self.source, call, "native TestSuite is unavailable"));
        };
        let suite_class = *self
            .classes
            .declarations
            .ids
            .get(suite_def)
            .ok_or_else(|| error(self.source, call, "native TestSuite is unavailable"))?;
        Ok(suite_class)
    }

    pub(super) fn unittest_suite_from_module(
        &mut self,
        call: &ast::ExprCall,
        module: ModuleId,
        receiver: Expr,
        pattern: Option<&ast::Expr>,
    ) -> Result<Expr, String> {
        let Type::Class(class) = receiver.ty else {
            unreachable!()
        };
        let unittest_module = self.classes.declarations.definitions[class]
            .definition
            .module;
        let unittest = &self.graph.modules[unittest_module];
        let suite_class = self.unittest_suite_class(call, unittest_module)?;
        let suite_ty = Type::Class(suite_class);
        let unit = &self.graph.modules[module];
        if ["__dir__", "__getattr__"]
            .iter()
            .any(|key| unit.bindings.contains_key(*key))
        {
            return Err(error(
                self.source,
                call,
                "native loadTestsFromModule requires a fixed module namespace",
            ));
        }
        if unit.bindings.contains_key("setUpModule") || unit.bindings.contains_key("tearDownModule")
        {
            return Err(self.object_runtime_export("unittest module fixtures", call));
        }
        let hook = match unit.bindings.get("load_tests") {
            Some(Binding::Definition(def)) => Some((
                *def,
                self.functions.get(def).cloned().ok_or_else(|| {
                    error(
                        self.source,
                        call,
                        "native load_tests must be a source function",
                    )
                })?,
            )),
            Some(_) => {
                return Err(error(
                    self.source,
                    call,
                    "native load_tests requires a resolved function",
                ));
            }
            None => None,
        };
        let loader = self.hidden(receiver.ty);
        let suite = self.hidden(suite_ty);
        let mut clear = vec![loader, suite];
        let mut body = vec![Statement::Assign(loader, receiver)];
        let pattern = if let Some(pattern) = pattern {
            let value = self.expr(pattern)?;
            let slot = self.hidden(value.ty);
            body.push(Statement::Assign(slot, value));
            clear.push(slot);
            self.local(slot)
        } else {
            Expr::none()
        };
        body.push(Statement::Assign(
            suite,
            self.unittest_default_construct(call, suite_class)?,
        ));
        body.extend(self.unittest_module_cases(call, module, suite, loader)?);
        if let Some((def, hook)) = hook {
            let loaded = self.unittest_load_hook(
                call,
                &hook,
                [self.local(loader), self.local(suite), pattern],
                &mut body,
                &mut clear,
            )?;
            let statement = if loaded.ty == suite_ty {
                Statement::Assign(suite, loaded)
            } else if matches!(&self.graph.modules[def.module].parsed.syntax().body[def.statement],
                               ast::Stmt::FunctionDef(f) if matches!(f.body.as_slice(), [ast::Stmt::Raise(_)]))
            {
                // An always-raising function with no return annotation infers None.
                Statement::Expr(loaded)
            } else {
                return Err(error(
                    self.source,
                    call,
                    "native load_tests hook must return a TestSuite",
                ));
            };
            let caught = self.hidden(Type::Exception);
            let Some(Binding::Definition(failed)) = unittest.bindings.get("_failedModuleTests")
            else {
                return Err(error(
                    self.source,
                    call,
                    "missing native load_tests failure suite",
                ));
            };
            let failed = self.functions.get(failed).cloned().ok_or_else(|| {
                error(self.source, call, "missing native load_tests failure suite")
            })?;
            self.calls.insert(failed.id);
            let fallback = self.call_value(
                &failed,
                suite_ty,
                ir::CallArgs::positional(vec![
                    self.local(loader),
                    unpacking::string(&unit.name),
                    self.local(caught),
                ]),
            );
            body.push(Statement::Try {
                body: vec![statement],
                handlers: vec![ir::ExceptHandler {
                    matcher: crate::ir::ExceptionMatcher::Categories(vec![
                        exception_kind("Exception").unwrap(),
                    ]),
                    name: Some(caught),
                    body: vec![Statement::Assign(suite, fallback)],
                }],
                orelse: vec![],
                finalbody: vec![],
            });
        }
        let value = self.local(suite);
        Ok(Expr {
            ty: value.ty,
            kind: ExprKind::Scope {
                body,
                clear,
                value: Box::new(value),
            },
        })
    }

    /// Bind the real hook with a normal source call, including inference and *args
    /// packing, using the same objects on which discovery took place.
    fn unittest_load_hook(
        &mut self,
        call: &ast::ExprCall,
        signature: &Signature,
        values: [Expr; 3],
        body: &mut Vec<Statement>,
        clear: &mut Vec<usize>,
    ) -> Result<Expr, String> {
        let mut args = Vec::new();
        let mut locals = Vec::new();
        for value in values {
            let slot = self.hidden(value.ty);
            body.push(Statement::Assign(slot, value));
            let name = format!("@load_tests_arg{slot}");
            self.names.insert(name.clone(), slot);
            self.assigned.insert(slot);
            args.push(ast::Expr::Name(ast::ExprName {
                node_index: ast::AtomicNodeIndex::NONE,
                range: call.range(),
                id: name.clone().into(),
                ctx: ast::ExprContext::Load,
            }));
            locals.push((name, slot));
        }
        let mut invocation = call.clone();
        invocation.arguments.args = args.into();
        invocation.arguments.keywords = Default::default();
        let result = self.source_call(&invocation, signature, None, 0);
        for (name, slot) in locals {
            self.names.remove(&name);
            self.assigned.remove(&slot);
            clear.push(slot);
        }
        result
    }

    /// Gather inherited callable and shadowed names from original declarations,
    /// even when reachable compilation prunes the method bodies.
    pub(super) fn fixed_class_attributes(&self, id: usize) -> local_classes::Attributes {
        let classes = &self.classes.declarations.definitions;
        let target = &classes[id];
        let mut attributes = BTreeMap::new();
        // The callable table can omit an uncalled method in reachable mode.
        // Collect bindings from original declarations in base-to-derived order.
        for &index in target.bases.iter().rev().chain(std::iter::once(&id)) {
            let class = &classes[index];
            let mut names = HashSet::new();
            for stmt in &class.node.body {
                let (name, callable) = match stmt {
                    ast::Stmt::FunctionDef(method) => {
                        let kind = crate::classes::method_kind(
                            self.graph,
                            class.definition.module,
                            method,
                            &names,
                        );
                        (
                            method.name.as_str(),
                            kind.ok().map(|k| {
                                matches!(
                                    k,
                                    MethodKind::Instance | MethodKind::Class | MethodKind::Static
                                )
                            }),
                        )
                    }
                    ast::Stmt::Assign(a) if a.targets.len() == 1 => {
                        let Some(n) = a.targets[0].as_name_expr() else {
                            continue;
                        };
                        let name = n.id.as_str();
                        let callable = if class.exception_aliases.contains_key(name) {
                            Some(true)
                        } else if class.static_bindings.contains_key(name)
                            || crate::classes::property_accessors(
                                self.graph,
                                class.definition.module,
                                &a.value,
                            )
                            .is_some()
                        {
                            Some(false)
                        } else {
                            None
                        };
                        (name, callable)
                    }
                    ast::Stmt::AnnAssign(a) if a.value.is_some() => {
                        let Some(n) = a.target.as_name_expr() else {
                            continue;
                        };
                        (n.id.as_str(), Some(false))
                    }
                    _ => continue,
                };
                names.insert(name.to_owned());
                attributes.insert(name.to_owned(), callable);
            }
        }
        attributes
    }
}
