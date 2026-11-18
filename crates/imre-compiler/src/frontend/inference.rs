//! Resolve function signatures to native layouts or registered Python argument storage.

use std::cell::{Cell, RefCell};
use std::ops::ControlFlow;

use ruff_python_ast::visitor::{Visitor, walk_expr, walk_stmt};

use super::*;

/// An element inserted into a local container and its direct iteration source, when known.
struct Insertion<'a> {
    value: &'a ast::Expr,
    for_loop: Option<&'a ast::StmtFor>,
    class: Option<usize>,
}

fn insertion_operand<'a>(stmt: &'a ast::Stmt, name: &str, method: &str) -> Option<&'a ast::Expr> {
    let call = stmt.as_expr_stmt()?.value.as_call_expr()?;
    let attr = call.func.as_attribute_expr()?;
    let [value] = call.arguments.args.as_ref() else {
        return None;
    };
    (attr.attr.as_str() == method
        && attr.value.as_name_expr()?.id.as_str() == name
        && call.arguments.keywords.is_empty())
    .then_some(value)
}

/// Direct class appends may share a declared base across ordinary if/for
/// statements; any other statement or list reference ends widening.
fn first_insertion<'ast>(
    checker: &Checker<'_>,
    name: &str,
    method: &str,
    statements: &'ast [ast::Stmt],
) -> Option<Insertion<'ast>> {
    struct Appends<'checker, 'graph, 'name, 'ast> {
        checker: &'checker Checker<'graph>,
        name: &'name str,
        method: &'name str,
        insertion: Option<Insertion<'ast>>,
        original_class: Option<usize>,
        for_loop: Option<&'ast ast::StmtFor>,
        widen: bool,
    }

    impl<'ast> Visitor<'ast> for Appends<'_, '_, '_, 'ast> {
        fn visit_stmt(&mut self, stmt: &'ast ast::Stmt) {
            if self.insertion.is_some() && !self.widen {
                return;
            }
            if matches!(stmt, ast::Stmt::FunctionDef(_) | ast::Stmt::ClassDef(_)) {
                self.widen = false;
                return;
            }
            if let ast::Stmt::For(loop_) = stmt {
                self.widen &= !loop_.is_async;
                self.visit_expr(&loop_.iter);
                self.visit_expr(&loop_.target);
                let enclosing = self.for_loop.replace(loop_);
                self.visit_body(&loop_.body);
                self.for_loop = enclosing;
                self.visit_body(&loop_.orelse);
                return;
            }
            if let Some(value) = insertion_operand(stmt, self.name, self.method) {
                self.visit_expr(value);
                if self.insertion.is_some() && !self.widen {
                    return;
                }
                let class = value
                    .as_call_expr()
                    .and_then(|call| self.checker.class_reference(&call.func));
                if let Some(first) = &mut self.insertion {
                    let declarations = self.checker.classes.declarations;
                    let common = first.class.zip(class).and_then(|(current, next)| {
                        std::iter::once(current)
                            .chain(declarations.definitions[current].bases.iter().copied())
                            .find(|&base| declarations.subclass(next, base))
                    });
                    first.class = common.or(self.original_class);
                    self.widen = common.is_some();
                } else {
                    self.original_class = class;
                    self.insertion = Some(Insertion {
                        value,
                        for_loop: self.for_loop,
                        class,
                    });
                    self.widen &= class.is_some();
                }
            } else {
                if !matches!(stmt, ast::Stmt::If(_)) {
                    self.widen = false;
                }
                if self.insertion.is_none() || self.widen {
                    // With widening disabled, this is the old first-insertion scan.
                    walk_stmt(self, stmt);
                }
            }
        }

        fn visit_expr(&mut self, expr: &'ast ast::Expr) {
            if !self.widen {
                return;
            }
            if expr
                .as_name_expr()
                .is_some_and(|n| n.id.as_str() == self.name)
            {
                self.widen = false;
            } else {
                walk_expr(self, expr);
            }
        }
    }

    let mut appends = Appends {
        checker,
        name,
        method,
        insertion: None,
        original_class: None,
        for_loop: None,
        widen: method == "append",
    };
    appends.visit_body(statements);
    appends.insertion
}

impl Checker<'_> {
    /// A concrete insertion can determine the layout of a fresh list or set. A set
    /// filled from an exact text iterator can also use its inferred element type.
    pub(super) fn inserted_element(
        &self,
        name: &str,
        set: bool,
        later: &[ast::Stmt],
    ) -> Option<Type> {
        let insertion = first_insertion(self, name, if set { "add" } else { "append" }, later)?;
        if let Some(class) = insertion.class {
            return Some(Type::Class(class));
        }
        if let Some(ty) = self.graph.session.scalar_type(self.module, insertion.value) {
            return Some(ty);
        }
        if !set || self.returns == Type::PythonObject {
            return None;
        }
        let for_loop = insertion.for_loop?;
        let item = for_loop.target.as_name_expr()?;
        let source = for_loop.iter.as_name_expr()?;
        let element = match self.locals[*self.names.get(source.id.as_str())?] {
            Type::Str => Type::Str,
            Type::Bytes | Type::ByteArray => Type::Int,
            _ => return None,
        };
        let assigned = narrowing::assigned_names(later);
        if for_loop.is_async
            || insertion.value.as_name_expr()?.id != item.id
            || narrowing::assigned_names(&for_loop.body).contains(item.id.as_str())
            || assigned.contains(source.id.as_str())
        {
            return None;
        }
        (!assigned.contains(name)
            && !["len", "sorted", "list", "tuple"]
                .iter()
                .any(|builtin| assigned.contains(*builtin))
            && self.local_set_storage(name, element, for_loop, item.id.as_str(), later))
        .then_some(element)
    }

    /// Select iteration-derived native storage only when the fresh set cannot escape
    /// or receive an incompatible value. This runs before resolved IR exists, so it
    /// accepts only source operations whose exact receiver and operands prove that
    /// they cannot call Python code. Unknown operations require ordinary storage.
    ///
    /// ```python
    /// values = set()
    /// for char in text:
    ///     values.add(char)
    /// return sorted(values)
    /// ```
    fn local_set_storage(
        &self,
        name: &str,
        element: Type,
        insertion_loop: &ast::StmtFor,
        item: &str,
        later: &[ast::Stmt],
    ) -> bool {
        struct Uses<'checker, 'graph, 'ast> {
            checker: &'checker Checker<'graph>,
            name: &'checker str,
            element: Type,
            insertion_loop: &'ast ast::StmtFor,
            item: &'checker str,
            in_insertion_loop: bool,
            safe: bool,
        }

        impl Uses<'_, '_, '_> {
            fn owner(&self, value: &ast::Expr) -> bool {
                value
                    .as_name_expr()
                    .is_some_and(|name| name.id == self.name)
            }

            fn fits(&self, value: &ast::Expr) -> bool {
                self.exact_scalar(value) == Some(self.element)
            }

            /// A native scalar or intrinsic result cannot dispatch an overloaded
            /// Python operator. ty assignability alone does not establish that fact.
            fn exact_scalar(&self, value: &ast::Expr) -> Option<Type> {
                let scalar = |ty| {
                    matches!(
                        ty,
                        Type::Bool | Type::Int | Type::Float | Type::Str | Type::Bytes
                    )
                    .then_some(ty)
                };
                match value {
                    ast::Expr::Name(name) if self.in_insertion_loop && name.id == self.item => {
                        scalar(self.element)
                    }
                    ast::Expr::Name(name) => self
                        .checker
                        .names
                        .get(name.id.as_str())
                        .and_then(|&slot| scalar(self.checker.locals[slot])),
                    ast::Expr::NumberLiteral(_)
                    | ast::Expr::BooleanLiteral(_)
                    | ast::Expr::StringLiteral(_)
                    | ast::Expr::BytesLiteral(_) => self
                        .checker
                        .graph
                        .session
                        .scalar_type(self.checker.module, value)
                        .and_then(scalar),
                    ast::Expr::Call(call)
                        if self.checker.unshadowed_builtin(&call.func, "len")
                            && matches!(call.arguments.args.as_slice(), [arg] if self.owner(arg))
                            && call.arguments.keywords.is_empty() =>
                    {
                        Some(Type::Int)
                    }
                    ast::Expr::BinOp(binary)
                        if self.exact_scalar(&binary.left).is_some()
                            && self.exact_scalar(&binary.right).is_some() =>
                    {
                        self.checker
                            .graph
                            .session
                            .scalar_type(self.checker.module, value)
                            .and_then(scalar)
                    }
                    ast::Expr::UnaryOp(unary) if self.exact_scalar(&unary.operand).is_some() => {
                        self.checker
                            .graph
                            .session
                            .scalar_type(self.checker.module, value)
                            .and_then(scalar)
                    }
                    ast::Expr::Compare(compare)
                        if compare.ops.len() == 1
                            && matches!(compare.ops[0], ast::CmpOp::In | ast::CmpOp::NotIn)
                            && self.owner(&compare.operands[1])
                            && self.fits(&compare.operands[0]) =>
                    {
                        Some(Type::Bool)
                    }
                    ast::Expr::Compare(compare)
                        if compare
                            .operands
                            .iter()
                            .all(|operand| self.exact_scalar(operand).is_some()) =>
                    {
                        Some(Type::Bool)
                    }
                    ast::Expr::BoolOp(boolean)
                        if boolean
                            .values
                            .iter()
                            .all(|operand| self.exact_scalar(operand).is_some()) =>
                    {
                        self.checker
                            .graph
                            .session
                            .scalar_type(self.checker.module, value)
                            .and_then(scalar)
                    }
                    _ => None,
                }
            }
        }

        impl<'ast> Visitor<'ast> for Uses<'_, '_, 'ast> {
            fn visit_stmt(&mut self, stmt: &'ast ast::Stmt) {
                if !self.safe {
                    return;
                }
                match stmt {
                    ast::Stmt::For(loop_) if std::ptr::eq(loop_, self.insertion_loop) => {
                        self.visit_expr(&loop_.iter);
                        self.in_insertion_loop = true;
                        self.visit_body(&loop_.body);
                        self.in_insertion_loop = false;
                        self.visit_body(&loop_.orelse);
                    }
                    ast::Stmt::Expr(stmt) => self.visit_expr(&stmt.value),
                    ast::Stmt::Return(stmt) => {
                        if let Some(value) = &stmt.value {
                            self.visit_expr(value);
                        }
                    }
                    ast::Stmt::Assign(stmt) if stmt.targets.iter().all(ast::Expr::is_name_expr) => {
                        for target in &stmt.targets {
                            self.visit_expr(target);
                        }
                        self.visit_expr(&stmt.value);
                    }
                    ast::Stmt::If(branch) => {
                        for test in std::iter::once(&*branch.test).chain(
                            branch
                                .elif_else_clauses
                                .iter()
                                .filter_map(|clause| clause.test.as_ref()),
                        ) {
                            if self.exact_scalar(test).is_none() {
                                self.safe = false;
                                return;
                            }
                            self.visit_expr(test);
                        }
                        self.visit_body(&branch.body);
                        for clause in &branch.elif_else_clauses {
                            self.visit_body(&clause.body);
                        }
                    }
                    ast::Stmt::Pass(_) | ast::Stmt::Break(_) | ast::Stmt::Continue(_) => {}
                    _ => self.safe = false,
                }
            }

            fn visit_expr(&mut self, expr: &'ast ast::Expr) {
                if !self.safe {
                    return;
                }
                match expr {
                    ast::Expr::Call(call) => {
                        if let ast::Expr::Attribute(attr) = &*call.func
                            && self.owner(&attr.value)
                            && matches!(attr.attr.as_str(), "add" | "discard")
                            && let [value] = call.arguments.args.as_ref()
                            && call.arguments.keywords.is_empty()
                            && self.fits(value)
                        {
                            self.visit_expr(value);
                        } else if let [value] = call.arguments.args.as_ref()
                            && self.owner(value)
                            && call.arguments.keywords.is_empty()
                            && ["len", "sorted", "list", "tuple"]
                                .iter()
                                .any(|builtin| self.checker.unshadowed_builtin(&call.func, builtin))
                        {
                            // These builtins read the set without exposing it to Python code.
                        } else {
                            self.safe = false;
                        }
                    }
                    ast::Expr::Compare(compare)
                        if compare.ops.len() == 1
                            && matches!(compare.ops[0], ast::CmpOp::In | ast::CmpOp::NotIn)
                            && self.owner(&compare.operands[1])
                            && self.fits(&compare.operands[0]) =>
                    {
                        self.visit_expr(&compare.operands[0]);
                    }
                    ast::Expr::BinOp(binary) if self.exact_scalar(expr).is_some() => {
                        self.visit_expr(&binary.left);
                        self.visit_expr(&binary.right);
                    }
                    ast::Expr::Compare(compare) if self.exact_scalar(expr).is_some() => {
                        for operand in &compare.operands {
                            self.visit_expr(operand);
                        }
                    }
                    ast::Expr::UnaryOp(unary) if self.exact_scalar(expr).is_some() => {
                        self.visit_expr(&unary.operand);
                    }
                    ast::Expr::BoolOp(boolean) if self.exact_scalar(expr).is_some() => {
                        for operand in &boolean.values {
                            self.visit_expr(operand);
                        }
                    }
                    ast::Expr::Name(name) => self.safe &= name.id != self.name,
                    ast::Expr::NumberLiteral(_)
                    | ast::Expr::BooleanLiteral(_)
                    | ast::Expr::StringLiteral(_)
                    | ast::Expr::BytesLiteral(_)
                    | ast::Expr::NoneLiteral(_) => {}
                    ast::Expr::List(_) | ast::Expr::Tuple(_) => {
                        ruff_python_ast::visitor::walk_expr(self, expr);
                    }
                    _ => self.safe = false,
                }
            }
        }

        let mut uses = Uses {
            checker: self,
            name,
            element,
            insertion_loop,
            item,
            in_insertion_loop: false,
            safe: true,
        };
        uses.visit_body(later);
        uses.safe
    }

    /// A later direct call can supply the same Callable context as an inline lambda.
    /// Its body and every use of the resulting local are still checked normally.
    pub(super) fn local_lambda_context(&self, local: &str, later: &[ast::Stmt]) -> Option<Type> {
        struct Calls<'a, F> {
            hint: Option<Type>,
            context: &'a F,
        }
        impl<'ast, F: Fn(&ast::ExprCall) -> Option<Type>> Visitor<'ast> for Calls<'_, F> {
            fn visit_stmt(&mut self, stmt: &'ast ast::Stmt) {
                if self.hint.is_none()
                    && !matches!(stmt, ast::Stmt::FunctionDef(_) | ast::Stmt::ClassDef(_))
                {
                    walk_stmt(self, stmt);
                }
            }

            fn visit_expr(&mut self, expr: &'ast ast::Expr) {
                match expr {
                    ast::Expr::Call(call) if self.hint.is_none() => {
                        self.hint = (self.context)(call)
                    }
                    ast::Expr::Lambda(_)
                    | ast::Expr::Generator(_)
                    | ast::Expr::ListComp(_)
                    | ast::Expr::SetComp(_)
                    | ast::Expr::DictComp(_) => return,
                    _ => {}
                }
                if self.hint.is_none() {
                    ruff_python_ast::visitor::walk_expr(self, expr);
                }
            }
        }

        let assigned = narrowing::assigned_names(later);
        if assigned.contains(local) {
            return None;
        }
        let context = |call: &ast::ExprCall| {
            let mut root = call.func.as_ref();
            while let ast::Expr::Attribute(attr) = root {
                root = &attr.value;
            }
            let name = root.as_name_expr()?.id.as_str();
            if assigned.contains(name)
                || self.names.contains_key(name)
                || self.local_functions.contains_key(name)
            {
                return None;
            }
            let Binding::Definition(definition) = self.resolve(&call.func)? else {
                return None;
            };
            let params = &self.functions.get(&definition)?.params;
            params.iter().enumerate().find_map(|(index, param)| {
                let arg = param
                    .kind
                    .known_argument(&call.arguments, index, &param.name)?;
                let ty = self.types.payload(param.ty).unwrap_or(param.ty);
                (arg.as_name_expr()?.id == local && self.types.signature(ty).is_some())
                    .then_some(ty)
            })
        };
        let mut calls = Calls {
            hint: None,
            context: &context,
        };
        calls.visit_body(later);
        calls.hint
    }

    /// In a truthy branch, an immediately repeated native dict lookup is present
    /// until the first possible effect. An exact local list/set receiver and
    /// a literal string key cannot execute Python code before reading the argument.
    pub(super) fn immediate_present_subscript(
        &self,
        test: &ast::Expr,
        body: &[ast::Stmt],
    ) -> Option<ruff_text_size::TextRange> {
        fn literal_lookup(expression: &ast::Expr) -> Option<(&str, &str)> {
            let ast::Expr::Subscript(sub) = expression else {
                return None;
            };
            let ast::Expr::Name(name) = &*sub.value else {
                return None;
            };
            let ast::Expr::StringLiteral(key) = &*sub.slice else {
                return None;
            };
            Some((name.id.as_str(), key.value.to_str()))
        }
        let (name, key) = literal_lookup(test)?;
        let &dict = self.names.get(name)?;
        let (key_type, value_type) = self.types.mapping(self.locals[dict])?;
        if key_type != Type::Str || self.types.payload(value_type).is_none() {
            return None;
        }
        let ast::Stmt::Expr(stmt) = body.first()? else {
            return None;
        };
        let ast::Expr::Call(call) = &*stmt.value else {
            return None;
        };
        let ast::Expr::Attribute(method) = &*call.func else {
            return None;
        };
        let ast::Expr::Name(receiver) = &*method.value else {
            return None;
        };
        let &owner = self.names.get(receiver.id.as_str())?;
        let ty = self.locals[owner];
        let intrinsic = (method.attr.as_str() == "add" && self.types.set_element(ty).is_some())
            || (method.attr.as_str() == "append" && self.types.element(ty).is_some());
        let [argument] = call.arguments.args.as_ref() else {
            return None;
        };
        (intrinsic
            && call.arguments.keywords.is_empty()
            && literal_lookup(argument) == Some((name, key)))
        .then(|| argument.range())
    }

    /// Join direct assignments across an exhaustive conditional before its first
    /// suite fixes a local's storage. This supplies a layout for empty-list arms
    /// and nullable scalar arms while retaining normal definite-assignment checks.
    pub(super) fn branch_layouts(
        &mut self,
        body: &[ast::Stmt],
        clauses: &[ast::ElifElseClause],
    ) -> Result<(), String> {
        if !clauses.last().is_some_and(|c| c.test.is_none())
            || !body.iter().any(|stmt| matches!(stmt, ast::Stmt::Assign(assign)
                if matches!(assign.targets.as_slice(), [ast::Expr::Name(name)] if !self.names.contains_key(name.id.as_str()))))
        {
            return Ok(());
        }
        let mut arms: HashMap<&str, Vec<&ast::Expr>> = HashMap::new();
        for suite in std::iter::once(body).chain(clauses.iter().map(|c| c.body.as_slice())) {
            for stmt in suite {
                if let ast::Stmt::Assign(a) = stmt
                    && let [ast::Expr::Name(n)] = a.targets.as_slice()
                    && !self.names.contains_key(n.id.as_str())
                {
                    arms.entry(n.id.as_str()).or_default().push(&a.value);
                }
            }
        }
        for (name, values) in arms {
            if values.len() != clauses.len() + 1 {
                continue;
            }
            let site = values[0];
            if values.iter().all(|e| matches!(e, ast::Expr::List(_)))
                && values
                    .iter()
                    .any(|e| matches!(e, ast::Expr::List(l) if l.elts.is_empty()))
            {
                let mut elements = values.iter().flat_map(|e| {
                    let ast::Expr::List(list) = e else {
                        unreachable!()
                    };
                    list.elts
                        .iter()
                        .map(|element| self.graph.session.scalar_type(self.module, element))
                });
                if let Some(Some(first)) = elements.next()
                    && elements.all(|ty| ty == Some(first))
                {
                    self.declare(name, self.types.list(first), site)?;
                }
            } else if values
                .iter()
                .any(|e| matches!(e, ast::Expr::NoneLiteral(_)))
            {
                let mut concrete = values
                    .iter()
                    .filter(|e| !matches!(e, ast::Expr::NoneLiteral(_)))
                    .map(|e| self.graph.session.scalar_type(self.module, e));
                if let Some(Some(first)) = concrete.next()
                    && concrete.all(|ty| ty == Some(first))
                {
                    self.declare(name, self.types.optional(first), site)?;
                }
            }
        }
        Ok(())
    }
}

