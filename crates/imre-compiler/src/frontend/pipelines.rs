//! Typed iterator pipelines retain their state in private local slots. Creation,
//! iterator acquisition, and each pull stay separate so composition preserves
//! Python's lazy evaluation and mutation checks without escaping iterator values.

use imre_ast::comprehension::Plan;
use ruff_python_ast::UnaryOp;

use super::comprehensions::{EagerDisplay, filter_suite};
use super::*;
use crate::ir::TextOperation;

mod groupby;

/// Recognize a forward split with a literal one-byte separator and no limit.
fn byte_split_source(source: &Iteration) -> Option<(&Expr, u8)> {
    let Iteration::Container {
        iterable,
        items: false,
        start: None,
    } = source
    else {
        return None;
    };
    byte_split(iterable)
}

/// The immutable input and the exact byte consumed by the field cursor.
pub(super) fn byte_split(iterable: &Expr) -> Option<(&Expr, u8)> {
    let ExprKind::Builtin(Builtin::Text(TextOperation::Split(false)), args) = &iterable.kind else {
        return None;
    };
    let [text, separator, limit] = args.as_slice() else {
        return None;
    };
    let ExprKind::Present(separator) = &separator.kind else {
        return None;
    };
    let ExprKind::Str(separator) = &separator.kind else {
        return None;
    };
    let [byte] = *separator.as_bytes() else {
        return None;
    };
    matches!(&limit.kind, ExprKind::Int(n) if n == "-1").then_some((text, byte))
}

/// `evaluate` evaluates an iterable expression; `acquire` obtains its iterator.
/// Constructor expressions fold their own acquisition into `evaluate`. Every
/// successful `next` owns `value`, and its consumer clears that transient owner.
pub(super) struct Prepared {
    pub evaluate: Vec<Statement>,
    pub(super) acquire: Vec<Statement>,
    pub(super) next: Vec<Statement>,
    pub(super) value: usize,
    pub(super) ready: usize,
    /// An empty tuple has no element type or executable item assignment.
    elementless: bool,
    /// An eager list alternative of this exact virtual generator call.
    eager_list: Option<(TextRange, EagerList)>,
    /// Enclosing slots a later pull may replace, after constructor arguments.
    writes: HashSet<usize>,
    pub clear: Vec<usize>,
}

#[derive(Clone, Copy)]
pub(super) struct EagerList {
    pub(super) value: usize,
    pub(super) selected: usize,
}

impl Prepared {
    pub(super) fn cursor(
        cursor: Expr,
        element: Type,
        mut hidden: impl FnMut(Type) -> usize,
    ) -> Self {
        let kind = match cursor.kind {
            ExprKind::Builtin(Builtin::SplitByteIter(_) | Builtin::SortedSplitByteIter(_), _) => {
                crate::ir::IterNextKind::SplitField
            }
            _ => crate::ir::IterNextKind::Generic,
        };
        Self::pull_cursor(cursor, element, kind, |ty, _| hidden(ty))
    }

    pub(super) fn pull_cursor(
        cursor: Expr,
        element: Type,
        kind: ir::IterNextKind,
        mut hidden: impl FnMut(Type, Option<ir::IterNextKind>) -> usize,
    ) -> Self {
        let iterator = hidden(cursor.ty, Some(kind));
        let value = hidden(element, None);
        let ready = hidden(Type::Bool, None);
        Prepared {
            evaluate: vec![Statement::Assign(iterator, cursor)],
            acquire: Vec::new(),
            next: vec![
                Statement::Clear(value),
                Statement::IterNext {
                    kind,
                    iterator,
                    key: value,
                    value: None,
                    ready,
                },
            ],
            value,
            ready,
            elementless: false,
            eager_list: None,
            writes: HashSet::new(),
            clear: vec![iterator, value, ready],
        }
    }

    /// A direct Python cursor can supply both target values without creating a row.
    pub(super) fn bind_loop_target<B: unpacking::PatternBinder>(
        &mut self,
        binder: &mut B,
        target: &ast::Expr,
        pair_pull: bool,
    ) -> StdResult<&[Statement], B::Error> {
        let row = binder.pattern_local(self.value);
        if row.ty != Type::PythonObject || target.is_name_expr() {
            self.next
                .push(Self::exhaust_if_empty(Expr::local(self.ready, Type::Bool)));
            let start = self.next.len();
            let projected =
                matches!(row.ty, Type::Tuple(_)).then_some(unpacking::Row::Projected(self.value));
            unpacking::bind_input(binder, target, row, projected, &mut self.next)?;
            let end = self.next.len();
            self.next.push(Statement::Clear(self.value));
            return Ok(&self.next[start..end]);
        }
        // The acquired Python cursor and its unadapted one-row pull must still
        // refer to the same slots. Typed rows keep their own snapshot layout.
        let cursor = match (self.evaluate.last(), self.next.as_slice()) {
            (
                Some(Statement::Assign(slot, acquisition)),
                [
                    Statement::Clear(clear),
                    Statement::IterNext {
                        kind: ir::IterNextKind::Generic,
                        iterator,
                        key,
                        value: None,
                        ready,
                    },
                ],
            ) if pair_pull
                && slot == iterator
                && *clear == self.value
                && *key == self.value
                && *ready == self.ready
                && matches!(
                    acquisition.kind,
                    ExprKind::Builtin(Builtin::RegisteredIter(ir::IteratorOp::Loop(_)), _)
                ) =>
            {
                Some(*iterator)
            }
            _ => None,
        };
        let mut scope = self.pull_scope();
        let source = unpacking::Row::Pulled {
            scope: &mut scope,
            pair: cursor.map(|cursor| (cursor, self.ready)),
        };
        let pair = unpacking::bind_input(binder, target, row, Some(source), &mut self.next)?;
        if let (Some(cursor), Some(ordinary)) = (cursor, pair) {
            self.acquire.push(Statement::Assign(
                ordinary,
                Expr {
                    ty: Type::Bool,
                    kind: ExprKind::Builtin(
                        Builtin::RegisteredPairNeedsRow,
                        vec![Expr::local(cursor, Type::PythonObject)],
                    ),
                },
            ));
            self.clear.push(ordinary);
        }
        Ok(&self.next)
    }

    /// An enclosing adapter has its own iteration semantics even if it reuses this state.
    pub(super) fn eager_list_at(&self, source: &impl Ranged) -> Option<EagerList> {
        self.eager_list
            .filter(|(range, _)| *range == source.range())
            .map(|(_, list)| list)
    }

    /// Only reads during a pull can observe an exception binding cleared by the consumer.
    pub(super) fn reads_on_pull(&self, slot: usize) -> bool {
        let mut reads = false;
        ir::walk_expressions(&self.next, &mut |expr| {
            reads |= matches!(expr.kind, ExprKind::Local(read) if read == slot);
        });
        reads
    }

    /// Execute one pull per iteration and release the cursor before running the exhaustion suite.
    pub(super) fn for_suite(
        mut self,
        body: Vec<Statement>,
        otherwise: Vec<Statement>,
        before_iteration: Vec<Statement>,
    ) -> Vec<Statement> {
        self.next.extend(body);
        self.loop_suite(otherwise, before_iteration)
    }

    /// The suspension owns this pull, including the exhaustion edge and the yielded item.
    pub(super) fn pull_expression(&mut self, ty: Type) -> Expr {
        self.pull_scope().finish(Expr::local(self.value, ty))
    }

    /// Keep the actual pull and its consumer in the same temporary scope.
    pub(super) fn pull_scope(&mut self) -> calls::ArgumentScope {
        self.next
            .push(Self::exhaust_if_empty(Expr::local(self.ready, Type::Bool)));
        calls::ArgumentScope {
            body: std::mem::take(&mut self.next),
            clear: vec![self.value],
        }
    }

    pub(super) fn loop_suite(
        mut self,
        otherwise: Vec<Statement>,
        before_iteration: Vec<Statement>,
    ) -> Vec<Statement> {
        self.evaluate.append(&mut self.acquire);
        self.evaluate.extend(before_iteration);
        scoped_loop(self.evaluate, self.clear, otherwise, |otherwise| {
            Statement::While(Expr::boolean(true), self.next, otherwise)
        })
    }

    pub(super) fn exhaust_if_empty(ready: Expr) -> Statement {
        Statement::If(not(ready), vec![Statement::Exhaust], Vec::new())
    }
}

/// The loop owns its private roots on every exit. Release them before a nonempty
/// else suite so it can observe effects such as the last file reference flushing.
pub(super) fn scoped_loop(
    mut setup: Vec<Statement>,
    clear: Vec<usize>,
    otherwise: Vec<Statement>,
    make_loop: impl FnOnce(Vec<Statement>) -> Statement,
) -> Vec<Statement> {
    let otherwise = if otherwise.is_empty() {
        otherwise
    } else {
        clear
            .iter()
            .copied()
            .map(Statement::Clear)
            .chain(otherwise)
            .collect()
    };
    setup.push(make_loop(otherwise));
    vec![Statement::Scope { body: setup, clear }]
}

pub(super) struct AccumulateArgs {
    pub(super) scope: calls::ArgumentScope,
    pub(super) source: Expr,
    pub(super) function: Expr,
    pub(super) initial: Option<Expr>,
    pub(super) element: Type,
}

impl Checker<'_> {
    /// Generator creation captures arguments now. The first pull runs the body
    /// and acquires the list yielded from; later pulls observe that same list.
    pub(super) fn record_iteration(
        &mut self,
        value: &Expr,
        node: &impl Ranged,
    ) -> Result<Option<Prepared>, String> {
        if value.ty == Type::Range {
            let cursor = self.range_cursor(value.clone());
            return Ok(Some(self.prepare_cursor(cursor, Type::Int)));
        }
        if let Some(element) = self.types.cursor_element(value.ty) {
            return Ok(Some(self.prepare_cursor(value.clone(), element)));
        }
        let mut evaluate = Vec::new();
        let mut roots = Vec::new();
        let mut eager = None;
        let call = if self.has_delegated_call(value) {
            self.capture_delegated_call(value.clone(), &mut evaluate, &mut roots, &mut eager)
        } else if let Type::Class(class) = value.ty
            && self.classes.has_record_iterator(class)
        {
            let mut receiver = value.clone();
            evaluate.push(self.capture_generator_operand(&mut receiver, &mut roots));
            self.record_primitive(receiver, "__iter__", &[], node)?
        } else {
            return Ok(None);
        };
        let mut prepared = self.prepare_container(call, false, node, false)?;
        if let Some(list) = eager {
            evaluate.insert(0, Statement::Assign(list.selected, Expr::boolean(false)));
            prepared.eager_list = Some((node.range(), list));
        }
        let started = self.hidden(Type::Bool);
        let mut initialize = std::mem::replace(&mut prepared.evaluate, evaluate);
        initialize.extend(std::mem::replace(
            &mut prepared.acquire,
            vec![Statement::Assign(started, Expr::boolean(false))],
        ));
        initialize.push(Statement::Assign(started, Expr::boolean(true)));
        prepared.next.insert(
            0,
            Statement::If(not(self.local(started)), initialize, Vec::new()),
        );
        prepared.clear.push(started);
        prepared.clear.extend(roots);
        Ok(Some(prepared))
    }

    /// Identify generator branches in the checked dispatch expression.
    fn has_delegated_call(&self, value: &Expr) -> bool {
        match &value.kind {
            ExprKind::Call(id, _) => self.classes.generators.contains(id),
            ExprKind::Scope { value, .. } => self.has_delegated_call(value),
            ExprKind::Conditional(_, yes, no) => {
                self.has_delegated_call(yes) || self.has_delegated_call(no)
            }
            _ => false,
        }
    }

    /// Evaluate dispatch and argument binding at construction. The returned expression
    /// invokes only the selected method, using those captured operands on the first pull.
    fn capture_delegated_call(
        &mut self,
        mut call: Expr,
        evaluate: &mut Vec<Statement>,
        roots: &mut Vec<usize>,
        eager: &mut Option<EagerList>,
    ) -> Expr {
        match call.kind {
            ExprKind::Call(id, ref mut args) if self.classes.generators.contains(&id) => {
                for arg in &mut args.values {
                    evaluate.push(self.capture_generator_operand(arg, roots));
                }
                self.calls.insert(id);
                call
            }
            ExprKind::Scope {
                mut body,
                value,
                clear,
            } => {
                let delayed = self.capture_delegated_call(*value, &mut body, roots, eager);
                evaluate.push(Statement::Expr(Expr {
                    ty: Type::None,
                    kind: ExprKind::Scope {
                        body,
                        value: Box::new(Expr::none()),
                        clear,
                    },
                }));
                delayed
            }
            ExprKind::Conditional(mut test, yes, no) => {
                evaluate.push(self.capture_generator_operand(&mut test, roots));
                let mut on_yes = Vec::new();
                let mut on_no = Vec::new();
                let yes = self.capture_delegated_call(*yes, &mut on_yes, roots, eager);
                let no = self.capture_delegated_call(*no, &mut on_no, roots, eager);
                evaluate.push(Statement::If((*test).clone(), on_yes, on_no));
                Expr {
                    ty: call.ty,
                    kind: ExprKind::Conditional(test, Box::new(yes), Box::new(no)),
                }
            }
            _ => {
                let list = eager.get_or_insert_with(|| {
                    let value = self.hidden(call.ty);
                    let selected = self.hidden(Type::Bool);
                    roots.extend([value, selected]);
                    EagerList { value, selected }
                });
                evaluate.push(Statement::Assign(list.value, call));
                evaluate.push(Statement::Assign(list.selected, Expr::boolean(true)));
                self.local(list.value)
            }
        }
    }

    /// Keep one evaluated operand live until the prepared iterator is released.
    fn capture_generator_operand(&mut self, value: &mut Expr, roots: &mut Vec<usize>) -> Statement {
        let slot = self.hidden(value.ty);
        roots.push(slot);
        Statement::Assign(slot, std::mem::replace(value, self.local(slot)))
    }
}

