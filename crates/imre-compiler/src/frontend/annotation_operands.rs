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

pub(crate) enum PreparedAnnotation<R> {
    String {
        text: String,
        body: Option<AnnotationBody<R>>,
    },
    Value(Expr),
}

impl<R> PreparedAnnotation<R> {
    pub(super) fn operand(&self, site: TextSize) -> Expr {
        match self {
            Self::Value(value) => value.clone(),
            Self::String { text, body } => Expr {
                ty: Type::Str,
                kind: if body.is_some() {
                    ExprKind::Builtin(Builtin::BindAnnotationExpression(site), Vec::new())
                } else {
                    ExprKind::Str(text.clone())
                },
            },
        }
    }
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

fn checked_annotation_value(expr: &ast::Expr, source: RegisteredSource<'_>) -> Option<Expr> {
    let ast::Expr::Starred(starred) = expr else {
        return Region::Suite.operand(expr, source);
    };
    Some(unpack_annotation(
        Region::Suite.operand(&starred.value, source)?,
    ))
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
        kind: ExprKind::Builtin(Builtin::ListGet, vec![items, Expr::integer(0)]),
    }
}

/// Prepare each operand in its declaring source and key it by its expression’s byte offset.
pub(crate) fn annotation_operands<R>(
    stmts: &[ast::Stmt],
    string_annotations: bool,
    source: RegisteredSource<'_>,
    bundled: bool,
    prepare_child: impl Fn(&str, &Parsed<ast::ModExpression>) -> Option<AnnotationBody<R>>,
    namespace_annotations: &mut HashMap<Option<TextSize>, Vec<TextSize>>,
) -> HashMap<TextSize, PreparedAnnotation<R>> {
    #[derive(Clone, Copy)]
    enum Scope {
        Module,
        Class(TextSize),
        Function,
    }

    struct Strings<'a, R> {
        sites: HashMap<TextSize, PreparedAnnotation<R>>,
        prepare_child: &'a PrepareChild<'a, R>,
        source: RegisteredSource<'a>,
        string_annotations: bool,
        bundled: bool,
        scope: Scope,
        namespace_annotations: &'a mut HashMap<Option<TextSize>, Vec<TextSize>>,
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
                    checked_annotation_value(expr, self.source)
                };
                if let Some(value) = value {
                    self.sites
                        .insert(expr.start(), PreparedAnnotation::Value(value));
                }
                return;
            };
            let body = if self.bundled {
                None
            } else {
                annotation_expression(&text).and_then(|parsed| (self.prepare_child)(&text, &parsed))
            };
            self.sites
                .insert(expr.start(), PreparedAnnotation::String { text, body });
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
                    let site = class
                        .decorator_list
                        .first()
                        .map_or(class.start(), Ranged::start);
                    self.nested(&class.body, Scope::Class(site));
                }
                ast::Stmt::AnnAssign(ann) if ann.simple && ann.target.is_name_expr() => {
                    let owner = match self.scope {
                        Scope::Module => None,
                        Scope::Class(start) => Some(start),
                        Scope::Function => return,
                    };
                    self.record(&ann.annotation, owner.is_none());
                    if self.string_annotations {
                        self.namespace_annotations
                            .entry(owner)
                            .or_default()
                            .push(ann.annotation.start());
                    }
                }
                _ => walk_stmt(self, stmt),
            }
        }
    }

    let mut strings = Strings {
        sites: HashMap::new(),
        prepare_child: &prepare_child,
        source,
        string_annotations,
        bundled,
        scope: Scope::Module,
        namespace_annotations,
    };
    strings.visit_body(stmts);
    strings.sites
}

#[cfg(test)]
pub(super) fn test_annotation_body(
    text: &str,
    parsed: &Parsed<ast::ModExpression>,
) -> Option<AnnotationBody<()>> {
    let source = RegisteredSource {
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
            text,
            replacements: &HashMap::new(),
            has_templates: false,
            method_imports: &Default::default(),
            private_names: None,
            template_fields: None,
            generators: None,
            lambdas: None,
        };
        let mut namespace_annotations = HashMap::new();
        let sites = annotation_operands(
            &parsed.syntax().body,
            false,
            source,
            false,
            test_annotation_body,
            &mut namespace_annotations,
        );
        assert!(namespace_annotations.is_empty());
        assert_eq!(sites.len(), 4);
        for (start, _) in text.match_indices("'list[int]'") {
            let entry = &sites[&TextSize::try_from(start).unwrap()];
            let PreparedAnnotation::String {
                body:
                    Some(AnnotationBody {
                        code: AnnotationCode { range, .. },
                        ..
                    }),
                ..
            } = entry
            else {
                panic!("expected a checked function argument");
            };
            assert_eq!(range.start().to_u32(), 0);
            assert_eq!(range.end().to_u32(), 9);
        }
        let site = |token| TextSize::try_from(text.find(token).unwrap()).unwrap();
        assert!(matches!(
            sites[&site("'(lambda")],
            PreparedAnnotation::String { body: Some(_), .. }
        ));
        assert!(matches!(
            sites[&site("'Name'")],
            PreparedAnnotation::String { body: None, .. }
        ));
        assert!(!sites.contains_key(&site("'skip")));
    }

    #[test]
    fn prepare_lazy_function_and_class_values_in_the_declaring_source() {
        let text = "class C:\n    field: mark(1)\n    deferred: (lambda: 2)\n    def f(self, *args: *items) -> token: pass\n";
        let parsed = ruff_python_parser::parse_module(text).unwrap();
        let source = RegisteredSource {
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
            &mut HashMap::new(),
        );
        let offset = |s| TextSize::try_from(text.find(s).unwrap()).unwrap();
        assert!(matches!(
            sites[&offset("mark(1)")],
            PreparedAnnotation::Value(_)
        ));
        assert!(matches!(
            sites[&offset("token")],
            PreparedAnnotation::Value(_)
        ));
        assert!(matches!(
            sites[&offset("lambda")],
            PreparedAnnotation::Value(_)
        ));
        let PreparedAnnotation::Value(Expr {
            kind: ExprKind::Builtin(Builtin::ListGet, items),
            ..
        }) = &sites[&offset("*items")]
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
    fn future_assignments_belong_to_their_declaring_namespace() {
        let text = "mod_field: A[B]
@decorator
class C:
    field: C[D]
    def f(self, arg: E[F]):
        local: G[H]
        class Nested:
            field: I[J]
after: K[L]
";
        let parsed = ruff_python_parser::parse_module(text).unwrap();
        let source = RegisteredSource {
            text,
            replacements: &HashMap::new(),
            has_templates: false,
            method_imports: &Default::default(),
            private_names: None,
            template_fields: None,
            generators: None,
            lambdas: None,
        };
        let mut groups = HashMap::new();
        annotation_operands(
            &parsed.syntax().body,
            true,
            source,
            false,
            test_annotation_body,
            &mut groups,
        );
        let site = |spelling| TextSize::try_from(text.find(spelling).unwrap()).unwrap();
        assert_eq!(
            groups,
            HashMap::from([
                (None, vec![site("A[B]"), site("K[L]")]),
                (Some(site("@decorator")), vec![site("C[D]")]),
                (Some(site("class Nested")), vec![site("I[J]")]),
            ])
        );
    }
}
