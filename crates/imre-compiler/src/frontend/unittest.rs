//! Native unittest scopes use the same exception hierarchy and cleanup edges as try/finally.

use super::*;

#[derive(Default)]
struct Expected {
    kinds: Vec<u32>,
    label: String,
    repr: String,
}

impl Checker<'_> {
    /// Read the concrete case's class metadata even when a fixture receives a base-typed case.
    pub(super) fn testcase_class_text(
        &mut self,
        attribute: &ast::ExprAttribute,
    ) -> Result<Option<Expr>, String> {
        let name = attribute.attr.as_str();
        let Some(owner) = attribute.value.as_attribute_expr() else {
            return Ok(None);
        };
        if owner.attr.as_str() != "__class__" || !matches!(name, "__module__" | "__qualname__") {
            return Ok(None);
        }
        let receiver = self.expr(&owner.value)?;
        let Type::Class(base) = receiver.ty else {
            return Ok(None);
        };
        if !self.unittest_case(base) {
            return Ok(None);
        }
        self.dispatch_record(base, receiver, attribute, |checker, class, _| {
            let record = &checker.classes.declarations.definitions[class];
            if record
                .methods
                .iter()
                .any(|m| m.name.as_str() == "__class__")
                || record.fields.iter().any(|f| f.name == "__class__")
                || record.static_bindings.contains_key("__class__")
                || record.static_bindings.contains_key(name)
            {
                return Err(error(
                    checker.source,
                    attribute,
                    "custom TestCase class metadata requires its original descriptor",
                ));
            }
            let module = &checker.graph.modules[record.definition.module];
            let text = if name == "__qualname__" {
                record.name.as_str()
            } else if record.definition.module == checker.graph.entry {
                "__main__"
            } else if module.bundled && module.name == "unittest" && record.name == "TestCase" {
                "unittest.case"
            } else {
                module.name.as_str()
            };
            Ok(unpacking::string(text))
        })
        .map(Some)
    }

    pub(super) fn unittest_case(&self, class: usize) -> bool {
        let classes = &self.classes.declarations.definitions;
        std::iter::once(&class)
            .chain(&classes[class].bases)
            .any(|&index| {
                let record = &classes[index];
                let module = &self.graph.modules[record.definition.module];
                record.name == "TestCase" && module.bundled && module.name == "unittest"
            })
    }

    /// Format a case's failure class without constructing an exception. A
    /// case stored in a base-typed local may select a subclass's replacement.
    pub(super) fn unittest_failure_repr(
        &mut self,
        value: &ast::Expr,
    ) -> Result<Option<Expr>, String> {
        let Some(attr) = value.as_attribute_expr() else {
            return Ok(None);
        };
        if attr.attr.as_str() != "failureException" {
            return Ok(None);
        }
        let Some(name) = attr.value.as_name_expr() else {
            return Ok(None);
        };
        let Some(slot) = self.names.get(name.id.as_str()).copied() else {
            return Ok(None);
        };
        let Type::Class(class) = self.read_local(slot).ty else {
            return Ok(None);
        };
        if !self.unittest_case(class) {
            return Ok(None);
        }
        let receiver = self.expr(&attr.value)?;
        self.dispatch_record(class, receiver, value, |checker, concrete, _| {
            let (_, _, qualified) = checker
                .unittest_category(value, Some((name.id.as_str(), concrete)))
                .ok_or_else(|| {
                    error(
                        checker.source,
                        value,
                        "cannot identify the case's failureException",
                    )
                })?;
            Ok(unpacking::string(format!("<class '{qualified}'>")))
        })
        .map(Some)
    }

    /// An ordinary except clause has a fixed category only when every concrete
    /// case of this local type selects the same exception. Otherwise the usual
    /// diagnostic is safer than catching the wrong sibling.
    pub(super) fn fixed_testcase_failure_kind(&self, value: &ast::Expr) -> Option<u32> {
        let attr = value.as_attribute_expr()?;
        if attr.attr.as_str() != "failureException" {
            return None;
        }
        let name = attr.value.as_name_expr()?.id.as_str();
        let slot = *self.names.get(name)?;
        if !self.assigned.contains(&slot) {
            return None;
        }
        let Type::Class(base) = self.read_local(slot).ty else {
            return None;
        };
        if !self.unittest_case(base) {
            return None;
        }
        let mut matched = None;
        for (id, record) in self.classes.declarations.definitions.iter().enumerate() {
            if (id == base || record.bases.contains(&base)) && record.abstract_methods.is_empty() {
                let kind = self.unittest_category(value, Some((name, id)))?.0;
                if matched.is_some_and(|previous| previous != kind) {
                    return None;
                }
                matched = Some(kind);
            }
        }
        matched
    }

    /// Match a source or builtin category by identity. Names are used only to render
    /// CPython's failure message; the native handler matches the category and its subclasses.
    fn unittest_category(
        &self,
        value: &ast::Expr,
        this: Option<(&str, usize)>,
    ) -> Option<(u32, String, String)> {
        let mut definition = None;
        let mut alias = None;
        if let ast::Expr::Attribute(attr) = value {
            let records = &self.classes.declarations.definitions;
            let class = if let Some(name) = attr.value.as_name_expr()
                && let Some((receiver, actual)) = this
                && name.id.as_str() == receiver
            {
                Some(actual)
            } else if let Some(name) = attr.value.as_name_expr()
                && let Some(slot) = self.names.get(name.id.as_str())
                && self.assigned.contains(slot)
                && let Type::Class(class) = self.read_local(*slot).ty
            {
                // A different, upcast operand could have its own constructor override.
                (!self.classes.has_concrete_subclasses(class)).then_some(class)
            } else {
                self.class_reference(&attr.value)
            };
            alias =
                class.and_then(|class| records[class].exception_aliases.get(attr.attr.as_str()));
        }
        let kind = match alias {
            Some(crate::classes::ExceptionAlias::Builtin { kind, name }) => {
                return Some((*kind, name.clone(), name.clone()));
            }
            Some(crate::classes::ExceptionAlias::Named(def)) => {
                definition = Some(*def);
                self.classes.declarations.exceptions[def]
            }
            None => self.exception_category(value)?,
        };
        if let Some(id) = definition.or_else(|| match self.resolve(value) {
            Some(Binding::Definition(id)) => Some(id),
            _ => None,
        }) {
            let module = &self.graph.modules[id.module];
            let ast::Stmt::ClassDef(class) = &module.parsed.syntax().body[id.statement] else {
                return None;
            };
            let name = class.name.to_string();
            let qualified = format!(
                "{}.{}",
                if id.module == self.graph.entry {
                    "__main__"
                } else {
                    &module.name
                },
                name
            );
            return Some((kind, name, qualified));
        }
        if let Some(Binding::Intrinsic(symbol)) = self.resolve(value) {
            // These are CPython display names. Identity comes exclusively from
            // exception_category and the module export, including sparse ABI IDs.
            // A module supplied by another owner is unavailable until it ships.
            let (export, name) = [
                ("asyncio", "CancelledError"),
                ("io", "UnsupportedOperation"),
                ("subprocess", "SubprocessError"),
                ("subprocess", "CalledProcessError"),
                ("subprocess", "TimeoutExpired"),
                ("gzip", "BadGzipFile"),
                ("zipfile", "BadZipFile"),
                ("shutil", "Error"),
                ("shutil", "SameFileError"),
                ("shutil", "SpecialFileError"),
                ("shutil", "ReadError"),
                ("shutil", "RegistryError"),
                ("tarfile", "TarError"),
                ("tarfile", "FilterError"),
                ("tarfile", "OutsideDestinationError"),
                ("tarfile", "SpecialFileError"),
                ("tarfile", "AbsoluteLinkError"),
                ("tarfile", "LinkOutsideDestinationError"),
                ("zlib", "error"),
                ("tomllib", "TOMLDecodeError"),
                ("json", "JSONDecodeError"),
                ("csv", "Error"),
                ("socket", "gaierror"),
            ]
            .into_iter()
            .find(|(module, name)| symbol.exported_by(module, name))?;
            let origin = match export {
                "asyncio" => "asyncio.exceptions",
                "json" => "json.decoder",
                "csv" => "_csv",
                other => other,
            };
            return Some((kind, name.into(), format!("{origin}.{name}")));
        }
        let name = value.as_name_expr()?.id.as_str();
        (self.resolve(value).is_none()
            && !self.names.contains_key(name)
            && exception_kind(name) == Some(kind))
        .then(|| (kind, name.to_owned(), name.to_owned()))
    }

    /// Tuple entries use their class repr, including singleton commas and nested tuples.
    /// A non-class operand still has to run before unittest reports its TypeError.
    fn unittest_expected(
        &mut self,
        value: &ast::Expr,
        evaluated: &mut Vec<Statement>,
        this: Option<(&str, usize)>,
    ) -> Result<Option<Expected>, String> {
        if let ast::Expr::Tuple(tuple) = value {
            let mut kinds = Vec::new();
            let mut labels = Vec::new();
            let mut valid = true;
            for entry in &tuple.elts {
                if let Some(entry) = self.unittest_expected(entry, evaluated, this)? {
                    kinds.extend(entry.kinds);
                    labels.push(entry.repr);
                } else {
                    valid = false;
                }
            }
            let label = format!(
                "({}{})",
                labels.join(", "),
                if labels.len() == 1 { "," } else { "" }
            );
            return Ok(valid.then(|| Expected {
                kinds,
                label: label.clone(),
                repr: label,
            }));
        }
        if let Some((kind, name, qualified)) = self.unittest_category(value, this) {
            return Ok(Some(Expected {
                kinds: vec![kind],
                label: name,
                repr: format!("<class '{qualified}'>"),
            }));
        }
        // These bare class objects have no effects; fixed records can be classified
        // from their declaration without incorrectly constructing an instance.
        let builtin = value.as_name_expr().is_some_and(|name| {
            !self.names.contains_key(name.id.as_str())
                && self.resolve(value).is_none()
                && matches!(
                    name.id.as_str(),
                    "object"
                        | "type"
                        | "bool"
                        | "int"
                        | "float"
                        | "str"
                        | "bytes"
                        | "tuple"
                        | "list"
                        | "dict"
                        | "set"
                )
        });
        if builtin
            || matches!(self.resolve(value), Some(Binding::Definition(def)) if self.classes.declarations.ids.contains_key(&def))
        {
            return Ok(None);
        }
        let operand = self.expr(value)?;
        if matches!(operand.ty, Type::Tuple(_)) {
            return Err(error(
                self.source,
                value,
                "assertRaises requires statically known exception tuple members",
            ));
        }
        evaluated.push(Statement::Expr(operand));
        Ok(None)
    }

    pub(super) fn unittest_context(&self, class: usize) -> bool {
        let record = &self.classes.declarations.definitions[class];
        let module = &self.graph.modules[record.definition.module];
        record.name == "_AssertRaisesContext" && module.bundled && module.name == "unittest"
    }

    /// Construct an ordinary native record even when the caller stores it before
    /// entering. Arguments are evaluated before reporting errors from unittest.
    pub(super) fn unittest_assert_raises(
        &mut self,
        call: &ast::ExprCall,
        class: usize,
        receiver: &Expr,
        name: &str,
    ) -> Result<Option<Expr>, String> {
        if !self.unittest_case(class) {
            return Ok(None);
        }
        let regex = name == "assertRaisesRegex";
        if regex && call.arguments.args.len() > 2 {
            return Err(error(
                self.source,
                call,
                "callable assertRaisesRegex requires native callback argument forwarding",
            ));
        }
        if !regex && call.arguments.args.len() > 1 {
            return self
                .unittest_callable_raises(call, class, receiver)
                .map(Some);
        }
        let this = call
            .func
            .as_attribute_expr()
            .and_then(|a| a.value.as_name_expr())
            .map(|n| (n.id.as_str(), class));
        let source = self
            .graph
            .modules
            .iter()
            .find(|m| m.bundled && m.name == "unittest")
            .unwrap();
        let Binding::Definition(def) = source.bindings["_AssertRaisesContext"] else {
            unreachable!()
        };
        let context = self.classes.declarations.ids[&def];
        let init = &self.classes.methods[context]["__init__"];
        let (initializer, case_type) = (init.id, init.params[1].ty);
        self.calls.insert(initializer);
        let case = self.hidden(case_type);
        let mut body = vec![Statement::Assign(
            case,
            self.coerce(receiver.clone(), case_type, call)?,
        )];
        let mut clear = vec![case];
        let mut expected = call
            .arguments
            .args
            .first()
            .map(|value| self.unittest_expected(value, &mut body, this))
            .transpose()?;
        let mut pattern = if regex {
            call.arguments
                .args
                .get(1)
                .map(|arg| self.unittest_pattern(arg, &mut body, &mut clear))
                .transpose()?
        } else {
            None
        };
        let mut msg_slot = None;
        let mut unexpected = None;
        let mut duplicate = None;
        for keyword in &call.arguments.keywords {
            let key = keyword
                .arg
                .as_ref()
                .expect("unpacking rejected before method lookup")
                .as_str();
            match key {
                "expected_exception" => {
                    if expected.is_some() {
                        duplicate.get_or_insert(key);
                    }
                    expected = Some(self.unittest_expected(&keyword.value, &mut body, this)?);
                }
                "expected_regex" if regex => {
                    if pattern.is_some() {
                        duplicate.get_or_insert(key);
                    }
                    pattern = Some(self.unittest_pattern(&keyword.value, &mut body, &mut clear)?);
                }
                "msg" => {
                    let value = self.expr(&keyword.value)?;
                    let slot = self.hidden(value.ty);
                    body.push(Statement::Assign(slot, value));
                    clear.push(slot);
                    msg_slot = Some(slot);
                }
                _ => {
                    body.push(Statement::Expr(self.expr(&keyword.value)?));
                    unexpected.get_or_insert(key);
                }
            }
        }
        let binding_failed =
            duplicate.is_some() || expected.is_none() || regex && pattern.is_none();
        let error = if let Some(key) = duplicate {
            Some(format!(
                "TestCase.{name}() got multiple values for argument '{key}'"
            ))
        } else if regex && expected.is_none() && pattern.is_none() {
            Some(format!(
                "TestCase.{name}() missing 2 required positional arguments: 'expected_exception' and 'expected_regex'"
            ))
        } else if expected.is_none() {
            Some(format!(
                "TestCase.{name}() missing 1 required positional argument: 'expected_exception'"
            ))
        } else if regex && pattern.is_none() {
            Some(format!(
                "TestCase.{name}() missing 1 required positional argument: 'expected_regex'"
            ))
        } else if let Some((_, Some(error))) = pattern {
            Some(error.into())
        } else if expected.as_ref().is_some_and(Option::is_none) {
            Some(format!(
                "{name}() arg 1 must be an exception type or tuple of exception types"
            ))
        } else {
            unexpected
                .map(|key| format!("'{key}' is an invalid keyword argument for this function"))
        };
        let Expected { kinds, label, .. } = expected.flatten().unwrap_or_default();
        let optional = self.types.optional(Type::Str);
        let absent = || Expr {
            ty: Type::None,
            kind: ExprKind::None,
        };
        let msg = if let Some(slot) = msg_slot
            && error.is_none()
        {
            self.local(slot)
        } else {
            absent()
        };
        let msg = self.coerce(msg, optional, call)?;
        let error = self.coerce(error.map_or_else(absent, unpacking::string), optional, call)?;
        let kinds = Expr {
            ty: Type::ListInt,
            kind: ExprKind::List(
                kinds
                    .into_iter()
                    .map(|kind| pipelines::integer(kind.into()))
                    .collect(),
            ),
        };
        let mut pattern = pattern
            .and_then(|(slot, _)| slot)
            .filter(|_| !binding_failed);
        if let Some(slot) = pattern
            && self.local(slot).ty != Type::RegexPattern
        {
            // Python evaluates all arguments before re.compile inside unittest.
            // A failed signature binding never enters the assertion at all.
            let compiled = self.hidden(Type::RegexPattern);
            body.push(Statement::Assign(
                compiled,
                Expr {
                    ty: Type::RegexPattern,
                    kind: ExprKind::Builtin(
                        Builtin::UnittestCompilePattern,
                        vec![Self::string_value(self.local(slot))],
                    ),
                },
            ));
            clear.push(compiled);
            pattern = Some(compiled);
        }
        let spelling = pattern.map_or_else(
            || unpacking::string(""),
            |slot| Expr {
                ty: Type::Str,
                kind: ExprKind::Builtin(Builtin::RegexSpelling, vec![self.local(slot)]),
            },
        );
        let optional_pattern = self.types.optional(Type::RegexPattern);
        let pattern = self.coerce(
            pattern.map_or_else(absent, |slot| self.local(slot)),
            optional_pattern,
            call,
        )?;
        let value = Expr {
            ty: Type::Class(context),
            kind: ExprKind::Construct {
                initializer,
                args: ir::CallArgs::positional(vec![
                    self.local(case),
                    kinds,
                    unpacking::string(label),
                    msg,
                    error,
                    pattern,
                    spelling,
                ]),
            },
        };
        Ok(Some(Expr {
            ty: value.ty,
            kind: ExprKind::Scope {
                body,
                value: Box::new(value),
                clear,
            },
        }))
    }

    /// Evaluate the pattern once, before the exception context is entered. A
    /// compiled value retains its flags and original Python spelling.
    fn unittest_pattern(
        &mut self,
        arg: &ast::Expr,
        body: &mut Vec<Statement>,
        clear: &mut Vec<usize>,
    ) -> Result<(Option<usize>, Option<&'static str>), String> {
        let value = if matches!(arg, ast::Expr::StringLiteral(_)) {
            self.fixed_regex(arg, false)?
        } else {
            self.expr(arg)?
        };
        match value.ty {
            Type::RegexPattern | Type::Str | Type::StrLiteral(_) => {
                let slot = self.hidden(value.ty);
                body.push(Statement::Assign(slot, value));
                clear.push(slot);
                Ok((Some(slot), None))
            }
            Type::None => {
                body.push(Statement::Expr(value));
                Ok((None, None))
            }
            Type::Bytes => Err(error(
                self.source,
                arg,
                "native exception assertions do not support bytes regex patterns yet",
            )),
            _ => {
                body.push(Statement::Expr(value));
                Ok((
                    None,
                    Some("first argument must be string or compiled pattern"),
                ))
            }
        }
    }

    /// Capture a forwarded value before entering the callback's exception
    /// handler. Reads from these synthetic locals cannot repeat user code.
    fn unittest_argument(
        &mut self,
        arg: &ast::Expr,
        body: &mut Vec<Statement>,
        locals: &mut Vec<(String, usize)>,
        discarded: bool,
    ) -> Result<ast::Expr, String> {
        // A rejected call still constructs its arguments. An unused empty
        // container has no element from which to infer a storage layout.
        let value = match arg {
            ast::Expr::Dict(d) if discarded && d.items.is_empty() => {
                self.expr_expected(arg, self.types.dict(Type::Str, Type::Int))?
            }
            ast::Expr::List(l) if discarded && l.elts.is_empty() => {
                self.expr_expected(arg, Type::ListInt)?
            }
            _ => self.expr(arg)?,
        };
        let slot = self.hidden(value.ty);
        let name = format!("@unittest_arg{slot}");
        self.names.insert(name.clone(), slot);
        self.assigned.insert(slot);
        body.push(Statement::Assign(slot, value));
        locals.push((name.clone(), slot));
        Ok(ast::Expr::Name(ast::ExprName {
            node_index: ast::AtomicNodeIndex::NONE,
            range: arg.range(),
            id: name.into(),
            ctx: ast::ExprContext::Load,
        }))
    }

    /// Raise at the Python binding or invocation boundary, after its inputs have
    /// run. A regular call also gives expression scopes a normal cleanup edge.
    fn unittest_call_type_error(&mut self, message: String) -> Statement {
        let module = self
            .graph
            .modules
            .iter()
            .find(|m| m.bundled && m.name == "unittest")
            .unwrap();
        let Binding::Definition(def) = module.bindings["_raise_call_type_error"] else {
            unreachable!()
        };
        let id = self.functions[&def].id;
        self.calls.insert(id);
        Statement::Expr(Expr {
            ty: Type::None,
            kind: ExprKind::Call(
                id,
                ir::CallArgs::positional(vec![unpacking::string(message)]),
            ),
        })
    }

    /// Callable assertRaises catches the invocation only, after the case,
    /// category, bound receiver and all forwarded arguments have been evaluated.
    fn unittest_callable_raises(
        &mut self,
        call: &ast::ExprCall,
        class: usize,
        receiver: &Expr,
    ) -> Result<Expr, String> {
        let args = &call.arguments.args;
        if args.iter().any(ast::Expr::is_starred_expr) {
            return Err(error(
                self.source,
                call,
                "callable assertRaises requires explicit positional arguments",
            ));
        }
        let this = call
            .func
            .as_attribute_expr()
            .and_then(|a| a.value.as_name_expr())
            .map(|n| (n.id.as_str(), class));
        let case = self.hidden(receiver.ty);
        let mut body = vec![Statement::Assign(case, receiver.clone())];
        let expected = self.unittest_expected(&args[0], &mut body, this)?;
        let mut locals = Vec::new();
        let checked = (|| {
            let target = &args[1];
            let mut callee = target.clone();
            let lexical = target
                .as_name_expr()
                .is_some_and(|n| self.names.contains_key(n.id.as_str()));
            let binding = if lexical { None } else { self.resolve(target) };
            let mut noncallable = None;
            let mut binding_error = None;
            let mut lambda_invocation = None;
            let nargs = args.len() - 2;
            let keys = &call.arguments.keywords;
            let label = match binding {
                Some(Binding::Definition(def)) => {
                    match &self.graph.modules[def.module].parsed.syntax().body[def.statement] {
                        ast::Stmt::FunctionDef(f) => {
                            binding_error = call_binding_errors::arity_error(f, None, nargs, keys);
                            f.name.to_string()
                        }
                        ast::Stmt::ClassDef(c) => c.name.to_string(),
                        _ => {
                            return Err(error(
                                self.source,
                                target,
                                "assertRaises needs a known callable name",
                            ));
                        }
                    }
                }
                Some(Binding::Intrinsic(_)) => {
                    if let Some(attr) = target.as_attribute_expr() {
                        attr.attr.to_string()
                    } else {
                        let spelling = target
                            .as_name_expr()
                            .ok_or_else(|| {
                                error(self.source, target, "unknown intrinsic callback")
                            })?
                            .id
                            .as_str();
                        self.graph.modules[self.module]
                            .parsed
                            .syntax()
                            .body
                            .iter()
                            .find_map(|stmt| {
                                let ast::Stmt::ImportFrom(import) = stmt else {
                                    return None;
                                };
                                import
                                    .names
                                    .iter()
                                    .find(|a| {
                                        a.asname.as_ref().unwrap_or(&a.name).as_str() == spelling
                                    })
                                    .map(|a| a.name.to_string())
                            })
                            .unwrap_or_else(|| spelling.to_owned())
                    }
                }
                _ => match &mut callee {
                    ast::Expr::Attribute(attr) => 'method: {
                        let unbound = if attr
                            .value
                            .as_name_expr()
                            .is_some_and(|n| self.names.contains_key(n.id.as_str()))
                        {
                            None
                        } else if let Some(Binding::Definition(def)) = self.resolve(&attr.value) {
                            self.classes.declarations.ids.get(&def).copied()
                        } else {
                            None
                        };
                        let owner = if let Some(class) = unbound {
                            class
                        } else {
                            let root =
                                self.unittest_argument(&attr.value, &mut body, &mut locals, false)?;
                            let slot = locals.last().unwrap().1;
                            // Sealed poll methods have no property lookup or user callback.
                            if self.locals[slot] == Type::Poll
                                && matches!(
                                    attr.attr.as_str(),
                                    "poll" | "register" | "modify" | "unregister"
                                )
                            {
                                *attr.value = root;
                                break 'method attr.attr.to_string();
                            }
                            let Type::Class(owner) = self.locals[slot] else {
                                return Err(error(
                                    self.source,
                                    target,
                                    "assertRaises needs a fixed native method receiver",
                                ));
                            };
                            *attr.value = root;
                            owner
                        };
                        if self.classes.methods[owner]
                            .get(attr.attr.as_str())
                            .is_some_and(|s| {
                                s.method.is_some_and(|m| {
                                    m.kind(&self.classes.declarations)
                                        == crate::classes::MethodKind::Property
                                })
                            })
                        {
                            return Err(error(
                                self.source,
                                target,
                                "assertRaises needs the property to resolve its callable before the arguments",
                            ));
                        }
                        binding_error = self.assertion_callback_arity(
                            owner,
                            attr.attr.as_str(),
                            unbound.is_none(),
                            nargs,
                            keys,
                        );
                        attr.attr.to_string()
                    }
                    ast::Expr::Lambda(lambda) => {
                        if lambda.parameters.is_none() && nargs == 0 && keys.is_empty() {
                            // A direct zero-argument lambda needs no escaping closure.
                            // Read its captured slots when the protected invocation runs.
                            lambda_invocation = Some(self.pull_lambda(lambda)?);
                        } else {
                            callee =
                                self.unittest_argument(target, &mut body, &mut locals, false)?;
                        }
                        "<lambda>".into()
                    }
                    ast::Expr::Name(name) if !lexical && binding.is_none() => name.id.to_string(),
                    _ => {
                        callee = self.unittest_argument(target, &mut body, &mut locals, false)?;
                        let ty = self.locals[locals.last().unwrap().1];
                        if let Some(name) = self.stored_assertion_name(ty) {
                            name.to_owned()
                        } else {
                            let name = match ty {
                                Type::None => "NoneType",
                                Type::Bool => "bool",
                                Type::Int => "int",
                                Type::Float => "float",
                                Type::Str | Type::StrLiteral(_) => "str",
                                Type::Bytes => "bytes",
                                Type::List(_) | Type::ListInt => "list",
                                Type::Dict(_) => "dict",
                                Type::Set(_) => "set",
                                Type::Tuple(_) => "tuple",
                                _ => {
                                    return Err(error(
                                        self.source,
                                        target,
                                        "assertRaises needs a known function or bound method",
                                    ));
                                }
                            };
                            noncallable = Some(format!("'{name}' object is not callable"));
                            String::new()
                        }
                    }
                },
            };
            let mut forwarded = call.arguments.clone();
            let discarded = noncallable.is_some()
                || binding_error.is_some()
                || expected.is_none()
                || keys
                    .iter()
                    .any(|kw| kw.arg.as_deref() == Some("expected_exception"));
            forwarded.args = args[2..]
                .iter()
                .map(|arg| self.unittest_argument(arg, &mut body, &mut locals, discarded))
                .collect::<Result<_, _>>()?;
            let mut duplicate = false;
            for kw in &mut forwarded.keywords {
                if kw.arg.as_deref() == Some("expected_exception") {
                    self.unittest_expected(&kw.value, &mut body, this)?;
                    duplicate = true;
                } else {
                    kw.value =
                        self.unittest_argument(&kw.value, &mut body, &mut locals, discarded)?;
                }
            }
            if duplicate || expected.is_none() {
                let message = if duplicate {
                    "TestCase.assertRaises() got multiple values for argument 'expected_exception'"
                } else {
                    "assertRaises() arg 1 must be an exception type or tuple of exception types"
                };
                body.push(self.unittest_call_type_error(message.into()));
                return Ok(());
            }
            let expected = expected.unwrap();
            let action = if let Some(message) = noncallable.or(binding_error) {
                self.unittest_call_type_error(message)
            } else if let Some(invocation) = lambda_invocation {
                Statement::Expr(invocation)
            } else {
                Statement::Expr(self.call(&ast::ExprCall {
                    node_index: ast::AtomicNodeIndex::NONE,
                    range_start: call.range_start,
                    func: Box::new(callee),
                    arguments: forwarded,
                })?)
            };
            let missing = self.classes.methods[class]["_raise_missing_callable"].id;
            self.calls.insert(missing);
            let failure = Statement::Expr(Expr {
                ty: Type::None,
                kind: ExprKind::Call(
                    missing,
                    ir::CallArgs::positional(vec![
                        self.local(case),
                        unpacking::string(expected.label),
                        unpacking::string(label),
                    ]),
                ),
            });
            if expected.kinds.is_empty() {
                // An empty tuple never catches, unlike a bare except clause.
                body.extend([action, failure]);
            } else {
                body.push(Statement::Try {
                    body: vec![action],
                    handlers: vec![ir::ExceptHandler {
                        kinds: expected.kinds,
                        name: None,
                        body: vec![],
                    }],
                    orelse: vec![failure],
                    finalbody: vec![],
                });
            }
            Ok(())
        })();
        let mut clear = vec![case];
        for (name, slot) in locals {
            self.names.remove(&name);
            self.assigned.remove(&slot);
            clear.push(slot);
        }
        checked?;
        Ok(Expr {
            ty: Type::None,
            kind: ExprKind::Scope {
                body,
                clear,
                value: Box::new(Expr {
                    ty: Type::None,
                    kind: ExprKind::None,
                }),
            },
        })
    }

    /// A normal exit (including return, break and continue) must raise an
    /// assertion failure. A thrown exception is suppressed only if it matches.
    pub(super) fn assert_raises_scope(
        &mut self,
        item: &ast::WithItem,
        remaining: &[ast::WithItem],
        suite: &[ast::Stmt],
        class: usize,
        manager: Expr,
    ) -> Result<(Vec<Statement>, bool), String> {
        let root = self.hidden(manager.ty);
        let exceptional = self.hidden(Type::Bool);
        let flag = |value| Expr {
            ty: Type::Bool,
            kind: ExprKind::Bool(value),
        };
        let mut enter = vec![
            Statement::Assign(root, manager),
            Statement::Assign(exceptional, flag(false)),
        ];
        if let Some(target) = &item.optional_vars {
            if target.as_name_expr().is_none() {
                return Err(error(
                    self.source,
                    target.as_ref(),
                    "assertRaises targets must be local names",
                ));
            }
            enter.push(self.assign(target, self.local(root))?);
        }
        let incoming = self.local_state();
        let earlier_breaks = self.take_breaks();
        let (protected, _) = self.with_items(remaining, suite)?;
        self.take_breaks();
        self.include_breaks(earlier_breaks);
        self.restore_state(incoming);
        if !remaining.is_empty() {
            self.narrowed = assignments::Narrowed::new();
        }
        for name in narrowing::assigned_names(suite) {
            if let Some(slot) = self.names.get(&name) {
                self.narrowed.remove_mut(slot);
            }
        }
        for name in narrowing::deleted_names(suite) {
            if let Some(slot) = self.names.get(&name) {
                self.assigned.remove(slot);
            }
        }
        let fields = &self.classes.declarations.definitions[class].fields;
        let index = |name| fields.iter().position(|f| f.name == name).unwrap();
        let expected = Expr {
            ty: Type::ListInt,
            kind: ExprKind::Field(Box::new(self.local(root)), index("_expected")),
        };
        let caught = index("_caught");
        let exc = self.hidden(Type::Exception);
        let matches = Expr {
            ty: Type::Bool,
            kind: ExprKind::Builtin(
                Builtin::ExceptionMatchesAny,
                vec![self.local(exc), expected],
            ),
        };
        let store = Statement::SetField(
            self.local(root),
            caught,
            self.coerce(self.local(exc), self.classes.fields[class][caught], item)?,
        );
        let missing = self.classes.methods[class]["_missing"].id;
        let check = self.classes.methods[class]["_check_regex"].id;
        self.calls.insert(check);
        let search = Statement::Expr(Expr {
            ty: Type::None,
            kind: ExprKind::Call(
                check,
                ir::CallArgs::positional(vec![self.local(root), self.local(exc)]),
            ),
        });
        self.calls.insert(missing);
        let fail = Statement::Expr(Expr {
            ty: Type::None,
            kind: ExprKind::Call(missing, ir::CallArgs::positional(vec![self.local(root)])),
        });
        enter.push(Statement::Try {
            body: protected,
            handlers: vec![ir::ExceptHandler {
                kinds: vec![],
                name: Some(exc),
                body: vec![
                    Statement::Assign(exceptional, flag(true)),
                    Statement::If(matches, vec![store, search], vec![Statement::Raise(None)]),
                ],
            }],
            orelse: vec![],
            finalbody: vec![Statement::If(self.local(exceptional), vec![], vec![fail])],
        });
        Ok((
            vec![Statement::Scope {
                body: enter,
                clear: vec![root, exceptional],
            }],
            true,
        ))
    }
}