/// These constructors have observable acquisition timing and cannot be passed
/// into an eager comprehension helper as ordinary captured container values.
pub(super) fn is_pipeline_expression(checker: &Checker<'_>, expression: &ast::Expr) -> bool {
    matches!(expression, ast::Expr::Generator(_))
        || matches!(expression, ast::Expr::Call(call)
            if checker.generator_signature(call).is_some()
            || checker.resolve(&call.func) == Some(Binding::Intrinsic(Intrinsic::Chain))
            || checker.is_chain_from_iterable(call)
            || checker.is_builtin_iter(call)
            || matches!(&*call.func, ast::Expr::Name(name)
                if checker.resolve(&call.func).is_none()
                    && matches!(name.id.as_str(), "zip" | "reversed" | "map" | "filter" | "enumerate"))
            || matches!(&*call.func, ast::Expr::Attribute(attribute)
                if matches!(attribute.attr.as_str(), "iterdir" | "glob" | "rglob" | "walk")))
}

fn binary(operation: Binary, left: Expr, right: Expr) -> Expr {
    Expr {
        ty: Type::Int,
        kind: ExprKind::Binary(operation, Box::new(left), Box::new(right)),
    }
}

pub(super) fn compare(operation: Compare, left: Expr, right: Expr) -> Expr {
    Expr {
        ty: Type::Bool,
        kind: ExprKind::Compare(vec![left, right], vec![operation.operator()]),
    }
}

fn not(value: Expr) -> Expr {
    Expr::not(value)
}

/// Handle exhaustion only inside the iterator or generator operation being pulled.
fn catch_stop_iteration(
    types: &Types,
    body: Vec<Statement>,
    on_stop: Vec<Statement>,
) -> Vec<Statement> {
    if !crate::effects::may_raise_locally(&body, types) {
        return body;
    }
    vec![Statement::Try {
        body,
        handlers: vec![ir::ExceptHandler {
            matcher: crate::ir::ExceptionMatcher::Categories(vec![
                exception_kind("StopIteration").unwrap(),
            ]),
            name: None,
            body: on_stop,
        }],
        orelse: Vec::new(),
        finalbody: Vec::new(),
    }]
}

fn raise_value_error(message: String) -> Statement {
    Statement::raise(Expr {
        ty: Type::Exception,
        kind: ExprKind::Exception {
            kind: exception_kind("ValueError").unwrap(),
            message: Some(Box::new(Expr {
                ty: Type::Str,
                kind: ExprKind::Str(message),
            })),
        },
    })
}

pub(super) fn check_range_step(step: Expr) -> Statement {
    Statement::If(
        compare(Compare::Equal, step, Expr::integer(0)),
        vec![raise_value_error("range() arg 3 must not be zero".into())],
        Vec::new(),
    )
}

