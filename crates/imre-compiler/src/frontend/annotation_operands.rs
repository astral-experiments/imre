//! Prepare the operands evaluated by function, class, and module annotation providers.

use std::collections::HashMap;

use ruff_python_ast as ast;
use ruff_python_ast::statement_visitor::{StatementVisitor, walk_stmt};
use ruff_python_parser::Parsed;
use ruff_text_size::{Ranged, TextRange, TextSize};

use super::registered::values::Region;
use super::{
    RegisteredOperand, RegisteredScope, RegisteredSource, annotation_expression,
    annotation_spelling, parameter_annotations,
};
use crate::ir::{Builtin, Expr, ExprKind, Type};

pub(crate) struct AnnotationOperands<R> {
    pub(crate) values: HashMap<TextSize, Expr>,
    pub(crate) children: HashMap<TextSize, AnnotationBody<R>>,
}

pub(crate) struct AnnotationBody<R> {
    pub(crate) code: AnnotationCode,
    pub(crate) scope: R,
}

pub(crate) struct AnnotationCode {
    pub(crate) range: TextRange,
    pub(crate) operand: Box<RegisteredOperand>,
}

impl AnnotationCode {
    pub(crate) fn prepare(expr: &ast::Expr, source: RegisteredSource<'_>) -> Option<Self> {
        let operand = RegisteredOperand::prepare(
            expr,
            source,
            RegisteredScope::Function { is_async: false },
        )?;
        Some(Self {
            range: expr.range(),
            operand: Box::new(operand),
        })
    }
}

type PrepareChild<'a, R> =
    dyn Fn(&str, &Parsed<ast::ModExpression>) -> Option<AnnotationBody<R>> + 'a;

pub(super) fn checked_annotation_value(
    expr: &ast::Expr,
    mut operand: impl FnMut(&ast::Expr) -> Option<Expr>,
) -> Option<Expr> {
    match expr {
        ast::Expr::Starred(starred) => operand(&starred.value).map(unpack_annotation),
        _ => operand(expr),
    }
}

/// A starred annotation must yield exactly one type.
pub(super) fn unpack_annotation(value: Expr) -> Expr {
    let items = Expr {
        ty: Type::PythonObject,
        kind: ExprKind::Builtin(
            Builtin::RegisteredUnpack {
                count: 1,
                star: None,
                target: None,
            },
            vec![value],
        ),
    };
    Expr {
        ty: Type::PythonObject,
        kind: ExprKind::Builtin(Builtin::ListGet(None), vec![items, Expr::integer(0)]),
    }
}

/// Prepare each operand in its declaring source and key it by its expression’s byte offset.
pub(crate) fn annotation_operands<R>(
    stmts: &[ast::Stmt],
    string_annotations: bool,
    source: RegisteredSource<'_>,
    bundled: bool,
    prepare_child: impl Fn(&str, &Parsed<ast::ModExpression>) -> Option<AnnotationBody<R>>,
) -> AnnotationOperands<R> {
    #[derive(Clone, Copy)]
    enum Scope {
        Module,
        Class,
        Function,
    }

    struct Strings<'a, R> {
        operands: AnnotationOperands<R>,
        prepare_child: &'a PrepareChild<'a, R>,
        source: RegisteredSource<'a>,
        string_annotations: bool,
        bundled: bool,
        scope: Scope,
    }

    impl<R> Strings<'_, R> {
        fn record(&mut self, expr: &ast::Expr, module: bool) {
            let text = if self.string_annotations {
                annotation_spelling(expr)
            } else if let ast::Expr::StringLiteral(literal) = expr
                && !self.source.has_templates
                && self.source.replacements.is_empty()
            {
                literal.value.to_str().to_owned()
            } else {
                let value = if module {
                    Region::Suite.operand(expr, self.source)
                } else {
                    checked_annotation_value(expr, |input| {
                        Region::Suite.operand(input, self.source)
                    })
                };
                if let Some(value) = value {
                    self.operands.values.insert(expr.start(), value);
                }
                return;
            };
            let body = if self.bundled {
                None
            } else {
                let expanded;
                let expression = if text
                    .trim_start_matches(imre_ast::strings::python_whitespace)
                    .starts_with('*')
                {
                    expanded = imre_ast::forward_refs::starred_expression(&text);
                    expanded.as_str()
                } else {
                    &text
                };
                annotation_expression(expression)
                    .and_then(|parsed| (self.prepare_child)(expression, &parsed))
            };
            let site = expr.start();
            let kind = match body {
                Some(body) => {
                    self.operands.children.insert(site, body);
                    ExprKind::Builtin(
                        Builtin::BindAnnotationExpression {
                            site,
                            spelling: text,
                        },
                        Vec::new(),
                    )
                }
                None => ExprKind::Str(text),
            };
            self.operands.values.insert(
                site,
                Expr {
                    ty: Type::Str,
                    kind,
                },
            );
        }

        fn nested(&mut self, stmts: &[ast::Stmt], scope: Scope) {
            let enclosing = std::mem::replace(&mut self.scope, scope);
            self.visit_body(stmts);
            self.scope = enclosing;
        }
    }

    impl<'a, R> StatementVisitor<'a> for Strings<'_, R> {
        fn visit_stmt(&mut self, stmt: &'a ast::Stmt) {
            match stmt {
                ast::Stmt::FunctionDef(def) => {
                    for (_, annotation) in
                        parameter_annotations(&def.parameters, def.returns.as_deref())
                    {
                        self.record(annotation, false);
                    }
                    self.nested(&def.body, Scope::Function);
                }
                ast::Stmt::ClassDef(class) => {
                    self.nested(&class.body, Scope::Class);
                }
                ast::Stmt::AnnAssign(ann) if ann.simple && ann.target.is_name_expr() => {
                    match self.scope {
                        Scope::Module => self.record(&ann.annotation, true),
                        Scope::Class => self.record(&ann.annotation, false),
                        Scope::Function => {}
                    }
                }
                _ => walk_stmt(self, stmt),
            }
        }
    }

    let mut strings = Strings {
        operands: AnnotationOperands {
            values: HashMap::new(),
            children: HashMap::new(),
        },
        prepare_child: &prepare_child,
        source,
        string_annotations,
        bundled,
        scope: Scope::Module,
    };
    strings.visit_body(stmts);
    strings.operands
}

