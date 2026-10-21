//! Eager displays normally use private functions. Displays containing assignment
//! expressions run inline with private iteration slots so writes reach their
//! enclosing function, including when a later iteration raises.

use std::cell::RefCell;

use ruff_python_ast::visitor::{Visitor, walk_expr};

use super::*;

mod slices;

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

    pub(super) fn reserve(&self) -> usize {
        let mut functions = self.functions.borrow_mut();
        let id = self.first + functions.len();
        functions.push(None);
        id
    }

    pub(super) fn finish(&self, id: usize, helper: Helper) {
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
    Sum(&'a ast::Expr),
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
            Yield::List(value) | Yield::Sum(value) | Yield::Set(value) => {
                visitor.visit_expr(value);
            }
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
            Yield::List(value) | Yield::Sum(value) | Yield::Set(value) => (value, None),
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
    nested_lambdas: bool,
}

/// Finds lexical free reads for capture rejection, including through nested
/// lambdas. Comprehension targets and each lambda's own locals shadow normally.
pub(super) fn free_reads(expression: &ast::Expr, bound: HashSet<String>) -> HashSet<String> {
    let mut reads = Reads {
        names: HashSet::new(),
        bound: vec![bound],
        nested_lambdas: true,
    };
    reads.visit_expr(expression);
    reads.names
}

impl<'a> Visitor<'a> for Reads {
    fn visit_expr(&mut self, expression: &'a ast::Expr) {
        if let ast::Expr::Lambda(lambda) = expression {
            if self.nested_lambdas {
                let mut bound = narrowing::expression_assignments(&lambda.body);
                if let Some(parameters) = &lambda.parameters {
                    for parameter in parameters.iter_non_variadic_params() {
                        if let Some(default) = &parameter.default {
                            self.visit_expr(default);
                        }
                        bound.insert(parameter.name().to_string());
                    }
                }
                self.bound.push(bound);
                self.visit_expr(&lambda.body);
                self.bound.pop();
            }
        } else if let ast::Expr::Generator(generator) = expression {
            self.visit_expr(&generator.generators[0].iter);
            let mut targets = HashSet::new();
            for clause in &generator.generators {
                target_names(&clause.target, &mut targets);
            }
            self.bound.push(targets);
            for (index, clause) in generator.generators.iter().enumerate() {
                self.visit_expr(&clause.target);
                if index != 0 {
                    self.visit_expr(&clause.iter);
                }
                for condition in &clause.ifs {
                    self.visit_expr(condition);
                }
            }
            self.visit_expr(&generator.elt);
            self.bound.pop();
        } else if let Some(display) = Display::from_expr(expression) {
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
        self.comprehension_display(expression, display, expected)
    }

    /// Reduces an immediately consumed integer comprehension in its original
    /// eager order. Integer addition has no callbacks or recoverable failures;
    /// the private list and its element owners cannot otherwise be observed.
    pub(super) fn sum_comprehension(&mut self, expression: &ast::Expr) -> Result<Expr, String> {
        let ast::Expr::ListComp(value) = expression else {
            unreachable!("sum selected a list comprehension");
        };
        self.comprehension_display(
            expression,
            Display {
                clauses: &value.generators,
                value: Yield::Sum(&value.elt),
            },
            Some(Type::Int),
        )
    }

    fn comprehension_display(
        &mut self,
        expression: &ast::Expr,
        display: Display<'_>,
        expected: Option<Type>,
    ) -> Result<Expr, String> {
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
        if !narrowing::expression_assignments(expression).is_empty()
            || pipelines::is_pipeline_expression(&display.clauses[0].iter)
        {
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
            nested_lambdas: false,
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
            self.globals,
        );
        helper.inference = self.inference;
        helper.class = self.class;
        helper.class_receiver = self
            .class_receiver
            .clone()
            .filter(|name| !targets.contains(name));
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
            Iteration::Prepared(_) => unreachable!("pipeline comprehensions run inline"),
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
            let value = self.read_local(source_slot);
            let slot = helper.declare(&name, value.ty, expression)?;
            helper.assigned.insert(slot);
            params.push(slot);
            args.push(value);
        }
        // The leaf establishes the result type after its target/filter facts are
        // known. No expression can read this private slot before initialization.
        let gather_input = match &input {
            Iteration::Container {
                iterable,
                items: false,
                start: None,
            } if iterable.ty == Type::Str => Some(iterable.clone()),
            _ => None,
        };
        let result = helper.hidden(Type::None);
        let loops = helper.comprehension_clause(display, 0, Some(input), expected, result)?;
        let ty = helper.locals[result];
        let empty = match display.value {
            Yield::List(_) => ExprKind::List(Vec::new()),
            Yield::Sum(_) => ExprKind::Int("0".into()),
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
        if ty == Type::ListInt
            && let Some(mut input) = gather_input
            && let Some(mapping) = helper.dictionary_gather_mapping(display)?
        {
            // Validate at exactly the original decode's evaluation point, before
            // subsequent captures and before entering the helper. Immutable bytes
            // then keep the gather's borrowed UTF-8 view valid without a Str copy.
            if let ExprKind::Local(slot) = &input.kind
                && params.first() == Some(slot)
                && let Some(argument) = args.first_mut()
                && let ExprKind::Builtin(operation, decoded) = &mut argument.kind
                && matches!(operation, Builtin::Decode)
                && matches!(decoded.as_slice(), [value] if value.ty == Type::Bytes)
            {
                *operation = Builtin::BytesValidateUtf8;
                argument.ty = Type::Bytes;
                helper.locals[*slot] = Type::Bytes;
                input.ty = Type::Bytes;
            }
            let operation = if input.ty == Type::Bytes {
                Builtin::DictGatherBytesInt
            } else {
                Builtin::DictGatherStrInt
            };
            let gather = Expr {
                ty: Type::ListInt,
                kind: ExprKind::Conditional(
                    Box::new(Expr {
                        ty: Type::Bool,
                        kind: ExprKind::Builtin(Builtin::Bool, vec![input.clone()]),
                    }),
                    Box::new(Expr {
                        ty: Type::ListInt,
                        kind: ExprKind::Builtin(operation, vec![input, mapping]),
                    }),
                    Box::new(Expr {
                        ty: Type::ListInt,
                        kind: ExprKind::List(Vec::new()),
                    }),
                ),
            };
            body = vec![Statement::Return(gather)];
        }
        if matches!(display.value, Yield::Sum(_)) && display.clauses.len() == 1 {
            slices::fuse(&mut body, &params, &mut args);
        }
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

    /// Finds the mapping in a checked, callback-free character comprehension.
    /// Ordinary captures and type checks remain intact; the replacement skips
    /// this mapping load for an empty source, just as the original loop does.
    fn dictionary_gather_mapping(&mut self, display: Display<'_>) -> Result<Option<Expr>, String> {
        let [clause] = display.clauses else {
            return Ok(None);
        };
        if !clause.ifs.is_empty() {
            return Ok(None);
        }
        let ast::Expr::Name(target) = &clause.target else {
            return Ok(None);
        };
        let Yield::List(ast::Expr::Subscript(subscript)) = display.value else {
            return Ok(None);
        };
        let ast::Expr::Name(key) = &*subscript.slice else {
            return Ok(None);
        };
        let ast::Expr::Name(mapping) = &*subscript.value else {
            return Ok(None);
        };
        if key.id != target.id || mapping.id == target.id {
            return Ok(None);
        }
        let mapping = self.expr(&subscript.value)?;
        if self.types.mapping(mapping.ty) != Some((Type::Str, Type::Int))
            || !matches!(
                mapping.kind,
                ExprKind::Local(_) | ExprKind::Builtin(Builtin::GlobalGet(_), _)
            )
        {
            return Ok(None);
        }
        Ok(Some(mapping))
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
        let class_receiver = self.class_receiver.clone();
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
        if self
            .class_receiver
            .as_ref()
            .is_some_and(|name| private_targets.contains(name))
        {
            self.class_receiver = None;
        }
        self.comprehension_targets.extend(private_targets);
        self.comprehension_depth += 1;
        let result = self.hidden(Type::None);
        let loops = self.comprehension_clause(display, 0, Some(input), expected, result)?;
        let ty = self.locals[result];
        let empty = match display.value {
            Yield::List(_) => ExprKind::List(Vec::new()),
            Yield::Sum(_) => ExprKind::Int("0".into()),
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
        self.class_receiver = class_receiver;
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
            Yield::Sum(expression) => {
                let value = self.expr_expected(expression, Type::Int)?;
                self.locals[result] = Type::Int;
                Ok(vec![Statement::Assign(
                    result,
                    Expr {
                        ty: Type::Int,
                        kind: ExprKind::Binary(
                            Binary::Add,
                            Box::new(self.local(result)),
                            Box::new(value),
                        ),
                    },
                )])
            }
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
    use crate::ir::{Builtin, ExprKind, Statement, Type, walk_expressions};
    use crate::{CompileOptions, check, compile};

    #[test]
    fn character_lookup_fusion_requires_an_exact_pure_comprehension() {
        for (expression, fused) in [
            ("[mapping[c] for c in text]", true),
            ("[mapping[c] for c in text if c]", false),
            ("[mapping[c] for c in text for repeat in range(2)]", false),
            ("[mapping[c] + 1 for c in text]", false),
        ] {
            let source = format!(
                "def gather(mapping: dict[str, int], text: str) -> list[int]:\n    return {expression}\n"
            );
            let llvm = compile(&source, &CompileOptions::default()).unwrap();
            assert_eq!(
                llvm.contains("call ptr @relar_dict_gather_str_int"),
                fused,
                "{expression}"
            );
        }
        let contextual = "def gather(mapping: dict[str, int], text: str) -> list[int | None]:\n    return [mapping[c] for c in text]\n";
        let llvm = compile(contextual, &CompileOptions::default()).unwrap();
        assert!(!llvm.contains("call ptr @relar_dict_gather_str_int"));
    }

    #[test]
    fn decoded_character_fusion_requires_the_exact_decode_input() {
        for (expression, fused) in [
            ("[mapping[c] for c in data.decode()]", true),
            ("[mapping[c] for c in data.decode().rstrip()]", false),
            ("[mapping[c] for c in data.decode() if c]", false),
            ("[mapping[c] + 1 for c in data.decode()]", false),
        ] {
            let source = format!(
                "def gather(mapping: dict[str, int], data: bytes) -> list[int]:\n    return {expression}\n"
            );
            let llvm = compile(&source, &CompileOptions::default()).unwrap();
            assert_eq!(
                llvm.contains("call ptr @relar_bytes_validate_utf8"),
                fused,
                "{expression}"
            );
            assert_eq!(
                llvm.contains("call ptr @relar_dict_gather_bytes_int"),
                fused
            );
            assert_eq!(llvm.contains("call ptr @relar_bytes_decode"), !fused);
        }
    }

    #[test]
    fn decoded_gather_validates_before_calling_the_lazy_global_helper() {
        let directory = tempfile::tempdir().unwrap();
        let entry = directory.path().join("entry.py");
        std::fs::write(
            &entry,
            "from codec import gather\nvalues = gather(b'x')\nprint(len(values))\n",
        )
        .unwrap();
        std::fs::write(
            directory.path().join("codec.py"),
            "mapping: dict[str, int] = {'x': 1}\ndef gather(data: bytes) -> list[int]:\n    return [mapping[c] for c in data.decode()]\n",
        )
        .unwrap();
        let graph = crate::modules::SourceGraph::load(&entry, Some(directory.path())).unwrap();
        let module = crate::frontend::lower(&graph).unwrap();
        let mut helper = None;
        walk_expressions(&module.functions[0].body, &mut |value| {
            if let ExprKind::Call(id, arguments) = &value.kind {
                assert_eq!(arguments.values.len(), 1);
                assert_eq!(arguments.values[0].ty, Type::Bytes);
                assert!(matches!(
                    arguments.values[0].kind,
                    ExprKind::Builtin(Builtin::BytesValidateUtf8, _)
                ));
                helper = Some(*id);
            }
        });
        let helper = &module.functions[helper.unwrap()];
        assert_eq!(helper.locals[helper.params[0]], Type::Bytes);
        let [Statement::Return(value)] = helper.body.as_slice() else {
            panic!("the fused helper must contain only its conditional gather")
        };
        let ExprKind::Conditional(condition, nonempty, empty) = &value.kind else {
            panic!("empty input must bypass the mapping load")
        };
        let ExprKind::Builtin(Builtin::Bool, predicate) = &condition.kind else {
            panic!("the helper must test only whether the byte input is empty")
        };
        assert_eq!(predicate[0].ty, Type::Bytes);
        assert!(matches!(predicate[0].kind, ExprKind::Local(slot) if slot == helper.params[0]));
        let ExprKind::Builtin(Builtin::DictGatherBytesInt, arguments) = &nonempty.kind else {
            panic!("the nonempty arm must gather from the validated bytes")
        };
        assert!(matches!(
            arguments[1].kind,
            ExprKind::Builtin(Builtin::GlobalGet(_), _)
        ));
        assert!(matches!(&empty.kind, ExprKind::List(values) if values.is_empty()));
    }

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
