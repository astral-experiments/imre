//! Bind public Fraction constructors to their exact scalar conversion.
use super::*;

impl Checker<'_> {
    pub(super) fn fraction_class(&self, class: usize) -> bool {
        let decl = &self.classes.declarations.definitions[class];
        self.graph.modules[decl.definition.module].is_bundled(crate::modules::FRACTION_CLASS)
    }

    pub(super) fn fraction_constructor(
        &mut self,
        call: &ast::ExprCall,
        class: usize,
    ) -> Result<Expr, String> {
        let (numerator, denominator) = match (
            call.arguments.args.as_ref(),
            call.arguments.keywords.as_ref(),
        ) {
            ([], []) => (None, None),
            ([n], []) => (Some(n), None),
            ([n, d], []) => (Some(n), Some(d)),
            ([], [n]) if n.arg.as_deref() == Some("numerator") => (Some(&n.value), None),
            ([], [d]) if d.arg.as_deref() == Some("denominator") => (None, Some(&d.value)),
            ([n], [d]) if d.arg.as_deref() == Some("denominator") => (Some(n), Some(&d.value)),
            ([], [n, d])
                if n.arg.as_deref() == Some("numerator")
                    && d.arg.as_deref() == Some("denominator") =>
            {
                (Some(&n.value), Some(&d.value))
            }
            _ => return Err(self.object_builtin_error("Fraction constructor", call)),
        };
        let int = |n: i64| Expr {
            ty: Type::Int,
            kind: ExprKind::Int(n.to_string()),
        };
        let n = if let Some(n) = numerator {
            self.expr(n)?
        } else {
            int(0)
        };
        let d = if let Some(d) = denominator {
            Some(self.expr(d)?)
        } else {
            None
        };
        let integer = |expr: Expr| {
            if expr.ty == Type::Bool {
                Expr {
                    ty: Type::Int,
                    kind: ExprKind::Builtin(Builtin::Int, vec![expr]),
                }
            } else {
                expr
            }
        };
        let (helper, args) = match (n.ty, d) {
            (Type::Int | Type::Bool, None) => {
                let initializer = self.classes.methods[class]["__init__"].id;
                self.calls.insert(initializer);
                return Ok(Expr {
                    ty: Type::Class(class),
                    kind: ExprKind::Construct {
                        initializer,
                        args: ir::CallArgs::positional(vec![integer(n), int(1)]),
                    },
                });
            }
            (Type::Int | Type::Bool, Some(d)) if matches!(d.ty, Type::Int | Type::Bool) => {
                ("_normalized", vec![integer(n), integer(d)])
            }
            (Type::Str, None) => ("_from_text", vec![n]),
            (Type::Class(id), None) if id == class => ("_copy", vec![n]),
            _ => return Err(self.object_builtin_error("Fraction numeric protocol", call)),
        };
        let module = self.classes.declarations.definitions[class]
            .definition
            .module;
        let Binding::Definition(def) = self.graph.modules[module].bindings[helper] else {
            unreachable!()
        };
        let Some(signature) = self.functions.get(&def) else {
            return Err(self.object_builtin_error("Fraction conversion", call));
        };
        let id = signature.id;
        self.calls.insert(id);
        Ok(Expr {
            ty: Type::Class(class),
            kind: ExprKind::Call(id, ir::CallArgs::positional(args)),
        })
    }
}
