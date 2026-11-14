//! Prepare the operands evaluated by function, class, and module annotation providers.

use ruff_python_ast as ast;
use ruff_python_parser::Parsed;
use ruff_text_size::{Ranged, TextRange, TextSize};
use std::collections::HashMap;

use super::registered::values::Region;
use super::{
    RegisteredOperand, RegisteredScope, RegisteredSource, annotation_expression,
    annotation_spelling,
};
use crate::ir::{Builtin, Expr, ExprKind, Type};

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

/// Prepare an annotation at its declaration. Only a linked string child needs a site identity.
pub(super) fn annotation_operand<R>(
    expr: &ast::Expr,
    module: bool,
    source: RegisteredSource<'_>,
    string_annotations: bool,
    bundled: bool,
    children: &mut HashMap<TextSize, AnnotationBody<R>>,
    prepare_child: impl FnOnce(&str, &Parsed<ast::ModExpression>) -> Option<AnnotationBody<R>>,
) -> Option<Expr> {
    let text = if string_annotations {
        annotation_spelling(expr)
    } else if let ast::Expr::StringLiteral(literal) = expr
        // Ruff substitutes U+FFFD for surrogates. Only this literal's own
        // parts can require the code-point-preserving operand below.
        && !literal
            .value
            .iter()
            .any(|part| source.replacements.contains_key(&part.range()))
    {
        literal.value.to_str().to_owned()
    } else {
        return if module {
            Region::Suite.operand(expr, source)
        } else {
            checked_annotation_value(expr, |input| Region::Suite.operand(input, source))
        };
    };
    // A postponed name already has its spelling and needs no executable child.
    let body = if bundled || (string_annotations && expr.is_name_expr()) {
        None
    } else {
        let expanded;
        let expression = if imre_ast::forward_refs::is_starred(&text) {
            expanded = imre_ast::forward_refs::starred_expression(&text);
            expanded.as_str()
        } else {
            &text
        };
        crate::normalize_source(expression, crate::SourceKind::Command)
            .ok()
            .and_then(|source| {
                let parsed = annotation_expression(&source)?;
                prepare_child(&source, &parsed)
            })
    };
    let kind = if let Some(child) = body {
        children.insert(expr.start(), child);
        ExprKind::Builtin(
            Builtin::BindAnnotationExpression {
                site: expr.start(),
                spelling: text,
            },
            Vec::new(),
        )
    } else {
        ExprKind::Str(text)
    };
    Some(Expr {
        ty: Type::Str,
        kind,
    })
}

#[cfg(test)]
pub(super) fn test_annotation_body(
    text: &str,
    parsed: &Parsed<ast::ModExpression>,
) -> Option<AnnotationBody<()>> {
    let source = RegisteredSource {
        python_optimization: 0,
        operand_types: None,
        text,
        replacements: &HashMap::new(),
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

    struct ObservedOperands {
        values: HashMap<TextSize, Expr>,
        children: HashMap<TextSize, AnnotationBody<()>>,
    }

    fn observe(
        stmts: &[ast::Stmt],
        source: RegisteredSource<'_>,
        postponed: bool,
        bundled: bool,
    ) -> ObservedOperands {
        let mut names = imre_ast::Names::default();
        names.string_annotations = postponed;
        let mut observed = ObservedOperands {
            values: HashMap::new(),
            children: HashMap::new(),
        };
        let (_, children) =
            super::super::function_scope::ScopeFacts::prepare(stmts, &names, |_, _| {}, |_| {});
        super::super::registered::collect_sites(
            (stmts, children),
            &names,
            source,
            |expr, module| {
                let value = annotation_operand(
                    expr,
                    module,
                    source,
                    postponed,
                    bundled,
                    &mut observed.children,
                    test_annotation_body,
                );
                if let Some(value) = &value {
                    assert!(
                        observed
                            .values
                            .insert(expr.start(), value.clone())
                            .is_none()
                    );
                }
                value
            },
        );
        observed
    }

    #[test]
    fn prepare_string_children_in_their_declaring_scope() {
        let text = "class C:\n    field: '(lambda: 1)()'\n    def f(self, arg: 'list[int]') -> 'Name':\n        local: 'skip[T]'\n        async def nested(arg: 'list[int]'): pass\n";
        let parsed = ruff_python_parser::parse_module(text).unwrap();
        let source = RegisteredSource {
            python_optimization: 0,
            operand_types: None,
            text,
            replacements: &HashMap::new(),
            method_imports: &Default::default(),
            private_names: None,
            template_fields: None,
            generators: None,
            lambdas: None,
        };
        let sites = observe(&parsed.syntax().body, source, false, false);
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
    fn quoted_children_use_their_own_literal_parts() {
        let text = r#"def f(a: "call" "(x)", b: "keep" "\ud800", c: r"'\ud800'"): pass
def dormant(): return f"\ud801{1}", t"{x}"
"#;
        let parsed = ruff_python_parser::parse_module(text).unwrap();
        let replacements = crate::frontend::literal_replacements(text, parsed.tokens());
        let source = RegisteredSource {
            python_optimization: 0,
            operand_types: None,
            text,
            replacements: &replacements,
            method_imports: &Default::default(),
            private_names: None,
            template_fields: None,
            generators: None,
            lambdas: None,
        };
        let sites = observe(&parsed.syntax().body, source, false, false);
        let site = TextSize::try_from(text.find(r#""call""#).unwrap()).unwrap();
        assert_eq!(sites.children.len(), 2);
        assert!(sites.children.contains_key(&site));
        let quoted = TextSize::try_from(text.find("r\"").unwrap()).unwrap();
        assert!(sites.children.contains_key(&quoted));
        assert!(matches!(
            &sites.values[&site].kind,
            ExprKind::Builtin(Builtin::BindAnnotationExpression { spelling, .. }, _) if spelling == "call(x)"
        ));
    }

    #[test]
    fn prepare_lazy_function_and_class_values_in_the_declaring_source() {
        let text = "class C:\n    field: mark(1)\n    deferred: (lambda: 2)\n    def f(self, *args: *items) -> token: pass\n";
        let parsed = ruff_python_parser::parse_module(text).unwrap();
        let source = RegisteredSource {
            python_optimization: 0,
            operand_types: None,
            text,
            replacements: &HashMap::new(),
            method_imports: &Default::default(),
            private_names: None,
            template_fields: None,
            generators: None,
            lambdas: None,
        };
        let sites = observe(&parsed.syntax().body, source, false, false);
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
    def f(self, arg: E[F], *rest: *Items) -> None:
        local: G[H]
        class Nested:
            field: I[J]
after: K[L]
";
        let parsed = ruff_python_parser::parse_module(text).unwrap();
        let source = RegisteredSource {
            python_optimization: 0,
            operand_types: None,
            text,
            replacements: &HashMap::new(),
            method_imports: &Default::default(),
            private_names: None,
            template_fields: None,
            generators: None,
            lambdas: None,
        };
        let site = |spelling| TextSize::try_from(text.find(spelling).unwrap()).unwrap();
        for bundled in [false, true] {
            let sites = observe(&parsed.syntax().body, source, true, bundled);
            assert_eq!(sites.values.len(), 7);
            assert_eq!(sites.children.len(), if bundled { 0 } else { 7 });
            for spelling in ["A[B]", "C[D]", "E[F]", "*Items", "None", "I[J]", "K[L]"] {
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
