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
        let owner = unpacking::string(owner);
        let signature = self.classes.methods[loader]
            .get("_getTestCaseNames")
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
        let args = vec![receiver.clone(), owner, items(Some(true)), items(None)];
        Ok(Some(self.call_value(
            &signature,
            signature.returns,
            ir::CallArgs::positional(args),
        )))
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
