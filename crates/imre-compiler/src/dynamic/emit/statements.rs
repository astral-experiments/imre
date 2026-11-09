pub(super) mod class_annotations;
use crate::frontend::function_scope::ClassBindings;

use crate::ir::{ItemUpdate, Statement};
use imre_ast::code_flags;
use ruff_python_ast as ast;
use ruff_text_size::{Ranged, TextSize};
use serde_json::json;

use super::{Body, Cleanup, ContextExit, DefinitionName, Entry, Function, LoopTarget};

enum Place {
    Name(String),
    Item(String, String),
}

impl Function<'_, '_> {
    pub(super) fn declare_globals(&mut self, globals: &std::collections::BTreeSet<String>) {
        for name in globals {
            self.declared_globals.insert(name.to_owned());
            let name = self.code_name(name);
            self.frame
                .op("declare_global", &[&self.scope.clone(), &name]);
        }
    }

    fn start_statement(&mut self) -> bool {
        if self.body.terminated {
            if self.module_annotation_sites.is_none() {
                return false;
            }
            // Unreachable statements still contribute to a module provider's site order.
            let dead = self.name("b");
            self.label(&dead);
        }
        true
    }

    /// A physically complete frame takes the canonical function entry. Partial
    /// frames interleave its checked blocks with only their unprepared source.
    pub(super) fn prepared_function(
        &mut self,
        prepared: &crate::frontend::RegisteredSuite,
        emit_source: impl FnMut(&mut Self, std::ops::Range<usize>),
    ) {
        if prepared.fully_checked() {
            self.checked_body(&prepared.checked);
        } else {
            self.checked_runs(prepared, emit_source);
        }
    }

    pub(super) fn checked_runs(
        &mut self,
        prepared: &crate::frontend::RegisteredSuite,
        mut emit_source: impl FnMut(&mut Self, std::ops::Range<usize>),
    ) {
        self.with_checked_slots(&prepared.checked, |code| {
            let body = &prepared.checked.function.body;
            let mut offset = 0;
            for run in &prepared.residual {
                if offset < run.at && code.start_statement() {
                    code.checked_block(&body[offset..run.at], &prepared.checked);
                }
                emit_source(code, run.statements.clone());
                offset = run.at;
            }
            if offset < body.len() && code.start_statement() {
                code.checked_block(&body[offset..], &prepared.checked);
            }
        });
    }

    pub(super) fn statements(&mut self, suite: &[ast::Stmt]) {
        if self.prepare_nested_suites {
            if let Some(first) = suite.first() {
                let plan = &self.source.nested_bodies[&first.start()];
                self.checked_runs(plan, |code, run| code.source_statements(&suite[run]));
            }
        } else {
            self.source_statements(suite);
        }
    }

