//! Resolve function signatures to native layouts or registered Python argument storage.

use std::cell::{Cell, RefCell};
use std::ops::ControlFlow;

use ruff_python_ast::visitor::{Visitor, walk_stmt};

use super::*;

/// An element inserted into a local container and its direct iteration source, when known.
struct Insertion<'a> {
    value: &'a ast::Expr,
    for_loop: Option<&'a ast::StmtFor>,
}

/// A direct element insertion in this lexical scope can supply a container layout hint.
/// The ordinary checker still checks its operand and every other use of the list.
fn first_insertion<'ast>(
    name: &str,
    method: &str,
    statements: &'ast [ast::Stmt],
) -> Option<Insertion<'ast>> {
    struct Appends<'name, 'ast> {
        name: &'name str,
        method: &'name str,
        insertion: Option<Insertion<'ast>>,
        for_loop: Option<&'ast ast::StmtFor>,
    }

    impl<'ast> Visitor<'ast> for Appends<'_, 'ast> {
        fn visit_stmt(&mut self, stmt: &'ast ast::Stmt) {
            if self.insertion.is_some()
                || matches!(stmt, ast::Stmt::FunctionDef(_) | ast::Stmt::ClassDef(_))
            {
                return;
            }
            if let ast::Stmt::For(loop_) = stmt {
                let enclosing = self.for_loop.replace(loop_);
                self.visit_body(&loop_.body);
                self.for_loop = enclosing;
                self.visit_body(&loop_.orelse);
                return;
            }
            if let ast::Stmt::Expr(stmt) = stmt
                && let ast::Expr::Call(call) = &*stmt.value
                && let ast::Expr::Attribute(method) = &*call.func
                && method.attr.as_str() == self.method
                && matches!(&*method.value, ast::Expr::Name(name) if name.id == self.name)
                && let [value] = call.arguments.args.as_ref()
                && call.arguments.keywords.is_empty()
            {
                self.insertion = Some(Insertion {
                    value,
                    for_loop: self.for_loop,
                });
            } else {
                walk_stmt(self, stmt);
            }
        }

        fn visit_expr(&mut self, _: &'ast ast::Expr) {}
    }

    let mut appends = Appends {
        name,
        method,
        insertion: None,
        for_loop: None,
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
        let insertion = first_insertion(name, if set { "add" } else { "append" }, later)?;
        if let ast::Expr::Call(call) = insertion.value
            && let Some(class) = self.class_reference(&call.func)
        {
            return Some(Type::Class(class));
        }
        if let Some(ty) = self.graph.session.scalar_type(self.module, insertion.value) {
            return Some(ty);
        }
        if !set || self.graph.layout_hint.get() == crate::modules::LayoutHint::Disabled {
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
        if for_loop.is_async
            || insertion.value.as_name_expr()?.id != item.id
            || narrowing::assigned_names(&for_loop.body).contains(item.id.as_str())
            || narrowing::assigned_names(later).contains(source.id.as_str())
        {
            return None;
        }
        // Later uses may need heterogeneous storage. A failed native attempt
        // must recover the ordinary diagnostics and object fallback for set().
        self.graph
            .layout_hint
            .set(crate::modules::LayoutHint::Applied);
        Some(element)
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
            .classified(Kind::UnresolvedParameter(module, function.start()))
        } else {
            Diagnostic::at(
                source,
                function,
                "cannot infer function return type; add an annotation; recursive calls remain unsupported".to_owned(),
            )
        };
        Some(report.in_module(graph, module))
    }
}

