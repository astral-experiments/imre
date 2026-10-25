//! Lower Mapping mixins through the concrete subclass's abstract primitives.

use super::*;

/// Preserve evaluated arguments when a class or ABC check raises at runtime.
fn type_error(result: Type, message: String, mut inputs: Vec<Expr>) -> Expr {
    inputs.push(Expr {
        ty: Type::Exception,
        kind: ExprKind::Exception {
            kind: exception_kind("TypeError").unwrap(),
            message: Some(Box::new(unpacking::string(message))),
        },
    });
    Expr {
        ty: result,
        kind: ExprKind::Builtin(Builtin::RaiseException, inputs),
    }
}

impl Checker<'_> {
    /// A statically known abstract Mapping still fails when the call executes.
    pub(super) fn abstract_mapping_call(
        &mut self,
        class: usize,
        call: &ast::ExprCall,
    ) -> Result<Expr, String> {
        calls::reject_unpacking(call, self.source)?;
        let declared = &self.classes.declarations.definitions[class];
        let mut names = declared
            .abstract_methods
            .iter()
            .map(|name| format!("'{name}'"))
            .collect::<Vec<_>>();
        names.sort();
        let plural = if names.len() == 1 {
            "method"
        } else {
            "methods"
        };
        let message = format!(
            "Can't instantiate abstract class {} without an implementation for abstract {plural} {}",
            declared.name,
            names.join(", ")
        );
        let mut inputs = Vec::new();
        for value in &call.arguments.args {
            inputs.push(self.expr(value)?);
        }
        for keyword in &call.arguments.keywords {
            inputs.push(self.expr(&keyword.value)?);
        }
        self.graph
            .checked_abstract_class_call(self.module, call.range());
        Ok(type_error(Type::Class(class), message, inputs))
    }

    /// Class objects are static here: preserve dict's virtual registration as
    /// well as nominal Mapping bases, without guessing at other class values.
    pub(super) fn mapping_issubclass(&self, call: &ast::ExprCall) -> Result<Option<Expr>, String> {
        let [child, parent] = call.arguments.args.as_ref() else {
            return Ok(None);
        };
        let Some(Binding::Intrinsic(Intrinsic::Mapping { typing_alias })) = self.resolve(parent)
        else {
            return Ok(None);
        };
        self.reject_builtin_keywords(call)?;
        if !typing_alias
            && matches!(
                self.resolve(child),
                Some(Binding::Intrinsic(Intrinsic::Mapping {
                    typing_alias: true
                }))
            )
        {
            // typing.Mapping is a typing alias, not a class accepted by ABCMeta.
            return Ok(Some(type_error(
                Type::Bool,
                "issubclass() arg 1 must be a class".into(),
                vec![],
            )));
        }
        let matched = if let Some(id) = self.class_reference(child) {
            self.classes.declarations.definitions[id].mapping
        } else if matches!(
            self.resolve(child),
            Some(Binding::Intrinsic(
                Intrinsic::Mapping { .. } | Intrinsic::Counter | Intrinsic::DefaultDict
            ))
        ) {
            true
        } else if let ast::Expr::Name(n) = child
            && !self.names.contains_key(n.id.as_str())
            && self.resolve(child).is_none()
            && matches!(
                n.id.as_str(),
                "dict"
                    | "str"
                    | "list"
                    | "tuple"
                    | "bool"
                    | "int"
                    | "bytes"
                    | "float"
                    | "object"
                    | "set"
            )
        {
            n.id == "dict"
        } else {
            return Err(error(
                self.source,
                child,
                "issubclass requires a supported static class",
            ));
        };
        Ok(Some(Expr {
            ty: Type::Bool,
            kind: ExprKind::Bool(matched),
        }))
    }

    pub(super) fn mapping_class(&self, ty: Type) -> Option<usize> {
        let Type::Class(id) = ty else { return None };
        self.classes.declarations.definitions[id]
            .mapping
            .then_some(id)
    }

    fn checked_mapping_size(&self, size: Expr, at: &impl Ranged) -> Result<Expr, String> {
        let size = if size.ty == Type::Bool {
            Expr {
                ty: Type::Int,
                kind: ExprKind::Builtin(Builtin::Int, vec![size]),
            }
        } else {
            self.expect(Type::Int, size.ty, at)?;
            size
        };
        Ok(Expr {
            ty: Type::Int,
            kind: ExprKind::Builtin(Builtin::LengthResult, vec![size]),
        })
    }

    pub(super) fn mapping_length(
        &mut self,
        receiver: Expr,
        at: &impl Ranged,
    ) -> Result<Expr, String> {
        let size = self.record_primitive(receiver, "__len__", &[], at)?;
        self.checked_mapping_size(size, at)
    }

    /// Truth uses the concrete class's __bool__ when present, otherwise __len__.
    pub(super) fn mapping_truth(
        &mut self,
        class: usize,
        value: Expr,
        at: &impl Ranged,
    ) -> Result<Expr, String> {
        let direct = |checker: &mut Self, class: usize, receiver: Expr| {
            if checker.classes.methods[class].contains_key("__bool__") {
                checker.record_slot(class, receiver, "__bool__", &[], at)
            } else {
                let size = checker.record_slot(class, receiver, "__len__", &[], at)?;
                Ok(Expr {
                    ty: Type::Bool,
                    kind: ExprKind::Builtin(
                        Builtin::Bool,
                        vec![checker.checked_mapping_size(size, at)?],
                    ),
                })
            }
        };
        if self.classes.has_concrete_subclasses(class) {
            self.dispatch_record(class, value, at, direct)
        } else {
            direct(self, class, value)
        }
    }

    /// The caller has captured the arguments in source order and selected the
    /// concrete class. Only exceptions from __getitem__ reach the handler.
    fn mapping_lookup(
        &mut self,
        receiver: Expr,
        key: Expr,
        default: Option<Expr>,
        at: &impl Ranged,
    ) -> Result<Expr, String> {
        let class = self.mapping_class(receiver.ty).unwrap();
        let lookup = self.record_slot(class, receiver, "__getitem__", &[key], at)?;
        let (result, found, missing) = if let Some(default) = default {
            let common = self.types.join(lookup.ty, default.ty).ok_or_else(|| {
                error(
                    self.source,
                    at,
                    "Mapping.get requires a representable result and default",
                )
            })?;
            let result = self.hidden(common);
            (
                result,
                vec![Statement::Assign(result, self.coerce(lookup, common, at)?)],
                self.coerce(default, common, at)?,
            )
        } else {
            let result = self.hidden(Type::Bool);
            let flag = |value| Expr {
                ty: Type::Bool,
                kind: ExprKind::Bool(value),
            };
            (
                result,
                vec![
                    Statement::Expr(lookup),
                    Statement::Assign(result, flag(true)),
                ],
                flag(false),
            )
        };
        let attempt = Statement::Try {
            body: found,
            handlers: vec![ir::ExceptHandler {
                kinds: vec![exception_kind("KeyError").unwrap()],
                name: None,
                body: vec![Statement::Assign(result, missing)],
            }],
            orelse: vec![],
            finalbody: vec![],
        };
        let value = self.local(result);
        Ok(Expr {
            ty: value.ty,
            kind: ExprKind::Scope {
                body: vec![attempt],
                value: Box::new(value),
                clear: vec![result],
            },
        })
    }

    /// Membership evaluates the needle before its receiver, then dispatches a
    /// concrete override or the inherited Mapping lookup.
    pub(super) fn mapping_membership(
        &mut self,
        key: Expr,
        owner: Expr,
        at: &impl Ranged,
    ) -> Result<Expr, String> {
        let at_key = self.hidden(key.ty);
        let at_owner = self.hidden(owner.ty);
        let class = self.mapping_class(owner.ty).unwrap();
        let direct = |checker: &mut Self, class: usize, receiver: Expr| {
            if checker.classes.methods[class].contains_key("__contains__") {
                let found = checker.record_slot(
                    class,
                    receiver,
                    "__contains__",
                    &[checker.local(at_key)],
                    at,
                )?;
                checker.test_truth(found, at)
            } else {
                checker.mapping_lookup(receiver, checker.local(at_key), None, at)
            }
        };
        let value = if self.classes.has_concrete_subclasses(class) {
            self.dispatch_record(class, self.local(at_owner), at, direct)?
        } else {
            direct(self, class, self.local(at_owner))?
        };
        Ok(Expr {
            ty: Type::Bool,
            kind: ExprKind::Scope {
                body: vec![
                    Statement::Assign(at_key, key),
                    Statement::Assign(at_owner, owner),
                ],
                value: Box::new(value),
                clear: vec![at_owner, at_key],
            },
        })
    }

    /// A call evaluates the Mapping receiver, key and supplied default before a lookup.
    pub(super) fn mapping_get(
        &mut self,
        call: &ast::ExprCall,
        receiver: Expr,
    ) -> Result<Expr, String> {
        calls::reject_unpacking(call, self.source)?;
        let owner = self.hidden(receiver.ty);
        let mut body = vec![Statement::Assign(owner, receiver)];
        let mut clear = vec![owner];
        let mut slots = [None; 2];
        let keywords = call.arguments.keywords.iter().map(|keyword| {
            (
                match keyword.arg.as_ref().map(|name| name.as_str()) {
                    Some("key") => 0,
                    Some("default") => 1,
                    _ => 2,
                },
                &keyword.value,
            )
        });
        for (position, expression) in call.arguments.args.iter().enumerate().chain(keywords) {
            if position >= slots.len() || slots[position].is_some() {
                return Err(error(
                    self.source,
                    call,
                    "Mapping.get requires a key and an optional default",
                ));
            }
            let value = self.expr(expression)?;
            let slot = self.hidden(value.ty);
            body.push(Statement::Assign(slot, value));
            clear.push(slot);
            slots[position] = Some(slot);
        }
        let key = slots[0].ok_or_else(|| error(self.source, call, "Mapping.get requires a key"))?;
        let default = slots[1].map(|slot| self.local(slot)).unwrap_or(Expr {
            ty: Type::None,
            kind: ExprKind::None,
        });
        let value = self.mapping_lookup(self.local(owner), self.local(key), Some(default), call)?;
        if !call.arguments.keywords.is_empty() {
            self.graph.checked_overload(self.module, call.range());
        }
        Ok(Expr {
            ty: value.ty,
            kind: ExprKind::Scope {
                body,
                clear,
                value: Box::new(value),
            },
        })
    }
}