    pub(super) fn source_statements(&mut self, suite: &[ast::Stmt]) {
        for stmt in suite {
            if !self.start_statement() {
                break;
            }
            self.checkpoint(stmt);
            self.simple_call = imre_ast::unhighlighted_call(stmt);
            if let Some(import) = self.source.namespace_operations.get(&stmt.range()) {
                self.registered_value(import);
                continue;
            }
            if let Some(effects) = crate::frontend::attribute_effects(stmt) {
                for effect in effects {
                    self.attribute_operation(
                        effect.action,
                        effect.field.attr.as_str(),
                        effect.field.range(),
                        effect.operands(),
                        |code, node| code.expr(node),
                    );
                }
                continue;
            }
            match stmt {
                ast::Stmt::Expr(v) => {
                    if !matches!(&*v.value, ast::Expr::StringLiteral(_)) {
                        let value = self.expr(&v.value);
                        self.line(&format!(
                            "call void @imre_dyn_discard_expression(ptr %ctx, i64 {value})"
                        ));
                    }
                }
                ast::Stmt::Pass(_) | ast::Stmt::Global(_) | ast::Stmt::Nonlocal(_) => {}
                ast::Stmt::TypeAlias(v)
                    if self.class_locals.is_none()
                        && let Some(name) = v.name.as_name_expr() =>
                {
                    let outer = self.scope.clone();
                    let params = if let Some(params) = v.type_params.as_deref() {
                        self.scope = self.parameter_scope(&outer, params, false);
                        let key = self.code_name(".type_params");
                        self.frame.op("lookup", &[&self.scope.clone(), &key])
                    } else {
                        "0".into()
                    };
                    let evaluator = self.define(
                        v.start(),
                        DefinitionName::Plain(name.id.as_str()),
                        None,
                        None,
                        None,
                        Body::TypeAlias(&v.value),
                    );
                    let label = self.code_name(name.id.as_str());
                    let alias = self.op("type_alias", &[&label, &evaluator, &params]);
                    self.scope = outer;
                    self.bind(name.id.as_str(), &alias);
                }
                ast::Stmt::Assign(v) => {
                    if let [target] = v.targets.as_slice()
                        && let Some((receiver, key)) = self.checked_index_target(target)
                        && let Some(rhs) = crate::frontend::registered_value(
                            &v.value,
                            self.source.registered_source(),
                        )
                    {
                        self.checked_statement(&Statement::SetItem(receiver, key, rhs));
                        continue;
                    }
                    let value = self.expr(&v.value);
                    let chained = v.targets.len() > 1;
                    if chained {
                        self.live.push(value.clone());
                    }
                    for (index, target) in v.targets.iter().enumerate() {
                        if chained && index + 1 == v.targets.len() {
                            self.live.pop();
                        }
                        let owned = chained
                            || matches!(
                                target,
                                ast::Expr::Name(_) | ast::Expr::List(_) | ast::Expr::Tuple(_)
                            );
                        self.assign_target(target, &value, owned);
                    }
                }
                ast::Stmt::Delete(v) => {
                    for target in crate::frontend::delete_targets(v) {
                        if let Some(binding) = self.source.namespace_operations.get(&target.range())
                        {
                            self.registered_value(binding);
                            continue;
                        }
                        if let ast::Expr::Attribute(field) = target {
                            self.attribute_operation(
                                crate::ir::AttributeAction::Delete,
                                field.attr.as_str(),
                                field.range(),
                                std::iter::once(field.value.as_ref()),
                                |code, expr| code.expr(expr),
                            );
                            continue;
                        }
                        if let Some((receiver, key)) = self.checked_index_target(target) {
                            self.checked_statement(&Statement::Expr(crate::frontend::delete_item(
                                receiver, key,
                            )));
                            continue;
                        }
                        match self.place(target) {
                            Some(Place::Item(obj, key)) => {
                                self.delete_item(&obj, &key);
                            }
                            _ => {
                                self.unsupported("deletion target");
                            }
                        }
                    }
                }
                ast::Stmt::AnnAssign(v) => {
                    if let Some(value) = &v.value {
                        let value = self.expr(value);
                        self.assign_target(
                            &v.target,
                            &value,
                            matches!(&*v.target, ast::Expr::Name(_)),
                        );
                    } else {
                        match &*v.target {
                            ast::Expr::Attribute(attr) => {
                                self.discard_annotation_operand(&attr.value);
                            }
                            ast::Expr::Subscript(subscript) => {
                                self.discard_annotation_operand(&subscript.value);
                                self.discard_annotation_slice(&subscript.slice);
                            }
                            _ => {}
                        }
                    }
                    if self.module
                        && let Some(marker) =
                            self.source.namespace_operations.get(&v.target.range())
                    {
                        self.registered_value(marker);
                    } else if let ast::Expr::Name(name) = &*v.target
                        && v.simple
                        && (self.class_locals.is_some()
                            || self.module && self.source.string_annotations)
                    {
                        let key = self.code_name(name.id.as_str());
                        if !self.source.string_annotations {
                            self.remember_annotation(v, name.id.as_str());
                            let offset = v.start().to_u32();
                            self.frame.checked(&format!("call i64 @imre_dyn_mark_class_annotation(ptr %ctx, i64 {}, i64 {key}, i32 {offset})", self.scope));
                        } else {
                            let value = self.annotation_text(
                                &crate::frontend::annotation_spelling(&v.annotation),
                            );
                            self.frame
                                .op("annotate", &[&self.scope.clone(), &key, &value]);
                        }
                    }
                }
                ast::Stmt::AugAssign(v) => {
                    if let Some((receiver, key)) = self.checked_index_target(&v.target) {
                        if let Some(rhs) = crate::frontend::registered_value(
                            &v.value,
                            self.source.registered_source(),
                        ) {
                            self.checked_statement(&Statement::AugmentItem(
                                receiver,
                                key,
                                ItemUpdate::Protocol(v.op.as_str()),
                                rhs,
                            ));
                            continue;
                        }
                        let receiver = self.registered_value(&receiver);
                        let key = self.registered_value(&key);
                        self.update_item(receiver, key, v.op.as_str(), |code| code.expr(&v.value));
                        continue;
                    }
                    if let Some(target) = self.place(&v.target) {
                        self.update_place(&target, v.op.as_str(), |code| code.expr(&v.value));
                    }
                }
                ast::Stmt::FunctionDef(v) => {
                    let decorators = v
                        .decorator_list
                        .iter()
                        .map(|d| self.expr(&d.expression))
                        .collect::<Vec<_>>();
                    let function = self.function_definition(v, None);
                    let function = self.apply_decorators(&decorators, &v.decorator_list, function);
                    self.bind(v.name.as_str(), &function);
                }
                ast::Stmt::ClassDef(v) => {
                    let decorators = v
                        .decorator_list
                        .iter()
                        .map(|d| self.expr(&d.expression))
                        .collect::<Vec<_>>();
                    let class = if v.type_params.is_some() {
                        let name = format!("<generic parameters of {}>", v.name);
                        let header = self.define(
                            v.start(),
                            DefinitionName::Plain(&name),
                            None,
                            None,
                            None,
                            Body::ClassParameters(v),
                        );
                        self.class_position(Some(v));
                        let class = self.invoke(&header, &[]);
                        self.class_position(None);
                        class
                    } else {
                        self.create_class(v, None)
                    };
                    let class = self.apply_decorators(&decorators, &v.decorator_list, class);
                    self.bind(v.name.as_str(), &class);
                }
                ast::Stmt::Return(v) => {
                    let value = v
                        .value
                        .as_deref()
                        .map_or_else(|| "0".into(), |e| self.expr(e));
                    self.return_from_scope(value);
                }
                ast::Stmt::Match(v) => self.match_statement(v),
                ast::Stmt::If(v) => {
                    let done = self.name("b");
                    let mut next = self.if_arm(&v.test, &v.body, &done);
                    for clause in &v.elif_else_clauses {
                        if !next {
                            break;
                        }
                        if let Some(test) = &clause.test {
                            next = self.if_arm(test, &clause.body, &done);
                        } else {
                            self.statements(&clause.body);
                        }
                    }
                    self.jump(&done);
                    self.label(&done);
                }
                ast::Stmt::For(v) => {
                    let iter = self.iteration_cursor(&v.iter, v.is_async);
                    self.loop_body(&v.body, &v.orelse, Some(&iter), |f, yes, no| {
                        if f.native_suspension {
                            // A retained frame exposes the header after its final pull.
                            f.set_statement_line(f.source.positions.line(v.start()));
                        }
                        f.enter_iteration(
                            &iter,
                            &v.target,
                            v.is_async.then(|| v.iter.range()),
                            !v.orelse.is_empty(),
                            yes,
                            no,
                        );
                    });
                }
                ast::Stmt::While(v) => {
                    if self.native_suspension
                        && matches!(&*v.test, ast::Expr::BooleanLiteral(lit) if !lit.value)
                    {
                        self.statements(&v.orelse);
                        continue;
                    }
                    self.loop_body(&v.body, &v.orelse, None, |f, yes, no| {
                        f.branch_expr(&v.test, yes, no);
                        f.label(yes);
                    });
                }
                ast::Stmt::Break(_) | ast::Stmt::Continue(_) => {
                    if let Some(target) = self.loops.last() {
                        let (dest, iterator) = if matches!(stmt, ast::Stmt::Break(_)) {
                            (target.done.clone(), target.iterator.clone())
                        } else {
                            (target.step.clone(), None)
                        };
                        self.run_cleanups(target.cleanup_depth, None);
                        if let Some(iter) = iterator
                            && !self.body.terminated
                        {
                            self.op("break_iter", &[&iter]);
                        }
                        self.jump(&dest);
                    } else {
                        self.unsupported("loop control outside a loop");
                    }
                }
                ast::Stmt::Try(v) => {
                    if v.finalbody.is_empty() {
                        self.try_except(v);
                    } else {
                        self.try_finally(v);
                    }
                }
                ast::Stmt::With(v) if !v.is_async || self.native_suspension => {
                    self.with(&v.items, &v.body, v.is_async);
                }
                ast::Stmt::Raise(v) => {
                    let exc = if let Some(e) = &v.exc {
                        self.expr(e)
                    } else {
                        self.caught.last().cloned().unwrap_or_else(|| "0".into())
                    };
                    let cause = v.cause.as_deref().map(|e| self.expr(e));
                    self.raise(&exc, cause.as_deref(), v.exc.is_none());
                }
                ast::Stmt::Assert(v) => {
                    let pass = self.name("b");
                    let fail = self.name("b");
                    self.branch_expr(&v.test, &pass, &fail);
                    self.label(&fail);
                    let cls = self.lookup("AssertionError");
                    let error = if let Some(msg) = &v.msg {
                        let msg = self.expr(msg);
                        self.invoke(&cls, &[&msg])
                    } else {
                        self.invoke(&cls, &[])
                    };
                    self.raise(&error, None, false);
                    self.label(&pass);
                }
                _ => {
                    self.unsupported(&format!("{:?}", ast::AnyNodeRef::from(stmt).kind()));
                }
            }
        }
    }

    fn class_position(&mut self, class: Option<&ast::StmtClassDef>) {
        if !self.output.declarations.declared_definition_position {
            self.output
                .functions
                .push_str("declare void @imre_dyn_definition_position(ptr, ptr)\n");
            self.output.declarations.declared_definition_position = true;
        }
        let position = class.map_or_else(
            || "null".to_owned(),
            |class| {
                let index = self
                    .source
                    .class_keywords
                    .partition_point(|&p| p < class.name.start());
                let (line, start) = self
                    .source
                    .positions
                    .line_and_column(self.source.class_keywords[index - 1]);
                let (end_line, end) = self.source.positions.line_and_column(class.end());
                self.expression_line = line;
                self.output.position([line, end_line, start, end])
            },
        );
        self.line(&format!(
            "call void @imre_dyn_definition_position(ptr %ctx, ptr {position})"
        ));
    }