enum SignatureUse {
    Return,
    Callable {
        skip: usize,
        expected: Option<(Vec<Type>, Type)>,
    },
}

/// Return information required the next time discovery visits this signature.
#[derive(Clone, Copy)]
enum ReturnDiscovery {
    Declared,
    /// The checked body has no arguments, source callees or mutable layout inputs.
    Independent,
    Body,
    Waiting(u32),
    Checked,
}

struct Pending {
    used_as_value: bool,
    inferred_params: Vec<bool>,
    params: Vec<Option<Type>>,
    none_defaults: Vec<usize>,
    document_parameters: Vec<usize>,
    document_return: Option<Type>,
    returns: Option<Type>,
    /// Storage requested by a checked consumer of this unannotated return.
    return_context: Option<Type>,
    return_context_consumers: Vec<(usize, usize)>,
    return_context_refused: bool,
    discovery: ReturnDiscovery,
}

impl Pending {
    fn is_resolved(&self) -> bool {
        self.returns.is_some() && self.params.iter().all(Option::is_some)
    }

    fn unresolved(
        &self,
        graph: &SourceGraph,
        module: ModuleId,
        function: &ast::StmtFunctionDef,
        signature: &Signature,
    ) -> Option<Diagnostic> {
        let parameter = self.params.iter().position(Option::is_none);
        if parameter.is_none() && self.returns.is_some() {
            return None;
        }
        let source = &graph.modules[module].source;
        let report = if self.used_as_value {
            Diagnostic::at(
                source,
                function,
                "using this function as a value requires Python argument binding".to_owned(),
            )
            .classified(Kind::ScopedObject)
        } else if let Some(index) = parameter {
            let name = &signature.params[index].name;
            Diagnostic::at(
                source,
                function,
                format!(
                    "cannot infer parameter {name:?}; add an annotation or a concrete direct call"
                ),
            )
            .classified(Kind::UnresolvedLayout(module, function.start()))
        } else {
            Diagnostic::at(
                source,
                function,
                "cannot infer function return type; add an annotation".to_owned(),
            )
        };
        Some(report.in_module(graph, module))
    }
}

/// An annotated sequence keeps its declared list layout and the layout observed at calls.
struct SequenceParameter {
    declared_list: Type,
    observed: Option<Type>,
}

/// Checked calls and function bodies supply missing parameter and return types.
/// Once signatures settle, the final body check produces the operations for local callbacks.
#[derive(Default)]
pub(super) struct Inference {
    pending: RefCell<Vec<Pending>>,
    sequence_parameters: RefCell<HashMap<(usize, usize), SequenceParameter>>,
    document_fields: RefCell<HashSet<(usize, usize)>>,
    /// Source contract checks shared by discovery, recovery and final body lowering.
    pub(super) source_checks: registered::SourceChecks,
    /// An unavailable class return uses a Python handle while its source neighbors are checked.
    pub(super) source_class_return: RefCell<Option<Diagnostic>>,
    revision: Cell<usize>,
    widened: Cell<bool>,
}

pub(super) struct Solution {
    pub(super) method_returns: HashMap<usize, PreparedBody>,
    pub(super) registered_parameters: HashMap<usize, Diagnostic>,
    /// Definitions checked to completion with the final input and return types.
    pub(super) checked_bodies: HashSet<usize>,
    pub(super) suspended_bodies: HashSet<usize>,
    pub(super) needs_registered_entry: bool,
    pub(super) entry_needs_object: bool,
}

impl Inference {
    pub fn register(
        &self,
        params: Vec<Option<Type>>,
        returns: Option<Type>,
        none_defaults: Vec<usize>,
    ) -> usize {
        // Even a fully annotated return can have an unannotated default that a
        // supplied argument must widen from None to its nullable scalar layout.
        if !none_defaults.is_empty() {
            self.widened.set(true);
        }
        let mut pending = self.pending.borrow_mut();
        let id = pending.len();
        pending.push(Pending {
            used_as_value: false,
            inferred_params: params.iter().map(Option::is_none).collect(),
            params,
            none_defaults,
            document_parameters: Vec::new(),
            document_return: None,
            returns,
            return_context: None,
            return_context_consumers: Vec::new(),
            return_context_refused: false,
            discovery: if returns.is_some() {
                ReturnDiscovery::Declared
            } else {
                ReturnDiscovery::Body
            },
        });
        id
    }

    pub(super) fn sequence_parameters(&self, id: usize, parameters: Vec<(usize, Type)>) {
        if !parameters.is_empty() {
            self.sequence_parameters
                .borrow_mut()
                .extend(parameters.into_iter().map(|(index, declared_list)| {
                    (
                        (id, index),
                        SequenceParameter {
                            declared_list,
                            observed: None,
                        },
                    )
                }));
            self.widened.set(true);
        }
    }

    /// An empty list display still needs the declaration's item type after an earlier tuple.
    pub(super) fn sequence_context(
        &self,
        id: usize,
        index: usize,
        expected: Type,
        site: &ast::Expr,
    ) -> Type {
        if site.is_list_expr()
            && let Some(parameter) = self.sequence_parameters.borrow().get(&(id, index))
        {
            parameter.declared_list
        } else {
            expected
        }
    }

    pub(super) fn sequence_member(
        &self,
        id: usize,
        index: usize,
        actual: Type,
        types: &Types,
    ) -> Option<Type> {
        let list = self
            .sequence_parameters
            .borrow()
            .get(&(id, index))?
            .declared_list;
        let item = types.element(list)?;
        if actual == list || types.sequence_union_element(actual) == Some(item) {
            Some(actual)
        } else if types.is_plain_tuple(actual) || types.var_tuple_element(actual).is_some() {
            Some(types.var_tuple(item))
        } else {
            None
        }
    }

    /// Bind the concrete container after its element layout has been checked. A second
    /// different container widens the slot once, without changing either caller's storage.
    pub(super) fn bind_sequence(
        &self,
        id: usize,
        index: usize,
        member: Type,
        checker: &Checker<'_>,
        value: Expr,
        site: &ast::Expr,
    ) -> Result<Expr, String> {
        let value = checker.coerce(value, member, site)?;
        let mut sequences = self.sequence_parameters.borrow_mut();
        let parameter = sequences.get_mut(&(id, index)).unwrap();
        let layout = if parameter.observed.is_none_or(|seen| seen == member) {
            member
        } else {
            checker.types.sequence_union(parameter.declared_list)
        };
        parameter.observed = Some(layout);
        if self.parameter(id, index) != Some(layout) {
            self.widen_parameter(id, index, layout);
        }
        checker.coerce(value, layout, site)
    }

    /// A first-class callable exposed with a list ABI still takes that exact pointer.
    fn keep_sequence_list(&self, id: usize, index: usize, actual: Type) {
        let slot = (id, index);
        if self
            .sequence_parameters
            .borrow()
            .get(&slot)
            .map(|parameter| parameter.declared_list)
            == Some(actual)
        {
            self.sequence_parameters.borrow_mut().remove(&slot);
            if self.parameter(id, index) != Some(actual) {
                self.widen_parameter(id, index, actual);
            }
        }
    }

    pub(super) fn document_annotations(
        &self,
        id: usize,
        parameters: Vec<usize>,
        returns: Option<Type>,
    ) {
        if !parameters.is_empty() || returns.is_some() {
            self.widened.set(true);
            let pending = &mut self.pending.borrow_mut()[id];
            pending.document_parameters = parameters;
            pending.document_return = returns;
        }
    }

    pub(super) fn discover_module_list(&self, types: &Types, owner: DefinitionId) {
        types.infer_module_list(owner);
        self.widened.set(true);
    }

    /// A fresh annotated list has no initializer element to settle its storage.
    pub(super) fn discover_list_elements(
        &self,
        graph: &SourceGraph,
        module: ModuleId,
        function: Option<&ast::StmtFunctionDef>,
    ) {
        if !self.widened.get()
            && graph
                .session
                .has_annotated_empty_list(module, function, |annotation| {
                    documents::collects_lists(graph, module, annotation)
                })
        {
            self.widened.set(true);
        }
    }

