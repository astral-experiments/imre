//! Bind the original Python constructor once, then fill the shared exception record prefix.
use super::*;
use crate::classes::ForwardingInitializer;

struct Bound {
    names: HashMap<String, Expr>,
    tail: Vec<Expr>,
}

/// Python spells the missing names in declaration order.
fn missing(names: &[&str]) -> String {
    let quoted: Vec<_> = names.iter().map(|name| format!("'{name}'")).collect();
    match quoted.as_slice() {
        [one] => one.clone(),
        [one, two] => format!("{one} and {two}"),
        [rest @ .., last] => format!("{}, and {last}", rest.join(", ")),
        [] => String::new(),
    }
}

impl Checker<'_> {
    fn bind_forwarder(
        &self,
        plan: &ForwardingInitializer,
        name: &str,
        positional: Vec<Expr>,
        named: Vec<(String, Expr)>,
    ) -> Result<Bound, String> {
        let supplied = positional.len();
        let mut names = HashMap::new();
        let mut tail = Vec::new();
        for (i, value) in positional.into_iter().enumerate() {
            if i < plan.positional {
                names.insert(plan.parameters[i].0.clone(), value);
            } else {
                tail.push(value);
            }
        }
        for (keyword, value) in named {
            let Some(i) = plan.parameters.iter().position(|(n, _)| n == &keyword) else {
                return Err(format!(
                    "{name}.__init__() got an unexpected keyword argument '{keyword}'"
                ));
            };
            if i < plan.positional_only {
                return Err(format!(
                    "{name}.__init__() got some positional-only arguments passed as keyword arguments: '{keyword}'"
                ));
            }
            if names.insert(keyword.clone(), value).is_some() {
                return Err(format!(
                    "{name}.__init__() got multiple values for argument '{keyword}'"
                ));
            }
        }
        if plan.variadic.is_none() && !tail.is_empty() {
            let count = plan.positional + 1;
            let minimum = 1 + plan.parameters[..plan.positional]
                .iter()
                .filter(|(_, d)| d.is_none())
                .count();
            let expected = if count == minimum {
                count.to_string()
            } else {
                format!("from {minimum} to {count}")
            };
            let noun = if count == 1 { "argument" } else { "arguments" };
            return Err(format!(
                "{name}.__init__() takes {expected} positional {noun} but {} were given",
                supplied + 1
            ));
        }
        for (kind, from, to) in [
            ("positional", 0, plan.positional),
            ("keyword-only", plan.positional, plan.parameters.len()),
        ] {
            let mut absent = Vec::new();
            for (key, default) in &plan.parameters[from..to] {
                if names.contains_key(key) {
                    continue;
                }
                if let Some(default) = default {
                    names.insert(key.clone(), calls::scalar_literal(default, self.source)?);
                } else {
                    absent.push(key.as_str());
                }
            }
            if !absent.is_empty() {
                let plural = if absent.len() == 1 { "" } else { "s" };
                return Err(format!(
                    "{name}.__init__() missing {} required {kind} argument{plural}: {}",
                    absent.len(),
                    missing(&absent)
                ));
            }
        }
        Ok(Bound { names, tail })
    }

    /// Each operand stays rooted and is evaluated before Python argument binding can raise.
    pub(super) fn forwarding_exception(
        &mut self,
        call: &ast::ExprCall,
        class: usize,
    ) -> Result<Expr, String> {
        let mut scope = calls::ArgumentScope::default();
        let mut args = Vec::new();
        for arg in &call.arguments.args {
            if let ast::Expr::Starred(star) = arg {
                args.extend(scope.spread(self, star)?);
            } else {
                let value = Self::string_value(self.expr(arg)?);
                args.push(scope.hold(self, value));
            }
        }
        let mut kwargs = Vec::new();
        for kw in &call.arguments.keywords {
            let Some(name) = &kw.arg else {
                return Err(error(
                    self.source,
                    kw,
                    "forwarding exception keywords require a statically named key",
                ));
            };
            let value = Self::string_value(self.expr(&kw.value)?);
            kwargs.push((name.to_string(), scope.hold(self, value)));
        }
        let mut current = class;
        let mut stored = HashMap::new();
        loop {
            let plan = self.classes.declarations.definitions[current]
                .forwarder
                .as_ref()
                .unwrap();
            let lexical = self.classes.declarations.ids[&plan.owner];
            let decl = &self.classes.declarations.definitions[lexical];
            let bound = match self.bind_forwarder(plan, &decl.name, args, kwargs) {
                Ok(bound) => bound,
                Err(message) => {
                    self.graph.checked_overload(self.module, call.range());
                    self.graph.checked_keywords(
                        self.module,
                        call.arguments.keywords.iter().map(Ranged::range),
                    );
                    let error = Expr {
                        ty: Type::Exception,
                        kind: ExprKind::Exception {
                            kind: exception_kind("TypeError").unwrap(),
                            message: Some(Box::new(unpacking::string(message))),
                        },
                    };
                    return Ok(scope.finish(Expr {
                        ty: Type::NamedException(class),
                        kind: ExprKind::Builtin(Builtin::RaiseException, vec![error]),
                    }));
                }
            };
            for (field, source) in &plan.stores {
                stored.insert(field.clone(), bound.names[source].clone());
            }
            args = Vec::new();
            for (name, spread) in &plan.arguments {
                if *spread {
                    args.extend(bound.tail.iter().cloned());
                } else {
                    args.push(bound.names[name].clone());
                }
            }
            kwargs = plan
                .keywords
                .iter()
                .map(|(name, source)| (name.clone(), bound.names[source].clone()))
                .collect();
            if let Some(&base) = decl.bases.first() {
                current = base;
            } else {
                break;
            }
        }
        let tuple = Expr {
            ty: self.types.tuple(args.iter().map(|arg| arg.ty).collect()),
            kind: ExprKind::Tuple(args),
        };
        let args = Expr {
            ty: Type::ExceptionField,
            kind: ExprKind::Builtin(Builtin::ExceptionTupleView, vec![tuple]),
        };
        let decl = &self.classes.declarations.definitions[class];
        let hidden = decl.exception_argument_slot();
        let mut fields = Vec::new();
        for (i, field) in decl.fields.iter().enumerate() {
            if i == hidden {
                fields.push(args.clone());
                continue;
            }
            let value = stored.remove(&field.name).ok_or_else(|| {
                error(
                    self.source,
                    call,
                    "forwarding exception did not initialize a declared field",
                )
            })?;
            fields.push(if value.ty == Type::ExceptionField {
                value
            } else {
                Expr {
                    ty: Type::ExceptionField,
                    kind: ExprKind::Builtin(Builtin::BoxExceptionField, vec![value]),
                }
            });
        }
        let record = scope.finish(Expr {
            ty: Type::Class(class),
            kind: ExprKind::Builtin(Builtin::ExceptionRecordFields, fields),
        });
        Ok(self.wrap_exception_record(class, record))
    }
}
