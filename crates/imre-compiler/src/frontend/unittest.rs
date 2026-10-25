//! Native unittest scopes use the same exception hierarchy and cleanup edges as try/finally.

use super::*;

#[derive(Default)]
struct Expected {
    kinds: Vec<u32>,
    label: String,
    repr: String,
}

impl Checker<'_> {
    fn unittest_case(&self, class: usize) -> bool {
        let classes = &self.classes.declarations.definitions;
        std::iter::once(&class)
            .chain(&classes[class].bases)
            .any(|&index| {
                let record = &classes[index];
                let module = &self.graph.modules[record.definition.module];
                record.name == "TestCase" && module.bundled && module.name == "unittest"
            })
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
                ("zlib", "error"),
                ("tomllib", "TOMLDecodeError"),
                ("json", "JSONDecodeError"),
                ("csv", "Error"),
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
    ) -> Result<Option<Expr>, String> {
        if !self.unittest_case(class) || call.arguments.args.len() > 1 {
            return Ok(None);
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
        let mut msg_slot = None;
        let mut unexpected = None;
        let mut duplicate = false;
        for keyword in &call.arguments.keywords {
            let key = keyword
                .arg
                .as_ref()
                .expect("unpacking rejected before method lookup")
                .as_str();
            match key {
                "expected_exception" => {
                    duplicate = expected.is_some();
                    expected = Some(self.unittest_expected(&keyword.value, &mut body, this)?);
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
        let error = if duplicate {
            Some(
                "TestCase.assertRaises() got multiple values for argument 'expected_exception'"
                    .into(),
            )
        } else if expected.is_none() {
            Some("TestCase.assertRaises() missing 1 required positional argument: 'expected_exception'".into())
        } else if expected.as_ref().is_some_and(Option::is_none) {
            Some(
                "assertRaises() arg 1 must be an exception type or tuple of exception types".into(),
            )
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
                    Statement::If(matches, vec![store], vec![Statement::Raise(None)]),
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