    // Use the bound argument's storage before coercion. A map child still needs
    // its ordinary shape check, while an Any parameter retains the tagged owner.
    fn document_parameter(
        &self,
        signature: &Signature,
        classes: &ClassInfo,
        index: usize,
        expected: Type,
        actual: Type,
    ) -> Type {
        let id = signature.id;
        if !matches!(actual, documents::ANY | documents::MAP | documents::LIST)
            || !self.pending.borrow()[id]
                .document_parameters
                .contains(&index)
        {
            return expected;
        }
        let ty = match expected {
            Type::ObjectValue => documents::ANY,
            Type::JsonObject => documents::MAP,
            Type::List(_) => documents::LIST,
            _ => return expected,
        };
        if ty == documents::MAP
            && let Some(method) = signature.method
            && method.generated_initializer(classes.declarations)
            && let Some(field) = classes.declarations.definitions[method.class]
                .fields
                .iter()
                .position(|field| field.name == signature.params[index].name)
            && classes.fields[method.class][field] == Type::JsonObject
        {
            self.document_field(method.class, field);
        }
        self.widen_parameter(id, index, ty);
        ty
    }

    pub(super) fn document_field(&self, class: usize, field: usize) {
        self.document_fields.borrow_mut().insert((class, field));
    }

    fn apply_document_fields(&self, classes: &mut ClassInfo) {
        let stores = self.document_fields.take();
        if stores.is_empty() {
            return;
        }
        // An inherited prefix has one storage layout for every concrete receiver.
        let declarations = &classes.declarations.definitions;
        for (mut owner, field) in stores {
            while let Some(parent) = declarations[owner].storage_base
                && field < declarations[parent].fields.len()
            {
                owner = parent;
            }
            for (id, layout) in classes.fields.iter_mut().enumerate() {
                if std::iter::successors(Some(id), |&id| declarations[id].storage_base)
                    .any(|base| base == owner)
                {
                    layout[field] = documents::MAP;
                }
            }
        }
        self.layout_changed();
    }

    pub(super) fn len(&self) -> usize {
        self.pending.borrow().len()
    }

    /// A caller-supplied sequence or callable keeps its live Python protocol in this frame.
    pub(super) fn has_inferred_protocol_argument(
        &self,
        signature: &Signature,
        types: &Types,
    ) -> bool {
        let slots = self.pending.borrow();
        let pending = &slots[signature.id];
        pending.is_resolved()
            && pending
                .inferred_params
                .iter()
                .zip(&signature.params)
                .any(|(&inferred, param)| {
                    inferred
                        && (types.with_callback_signature(param.ty, |_, _| ()).is_some()
                            || types.indexed_element(param.ty).is_some())
                })
    }

    pub(super) fn used_as_value(&self, id: usize) -> bool {
        self.pending.borrow()[id].used_as_value
    }

    fn unresolved(&self) -> bool {
        self.pending
            .borrow()
            .iter()
            .any(|value| !value.is_resolved())
    }

    fn update(&self, signature: &mut Signature) {
        let pending = self.pending.borrow();
        let inferred = &pending[signature.id];
        for (parameter, ty) in signature.params.iter_mut().zip(&inferred.params) {
            if let Some(ty) = ty {
                parameter.ty = *ty;
                if *ty == documents::ANY
                    && let Some(default) = &mut parameter.default
                    && matches!(&default.kind, ExprKind::Builtin(Builtin::BoxValue, values)
                        if matches!(values.as_slice(), [value] if value.ty == Type::None))
                {
                    *default = documents::op(ir::DocumentOp::Null, documents::ANY, vec![]);
                }
                if *ty == documents::MAP
                    && let Some(default) = &mut parameter.default
                    && matches!(
                        &default.kind,
                        ExprKind::Builtin(Builtin::JsonEmptyObject, _)
                    )
                {
                    *default = documents::op(ir::DocumentOp::EmptyMap, documents::MAP, vec![]);
                }
                if matches!(ty, Type::Optional(_))
                    && let Some(default) = &mut parameter.default
                    && default.ty == Type::None
                {
                    *default = Expr {
                        ty: *ty,
                        kind: ExprKind::Absent(Box::new(default.clone())),
                    };
                }
            }
        }
        if let Some(returns) = inferred.returns {
            signature.returns = returns;
        }
    }