    pub(super) fn parameterized_class(&mut self, class: &ast::StmtClassDef) {
        self.checkpoint(&class.name);
        self.class_position(Some(class));
        let frame = self.scope.clone();
        let outer = self.op("annotation_scope", &[&frame]);
        let params = class.type_params.as_deref().expect("generic class");
        let scope = self.parameter_scope(&frame, params, true);
        self.scope = self.op("class_base_scope", &[&scope, &outer]);
        self.class_position(None);
        let class = self.create_class(class, Some(scope));
        self.end(&format!("ret i64 {class}"));
    }

    fn create_class(&mut self, class: &ast::StmtClassDef, parameters: Option<String>) -> String {
        let (bases, kwargs) = if let Some(args) = class.arguments.as_deref() {
            self.argument_pack(
                &crate::frontend::packed_arguments(args),
                crate::frontend::registered_arguments(args),
                None,
                &mut Self::expr,
            )
        } else {
            (self.op("list", &[]), "0".into())
        };
        let generic = parameters.is_some();
        if let Some(scope) = parameters {
            self.scope = scope;
            let base = self.lookup(".generic_base");
            self.op("append", &[&bases, &base]);
        }
        let init = self.define(
            class
                .decorator_list
                .first()
                .map_or(class.start(), Ranged::start),
            DefinitionName::Binding(&class.name),
            None,
            None,
            None,
            Body::Namespace(&class.body),
        );
        let operation = if generic {
            self.class_position(Some(class));
            "parameterized_class"
        } else {
            "class"
        };
        self.op(operation, &[&kwargs, &bases, &init])
    }

    fn parameter_scope(&mut self, outer: &str, params: &ast::TypeParams, class: bool) -> String {
        let names = self.op("dict", &[]);
        for param in params {
            if param.default().is_some()
                || matches!(param, ast::TypeParam::TypeVar(v) if v.bound.is_some())
            {
                self.unsupported("class type parameter bounds and defaults");
            }
            let binding = self.code_name(param.name().as_str());
            let name = self
                .frame
                .code_name(self.source.private_names.definition(param.name()));
            let declaration = match param {
                ast::TypeParam::TypeVar(_) => name,
                ast::TypeParam::TypeVarTuple(_) => self.op("class_typevartuple", &[&name]),
                ast::TypeParam::ParamSpec(_) => self.op("class_paramspec", &[&name]),
            };
            self.op("put", &[&names, &binding, &declaration]);
        }
        let op = if class {
            "class_parameter_frame"
        } else {
            "function_parameter_scope"
        };
        self.frame.op(op, &[outer, &names])
    }

    pub(super) fn registered_definition(&mut self, start: TextSize, values: &[String]) -> String {
        match self.source.definitions.as_ref().expect("registered scope")[&start] {
            super::DefinitionSite::Function(definition) => {
                let (decorators, defaults) = values.split_at(definition.decorator_list.len());
                let function = self.function_definition(definition, Some(defaults));
                self.apply_decorators(decorators, &definition.decorator_list, function)
            }
            super::DefinitionSite::Class(definition) => {
                let class = self.create_class(definition, None);
                self.apply_decorators(values, &definition.decorator_list, class)
            }
        }
    }

    fn function_definition(
        &mut self,
        v: &ast::StmtFunctionDef,
        defaults: Option<&[String]>,
    ) -> String {
        let generator = code_flags::is_generator(&v.body);
        let body = match (v.is_async, generator) {
            (false, false) => Body::Statements(&v.body),
            (false, true) => Body::GeneratorFunction(&v.body, code_flags::GENERATOR),
            (true, false) => Body::GeneratorFunction(&v.body, code_flags::COROUTINE),
            (true, true) => Body::GeneratorFunction(&v.body, code_flags::ASYNC_GENERATOR),
        };
        // A method's lazy annotations also need its defining class namespace.
        let type_params = if self.class_locals.is_some() && !self.source.string_annotations {
            None
        } else {
            crate::dynamic::boxed_function_type_params(
                v,
                self.source.string_annotations,
                !self.prefix.is_empty(),
                Some(&self.source.private_names),
            )
        };
        self.define(
            v.decorator_list.first().map_or(v.start(), Ranged::start),
            DefinitionName::Binding(&v.name),
            Some(&v.parameters),
            Some((v, defaults)),
            type_params,
            body,
        )
    }

