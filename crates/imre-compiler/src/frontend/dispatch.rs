//! Dispatch through a fixed-layout base when a program admits several concrete records.

use super::*;

impl ClassInfo<'_> {
    /// Concrete layouts whose objects can inhabit a slot typed as the base class.
    pub(super) fn concrete_layouts(&self, base: usize) -> Vec<usize> {
        self.declarations
            .definitions
            .iter()
            .enumerate()
            .filter(|(id, class)| {
                (*id == base || class.bases.contains(&base)) && class.abstract_methods.is_empty()
            })
            .map(|(id, _)| id)
            .collect()
    }

    /// A generated dataclass repr shadows an inherited override even though the
    /// generated method has no source signature in the method table.
    pub(super) fn has_repr_override(&self, id: usize) -> bool {
        self.repr_override(id).is_some()
    }

    pub(super) fn repr_override(&self, id: usize) -> Option<(DefinitionId, &ast::StmtFunctionDef)> {
        for &owner in std::iter::once(&id).chain(&self.declarations.definitions[id].bases) {
            let class = &self.declarations.definitions[owner];
            if let Some(method) = class.node.body.iter().find_map(|stmt| match stmt {
                ast::Stmt::FunctionDef(f) if f.name.as_str() == "__repr__" => Some(f),
                _ => None,
            }) {
                return Some((class.definition, method));
            }
            if class.dataclass.is_some_and(|options| options.repr) {
                return None;
            }
        }
        None
    }

    pub(super) fn missing_key_reprs(
        &self,
        keys: HashSet<Type>,
        graph: &SourceGraph,
        types: &Types,
        owners: &mut HashSet<DefinitionId>,
    ) {
        let mut pending: Vec<_> = keys.into_iter().collect();
        let mut seen = HashSet::new();
        while let Some(ty) = pending.pop() {
            if !seen.insert(ty) {
                continue;
            }
            if let Type::Class(id) = ty {
                for layout in self.concrete_layouts(id) {
                    let class = &self.declarations.definitions[layout];
                    if let Some((owner, method)) = self.repr_override(layout) {
                        if !method.is_async && !graph.includes_method(owner.module, method) {
                            owners.insert(owner);
                        }
                    } else if class.dataclass.is_some_and(|d| d.repr) {
                        pending.extend(
                            class
                                .fields
                                .iter()
                                .zip(&self.fields[layout])
                                .filter_map(|(field, &ty)| field.repr.then_some(ty)),
                        );
                    }
                }
            } else {
                pending.extend(types.children(ty));
            }
        }
    }

    pub(super) fn has_concrete_subclasses(&self, class: usize) -> bool {
        self.declarations
            .definitions
            .iter()
            .any(|c| c.abstract_methods.is_empty() && c.bases.contains(&class))
    }

    /// A concrete Mapping subclass supplies an iterator omitted by its abstract base.
    pub(super) fn has_record_iterator(&self, class: usize) -> bool {
        self.methods[class].contains_key("__iter__")
            || (self.declarations.definitions[class]
                .abstract_methods
                .contains("__iter__")
                && self.has_concrete_subclasses(class))
    }
}