/// Checked calls and function bodies supply missing parameter and return types.
/// Once signatures settle, the final body check produces the operations for local callbacks.
#[derive(Default)]
pub(super) struct Inference {
    pending: RefCell<Vec<Pending>>,
    document_fields: RefCell<HashSet<(usize, usize)>>,
    /// Source contract checks shared by discovery, recovery and final body lowering.
    pub(super) source_checks: registered::SourceChecks,
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
            discovery: if returns.is_some() {
                ReturnDiscovery::Declared
            } else {
                ReturnDiscovery::Body
            },
        });
        id
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

    pub(super) fn parameter(&self, id: usize, index: usize) -> Option<Type> {
        self.pending.borrow()[id].params[index]
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
            Type::Str | Type::Bytes | Type::Bool | Type::Int | Type::Float
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
        module: ModuleId,
        range: TextRange,
    ) -> Result<Type, &'static str> {
        let needs_parameters = matches!(request, SignatureUse::Callable { .. });
        if let SignatureUse::Callable {
            skip,
            expected: Some((params, returns)),
        } = request
        {
            if params.len() + skip != signature.params.len() {
                return Err(Diagnostic::plain(
                    "function value does not match its Callable parameter count",
                )
                .into());
            }
            for (index, ty) in params.into_iter().enumerate() {
                self.learn_parameter(signature.id, index + skip, ty);
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
                module,
                range,
            })
    }

    pub(super) fn callable_signature(
        &self,
        signature: &mut Signature,
        skip: usize,
        expected: Option<(Vec<Type>, Type)>,
        checker: &Checker<'_>,
        site: &impl Ranged,
    ) -> Result<(), String> {
        self.lookup(
            signature,
            SignatureUse::Callable { skip, expected },
            checker.module,
            site.range(),
        )
        .map_err(|failure| failure.at(checker.source, site))?;
        self.update(signature);
        Ok(())
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
        let mut checked_before_registration = HashSet::new();
        // Without input or layout inference, resolved return slots stay resolved.
        let mut first_open_return = (!self.widened.get()
            && self
                .pending
                .borrow()
                .iter()
                .all(|p| !p.inferred_params.contains(&true)))
            .then_some(0);
        let mut before = self.revision.get();
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
                first_open_return = None;
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
                        // Methods and generators can still supply a prepared frame.
                        // Leave ordinary physical functions to their consuming body check.
                        if *slot == signatures.len()
                            && signature.physical_frame
                            && signature.method.is_none()
                            && !graph.session.is_generator_definition(*module, function)
                        {
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
                    || suspended_bodies.contains(&signature.id)
                {
                    checked_bodies.insert(signature.id);
                    continue;
                }
                // The round already updated each signature. Earlier calls can refine its inputs.
                let mut updated;
                let signature = if self.revision.get() == before {
                    signature
                } else {
                    updated = signature.clone();
                    self.update(&mut updated);
                    &updated
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
                    checker.registered = Some(graph.modules[*module].registered_values());
                }
                checker.inference = Some(self);
                checker.discover_document_return = document_return;
                checker.inferred_returns = discover_returns.then(Vec::new);
                let body_revision = self.revision.get();
                // A physical method reached after the final signature change can keep the
                // return it is about to check. Discovery still observes all calls and stores.
                let keep_return = body_revision != before
                    && !self.widened.get()
                    && !discover_returns
                    && !signature.registered_frame()
                    && matches!(function.body.as_slice(), [ast::Stmt::Return(_)])
                    && !checker.fault_enabled()
                    && registered::physical_method(signature, function, classes).is_some();
                checker.placement_only = !keep_return;
                let method_storage = keep_return.then(|| (helpers.len(), globals.types().len()));
                let mut return_body = keep_return.then(|| {
                    checker.frame = LexicalFrame::Named(function.name.id.clone());
                    checker.fixed_sites = Some(fixed_body_source::SourceSites::new(true));
                    (Vec::new(), Vec::new())
                });
                let mut checked_return = false;
                let mut implicit_none = false;
                let mut superseded_returns = false;
                let mut checked_call_cannot_return = false;
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
                        &checker, function, signature, self, None,
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
                let place_suspended = |checker: &Checker<'_>| -> Result<_, String> {
                    Ok(registered::prepare_checked_definition(
                        checker, function, signature, self, None,
                    )?
                    .map(|(_, placement)| placement.place(graph, &self.source_checks)))
                };
                let mut placement = (physical
                    && function.is_async
                    && signature.method.is_none()
                    && graph.modules[*module].is_program_source()
                    && graph
                        .session
                        .coroutine_uses_async_iteration(*module, function))
                .then(|| place_suspended(&checker));
                let mut retained = matches!(placement, Some(Ok(Some(_))));
                let result = (|| {
                    if retained {
                        return Ok(());
                    }
                    checker.bind_function_signature(function, signature, |slot| {
                        if let Some((params, _)) = &mut return_body {
                            params.push(slot);
                        }
                    })?;
                    checker.collect_local_definitions(&function.body)?;
                    let falls_through = checker
                        .lower_block(&function.body, return_body.as_mut().map(|(_, body)| body))?;
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
                let mut result = checker.finish_region(result);
                if let Err(diagnostic) = &mut result {
                    // A completed return check can still require Python operations. Only the
                    // registered frame may own that body, after its ordinary source check.
                    if checked_return
                        && diagnostic.is_representation()
                        && graph.modules[*module].is_program_source()
                    {
                        staged_returns.push(signature.id);
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
                                Ok(placement) => placement.is_some(),
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
                        && {
                            let pending = self.pending.borrow();
                            pending[*callee].returns.is_none()
                                && !pending[signature.id].inferred_params.contains(&true)
                                && !pending[*callee].inferred_params.contains(&true)
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
                            method_returns.insert(signature.id, Err(std::mem::take(sites)));
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
                        && signature.params.is_empty()
                        && !signature.registered_frame()
                        && self.pending.borrow()[signature.id].returns == Some(Type::None)
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
                    } else if retrying {
                        self.pending.borrow_mut()[signature.id].discovery =
                            ReturnDiscovery::Checked;
                    }
                    if let Some((params, body)) = return_body
                        && checked_return && !checked_call_cannot_return && body_revision == revision
                        && checker.calls.is_empty() && checker.class_reads.borrow().is_empty()
                        && checker.unrepresented.borrow().is_none()
                        && method_storage == Some((helpers.len(), globals.types().len()))
                        && let Some(source) = checker.fixed_body_source(function, signature, function.body.len(), &body)
                        // Module storage and method targets belong to the graph that selected them.
                        // A written global callee will be looked up again by the Python frame.
                        && source.attributes.values().all(|attribute| match attribute {
                            FixedAttribute::Operation { operation, .. } => matches!(**operation, Builtin::RegisteredAttribute { .. }),
                            FixedAttribute::Call(Some(target), _) => matches!(&target.without_location().kind,
                                ExprKind::Builtin(Builtin::RegisteredName(_), args) if args.is_empty()),
                            FixedAttribute::Call(None, _) | FixedAttribute::ModuleName(_) => false,
                        })
                    {
                        method_returns.insert(signature.id, Ok(checker.completed_body(params, body, Some(source), false)));
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
            let result = checker.lower_block(entry_statements, None);
            let entry_callable = match checker.finish_region(result) {
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
            };
            self.apply_document_fields(classes);
            if self.revision.get() == before {
                // Recovery can borrow the unchanged round's signatures for its resolved slots.
                let resolved = |(slot, _): &(usize, _)| self.pending.borrow()[*slot].is_resolved();
                // Reproduce the canonical diagnostic pass if discovery stalls.
                if skipped && self.unresolved() {
                    self.revisit_checked_returns();
                    continue;
                }
                if conflicting_returns.is_none() && !self.unresolved() {
                    if completed_calls.len() == initial_nonreturning
                        && classes.nonreturning_functions.len() == initial_nonreturning
                    {
                        for (id, placement) in checked_callers {
                            checked_bodies.insert(id);
                            placement.place(graph, &self.source_checks);
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
                    // An earlier module error can leave a generator's arguments unsolved.
                    // Named startup still needs its suspended body, with the lexical Python binder.
                    if graph.registered_startup {
                        let pending = self.pending.borrow();
                        for (owner, signature) in signatures.iter() {
                            if pending[signature.id].params.iter().any(Option::is_none) {
                                self.source_checks.unresolved.push(*owner);
                            }
                        }
                    }
                    if (!checked_bodies.is_empty() || !staged_returns.is_empty())
                        && !retries_failed_layout(graph)
                    {
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
                                .map(|slot| (slot, None))
                                .filter(resolved),
                        );
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
                    if primary.kind.is_object() && !retries_failed_layout(graph) {
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
                    if !retries_failed_layout(graph)
                        && (unresolved.kind.is_object()
                            || matches!(unresolved.kind, Kind::UnresolvedParameter(..)))
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
        Ok(Solution {
            method_returns,
            registered_parameters: registered,
            checked_bodies,
            suspended_bodies,
            needs_registered_entry,
        })
    }
}

impl Checker<'_> {
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
        let ty = self.enter_source_call(call, signature)?;
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
        Ok((self.enter_source_call(call, signature)?, args))
    }

    /// The caller needs the return layout only after evaluating its arguments.
    pub(super) fn enter_source_call(
        &mut self,
        call: &ast::ExprCall,
        signature: &Signature,
    ) -> Result<Type, String> {
        self.calls.insert(signature.id);
        match self.inference {
            Some(inference) => inference
                .lookup(signature, SignatureUse::Return, self.module, call.range())
                .map_err(|failure| failure.at(self.source, call)),
            None => Ok(signature.returns),
        }
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
        let stat_mode = matches!(self.resolve(&call.func), Some(Binding::Definition(id))
            if self.graph.modules[id.module].is_bundled("stat"));
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
                if selector_wait && parameter.name == "timeout" {
                    let node = argument.site();
                    let value = argument.value(checker)?;
                    let value = checker.selector_seconds(value);
                    return checker.coerce(value, parameter.ty, node);
                }
                if stat_mode && parameter.ty == Type::Int {
                    let node = argument.site();
                    let value = argument.value(checker)?;
                    return if value.ty == Type::Bool {
                        Ok(Expr {
                            ty: Type::Int,
                            kind: ExprKind::Builtin(Builtin::Int(None), vec![value]),
                        })
                    } else {
                        checker.direct_value(value, node, parameter.ty, parameter.sign_only)
                    };
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
                        if supplied.ty == Type::None
                            && ty != Type::None
                            && checker.types.payload(ty).is_none()
                        {
                            let expected = checker.types.optional(ty);
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
                    let types = checker.types;
                    if inference.pending.borrow()[signature.id].inferred_params[index]
                        && (matches!(ty, Type::None | Type::Str | Type::Bytes)
                            || types.element(ty) == Some(Type::Str))
                    {
                        let lines = types.list(Type::Str);
                        let wider = |other| {
                            if ty == other {
                                return None;
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
                        argument.check_with(
                            checker,
                            ty,
                            parameter.sign_only,
                            declaration,
                            |checker, value, expected, site| {
                                let expected = inference.document_parameter(
                                    signature,
                                    checker.classes,
                                    index,
                                    expected,
                                    value.ty,
                                );
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
                    value
                };
                if !can_stop || !checker.terminating_operand(&value) {
                    inference.learn_parameter(signature.id, index, value.ty);
                }
                Ok(value)
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn named_startup_keeps_each_refused_function_operand() {
        let text = concat!(
            "def unused() -> int:\n    values: list[int] = [1]\n    values.append(True)\n    return 1\n",
            "def called() -> int:\n    values: list[int] = [2]\n    values.append('two')\n    return 2\n",
            "print(called())\n",
        );
        let mut graph = SourceGraph::single(text).unwrap();
        graph.registered_startup = true;
        let report = super::super::lower_with_caller_cache(&mut graph, None, None)
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
        let diagnostic = super::super::lower_with_caller_cache(&mut graph, None, None)
            .err()
            .unwrap();
        assert_eq!(diagnostic.kind, Kind::ExactType);
        assert!(diagnostic.to_string().contains(":3:19:"), "{diagnostic}");
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
        let error = super::super::lower_with_caller_cache(&mut graph, None, None)
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