    pub(super) fn define(
        &mut self,
        start: TextSize,
        name: DefinitionName<'_>,
        params: Option<&ast::Parameters>,
        definition: Option<(&ast::StmtFunctionDef, Option<&[String]>)>,
        type_params: Option<&ast::TypeParams>,
        body: Body<'_>,
    ) -> String {
        let function = definition.map(|(function, _)| function);
        let mut evaluated_defaults = definition
            .and_then(|(_, defaults)| defaults)
            .map(<[_]>::iter);
        let (name, global) = match name {
            DefinitionName::Binding(identifier) => (
                self.source.private_names.definition(identifier),
                self.declared_globals.contains(identifier.as_str()),
            ),
            DefinitionName::Plain(name) => (name, false),
        };
        let qualname = if global {
            name.to_owned()
        } else {
            format!("{}{name}", self.prefix)
        };
        let returns = function.and_then(|f| f.returns.as_deref());
        // A generic function's immediate lexical parent is its type-parameter scope,
        // even when lowering does not materialize a separate parameter frame.
        let generic = function.is_some_and(|f| f.type_params.is_some());
        let nested = self.in_function || generic;
        let header = matches!(body, Body::ClassParameters(_));
        let prefix = if let Body::ClassParameters(class) = &body {
            if self.declared_globals.contains(class.name.as_str()) {
                String::new()
            } else {
                self.prefix.clone()
            }
        } else if matches!(body, Body::Namespace(..)) {
            format!("{qualname}.")
        } else {
            format!("{qualname}.<locals>.")
        };
        let statements = match &body {
            Body::Statements(stmts) | Body::GeneratorFunction(stmts, _) => Some(*stmts),
            _ => None,
        };
        let doc = statements
            .filter(|_| self.output.keep_docstrings)
            .and_then(|stmts| self.source.docstring(&qualname, stmts));
        let generator_flags = body.generator_flags();
        let flags = match &body {
            Body::Namespace(..) => None,
            _ => {
                let mut flags = self.source.function_flags(
                    params,
                    nested,
                    self.class_locals.is_some()
                        && self.annotation_values.is_none()
                        && !header
                        && !generic,
                );
                if doc.is_some() {
                    flags |= code_flags::HAS_DOCSTRING;
                }
                Some(flags | generator_flags)
            }
        };
        let doc = doc.map_or_else(|| "0".into(), |s| self.text(&s));
        let entries = params.map_or_else(Vec::new, |p| {
            crate::frontend::parameter_annotations(p, returns)
        });
        let outer = self.scope.clone();
        let scope = type_params
            .map(|params| self.parameter_scope(&outer, params, false))
            .unwrap_or(outer);
        let annotate = if entries.is_empty() {
            "0".into()
        } else {
            let class = self
                .class_locals
                .as_ref()
                .filter(|_| !self.source.string_annotations)
                .cloned();
            let reads = self.source.scopes.annotations.parameter_reads(
                params.expect("annotated function parameters").start(),
                class.as_ref(),
            );
            let mut metadata = json!({
                "posonly": ["format"],
                "code_flags": self.source.function_flags(None, nested, false),
                "scope_reads": reads,
                "scope_complete": true,
                "scope_locals_complete": true,
                "annotation_class": class.is_some(),
            });
            if !self.source.python_annotations && type_params.is_none() {
                for (slot, expr) in &entries {
                    if let Some(reference) = scalar_reference(expr) {
                        let root = reference.split('.').next().unwrap();
                        if class
                            .as_ref()
                            .is_some_and(|class| class.globals.contains(root))
                        {
                            continue;
                        }
                        metadata["annotation_references"][slot] = json!(reference);
                    }
                }
            }
            let sig = self.text(&metadata.to_string());
            let code = self.frame.output.definition(
                self.source,
                String::new(),
                Body::Annotations(entries, class),
                params.map_or(start, Ranged::start),
                false,
                Entry::Compiled,
            );
            let aname = self.text("__annotate__");
            let aqname = self.text(&format!("{qualname}.__annotate__"));
            self.checked(&format!("call i64 @imre_dyn_define(ptr %ctx, ptr {code}, i64 {}, i64 {aname}, i64 {sig}, i64 0, i64 {aqname}, i64 0, i64 0, i1 true)", scope))
        };
        let qualname = self.text(&qualname);
        let mut signature = json!({});
        if let Some(flags) = flags {
            signature["code_flags"] = json!(flags);
        }
        if matches!(body, Body::Generator(..)) {
            // generator() binds the implicit .0 argument without calling the step function.
            signature["positional"] = json!([".0"]);
        }
        // A synchronous generator expression has its own compiled comprehension step.
        let entry = match generator_flags {
            0 => Entry::Compiled,
            code_flags::GENERATOR if matches!(body, Body::Generator(_)) => Entry::Compiled,
            _ => Entry::Suspended,
        };
        let scopes = &self.source.scopes;
        let scope_facts = match &body {
            Body::Statements(_) | Body::GeneratorFunction(..) => Some(&scopes.functions[&start]),
            Body::Namespace(_) => {
                let class = &scopes.classes[&start];
                signature["postponed_annotations"] = json!(class.postponed_annotations);
                signature["needs_class_cell"] = json!(class.needs_class_cell);
                signature["needs_dict_cell"] = json!(class.needs_dict_cell);
                signature["static_attributes"] = json!(class.static_attributes);
                signature["class_frame_reads"] = json!(class.frame_reads);
                if !class.bindings.globals.is_empty() {
                    signature["class_globals"] = json!(class.bindings.globals);
                }
                Some(&class.namespace)
            }
            Body::TypeAlias(_) => Some(&scopes.aliases[&start]),
            Body::Generator(_) => Some(&scopes.generators[&start]),
            _ => None,
        };
        if let Some(facts) = scope_facts {
            facts.write_signature(&mut signature);
            let callable = matches!(body, Body::Statements(_) | Body::GeneratorFunction(..));
            if callable && facts.captures_class {
                signature["class_cell"] = json!(true);
            }
            if (callable || matches!(body, Body::Generator(_)))
                && self.class_locals.is_some()
                && self.annotation_values.is_none()
                && type_params.is_none()
            {
                signature["method_class_cell"] = json!(facts.captures_class);
            }
        } else {
            signature["scope_cellvars"] = serde_json::Value::Null;
            signature["scope_complete"] = json!(false);
            signature["scope_locals_complete"] = json!(true);
        }
        if let Some(entry) = function.and_then(|def| self.source.thread_entries.get(&def.start())) {
            match entry {
                imre_ast::ThreadEntry::Parameters { names, lock_only } => {
                    signature["thread_parameters"] = json!(names);
                    if *lock_only {
                        signature["thread_worker_locks"] = json!(names);
                    }
                }
                imre_ast::ThreadEntry::Captured(names) => {
                    signature["thread_captured_locks"] = json!(names);
                }
                imre_ast::ThreadEntry::Bound(field) => {
                    signature["thread_bound_lock_field"] = json!(field)
                }
            }
        }
        // A type alias thunk has an implicit default for its format parameter.
        let defaults = if matches!(body, Body::TypeAlias(_))
            || params.is_some_and(|p| {
                p.iter_non_variadic_params()
                    .any(|arg| arg.default.is_some())
            }) {
            self.op("dict", &[])
        } else {
            "0".into()
        };
        if matches!(body, Body::TypeAlias(_)) {
            signature["posonly"] = json!([".format"]);
            let format = self.text(".format");
            let value_format = self.integer(1);
            self.op("put", &[&defaults, &format, &value_format]);
        }
        let in_function = self.in_function || !matches!(body, Body::Namespace(..));
        // A generator call returns a suspended frame, before it yields or returns a value.
        let scalar = |expr: &ast::Expr| {
            if self.source.python_annotations {
                return None;
            }
            scalar_reference(expr).filter(|name| {
                !type_params.is_some_and(|params| params.iter().any(|p| p.name().as_str() == name))
            })
        };
        if matches!(entry, Entry::Compiled)
            && let Some(ty) = returns.and_then(scalar)
        {
            signature["return_type"] = json!(ty);
        }
        let code =
            self.frame
                .output
                .definition(self.source, prefix, body, start, in_function, entry);
        if let Some(p) = params {
            self.output
                .interned_identifiers
                .extend(p.iter().map(|p| p.name().to_string()));
            for (group, args) in [
                ("posonly", &p.posonlyargs),
                ("positional", &p.args),
                ("kwonly", &p.kwonlyargs),
            ] {
                let mut names = Vec::with_capacity(args.len());
                for arg in args {
                    let name = arg.parameter.name.as_str();
                    if let Some(ty) = arg.parameter.annotation.as_deref().and_then(scalar) {
                        signature["types"][name] = json!(ty);
                        if ty == "float"
                            && statements
                                .is_some_and(|body| crate::frontend::only_compares_zero(body, name))
                        {
                            signature["integer_sign_parameters"][name] = json!(true);
                        }
                    }
                    names.push(name);
                    if let Some(default) = &arg.default {
                        let default = match &mut evaluated_defaults {
                            Some(values) => values.next().expect("lowered default").clone(),
                            None => self.expr(default),
                        };
                        let name = self.code_name(name);
                        self.op("put", &[&defaults, &name, &default]);
                    }
                }
                if !names.is_empty() {
                    signature[group] = json!(names);
                }
            }
            for (key, parameter) in [("varargs", &p.vararg), ("kwargs", &p.kwarg)] {
                if let Some(parameter) = parameter {
                    signature[key] = json!(parameter.name.as_str());
                }
            }
        }
        debug_assert!(evaluated_defaults.is_none_or(|mut rest| rest.next().is_none()));
        let sig = self.text(&signature.to_string());
        let name = self.code_name(name);
        let function = self.checked(&format!("call i64 @imre_dyn_define(ptr %ctx, ptr {code}, i64 {}, i64 {name}, i64 {sig}, i64 {defaults}, i64 {qualname}, i64 {doc}, i64 {annotate}, i1 {header})", scope));
        if type_params.is_some() {
            let key = self.code_name(".type_params");
            let params = self.op("lookup", &[&scope, &key]);
            let key = self.code_name("__type_params__");
            self.op("setattr", &[&function, &key, &params]);
        }
        function
    }

    fn check_annotation_format(&mut self) {
        let format = self.lookup("format");
        self.op("annotation_format", &[&format]);
    }