    /// Borrow the round's layout when applying the pending types cannot change it.
    /// A known parameter with a default may also need its default operand rewritten.
    fn updated_signature<'s>(&self, signature: &'s Signature) -> Cow<'s, Signature> {
        let pending = self.pending.borrow();
        let inferred = &pending[signature.id];
        if inferred.returns.is_none_or(|ty| ty == signature.returns)
            && inferred
                .params
                .iter()
                .zip(&signature.params)
                .all(|(ty, param)| ty.is_none_or(|ty| ty == param.ty && param.default.is_none()))
        {
            return Cow::Borrowed(signature);
        }
        let mut updated = signature.clone();
        self.update(&mut updated);
        Cow::Owned(updated)
    }

    pub(super) fn parameter(&self, id: usize, index: usize) -> Option<Type> {
        self.pending.borrow()[id].params[index]
    }

    pub(super) fn inferred_parameter(&self, id: usize, index: usize) -> bool {
        self.pending.borrow()[id].inferred_params[index]
    }

    /// An incompatible write constrains the original incoming list, never a copy.
    pub(super) fn demand_list_write(&self, id: usize, index: usize, prior: Type, types: &Types) {
        let wide = types.list(Type::ObjectValue);
        if self.inferred_parameter(id, index)
            && self.parameter(id, index) == Some(prior)
            && prior != wide
        {
            self.widen_parameter(id, index, wide);
        }
    }

    pub(super) fn inferred_return(&self, id: usize) -> bool {
        self.pending
            .borrow()
            .get(id)
            .is_some_and(|p| !matches!(p.discovery, ReturnDiscovery::Declared))
    }

    /// A checked call argument can request storage from an unannotated producer.
    /// Publishing the request once makes unsuccessful or recursive demands converge.
    pub(super) fn demand_list_return(
        &self,
        id: usize,
        actual: Type,
        expected: Type,
        consumer: Option<(usize, usize)>,
    ) {
        let mut pending = self.pending.borrow_mut();
        let producer = &mut pending[id];
        if matches!(producer.discovery, ReturnDiscovery::Declared) {
            return;
        }
        if producer.return_context_refused {
            if consumer.is_none() && producer.returns != Some(Type::PythonObject) {
                producer.returns = Some(Type::PythonObject);
                producer.discovery = ReturnDiscovery::Independent;
                drop(pending);
                self.layout_changed();
            }
            return;
        }
        if let Some(consumer) = consumer
            && !producer.return_context_consumers.contains(&consumer)
        {
            producer.return_context_consumers.push(consumer);
        }
        if producer.returns != Some(actual) || producer.return_context == Some(expected) {
            return;
        }
        producer.return_context = Some(expected);
        drop(pending);
        self.layout_changed();
    }

    /// A shared native return ABI cannot satisfy incompatible checked consumers.
    /// Release its context and place the inferred consumers requesting it. If
    /// the write is through an alias, the producer itself needs ordinary storage.
    fn refuse_list_return(&self, id: usize) {
        let mut pending = self.pending.borrow_mut();
        if pending[id].return_context_refused {
            return;
        }
        pending[id].return_context_refused = true;
        pending[id].return_context = None;
        if pending[id].return_context_consumers.is_empty() {
            pending[id].returns = Some(Type::PythonObject);
            pending[id].discovery = ReturnDiscovery::Independent;
        }
        for (consumer, index) in std::mem::take(&mut pending[id].return_context_consumers) {
            pending[consumer].params[index] = Some(Type::PythonObject);
        }
        drop(pending);
        self.layout_changed();
    }

    pub(super) fn learn_parameter(&self, id: usize, index: usize, ty: Type) -> Type {
        let mut pending = self.pending.borrow_mut();
        let slot = &mut pending[id].params[index];
        *slot.get_or_insert_with(|| {
            self.revision.set(self.revision.get() + 1);
            ty
        })
    }

    /// The default None establishes absence, not the payload type of supplied arguments.
    /// An annotation or a non-None default keeps its original exact type contract.
    fn none_default(&self, id: usize, index: usize) -> bool {
        let pending = self.pending.borrow();
        pending[id].none_defaults.contains(&index) && pending[id].params[index] == Some(Type::None)
    }

    fn widen_none_default(&self, id: usize, index: usize, value: Type, types: &Types) -> Type {
        let payload = types.payload(value).unwrap_or(value);
        if !matches!(
            payload,
            Type::Str | Type::Bytes | Type::Bool | Type::Int | Type::Float | Type::Class(_)
        ) {
            return Type::None;
        }
        let expected = types.optional(payload);
        self.widen_parameter(id, index, expected);
        expected
    }

    /// Discover the actual input type of closed keyword calls even when all
    /// written signatures have annotations. Python annotations do not reject None.
    pub(super) fn discover_packed_arguments(&self) {
        self.widened.set(true);
    }

    fn widen_parameter(&self, id: usize, index: usize, ty: Type) {
        self.pending.borrow_mut()[id].params[index] = Some(ty);
        self.layout_changed();
    }

    pub(super) fn layout_changed(&self) {
        for function in &mut *self.pending.borrow_mut() {
            if !matches!(
                function.discovery,
                ReturnDiscovery::Declared | ReturnDiscovery::Independent
            ) {
                function.returns = None;
            }
        }
        self.widened.set(true);
        self.revision.set(self.revision.get() + 1);
    }

    /// A later call from a different subcommand replaces a child layout with
    /// its stable parent prefix. Revisit helpers whose result may pass it on.
    fn widen_namespace(&self, id: usize, index: usize, parent: Type, parsers: &argparse::Parsers) {
        let mut pending = self.pending.borrow_mut();
        pending[id].params[index] = Some(parent);
        for function in &mut *pending {
            if !matches!(function.discovery, ReturnDiscovery::Declared)
                && let (Some(Type::Class(child)), Type::Class(parent)) = (function.returns, parent)
                && parsers.child_of(child, parent)
            {
                function.returns = None;
            }
        }
        self.widened.set(true);
        self.revision.set(self.revision.get() + 1);
    }

    /// Revisit checked bodies after widening or before choosing a terminal diagnostic.
    fn revisit_checked_returns(&self) {
        for signature in self.pending.borrow_mut().iter_mut() {
            if matches!(
                signature.discovery,
                ReturnDiscovery::Waiting(_) | ReturnDiscovery::Checked
            ) {
                signature.discovery = ReturnDiscovery::Body;
            }
        }
    }

    /// Learn evidence from an expected callable, then answer the requested signature query.
    /// An ordinary call can use a known return while a first-class callable needs every parameter.
    fn lookup(
        &self,
        signature: &Signature,
        request: SignatureUse,
        checker: &Checker<'_>,
        range: TextRange,
    ) -> Result<Type, String> {
        let needs_parameters = matches!(request, SignatureUse::Callable { .. });
        if let SignatureUse::Callable {
            skip,
            expected: Some((params, returns)),
        } = request
        {
            if params.len() + skip != signature.params.len() {
                return Err(error(
                    checker.source,
                    &range,
                    "function value does not match its Callable parameter count",
                ));
            }
            for (index, ty) in params.into_iter().enumerate() {
                let index = index + skip;
                self.keep_sequence_list(signature.id, index, ty);
                self.learn_parameter(signature.id, index, ty);
            }
            let mut pending = self.pending.borrow_mut();
            if pending[signature.id].returns.is_none() {
                pending[signature.id].returns = Some(returns);
                self.revision.set(self.revision.get() + 1);
            }
        }
        let mut pending = self.pending.borrow_mut();
        let candidate = &mut pending[signature.id];
        candidate.used_as_value |= needs_parameters;
        let missing_callable_parameters =
            needs_parameters && candidate.params.iter().any(Option::is_none);
        candidate
            .returns
            .filter(|_| !missing_callable_parameters)
            .ok_or(Failure::PendingSignature {
                signature: signature.id,
                missing_callable_parameters,
                module: checker.module,
                range,
            })
    }

    pub(super) fn callable_signature<'s>(
        &self,
        signature: &'s Signature,
        skip: usize,
        expected: Option<(Vec<Type>, Type)>,
        checker: &Checker<'_>,
        site: &impl Ranged,
    ) -> Result<Cow<'s, Signature>, String> {
        self.lookup(
            signature,
            SignatureUse::Callable { skip, expected },
            checker,
            site.range(),
        )?;
        Ok(self.updated_signature(signature))
    }

    /// Inherited copies of one method have distinct receivers but identical
    /// source parameters. At a fixed point, a copy with no direct calls can use
    /// a parameter type on which the called copies agree. Each body still has
    /// to type-check for its own receiver, including overridden method calls.
    fn infer_inherited_parameters(
        &self,
        definitions: &[FunctionDefinition<'_>],
        classes: &ClassInfo,
    ) -> bool {
        let mut copies = HashMap::new();
        for (_, function, signature) in definitions {
            if let Some(method) = signature.method
                && method.kind(classes.declarations) == crate::classes::MethodKind::Instance
            {
                let class = &classes.declarations.definitions[method.class];
                let origin = (
                    class.method_origins[method.index],
                    function.name.as_str(),
                    function.range(),
                );
                copies
                    .entry(origin)
                    .or_insert_with(Vec::new)
                    .push(signature.id);
            }
        }
        let mut pending = self.pending.borrow_mut();
        let mut changed = false;
        for ids in copies.values().filter(|ids| ids.len() > 1) {
            for index in 1..pending[ids[0]].params.len() {
                let mut known = ids.iter().filter_map(|&id| pending[id].params[index]);
                let Some(first) = known.next() else { continue };
                if known.any(|ty| ty != first) {
                    continue;
                }
                for &id in ids {
                    if pending[id].params[index].is_none() {
                        pending[id].params[index] = Some(first);
                        changed = true;
                    }
                }
            }
        }
        changed
    }

    /// At the scalar fixed point, the registered binder can supply real Python objects
    /// for the remaining arguments of first-party source functions. A missing return
    /// alone may belong to an unresolved recursive call, which keeps its original diagnostic.
    fn use_registered_arguments(
        &self,
        graph: &SourceGraph,
        definitions: &[FunctionDefinition<'_>],
        registered: &mut HashMap<usize, Diagnostic>,
    ) -> bool {
        let mut changed = false;
        let mut pending = self.pending.borrow_mut();
        for (module, function, signature) in definitions {
            let state = &mut pending[signature.id];
            if signature.method.is_some()
                || !graph.modules[*module].is_program_source()
                || state.params.iter().all(Option::is_some)
            {
                continue;
            }
            let reason = state
                .unresolved(graph, *module, function, signature)
                .expect("unresolved source parameter");
            registered.insert(signature.id, reason);
            for parameter in &mut state.params {
                parameter.get_or_insert(Type::PythonObject);
            }
            state.returns.get_or_insert(Type::PythonObject);
            changed = true;
        }
        changed
    }

    /// Stops when every signature is concrete, or no new type evidence is available.
    /// A slot can widen from a command to its common namespace only once.
    #[expect(
        clippy::too_many_arguments,
        reason = "the caller reuses the entry statements for lowering and owns the signature and layout tables"
    )]
    pub fn solve(
        &mut self,
        graph: &SourceGraph,
        entry_statements: &[ast::Stmt],
        owners: &[DefinitionId],
        definitions: &mut [FunctionDefinition<'_>],
        signatures: &mut HashMap<DefinitionId, Signature>,
        classes: &mut ClassInfo,
        types: &Types,
    ) -> std::result::Result<Solution, Diagnostic> {
        if classes
            .fields
            .iter()
            .any(|fields| fields.contains(&Type::JsonObject))
        {
            self.widened.set(true);
        }
        let mut registered = HashMap::new();
        let mut checked_bodies = HashSet::new();
        let mut suspended_bodies = HashSet::new();
        let mut method_returns = HashMap::new();
        // Completed returns that use only bound locals survive unrelated revisions.
        let mut local_returns = HashMap::new();
        let mut discovered_set_bodies = Vec::new();
        let mut checked_before_registration = HashSet::new();
        let sources = (|| {
            let pending = self.pending.borrow();
            let mut inputs = HashSet::new();
            let mut callers = HashSet::new();
            for (module, function, signature) in &*definitions {
                let state = &pending[signature.id];
                if state.none_defaults.is_empty() && !state.inferred_params.contains(&true) {
                    continue;
                }
                let owner = owners[signature.id];
                if *module != graph.entry
                    || signature.method.is_some()
                    || !fixed_frame_provider(graph, owner, function)
                {
                    return None;
                }
                inputs.insert(owner);
                callers.insert(function.start());
            }
            if inputs.is_empty() {
                return Some(callers);
            }
            let mut names = HashSet::new();
            for (id, module) in graph.modules.iter().enumerate() {
                if id != graph.entry && !module.bundled {
                    return None;
                }
                for (name, binding) in &module.bindings {
                    match binding {
                        Binding::Module(module) if *module == graph.entry => return None,
                        Binding::Definition(def) if inputs.contains(def) => {
                            if id != graph.entry {
                                return None;
                            }
                            names.insert(name.as_str());
                        }
                        _ => {}
                    }
                }
            }
            let original = callers.len();
            let identifies = |expr: &ast::Expr| {
                expr.as_name_expr()
                    .is_some_and(|n| names.contains(n.id.as_str()))
            };
            for statement in graph.modules[graph.entry].parsed.suite() {
                let observed = if let ast::Stmt::FunctionDef(f) = statement {
                    f.type_params.is_some()
                        || imre_ast::function_definition_expressions(f)
                            .chain(
                                super::parameter_annotations(&f.parameters, f.returns.as_deref())
                                    .map(|(_, value)| value),
                            )
                            .any(|value| ast::helpers::any_over_expr(value, &identifies))
                } else {
                    local_functions::reads_function_value(
                        std::slice::from_ref(statement),
                        identifies,
                        None,
                        |_| false,
                    )
                };
                if observed {
                    return None;
                }
            }
            graph
                .session
                .module_function_readers(graph.entry, &names, |function| {
                    if local_functions::reads_function_value(
                        &function.body,
                        identifies,
                        None,
                        |_| true,
                    ) {
                        return false;
                    }
                    callers.insert(function.start());
                    true
                })?;
            (callers.len() > original).then_some(callers)
        })();
        let mut first_open_return = sources.as_ref().map(|_| 0);
        let argument_sources = sources.as_ref().filter(|callers| !callers.is_empty());
        let mut before = self.revision.get();
        let mut entry_object_revision = None;
        let fixed_dependencies = loop {
            let widened = self.widened.replace(false);
            let signatures_settled = !self.unresolved();
            if !widened && signatures_settled {
                break None;
            }
            method_returns.clear();
            let previous_checks = std::mem::take(&mut checked_before_registration);
            checked_bodies.clear();
            if widened {
                local_returns.retain(|id, _| previous_checks.contains(id));
                // Registration can request the initial pass before any layout changes.
                if self.revision.get() != 0 {
                    first_open_return = None;
                }
                self.revisit_checked_returns();
            }
            let mut skipped = false;
            for (_, _, signature) in definitions.iter_mut() {
                self.update(signature);
            }
            for signature in signatures.values_mut() {
                self.update(signature);
            }
            for methods in classes.methods.iter_mut().chain(&mut classes.setters) {
                for signature in methods.values_mut() {
                    self.update(signature);
                }
            }
            before = self.revision.get();
            let registered_entry = signatures_settled
                && (!suspended_bodies.is_empty()
                    || signatures.values().any(Signature::registered_frame));
            let class_globals = classes.declarations.has_class_values(graph.entry);
            let helpers = Helpers::new(self.len() + graph.initializers(class_globals).count());
            let globals = ModuleValues::for_initializers(!registered.is_empty());
            let mut available = HashSet::new();
            let mut errors = Vec::new();
            let mut checked_callers = Vec::new();
            let mut completed_calls = classes.nonreturning_functions.clone();
            let initial_nonreturning = completed_calls.len();
            let mut checked_revision = before;
            let mut conflicting_returns = None;
            let mut opaque_returns = Vec::new();
            let mut staged_returns = Vec::new();
            let mut fixed_dependencies = None;
            for (module, range) in graph.initializers(class_globals) {
                let mut checker = Checker::new(
                    graph, module, signatures, classes, types, &helpers, &globals,
                );
                checker.inference = Some(self);
                if let Err(diagnostic) = Initialization::lower(checker, &mut available, range, None)
                {
                    if registered.is_empty()
                        && graph.modules[module].is_program_source()
                        && diagnostic.is_representation()
                    {
                        fixed_dependencies.get_or_insert_with(Vec::new);
                    }
                    errors.push((module, None, diagnostic));
                }
            }
            for (module, function, signature) in definitions.iter() {
                let physical = matches!(function, Cow::Borrowed(_));
                let function = function.as_ref();
                if graph.modules[*module].archive.is_some() {
                    continue; // Native declarations must be fully annotated.
                }
                // The call binder transfers mutable document layouts to their fields.
                // The final body checker owns the generated parameter stores.
                if signature
                    .method
                    .is_some_and(|method| method.generated_initializer(classes.declarations))
                {
                    continue;
                }
                let (ready, infer_return, retrying, document_return) = {
                    let signatures = self.pending.borrow();
                    if let Some(slot) = &mut first_open_return
                        && !self.widened.get()
                    {
                        while *slot < signatures.len() && signatures[*slot].returns.is_some() {
                            *slot += 1;
                        }
                        // The final body check owns settled functions and methods.
                        if signatures[signature.id].returns.is_some()
                            && argument_sources.is_none_or(|sources| {
                                *module == graph.entry
                                    && physical
                                    && !sources.contains(&function.start())
                            })
                            && signature.physical_frame
                            && !graph.session.is_generator_definition(*module, function)
                        {
                            skipped |= argument_sources.is_some() || *slot < signatures.len();
                            continue;
                        }
                    }
                    let pending = &signatures[signature.id];
                    let skip = match pending.discovery {
                        ReturnDiscovery::Independent => continue,
                        ReturnDiscovery::Waiting(callee) => {
                            signatures[callee as usize].returns.is_none()
                        }
                        ReturnDiscovery::Checked => true,
                        _ => false,
                    };
                    if skip && !self.widened.get() {
                        skipped = true;
                        continue;
                    }
                    (
                        pending.params.iter().all(Option::is_some),
                        pending.returns.is_none(),
                        matches!(pending.discovery, ReturnDiscovery::Waiting(_)),
                        pending.document_return,
                    )
                };
                if !ready {
                    continue;
                }
                if (before == self.revision.get() && previous_checks.contains(&signature.id))
                    || (!self.widened.get() && local_returns.contains_key(&signature.id))
                    || suspended_bodies.contains(&signature.id)
                {
                    checked_bodies.insert(signature.id);
                    continue;
                }
                // The round already updated each signature. Earlier calls can refine its inputs.
                let updated;
                let signature = if self.revision.get() == before {
                    signature
                } else {
                    updated = self.updated_signature(signature);
                    updated.as_ref()
                };
                let discover_returns = infer_return || document_return.is_some();
                // Local scalar and Python values cannot refine another declaration.
                // Let the ordinary body checker retain their operations and positions.
                if signatures_settled
                    && !discover_returns
                    && registered.contains_key(&signature.id)
                    && signature.params.iter().all(|param| {
                        param.ty == Type::PythonObject || param.ty.builtin_scalar_name().is_some()
                    })
                    && graph
                        .session
                        .function_uses_fixed_names(*module, function, false, |name| {
                            function.returns.is_none()
                                && module_builtin_name(&graph.modules[*module], name)
                        })
                {
                    continue;
                }
                let mut callable_inputs = HashMap::new();
                let mut checker = Checker::new(
                    graph, *module, signatures, classes, types, &helpers, &globals,
                );
                if graph.registered_startup
                    && signature.method.is_none()
                    && signature.params.is_empty()
                    && function.returns.is_none()
                    && signature.physical_frame
                {
                    checker.settled_attributes = Some(&mut callable_inputs);
                }
                checker.nonreturning_functions = &completed_calls;
                if signature.registered_frame() {
                    checker.bound_function_imports =
                        self.source_checks.accepts(graph, *module, function.range());
                    checker.registered = Some(graph.modules[*module].registered_values());
                }
                globals.provisional_return.set(false);
                checker.inference = Some(self);
                checker.discover_document_return = document_return;
                checker.inferred_returns = discover_returns.then(Vec::new);
                checker.inferred_return_context =
                    self.pending.borrow()[signature.id].return_context;
                let body_revision = self.revision.get();
                // With only scalar inputs and local bindings, a checked return can keep
                // its settled scalar, list or tuple layout across unrelated revisions.
                let keep_return = (body_revision != before
                    || registered_entry && signature.method.is_some())
                    && !self.widened.get()
                    && !discover_returns
                    && !signature.registered_frame()
                    && !checker.fault_enabled()
                    && (registered::physical_method(signature, function, classes).is_some()
                        || signature.method.is_none()
                            && physical
                            && graph.modules[*module].is_program_source()
                            && fixed_frame_entry(function)
                            && (signature.returns.builtin_scalar_name().is_some()
                                || types.is_plain_tuple(signature.returns)
                                || types.element(signature.returns).is_some())
                            && signature
                                .params
                                .iter()
                                .all(|p| p.ty.builtin_scalar_name().is_some())
                            && graph.session.function_uses_fixed_names(
                                *module,
                                function,
                                false,
                                |_| false,
                            ));
                // Preserve the actual operations when an open return may be settled by a
                // fresh native set. This queries the existing semantic index, not the suite.
                let reads_set = Cell::new(false);
                let keep_set_body = infer_return
                    && document_return.is_none()
                    && physical
                    && signature.method.is_none()
                    && function.returns.is_none()
                    && fixed_frame_entry(function)
                    && !signature.registered_frame()
                    && !checker.fault_enabled()
                    && signature.params.iter().all(|param| {
                        param.default.is_none() && param.ty.builtin_scalar_name().is_some()
                    })
                    && graph.session.function_reads_only(
                        *module,
                        function,
                        crate::analysis::EagerDisplays::Excluded,
                        |_| true,
                        |name| {
                            if name == "set" {
                                reads_set.set(true);
                            }
                            module_builtin_name(&graph.modules[*module], name)
                        },
                        |_| true,
                    )
                    && reads_set.get();
                // A settled native body needs its instructions even without Python operand sites.
                checker.placement_only =
                    !keep_set_body && (!keep_return || signature.physical_frame);
                let mut method_storage =
                    (keep_return || keep_set_body).then(|| (helpers.len(), globals.types().len()));
                if keep_return || keep_set_body {
                    checker.frame = LexicalFrame::Named(function.name.id.clone());
                    checker.fixed_sites = (keep_return
                        && (signature.physical_frame
                            || signature.method.is_none()
                                && fixed_frame_provider(graph, owners[signature.id], function)))
                    .then(|| fixed_body_source::SourceSites::new(signature.physical_frame));
                }
                // Solving an open signature invalidates a method's layout. Check its operand
                // sites as usual, but only keep the instructions when they can survive the round.
                let mut return_body = ((keep_return || keep_set_body)
                    && (signature.method.is_none() || signatures_settled || !self.unresolved()))
                .then(|| (Vec::new(), Vec::new()));
                let statements = checker.function_statements(function);
                let mut checked_return = false;
                let mut implicit_none = false;
                let mut superseded_returns = false;
                let mut checked_call_cannot_return = false;
                let mut discovered_set_return = None;
                // A reachable call to a definition that already declined fixed lowering
                // can use its live Python binding. Keep the frame only in its settled round.
                if physical
                    && signatures_settled
                    && self.revision.get() == before
                    && fixed_dependencies.is_none()
                    && signature.method.is_none()
                    && !function.is_async
                    && graph.needs_object_runtime.get()
                    && graph.modules[*module].is_program_source()
                    && errors.iter().any(|(id, owner, failure)| {
                        *id == *module
                            && failure.is_representation()
                            && owner.is_some_and(|owner| {
                                let slot = signatures[&owner].id;
                                graph.session.function_reads_global(
                                    *module,
                                    function,
                                    &definitions[slot].1.name.id,
                                )
                            })
                    })
                    && !graph.session.is_generator_definition(*module, function)
                    && let Ok(Some((body, placement))) = registered::prepare_checked_definition(
                        &checker,
                        function,
                        signature,
                        self,
                        None,
                        registered::ReturnCheck::Declared,
                    )
                    && !body.cannot_return
                    && self.revision.get() == before
                    && errors.iter().any(|(id, owner, failure)| {
                        *id == *module
                            && failure.is_representation()
                            && owner
                                .is_some_and(|owner| body.calls.contains(&signatures[&owner].id))
                    })
                {
                    checked_callers.push((signature.id, placement));
                    continue;
                }
                // A binding in the semantic index already selects this coroutine's Python
                // iterator protocol. Keep its checked placement across diagnostic discovery.
                let place_suspended = |checker: &Checker<'_>| -> Result<bool, String> {
                    let Some((_, placement)) = registered::prepare_checked_definition(
                        checker,
                        function,
                        signature,
                        self,
                        None,
                        registered::ReturnCheck::Declared,
                    )?
                    else {
                        return Ok(false);
                    };
                    placement.place(graph);
                    Ok(true)
                };
                let mut lexical = None;
                let mut placement = (physical
                    && function.is_async
                    && signature.method.is_none()
                    && graph.modules[*module].is_program_source()
                    && graph
                        .session
                        .coroutine_uses_async_iteration(*module, function))
                .then(|| place_suspended(&checker));
                let infer_namespace = physical
                    && infer_return
                    && !function.is_async
                    && signature.method.is_none()
                    && graph.modules[*module].is_program_source()
                    && function.returns.as_deref().is_some_and(|annotation| {
                        graph.resolve(*module, annotation)
                            == Some(Binding::Intrinsic(Intrinsic::ArgparseNamespace))
                    })
                    && self.source_checks.accepts(graph, *module, function.range());
                if infer_namespace
                    && graph
                        .session
                        .returns_fixed_operand(*module, function, |range| {
                            classes.parsers.has_fixed_call(*module, range)
                        })
                {
                    if !checker.fault_enabled() && signature.params.is_empty() {
                        checker.placement_only = false;
                        return_body = Some((Vec::new(), Vec::new()));
                        method_storage = Some((helpers.len(), globals.types().len()));
                    }
                } else if infer_namespace {
                    let mut candidate = signature.clone();
                    candidate.returns = Type::PythonObject;
                    placement = Some(
                        registered::prepare_checked_definition(
                            &checker,
                            function,
                            &candidate,
                            self,
                            None,
                            registered::ReturnCheck::Infer,
                        )
                        .map(|placed| {
                            placed
                                .filter(|(body, _)| body.calls.is_empty() && !body.cannot_return)
                                .is_some_and(|(body, placement)| {
                                    if body.python_call_result {
                                        placement.place(graph);
                                        true
                                    } else {
                                        lexical = Some(placement);
                                        false
                                    }
                                })
                        }),
                    );
                }
                let mut retained = matches!(placement, Some(Ok(true)));
                let result = (|| {
                    if retained {
                        return Ok(());
                    }
                    checker.bind_function_signature(function, signature, |slot| {
                        if let Some((params, _)) = &mut return_body {
                            params.push(slot);
                        }
                    })?;
                    checker.collect_local_definitions(statements)?;
                    let falls_through = checker.lower_block_with_placement(
                        statements,
                        return_body.as_mut().map(|(_, body)| body),
                        None,
                        Some(function),
                    )?;
                    checker.finish_caller_frame_stores(signature, falls_through);
                    // An outstanding call cannot establish an implicit return, even
                    // when checking subsequent statements reached the end of the body.
                    if matches!(
                        *checker.unrepresented.borrow(),
                        Some(Failure::PendingSignature { .. })
                    ) {
                        return Ok(());
                    }
                    checked_call_cannot_return =
                        checker.checked_call_cannot_return(function, falls_through);
                    if let Some(document) = checker.discover_document_return {
                        let returns = checker.inferred_returns.take().unwrap();
                        if !falls_through
                            && !returns.is_empty()
                            && returns.iter().all(|&ty| ty == document)
                        {
                            let mut pending = self.pending.borrow_mut();
                            pending[signature.id].returns = Some(document);
                            pending[signature.id].document_return = None;
                            drop(pending);
                            self.layout_changed();
                        }
                    } else if let Some(mut returns) = checker.inferred_returns.take() {
                        superseded_returns =
                            !falls_through && !returns.is_empty() && !checker.explicit_return;
                        if falls_through || returns.is_empty() {
                            returns.push(Type::None);
                        }
                        // A Python result can also hold every concrete returning branch.
                        let mut ty = if returns.contains(&Type::PythonObject) {
                            Type::PythonObject
                        } else {
                            returns[0]
                        };
                        for &other in &returns[1..] {
                            let Some(joined) = types
                                .join_results(ty, other)
                                .or_else(|| classes.parsers.common_namespace(ty, other))
                            else {
                                conflicting_returns.get_or_insert((*module, function.range()));
                                return Ok(());
                            };
                            ty = joined;
                        }
                        implicit_none =
                            ty == Type::None && falls_through && !checker.explicit_return;
                        if keep_set_body
                            && !falls_through
                            && returns.iter().all(|&other| other == ty)
                            && types
                                .element(ty)
                                .is_some_and(|element| element.builtin_scalar_name().is_some())
                            && checker
                                .inferred_containers
                                .iter()
                                .any(|&slot| types.set_element(checker.locals[slot]).is_some())
                        {
                            discovered_set_return = Some(ty);
                        }
                        self.pending.borrow_mut()[signature.id].returns = Some(ty);
                        self.revision.set(self.revision.get() + 1);
                    }
                    checked_return =
                        !infer_return && (!falls_through || signature.returns == Type::None);
                    Ok::<_, Failure>(())
                })();
                let revision = self.revision.get();
                if revision != checked_revision {
                    method_returns.clear();
                    checked_bodies.clear();
                    checked_revision = revision;
                }
                // A later round can select a different operand if this body used a provisional return.
                let settled_return_inputs = !globals.provisional_return.get();
                let mut result = checker.finish_region(result);
                if let Err(diagnostic) = &mut result {
                    if let Failure::Diagnostic(report) = diagnostic
                        && let Kind::InferredMethodReceiver(index) = report.kind
                        && physical
                        && let Some(parameter) = signature.params.get(index)
                        && checker.names.get(&parameter.name) == Some(&index)
                        && self.pending.borrow()[signature.id].inferred_params[index]
                        && self.parameter(signature.id, index) != Some(Type::PythonObject)
                    {
                        // This incoming layout cannot supply the live method of the bound parameter.
                        self.widen_parameter(signature.id, index, Type::PythonObject);
                        continue;
                    }
                    // A completed return check can still require Python operations. Only the
                    // registered frame may own that body, after its ordinary source check.
                    if diagnostic.is_representation() && graph.modules[*module].is_program_source()
                    {
                        if checked_return {
                            staged_returns.push(signature.id);
                        } else if implicit_none
                            && revision == body_revision + 1
                            && settled_return_inputs
                        {
                            // The body reached its end and only its return changed. Widening
                            // or a stalled diagnostic pass will revisit the original calls.
                            self.pending.borrow_mut()[signature.id].discovery =
                                ReturnDiscovery::Checked;
                        }
                    }
                    // An invalid operand does not give a procedure a different return type.
                    // Let the ordinary binder report the same body as an explicit -> None.
                    if infer_return
                        && matches!(diagnostic, Failure::Diagnostic(report) if report.exact_operand().is_some())
                        && !function.is_async
                        && !imre_ast::code_flags::is_generator(&function.body)
                    {
                        let exits = crate::analysis::explicit_returns(function);
                        if exits.iter().all(|ret| {
                            matches!(ret.value.as_deref(), None | Some(ast::Expr::NoneLiteral(_)))
                        }) && (graph.session.call_may_return_implicitly(*module, function)
                            || graph.session.has_reachable_return(*module, &exits))
                        {
                            self.pending.borrow_mut()[signature.id].returns = Some(Type::None);
                            self.revision.set(self.revision.get() + 1);
                        }
                    }
                    retained = if matches!(diagnostic, Failure::Diagnostic(report)
                        if report.kind == Kind::FrameSuspension && signature.method.is_none())
                    {
                        if let Some(retained) = registered::retain_generator(
                            graph,
                            owners[signature.id],
                            &self.source_checks,
                        ) {
                            retained
                        } else if physical
                            && function.is_async
                            && graph.modules[*module].is_program_source()
                        {
                            match placement
                                .take()
                                .unwrap_or_else(|| place_suspended(&checker))
                            {
                                Ok(retained) => retained,
                                Err(reason) => {
                                    *diagnostic = reason;
                                    false
                                }
                            }
                        } else {
                            false
                        }
                    } else {
                        false
                    };
                }
                if result.is_ok()
                    && let Some(frame) = lexical
                    && revision == body_revision + 1
                    && self.pending.borrow()[signature.id].returns.is_some()
                {
                    // Only publishing this definition's return advanced inference. A later
                    // signature or nonreturning change discards it with the other prepared bodies.
                    method_returns.insert(signature.id, PreparedBody::Python(frame));
                }
                if retained {
                    suspended_bodies.insert(signature.id);
                    checked_bodies.insert(signature.id);
                    if infer_return {
                        self.pending.borrow_mut()[signature.id].returns = Some(Type::PythonObject);
                        self.revision.set(self.revision.get() + 1);
                    }
                    continue;
                }
                if let Err(diagnostic) = result {
                    if self.pending.borrow()[signature.id].returns.is_none()
                        && diagnostic.is_representation()
                        && graph.modules[*module].is_program_source()
                    {
                        opaque_returns.push(signature.id);
                    }
                    if let Failure::PendingSignature {
                        signature: callee, ..
                    } = &diagnostic
                        && infer_return
                        && settled_return_inputs
                        && {
                            let pending = self.pending.borrow();
                            pending[*callee].returns.is_none()
                                && !pending[signature.id].inferred_params.contains(&true)
                                && pending[*callee].params.iter().all(Option::is_some)
                        }
                        && let Ok(callee) = u32::try_from(*callee)
                    {
                        self.pending.borrow_mut()[signature.id].discovery =
                            ReturnDiscovery::Waiting(callee);
                    }
                    let owner = signature.method.is_none().then(|| owners[signature.id]);
                    errors.push((*module, owner, diagnostic));
                } else {
                    if (checked_return || implicit_none)
                        && revision == body_revision + usize::from(implicit_none)
                        && (revision == before
                            || signature.physical_frame
                                && signature.method.is_none()
                                && !checked_call_cannot_return
                                && !checker.calls.contains(&signature.id))
                    {
                        checked_bodies.insert(signature.id);
                        if implicit_none
                            && let Some(sites) = checker.settled_attributes.take()
                            && !sites.is_empty()
                        {
                            method_returns.insert(
                                signature.id,
                                PreparedBody::Attributes(std::mem::take(sites)),
                            );
                        }
                        if let Some(dependencies) = &mut fixed_dependencies
                            && (!signature.physical_frame
                                || signature.method.is_some()
                                || !checker.calls.is_empty()
                                || checked_call_cannot_return
                                || checker.wrapper_link.get())
                        {
                            dependencies.push(signature.id);
                        }
                    }
                    if infer_return
                        && settled_return_inputs
                        && signature.params.is_empty()
                        && !signature.registered_frame()
                        && self.pending.borrow()[signature.id]
                            .returns
                            .is_some_and(|ty| {
                                ty == Type::None || ty.builtin_scalar_name().is_some()
                            })
                        && checker.calls.is_empty()
                        && checker.class_reads.borrow().is_empty()
                        && graph.session.function_uses_fixed_names(
                            *module,
                            function,
                            false,
                            |name| {
                                exception_kind(name).is_some()
                                    && !graph.modules[*module].bindings.contains_key(name)
                            },
                        )
                    {
                        self.pending.borrow_mut()[signature.id].discovery =
                            ReturnDiscovery::Independent;
                    } else if retrying && settled_return_inputs {
                        self.pending.borrow_mut()[signature.id].discovery =
                            ReturnDiscovery::Checked;
                    }
                    if let Some(ty) = discovered_set_return
                        && revision == body_revision + 1
                        && settled_return_inputs
                        && !checked_call_cannot_return
                        && checker.calls.is_empty()
                        && checker.class_reads.borrow().is_empty()
                        && checker.unrepresented.borrow().is_none()
                        && method_storage == Some((helpers.len(), globals.types().len()))
                        && let Some((params, body)) = return_body.take()
                    {
                        checker.returns = ty;
                        local_returns.insert(signature.id, PreparedBody::Fixed(checker.completed_body(params, body, None, false)));
                        discovered_set_bodies.push(signature.id);
                    } else if infer_return && revision == body_revision + 1 && !checked_call_cannot_return
                        && let Some((params, body)) = &mut return_body
                        && let [Statement::Return(value)] = body.as_slice()
                        && let ExprKind::Builtin(Builtin::ParseArgs { process_argv: true, .. }, args) = &value.kind
                        && matches!(args[1].kind, ExprKind::Str(_))
                        && self.pending.borrow()[signature.id].returns == Some(value.ty)
                        && checker.locals.is_empty() && checker.calls.is_empty() && checker.class_reads.borrow().is_empty()
                        && method_storage == Some((helpers.len(), globals.types().len()))
                    {
                        // With a literal description, the schema and process argv need no other
                        // signature or module storage. Keep the operation that settled the return.
                        checker.returns = value.ty;
                        // The Python launcher will bind and call the live parser. Its native
                        // slot needs the checked return type, but has no instructions to emit.
                        let body = if graph.registered_startup { Vec::new() } else { std::mem::take(body) };
                        let result = checker.completed_body(std::mem::take(params), body, None, false);
                        local_returns.insert(signature.id, PreparedBody::Fixed(result));
                    } else if let Some((params, body)) = return_body
                        && checked_return && !checked_call_cannot_return && body_revision == revision
                        && checker.calls.is_empty() && checker.class_reads.borrow().is_empty()
                        && checker.unrepresented.borrow().is_none()
                        && method_storage == Some((helpers.len(), globals.types().len()))
                        && let source = checker.fixed_body_source(function, signature, &params, statements.len(), &body)
                        // Module storage and method targets belong to the graph that selected them.
                        // A written global callee will be looked up again by the Python frame.
                        && source.as_ref().map_or(!signature.physical_frame, |source| {
                            source.attributes.values().all(|attribute| match attribute {
                                FixedAttribute::Load(_) | FixedAttribute::UnhighlightedCall(_) => true,
                                FixedAttribute::Call(Some(target), _) => matches!(&target.without_location().kind,
                                    ExprKind::Builtin(Builtin::RegisteredName(_), args) if args.is_empty()),
                                // SourceSites::method checked the receiver and every written operand.
                                FixedAttribute::Call(None, call) => signature.method.is_some()
                                    && matches!(call.target, ir::RegisteredCallTarget::Method { .. })
                                    && matches!(call.arguments, ir::RegisteredArguments::Positional),
                                FixedAttribute::OriginalCall(_) | FixedAttribute::CallValue { .. } | FixedAttribute::ModuleName(_) => false,
                            })
                        })
                    {
                        let returns = if signature.method.is_none() {
                            &mut local_returns
                        } else {
                            &mut method_returns
                        };
                        returns.insert(signature.id, PreparedBody::Fixed(checker.completed_body(params, body, source, false)));
                    }
                    if superseded_returns
                        && !graph.session.call_may_return_implicitly(*module, function)
                    {
                        classes.nonreturning_functions.insert(signature.id);
                    }
                    if checked_call_cannot_return {
                        completed_calls.insert(signature.id);
                    }
                }
            }
            // A changed round with only resolved return slots has no input or layout
            // left to learn from the entry. Its ordinary check can own these operations.
            let entry_callable = if argument_sources.is_none()
                && first_open_return == Some(self.len())
                && self.revision.get() != before
                && !self.widened.get()
            {
                None
            } else {
                let mut checker = Checker::new(
                    graph,
                    graph.entry,
                    signatures,
                    classes,
                    types,
                    &helpers,
                    &globals,
                );
                checker.inference = Some(self);
                checker.module_scope = true;
                checker.nonreturning_functions = &completed_calls;
                let entry_revision = self.revision.get();
                let result = checker.lower_block(entry_statements, None);
                let result = checker.finish_region(result);
                entry_object_revision = result
                    .as_ref()
                    .is_err_and(Failure::is_object)
                    .then_some(entry_revision);
                match result {
                    Err(Failure::PendingSignature {
                        signature,
                        missing_callable_parameters: true,
                        ..
                    }) => Some(signature),
                    Err(diagnostic) => {
                        errors.push((graph.entry, None, diagnostic));
                        None
                    }
                    Ok(_) => None,
                }
            };
            self.apply_document_fields(classes);
            if self.revision.get() == before {
                // Recovery can borrow the unchanged round's signatures for its resolved slots.
                let resolved = |(slot, _): &(usize, _)| self.pending.borrow()[*slot].is_resolved();
                // Reproduce the canonical diagnostic pass if discovery stalls.
                if skipped && self.unresolved() {
                    first_open_return = None;
                    self.revisit_checked_returns();
                    continue;
                }
                if conflicting_returns.is_none() && !self.unresolved() {
                    if completed_calls.len() == initial_nonreturning
                        && classes.nonreturning_functions.len() == initial_nonreturning
                    {
                        for (id, placement) in checked_callers {
                            checked_bodies.insert(id);
                            placement.place(graph);
                        }
                    }
                    break fixed_dependencies;
                }
                let mut reports = errors
                    .into_iter()
                    .filter_map(|(module, owner, failure)| match failure {
                        Failure::Diagnostic(mut report) => {
                            if let Some(owner) = owner {
                                report.in_function(
                                    graph,
                                    owner,
                                    super::phase::BodyCheck::Inference,
                                );
                            }
                            Some(report.in_module(graph, module))
                        }
                        Failure::PendingSignature { .. } => None,
                    })
                    .collect::<Vec<_>>();
                if let Some((module, range)) = conflicting_returns {
                    let mut report = Diagnostic::at(
                        &graph.modules[module].source,
                        &range,
                        "inferred returns require one supported exact or nullable type".to_owned(),
                    );
                    report.kind = Kind::ReturnType;
                    return Err(report
                        .in_module(graph, module)
                        .with_following(reports.into_iter()));
                }
                if let Some(index) = reports
                    .iter()
                    .position(|report| !report.kind.is_representation())
                {
                    let unresolved = if graph.registered_startup {
                        let pending = self.pending.borrow();
                        signatures
                            .iter()
                            .filter_map(|(owner, signature)| {
                                pending[signature.id]
                                    .params
                                    .iter()
                                    .any(Option::is_none)
                                    .then_some(*owner)
                            })
                            .collect::<Vec<_>>()
                    } else {
                        Vec::new()
                    };
                    if !checked_bodies.is_empty() || !staged_returns.is_empty() {
                        registered::capture(
                            graph,
                            signatures,
                            definitions,
                            owners,
                            classes,
                            types,
                            self,
                            registered::Capture::Solved(Some(&checked_bodies)),
                            checked_bodies
                                .iter()
                                .copied()
                                .chain(
                                    staged_returns
                                        .into_iter()
                                        .filter(|&slot| definitions[slot].2.method.is_none()),
                                )
                                .map(|slot| (slot, local_returns.remove(&slot)))
                                .filter(resolved),
                        );
                    }
                    // Retain unresolved generators after capturing the settled bodies.
                    for owner in unresolved {
                        let _ = registered::retain_generator(graph, owner, &self.source_checks);
                    }
                    let primary = reports.remove(index);
                    return Err(primary.with_following(reports.into_iter()));
                }
                if self.infer_inherited_parameters(definitions, classes) {
                    continue;
                }
                // A failed representation leaves a Python value as the possible result.
                // A wait for another signature supplies no such evidence (in particular
                // a recursive call must still reach its ordinary recursion diagnostic).
                if !opaque_returns.is_empty() {
                    let mut pending = self.pending.borrow_mut();
                    for id in opaque_returns {
                        pending[id].returns = Some(Type::PythonObject);
                    }
                    // A caller may now continue past this result and discover exact
                    // parameter types at other calls before we fill any remaining slots.
                    continue;
                }
                if self.use_registered_arguments(graph, definitions, &mut registered) {
                    // Giving other functions Python arguments does not change a checked scalar
                    // contract that reads only its locals and the unshadowed builtins.
                    checked_before_registration = std::mem::take(&mut checked_bodies);
                    checked_before_registration.retain(|id| {
                        let (module, function, signature) = &definitions[*id];
                        !completed_calls.contains(id)
                            && signature.method.is_none()
                            && signature.returns.builtin_scalar_name().is_some()
                            && signature.params.iter().all(|p| {
                                p.ty.builtin_scalar_name().is_some() && p.default.is_none()
                            })
                            && graph.session.function_uses_fixed_names(
                                *module,
                                function,
                                true,
                                |name| module_builtin_name(&graph.modules[*module], name),
                            )
                    });
                    // Revisit actual calls now that registered initializers can supply their operands.
                    self.widened.set(true);
                    continue;
                }
                let mut reports = reports.into_iter();
                if let Some(primary) = reports.next() {
                    if primary.kind.is_object() {
                        registered::capture(
                            graph,
                            signatures,
                            definitions,
                            owners,
                            classes,
                            types,
                            self,
                            registered::Capture::Solved(None),
                            signatures
                                .values()
                                .map(|signature| (signature.id, None))
                                .filter(resolved),
                        );
                    }
                    return Err(primary.with_following(reports));
                }
                let pending = self.pending.borrow();
                // The entry's callable read already resolved a declaration. Use its
                // binder diagnostic before consulting unrelated unsolved signatures.
                let selected = entry_callable.map(|id| &definitions[id]);
                for (module, function, signature) in selected.into_iter().chain(definitions.iter())
                {
                    let Some(unresolved) =
                        pending[signature.id].unresolved(graph, *module, function, signature)
                    else {
                        continue;
                    };
                    if unresolved.kind.is_object()
                        || matches!(unresolved.kind, Kind::UnresolvedLayout(..))
                    {
                        registered::capture(
                            graph,
                            signatures,
                            definitions,
                            owners,
                            classes,
                            types,
                            self,
                            registered::Capture::Solved(None),
                            signatures
                                .values()
                                .map(|signature| (signature.id, None))
                                .filter(resolved),
                        );
                    }
                    return Err(unresolved);
                }
                unreachable!("an unresolved signature has a missing parameter or return");
            } else if checked_revision != self.revision.get()
                || completed_calls.len() != initial_nonreturning
                || classes.nonreturning_functions.len() != initial_nonreturning
            {
                method_returns.clear();
                checked_bodies.clear();
            }
        };
        for (_, _, signature) in definitions.iter_mut() {
            self.update(signature);
        }
        for signature in signatures.values_mut() {
            self.update(signature);
        }
        for methods in classes.methods.iter_mut().chain(&mut classes.setters) {
            for signature in methods.values_mut() {
                self.update(signature);
            }
        }
        let needs_registered_entry =
            !suspended_bodies.is_empty() || signatures.values().any(Signature::registered_frame);
        // A body checked before return publication has no Python operand sites. Its
        // exact ABI must still match the solved signature, and a registered graph
        // must take the ordinary placement path instead.
        for id in discovered_set_bodies {
            let signature = &definitions[id].2;
            let keep = !needs_registered_entry
                && !graph.registered_startup
                && !graph.needs_object_runtime.get()
                && local_returns.get(&id).is_some_and(|body| {
                    let PreparedBody::Fixed((body, _)) = body else {
                        return false;
                    };
                    body.function.returns == signature.returns
                        && body.function.params.len() == signature.params.len()
                        && body
                            .function
                            .params
                            .iter()
                            .zip(&signature.params)
                            .all(|(&slot, param)| body.function.locals[slot] == param.ty)
                });
            if !keep {
                local_returns.remove(&id);
            }
        }
        // Registered captures use the complete unchanged round. A failed fixed
        // initializer can keep independent checks. Its callee and wrapper facts
        // must still be published by the ordinary fixed pass.
        if needs_registered_entry != (before == self.revision.get()) {
            if let Some(dependencies) = fixed_dependencies.filter(|_| !needs_registered_entry) {
                for id in dependencies {
                    checked_bodies.remove(&id);
                }
            } else {
                checked_bodies.clear();
            }
        }
        for (id, body) in method_returns {
            if needs_registered_entry && matches!(&body, PreparedBody::Fixed(_)) {
                checked_bodies.insert(id);
            }
            match body {
                PreparedBody::Python(frame) if !graph.registered_startup => {
                    frame.retain_for_launch(graph, false);
                }
                body => {
                    local_returns.insert(id, body);
                }
            }
        }
        Ok(Solution {
            method_returns: local_returns,
            registered_parameters: registered,
            checked_bodies,
            suspended_bodies,
            needs_registered_entry,
            entry_needs_object: entry_object_revision == Some(self.revision.get()),
        })
    }
}

