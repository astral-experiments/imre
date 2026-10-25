//! Bind the original Python constructor once, then fill the shared exception record prefix.
use super::*;
use crate::classes::ForwardingInitializer;

struct Bound {
    names: HashMap<String, Expr>,
    tail: Vec<Expr>,
}

enum BindFailure {
    Argument(String),
    Unknown(String, String),
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
    /// Only admit values whose runtime type, repr, scalar comparison and indexing we can expose.
    pub(super) fn forwarding_value_supported(&self, ty: Type) -> bool {
        matches!(
            ty,
            Type::None
                | Type::Bool
                | Type::Int
                | Type::Str
                | Type::StrLiteral(_)
                | Type::Bytes
                | Type::Float
                | Type::ExceptionField
        ) || self
            .types
            .payload(ty)
            .or_else(|| self.types.element(ty))
            .is_some_and(|item| self.forwarding_value_supported(item))
            || self.types.fields(ty).is_some_and(|items| {
                items
                    .into_iter()
                    .all(|item| self.forwarding_value_supported(item))
            })
    }

    /// Prove only the exact argument ranges independently diagnosed by ty.
    fn checked_forwarding_call(
        &self,
        plan: &ForwardingInitializer,
        call: &ast::ExprCall,
        given: usize,
    ) {
        let keywords = &call.arguments.keywords;
        let mut missing = false;
        for (i, (name, default)) in plan.parameters.iter().enumerate() {
            let positional = i < plan.positional && i < given;
            let keyword = keywords
                .iter()
                .find(|kw| kw.arg.as_ref().is_some_and(|n| n.as_str() == name));
            if positional && let Some(kw) = keyword {
                self.graph
                    .checked_duplicate_argument(self.module, kw.range());
            }
            missing |= !positional && keyword.is_none() && default.is_none();
        }
        if missing {
            self.graph
                .checked_missing_argument(self.module, call.range());
        }
        if plan.variadic.is_none() && !calls::starred(call) {
            for arg in call.arguments.args.iter().skip(plan.positional) {
                self.graph.checked_excess_argument(self.module, arg.range());
            }
        }
        for kw in keywords {
            let Some(name) = &kw.arg else { continue };
            if name.as_str() == plan.receiver {
                self.graph
                    .checked_duplicate_argument(self.module, kw.range());
            } else if !plan.parameters.iter().any(|(key, _)| key == name.as_str()) {
                self.graph
                    .checked_keywords(self.module, std::iter::once(kw.range()));
            }
        }
    }

    fn bind_forwarder(
        &self,
        plan: &ForwardingInitializer,
        name: &str,
        positional: Vec<Expr>,
        named: Vec<(String, Expr)>,
    ) -> Result<Bound, BindFailure> {
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
            if keyword == plan.receiver {
                return Err(BindFailure::Argument(format!(
                    "{name}.__init__() got multiple values for argument '{keyword}'"
                )));
            }
            if !plan.parameters.iter().any(|(n, _)| n == &keyword) {
                return Err(BindFailure::Unknown(
                    format!("{name}.__init__() got an unexpected keyword argument '{keyword}'"),
                    keyword,
                ));
            };
            if names.insert(keyword.clone(), value).is_some() {
                return Err(BindFailure::Argument(format!(
                    "{name}.__init__() got multiple values for argument '{keyword}'"
                )));
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
            let kw_only = plan.parameters[plan.positional..]
                .iter()
                .filter(|(key, _)| names.contains_key(key))
                .count();
            let suffix = if kw_only == 0 {
                String::new()
            } else {
                format!(
                    " positional arguments (and {kw_only} keyword-only argument{})",
                    if kw_only == 1 { "" } else { "s" }
                )
            };
            return Err(BindFailure::Argument(format!(
                "{name}.__init__() takes {expected} positional {noun} but {}{suffix} were given",
                supplied + 1
            )));
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
                    names.insert(key.clone(), default.clone());
                } else {
                    absent.push(key.as_str());
                }
            }
            if !absent.is_empty() {
                let plural = if absent.len() == 1 { "" } else { "s" };
                return Err(BindFailure::Argument(format!(
                    "{name}.__init__() missing {} required {kind} argument{plural}: {}",
                    absent.len(),
                    missing(&absent)
                )));
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
        let mut after_super = Vec::new();
        for value in args.iter().chain(kwargs.iter().map(|(_, v)| v)) {
            if !self.forwarding_value_supported(value.ty) {
                return Err(error(
                    self.source,
                    call,
                    "forwarding exception fields require native scalars, fixed tuples, or typed lists of these values",
                ));
            }
        }
        loop {
            let plan = self.classes.declarations.definitions[current]
                .forwarder
                .as_ref()
                .unwrap();
            let lexical = self.classes.declarations.ids[&plan.owner];
            let decl = &self.classes.declarations.definitions[lexical];
            if current == class {
                self.checked_forwarding_call(plan, call, args.len());
            }
            let bound = match self.bind_forwarder(plan, &decl.name, args, kwargs) {
                Ok(bound) => bound,
                Err(failure) => {
                    let message = match failure {
                        BindFailure::Argument(s) => unpacking::string(s),
                        BindFailure::Unknown(s, k) => {
                            let candidates: Vec<_> = std::iter::once(&plan.receiver)
                                .chain(plan.parameters.iter().map(|(name, _)| name))
                                .cloned()
                                .map(unpacking::string)
                                .collect();
                            let tuple = Expr {
                                ty: self.types.tuple(vec![Type::Str; candidates.len()]),
                                kind: ExprKind::Tuple(candidates),
                            };
                            Expr {
                                ty: Type::Str,
                                kind: ExprKind::Builtin(
                                    Builtin::ExceptionKeywordSuggestion,
                                    vec![unpacking::string(s), unpacking::string(k), tuple],
                                ),
                            }
                        }
                    };
                    let error = Expr {
                        ty: Type::Exception,
                        kind: ExprKind::Exception {
                            kind: exception_kind("TypeError").unwrap(),
                            message: Some(Box::new(message)),
                        },
                    };
                    return Ok(scope.finish(Expr {
                        ty: Type::NamedException(class),
                        kind: ExprKind::Builtin(Builtin::RaiseException, vec![error]),
                    }));
                }
            };
            for (field, source) in &plan.stores[..plan.before_super] {
                stored.insert(field.clone(), bound.names[source].clone());
            }
            after_super.push(
                plan.stores[plan.before_super..]
                    .iter()
                    .map(|(field, source)| (field.clone(), bound.names[source].clone()))
                    .collect::<Vec<_>>(),
            );
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
        for assignments in after_super.into_iter().rev() {
            stored.extend(assignments);
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