    pub(super) fn registered_module_annotation(
        &mut self,
        annotation: &crate::ir::ModuleAnnotation,
    ) {
        match annotation {
            crate::ir::ModuleAnnotation::Deferred(site) => {
                let name = &self.source.module_annotations[site].name;
                self.code_name(name);
                let sites = self
                    .module_annotation_sites
                    .as_mut()
                    .expect("module annotation provider");
                // A finally suite can emit the same declaration on several independent paths.
                let index = sites.len();
                sites.push(*site);
                let index = self.integer(index);
                self.frame
                    .op("mark_module_annotation", &[&self.scope.clone(), &index]);
            }
            crate::ir::ModuleAnnotation::String { name, spelling } => {
                let name = self.code_name(name);
                let value = self.text(spelling);
                self.frame
                    .op("annotate", &[&self.scope.clone(), &name, &value]);
            }
        }
    }

    /// Publish a module's provider before its first statement. Every simple
    /// annotation, including an unconditional one, becomes visible only after
    /// execution reaches its assignment.
    pub(super) fn setup_deferred_module_annotations(&mut self, sites: &[TextSize]) {
        if !sites.is_empty() {
            let entries: Vec<_> = sites
                .iter()
                .map(|site| &self.source.module_annotations[site])
                .collect();
            let plan = crate::frontend::DeferredModuleAnnotation::prepare_body(&entries);
            let signature = self.text(
                &json!({
                    "posonly": ["format"], "scope_complete": true, "scope_locals_complete": true,
                    "code_flags": self.source.function_flags(None, false, false),
                })
                .to_string(),
            );
            let code = self.frame.output.definition(
                self.source,
                String::new(),
                Body::ModuleAnnotations(&plan, &entries),
                self.source.module_start,
                false,
                Entry::Compiled,
            );
            let name = self.text("__annotate__");
            let provider = self.frame.checked(&format!("call i64 @imre_dyn_define(ptr %ctx, ptr {code}, i64 {}, i64 {name}, i64 {signature}, i64 0, i64 {name}, i64 0, i64 0, i1 true)", self.scope));
            self.bind("__annotate__", &provider);
        }
        self.frame
            .op("setup_module_annotations", &[&self.scope.clone()]);
    }

    pub(super) fn begin_annotations(&mut self, class: Option<ClassBindings>) {
        self.check_annotation_format();
        self.annotation_values = Some(self.op("dict", &[]));
        // Annotation expressions resolve outside the provider's public format parameter.
        self.scope = self.frame.op("annotation_scope", &[&self.scope.clone()]);
        self.class_locals = class;
    }

    pub(super) fn insert_annotation(
        &mut self,
        index: Option<usize>,
        name: &str,
        evaluate: impl FnOnce(&mut Self) -> String,
    ) {
        let after = index.map(|index| {
            let active = self.lookup("__conditional_annotations__");
            let index = self.integer(index);
            let present = self.op("in", &[&index, &active]);
            let condition = self.truth(&present);
            let yes = self.name("b");
            let after = self.name("b");
            self.branch(&condition, &yes, &after);
            self.label(&yes);
            after
        });
        let key = self.code_name(name);
        let value = evaluate(self);
        let values = self
            .annotation_values
            .as_ref()
            .expect("annotation dictionary")
            .clone();
        self.op("put", &[&values, &key, &value]);
        if let Some(after) = after {
            self.jump(&after);
            self.label(&after);
        }
    }

    pub(super) fn take_annotation_result(&mut self) -> String {
        self.annotation_values
            .take()
            .expect("annotation dictionary")
    }

    pub(super) fn unprepared_annotation(
        &mut self,
        index: usize,
        entry: &crate::frontend::DeferredModuleAnnotation,
    ) {
        let crate::frontend::AnnotationValue::Source(expression) = &entry.value else {
            unreachable!("checked annotation outside its prepared run")
        };
        self.insert_annotation(Some(index), &entry.name, |code| {
            code.source_annotation_value(expression)
        });
    }

    pub(super) fn annotations(
        &mut self,
        entries: &[(&str, &ast::Expr)],
        class: Option<ClassBindings>,
    ) {
        self.begin_annotations(class);
        for &(name, expression) in entries {
            self.insert_annotation(None, name, |code| code.source_annotation_value(expression));
        }
        let values = self.take_annotation_result();
        self.return_value(&values);
    }

    fn source_annotation_value(&mut self, expression: &ast::Expr) -> String {
        if self.source.string_annotations {
            self.annotation_text(&crate::frontend::annotation_spelling(expression))
        } else if let ast::Expr::Starred(starred) = expression {
            let value = self.expr(&starred.value);
            let count = self.integer(1);
            let items = self.op("unpack", &[&value, &count, "0"]);
            let index = self.integer(0);
            self.op("get", &[&items, &index])
        } else {
            self.annotation_value(expression)
        }
    }

    fn annotation_value(&mut self, expression: &ast::Expr) -> String {
        if let ast::Expr::StringLiteral(literal) = expression
            && !self.source.has_templates
            && self.source.replacements.is_empty()
        {
            self.annotation_text(literal.value.to_str())
        } else {
            self.expr(expression)
        }
    }

    fn apply_decorators(
        &mut self,
        decorators: &[String],
        positions: &[ast::Decorator],
        mut value: String,
    ) -> String {
        for (index, decorator) in decorators.iter().enumerate().rev() {
            let expression = &positions[index].expression;
            self.set_statement_line(self.source.positions.line(expression.start()));
            value = self.invoke(decorator, &[&value]);
        }
        value
    }

    fn discard_annotation_operand(&mut self, expr: &ast::Expr) {
        let mark = self.temporary_mark();
        self.expr(expr);
        self.discard_truth(&mark);
    }

    // Without an assignment, CPython evaluates each subscript operand but
    // neither constructs an extended slice nor invokes __getitem__.
    fn discard_annotation_slice(&mut self, slice: &ast::Expr) {
        match slice {
            ast::Expr::Slice(s) => {
                for bound in [&s.lower, &s.upper, &s.step].into_iter().flatten() {
                    self.discard_annotation_operand(bound);
                }
            }
            ast::Expr::Tuple(t) => {
                for item in &t.elts {
                    self.discard_annotation_slice(item);
                }
            }
            expr => self.discard_annotation_operand(expr),
        }
    }

    pub(super) fn index_operand(&mut self, key: &ast::Expr) -> String {
        if let Some(key) =
            crate::frontend::registered_index_key(key, self.source.registered_source())
        {
            self.registered_value(&key)
        } else {
            self.expr(key)
        }
    }

    fn checked_index_target(
        &self,
        target: &ast::Expr,
    ) -> Option<(crate::ir::Expr, crate::ir::Expr)> {
        crate::frontend::registered_index_target(target, self.source.registered_source())
    }

    fn place(&mut self, target: &ast::Expr) -> Option<Place> {
        Some(match target {
            ast::Expr::Name(n) => Place::Name(self.code_name(n.id.as_str())),
            ast::Expr::Subscript(s) => {
                let obj = self.expr(&s.value);
                Place::Item(obj, self.index_operand(&s.slice))
            }
            _ => {
                self.unsupported("assignment target");
                return None;
            }
        })
    }

    fn read(&mut self, place: &Place) -> String {
        match place {
            Place::Name(n) => self.frame.op("lookup", &[&self.scope.clone(), n]),
            Place::Item(v, k) => self.frame.op("get", &[v, k]),
        }
    }

    fn write(&mut self, place: &Place, value: &str) {
        match place {
            Place::Name(n) => self.frame.op("assign", &[&self.scope.clone(), n, value]),
            Place::Item(v, k) => self.put_item(v, k, value),
        };
    }