#[cfg(test)]
pub(super) fn test_annotation_body(
    text: &str,
    parsed: &Parsed<ast::ModExpression>,
) -> Option<AnnotationBody<()>> {
    let source = RegisteredSource {
        operand_types: None,
        text,
        replacements: &HashMap::new(),
        has_templates: false,
        template_fields: None,
        generators: None,
        lambdas: None,
        method_imports: &Default::default(),
        private_names: None,
    };
    Some(AnnotationBody {
        code: AnnotationCode::prepare(&parsed.syntax().body, source)?,
        scope: (),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prepare_string_children_in_their_declaring_scope() {
        let text = "class C:\n    field: '(lambda: 1)()'\n    def f(self, arg: 'list[int]') -> 'Name':\n        local: 'skip[T]'\n        async def nested(arg: 'list[int]'): pass\n";
        let parsed = ruff_python_parser::parse_module(text).unwrap();
        let source = RegisteredSource {
            operand_types: None,
            text,
            replacements: &HashMap::new(),
            has_templates: false,
            method_imports: &Default::default(),
            private_names: None,
            template_fields: None,
            generators: None,
            lambdas: None,
        };
        let sites = annotation_operands(
            &parsed.syntax().body,
            false,
            source,
            false,
            test_annotation_body,
        );
        assert_eq!(sites.values.len(), 4);
        for (start, _) in text.match_indices("'list[int]'") {
            let range = sites.children[&TextSize::try_from(start).unwrap()]
                .code
                .range;
            assert_eq!(range.start().to_u32(), 0);
            assert_eq!(range.end().to_u32(), 9);
        }
        let site = |token| TextSize::try_from(text.find(token).unwrap()).unwrap();
        assert!(sites.children.contains_key(&site("'(lambda")));
        assert!(
            matches!(&sites.values[&site("'Name'")].kind, ExprKind::Str(text) if text == "Name")
        );
        assert!(!sites.children.contains_key(&site("'Name'")));
        assert!(!sites.values.contains_key(&site("'skip")));
    }

    #[test]
    fn prepare_lazy_function_and_class_values_in_the_declaring_source() {
        let text = "class C:\n    field: mark(1)\n    deferred: (lambda: 2)\n    def f(self, *args: *items) -> token: pass\n";
        let parsed = ruff_python_parser::parse_module(text).unwrap();
        let source = RegisteredSource {
            operand_types: None,
            text,
            replacements: &HashMap::new(),
            has_templates: false,
            method_imports: &Default::default(),
            private_names: None,
            template_fields: None,
            generators: None,
            lambdas: None,
        };
        let sites = annotation_operands(
            &parsed.syntax().body,
            false,
            source,
            false,
            test_annotation_body,
        );
        let offset = |s| TextSize::try_from(text.find(s).unwrap()).unwrap();
        assert!(sites.children.is_empty());
        for spelling in ["mark(1)", "token", "lambda"] {
            assert!(sites.values.contains_key(&offset(spelling)));
        }
        let ExprKind::Builtin(Builtin::ListGet(_), items) = &sites.values[&offset("*items")].kind
        else {
            panic!("expected an unpacked annotation");
        };
        assert!(matches!(
            items[0].kind,
            ExprKind::Builtin(
                Builtin::RegisteredUnpack {
                    count: 1,
                    star: None,
                    target: None
                },
                _
            )
        ));
    }

    #[test]
    fn future_operands_require_a_child_only_when_their_spelling_is_compiled() {
        let text = "mod_field: A[B]
@decorator
class C:
    field: C[D]
    def f(self, arg: E[F], *rest: *Items):
        local: G[H]
        class Nested:
            field: I[J]
after: K[L]
";
        let parsed = ruff_python_parser::parse_module(text).unwrap();
        let source = RegisteredSource {
            operand_types: None,
            text,
            replacements: &HashMap::new(),
            has_templates: false,
            method_imports: &Default::default(),
            private_names: None,
            template_fields: None,
            generators: None,
            lambdas: None,
        };
        let site = |spelling| TextSize::try_from(text.find(spelling).unwrap()).unwrap();
        for bundled in [false, true] {
            let sites = annotation_operands(
                &parsed.syntax().body,
                true,
                source,
                bundled,
                test_annotation_body,
            );
            assert_eq!(sites.values.len(), 6);
            assert_eq!(sites.children.len(), if bundled { 0 } else { 6 });
            for spelling in ["A[B]", "C[D]", "E[F]", "*Items", "I[J]", "K[L]"] {
                let location = site(spelling);
                let value = &sites.values[&location].kind;
                if bundled {
                    assert!(matches!(value, ExprKind::Str(text) if text == spelling));
                } else {
                    assert!(
                        matches!(value, ExprKind::Builtin(Builtin::BindAnnotationExpression { site, spelling: original }, _) if *site == location && original == spelling)
                    );
                }
            }
            assert!(!sites.values.contains_key(&site("G[H]")));
        }
    }
}
