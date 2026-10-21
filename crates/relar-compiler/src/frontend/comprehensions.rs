//! Eager displays normally use private functions. Displays containing assignment
//! expressions run inline with private iteration slots so writes reach their
//! enclosing function, including when a later iteration raises.

use std::cell::RefCell;

use ruff_python_ast::visitor::{Visitor, walk_expr};

use super::*;

pub(super) struct Helper {
    pub function: ir::Function,
    pub calls: HashSet<usize>,
    pub recursion_error: String,
}

/// Source functions reserve the leading IDs; nested displays reserve an ID before
/// checking their bodies, then fill it without disturbing earlier call targets.
pub(super) struct Helpers {
    first: usize,
    functions: RefCell<Vec<Option<Helper>>>,
}

impl Helpers {
    pub fn new(first: usize) -> Self {
        Self {
            first,
            functions: RefCell::new(Vec::new()),
        }
    }

    fn reserve(&self) -> usize {
        let mut functions = self.functions.borrow_mut();
        let id = self.first + functions.len();
        functions.push(None);
        id
    }

    fn finish(&self, id: usize, helper: Helper) {
        self.functions.borrow_mut()[id - self.first] = Some(helper);
    }

    pub fn into_functions(self) -> impl Iterator<Item = Helper> {
        self.functions
            .into_inner()
            .into_iter()
            .map(|helper| helper.expect("checked comprehension filled its helper"))
    }
}

#[derive(Clone, Copy)]
enum Yield<'a> {
    List(&'a ast::Expr),
    Set(&'a ast::Expr),
    Dict(&'a ast::Expr, &'a ast::Expr),
}

#[derive(Clone, Copy)]
struct Display<'a> {
    clauses: &'a [ast::Comprehension],
    value: Yield<'a>,
}

impl<'a> Display<'a> {
    fn from_expr(expression: &'a ast::Expr) -> Option<Self> {
        match expression {
            ast::Expr::ListComp(value) => Some(Self {
                clauses: &value.generators,
                value: Yield::List(&value.elt),
            }),
            ast::Expr::SetComp(value) => Some(Self {
                clauses: &value.generators,
                value: Yield::Set(&value.elt),
            }),
            ast::Expr::DictComp(value) => Some(Self {
                clauses: &value.generators,
                value: Yield::Dict(value.key.as_deref()?, &value.value),
            }),
            _ => None,
        }
    }

    fn visit_value(self, visitor: &mut impl Visitor<'a>) {
        match self.value {
            Yield::List(value) | Yield::Set(value) => visitor.visit_expr(value),
            Yield::Dict(key, value) => {
                visitor.visit_expr(key);
                visitor.visit_expr(value);
            }
        }
    }

    /// Only this clause's suffix repeats on its backedge. An earlier filter can
    /// safely narrow a name when the remaining filters and yielded value do not
    /// replace it.
    fn repeated_writes(self, index: usize) -> HashSet<String> {
        let (first, second) = match self.value {
            Yield::List(value) | Yield::Set(value) => (value, None),
            Yield::Dict(key, value) => (key, Some(value)),
        };
        self.clauses[index..]
            .iter()
            .flat_map(|clause| std::iter::once(&clause.target).chain(clause.ifs.iter()))
            .chain([first])
            .chain(second)
            .flat_map(narrowing::expression_assignments)
            .collect()
    }
}

fn target_names(target: &ast::Expr, names: &mut HashSet<String>) {
    if let ast::Expr::Name(name) = target {
        names.insert(name.id.to_string());
    } else if let ast::Expr::Starred(starred) = target {
        target_names(&starred.value, names);
    } else if let Some(elements) = pattern_elements(target) {
        for element in elements {
            target_names(element, names);
        }
    }
}

/// Free reads respect nested comprehension scopes: only their leftmost iterable
/// belongs to the surrounding scope; all target names shadow throughout the rest.
struct Reads {
    names: HashSet<String>,
    bound: Vec<HashSet<String>>,
}

impl<'a> Visitor<'a> for Reads {
    fn visit_expr(&mut self, expression: &'a ast::Expr) {
        if let Some(display) = Display::from_expr(expression) {
            self.visit_expr(&display.clauses[0].iter);
            let mut targets = HashSet::new();
            for clause in display.clauses {
                target_names(&clause.target, &mut targets);
            }
            self.bound.push(targets);
            for (index, clause) in display.clauses.iter().enumerate() {
                self.visit_expr(&clause.target);
                if index != 0 {
                    self.visit_expr(&clause.iter);
                }
                for condition in &clause.ifs {
                    self.visit_expr(condition);
                }
            }
            display.visit_value(self);
            self.bound.pop();
        } else if let ast::Expr::Name(name) = expression {
            if name.ctx == ast::ExprContext::Load
                && !self
                    .bound
                    .iter()
                    .any(|scope| scope.contains(name.id.as_str()))
            {
                self.names.insert(name.id.to_string());
            }
        } else {
            walk_expr(self, expression);
        }
    }
}