    fn update_place(&mut self, place: &Place, op: &str, rhs: impl FnOnce(&mut Self) -> String) {
        let left = self.read(place);
        let right = rhs(self);
        let value = self.op(&format!("inplace:{op}"), &[&left, &right]);
        self.write(place, &value);
    }

    /// Keep the evaluated receiver and key through the read, RHS and in-place protocol.
    pub(super) fn update_item(
        &mut self,
        receiver: String,
        key: String,
        op: &str,
        rhs: impl FnOnce(&mut Self) -> String,
    ) {
        self.update_place(&Place::Item(receiver, key), op, rhs)
    }

    pub(super) fn delete_item(&mut self, receiver: &str, key: &str) -> String {
        self.op("delitem", &[receiver, key])
    }

    pub(super) fn put_item(&mut self, receiver: &str, index: &str, value: &str) -> String {
        self.op("put", &[receiver, index, value])
    }

    pub(super) fn assign(&mut self, target: &ast::Expr, value: &str) {
        self.assign_target(target, value, false);
    }

    /// Transfer unpacked items to their targets while retaining the outstanding operands.
    fn assign_target(&mut self, target: &ast::Expr, value: &str, owned: bool) {
        if let Some((receiver, key)) = self.checked_index_target(target) {
            let receiver = self.registered_value(&receiver);
            let key = self.registered_value(&key);
            self.put_item(&receiver, &key, value);
            if owned {
                self.release_target(None);
            }
            return;
        }
        let elements = match target {
            ast::Expr::Tuple(t) => &t.elts,
            ast::Expr::List(l) => &l.elts,
            ast::Expr::Attribute(field) => {
                self.store_attribute(field.attr.as_str(), field.range(), value, |code| {
                    code.expr(&field.value)
                });
                if owned {
                    self.release_target(None);
                }
                return;
            }
            _ => {
                if let Some(p) = self.place(target) {
                    if owned {
                        if let Place::Name(name) = &p {
                            let scope = self.scope.clone();
                            self.assign_owned_name(&scope, name, value, None);
                        } else {
                            self.write(&p, value);
                            // An item setter may replace a writer.
                            self.release_target(None);
                        }
                    } else {
                        self.write(&p, value);
                    }
                }
                return;
            }
        };
        if !owned && let Some([left, right]) = Self::pair_target(target) {
            self.max_live = self.max_live.max(2);
            self.checked(&format!(
                "call i64 @imre_dyn_unpack_pair(ptr %ctx, i64 {value}, ptr %live)"
            ));
            self.assign_pair_slots(left, right);
            return;
        }
        let count = self.integer(elements.len());
        let star = elements
            .iter()
            .position(|v| matches!(v, ast::Expr::Starred(_)))
            .map_or_else(|| "0".into(), |i| self.integer(i));
        let operation = if owned { "unpack" } else { "unpack_read" };
        let sequence = self.op(operation, &[value, &count, &star]);
        if owned {
            self.live.push(sequence.clone());
        }
        for (index, target) in elements.iter().enumerate() {
            let i = self.integer(index);
            let item = self.op(if owned { "unpack_take" } else { "get" }, &[&sequence, &i]);
            let target = if let ast::Expr::Starred(s) = target {
                &s.value
            } else {
                target
            };
            self.assign_target(target, &item, owned);
        }
        if owned {
            self.live.pop();
        }
    }

    /// Enter the loop body with its target bound, or branch to the exhaustion successor.
    pub(super) fn enter_iteration(
        &mut self,
        iter: &str,
        target: &ast::Expr,
        async_range: Option<ruff_text_size::TextRange>,
        has_else: bool,
        body: &str,
        exhausted: &str,
    ) {
        if let Some(range) = async_range {
            let item = self.async_pull(iter, range, body, exhausted);
            self.label(body);
            self.assign(target, &item);
            return;
        }
        let more = if Self::pair_target(target).is_some() {
            self.max_live = self.max_live.max(2);
            self.checked(&format!(
                "call i64 @imre_dyn_next_pair(ptr %ctx, i64 {iter}, ptr %live, i1 {has_else})"
            ))
        } else {
            self.op(if has_else { "more_else" } else { "more" }, &[iter])
        };
        let ready = self.value(&format!("icmp eq i64 {more}, 2"));
        self.branch(&ready, body, exhausted);
        self.label(body);
        if let Some([left, right]) = Self::pair_target(target) {
            self.assign_pair_slots(left, right);
        } else {
            let item = self.op("next", &[iter]);
            self.assign(target, &item);
        }
    }

    /// Only the implicit __anext__ call and its await can exhaust an async for.
    fn async_pull(
        &mut self,
        iter: &str,
        range: ruff_text_size::TextRange,
        body: &str,
        exhausted: &str,
    ) -> String {
        let handler = self.name("b");
        let outer = std::mem::replace(&mut self.body.error_target, handler.clone());
        let mark = self.temporary_mark();
        let next = self.op("async_next_iter", &[iter]);
        let item = self.suspend_at(&next, super::suspension::Suspension::Await, range, &mark);
        self.flush_check();
        self.body.error_target = outer;
        self.jump(body);
        self.label(&handler);
        self.discard_truth(&mark);
        let error = self.op("take_error", &[]);
        let name = self.text("StopAsyncIteration");
        let class = self.op("builtin", &[&name]);
        let matches = self.op("matches", &[&error, &class]);
        let stop = self.value(&format!("icmp eq i64 {matches}, 2"));
        let propagate = self.name("b");
        self.branch(&stop, exhausted, &propagate);
        self.label(&propagate);
        self.raise(&error, None, true);
        item
    }

    fn pair_target(target: &ast::Expr) -> Option<[&ast::Expr; 2]> {
        let elements = match target {
            ast::Expr::Tuple(t) => &t.elts,
            ast::Expr::List(l) => &l.elts,
            _ => return None,
        };
        let [left, right] = elements.as_slice() else {
            return None;
        };
        (!left.is_starred_expr() && !right.is_starred_expr()).then_some([left, right])
    }

    fn assign_pair_slots(&mut self, left: &ast::Expr, right: &ast::Expr) {
        // Target expressions may make calls that reuse the scratch slots.
        let first = self.value("load i64, ptr %live");
        let second_slot = self.value("getelementptr i64, ptr %live, i64 1");
        let second = self.value(&format!("load i64, ptr {second_slot}"));
        self.assign(left, &first);
        self.assign(right, &second);
    }

    /// Return whether control can reach a subsequent arm.
    fn if_arm(&mut self, test: &ast::Expr, body: &[ast::Stmt], done: &str) -> bool {
        if self.native_suspension
            && let ast::Expr::BooleanLiteral(lit) = test
        {
            if lit.value {
                self.statements(body);
                self.jump(done);
            }
            return !lit.value;
        }
        let body_label = self.name("b");
        let next = self.name("b");
        self.branch_expr(test, &body_label, &next);
        self.label(&body_label);
        self.statements(body);
        self.jump(done);
        self.label(&next);
        true
    }

    fn loop_body(
        &mut self,
        body: &[ast::Stmt],
        otherwise: &[ast::Stmt],
        iterator: Option<&str>,
        enter_body: impl FnOnce(&mut Self, &str, &str),
    ) {
        let step = self.name("b");
        let body_label = self.name("b");
        let exhausted = self.name("b");
        let done = self.name("b");
        if let Some(iterator) = iterator {
            self.live.push(iterator.to_owned());
        }
        self.jump(&step);
        self.label(&step);
        enter_body(self, &body_label, &exhausted);
        self.loops.push(LoopTarget {
            step: step.clone(),
            done: done.clone(),
            iterator: iterator.map(str::to_owned),
            cleanup_depth: self.exits.len(),
        });
        self.statements(body);
        self.loops.pop();
        self.jump(&step);
        self.label(&exhausted);
        if iterator.is_some() {
            self.live.pop();
        }
        self.statements(otherwise);
        self.jump(&done);
        self.label(&done);
    }

