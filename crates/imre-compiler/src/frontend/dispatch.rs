//! Dispatch through a fixed-layout base when a program admits several concrete records.

use super::*;

impl ClassInfo {
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
        let Some((_, mut value)) = alternatives.pop() else {
            return Err(error(
                self.source,
                at,
                "abstract record has no concrete implementation",
            ));
        };
        while let Some((class, mut next)) = alternatives.pop() {
            if next.ty != value.ty {
                if !matches!((next.ty, value.ty), (Type::Class(a), Type::Class(b)) if
                    (a == base || self.classes.declarations.definitions[a].bases.contains(&base)) &&
                    (b == base || self.classes.declarations.definitions[b].bases.contains(&base)))
                {
                    return Err(error(
                        self.source,
                        at,
                        "record overrides must have a common result layout",
                    ));
                }
                next = self.coerce(next, Type::Class(base), at)?;
                value = self.coerce(value, Type::Class(base), at)?;
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
        let method = self.classes.methods[class]
            .get(name)
            .ok_or_else(|| error(self.source, at, "missing record primitive"))?
            .clone();
        if method.params.len() != arguments.len() + 1 {
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
        Ok(Expr {
            ty: method.returns,
            kind: ExprKind::Call(method.id, ir::CallArgs::positional(inputs)),
        })
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
