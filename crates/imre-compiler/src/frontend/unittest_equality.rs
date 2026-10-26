//! Select TestCase's equality routine for the concrete operand type. Arguments
//! still use the regular source-order call binder, including subclass overrides.

use super::*;

impl Checker<'_> {
    /// Resolve a membership assertion from its container. The ordinary argument
    /// binder still evaluates the receiver, member, container, and message in
    /// source order, including when they are supplied by keyword.
    pub(super) fn unittest_membership(
        &mut self,
        call: &ast::ExprCall,
        name: &str,
        class: usize,
        receiver: &Expr,
    ) -> Result<Option<Expr>, String> {
        if !self.unittest_case(class) {
            return Ok(None);
        }
        if let Some(signature) = self.classes.methods[class].get(name) {
            let method = signature.method.unwrap();
            let declarations = &self.classes.declarations;
            let origin = declarations.definitions[method.class].method_origins[method.index];
            let owner = &declarations.definitions[declarations.ids[&origin]];
            let module = &self.graph.modules[origin.module];
            if owner.name != "TestCase" || !module.bundled || module.name != "unittest" {
                return Ok(None);
            }
        }
        let operand = |i: usize, key: &str| {
            call.arguments.args.get(i).or_else(|| {
                call.arguments
                    .keywords
                    .iter()
                    .find(|kw| kw.arg.as_deref() == Some(key))
                    .map(|kw| &kw.value)
            })
        };
        let container = operand(1, "container");
        let method = match container {
            Some(ast::Expr::List(list)) if list.elts.is_empty() => {
                let Some(member) = operand(0, "member") else {
                    return Ok(None);
                };
                match self.expr(member)?.ty {
                    Type::Str | Type::StrLiteral(_) => "_assertTextListMembership",
                    Type::Int => "_assertIntListMembership",
                    _ => {
                        return Err(error(
                            self.source,
                            member,
                            "unsupported native unittest list member type",
                        ));
                    }
                }
            }
            Some(ast::Expr::Dict(dict)) if dict.items.is_empty() => {
                let text = match operand(0, "member") {
                    Some(member) => {
                        matches!(self.expr(member)?.ty, Type::Str | Type::StrLiteral(_))
                    }
                    None => false,
                };
                if text {
                    "_assertTextDictMembership"
                } else {
                    "_assertIntKeyMembership"
                }
            }
            Some(node) => {
                let ty = self.expr(node)?.ty;
                match ty {
                    Type::Str | Type::StrLiteral(_) => "_assertTextMembership",
                    Type::ListInt => "_assertIntListMembership",
                    _ if self.types.element(ty) == Some(Type::Str) => "_assertTextListMembership",
                    _ if self.types.mapping(ty) == Some((Type::Str, Type::Str)) => {
                        "_assertTextDictMembership"
                    }
                    _ if self.types.mapping(ty) == Some((Type::Int, Type::Str)) => {
                        "_assertIntKeyMembership"
                    }
                    _ => {
                        return Err(error(
                            self.source,
                            node,
                            "unsupported native unittest membership container",
                        ));
                    }
                }
            }
            None => "_assertTextMembership",
        };
        let signature = self.classes.methods[class][method].clone();
        let mut args = self.arguments(
            call,
            &signature.params[1..signature.params.len() - 1],
            Some(receiver.clone()),
        )?;
        args.order.push(args.values.len());
        args.values.push(Expr {
            ty: Type::Bool,
            kind: ExprKind::Bool(name == "assertIn"),
        });
        self.calls.insert(signature.id);
        Ok(Some(Expr {
            ty: Type::None,
            kind: ExprKind::Call(signature.id, args),
        }))
    }

    pub(super) fn unittest_equality(
        &mut self,
        call: &ast::ExprCall,
        name: &str,
        class: usize,
        receiver: &Expr,
    ) -> Result<Option<Expr>, String> {
        if !self.unittest_case(class) || self.classes.methods[class].contains_key(name) {
            return Ok(None);
        }
        let (a, b) = if name == "assertListEqual" {
            ("list1", "list2")
        } else {
            ("first", "second")
        };
        let operand = |i: usize, key: &str| {
            call.arguments.args.get(i).or_else(|| {
                call.arguments
                    .keywords
                    .iter()
                    .find(|kw| kw.arg.as_deref() == Some(key))
                    .map(|kw| &kw.value)
            })
        };
        let left = operand(0, a);
        let right = operand(1, b);
        // Empty container literals take their element types from the other operand.
        let empty = |arg: &ast::Expr| {
            matches!(arg, ast::Expr::List(list) if list.elts.is_empty())
                || matches!(arg, ast::Expr::Dict(dict) if dict.items.is_empty())
        };
        let ty = if let Some(expr) = left
            .filter(|e| !empty(e))
            .or_else(|| right.filter(|e| !empty(e)))
        {
            self.expr(expr)?.ty
        } else if name != "assertListEqual" && matches!(left, Some(ast::Expr::Dict(_))) {
            self.types.dict(Type::Str, Type::Str)
        } else if name == "assertListEqual" || left.is_some_and(empty) {
            Type::ListInt
        } else {
            Type::Int
        };
        let method = if matches!(ty, Type::List(_) | Type::ListInt) {
            if self.classes.methods[class].contains_key("assertListEqual") {
                "assertListEqual"
            } else {
                match self.types.element(ty) {
                    Some(Type::Str) => "_assertTextListEqual",
                    Some(Type::Bytes) => "_assertBytesListEqual",
                    Some(Type::Int) => "_assertIntListEqual",
                    _ => {
                        return Err(error(
                            self.source,
                            call,
                            "unsupported list element type for unittest equality",
                        ));
                    }
                }
            }
        } else if name == "assertListEqual" {
            return Err(error(
                self.source,
                call,
                "native assertListEqual expects list operands",
            ));
        } else if self.types.mapping(ty) == Some((Type::Str, Type::Str)) {
            "assertDictEqual"
        } else if matches!(ty, Type::Str | Type::StrLiteral(_)) {
            "assertMultiLineEqual"
        } else if matches!(
            ty,
            Type::Calendar(ir::CalendarType::Date | ir::CalendarType::DateTime)
        ) {
            "_assertDateEqual"
        } else if ty == Type::Calendar(ir::CalendarType::Delta) {
            "_assertDeltaEqual"
        } else {
            "_baseAssertEqual"
        };
        let mut signature = self.classes.methods[class]
            .get(method)
            .cloned()
            .ok_or_else(|| {
                error(
                    self.source,
                    call,
                    &format!("native TestCase equality method {method} is unavailable"),
                )
            })?;
        signature.params[1].name = a.into();
        signature.params[2].name = b.into();
        self.source_call(call, &signature, Some(receiver.clone()), 1)
            .map(Some)
    }
}