/// Which implementation needs to know that this source function can be called.
pub(super) enum CallEdge {
    Fixed,
    Boxed,
    /// The Python frame resolves its callable at execution.
    Live,
}

impl Checker<'_> {
    /// A method's round-start layout remains usable while its return is being inferred.
    pub(super) fn published_return(&self, signature: &Signature) -> Type {
        if let Some(inference) = self.inference
            && inference.pending.borrow()[signature.id].returns != Some(signature.returns)
        {
            self.globals.provisional_return.set(true);
        }
        signature.returns
    }

    /// A call can supply a value only if its checked body has a normal exit.
    pub(super) fn checked_call_cannot_return(
        &self,
        function: &ast::StmtFunctionDef,
        falls: bool,
    ) -> bool {
        !falls
            && !self.explicit_return
            && !self
                .graph
                .session
                .call_may_return_implicitly(self.module, function)
    }

    pub(super) fn terminating_call(&self, value: &Expr) -> bool {
        let id = match &value.kind {
            ExprKind::Call(id, _) => id,
            ExprKind::Invoke { args, .. } => match &args.site {
                ir::CallSite::SavedFunction(_, id) => id,
                _ => return false,
            },
            _ => return false,
        };
        self.nonreturning_functions.contains(id)
    }

    /// A nested eager call can end its argument scope with the same checked
    /// transfer. Its result slot is reached only if that last operand returns.
    pub(super) fn terminating_operand(&self, mut value: &Expr) -> bool {
        while let ExprKind::Scope { body, .. } = &value.kind
            && let Some(Statement::Expr(last)) = body.last()
        {
            value = last;
        }
        self.terminating_call(value)
    }

    /// Pass a storage requirement through an already checked direct argument.
    /// The list itself is never coerced between mutable representations.
    fn constrain_list_argument(
        &self,
        inference: &Inference,
        consumer: usize,
        index: usize,
        site: &ast::Expr,
        actual: Type,
        expected: Type,
    ) -> Type {
        if !inference.inferred_parameter(consumer, index) {
            return expected;
        }
        let wide = self.types.list(Type::ObjectValue);
        if actual == wide
            && expected != wide
            && self
                .types
                .element(expected)
                .is_some_and(|item| self.object_value_supported(item))
        {
            inference.widen_parameter(consumer, index, wide);
            return wide;
        }
        if expected == wide
            && actual != wide
            && self
                .types
                .element(actual)
                .is_some_and(|item| self.object_value_supported(item))
            && let ast::Expr::Call(call) = site
            && let Some(Binding::Definition(definition)) = self.resolve(&call.func)
            && let Some(producer) = self.functions.get(&definition)
        {
            if inference.pending.borrow()[producer.id].return_context_refused {
                inference.widen_parameter(consumer, index, Type::PythonObject);
                return Type::PythonObject;
            }
            inference.demand_list_return(producer.id, actual, expected, Some((consumer, index)));
        }
        expected
    }

    /// One fixed producer ABI cannot satisfy a declared narrow consumer and a
    /// separate consumer's wider mutable storage requirement. Retry placement.
    fn conflicting_list_argument(
        &self,
        inference: &Inference,
        site: &ast::Expr,
        actual: Type,
        expected: Type,
    ) -> bool {
        if actual == self.types.list(Type::ObjectValue)
            && expected != actual
            && self.types.element(expected).is_some()
            && let ast::Expr::Call(call) = site
            && let Some(Binding::Definition(definition)) = self.resolve(&call.func)
            && let Some(producer) = self.functions.get(&definition)
            && inference.pending.borrow()[producer.id].return_context == Some(actual)
        {
            inference.refuse_list_return(producer.id);
            true
        } else {
            false
        }
    }

    pub(super) fn source_call(
        &mut self,
        call: &ast::ExprCall,
        signature: &Signature,
        receiver: Option<Expr>,
        skip: usize,
    ) -> Result<Expr, String> {
        if self.classes.cached.contains_key(&signature.id) && !call.arguments.keywords.is_empty() {
            return Err(error(
                self.source,
                call,
                "cached calls currently require a positional key",
            ));
        }
        let mut scope = calls::ArgumentScope::default();
        let direct = self.registered.is_none() && receiver.is_none() && skip == 0;
        let args = self.source_arguments(
            call,
            signature,
            receiver,
            skip,
            (calls::starred(call)
                || calls::named_mapping(call, &signature.params[skip..]).is_some()
                || signature.params.iter().any(|p| {
                    matches!(p.kind, ParameterKind::Variadic | ParameterKind::KeywordRest)
                }))
            .then_some(&mut scope),
            direct,
        )?;
        let args = match args {
            ControlFlow::Continue(args) => args,
            ControlFlow::Break(stop) => return Ok(scope.finish(stop)),
        };
        let ty = self.enter_source_call(call, signature, CallEdge::Fixed)?;
        // A successfully bound source call can enter the definition's registered body.
        self.globals
            .missing_bindings
            .borrow_mut()
            .remove(&signature.id);
        Ok(scope.finish(self.call_value(signature, ty, args)))
    }

    pub(super) fn bind_source(
        &mut self,
        call: &ast::ExprCall,
        signature: &Signature,
        receiver: Option<Expr>,
        skip: usize,
        scope: Option<&mut calls::ArgumentScope>,
    ) -> Result<(Type, ir::CallArgs), String> {
        let args = self
            .source_arguments(call, signature, receiver, skip, scope, false)?
            .continue_value()
            .expect("this caller owns the loaded receiver");
        Ok((
            self.enter_source_call(call, signature, CallEdge::Fixed)?,
            args,
        ))
    }

    /// The caller needs the return layout only after evaluating its arguments.
    pub(super) fn enter_source_call(
        &mut self,
        call: &ast::ExprCall,
        signature: &Signature,
        edge: CallEdge,
    ) -> Result<Type, String> {
        let returned = match self.inference {
            Some(inference) => {
                inference.lookup(signature, SignatureUse::Return, self, call.range())
            }
            None => Ok(signature.returns),
        };
        if returned.is_ok() {
            self.apply_caller_frame_stores(signature, call);
        }
        match edge {
            CallEdge::Live => {}
            CallEdge::Boxed
                if returned.is_ok()
                    && let Some(calls) = &mut self.boxed_calls =>
            {
                calls.insert(signature.id);
            }
            _ => {
                self.calls.insert(signature.id);
            }
        }
        returned
    }

    fn source_arguments(
        &mut self,
        call: &ast::ExprCall,
        signature: &Signature,
        receiver: Option<Expr>,
        skip: usize,
        scope: Option<&mut calls::ArgumentScope>,
        direct: bool,
    ) -> Result<ControlFlow<Expr, ir::CallArgs>, String> {
        // CPython's accelerated stat functions take an unsigned mode_t through
        // the index protocol. A boolean argument must reach their integer body
        // as 0 or 1; ordinary annotated Python functions retain their own types.
        let binding = self.resolve(&call.func);
        let stat_mode = matches!(binding, Some(Binding::Definition(id))
            if self.graph.modules[id.module].is_bundled("stat"));
        // Only a bound input reaches dedent's runtime error. Malformed calls
        // retain the ordinary argument checker and its first source diagnostic.
        let dedent = matches!(binding, Some(Binding::Definition(id)) if self.graph.is_textwrap_dedent(id))
            && call.arguments.len() == 1
            && !calls::has_unpacking(call);
        let selector_wait = signature.method.is_some_and(|method| {
            let class = &self.classes.declarations.definitions[method.class];
            self.graph.modules[class.definition.module].is_bundled("selectors")
                && class.methods[method.index].name.as_str() == "select"
        });
        let inference = self.inference;
        let can_stop = direct && scope.is_none() && !self.nonreturning_functions.is_empty();
        self.bind_eager_arguments(
            call,
            calls::CallParameters::Source { signature, skip },
            receiver,
            scope,
            can_stop.then_some(signature.returns),
            &mut |checker, index, argument| {
                let index = index + skip;
                let parameter = &signature.params[index];
                let declaration = signature.parameter_declaration(index);
                let direct_value =
                    |checker: &Checker<'_>, value: Expr, expected, site: &ast::Expr| {
                        if can_stop && checker.terminating_operand(&value) {
                            Ok(value)
                        } else {
                            checker.direct_value(value, site, expected, parameter.sign_only)
                        }
                    };
                let timeout = selector_wait && parameter.name == "timeout";
                if dedent || timeout || (stat_mode && parameter.ty == Type::Int) {
                    let node = argument.site();
                    let value = argument.value(checker)?;
                    if can_stop && checker.terminating_operand(&value) {
                        return Ok(value);
                    }
                    if dedent {
                        return checker.dedent_text(value, node);
                    }
                    if timeout {
                        let seconds = checker.selector_seconds(value);
                        return checker.coerce(seconds, parameter.ty, node);
                    }
                    if value.ty == Type::Bool {
                        return Ok(Expr {
                            ty: Type::Int,
                            kind: ExprKind::Builtin(Builtin::Int(None), vec![value]),
                        });
                    }
                    return checker.direct_value(value, node, parameter.ty, parameter.sign_only);
                }
                let Some(inference) = inference else {
                    return argument.check_with(
                        checker,
                        parameter.ty,
                        parameter.sign_only,
                        declaration,
                        direct_value,
                    );
                };
                let node = argument.site();
                let value = if let Some(ty) = inference.parameter(signature.id, index) {
                    if inference.none_default(signature.id, index) {
                        let value = argument.value(checker)?;
                        let expected = inference.widen_none_default(
                            signature.id,
                            index,
                            value.ty,
                            checker.types,
                        );
                        return checker.coerce(value, expected, node);
                    }
                    let types = checker.types;
                    let inferred = inference.pending.borrow()[signature.id].inferred_params[index];
                    // Absence and a present value share the same optional ABI in either call order.
                    let nullable = |prior, actual| {
                        if inferred
                            && (prior == Type::None || actual == Type::None)
                            && prior != Type::PythonObject
                            && actual != Type::PythonObject
                        {
                            types.join(prior, actual).filter(|&layout| layout != prior)
                        } else {
                            None
                        }
                    };
                    // Normalized inline keywords carry the value range; source names have their own.
                    if closed_calls::keyword_field(node)
                        || call.arguments.keywords.iter().any(|kw| {
                            kw.arg.as_ref().is_some_and(|name| {
                                name.range() == node.range() && kw.value.range() == node.range()
                            })
                        })
                    {
                        let supplied = argument.value(checker)?;
                        let ty = inference.document_parameter(
                            signature,
                            checker.classes,
                            index,
                            ty,
                            supplied.ty,
                        );
                        let ty = checker.constrain_list_argument(
                            inference,
                            signature.id,
                            index,
                            node,
                            supplied.ty,
                            ty,
                        );
                        if checker.conflicting_list_argument(inference, node, supplied.ty, ty) {
                            return Err(checker.classified_error(
                                Kind::ScopedObject, node, "the inferred list return requires distinct consumer storage",
                            ));
                        }
                        let optional = nullable(ty, supplied.ty).or_else(|| {
                            (supplied.ty == Type::None
                                && ty != Type::None
                                && types.payload(ty).is_none())
                            .then(|| types.optional(ty))
                        });
                        if let Some(expected) = optional {
                            inference.widen_parameter(signature.id, index, expected);
                            return checker.coerce(supplied, expected, node);
                        }
                        if checker.types.payload(supplied.ty) == Some(ty) {
                            inference.widen_parameter(signature.id, index, supplied.ty);
                            return Ok(supplied);
                        }
                        return checker.direct_value(supplied, node, ty, parameter.sign_only);
                    }
                    // An inferred argument can supply the optional layout for a prior
                    // None or its payload, or the union of two supported text layouts.
                    if inferred
                        && (matches!(ty, Type::None | Type::Str | Type::Bytes)
                            || types.element(ty) == Some(Type::Str))
                    {
                        let lines = types.list(Type::Str);
                        let wider = |other| {
                            if ty == other {
                                return None;
                            }
                            if let Some(layout) = nullable(ty, other) {
                                return Some(layout);
                            }
                            if other == types.list(Type::ObjectValue) && types.element(ty).is_some()
                            {
                                return Some(other);
                            }
                            if types
                                .payload(other)
                                .is_some_and(|payload| ty == Type::None || ty == payload)
                            {
                                return Some(other);
                            }
                            types.union(&[ty, other]).filter(|&union| {
                                types.members(union).is_some_and(|members| {
                                    members == [Type::Str, Type::Bytes]
                                        || members == [Type::Str, lines]
                                })
                            })
                        };
                        let value = match argument {
                            calls::Supplied::Source(node) => {
                                checker.expr_expected_if(node, ty, |_, actual| {
                                    wider(actual).is_some()
                                })?
                            }
                            calls::Supplied::Field(value, _) => value,
                        };
                        let expected = if let Some(union) = wider(value.ty) {
                            inference.widen_parameter(signature.id, index, union);
                            union
                        } else {
                            ty
                        };
                        return checker.direct_value(value, node, expected, parameter.sign_only);
                    }
                    if matches!(ty, Type::Class(class) if checker.classes.parsers.namespace(class))
                    {
                        let value = argument.value(checker)?;
                        let expected = if value.ty == ty {
                            ty
                        } else if let Some(parent) =
                            checker.classes.parsers.common_namespace(ty, value.ty)
                        {
                            if parent != ty {
                                inference.widen_namespace(
                                    signature.id,
                                    index,
                                    parent,
                                    &checker.classes.parsers,
                                );
                            }
                            parent
                        } else {
                            ty
                        };
                        checker.coerce(value, expected, node)?
                    } else {
                        let ty =
                            inference.sequence_context(signature.id, index, ty, argument.site());
                        argument.check_with(
                            checker,
                            ty,
                            parameter.sign_only,
                            declaration,
                            |checker, value, expected, site| {
                                if let Some(member) = inference.sequence_member(
                                    signature.id,
                                    index,
                                    value.ty,
                                    checker.types,
                                ) {
                                    return inference.bind_sequence(
                                        signature.id,
                                        index,
                                        member,
                                        checker,
                                        value,
                                        site,
                                    );
                                }
                                let expected = inference.document_parameter(
                                    signature,
                                    checker.classes,
                                    index,
                                    expected,
                                    value.ty,
                                );
                                let expected = checker.constrain_list_argument(
                                    inference,
                                    signature.id,
                                    index,
                                    site,
                                    value.ty,
                                    expected,
                                );
                                if checker.conflicting_list_argument(inference, site, value.ty, expected) {
                                    return Err(checker.classified_error(
                                        Kind::ScopedObject, site, "the inferred list return requires distinct consumer storage",
                                    ));
                                }
                                if let Some(layout) = nullable(expected, value.ty) {
                                    inference.widen_parameter(signature.id, index, layout);
                                    return direct_value(checker, value, layout, site);
                                }
                                // The bound operand supplies storage evidence for this parameter.
                                if value.ty == Type::PythonObject
                                    && expected != Type::PythonObject
                                    && inference.pending.borrow()[signature.id].inferred_params
                                        [index]
                                {
                                    inference.widen_parameter(
                                        signature.id,
                                        index,
                                        Type::PythonObject,
                                    );
                                    Ok(value)
                                } else {
                                    direct_value(checker, value, expected, site)
                                }
                            },
                        )?
                    }
                } else {
                    let value = argument.value(checker)?;
                    checker.keep_module_list_layout(&value);
                    if !can_stop || !checker.terminating_operand(&value) {
                        inference.learn_parameter(signature.id, index, value.ty);
                    }
                    value
                };
                Ok(value)
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_list_joins_direct_conditional_and_loop_constructors() {
        let source = r#"class Base: pass
class Left(Base): pass
class Right(Base): pass
def run(flag: bool, items: list[int]) -> None:
    values = []
    if flag: values.append(Left())
    if flag: values.append(Right())
    if flag: values.append(Left())
    if flag: values.append(Right())
    if flag: values.append(Left())
    if flag: values.append(Right())
    for item in items: values.append(Left())
    for item in items: values.append(Right())
    for item in items: values.append(Left())
    for item in items: values.append(Right())
    for item in items: values.append(Left())
    for item in items: values.append(Right())
    if not values: values = [Left()]
run(True, [1])
"#;
        crate::check(source).unwrap();
    }

    #[test]
    fn fresh_list_cannot_join_past_another_reference() {
        let classes = "class Base: pass\nclass Left(Base): pass\nclass Right(Base): pass\n";
        for boundary in [
            "    consume([values])\n",
            "    alias = values\n",
            "    alias = values\n    alias.append(Right())\n",
            "    values = []\n",
            "    if flag: values = []\n",
            "    if not values: pass\n",
            "    print(0)\n",
        ] {
            let source = format!(
                "{classes}def consume(groups: list[list[Left]]) -> None: pass\ndef run(flag: bool) -> None:\n    values = []\n    values.append(Left())\n{boundary}    values.append(Right())\nrun(True)\n"
            );
            let error = crate::check(&source).unwrap_err();
            assert!(error.contains("expected main.Left, got main.Right"), "{error}");
        }
        let monomorphic = format!(
            "{classes}def run() -> None:\n    values = []\n    print(0)\n    values.append(Left())\n    values.append(Left())\nrun()\n"
        );
        crate::check(&monomorphic).unwrap();
    }

    #[test]
    fn list_contracts_and_nonconstructor_insertions_are_not_widened() {
        let classes = "class Base: pass\nclass Left(Base): pass\nclass Right(Base): pass\nclass Other: pass\ndef make() -> Right: return Right()\n";
        for (declaration, value) in [
            ("values: list[Left] = []", "Right()"),
            ("values = []", "make()"),
            ("values = []", "Other()"),
        ] {
            let source = format!(
                "{classes}def run() -> None:\n    {declaration}\n    values.append(Left())\n    values.append({value})\nrun()\n"
            );
            assert!(crate::check(&source).is_err());
        }
        let incompatible = format!(
            "{classes}def run() -> None:\n    values = []\n    values.append(Left())\n    values.append(Right())\n    values.append(make())\nrun()\n"
        );
        let error = crate::check(&incompatible).unwrap_err();
        assert!(error.contains("expected main.Left, got main.Right"), "{error}");
        crate::check(
            "def run() -> None:\n    values = []\n    alias = values\n    values.append(1)\n    seen = set()\n    seen.add(1)\nrun()\n",
        )
        .unwrap();
    }

    #[test]
    fn a_provisional_method_result_preserves_the_first_argument_supplier() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("main.py");
        for initializer in ["list(map(Item().read, [0]))[0]", "copy.copy(Item())"] {
            let source = format!(
                r#"import copy

class Item:
    def __copy__(self): return self
    def read(self, n=0): return self
    def __getitem__(self, key) -> bool: return True

SOURCE = {initializer}
VALUE = SOURCE

def first():
    def get(): return [VALUE]
    values = get()
    (values[0] or [False])[0]
    values += values

def rival(item: Item): return item[later()]
def later(): return "key"

first()
rival(Item())
"#
            );
            std::fs::write(&file, source).unwrap();
            let error = crate::check_file(&file, None).unwrap_err();
            assert!(
                error.contains(":17:31: Imre: expected int, got str"),
                "{error}"
            );
        }
    }

    #[test]
    fn a_saved_method_uses_its_later_inferred_return() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("main.py");
        let source = r"from typing import Callable
import logging
class Reader:
    def read(self, n=0): return later()
class Saver:
    def save(self, r: Reader):
        saved = r.read
        accept(saved)
def accept(fn: Callable[[int], None]) -> None: pass
def later(): return payload()
def payload(): return 9
Saver().save(Reader())
print(Reader().read())
";
        std::fs::write(&file, source).unwrap();
        let error = crate::check_file(&file, None).unwrap_err();
        assert!(
            error.contains("8:16: Imre: expected Callable[[int], None], got Callable[[int], int]"),
            "{error}"
        );
    }

    #[test]
    fn a_late_bound_method_keeps_the_other_parameter_contract() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("main.py");
        let source = concat!(
            "def invoke(receiver, n: int): return receiver.render(n)\n",
            "class Reader:\n    def render(self, n: int) -> int: return n\n",
            "try: invoke(0, 1)\nexcept AttributeError: pass\n",
            "print(invoke(Reader(), 2))\n",
        );
        std::fs::write(&file, source).unwrap();
        crate::check_file(&file, None).unwrap();
        std::fs::write(&file, source.replace("Reader(), 2", "Reader(), 'bad'")).unwrap();
        let error = crate::check_file(&file, None).unwrap_err();
        assert!(error.contains("expected int, got str"), "{error}");
    }

    #[test]
    fn a_none_default_accepts_a_guarded_record() {
        let mut graph = SourceGraph::single(concat!(
            "class Parcel:\n    def __init__(self, n: int): self.n = n\n",
            "class Scale:\n    def weight(self, value=None) -> int:\n",
            "        if value is not None: return value.n\n        return -1\n",
            "scale = Scale()\nprint(scale.weight(), scale.weight(Parcel(7)), scale.weight(value=None))\n",
        ))
        .unwrap();
        assert!(matches!(
            crate::checked_lower(&mut graph).unwrap(),
            crate::CheckedProgram::Native(_)
        ));
    }

    #[test]
    fn an_inferred_set_body_is_not_lowered_again_after_return_publication() {
        let text = concat!(
            "def gather(text):\n",
            "    values = set()\n",
            "    for char in text:\n",
            "        values.add(char)\n",
            "    return sorted(values)\n",
            "print(gather('aba'))\n",
        );
        let mut graph = SourceGraph::single(text).unwrap();
        let (checked, lowered) =
            caller_cache::observe_lowerings(|| crate::checked_lower(&mut graph).unwrap());
        assert!(!lowered.iter().any(|name| name == "gather"), "{lowered:?}");
        let crate::CheckedProgram::Native(fixed) = checked else {
            panic!("native gather");
        };
        let mut constructor = false;
        let mut insertion = false;
        ir::walk_expressions(
            &fixed.ir.0.functions[0].body,
            &mut |value| match value.kind {
                ExprKind::Set {
                    element: Type::Str, ..
                } => constructor = true,
                ExprKind::Builtin(Builtin::SetAdd, _) => insertion = true,
                _ => {}
            },
        );
        assert!(constructor && insertion);
    }

    #[test]
    fn an_imported_native_leaf_keeps_its_settled_return() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/inference/imported");
        let mut graph = SourceGraph::load(&root.join("main.py"), Some(&root), &[]).unwrap();
        let (program, lowered) =
            caller_cache::observe_lowerings(|| crate::checked_lower(&mut graph).unwrap());
        assert!(matches!(program, crate::CheckedProgram::Native(_)));
        assert!(!lowered.iter().any(|name| name == "increment"));
    }

    #[test]
    fn named_scalar_leaf_keeps_its_settled_return() {
        let text = concat!(
            "def scale(value):\n    return increment(value) * 2\n",
            "def increment(value):\n    return value + 1\n",
            "print(scale(4))\n",
        );
        let mut graph = SourceGraph::single(text).unwrap();
        graph.registered_startup = true;
        let (_, lowered) =
            caller_cache::observe_lowerings(|| crate::checked_lower(&mut graph).unwrap());
        assert_eq!(lowered, ["scale"]);
        let start = TextSize::try_from(text.find("def increment").unwrap()).unwrap();
        let bodies = graph.registered_functions();
        let leaf = &bodies[&graph.entry][&start];
        assert_eq!(leaf.function.returns, Type::Int);
        assert_eq!(&text[leaf.fixed_sites[0]], "return value + 1");
    }

    #[test]
    fn named_startup_keeps_each_refused_function_operand() {
        let text = concat!(
            "def unused() -> int:\n    values: list[int] = [1]\n    values.append(True)\n    return 1\n",
            "def called() -> int:\n    values: list[int] = [2]\n    values.append('two')\n    return 2\n",
            "print(called())\n",
        );
        let mut graph = SourceGraph::single(text).unwrap();
        graph.registered_startup = true;
        let report = super::super::lower_with_caller_cache(&mut graph, None)
            .err()
            .unwrap();
        let suite = graph.modules[graph.entry].parsed.suite();
        assert_eq!(
            report
                .function_reports()
                .map(|(site, _)| site)
                .collect::<Vec<_>>(),
            [
                (graph.entry, suite[0].range()),
                (graph.entry, suite[1].range())
            ],
        );
    }

    #[test]
    fn an_independent_opaque_result_does_not_hide_a_checked_operand() {
        let mut graph = SourceGraph::single(concat!(
            "def rejected(arg):\n",
            "    values: list[int] = [1]\n",
            "    values.append(True)\n",
            "    return arg\n",
            "def distinct():\n",
            "    return set()\n",
        ))
        .unwrap();
        let diagnostic = super::super::lower_with_caller_cache(&mut graph, None)
            .err()
            .unwrap();
        assert_eq!(diagnostic.kind, Kind::ExactType);
        assert!(diagnostic.to_string().contains(":3:19:"), "{diagnostic}");
    }
    #[test]
    fn a_procedure_waits_for_its_call_before_binding_the_next_argument() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("main.py");
        let source = concat!(
            "import os\n\n",
            "def first():\n    blocked()\n    choose(later_int())\n\n",
            "def rival():\n    choose(later_str())\n\n",
            "def choose(value) -> None:\n    print(value)\n\n",
            "def later_int():\n    return 7\n\n",
            "def later_str():\n    return \"later\"\n\n",
            "def blocked():\n    os.path.abspath(1)\n\n",
            "first()\nrival()\n",
        );
        std::fs::write(&file, source).unwrap();
        let error = crate::check_file(&file, None).unwrap_err();
        assert!(
            error.contains(":5:12: Imre: expected str, got int"),
            "{error}"
        );
    }

    #[test]
    fn a_discarded_pending_call_does_not_hide_a_checked_operand() {
        let mut graph = SourceGraph::single(concat!(
            "def cycle(): return cycle()\n",
            "def rejected():\n",
            "    cycle()\n",
            "    values: list[int] = [1]\n",
            "    values.append(True)\n",
        ))
        .unwrap();
        let error = super::super::lower_with_caller_cache(&mut graph, None)
            .err()
            .unwrap();
        assert_eq!(error.kind, Kind::ExactType);
        assert!(error.to_string().contains(":5:19:"));
    }

    #[test]
    fn a_return_replaced_by_finally_does_not_supply_a_forwarded_argument() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("main.py");
        let source = concat!(
            "def choose():\n",
            "    try: return 'wrong'\n",
            "    finally: raise SystemExit(13)\n",
            "def take(value: int) -> int: return value\n",
            "def relay(arg): return take(arg)\n",
            "print(relay(choose()))\n",
        );
        std::fs::write(&file, source).unwrap();
        crate::check_file(&file, None).unwrap();
        let typed = source.replace(
            "print(relay(choose()))",
            "def caller() -> int: return take(choose())\nprint(caller())",
        );
        std::fs::write(&file, typed).unwrap();
        crate::check_file(&file, None).unwrap();
        let mixed = source.replace(
            "def relay(arg): return take(arg)\nprint(relay(choose()))",
            "def select(flag: bool):\n    if flag: return choose()\n    return 7\nprint(select(False))",
        );
        std::fs::write(&file, &mixed).unwrap();
        crate::check_file(&file, None).unwrap();
        std::fs::write(&file, mixed.replace("raise SystemExit(13)", "pass")).unwrap();
        let error = crate::check_file(&file, None).unwrap_err();
        assert!(
            error.contains("inferred returns require one supported exact or nullable type"),
            "{error}"
        );
        let stored = source.replace(
            "def relay(arg): return take(arg)\nprint(relay(choose()))",
            "value: int = take(choose())\nprint(value)",
        );
        let reannotated = stored.replace(
            "value: int = take(choose())\nprint(value)",
            "def outer() -> int:\n    value: str = 'old'\n    value: int = take(choose())\n    return value\nprint(outer())",
        );
        let item = stored.replace(
            "value: int = take(choose())\nprint(value)",
            "def key() -> int: return 0\nvalues = [0]\nvalues[key()] = take(choose())\nprint(values[0])",
        );
        let fresh = stored.replace("value: int =", "value =");
        for program in [&stored, &reannotated, &item, &fresh] {
            std::fs::write(&file, program).unwrap();
            let mut graph = crate::modules::SourceGraph::load(&file, None, &[]).unwrap();
            assert!(matches!(
                crate::checked_lower(&mut graph).unwrap(),
                crate::CheckedProgram::Native(_)
            ));
        }
        std::fs::write(&file, item.replace("raise SystemExit(13)", "pass")).unwrap();
        let error = crate::check_file(&file, None).unwrap_err();
        assert!(
            error.contains(":7:22: Imre: expected int, got str"),
            "{error}"
        );
        std::fs::write(&file, stored.replace("raise SystemExit(13)", "pass")).unwrap();
        let error = crate::check_file(&file, None).unwrap_err();
        assert!(
            error.contains(":5:19: Imre: expected int, got str"),
            "{error}"
        );
        std::fs::write(&file, source.replace("raise SystemExit(13)", "pass")).unwrap();
        let error = crate::check_file(&file, None).unwrap_err();
        assert!(error.contains("expected int, got str"), "{error}");
    }

    #[test]
    fn a_terminating_argument_supplies_neither_its_value_nor_a_later_argument() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("main.py");
        let source = concat!(
            "def first():\n",
            "    print('first')\n",
            "    raise SystemExit(13)\n",
            "def text(): return 'wrong'\n",
            "def pair(x: int, y: int): return y\n",
            "print(pair(first(), text()))\n",
        );
        std::fs::write(&file, source).unwrap();
        crate::check_file(&file, None).unwrap();
        for (returned, diagnostic) in [
            ("None", ":6:12: Imre: expected int, got None"),
            ("37", ":6:21: Imre: expected int, got str"),
        ] {
            std::fs::write(
                &file,
                source.replace("raise SystemExit(13)", &format!("return {returned}")),
            )
            .unwrap();
            let error = crate::check_file(&file, None).unwrap_err();
            assert!(error.contains(diagnostic), "{error}");
        }
    }

    #[test]
    fn inferred_returns_keep_precise_values_and_can_select_an_object() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("main.py");
        std::fs::write(
            &file,
            concat!(
                "def choose(mode: int):\n",
                "    if mode == 1: return 5\n",
                "    if mode == 2: return 'known'\n",
                "    return set()\n",
                "print(choose(1), choose(2), choose(3))\n",
            ),
        )
        .unwrap();
        crate::check_file(&file, None).unwrap();

        std::fs::write(
            &file,
            concat!(
                "def label():\n",
                "    set()\n",
                "    return 'known'\n",
                "def take(n: int): return n\n",
                "print(take(label()))\n",
            ),
        )
        .unwrap();
        let error = crate::check_file(&file, None).unwrap_err();
        assert!(
            error.contains("main.py:5:12: Imre: expected int, got str"),
            "{error}"
        );
    }

    #[test]
    fn a_typed_callback_reports_the_inferred_return_at_the_argument() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("main.py");
        let source = concat!(
            "from typing import Callable\n",
            "import argparse\n",
            "def invoke(f: Callable[[list[str]], str]) -> str:\n    return f([])\n",
            "def exercise() -> None:\n    print(invoke(produce))\n",
            "def produce(x: list[str]) -> argparse.Namespace:\n",
            "    p = argparse.ArgumentParser()\n",
            "    p.add_argument('--size', default='4')\n",
            "    return p.parse_args(x)\n",
            "exercise()\n",
        );
        std::fs::write(&file, source).unwrap();
        let error = crate::check_file(&file, None).unwrap_err();
        assert!(
            error.contains("main.py:6:18: Imre: expected Callable[[list[str]], str], got Callable[[list[str]], main.Namespace]"),
            "{error}"
        );
    }

    #[test]
    fn settling_a_return_preserves_the_following_generator_frame() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("main.py");
        let source = concat!(
            "def leaf(): return 42\n",
            "def sequence(x: int) -> object:\n",
            "    yield x\n",
            "print(next(sequence(leaf())))\n",
        );
        std::fs::write(&file, source).unwrap();
        let error = crate::check_file(&file, None).unwrap_err();
        assert!(
            error.contains("main.py:4:7: Imre: next requires a supported iterator"),
            "{error}"
        );
    }
}
