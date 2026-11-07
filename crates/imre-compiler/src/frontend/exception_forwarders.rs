//! Bind the original Python constructor once, then fill the shared exception record prefix.
use super::*;
use crate::classes::ForwardingInitializer;
use ruff_python_stdlib::identifiers::is_mangled_private;
use std::borrow::Cow;

struct Bound<T> {
    names: HashMap<String, T>,
    tail: Vec<T>,
}

struct Formals<'a, T> {
    receiver: &'a str,
    parameters: &'a [(String, Option<T>)],
    positional: usize,
    variadic: bool,
}

impl<'a> From<&'a ForwardingInitializer> for Formals<'a, Expr> {
    fn from(plan: &'a ForwardingInitializer) -> Self {
        Self {
            receiver: &plan.receiver,
            parameters: &plan.parameters,
            positional: plan.positional,
            variadic: plan.variadic.is_some(),
        }
    }
}

enum BindFailure {
    Argument(String),
    Unknown(String, String),
}

/// Callers bind private parameters by their lexical class spelling.
fn parameter_name<'a>(class: &str, name: &'a str) -> Cow<'a, str> {
    let owner = class.trim_start_matches('_');
    if is_mangled_private(name) && !owner.is_empty() {
        Cow::Owned(format!("_{owner}{name}"))
    } else {
        Cow::Borrowed(name)
    }
}