    fn raise(&mut self, exc: &str, cause: Option<&str>, reraise: bool) {
        // Raising always transfers control to the enclosing handler.
        let op = self
            .output
            .data(if reraise { b"reraise" } else { b"raise" });
        let explicit = if cause.is_some() { 2 } else { 0 };
        let cause = cause.unwrap_or("0");
        self.line(&format!(
            "call i64 @imre_dyn_op_at(ptr %ctx, ptr {op}, i64 {exc}, i64 {cause}, i64 {explicit}, ptr null)"
        ));
        self.frame.jump(&self.body.error_target.clone());
    }

    fn try_except(&mut self, stmt: &ast::StmtTry) {
        let handler = self.name("b");
        let done = self.name("b");
        let outer = std::mem::replace(&mut self.body.error_target, handler.clone());
        self.statements(&stmt.body);
        self.body.error_target = outer;
        self.statements(&stmt.orelse);
        self.jump(&done);
        self.label(&handler);
        let exc = self.op("take_error", &[]);
        self.live.push(exc.clone());
        self.caught.push(exc.clone());
        if stmt.is_star {
            self.except_star_handlers(&stmt.handlers, &exc, &done);
        } else {
            self.except_handlers(&stmt.handlers, &exc, &done);
        }
        self.caught.pop();
        self.live.pop();
        self.label(&done);
    }

    fn except_handlers(&mut self, handlers: &[ast::ExceptHandler], exc: &str, done: &str) {
        for h in handlers {
            let ast::ExceptHandler::ExceptHandler(h) = h;
            let next = self.name("b");
            if let Some(class) = &h.type_ {
                self.checkpoint(class.as_ref());
                let class = self.expr(class);
                let matches = self.op("matches", &[exc, &class]);
                let matches = self.value(&format!("icmp eq i64 {matches}, 2"));
                let body = self.name("b");
                self.branch(&matches, &body, &next);
                self.label(&body);
            }
            if let Some(name) = &h.name {
                self.bind(name.as_str(), exc);
                let failed = self.name("b");
                let outer = std::mem::replace(&mut self.body.error_target, failed.clone());
                self.exits
                    .push((Cleanup::ExceptionName(name.to_string()), outer.clone()));
                self.statements(&h.body);
                self.exits.pop();
                self.body.error_target = outer;
                if !self.body.terminated {
                    self.clear_exception_name(name.as_str());
                }
                self.jump(done);
                self.label(&failed);
                let error = self.op("take_error", &[]);
                self.clear_exception_name(name.as_str());
                self.raise(&error, None, true);
            } else {
                self.statements(&h.body);
                self.jump(done);
            }
            self.label(&next);
        }
        self.raise(exc, None, true);
    }

    fn except_star_handlers(&mut self, handlers: &[ast::ExceptHandler], exc: &str, done: &str) {
        let raised = self.op("list", &[]);
        self.live.push(raised.clone());
        let roots = self.live.len();
        let mut remainder = exc.to_owned();
        let mut active = exc.to_owned();
        for ast::ExceptHandler::ExceptHandler(handler) in handlers {
            let matcher = handler.type_.as_deref().expect("except* requires a type");
            self.live.extend([remainder.clone(), active.clone()]);
            self.checkpoint(handler);
            let matcher = self.expr(matcher);
            let pair = self.op("except_star_split", &[&remainder, &matcher]);
            let zero = self.integer(0);
            let matched = self.op("get", &[&pair, &zero]);
            let one = self.integer(1);
            remainder = self.op("get", &[&pair, &one]);

            let has_match = self.value(&format!("icmp ne i64 {matched}, 0"));
            // Later match expressions see the last subgroup handled, even across an empty match.
            active = self.value(&format!(
                "select i1 {has_match}, i64 {matched}, i64 {active}"
            ));
            *self.caught.last_mut().unwrap() = active.clone();
            self.live.truncate(roots);
            self.live.extend([remainder.clone(), active.clone()]);

            let body = self.name("b");
            let failed = self.name("b");
            let next = self.name("b");
            self.branch(&has_match, &body, &next);
            self.label(&body);
            self.op("restore_caught", &[&matched]);
            if let Some(name) = &handler.name {
                self.bind(name.as_str(), &matched);
            }
            let outer = std::mem::replace(&mut self.body.error_target, failed.clone());
            self.statements(&handler.body);
            self.body.error_target = outer;
            if !self.body.terminated
                && let Some(name) = &handler.name
            {
                self.clear_star_target(name.as_str());
            }
            self.jump(&next);
            self.label(&failed);
            let error = self.op("take_error", &[]);
            if let Some(name) = &handler.name {
                self.clear_star_target(name.as_str());
            }
            self.op("append", &[&raised, &error]);
            self.jump(&next);
            self.label(&next);
            self.live.truncate(roots);
        }

        self.op("append", &[&raised, &remainder]);
        let merged = self.op("except_star_merge", &[exc, &raised]);
        self.live.pop();
        let enclosing = self
            .caught
            .iter()
            .rev()
            .nth(1)
            .cloned()
            .unwrap_or_else(|| "0".into());
        self.op("restore_caught", &[&enclosing]);
        let propagate = self.name("b");
        let handled = self.value(&format!("icmp eq i64 {merged}, 0"));
        self.branch(&handled, done, &propagate);
        self.label(&propagate);
        self.raise(&merged, None, true);
    }

    /// A custom namespace observes both the None store and the deletion, outside the arm body.
    fn clear_star_target(&mut self, name: &str) {
        self.bind(name, "0");
        let key = self.code_name(name);
        self.frame.op("delname", &[&self.scope.clone(), &key]);
    }

    /// Remove a handler's binding even if it was changed or deleted by its body.
    fn clear_exception_name(&mut self, name: &str) {
        let name = self.code_name(name);
        self.frame.op("clearname", &[&self.scope.clone(), &name]);
    }

    /// Run the same cleanup for normal fallthrough, a pending exception and nonlocal exits.
    fn try_finally(&mut self, stmt: &ast::StmtTry) {
        let handler = self.name("b");
        let done = self.name("b");
        let outer = std::mem::replace(&mut self.body.error_target, handler.clone());
        self.exits.push((
            Cleanup::Finally {
                body: stmt.finalbody.as_slice().into(),
                loop_depth: self.loops.len(),
                caught_depth: self.caught.len(),
            },
            outer.clone(),
        ));
        if stmt.handlers.is_empty() {
            self.statements(&stmt.body);
        } else {
            self.try_except(stmt);
        }
        self.exits.pop();
        self.body.error_target = outer;
        if !self.body.terminated || self.module_annotation_sites.is_some() {
            self.statements(&stmt.finalbody);
            self.jump(&done);
        }

        self.label(&handler);
        let exception = self.op("take_error", &[]);
        self.live.push(exception.clone());
        let enclosing = self.caught.last().cloned().unwrap_or_else(|| "0".into());
        self.exits.push((
            Cleanup::PendingValue {
                value: exception.clone(),
                enclosing_exception: Some(enclosing),
            },
            self.body.error_target.clone(),
        ));
        self.caught.push(exception.clone());
        self.statements(&stmt.finalbody);
        self.caught.pop();
        self.exits.pop();
        self.live.pop();
        if !self.body.terminated {
            self.raise(&exception, None, true);
        }
        self.label(&done);
    }