impl Checker<'_> {
    /// Evaluates the receiver once. Only the selected branch evaluates arguments
    /// or default factories. The record header already carries its concrete layout.
    pub(super) fn dispatch_record(
        &mut self,
        base: usize,
        receiver: Expr,
        at: &impl Ranged,
        mut invoke: impl FnMut(&mut Self, usize, Expr) -> Result<Expr, String>,
    ) -> Result<Expr, String> {
        let candidates = self.classes.concrete_layouts(base);
        let slot = self.hidden(Type::Class(base));
        let mut alternatives = Vec::with_capacity(candidates.len());
        for class in candidates {
            let view = Expr {
                ty: Type::Class(class),
                kind: ExprKind::Builtin(Builtin::RecordView, vec![self.local(slot)]),
            };
            alternatives.push((class, invoke(self, class, view)?));
        }
        // An override can return one member of another implementation's declared
        // union. Find that layout before folding so class declaration order does
        // not decide whether, for example, None and a record can coexist.
        let union_result = alternatives.iter().map(|(_, expr)| expr.ty).find(|&ty| {
            matches!(ty, Type::Optional(_) | Type::Union(_))
                && alternatives
                    .iter()
                    .all(|(_, expr)| self.types.join(expr.ty, ty) == Some(ty))
        });
        let Some((_, mut value)) = alternatives.pop() else {
            return Err(error(
                self.source,
                at,
                "abstract record has no concrete implementation",
            ));
        };
        while let Some((class, mut next)) = alternatives.pop() {
            if next.ty != value.ty {
                let ty = if let Some(ty) = union_result {
                    ty
                } else if let Some(abi) = self.types.common_method_signature(next.ty, value.ty) {
                    abi
                } else if matches!((next.ty, value.ty), (Type::Class(a), Type::Class(b)) if
                    (a == base || self.classes.declarations.definitions[a].bases.contains(&base)) &&
                    (b == base || self.classes.declarations.definitions[b].bases.contains(&base)))
                {
                    Type::Class(base)
                } else {
                    return Err(error(
                        self.source,
                        at,
                        "record overrides must have a common result layout",
                    ));
                };
                next = self.coerce(next, ty, at)?;
                value = self.coerce(value, ty, at)?;
            }
            let test = Expr {
                ty: Type::Bool,
                kind: ExprKind::Builtin(Builtin::ExactRecord(class), vec![self.local(slot)]),
            };
            value = Expr {
                ty: value.ty,
                kind: ExprKind::Conditional(Box::new(test), Box::new(next), Box::new(value)),
            };
        }
        Ok(Expr {
            ty: value.ty,
            kind: ExprKind::Scope {
                body: vec![Statement::Assign(slot, receiver)],
                value: Box::new(value),
                clear: vec![slot],
            },
        })
    }

    /// Invoke the concrete slot, including overrides on a fixed-layout subclass.
    pub(super) fn record_primitive(
        &mut self,
        receiver: Expr,
        name: &str,
        arguments: &[Expr],
        at: &impl Ranged,
    ) -> Result<Expr, String> {
        let Type::Class(class) = receiver.ty else {
            unreachable!("record primitives require a fixed-layout receiver")
        };
        let direct = |checker: &mut Self, class: usize, receiver: Expr| {
            checker.record_slot(class, receiver, name, arguments, at)
        };
        if self.classes.has_concrete_subclasses(class) {
            self.dispatch_record(class, receiver, at, direct)
        } else {
            direct(self, class, receiver)
        }
    }

    /// Lower a primitive after its concrete receiver has been selected.
    pub(super) fn record_slot(
        &mut self,
        class: usize,
        receiver: Expr,
        name: &str,
        arguments: &[Expr],
        at: &impl Ranged,
    ) -> Result<Expr, String> {
        self.record_slot_at(class, receiver, name, arguments, at, ir::CallSite::None)
    }

    pub(super) fn record_slot_at(
        &mut self,
        class: usize,
        receiver: Expr,
        name: &str,
        arguments: &[Expr],
        at: &impl Ranged,
        site: ir::CallSite,
    ) -> Result<Expr, String> {
        let method = self.classes.methods[class]
            .get(name)
            .ok_or_else(|| error(self.source, at, "missing record primitive"))?
            .clone();
        if method.params.len() != arguments.len() + 1
            || (matches!(name, "get" | "__setitem__")
                && method.method.unwrap().kind(self.classes.declarations)
                    != crate::classes::MethodKind::Instance)
        {
            return Err(error(
                self.source,
                at,
                "unsupported record primitive signature",
            ));
        }
        let mut inputs = vec![receiver];
        for (index, (argument, param)) in arguments.iter().zip(&method.params[1..]).enumerate() {
            let ty = if let Some(inference) = self.inference {
                inference.learn_parameter(method.id, index + 1, argument.ty);
                inference.parameter(method.id, index + 1).unwrap()
            } else {
                param.ty
            };
            inputs.push(self.coerce(argument.clone(), ty, at)?);
        }
        self.calls.insert(method.id);
        Ok(self.call_value(
            &method,
            method.returns,
            ir::CallArgs::positional_at(inputs, site),
        ))
    }

    /// `type(record)()` uses the default constructor of the actual fixed layout.
    pub(super) fn construct_receiver_type(
        &mut self,
        call: &ast::ExprCall,
    ) -> Result<Option<Expr>, String> {
        let ast::Expr::Call(ty) = &*call.func else {
            return Ok(None);
        };
        if !matches!(&*ty.func, ast::Expr::Name(n) if n.id == "type" && !self.names.contains_key("type") && self.resolve(&ty.func).is_none())
            || !ty.arguments.keywords.is_empty()
        {
            return Ok(None);
        }
        let [arg] = ty.arguments.args.as_ref() else {
            return Ok(None);
        };
        let receiver = self.expr(arg)?;
        let Type::Class(base) = receiver.ty else {
            return Err(error(
                self.source,
                arg,
                "type(value) construction requires a fixed record",
            ));
        };
        let value = self.dispatch_record(base, receiver, call, |checker, class, _| {
            checker.construct(call, class)
        })?;
        Ok(Some(value))
    }
}