/// Python spells the missing names in declaration order.
fn missing(names: &[Cow<'_, str>]) -> String {
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
        self.forwarding_member_supported(ty, false)
    }

    fn forwarding_member_supported(&self, ty: Type, inside_list: bool) -> bool {
        // An erased element of a mutable list could point back into that list.
        // Immutable tuples of independently rooted views cannot introduce such a cycle.
        if ty == Type::ObjectValue {
            return !inside_list;
        }
        if let Some(item) = self.types.element(ty) {
            return self.forwarding_member_supported(item, true);
        }
        if let Some(item) = self.types.payload(ty) {
            return self.forwarding_member_supported(item, inside_list);
        }
        matches!(
            ty,
            Type::None
                | Type::Bool
                | Type::Int
                | Type::Str
                | Type::StrLiteral(_)
                | Type::Bytes
                | Type::Float
        ) || !self.types.is_partial_frame(ty)
            && self.types.fields(ty).is_some_and(|items| {
                items
                    .into_iter()
                    .all(|item| self.forwarding_member_supported(item, inside_list))
            })
    }

    /// Prove only the exact argument ranges independently diagnosed by ty.
    fn checked_forwarding_call(
        &self,
        module: crate::modules::ModuleId,
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
            // ty treats a leading double underscore as a legacy positional-only name.
            if i < plan.positional
                && is_mangled_private(name)
                && let Some(kw) = keyword
            {
                self.graph.checked_positional_keyword(module, kw.range());
            }
            if positional && let Some(kw) = keyword {
                self.graph.checked_repeated_keyword(module, kw.range());
            }
            missing |= !positional && keyword.is_none() && default.is_none();
        }
        if missing {
            self.graph.checked_missing_argument(module, call.range());
        }
        if plan.variadic.is_none() && !calls::starred(call) {
            for arg in call.arguments.args.iter().skip(plan.positional) {
                self.graph.checked_excess_argument(module, arg.range());
            }
        }
        for kw in keywords {
            let Some(name) = &kw.arg else { continue };
            if name.as_str() == plan.receiver {
                self.graph.checked_repeated_keyword(module, kw.range());
            } else if !plan.parameters.iter().any(|(key, _)| key == name.as_str()) {
                self.graph
                    .checked_keywords(module, std::iter::once(kw.range()));
            }
        }
    }

    fn bind_forwarder<T: Clone>(
        &self,
        plan: Formals<'_, T>,
        name: &str,
        positional: Vec<T>,
        named: Vec<(String, T)>,
    ) -> Result<Bound<T>, BindFailure> {
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
            if keyword == parameter_name(name, plan.receiver) {
                return Err(BindFailure::Argument(format!(
                    "{name}.__init__() got multiple values for argument '{keyword}'"
                )));
            }
            let Some((source, _)) = plan
                .parameters
                .iter()
                .find(|(raw, _)| parameter_name(name, raw) == keyword)
            else {
                return Err(BindFailure::Unknown(
                    format!("{name}.__init__() got an unexpected keyword argument '{keyword}'"),
                    keyword,
                ));
            };
            if names.insert(source.clone(), value).is_some() {
                return Err(BindFailure::Argument(format!(
                    "{name}.__init__() got multiple values for argument '{keyword}'"
                )));
            }
        }
        if !plan.variadic && !tail.is_empty() {
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
                    absent.push(parameter_name(name, key));
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

    fn binding_message<T>(
        &self,
        failure: BindFailure,
        owner: &str,
        receiver: &str,
        parameters: &[(String, Option<T>)],
    ) -> Expr {
        match failure {
            BindFailure::Argument(s) => unpacking::string(s),
            BindFailure::Unknown(s, k) => {
                let candidates: Vec<_> = std::iter::once(receiver)
                    .chain(parameters.iter().map(|(name, _)| name.as_str()))
                    .map(|name| unpacking::string(parameter_name(owner, name)))
                    .collect();
                self.keyword_error_message(s, k, candidates)
            }
        }
    }

    /// The wrapper accepts all operands. A binding error comes from its first
    /// base call, after all caller expressions have been evaluated in order.
    pub(super) fn forwarded_init_error(
        &mut self,
        class: usize,
        parent: DefinitionId,
        call: &ast::ExprCall,
        signature: &Signature,
    ) -> Result<Option<Expr>, String> {
        if calls::starred(call) || call.arguments.keywords.iter().any(|kw| kw.arg.is_none()) {
            return Ok(None);
        }
        let params = &signature.params[1..];
        let keywords = call
            .arguments
            .keywords
            .iter()
            .map(|kw| (kw.arg.as_ref().unwrap().to_string(), ()))
            .collect();
        // Bind only argument positions here. The ordinary binder still checks valid calls.
        let slots: Vec<_> = params
            .iter()
            .map(|p| (p.name.clone(), p.default.as_ref().map(|_| ())))
            .collect();
        let classes = &self.classes.declarations;
        let base = &classes.definitions[classes.ids[&parent]];
        let receiver = base.methods[base.initializer.unwrap()].parameters.args[0]
            .name()
            .to_string();
        let wrapper_receiver = &signature.params[0].name;
        let failure = if let Some(kw) = call.arguments.keywords.iter().find(|kw| {
            kw.arg
                .as_ref()
                .is_some_and(|name| name.as_str() == wrapper_receiver)
        }) {
            self.graph.checked_repeated_keyword(self.module, kw.range());
            let method = signature
                .method
                .expect("forwarding initializer is a method");
            let origin = classes.definitions[method.class].method_origins[method.index];
            let wrapper = &classes.definitions[classes.ids[&origin]].name;
            BindFailure::Argument(format!(
                "{wrapper}.__init__() got multiple values for argument '{wrapper_receiver}'"
            ))
        } else {
            let formals = Formals {
                receiver: &receiver,
                parameters: &slots,
                positional: slots.len(),
                variadic: false,
            };
            let Err(failure) = self.bind_forwarder(
                formals,
                &base.name,
                vec![(); call.arguments.args.len()],
                keywords,
            ) else {
                return Ok(None);
            };
            failure
        };
        let message = self.binding_message(failure, &base.name, &receiver, &slots);
        let mut operands = Vec::new();
        for arg in &call.arguments.args {
            operands.push(self.expr(arg)?);
        }
        for kw in &call.arguments.keywords {
            operands.push(self.expr(&kw.value)?);
        }
        Ok(Some(calls::type_error_message(
            Type::Class(class),
            operands,
            message,
        )))
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
        let mut source_module = self.module;
        let mut source_call = call;
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
            self.checked_forwarding_call(source_module, plan, source_call, args.len());
            let bound = match self.bind_forwarder(plan.into(), &decl.name, args, kwargs) {
                Ok(bound) => bound,
                Err(failure) => {
                    let message =
                        self.binding_message(failure, &decl.name, &plan.receiver, &plan.parameters);
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
            if let Some(base) = decl.storage_base {
                // The recognized constructor body contains exactly one call statement.
                source_call = decl.methods[decl.initializer.expect("forwarding initializer")]
                    .body
                    .iter()
                    .find_map(|stmt| stmt.as_expr_stmt()?.value.as_call_expr())
                    .expect("forwarded super call");
                source_module = plan.owner.module;
                current = base;
            } else {
                break;
            }
        }
        for assignments in after_super.into_iter().rev() {
            stored.extend(assignments);
        }
        let decl = &self.classes.declarations.definitions[class];
        let id = self.classes.declarations.exceptions[&decl.definition];
        let exception = self
            .classes
            .declarations
            .exception_declarations
            .iter()
            .find(|entry| entry.id == id)
            .unwrap();
        let inherits =
            |name| exception_kind(name).is_some_and(|kind| exception.ancestors.contains(&kind));
        // These builtin initializers validate or transform their positional arguments.
        if inherits("UnicodeEncodeError") || (args.len() > 1 && inherits("OSError")) {
            return Err(error(
                self.source,
                call,
                "forwarding these arguments to a specialized builtin exception initializer is unsupported",
            ));
        }
        let tuple = Expr {
            ty: self.types.tuple(args.iter().map(|arg| arg.ty).collect()),
            kind: ExprKind::Tuple(args),
        };
        let args = Expr {
            ty: Type::ObjectValue,
            kind: ExprKind::Builtin(Builtin::ExceptionTupleView, vec![tuple]),
        };
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
            fields.push(if value.ty == Type::ObjectValue {
                value
            } else {
                Expr {
                    ty: Type::ObjectValue,
                    kind: ExprKind::Builtin(Builtin::BoxValue, vec![value]),
                }
            });
        }
        let record = scope.finish(Expr {
            ty: Type::Class(class),
            kind: ExprKind::Builtin(Builtin::RecordFromFields, fields),
        });
        Ok(self.wrap_exception_record(class, record))
    }
}