impl Checker<'_> {
    pub(super) fn is_builtin_iter(&self, call: &ast::ExprCall) -> bool {
        self.unshadowed_builtin(&call.func, "iter")
            && !self.is_class_receiver(&call.func)
            && self
                .graph
                .session
                .is_stdlib_function(self.module, &call.func, "builtins", "iter")
    }

    fn is_chain_from_iterable(&self, call: &ast::ExprCall) -> bool {
        let ast::Expr::Attribute(method) = &*call.func else {
            return false;
        };
        let receiver = &*method.value;
        let mut root = receiver;
        while let ast::Expr::Attribute(attr) = root {
            root = &attr.value;
        }
        method.attr.as_str() == "from_iterable"
            && root
                .as_name_expr()
                .is_some_and(|n| !self.names.contains_key(n.id.as_str()))
            && self.resolve(receiver) == Some(Binding::Intrinsic(Intrinsic::Chain))
    }

    /// Only the literal membership test runs for a skipped field.
    pub(super) fn split_guard<'a>(&self, loop_: &'a ast::StmtFor) -> Option<&'a str> {
        let Some([ast::Expr::Name(index), ast::Expr::Name(field)]) =
            pattern_elements(&loop_.target)
        else {
            return None;
        };
        let [ast::Stmt::If(guard)] = loop_.body.as_slice() else {
            return None;
        };
        let ast::Expr::Compare(test) = &*guard.test else {
            return None;
        };
        let [ast::Expr::StringLiteral(needle), ast::Expr::Name(subject)] = test.operands.as_ref()
        else {
            return None;
        };
        (index.id != field.id
            && self
                .names
                .get(index.id.as_str())
                .is_none_or(|&slot| self.local(slot).ty == Type::Int)
            && self
                .names
                .get(field.id.as_str())
                .is_none_or(|&slot| self.local(slot).ty == Type::Str)
            && guard.elif_else_clauses.is_empty()
            && test.ops.as_ref() == [ast::CmpOp::In]
            && subject.id == field.id
            && {
                let writes = narrowing::assigned_names(&guard.body);
                !writes.contains(index.id.as_str()) && !writes.contains(field.id.as_str())
            })
        .then(|| needle.value.to_str())
    }

    fn matching_fields(&mut self, cursor: Expr) -> Prepared {
        let iterator = self.hidden(Type::Iterator);
        let index = self.hidden(Type::Int);
        let field = self.hidden(Type::Str);
        let ready = self.hidden(Type::Bool);
        let pair = self.types.tuple(vec![Type::Int, Type::Str]);
        let value = self.hidden(pair);
        Prepared {
            evaluate: vec![Statement::Assign(iterator, cursor)],
            acquire: Vec::new(),
            next: vec![
                Statement::IterNext {
                    kind: crate::ir::IterNextKind::Generic,
                    iterator,
                    key: index,
                    value: Some(field),
                    ready,
                },
                Statement::If(
                    self.local(ready),
                    vec![Statement::Assign(
                        value,
                        Expr {
                            ty: pair,
                            kind: ExprKind::Tuple(vec![self.local(index), self.local(field)]),
                        },
                    )],
                    Vec::new(),
                ),
            ],
            value,
            ready,
            elementless: false,
            eager_list: None,
            writes: HashSet::new(),
            clear: vec![iterator, index, field, ready, value],
        }
    }

    pub(super) fn is_accumulate(&self, call: &ast::ExprCall) -> bool {
        matches!(
            self.resolve(&call.func),
            Some(Binding::Intrinsic(Intrinsic::ObjectRuntimeExport(
                "itertools.accumulate"
            )))
        ) && self
            .graph
            .session
            .is_stdlib_class(self.module, &call.func, "itertools", "accumulate")
    }

    pub(super) fn accumulate_args(
        &mut self,
        call: &ast::ExprCall,
    ) -> Result<AccumulateArgs, String> {
        let positional = ParameterKind::PositionalOrKeyword;
        let params = [
            positional.parameter("iterable", Type::ListInt, None),
            positional.parameter("func", Type::None, Some(Expr::none())),
            ParameterKind::KeywordOnly.parameter("initial", Type::Int, Some(Expr::none())),
        ];
        let args = self
            .arguments_with(call, &params, None, |checker, index, node| {
                if index == 0 {
                    return checker.cursor_source_value(node);
                }
                if index == 1
                    && matches!(
                        checker.resolve(node),
                        Some(Binding::Intrinsic(Intrinsic::ObjectRuntimeExport(
                            "operator.add"
                        )))
                    )
                    && checker.graph.session.is_stdlib_function(
                        checker.module,
                        node,
                        "_operator",
                        "add",
                    )
                {
                    return Ok(Expr::none());
                }
                checker.evaluated_argument(node)
            })
            .map_err(|_| self.scoped_builtin_error("itertools.accumulate", call))?;
        let [source, func, start] = args.order[..] else {
            unreachable!()
        };
        let Some(element @ (Type::Int | Type::Float)) =
            self.cursor_source_element(args.values[source].ty)
        else {
            return Err(self.scoped_builtin_error("itertools.accumulate", call));
        };
        let operation = args.values[func].ty;
        if (operation != Type::None
            && !self
                .types
                .signature(operation)
                .is_some_and(|(params, result)| params == [element, element] && result == element))
            || (args.values[start].ty != Type::None && args.values[start].ty != element)
        {
            return Err(self.scoped_builtin_error("itertools.accumulate", call));
        }
        let mut scope = calls::ArgumentScope::default();
        let values: Vec<_> = args
            .values
            .into_iter()
            .map(|v| scope.hold(self, v))
            .collect();
        Ok(AccumulateArgs {
            element,
            source: values[source].clone(),
            function: values[func].clone(),
            initial: (values[start].ty != Type::None).then(|| values[start].clone()),
            scope,
        })
    }

    /// Each pull owns its result independently of the running total. User code
    /// may mutate the source or raise StopIteration during an addition.
    pub(super) fn prepare_accumulate(
        &mut self,
        call: &ast::ExprCall,
        args: AccumulateArgs,
    ) -> Result<Prepared, String> {
        let AccumulateArgs {
            mut scope,
            source,
            function,
            initial,
            element,
        } = args;
        let mut input = if self.types.cursor_element(source.ty).is_some() {
            self.prepare_cursor(source, element)
        } else {
            self.prepare_container(source, false, call, false)?
        };
        scope.body.append(&mut input.evaluate);
        scope.body.append(&mut input.acquire);
        scope.clear.append(&mut input.clear);
        let first = self.hidden(Type::Bool);
        let total = self.hidden(element);
        let output = self.hidden(element);
        let ready = self.hidden(Type::Bool);
        scope
            .body
            .push(Statement::Assign(first, Expr::boolean(true)));
        let custom = function.ty != Type::None;
        let operation = Expr {
            ty: element,
            kind: if custom {
                ExprKind::Invoke {
                    callable: Box::new(function),
                    args: ir::CallArgs::positional(vec![
                        self.local(total),
                        self.local(input.value),
                    ]),
                }
            } else {
                ExprKind::Binary(
                    Binary::Add,
                    Box::new(self.local(total)),
                    Box::new(self.local(input.value)),
                )
            },
        };
        let addition = Statement::Assign(total, operation);
        let update = if initial.is_some() {
            addition
        } else {
            Statement::If(
                self.local(first),
                vec![
                    Statement::Assign(first, Expr::boolean(false)),
                    Statement::Assign(total, self.local(input.value)),
                ],
                vec![addition],
            )
        };
        let mut produce = vec![update, Statement::Assign(output, self.local(total))];
        if custom {
            produce = catch_stop_iteration(
                self.types,
                produce,
                vec![Statement::Assign(ready, Expr::boolean(false))],
            );
        }
        produce.push(Statement::Clear(input.value));
        let mut next = input.next;
        next.push(Statement::Assign(ready, self.local(input.ready)));
        next.push(Statement::If(self.local(ready), produce, Vec::new()));
        if let Some(initial) = initial {
            scope.body.push(Statement::Assign(total, initial));
            next = vec![Statement::If(
                self.local(first),
                vec![
                    Statement::Assign(first, Expr::boolean(false)),
                    Statement::Assign(ready, Expr::boolean(true)),
                    Statement::Assign(output, self.local(total)),
                ],
                next,
            )];
        }
        next.insert(0, Statement::Clear(output));
        scope.clear.extend([first, total, output, ready]);
        Ok(Prepared {
            evaluate: scope.body,
            acquire: Vec::new(),
            next,
            ready,
            value: output,
            elementless: false,
            eager_list: None,
            writes: input.writes,
            clear: scope.clear,
        })
    }

    /// Return the sealed converter and its checked iterable. Eager integer consumers
    /// can use the input directly and perform the conversion while pulling.
    pub(super) fn builtin_map_input<'e>(
        &mut self,
        call: &'e ast::ExprCall,
        eager_int: bool,
    ) -> Result<Option<(&'e str, Iteration)>, String> {
        let [converter @ ast::Expr::Name(name), source] = call.arguments.args.as_ref() else {
            return Ok(None);
        };
        let name = name.id.as_str();
        if !self.unshadowed_builtin(&call.func, "map")
            || !matches!(name, "tuple" | "int" | "str" | "ord")
            || (eager_int && name != "int")
            || !self.unshadowed_builtin(converter, name)
        {
            return Ok(None);
        }
        if eager_int && !call.arguments.keywords.is_empty() {
            return Err(error(
                self.source,
                call,
                "map requires the builtin int converter and one concrete iterable",
            ));
        }
        self.reject_builtin_keywords(call)?;
        Ok(Some((name, self.expansion_input(source, None)?)))
    }

    /// Recognizes only sealed constructor syntax at a direct consumer boundary.
    pub(super) fn pipeline_input(
        &mut self,
        expression: &ast::Expr,
        element: Option<Type>,
        split_needle: Option<&str>,
    ) -> Result<Option<Iteration>, String> {
        if let Some(input) = self.enum_iteration(expression) {
            return Ok(Some(input));
        }
        if let ast::Expr::Generator(generator) = expression {
            return Ok(Some(Iteration::Prepared(
                self.generator_input(generator, element, None)?,
            )));
        }
        let ast::Expr::Call(call) = expression else {
            return Ok(None);
        };
        if self.is_accumulate(call) {
            let args = self.accumulate_args(call)?;
            return Ok(Some(Iteration::Prepared(
                self.prepare_accumulate(call, args)?,
            )));
        }
        if self.is_builtin_iter(call) && call.arguments.args.len() != 1 {
            return Ok(Some(Iteration::Prepared(self.sentinel_input(call)?)));
        }
        if matches!(
            self.resolve(&call.func),
            Some(Binding::Intrinsic(
                Intrinsic::Repeat
                    | Intrinsic::Count
                    | Intrinsic::Islice
                    | Intrinsic::Cycle
                    | Intrinsic::Combinatorial(_)
            ))
        ) {
            let cursor = self.call(call)?;
            let element = self.types.cursor_element(cursor.ty).unwrap();
            return Ok(Some(Iteration::Prepared(
                self.prepare_cursor(cursor, element),
            )));
        }
        if self.resolve(&call.func) == Some(Binding::Intrinsic(Intrinsic::Chain)) {
            return Ok(Some(Iteration::Prepared(self.chain_input(call, element)?)));
        }
        if self.is_chain_from_iterable(call) {
            return Ok(Some(Iteration::Prepared(self.chain_from_iterable(call)?)));
        }
        if let Some((signature, element)) = self.generator_signature(call) {
            let frame = self.source_call(call, &signature, None, 0)?;
            let handle = self.hidden(Type::GeneratorContext);
            let value = self.hidden(element);
            let ready = self.hidden(Type::Bool);
            let op = |kind, ty| contexts::op(kind, ty, vec![self.local(handle)]);
            let next = vec![
                Statement::Clear(value),
                Statement::Assign(ready, op(ir::ProcessOperation::GeneratorNext, Type::Bool)),
                Statement::If(
                    self.local(ready),
                    vec![Statement::Assign(
                        value,
                        op(ir::ProcessOperation::GeneratorTake, element),
                    )],
                    Vec::new(),
                ),
            ];
            return Ok(Some(Iteration::Prepared(Prepared {
                evaluate: vec![Statement::Assign(handle, frame)],
                acquire: Vec::new(),
                next,
                value,
                ready,
                elementless: false,
                eager_list: None,
                writes: HashSet::new(),
                clear: vec![handle, value, ready],
            })));
        }
        if self.resolve(&call.func).is_some() {
            return Ok(None);
        }
        if let ast::Expr::Attribute(attribute) = &*call.func
            && attribute.attr.as_str() == "values"
            && self.class_reference(&attribute.value).is_none()
        {
            let receiver = self.expr(&attribute.value)?;
            return self
                .mapping_values_iteration(call, attribute, receiver)
                .map(Some);
        }
        if let ast::Expr::Attribute(attribute) = &*call.func
            && matches!(
                attribute.attr.as_str(),
                "iterdir" | "glob" | "rglob" | "walk"
            )
            && self.class_reference(&attribute.value).is_none()
        {
            let receiver = self.expr(&attribute.value)?;
            if receiver.ty == Type::Path {
                let iterator = self.path_iterator(receiver, call, attribute.attr.as_str())?;
                let item = self.types.cursor_element(iterator.ty).unwrap();
                return Ok(Some(Iteration::Prepared(
                    self.prepare_cursor(iterator, item),
                )));
            }
            if let Type::Class(class) = receiver.ty {
                let iterable = self.iteration_method_call(call, attribute, class, receiver)?;
                return self.iteration_value(iterable, false, call).map(Some);
            }
            return Err(error(
                self.source,
                call,
                "directory iteration requires pathlib.Path",
            ));
        }
        let ast::Expr::Name(name) = &*call.func else {
            return Ok(None);
        };
        let input = match name.id.as_str() {
            "zip" => self.zip_input(call, element)?,
            "reversed" => {
                if !call.arguments.keywords.is_empty() || call.arguments.args.len() != 1 {
                    return Err(error(
                        self.source,
                        call,
                        "reversed requires one positional sequence",
                    ));
                }
                let source = &call.arguments.args[0];
                let input = self.expansion_input(source, element)?;
                let mut input = self.prepare_iteration(input, source, true)?;
                input.evaluate.append(&mut input.acquire);
                input
            }
            "map" if self.unshadowed_builtin(&call.func, "map") => {
                let Some((name, source)) = self.builtin_map_input(call, false)? else {
                    return Ok(Some(Iteration::Prepared(
                        self.callback_input(call, element, false)?,
                    )));
                };
                let mut input = self.prepare_iteration(source, call, false)?;
                let item = self.local(input.value);
                let converted = if name == "tuple" {
                    self.iterable_tuple(&item).ok_or_else(|| {
                        error(
                            self.source,
                            call,
                            "map(tuple, ...) requires concrete iterable elements",
                        )
                    })?
                } else if name == "ord" {
                    self.ordinal(item, call)?
                } else if name == "int" {
                    self.consumer_value(item, true, call)?
                } else {
                    match item.ty {
                        Type::Path => {
                            paths::operation(ir::PathOperation::Text, Type::Str, vec![item])
                        }
                        Type::Str => item,
                        Type::Class(_) | Type::CompletedProcess(..) => {
                            self.record_text(item, call)?
                        }
                        Type::Int | Type::Bool | Type::Float | Type::None | Type::Bytes => Expr {
                            ty: Type::Str,
                            kind: ExprKind::Builtin(Builtin::Str, vec![item]),
                        },
                        _ => {
                            return Err(error(
                                self.source,
                                call,
                                "map(str, ...) requires values with a supported string conversion",
                            ));
                        }
                    }
                };
                let value = self.hidden(converted.ty);
                input.next.push(Statement::If(
                    self.local(input.ready),
                    vec![
                        Statement::Assign(value, converted),
                        Statement::Clear(input.value),
                    ],
                    Vec::new(),
                ));
                input.next.insert(0, Statement::Clear(value));
                input.value = value;
                input.elementless = false;
                input.clear.push(value);
                input.evaluate.append(&mut input.acquire);
                input
            }
            "filter" => self.callback_input(call, element, true)?,
            "enumerate" => {
                if !call.arguments.keywords.is_empty()
                    || !(1..=2).contains(&call.arguments.args.len())
                {
                    return Err(error(
                        self.source,
                        call,
                        "enumerate requires an iterable and an optional positional integer start",
                    ));
                }
                let fields =
                    element.and_then(|ty| self.types.fields(self.types.payload(ty).unwrap_or(ty)));
                let element = fields.as_deref().and_then(|fields| match fields {
                    [Type::Int, element] => Some(*element),
                    _ => None,
                });
                let source = self.expansion_input(&call.arguments.args[0], element)?;
                if let Some(needle) = split_needle
                    && let Some((text, separator)) = byte_split_source(&source)
                    && !needle.is_empty()
                    && !needle.as_bytes().contains(&separator)
                {
                    let start = if let Some(expr) = call.arguments.args.get(1) {
                        self.expr_expected(expr, Type::Int)?
                    } else {
                        Expr::integer(0)
                    };
                    let cursor = Expr {
                        ty: Type::Iterator,
                        kind: ExprKind::Builtin(
                            Builtin::MatchingSplitIter(separator),
                            vec![text.clone(), unpacking::string(needle), start],
                        ),
                    };
                    return Ok(Some(Iteration::Prepared(self.matching_fields(cursor))));
                }
                let source = self.fuse_split(source);
                let mut input = self.prepare_iteration(source, call, false)?;
                self.hint_empty_iteration(&input, element);
                let start = if let Some(start) = call.arguments.args.get(1) {
                    self.expr_expected(start, Type::Int)?
                } else {
                    Expr::integer(0)
                };
                let counter = self.hidden(Type::Int);
                input.evaluate.push(Statement::Assign(counter, start));
                input.evaluate.append(&mut input.acquire);
                let ty = self.types.tuple(vec![Type::Int, self.locals[input.value]]);
                let value = self.hidden(ty);
                let cache = self.iterator_tuple_cache(ty, &mut input.evaluate, &mut input.clear);
                let mut result = if let Some(cache) = cache {
                    vec![
                        Statement::Assign(
                            value,
                            Expr {
                                ty,
                                kind: ExprKind::Builtin(
                                    Builtin::TupleCacheResult,
                                    vec![self.local(cache)],
                                ),
                            },
                        ),
                        Statement::SetField(self.local(value), 0, self.local(counter)),
                        Statement::SetField(self.local(value), 1, self.local(input.value)),
                    ]
                } else {
                    vec![Statement::Assign(
                        value,
                        Expr {
                            ty,
                            kind: ExprKind::Tuple(vec![
                                self.local(counter),
                                self.local(input.value),
                            ]),
                        },
                    )]
                };
                result.extend([
                    Statement::Clear(input.value),
                    Statement::Assign(
                        counter,
                        binary(Binary::Add, self.local(counter), Expr::integer(1)),
                    ),
                ]);
                input
                    .next
                    .push(Statement::If(self.local(input.ready), result, Vec::new()));
                input.next.insert(0, Statement::Clear(value));
                input.value = value;
                input.elementless = false;
                input.clear.extend([counter, value]);
                input
            }
            _ => return Ok(None),
        };
        Ok(Some(Iteration::Prepared(input)))
    }

    /// A sentinel iterator owns its stop value and latches exhaustion at first match.
    fn sentinel_input(&mut self, call: &ast::ExprCall) -> Result<Prepared, String> {
        self.reject_builtin_keywords(call)?;
        let [ast::Expr::Lambda(lambda), sentinel] = call.arguments.args.as_ref() else {
            return Err(error(
                self.source,
                call,
                "iter requires a nullary lambda and a sentinel",
            ));
        };
        let sentinel = self.expr(sentinel)?;
        let item = self.pull_lambda(lambda)?;
        if !matches!(sentinel.ty, Type::Str | Type::Bytes) || item.ty != sentinel.ty {
            return Err(error(
                self.source,
                call,
                "iter requires a producer and sentinel of the same exact str or bytes type",
            ));
        }
        let stop = self.hidden(sentinel.ty);
        let value = self.hidden(item.ty);
        let ready = self.hidden(Type::Bool);
        let exhaust = vec![
            Statement::Assign(ready, Expr::boolean(false)),
            Statement::Clear(stop),
            Statement::Clear(value),
        ];
        // Catch only exhaustion raised by the producer. Construction, the caller's
        // loop body, and other producer exceptions keep their ordinary handlers.
        let pull = Statement::Try {
            body: vec![Statement::Assign(value, item)],
            handlers: vec![ir::ExceptHandler {
                matcher: crate::ir::ExceptionMatcher::Categories(vec![
                    exception_kind("StopIteration").unwrap(),
                ]),
                name: None,
                body: exhaust.clone(),
            }],
            orelse: vec![Statement::If(
                compare(Compare::Equal, self.local(stop), self.local(value)),
                exhaust,
                Vec::new(),
            )],
            finalbody: Vec::new(),
        };
        Ok(Prepared {
            evaluate: vec![
                Statement::Assign(stop, sentinel),
                Statement::Assign(ready, Expr::boolean(true)),
            ],
            acquire: Vec::new(),
            next: vec![Statement::If(self.local(ready), vec![pull], Vec::new())],
            value,
            ready,
            elementless: false,
            eager_list: None,
            writes: HashSet::new(),
            clear: vec![stop, value, ready],
        })
    }

    /// Evaluate chain's arguments at construction, but acquire each iterator
    /// only when its predecessor is exhausted and the consumer wants another item.
    fn chain_input(
        &mut self,
        call: &ast::ExprCall,
        hint: Option<Type>,
    ) -> Result<Prepared, String> {
        self.reject_builtin_keywords(call)?;
        let mut inputs = Vec::new();
        let mut element = None;
        for arg in &call.arguments.args {
            let input = self.expansion_input(arg, hint)?;
            let input = self.prepare_iteration(input, arg, false)?;
            if !input.elementless {
                let ty = self.locals[input.value];
                if let Some(previous) = element {
                    self.expect(previous, ty, arg)?;
                }
                element = Some(ty);
            }
            inputs.push(input);
        }
        let value = self.hidden(element.or(hint).unwrap_or(Type::Int));
        let ready = self.hidden(Type::Bool);
        let active = self.hidden(Type::Int);
        let acquired = self.hidden(Type::Bool);
        let mut evaluate = vec![
            Statement::Assign(active, Expr::integer(0)),
            Statement::Assign(acquired, Expr::boolean(false)),
        ];
        let mut next = vec![
            Statement::Clear(value),
            Statement::Assign(ready, Expr::boolean(false)),
        ];
        let mut clear = vec![value, ready, active, acquired];
        let mut writes = HashSet::new();
        for (index, mut input) in inputs.into_iter().enumerate() {
            evaluate.append(&mut input.evaluate);
            clear.extend(input.clear);
            writes.extend(input.writes);
            input
                .acquire
                .push(Statement::Assign(acquired, Expr::boolean(true)));
            let mut pull = vec![Statement::If(
                not(self.local(acquired)),
                input.acquire,
                Vec::new(),
            )];
            pull.append(&mut input.next);
            let yielded = if input.elementless {
                Vec::new()
            } else {
                vec![
                    Statement::Assign(value, self.local(input.value)),
                    Statement::Clear(input.value),
                    Statement::Assign(ready, Expr::boolean(true)),
                ]
            };
            pull.push(Statement::If(
                self.local(input.ready),
                yielded,
                vec![
                    Statement::Assign(active, Expr::integer(index as i64 + 1)),
                    Statement::Assign(acquired, Expr::boolean(false)),
                ],
            ));
            next.push(Statement::If(
                compare(
                    Compare::Equal,
                    self.local(active),
                    Expr::integer(index as i64),
                ),
                pull,
                Vec::new(),
            ));
        }
        Ok(Prepared {
            evaluate,
            acquire: Vec::new(),
            next,
            value,
            ready,
            elementless: element.is_none(),
            eager_list: None,
            writes,
            clear,
        })
    }

    /// Acquire the outer iterator at construction and each inner iterator on demand.
    fn chain_from_iterable(&mut self, call: &ast::ExprCall) -> Result<Prepared, String> {
        self.reject_builtin_keywords(call)?;
        let [source] = call.arguments.args.as_ref() else {
            return Err(error(
                self.source,
                call,
                "chain.from_iterable requires one positional iterable",
            ));
        };
        let input = self.expansion_input(source, None)?;
        let mut outer = self.prepare_iteration(input, source, false)?;
        outer.evaluate.append(&mut outer.acquire);
        outer.eager_list = None;
        if outer.elementless {
            return Ok(outer);
        }

        let mut inner = self.prepare_container(self.local(outer.value), false, call, false)?;
        let active = self.hidden(Type::Bool);
        let done = self.hidden(Type::Bool);
        outer.evaluate.extend([
            Statement::Assign(active, Expr::boolean(false)),
            Statement::Assign(done, Expr::boolean(false)),
        ]);

        // An outer failure or a non-iterable item permanently exhausts a chain.
        // A failure while pulling an active inner iterator keeps that iterator.
        let mut advance = vec![Statement::Assign(done, Expr::boolean(true))];
        advance.append(&mut outer.next);
        let mut exhausted: Vec<_> = outer.clear.iter().copied().map(Statement::Clear).collect();
        exhausted.push(Statement::Break);
        advance.push(Statement::If(
            not(self.local(outer.ready)),
            exhausted,
            Vec::new(),
        ));
        advance.append(&mut inner.evaluate);
        advance.append(&mut inner.acquire);
        advance.extend([
            Statement::Clear(outer.value),
            Statement::Assign(active, Expr::boolean(true)),
            Statement::Assign(done, Expr::boolean(false)),
        ]);
        let mut pull = vec![Statement::If(not(self.local(active)), advance, Vec::new())];
        pull.append(&mut inner.next);
        pull.push(Statement::If(
            self.local(inner.ready),
            vec![Statement::Break],
            Vec::new(),
        ));
        pull.extend(inner.clear.iter().copied().map(Statement::Clear));
        pull.push(Statement::Assign(active, Expr::boolean(false)));
        outer.clear.extend(inner.clear);
        outer.clear.extend([active, done]);
        Ok(Prepared {
            next: vec![
                Statement::Assign(inner.ready, Expr::boolean(false)),
                Statement::While(not(self.local(done)), pull, Vec::new()),
            ],
            value: inner.value,
            ready: inner.ready,
            elementless: inner.elementless,
            ..outer
        })
    }

    /// Capture the immutable receiver once, before evaluating an enumerate start.
    /// The fresh split list is unobservable at these direct iteration sites.
    pub(super) fn fuse_split(&mut self, input: Iteration) -> Iteration {
        let (builtin, args) = if let Some((text, byte)) = byte_split_source(&input) {
            (Builtin::SplitByteIter(byte), vec![text.clone()])
        } else if let Iteration::Container {
            iterable:
                Expr {
                    kind: ExprKind::Builtin(Builtin::SplitLines, args),
                    ..
                },
            items: false,
            start: None,
        } = input
        {
            (Builtin::SplitLinesIter, args)
        } else {
            return input;
        };
        let cursor = Expr {
            ty: Type::Iterator,
            kind: ExprKind::Builtin(builtin, args),
        };
        Iteration::Prepared(self.prepare_cursor(cursor, Type::Str))
    }

    /// Consume a checked private split, or a direct sorted split of a string local.
    pub(super) fn prepared_split_iteration(
        &mut self,
        expression: &ast::Expr,
    ) -> Result<Option<Iteration>, String> {
        let (input, sorted) = match expression {
            ast::Expr::Name(_) => (expression, false),
            ast::Expr::Call(call)
                if self.unshadowed_builtin(&call.func, "sorted")
                    && call.arguments.keywords.is_empty() =>
            {
                let [input] = call.arguments.args.as_ref() else {
                    return Ok(None);
                };
                (input, true)
            }
            _ => return Ok(None),
        };
        let (receiver, byte) =
            if let Some(&byte) = self.consuming_lists.split_consumers.get(&input.range()) {
                (self.expr(input)?, byte)
            } else if sorted
                && let ast::Expr::Call(split) = input
                && split.arguments.keywords.is_empty()
                && let [ast::Expr::StringLiteral(sep)] = split.arguments.args.as_ref()
                && let [byte] = *sep.value.to_str().as_bytes()
                && let ast::Expr::Attribute(method) = &*split.func
                && method.attr.as_str() == "split"
                && let ast::Expr::Name(name) = &*method.value
                && self
                    .names
                    .get(name.id.as_str())
                    .is_some_and(|slot| self.locals[*slot] == Type::Str)
            {
                (self.expr(&method.value)?, byte)
            } else {
                return Ok(None);
            };
        debug_assert_eq!(receiver.ty, Type::Str);
        let op = if sorted {
            Builtin::SortedSplitByteIter(byte)
        } else {
            Builtin::SplitByteIter(byte)
        };
        let cursor = Expr {
            ty: Type::Iterator,
            kind: ExprKind::Builtin(op, vec![receiver]),
        };
        Ok(Some(Iteration::Prepared(
            self.prepare_cursor(cursor, Type::Str),
        )))
    }

    pub(super) fn mapping_values_iteration(
        &mut self,
        call: &ast::ExprCall,
        attribute: &ast::ExprAttribute,
        raw: Expr,
    ) -> Result<Iteration, String> {
        if raw.ty == Type::PythonObject || (raw.ty == Type::JsonObject && call.arguments.is_empty())
        {
            return self.object_method_iteration(call, attribute, raw);
        }
        if let Type::Class(class) = raw.ty {
            let iterable = self.iteration_method_call(call, attribute, class, raw)?;
            return self.iteration_value(iterable, false, call);
        }
        let source = if matches!(raw.ty, Type::Document(_)) {
            self.document_map(raw)
        } else {
            raw
        };
        let (_, item) = self
            .types
            .mapping(source.ty)
            .ok_or_else(|| error(self.source, call, "values() requires a concrete dictionary"))?;
        if !call.arguments.is_empty() {
            return Err(error(self.source, call, "dict.values() takes no arguments"));
        }
        let mut input = self.prepare_container(source, true, call, false)?;
        let pair = input.value;
        let value = self.hidden(item);
        input.next.push(Statement::If(
            self.local(input.ready),
            vec![
                Statement::Assign(
                    value,
                    Expr {
                        ty: item,
                        kind: ExprKind::Field(Box::new(self.local(pair)), 1),
                    },
                ),
                Statement::Clear(pair),
            ],
            Vec::new(),
        ));
        input.value = value;
        input.clear.push(value);
        Ok(Iteration::Prepared(input))
    }

    pub(super) fn prepare_cursor(&mut self, cursor: Expr, element: Type) -> Prepared {
        Prepared::cursor(cursor, element, |ty| self.hidden(ty))
    }

    pub(super) fn prepare_csv_reader(&mut self, reader: Expr) -> Prepared {
        self.prepare_pull_iterator(
            reader,
            self.types.list(Type::Str),
            Builtin::CsvNext { required: false },
        )
    }

    fn prepare_pull_iterator(
        &mut self,
        expression: Expr,
        element: Type,
        operation: Builtin,
    ) -> Prepared {
        let iterator = self.hidden(expression.ty);
        let optional = self.hidden(self.types.optional(element));
        let value = self.hidden(element);
        let ready = self.hidden(Type::Bool);
        let done = self.hidden(Type::Bool);
        let pull = vec![
            Statement::Assign(
                optional,
                Expr {
                    ty: self.locals[optional],
                    kind: ExprKind::Builtin(operation, vec![self.local(iterator)]),
                },
            ),
            Statement::Assign(
                ready,
                not(Expr {
                    ty: Type::Bool,
                    kind: ExprKind::Builtin(Builtin::IsNone, vec![self.local(optional)]),
                }),
            ),
            Statement::If(
                self.local(ready),
                vec![Statement::Assign(
                    value,
                    Expr {
                        ty: element,
                        kind: ExprKind::Unwrap(Box::new(self.local(optional))),
                    },
                )],
                vec![
                    Statement::Assign(done, Expr::boolean(true)),
                    Statement::Clear(iterator),
                ],
            ),
            Statement::Clear(optional),
        ];
        Prepared {
            evaluate: vec![
                Statement::Assign(iterator, expression),
                Statement::Assign(done, Expr::boolean(false)),
            ],
            acquire: Vec::new(),
            next: vec![
                Statement::Clear(value),
                Statement::Assign(ready, Expr::boolean(false)),
                Statement::If(not(self.local(done)), pull, Vec::new()),
            ],
            value,
            ready,
            elementless: false,
            eager_list: None,
            writes: HashSet::new(),
            clear: vec![iterator, optional, value, ready, done],
        }
    }

    /// Consume one prepared iterator into a fresh set and release the partial
    /// result if evaluating the source or pulling an element raises.
    pub(super) fn collect_set(
        &mut self,
        source: Prepared,
        node: &impl Ranged,
    ) -> Result<Expr, String> {
        let element = self.locals[source.value];
        if !self.types.hashable_key(element) {
            return Err(error(
                self.source,
                node,
                "set() requires hashable iterator elements",
            ));
        }
        let ty = self.types.set(element);
        let result = self.hidden(ty);
        let empty = Expr {
            ty,
            kind: ExprKind::Set {
                element,
                values: Vec::new(),
            },
        };
        let body = self.expand_items_after(
            Iteration::Prepared(source),
            node,
            vec![Statement::Assign(result, empty)],
            |checker, value| {
                Ok(vec![Statement::Expr(Expr {
                    ty: Type::None,
                    kind: ExprKind::Builtin(Builtin::SetAdd, vec![checker.local(result), value]),
                })])
            },
        )?;
        Ok(Expr {
            ty,
            kind: ExprKind::Scope {
                body,
                value: Box::new(self.local(result)),
                clear: vec![result],
            },
        })
    }

    fn may_own_file(&self, ty: Type, opaque_callables: bool) -> bool {
        self.types
            .may_own_file(ty, &self.classes.fields, opaque_callables)
    }

    /// Preserve CPython's iterator tuple lifetime when dropping a component can
    /// close a file, remove a directory, or release a suspended frame.
    fn iterator_tuple_cache(
        &mut self,
        ty: Type,
        evaluate: &mut Vec<Statement>,
        clear: &mut Vec<usize>,
    ) -> Option<usize> {
        if !self.may_own_file(ty, false) {
            return None;
        }
        let cache = self.hidden(ty);
        evaluate.push(Statement::Assign(
            cache,
            Expr {
                ty,
                kind: ExprKind::Builtin(Builtin::TupleCacheNew, Vec::new()),
            },
        ));
        clear.push(cache);
        Some(cache)
    }

    /// Keep the selected iterator live across the body. A dictionary retains
    /// its mutation checks, while a record's items method supplies its own iterable.
    pub(super) fn dict_or_record_items(
        &mut self,
        receiver: Expr,
        attr: &ast::ExprAttribute,
        call: &ast::ExprCall,
    ) -> Result<Prepared, String> {
        if !call.arguments.is_empty() {
            return Err(error(self.source, call, "dict.items() takes no arguments"));
        }
        let (slot, test, dict, record) = self.protocol_alternatives(receiver.ty).unwrap();
        let Type::Class(class) = record.ty else {
            unreachable!()
        };
        let items = self.iteration_method_call(call, attr, class, record)?;
        let mut a = self.prepare_container(dict, true, call, false)?;
        let mut b = self.prepare_container(items, false, call, false)?;
        self.expect(self.locals[a.value], self.locals[b.value], call)?;
        let value = self.hidden(self.locals[a.value]);
        let ready = self.hidden(Type::Bool);
        let selected = self.hidden(Type::Bool);
        for p in [&mut a, &mut b] {
            p.next.push(Statement::Assign(ready, self.local(p.ready)));
            p.next.push(Statement::If(
                self.local(ready),
                vec![
                    Statement::Assign(value, self.local(p.value)),
                    Statement::Clear(p.value),
                ],
                vec![],
            ));
        }
        a.clear.extend(b.clear);
        a.clear.extend([slot, selected, ready, value]);
        a.writes.extend(b.writes);
        Ok(Prepared {
            evaluate: vec![
                Statement::Assign(slot, receiver),
                Statement::Assign(selected, test),
                Statement::If(self.local(selected), a.evaluate, b.evaluate),
            ],
            acquire: vec![Statement::If(self.local(selected), a.acquire, b.acquire)],
            next: vec![Statement::If(self.local(selected), a.next, b.next)],
            ready,
            value,
            elementless: false,
            eager_list: None,
            writes: a.writes,
            clear: a.clear,
        })
    }

    /// Adapts existing concrete iteration without introducing a public iterator
    /// type. Reverse construction supports sequences whose indexing is sealed.
    pub(super) fn prepare_iteration(
        &mut self,
        input: Iteration,
        node: &impl Ranged,
        reverse: bool,
    ) -> Result<Prepared, String> {
        match input {
            Iteration::Prepared(input) if !reverse => Ok(input),
            Iteration::Prepared(mut input) if let Some(list) = input.eager_list_at(node) => {
                let mut reverse =
                    self.prepare_container(self.local(list.value), false, node, true)?;
                input.evaluate.push(Statement::If(
                    not(self.local(list.selected)),
                    vec![Statement::Expr(calls::type_error(
                        Type::None,
                        Vec::new(),
                        "'generator' object is not reversible".into(),
                    ))],
                    Vec::new(),
                ));
                input.evaluate.append(&mut reverse.evaluate);
                reverse.evaluate = input.evaluate;
                reverse.clear.extend(input.clear);
                Ok(reverse)
            }
            Iteration::Prepared(_) => Err(error(
                self.source,
                node,
                "reversed requires a list, tuple, string, bytes, or range",
            )),
            Iteration::Range(args) => Ok(self.prepare_range(args, reverse)),
            Iteration::Container {
                iterable,
                items,
                start: None,
            } => self.prepare_container(iterable, items, node, reverse),
            Iteration::Container { start: Some(_), .. } => {
                unreachable!("enumerate constructors use the shared pipeline path")
            }
        }
    }

    /// A finite range keeps arbitrary-precision cursor arithmetic. Constructing
    /// it checks the step before later call arguments; reversing never uses len.
    fn prepare_range(&mut self, args: Vec<Expr>, reverse: bool) -> Prepared {
        let mut evaluate = Vec::new();
        let mut arguments = Vec::new();
        for argument in args {
            let slot = self.hidden(Type::Int);
            evaluate.push(Statement::Assign(slot, argument));
            arguments.push(slot);
        }
        let (start, stop, step) = match arguments.as_slice() {
            [stop] => (Expr::integer(0), self.local(*stop), Expr::integer(1)),
            [start, stop] => (self.local(*start), self.local(*stop), Expr::integer(1)),
            [start, stop, step] => (self.local(*start), self.local(*stop), self.local(*step)),
            _ => unreachable!("range arity was checked"),
        };
        if arguments.len() == 3 {
            evaluate.push(check_range_step(step.clone()));
        }
        let cursor = self.hidden(Type::Int);
        let end = self.hidden(Type::Int);
        let stride = self.hidden(Type::Int);
        let value = self.hidden(Type::Int);
        let ready = self.hidden(Type::Bool);
        let done = self.hidden(Type::Bool);
        let positive = compare(Compare::Greater, step.clone(), Expr::integer(0));
        let nonempty = Expr {
            ty: Type::Bool,
            kind: ExprKind::Conditional(
                Box::new(positive.clone()),
                Box::new(compare(Compare::Less, start.clone(), stop.clone())),
                Box::new(compare(Compare::Greater, start.clone(), stop.clone())),
            ),
        };
        let mut acquire = vec![Statement::Assign(done, Expr::boolean(false))];
        if reverse {
            let sign = Expr {
                ty: Type::Int,
                kind: ExprKind::Conditional(
                    Box::new(positive),
                    Box::new(Expr::integer(1)),
                    Box::new(Expr::integer(-1)),
                ),
            };
            let last = binary(
                Binary::Add,
                start.clone(),
                binary(
                    Binary::Multiply,
                    binary(
                        Binary::FloorDivide,
                        binary(
                            Binary::Subtract,
                            binary(Binary::Subtract, stop, start.clone()),
                            sign,
                        ),
                        step.clone(),
                    ),
                    step.clone(),
                ),
            );
            acquire.push(Statement::If(
                nonempty,
                vec![
                    Statement::Assign(cursor, last),
                    Statement::Assign(end, binary(Binary::Subtract, start, step.clone())),
                    Statement::Assign(
                        stride,
                        Expr {
                            ty: Type::Int,
                            kind: ExprKind::Unary(UnaryOp::USub, Box::new(step)),
                        },
                    ),
                ],
                vec![
                    Statement::Assign(cursor, Expr::integer(0)),
                    Statement::Assign(end, Expr::integer(0)),
                    Statement::Assign(stride, Expr::integer(1)),
                ],
            ));
        } else {
            acquire.extend([
                Statement::Assign(cursor, start),
                Statement::Assign(end, stop),
                Statement::Assign(stride, step),
            ]);
        }
        let available = Expr {
            ty: Type::Bool,
            kind: ExprKind::Conditional(
                Box::new(compare(
                    Compare::Greater,
                    self.local(stride),
                    Expr::integer(0),
                )),
                Box::new(compare(Compare::Less, self.local(cursor), self.local(end))),
                Box::new(compare(
                    Compare::Greater,
                    self.local(cursor),
                    self.local(end),
                )),
            ),
        };
        let next = vec![
            Statement::Clear(value),
            Statement::Assign(ready, Expr::boolean(false)),
            Statement::If(
                not(self.local(done)),
                vec![Statement::If(
                    available,
                    vec![
                        Statement::Assign(value, self.local(cursor)),
                        Statement::Assign(
                            cursor,
                            binary(Binary::Add, self.local(cursor), self.local(stride)),
                        ),
                        Statement::Assign(ready, Expr::boolean(true)),
                    ],
                    vec![Statement::Assign(done, Expr::boolean(true))],
                )],
                Vec::new(),
            ),
        ];
        arguments.extend([cursor, end, stride, value, ready, done]);
        Prepared {
            evaluate,
            acquire,
            next,
            value,
            ready,
            elementless: false,
            eager_list: None,
            writes: HashSet::new(),
            clear: arguments,
        }
    }

    /// A file is its own iterator. Acquiring it checks for closure, and pulls
    /// share its position with every reader, including a nested loop on it.
    pub(super) fn prepare_file_lines(&mut self, file: Expr) -> Prepared {
        let line_type = if matches!(file.ty, Type::BinaryIO | Type::BytesIO) {
            Type::Bytes
        } else {
            Type::Str
        };
        let source = self.hidden(file.ty);
        let value = self.hidden(line_type);
        let ready = self.hidden(Type::Bool);
        Prepared {
            evaluate: vec![Statement::Assign(source, file)],
            acquire: vec![Statement::Expr(Expr {
                ty: Type::None,
                kind: ExprKind::Builtin(Builtin::TextEnter, vec![self.local(source)]),
            })],
            next: vec![
                Statement::Assign(
                    value,
                    Expr {
                        ty: line_type,
                        kind: ExprKind::Builtin(Builtin::FileReadline, vec![self.local(source)]),
                    },
                ),
                Statement::Assign(ready, self.truth_value(self.local(value))),
            ],
            value,
            ready,
            elementless: false,
            eager_list: None,
            writes: HashSet::new(),
            clear: vec![source, value, ready],
        }
    }

    /// List cursors re-read the live length; reverse cursors capture only their
    /// starting index. Dict/set/string cursors use the rooted runtime protocol.
    pub(super) fn prepare_container(
        &mut self,
        iterable: Expr,
        items: bool,
        node: &impl Ranged,
        reverse: bool,
    ) -> Result<Prepared, String> {
        let iterable = if items {
            iterable
        } else {
            self.json_iteration(iterable)
        };
        if !reverse && iterable.ty == Type::CsvReader {
            return Ok(self.prepare_csv_reader(iterable));
        }
        if !reverse
            && matches!(
                iterable.ty,
                Type::TextIO | Type::StringIO | Type::BinaryIO | Type::BytesIO
            )
        {
            return Ok(self.prepare_file_lines(iterable));
        }
        let mapping = self.types.mapping(iterable.ty);
        let set_element = self.types.readable_set_element(iterable.ty);
        let fields = self.types.fields(iterable.ty);
        if fields.as_ref().is_some_and(Vec::is_empty) {
            let source = self.hidden(iterable.ty);
            let value = self.hidden(Type::Int);
            let ready = self.hidden(Type::Bool);
            return Ok(Prepared {
                evaluate: vec![Statement::Assign(source, iterable)],
                acquire: Vec::new(),
                next: vec![
                    Statement::Clear(value),
                    Statement::Assign(ready, Expr::boolean(false)),
                ],
                value,
                ready,
                elementless: true,
                eager_list: None,
                writes: HashSet::new(),
                clear: vec![source, value, ready],
            });
        }
        let tuple_element = fields
            .as_deref()
            .and_then(|fields| self.types.tuple_iteration_element(fields));
        let element = self.types.iteration_element(iterable.ty, items)
            .or(tuple_element)
            .ok_or_else(|| error(self.source, node, "iteration requires a concrete container, homogeneous tuple, string, bytes, or range"))?;
        if reverse && (mapping.is_some() || set_element.is_some() || items) {
            return Err(error(
                self.source,
                node,
                "reversed requires a list, tuple, string, bytes, or range",
            ));
        }
        if matches!(element, Type::Callable(_))
            && let ExprKind::Tuple(values) = iterable.kind
        {
            return Ok(self.prepare_callable_tuple(values, element, reverse));
        }
        let ty = iterable.ty;
        let source = self.hidden(ty);
        let value = self.hidden(element);
        let ready = self.hidden(Type::Bool);
        let done = self.hidden(Type::Bool);
        let evaluate = vec![Statement::Assign(source, iterable)];
        let mut acquire = vec![Statement::Assign(done, Expr::boolean(false))];
        let mut clear = vec![source, value, ready, done];
        let mut pull = Vec::new();
        let indexed = self.types.element(ty).is_some()
            || self.types.var_tuple_element(ty).is_some()
            || fields.is_some()
            || ty == Type::Bytes;
        if indexed {
            let cursor = self.hidden(Type::Int);
            clear.push(cursor);
            let length = || Expr {
                ty: Type::Int,
                kind: ExprKind::Builtin(Builtin::Len, vec![Expr::local(source, ty)]),
            };
            acquire.push(Statement::Assign(
                cursor,
                if reverse {
                    binary(Binary::Subtract, length(), Expr::integer(1))
                } else {
                    Expr::integer(0)
                },
            ));
            let in_bounds = compare(Compare::Less, self.local(cursor), length());
            let available = if reverse {
                Expr {
                    ty: Type::Bool,
                    kind: ExprKind::Boolean(
                        true,
                        vec![
                            compare(Compare::GreaterEqual, self.local(cursor), Expr::integer(0)),
                            in_bounds,
                        ],
                    ),
                }
            } else {
                in_bounds
            };
            let item = if let Some(fields) = fields {
                let mut item = Expr {
                    ty: element,
                    kind: ExprKind::None,
                };
                // Read each field with its stored type before converting it to the
                // cursor's common layout, including the tag for a nullable element.
                for (index, &ty) in fields.iter().enumerate().rev() {
                    let field = self.coerce(
                        Expr {
                            ty,
                            kind: ExprKind::Field(Box::new(self.local(source)), index),
                        },
                        element,
                        node,
                    )?;
                    item = if index + 1 == fields.len() {
                        field
                    } else {
                        Expr {
                            ty: element,
                            kind: ExprKind::Conditional(
                                Box::new(compare(
                                    Compare::Equal,
                                    self.local(cursor),
                                    Expr::integer(index as i64),
                                )),
                                Box::new(field),
                                Box::new(item),
                            ),
                        }
                    };
                }
                item
            } else {
                Expr {
                    ty: element,
                    kind: ExprKind::Builtin(
                        if matches!(ty, Type::VarTuple(_)) {
                            Builtin::TupleIndex
                        } else if ty == Type::Bytes {
                            Builtin::BytesGet
                        } else if !reverse && self.consumes_list(ty, node.range()) {
                            Builtin::SequenceTakeInBounds
                        } else if matches!(ty, Type::List(_)) {
                            Builtin::SequenceGetInBounds
                        } else {
                            Builtin::ListGet(None)
                        },
                        vec![self.local(source), self.local(cursor)],
                    ),
                }
            };
            pull.push(Statement::If(
                available,
                vec![
                    Statement::Assign(value, item),
                    Statement::Assign(
                        cursor,
                        binary(
                            if reverse {
                                Binary::Subtract
                            } else {
                                Binary::Add
                            },
                            self.local(cursor),
                            Expr::integer(1),
                        ),
                    ),
                    Statement::Assign(ready, Expr::boolean(true)),
                ],
                vec![
                    Statement::Assign(done, Expr::boolean(true)),
                    Statement::Clear(source),
                ],
            ));
        } else {
            let iterator = self.hidden(Type::Iterator);
            clear.push(iterator);
            let builtin = if ty == Type::Str {
                if reverse {
                    Builtin::StringReverseIter
                } else {
                    Builtin::StringIter
                }
            } else if self.types.deque_element(ty).is_some() {
                Builtin::DequeIter { reverse }
            } else if set_element.is_some() {
                Builtin::SetIter
            } else {
                Builtin::DictIter(items)
            };
            acquire.push(Statement::Assign(
                iterator,
                Expr {
                    ty: Type::Iterator,
                    kind: ExprKind::Builtin(builtin, vec![self.local(source)]),
                },
            ));
            acquire.push(Statement::Clear(source));
            let key = if items {
                self.hidden(mapping.unwrap().0)
            } else {
                value
            };
            let second = items.then(|| self.hidden(mapping.unwrap().1));
            if items {
                clear.push(key);
            }
            clear.extend(second);
            pull.push(Statement::IterNext {
                kind: if ty == Type::Str {
                    ir::IterNextKind::String
                } else {
                    ir::IterNextKind::Generic
                },
                iterator,
                key,
                value: second,
                ready,
            });
            if let Some(second) = second {
                pull.push(Statement::If(
                    self.local(ready),
                    vec![
                        Statement::Assign(
                            value,
                            Expr {
                                ty: element,
                                kind: ExprKind::Tuple(vec![self.local(key), self.local(second)]),
                            },
                        ),
                        Statement::Clear(key),
                        Statement::Clear(second),
                    ],
                    Vec::new(),
                ));
            }
            pull.push(Statement::If(
                not(self.local(ready)),
                vec![
                    Statement::Assign(done, Expr::boolean(true)),
                    Statement::Clear(iterator),
                ],
                Vec::new(),
            ));
        }
        let next = vec![
            Statement::Clear(value),
            Statement::Assign(ready, Expr::boolean(false)),
            Statement::If(not(self.local(done)), pull, Vec::new()),
        ];
        Ok(Prepared {
            evaluate,
            acquire,
            next,
            value,
            ready,
            elementless: false,
            eager_list: None,
            writes: HashSet::new(),
            clear,
        })
    }

    /// The iterator cannot expose its tuple literal. Keep each callable (including its
    /// bound receiver) in its own local until exhaustion, with ordinary loop exits.
    fn prepare_callable_tuple(
        &mut self,
        items: Vec<Expr>,
        element: Type,
        reverse: bool,
    ) -> Prepared {
        let mut slots = Vec::new();
        let evaluate = items
            .into_iter()
            .map(|item| {
                let slot = self.hidden(element);
                slots.push(slot);
                Statement::Assign(slot, item)
            })
            .collect();
        let cursor = self.hidden(Type::Int);
        let value = self.hidden(element);
        let ready = self.hidden(Type::Bool);
        let release = slots.iter().rev().copied().map(Statement::Clear).collect();
        let mut clear = slots.clone();
        clear.extend([cursor, value, ready]);
        if reverse {
            slots.reverse();
        }
        let (last, prefix) = slots
            .split_last()
            .expect("a homogeneous callable tuple is nonempty");
        let item = prefix
            .iter()
            .enumerate()
            .rev()
            .fold(self.local(*last), |next, (i, &slot)| Expr {
                ty: element,
                kind: ExprKind::Conditional(
                    Box::new(compare(
                        Compare::Equal,
                        self.local(cursor),
                        Expr::integer(i as i64),
                    )),
                    Box::new(self.local(slot)),
                    Box::new(next),
                ),
            });
        Prepared {
            evaluate,
            acquire: vec![Statement::Assign(cursor, Expr::integer(0))],
            next: vec![
                Statement::Clear(value),
                Statement::Assign(
                    ready,
                    compare(
                        Compare::Less,
                        self.local(cursor),
                        Expr::integer(slots.len() as i64),
                    ),
                ),
                Statement::If(
                    self.local(ready),
                    vec![
                        Statement::Assign(value, item),
                        Statement::Assign(
                            cursor,
                            binary(Binary::Add, self.local(cursor), Expr::integer(1)),
                        ),
                    ],
                    release,
                ),
            ],
            value,
            ready,
            elementless: false,
            eager_list: None,
            writes: HashSet::new(),
            clear,
        }
    }

    /// Obtains one element from each source in order. Strict mismatch checking
    /// retains successful earlier pulls and performs only CPython's extra probes.
    ///
    /// ```python
    /// any(zip([1], [2, 3], strict=True))  # True: no second row is requested.
    /// ```
    fn zip_input(
        &mut self,
        call: &ast::ExprCall,
        element: Option<Type>,
    ) -> Result<Prepared, String> {
        if call
            .arguments
            .args
            .iter()
            .any(|arg| matches!(arg, ast::Expr::Starred(_)))
            || call.arguments.keywords.len() > 1
            || call
                .arguments
                .keywords
                .iter()
                .any(|keyword| keyword.arg.as_ref().map(|arg| arg.as_str()) != Some("strict"))
        {
            return Err(error(
                self.source,
                call,
                "zip accepts positional iterables and optional strict=",
            ));
        }
        let mut inputs = Vec::new();
        let fields = element.and_then(|ty| self.types.fields(self.types.payload(ty).unwrap_or(ty)));
        for (index, expression) in call.arguments.args.iter().enumerate() {
            let hint = fields
                .as_ref()
                .and_then(|fields| fields.get(index))
                .copied();
            let input = self.expansion_input(expression, hint)?;
            let input = self.prepare_iteration(input, expression, false)?;
            self.hint_empty_iteration(&input, hint);
            inputs.push(input);
        }
        let strict_expression = match call.arguments.keywords.first() {
            Some(keyword) => self.condition(&keyword.value)?,
            None => Expr::boolean(false),
        };
        let strict = self.hidden(Type::Bool);
        let ready = self.hidden(Type::Bool);
        let done = self.hidden(Type::Bool);
        let ty = self.types.tuple(
            inputs
                .iter()
                .map(|input| self.locals[input.value])
                .collect(),
        );
        let value = self.hidden(ty);
        let mut evaluate = Vec::new();
        let mut clear = vec![strict, ready, done, value];
        let writes = inputs
            .iter()
            .flat_map(|input| input.writes.iter().copied())
            .collect();
        for input in &mut inputs {
            evaluate.append(&mut input.evaluate);
            clear.extend(&input.clear);
        }
        evaluate.push(Statement::Assign(strict, strict_expression));
        for input in &mut inputs {
            evaluate.append(&mut input.acquire);
        }
        let cache = self.iterator_tuple_cache(ty, &mut evaluate, &mut clear);
        evaluate.push(Statement::Assign(done, Expr::boolean(inputs.is_empty())));
        let mut success = vec![Statement::Assign(ready, Expr::boolean(true))];
        if cache.is_none() {
            success.insert(
                0,
                Statement::Assign(
                    value,
                    Expr {
                        ty,
                        kind: ExprKind::Tuple(
                            inputs.iter().map(|input| self.local(input.value)).collect(),
                        ),
                    },
                ),
            );
        }
        success.extend(inputs.iter().map(|input| Statement::Clear(input.value)));
        for index in (0..inputs.len()).rev() {
            let mut exhausted = vec![
                Statement::Assign(done, Expr::boolean(true)),
                Statement::Clear(value),
            ];
            let strict_checks = if index == 0 {
                let mut probes = Vec::new();
                for (other_index, input) in inputs.iter().enumerate().skip(1) {
                    probes.extend(input.next.clone());
                    let previous = if other_index == 1 {
                        "argument 1".into()
                    } else {
                        format!("arguments 1-{other_index}")
                    };
                    probes.push(Statement::If(
                        self.local(input.ready),
                        vec![raise_value_error(format!(
                            "zip() argument {} is longer than {previous}",
                            other_index + 1
                        ))],
                        Vec::new(),
                    ));
                }
                probes
            } else {
                let previous = if index == 1 {
                    "argument 1".into()
                } else {
                    format!("arguments 1-{index}")
                };
                vec![raise_value_error(format!(
                    "zip() argument {} is shorter than {previous}",
                    index + 1
                ))]
            };
            exhausted.push(Statement::If(self.local(strict), strict_checks, Vec::new()));
            exhausted.extend(
                inputs
                    .iter()
                    .flat_map(|input| input.clear.iter().copied())
                    .map(Statement::Clear),
            );
            let mut step = inputs[index].next.clone();
            if cache.is_some() {
                success.insert(0, Statement::Clear(inputs[index].value));
                success.insert(
                    0,
                    Statement::SetField(self.local(value), index, self.local(inputs[index].value)),
                );
            }
            step.push(Statement::If(
                self.local(inputs[index].ready),
                success,
                exhausted,
            ));
            success = step;
        }
        if let Some(cache) = cache {
            success.insert(
                0,
                Statement::Assign(
                    value,
                    Expr {
                        ty,
                        kind: ExprKind::Builtin(Builtin::TupleCacheResult, vec![self.local(cache)]),
                    },
                ),
            );
        }
        let next = vec![
            Statement::Clear(value),
            Statement::Assign(ready, Expr::boolean(false)),
            Statement::If(not(self.local(done)), success, Vec::new()),
        ];
        Ok(Prepared {
            evaluate,
            acquire: Vec::new(),
            next,
            value,
            ready,
            elementless: false,
            eager_list: None,
            writes,
            clear,
        })
    }

    /// Resumes private comprehension targets while all captured enclosing names
    /// continue to refer to their live slots. Only the leftmost iterator is
    /// acquired at creation; every inner source and predicate remains lazy.
    ///
    /// ```python
    /// factor = 1
    /// sum((factor * x for x in [1, 2]), (factor := 2))  # 8
    /// ```
    pub(super) fn generator_input(
        &mut self,
        generator: &ast::ExprGenerator,
        element: Option<Type>,
        zip_sum: Option<&mut Option<zip_sums::Sum>>,
    ) -> Result<Prepared, String> {
        let plan = Plan::generator(generator);
        // A synchronous consumer cannot resume an asynchronous generator.
        if plan.body_suspends() {
            return Err(error(
                self.source,
                generator,
                "asynchronous generator expressions are unsupported",
            ));
        }
        let lists = zip_sum
            .as_ref()
            .and_then(|_| zip_sums::lists(self, generator));
        let first = self.expansion_input(plan.outer, None)?;
        let mut first = self.prepare_iteration(first, generator, false)?;
        let first_private = self.locals.len();
        let scope = self.enter_comprehension(plan.target_names());
        // Suspended generators can observe later enclosing writes. Facts about
        // those mutable slots must be proved again inside the lazy body.
        self.narrowed = assignments::Narrowed::new();
        // Each live iterator has one resume case, in clause order.
        struct Pull {
            input: Prepared,
            binding: Vec<Statement>,
            filters: Vec<Expr>,
        }
        first.evaluate.append(&mut first.acquire);
        let mut input = first;
        let mut pulls: Vec<Pull> = Vec::with_capacity(plan.clauses.len());
        let yielded = loop {
            let depth = pulls.len();
            let clause = &plan.clauses[depth];
            if depth != 0 {
                // Only private target slots remain stable while execution
                // pauses inside a nested iterator.
                let stable = self
                    .comprehension_targets
                    .iter()
                    .filter_map(|name| self.names.get(name).copied())
                    .collect::<HashSet<_>>();
                for (slot, _) in &self.narrowed.clone() {
                    if !stable.contains(slot) {
                        self.narrowed.remove_mut(slot);
                    }
                }
            }
            self.forget_comprehension_writes(&plan, depth);
            let row = self.local(input.value);
            let stable =
                matches!(row.ty, Type::Tuple(_)).then_some(unpacking::Row::Projected(input.value));
            let mut binding = Vec::new();
            unpacking::bind_input(self, &clause.target, row, stable, &mut binding)?;
            let filters = self.eager_filters(clause)?;
            pulls.push(Pull {
                input,
                binding,
                filters,
            });
            if let Some(inner) = plan.clauses.get(depth + 1) {
                let source = self.expansion_input(&inner.iter, None)?;
                input = self.prepare_iteration(source, &inner.iter, false)?;
            } else {
                let item = plan.output.value();
                break match element {
                    Some(ty) => self.expr_expected(item, ty)?,
                    None => self.expr(item)?,
                };
            }
        };
        let value = self.hidden(yielded.ty);
        if let (Some(out), Some((lists, targets))) = (zip_sum, lists)
            && yielded.ty == Type::Int
        {
            let items = targets.map(|name| self.names[name]);
            let zip = &pulls[0].input;
            let mut next = zip.next.clone();
            next.push(Prepared::exhaust_if_empty(self.local(zip.ready)));
            next.extend_from_slice(&pulls[0].binding);
            next.push(Statement::Clear(zip.value));
            *out = Some(zip_sums::Sum {
                lists,
                items,
                term: yielded.clone(),
                begin: zip.evaluate.clone(),
                next,
                clear: zip
                    .clear
                    .iter()
                    .copied()
                    .chain(first_private..value)
                    .collect(),
            });
        }
        let ready = self.hidden(Type::Bool);
        let done = self.hidden(Type::Bool);
        let resume_pc = self.hidden(Type::Int);
        let exported: HashSet<_> = self.leave_comprehension(scope).into_values().collect();
        let mut writes = pulls
            .iter()
            .flat_map(|pull| pull.input.writes.iter().copied())
            .collect::<HashSet<_>>();
        for slot in &exported {
            self.narrowed.remove_mut(slot);
            writes.insert(*slot);
        }
        let mut clear = pulls[0].input.clear.clone();
        clear.extend((first_private..self.locals.len()).filter(|slot| !exported.contains(slot)));
        let mut evaluate = std::mem::take(&mut pulls[0].input.evaluate);
        evaluate.extend([
            Statement::Assign(resume_pc, Expr::integer(0)),
            Statement::Assign(done, Expr::boolean(false)),
        ]);
        let mut cases = Vec::new();
        let mut accepted = vec![
            Statement::Assign(value, yielded),
            Statement::Assign(ready, Expr::boolean(true)),
        ];
        for (depth, pull) in pulls.into_iter().enumerate().rev() {
            let mut available = pull.binding;
            available.push(Statement::Clear(pull.input.value));
            available.extend(filter_suite(pull.filters, accepted));
            // An outer clause opens this iterator only after its own filters pass.
            accepted = pull.input.evaluate;
            let (roots, jump) = if depth == 0 {
                (&clear, Statement::Assign(done, Expr::boolean(true)))
            } else {
                accepted.extend(pull.input.acquire);
                accepted.push(Statement::Assign(resume_pc, Expr::integer(depth as i64)));
                (
                    &pull.input.clear,
                    Statement::Assign(resume_pc, Expr::integer((depth - 1) as i64)),
                )
            };
            let mut exhausted: Vec<_> = roots.iter().copied().map(Statement::Clear).collect();
            exhausted.push(jump);
            let mut step = pull.input.next;
            step.push(Statement::If(
                self.local(pull.input.ready),
                self.generator_body(available),
                exhausted,
            ));
            cases = vec![Statement::If(
                compare(
                    Compare::Equal,
                    self.local(resume_pc),
                    Expr::integer(depth as i64),
                ),
                step,
                cases,
            )];
        }
        // The leftmost iterator is already acquired. Restore the caller on yield or unwind.
        let resume = self.generator_frame(Statement::While(
            Expr {
                ty: Type::Bool,
                kind: ExprKind::Boolean(true, vec![not(self.local(ready)), not(self.local(done))]),
            },
            cases,
            Vec::new(),
        ));
        let next = vec![
            Statement::Clear(value),
            Statement::Assign(ready, Expr::boolean(false)),
            Statement::If(not(self.local(done)), vec![resume], Vec::new()),
        ];
        Ok(Prepared {
            evaluate,
            acquire: Vec::new(),
            next,
            value,
            ready,
            elementless: false,
            eager_list: None,
            writes,
            clear,
        })
    }

    /// Run one prepared generator step or an equivalent callback-free bulk operation.
    pub(super) fn generator_frame(&self, step: Statement) -> Statement {
        let module = if self.module == self.graph.entry {
            "__main__"
        } else {
            &self.graph.modules[self.module].name
        };
        let marker = |builtin| {
            Statement::Expr(Expr {
                ty: Type::None,
                kind: ExprKind::Builtin(builtin, Vec::new()),
            })
        };
        Statement::Try {
            body: vec![
                marker(Builtin::GeneratorFrameEnter(module.to_owned())),
                step,
            ],
            handlers: Vec::new(),
            orelse: Vec::new(),
            finalbody: vec![marker(Builtin::GeneratorFrameLeave)],
        }
    }

    /// Convert StopIteration escaping suspended generator code, retaining its cause.
    pub(super) fn generator_body(&self, body: Vec<Statement>) -> Vec<Statement> {
        let error = Expr {
            ty: Type::Exception,
            kind: ExprKind::Exception {
                kind: exception_kind("RuntimeError").unwrap(),
                message: Some(Box::new(unpacking::string(
                    "generator raised StopIteration",
                ))),
            },
        };
        let cause = Expr {
            ty: self.types.optional(Type::Exception),
            kind: ExprKind::Builtin(Builtin::ActiveException, Vec::new()),
        };
        let converted = Statement::raise_at(error, Some(cause), None);
        catch_stop_iteration(self.types, body, vec![converted])
    }

    /// Captures the callable before its source, then invokes it only after each
    /// pull has independently rooted an item and released runtime storage access.
    fn callback_input(
        &mut self,
        call: &ast::ExprCall,
        element: Option<Type>,
        filter: bool,
    ) -> Result<Prepared, String> {
        let [callback, source] = call.arguments.args.as_ref() else {
            return Err(error(
                self.source,
                call,
                "map/filter require a callback and one concrete iterable",
            ));
        };
        if !call.arguments.keywords.is_empty() {
            return Err(error(
                self.source,
                call,
                "map/filter do not accept keyword arguments",
            ));
        }
        let definition = match self.resolve(callback) {
            Some(Binding::Definition(definition)) => self.functions.get(&definition).cloned(),
            _ => None,
        };
        // Fixed function names and noncapturing lambdas have no enclosing-slot
        // effects. Their input type can therefore be learned from the source;
        // emitted creation still precedes that source's evaluation.
        let checked = if definition.is_some() || matches!(callback, ast::Expr::Lambda(_)) {
            None
        } else if let Some(binding) = self.local_function_binding(callback) {
            Some(self.local_callback(binding, callback)?)
        } else {
            Some(self.expr(callback)?)
        };
        let callback_input = checked
            .as_ref()
            .filter(|value| self.types.index_callable_signature(value.ty).is_none())
            .and_then(|value| self.types.signature(value.ty))
            .and_then(|(params, _)| (params.len() == 1).then(|| params[0]))
            .or_else(|| {
                definition.as_ref().and_then(|signature| {
                    if signature.params.len() != 1 {
                        return None;
                    }
                    self.inference
                        .map_or(Some(signature.params[0].ty), |inference| {
                            inference.parameter(signature.id, 0)
                        })
                })
            });
        // The destination describes surviving values. The truth predicate may
        // still receive None from a generator, list display, or nested pipeline.
        // The internal len argument frame belongs to the call, not the source iterable.
        let input_hint = callback_input
            .filter(|ty| !self.types.is_len_argument(*ty))
            .or_else(|| {
                if filter
                    && checked
                        .as_ref()
                        .is_some_and(|callback| callback.ty == Type::None)
                {
                    element.map(|ty| self.types.optional(ty))
                } else {
                    filter.then_some(element).flatten()
                }
            });
        let source = self.expansion_input(source, input_hint)?;
        let mut input = self.prepare_iteration(source, call, false)?;
        let input_type = self.locals[input.value];
        let callback = self.item_callback(
            callback,
            input_type,
            if filter { None } else { element },
            checked,
        )?;
        let truth_filter = filter && callback.ty == Type::None;
        let operation = if truth_filter {
            input.evaluate.insert(0, Statement::Expr(callback));
            self.local(input.value)
        } else {
            self.apply_item_callback(&mut input, callback, 0, "map/filter", call)?
                .0
        };
        input.evaluate.append(&mut input.acquire);
        if filter {
            let ready = self.hidden(Type::Bool);
            let mut step = std::mem::take(&mut input.next);
            let predicate = catch_stop_iteration(
                self.types,
                vec![Statement::If(
                    self.test_truth(operation, call)?,
                    vec![Statement::Assign(ready, Expr::boolean(true))],
                    vec![Statement::Clear(input.value)],
                )],
                vec![Statement::Clear(input.value), Statement::Break],
            );
            step.push(Statement::If(
                self.local(input.ready),
                predicate,
                vec![Statement::Break],
            ));
            input.next = vec![
                Statement::Clear(input.value),
                Statement::Assign(ready, Expr::boolean(false)),
                Statement::While(not(self.local(ready)), step, Vec::new()),
            ];
            input.ready = ready;
            input.clear.push(ready);
            if truth_filter && let Some(payload) = self.types.payload(input_type) {
                let value = self.hidden(payload);
                input.next.push(Statement::If(
                    self.local(ready),
                    vec![
                        Statement::Assign(
                            value,
                            Expr {
                                ty: payload,
                                kind: ExprKind::Unwrap(Box::new(self.local(input.value))),
                            },
                        ),
                        Statement::Clear(input.value),
                    ],
                    Vec::new(),
                ));
                input.next.insert(0, Statement::Clear(value));
                input.value = value;
                input.clear.push(value);
            }
        } else {
            let value = self.hidden(operation.ty);
            // The source can use its ready slot to latch exhaustion. A failed
            // callback ends this map pull without changing the source cursor.
            let ready = self.hidden(Type::Bool);
            input
                .next
                .push(Statement::Assign(ready, self.local(input.ready)));
            let mut mapped = catch_stop_iteration(
                self.types,
                vec![Statement::Assign(value, operation)],
                vec![Statement::Assign(ready, Expr::boolean(false))],
            );
            mapped.push(Statement::Clear(input.value));
            input
                .next
                .push(Statement::If(self.local(ready), mapped, Vec::new()));
            input.next.insert(0, Statement::Clear(value));
            input.value = value;
            input.ready = ready;
            input.elementless = false;
            input.clear.extend([value, ready]);
        }
        Ok(input)
    }

    /// Infer the parameter of a fixed unary function or noncapturing lambda from the
    /// pulled item. A caller may already have checked a dynamic callback to preserve
    /// its position among the constructor arguments.
    fn item_callback(
        &mut self,
        expression: &ast::Expr,
        item: Type,
        returns: Option<Type>,
        checked: Option<Expr>,
    ) -> Result<Expr, String> {
        if let Some(value) = checked {
            return Ok(value);
        }
        if let ast::Expr::Lambda(lambda) = expression {
            return self.lambda_value(lambda, Some(vec![item]), returns);
        }
        if let Some(Binding::Definition(definition)) = self.resolve(expression)
            && let Some(signature) = self.functions.get(&definition)
        {
            if signature.params.len() == 1
                && let Some(inference) = self.inference
            {
                inference.learn_parameter(signature.id, 0, item);
            }
            return self.callable_value(signature, None, None, expression);
        }
        if let Some(binding) = self.local_function_binding(expression) {
            return self.local_callback(binding, expression);
        }
        self.expr(expression)
    }

    /// Invokes fixed functions directly and captures a dynamic or bound callable
    /// once, at its position in the constructor's argument evaluation.
    fn apply_item_callback(
        &mut self,
        input: &mut Prepared,
        callback: Expr,
        evaluation_index: usize,
        consumer: &str,
        call: &ast::ExprCall,
    ) -> Result<(Expr, Option<usize>), String> {
        if let Type::StrDescriptor(method) = callback.ty {
            // Evaluate the descriptor at construction and apply it only when an item is pulled.
            let item = self.local(input.value);
            if matches!(item.ty, Type::Class(_) | Type::Enum(_))
                || (!matches!(item.ty, Type::Str | Type::StrLiteral(_))
                    && self.exact_argument_type_name(item.ty).is_none())
            {
                return Err(self.object_builtin_error("str descriptor callback", call));
            }
            let slot = self.hidden(callback.ty);
            input
                .evaluate
                .insert(evaluation_index, Statement::Assign(slot, callback));
            input.clear.push(slot);
            let mut bound = call.clone();
            bound.arguments.keywords.clear();
            let result =
                self.apply_str_descriptor(&bound, self.local(slot), method, item, call, &[])?;
            return Ok((result, Some(slot)));
        }
        let message = || {
            error(
                self.source,
                call,
                &format!("{consumer} requires a callable with one positional parameter"),
            )
        };
        let (params, returns) = self.types.signature(callback.ty).ok_or_else(message)?;
        let [parameter] = params.as_slice() else {
            return Err(message());
        };
        let item = self.local(input.value);
        let argument = if self.types.is_len_argument(*parameter) {
            if matches!(item.ty, Type::None | Type::Bool | Type::Int) {
                return Err(error(
                    self.source,
                    call,
                    "len callback requires items with a length",
                ));
            }
            self.pack_len_value(item, call)?
        } else if self.types.index_callable_signature(callback.ty).is_some() {
            self.index_value(item, call)?
        } else {
            self.coerce(item, *parameter, call)?
        };
        let args = ir::CallArgs::positional(vec![argument]);
        if matches!(returns, Type::Coroutine(_)) {
            let slot = self.hidden(callback.ty);
            input
                .evaluate
                .insert(evaluation_index, Statement::Assign(slot, callback));
            input.clear.push(slot);
            return Ok((
                self.indirect_coroutine(self.local(slot), args.values, returns),
                Some(slot),
            ));
        }
        let direct = match &callback.kind {
            ExprKind::Callable {
                function,
                receiver: None,
            }
            | ExprKind::Builtin(Builtin::StaticFunction(function), _) => Some(*function),
            _ => None,
        };
        let (kind, captured) = if let Some(function) = direct {
            self.calls.insert(function);
            (ExprKind::Call(function, args), None)
        } else {
            let slot = self.hidden(callback.ty);
            input
                .evaluate
                .insert(evaluation_index, Statement::Assign(slot, callback));
            input.clear.push(slot);
            (
                ExprKind::Invoke {
                    callable: Box::new(self.local(slot)),
                    args,
                },
                Some(slot),
            )
        };
        Ok((Expr { ty: returns, kind }, captured))
    }

    /// A source loop owns its complete pipeline across break, return, and error.
    /// Normal exhaustion drops the iterator before the user-visible else suite.
    pub(super) fn lower_prepared_iteration(
        &mut self,
        target: &ast::Expr,
        mut input: Prepared,
        orelse: &[ast::Stmt],
        before_iteration: Vec<Statement>,
        lower_body: impl FnOnce(&mut Self, &mut Vec<Statement>) -> Result<(), String>,
    ) -> Result<(Vec<Statement>, bool), String> {
        // A later consumer argument or zip constructor argument may have
        // established new facts since this lazy body was checked.
        for slot in &input.writes {
            self.narrowed.remove_mut(slot);
            self.assigned
                .set_type_source(*slot, self.declared_slots.get(slot).copied());
        }
        let before = self.assigned.clone();
        let mut facts = self.narrowed.clone();
        let binding = input.bind_loop_target(self, target, true)?;
        assignments::forget_binding_writes(&mut facts, binding);
        self.loop_breaks.push(None);
        lower_body(self, &mut input.next)?;
        let breaks = self.loop_breaks.pop().unwrap();
        self.assigned.after_iteration(before);
        self.narrowed = facts;
        let (orelse, falls) = self.loop_else(orelse, breaks, true)?;
        Ok((input.loop_suite(orelse, before_iteration), falls))
    }

    /// A statically empty tuple can adopt its consumer's element layout because
    /// it has no item-producing branch that could violate that representation.
    pub(super) fn hint_empty_iteration(&mut self, input: &Prepared, element: Option<Type>) {
        if input.elementless
            && let Some(element) = element
        {
            self.locals[input.value] = element;
        }
    }

    /// Reduce exact integers in one pass without retaining intermediate items.
    pub(super) fn extremum_call(
        &mut self,
        call: &ast::ExprCall,
        minimum: bool,
    ) -> Result<Expr, String> {
        let name = if minimum { "min" } else { "max" };
        self.reject_builtin_keywords(call)?;
        let order = if minimum {
            Compare::Less
        } else {
            Compare::Greater
        };
        if call.arguments.args.len() >= 2 {
            let args = call
                .arguments
                .args
                .iter()
                .map(|arg| self.expr_expected(arg, Type::Int))
                .collect::<std::result::Result<_, _>>()?;
            return Ok(Expr {
                ty: Type::Int,
                kind: ExprKind::Builtin(Builtin::IntExtremum { minimum }, args),
            });
        }
        let input = match call.arguments.args.as_ref() {
            [source @ ast::Expr::Generator(_)] => Some(self.expansion_input(source, Some(Type::Int))?),
            [source] => self.pipeline_input(source, None, None)?.filter(|input| {
                matches!(input, Iteration::Prepared(cursor) if self.locals[cursor.value] == Type::Int)
            }),
            _ => None,
        };
        let Some(input) = input else {
            return Err(error(
                self.source,
                call,
                &format!("{name} requires at least two integers or one integer generator"),
            ));
        };
        let best = self.hidden(Type::Int);
        let empty = self.hidden(Type::Bool);
        let mut body = vec![Statement::Assign(empty, Expr::boolean(true))];
        body.extend(self.expand_items(input, call, |checker, item| {
            checker.expect(Type::Int, item.ty, call)?;
            let better = Expr {
                ty: Type::Bool,
                kind: ExprKind::Boolean(
                    false,
                    vec![
                        checker.local(empty),
                        compare(order, item.clone(), checker.local(best)),
                    ],
                ),
            };
            Ok(vec![Statement::If(
                better,
                vec![
                    Statement::Assign(best, item),
                    Statement::Assign(empty, Expr::boolean(false)),
                ],
                Vec::new(),
            )])
        })?);
        body.push(Statement::If(
            self.local(empty),
            vec![raise_value_error(format!(
                "{name}() iterable argument is empty"
            ))],
            Vec::new(),
        ));
        Ok(Expr {
            ty: Type::Int,
            kind: ExprKind::Scope {
                body,
                value: Box::new(self.local(best)),
                clear: vec![empty, best],
            },
        })
    }

    /// Quantifiers consume truth values immediately and never pull after their
    /// deciding element. Fixed tuples preserve eager field evaluation first.
    pub(super) fn quantifier_call(
        &mut self,
        call: &ast::ExprCall,
        all: bool,
    ) -> Result<Expr, String> {
        if !call.arguments.keywords.is_empty() || call.arguments.args.len() != 1 {
            return Err(error(
                self.source,
                call,
                "any and all require one positional iterable",
            ));
        }
        let source = &call.arguments.args[0];
        let input = self.expansion_input(source, None)?;
        let result = self.hidden(Type::Bool);
        let mut body = vec![Statement::Assign(result, Expr::boolean(all))];
        let mut clear = vec![result];
        if let Iteration::Container {
            iterable,
            items: false,
            start: None,
        } = &input
            && let Some(fields) = self.types.fields(iterable.ty)
        {
            let captured = self.hidden(iterable.ty);
            clear.push(captured);
            body.push(Statement::Assign(captured, iterable.clone()));
            for (index, ty) in fields.into_iter().enumerate() {
                let value = self.test_truth(
                    Expr {
                        ty,
                        kind: ExprKind::Field(Box::new(self.local(captured)), index),
                    },
                    call,
                )?;
                body.push(Statement::If(
                    if all {
                        self.local(result)
                    } else {
                        not(self.local(result))
                    },
                    vec![Statement::Assign(result, value)],
                    Vec::new(),
                ));
            }
        } else {
            body.extend(self.expand_items(input, call, |checker, value| {
                let value = checker.test_truth(value, call)?;
                let deciding = if all { not(value) } else { value };
                Ok(vec![Statement::If(
                    deciding,
                    vec![
                        Statement::Assign(result, Expr::boolean(!all)),
                        Statement::Break,
                    ],
                    Vec::new(),
                )])
            })?);
        }
        Ok(Expr {
            ty: Type::Bool,
            kind: ExprKind::Scope {
                body,
                value: Box::new(self.local(result)),
                clear,
            },
        })
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn python_pulls_and_unpacking_preserve_the_following_contracts() {
        use crate::frontend::{Kind, lower_with_caller_cache};
        use crate::modules::SourceGraph;

        for (body, location) in [
            (
                "    for x in manager:\n        values: list[int] = [3]\n        values.append(True)",
                "4:23",
            ),
            (
                "    x: int = 2\n    for x in manager:\n        pass\n    values: list[int] = [3]\n    values.append(True)",
                "6:19",
            ),
            (
                "    x: int = 2\n    for _, x in zip([1], manager):\n        pass\n    values: list[int] = [3]\n    values.append(True)",
                "6:19",
            ),
            (
                "    for x in map(int, manager):\n        values: list[int] = [3]\n        values.append(True)",
                "4:23",
            ),
            (
                "    a, (b, *tail) = manager\n    values: list[int] = [3]\n    values.append(True)",
                "4:19",
            ),
        ] {
            let source = format!(
                "def check(manager):\n{body}\ndef get_context():\n    return set()\ncheck(get_context())\n"
            );
            let mut graph = SourceGraph::single(&source).unwrap();
            let Err(error) = lower_with_caller_cache(&mut graph, None, None) else {
                panic!("expected a checked list operand");
            };
            assert_eq!(error.kind, Kind::ExactType);
            assert!(error.to_string().contains(location), "{error}");
        }
    }
}