    /// Collect discarded owners while retaining the operands needed after the transfer.
    pub(super) fn return_from_scope(&mut self, value: String) {
        self.live.push(value.clone());
        self.run_cleanups(0, Some(&value));
        self.live.pop();
        self.return_value(&value);
    }

    /// A return, break or continue inside a finally may replace the pending exit.
    fn run_cleanups(&mut self, depth: usize, pending: Option<&str>) {
        let active = self.exits.clone();
        let mut discarded = Vec::new();
        for index in (depth..active.len()).rev() {
            if self.body.terminated {
                break;
            }
            self.exits.truncate(index);
            let (cleanup, outer) = &active[index];
            let previous = std::mem::replace(&mut self.body.error_target, outer.clone());
            match cleanup {
                Cleanup::With(context) => {
                    if context.is_async {
                        let slot = self
                            .live
                            .iter()
                            .rposition(|v| v == &context.callable)
                            .unwrap();
                        discarded.push((slot, self.live.remove(slot)));
                    }
                    self.exit_context(context, None);
                }
                Cleanup::ExceptionName(name) => self.clear_exception_name(name),
                Cleanup::PendingValue {
                    value,
                    enclosing_exception,
                } => {
                    if let Some(caught) = enclosing_exception {
                        self.op("restore_caught", &[caught]);
                    }
                    let slot = self.live.iter().rposition(|live| live == value).unwrap();
                    discarded.push((slot, self.live.remove(slot)));
                    self.release_target(None);
                }
                Cleanup::Finally {
                    body,
                    loop_depth,
                    caught_depth,
                } => {
                    // Loops and handlers entered inside the try do not enclose its finally.
                    let protected_loops = self.loops.split_off(*loop_depth);
                    let protected_handlers = self.caught.split_off(*caught_depth);
                    if let Some(value) = pending.filter(|value| *value != "0") {
                        self.exits.push((
                            Cleanup::PendingValue {
                                value: value.into(),
                                enclosing_exception: None,
                            },
                            outer.clone(),
                        ));
                    }
                    self.statements(body);
                    self.caught.extend(protected_handlers);
                    self.loops.extend(protected_loops);
                }
            }
            self.body.error_target = previous;
        }
        for (slot, value) in discarded.into_iter().rev() {
            self.live.insert(slot, value);
        }
        self.exits = active;
    }

    fn set_statement_line(&mut self, line: usize) {
        if !self.output.declarations.declared_statement_line {
            self.output
                .functions
                .push_str("declare void @imre_dyn_set_statement_line(ptr, i32)\n");
            self.output.declarations.declared_statement_line = true;
        }
        self.line(&format!(
            "call void @imre_dyn_set_statement_line(ptr %ctx, i32 {line})"
        ));
    }

    fn exit_context(&mut self, context: &ContextExit, exception: Option<&str>) -> String {
        let args = [
            context.callable.as_str(),
            exception.unwrap_or("0"),
            context.caught.as_str(),
        ];
        if !context.is_async {
            self.set_statement_line(self.source.positions.line(context.range.start()));
            return self.op("exit_context", &args);
        }

        // A pending return, outer managers and the body exception survive this await.
        // The exceptional exit also retains its callable through the truth test.
        let outer_roots = self.live.len();
        self.live.extend(exception.map(String::from));
        self.live.push(context.callable.clone());
        self.caught
            .push(exception.unwrap_or(&context.caught).into());
        self.checkpoint(&context.range);
        self.caught.pop();
        self.live.truncate(outer_roots);
        self.call_position(context.range);
        let awaitable = self.op("async_exit_context", &args);
        let mark = if exception.is_some() {
            outer_roots + 2
        } else {
            outer_roots
        }
        .to_string();
        self.keep_operands(&mark, &awaitable, "0");
        let result = self.await_at(&awaitable, "2", context.range, &mark);
        if let Some(exc) = exception {
            self.call_position(context.range);
            let suppress = self.op("truth", &[&result]);
            self.keep_operands(&outer_roots.to_string(), exc, &suppress);
            self.op("restore_caught", &[&context.caught]);
            suppress
        } else {
            self.discard_truth(&mark);
            "0".into()
        }
    }

    fn with(&mut self, items: &[ast::WithItem], body: &[ast::Stmt], is_async: bool) {
        let Some((item, rest)) = items.split_first() else {
            self.statements(body);
            return;
        };
        let range = item.context_expr.range();
        let acquire = |f: &mut Self, mark: Option<&str>| {
            let manager = f.expr(&item.context_expr);
            let mode = if is_async { "2" } else { "0" };
            let lookup = |f: &mut Self, name| {
                if is_async {
                    f.call_position(range);
                }
                f.op(name, &[&manager, mode])
            };
            let exit = lookup(f, "lookup_exit");
            let enter = lookup(f, "lookup_enter");
            let above_exit = mark.map(|mark| f.keep_operands(mark, &exit, &enter));
            if is_async {
                f.call_position(range);
            }
            let mut value = f.invoke(&enter, &[]);
            if let Some(mark) = &above_exit {
                f.keep_operands(mark, &value, "0");
                value = f.await_at(&value, "1", range, mark);
            }
            (exit, value, above_exit)
        };
        let (exit, value, above_exit) = if is_async {
            let mark = self.temporary_mark();
            self.unwind_operands(&mark, |f| acquire(f, Some(&mark)))
        } else {
            acquire(self, None)
        };
        let handler = self.name("b");
        let done = self.name("b");
        let outer = std::mem::replace(&mut self.body.error_target, handler.clone());
        let caught = self.caught.last().cloned().unwrap_or_else(|| "0".into());
        let context = ContextExit {
            range,
            is_async,
            callable: exit.clone(),
            caught,
        };
        self.exits
            .push((Cleanup::With(context.clone()), outer.clone()));
        self.live.push(exit);
        if let Some(target) = &item.optional_vars {
            self.assign(target, &value);
        }
        if let Some(mark) = above_exit {
            // The assignment consumed the enter result before the next item is evaluated.
            self.discard_truth(&mark);
        }
        self.with(rest, body, is_async);
        self.live.pop();
        self.exits.pop();
        self.body.error_target = outer;
        if !self.body.terminated {
            self.exit_context(&context, None);
            self.jump(&done);
        }
        self.label(&handler);
        let exc = self.op("take_error", &[]);
        let result = self.exit_context(&context, Some(&exc));
        let suppress = self.truth(&result);
        let propagate = self.name("b");
        self.branch(&suppress, &done, &propagate);
        self.label(&propagate);
        self.raise(&exc, None, true);
        self.label(&done);
    }
}

/// Names that can refer directly to a type without calling Python.
fn scalar_reference(expr: &ast::Expr) -> Option<String> {
    match expr {
        ast::Expr::NoneLiteral(_) => Some("None".to_owned()),
        ast::Expr::Name(name) if !name.id.starts_with("__") => Some(name.id.to_string()),
        ast::Expr::Attribute(attr) => {
            let module = attr.value.as_name_expr()?;
            (!module.id.starts_with("__") && !attr.attr.starts_with("__"))
                .then(|| format!("{}.{}", module.id, attr.attr))
        }
        _ => None,
    }
}