impl Checker<'_> {
    /// Evaluates the first iterable outside the target scope and passes rooted
    /// values into a private helper. Captures are local bindings, never delayed
    /// field/index reads, so mutation through aliases remains visible in the body.
    ///
    /// ```python
    /// x = [1, 2]
    /// values = [x + 1 for x in x]  # the iterable reads the enclosing x
    /// ```
    pub(super) fn comprehension(
        &mut self,
        expression: &ast::Expr,
        expected: Option<Type>,
    ) -> Result<Expr, String> {
        let display = Display::from_expr(expression)
            .ok_or_else(|| error(self.source, expression, "unsupported comprehension display"))?;
        if display.clauses.iter().any(|clause| clause.is_async) {
            return Err(error(
                self.source,
                expression,
                "asynchronous comprehensions are unsupported",
            ));
        }
        if display
            .clauses
            .iter()
            .any(|clause| !narrowing::expression_assignments(&clause.iter).is_empty())
        {
            return Err(error(
                self.source,
                expression,
                "assignment expressions are forbidden in comprehension iterables",
            ));
        }
        if !narrowing::expression_assignments(expression).is_empty() {
            return self.inline_comprehension(expression, display, expected);
        }
        let input = self.iteration_input(&display.clauses[0].iter)?;
        let mut targets = HashSet::new();
        for clause in display.clauses {
            target_names(&clause.target, &mut targets);
        }
        let mut reads = Reads {
            names: HashSet::new(),
            bound: vec![targets.clone()],
        };
        for (index, clause) in display.clauses.iter().enumerate() {
            reads.visit_expr(&clause.target);
            if index != 0 {
                reads.visit_expr(&clause.iter);
            }
            for condition in &clause.ifs {
                reads.visit_expr(condition);
            }
        }
        display.visit_value(&mut reads);
        let mut captures = reads.names.into_iter().collect::<Vec<_>>();
        captures.sort();
        let id = self.helpers.reserve();
        let mut helper = Checker::new(
            self.graph,
            self.module,
            self.functions,
            self.classes,
            self.types,
            self.helpers,
            Type::None,
        );
        helper.comprehension_targets = targets;
        let mut params = Vec::new();
        let mut args = Vec::new();
        let mut parameter = |value: Expr, helper: &mut Checker<'_>| {
            // Literal range bounds stay visible to interval analysis. Moving an
            // immutable literal into the helper cannot reorder observable work.
            let mut literal = &value;
            while let ExprKind::Negate(inner) = &literal.kind {
                literal = inner;
            }
            if matches!(literal.kind, ExprKind::Int(_)) {
                return value;
            }
            let slot = helper.hidden(value.ty);
            helper.assigned.insert(slot);
            params.push(slot);
            args.push(value);
            helper.local(slot)
        };
        let input = match input {
            Iteration::Range(values) => Iteration::Range(
                values
                    .into_iter()
                    .map(|value| parameter(value, &mut helper))
                    .collect(),
            ),
            Iteration::Container {
                iterable,
                items,
                start,
            } => Iteration::Container {
                iterable: parameter(iterable, &mut helper),
                items,
                start: start.map(|start| parameter(start, &mut helper)),
            },
        };
        for name in captures {
            let Some(&source_slot) = self.names.get(&name) else {
                continue;
            };
            if !self.assigned.contains(&source_slot) {
                return Err(error(
                    self.source,
                    expression,
                    "captured local may be uninitialized on this path",
                ));
            }
            let value = match self.narrowed.get(&source_slot).copied() {
                Some(Type::None) => Expr {
                    ty: Type::None,
                    kind: ExprKind::None,
                },
                Some(ty) => Expr {
                    ty,
                    kind: ExprKind::Unwrap(Box::new(self.local(source_slot))),
                },
                None => self.local(source_slot),
            };
            let slot = helper.declare(&name, value.ty, expression)?;
            helper.assigned.insert(slot);
            params.push(slot);
            args.push(value);
        }
        // The leaf establishes the result type after its target/filter facts are
        // known. No expression can read this private slot before initialization.
        let result = helper.hidden(Type::None);
        let loops = helper.comprehension_clause(display, 0, Some(input), expected, result)?;
        let ty = helper.locals[result];
        let empty = match display.value {
            Yield::List(_) => ExprKind::List(Vec::new()),
            Yield::Set(_) => ExprKind::Set {
                element: self.types.set_element(ty).unwrap(),
                values: Vec::new(),
            },
            Yield::Dict(..) => {
                let (key, value) = self.types.mapping(ty).unwrap();
                ExprKind::Dict {
                    key,
                    value,
                    entries: Vec::new(),
                }
            }
        };
        let mut body = vec![Statement::Assign(result, Expr { ty, kind: empty })];
        body.extend(loops);
        body.push(Statement::Return(helper.local(result)));
        self.helpers.finish(
            id,
            Helper {
                function: ir::Function {
                    params,
                    returns: ty,
                    locals: helper.locals,
                    body,
                },
                calls: helper.calls,
                recursion_error: self.graph.modules[self.module].error(
                    expression,
                    "recursive calls are unsupported in this prototype",
                ),
            },
        );
        self.calls.insert(id);
        Ok(Expr {
            ty,
            kind: ExprKind::Call(id, ir::CallArgs::positional(args)),
        })
    }

    /// Shares enclosing bindings while reserving private targets and owners.
    /// Exported assignments survive cleanup; all remaining private slots are
    /// cleared on both normal completion and exceptional loop exits.
    fn inline_comprehension(
        &mut self,
        expression: &ast::Expr,
        display: Display<'_>,
        expected: Option<Type>,
    ) -> Result<Expr, String> {
        let input = self.iteration_input(&display.clauses[0].iter)?;
        let before = self.local_state();
        let names = self.names.clone();
        let targets = self.comprehension_targets.clone();
        let first_private = self.locals.len();
        let writes = narrowing::expression_assignments(expression);
        let mut private_targets = HashSet::new();
        for clause in display.clauses {
            target_names(&clause.target, &mut private_targets);
        }
        for name in &private_targets {
            self.names.remove(name);
        }
        self.comprehension_targets.extend(private_targets);
        self.comprehension_depth += 1;
        let result = self.hidden(Type::None);
        let loops = self.comprehension_clause(display, 0, Some(input), expected, result)?;
        let ty = self.locals[result];
        let empty = match display.value {
            Yield::List(_) => ExprKind::List(Vec::new()),
            Yield::Set(_) => ExprKind::Set {
                element: self.types.set_element(ty).unwrap(),
                values: Vec::new(),
            },
            Yield::Dict(..) => {
                let (key, value) = self.types.mapping(ty).unwrap();
                ExprKind::Dict {
                    key,
                    value,
                    entries: Vec::new(),
                }
            }
        };
        let mut body = vec![Statement::Assign(result, Expr { ty, kind: empty })];
        body.extend(loops);
        self.comprehension_depth -= 1;
        self.comprehension_targets = targets;
        self.names = names;
        for (name, &slot) in &self.comprehension_exports {
            if !self.comprehension_targets.contains(name) {
                self.names.insert(name.clone(), slot);
            }
        }
        self.restore_state(before);
        for name in writes {
            if let Some(slot) = self.names.get(&name) {
                self.narrowed.remove(slot);
            }
        }
        let exported = self
            .comprehension_exports
            .values()
            .copied()
            .collect::<HashSet<_>>();
        let clear = (first_private..self.locals.len())
            .filter(|slot| !exported.contains(slot))
            .collect();
        Ok(Expr {
            ty,
            kind: ExprKind::Scope {
                body,
                value: Box::new(self.local(result)),
                clear,
            },
        })
    }

    /// Filters are nested in source order and contribute facts only on their true
    /// edges. Each nested iterable is evaluated anew under the preceding bindings.
    fn comprehension_clause(
        &mut self,
        display: Display<'_>,
        index: usize,
        first: Option<Iteration>,
        expected: Option<Type>,
        result: usize,
    ) -> Result<Vec<Statement>, String> {
        let clause = &display.clauses[index];
        let input = match first {
            Some(input) => input,
            None => self.iteration_input(&clause.iter)?,
        };
        for name in display.repeated_writes(index) {
            if let Some(slot) = self.names.get(&name) {
                self.narrowed.remove(slot);
            }
        }
        self.lower_iteration(&clause.target, input, &[], |checker| {
            let mut conditions = Vec::new();
            for condition in &clause.ifs {
                conditions.push(checker.condition(condition)?);
                checker.apply_facts(condition, true);
            }
            let mut body = if index + 1 == display.clauses.len() {
                checker.comprehension_leaf(display.value, expected, result)?
            } else {
                checker.comprehension_clause(display, index + 1, None, expected, result)?
            };
            for condition in conditions.into_iter().rev() {
                body = vec![Statement::If(condition, body, Vec::new())];
            }
            Ok(body)
        })
        .map(|(body, _)| body)
    }

    /// A dict display evaluates and roots its key before its value. Ordinary
    /// subscript assignment cannot express that order because it evaluates RHS first.
    fn comprehension_leaf(
        &mut self,
        value: Yield<'_>,
        expected: Option<Type>,
        result: usize,
    ) -> Result<Vec<Statement>, String> {
        match value {
            Yield::List(expression) | Yield::Set(expression) => {
                let is_set = matches!(value, Yield::Set(_));
                let element = expected.and_then(|ty| {
                    if is_set {
                        self.types.set_element(ty)
                    } else {
                        self.types.element(ty)
                    }
                });
                let value = if let Some(element) = element {
                    self.expr_expected(expression, element)?
                } else {
                    self.expr(expression)?
                };
                if is_set && !matches!(value.ty, Type::Int | Type::Str | Type::Bytes) {
                    return Err(error(
                        self.source,
                        expression,
                        "set elements must be exactly int, str, or bytes",
                    ));
                }
                self.locals[result] = if is_set {
                    self.types.set(value.ty)
                } else {
                    self.types.list(value.ty)
                };
                Ok(vec![Statement::Expr(Expr {
                    ty: Type::None,
                    kind: ExprKind::Builtin(
                        if is_set {
                            Builtin::SetAdd
                        } else {
                            Builtin::Append
                        },
                        vec![self.local(result), value],
                    ),
                })])
            }
            Yield::Dict(key, value) => {
                let expected = expected.and_then(|ty| self.types.mapping(ty));
                let key_value = if let Some((key_type, _)) = expected {
                    self.expr_expected(key, key_type)?
                } else {
                    self.expr(key)?
                };
                if !matches!(key_value.ty, Type::Int | Type::Str | Type::Bytes) {
                    return Err(error(
                        self.source,
                        key,
                        "dictionary keys must be exactly int, str, or bytes",
                    ));
                }
                let captured = self.hidden(key_value.ty);
                let value = if let Some((_, value_type)) = expected {
                    self.expr_expected(value, value_type)?
                } else {
                    self.expr(value)?
                };
                self.locals[result] = self.types.dict(key_value.ty, value.ty);
                Ok(vec![Statement::Scope {
                    body: vec![
                        Statement::Assign(captured, key_value),
                        Statement::SetItem(self.local(result), self.local(captured), value),
                    ],
                    clear: vec![captured],
                }])
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{CompileOptions, check, compile};

    #[test]
    fn contextual_nested_displays_preserve_private_target_types() {
        let source = r#"
def values() -> list[list[int | None]]:
    item = "outside"
    rows: list[list[int | None]] = [[item for item in range(3)] for row in range(2)]
    print(item)
    empty: list[dict[str, int]] = [{} for item in range(0)]
    return rows
"#;
        compile(source, &CompileOptions::default()).unwrap();
    }

    #[test]
    fn later_targets_are_local_even_before_their_binding() {
        let source = r#"
def values() -> list[int]:
    later = [1]
    return [item for first in [0] for item in later for later in [[2]]]
"#;
        assert!(check(source).is_err());
    }

    #[test]
    fn helper_calls_participate_in_recursion_validation() {
        let source = r#"
def recur(count: int) -> list[int]:
    return [len(recur(item)) for item in range(count)]
"#;
        let diagnostic = check(source).unwrap_err();
        assert!(diagnostic.contains("recursive calls"), "{diagnostic}");
    }

    #[test]
    fn comprehension_targets_do_not_invalidate_outer_optional_facts() {
        let source = r#"
def value(item: int | None) -> int:
    assert item is not None
    for turn in range(2):
        values = [item for item in [None]]
        print(item + len(values))
    return item
"#;
        compile(source, &CompileOptions::default()).unwrap();
    }

    #[test]
    fn constructor_self_shadowing_preserves_partial_object_restriction() {
        let accepted = r#"
class Values:
    values: list[int]
    def __init__(self) -> None:
        self.values = [self for self in range(3)]
"#;
        compile(accepted, &CompileOptions::default()).unwrap();
        for value in [
            "[len(self.values) for item in range(3)]",
            "[self for self in self.values]",
        ] {
            let source = accepted.replace("[self for self in range(3)]", value);
            let diagnostic = check(&source).unwrap_err();
            assert!(
                diagnostic.contains("self before construction"),
                "{diagnostic}"
            );
        }
    }

    #[test]
    fn literal_outer_range_bounds_survive_helper_capture() {
        for (arguments, small_cursor) in [
            ("3", true),
            ("3, -4, -2", true),
            ("-4611686018427387905, -4611686018427387902", false),
        ] {
            let source = format!("values = [item for item in range({arguments})]\n");
            let graph = crate::modules::SourceGraph::single(&source).unwrap();
            let module = super::super::lower(&graph).unwrap();
            assert_eq!(module.functions.len(), 1);
            assert!(module.functions[0].params.is_empty());
            let super::Statement::Scope { body, .. } = &module.functions[0].body[1] else {
                panic!("scoped range")
            };
            assert!(matches!(&body[0], super::Statement::For { .. }));
            let ranges = crate::ranges::analyze(&module);
            assert_eq!(ranges.range_cursor(&body[0]).is_some(), small_cursor);
        }
    }
}
