//! Checks Imre's exact source representations independently of lowering capability.
//!
//! ty supplies Python assignability diagnostics. These checks cover the additional fixed
//! representation promise at written declarations, where Python subtyping may be looser.

use super::*;
use ty_python_core::definition::DefinitionKind;
use ty_python_core::scope::NodeWithScopeKind;
use ty_python_semantic::types::KnownClass;

impl Session {
    pub(super) fn exact_source_contracts(
        &self,
        module: usize,
        path: &Path,
        skipped: Option<&[Vec<TextRange>]>,
        declarations: DeclaredSignatures<'_>,
        errors: &mut Vec<(TextSize, String)>,
    ) {
        let db = &self.db;
        let file = self.files[module];
        let source = db.program_file(file);
        let index = ty_python_core::semantic_index(db, source);
        let parsed = self.parsed(file);
        let model = SemanticModel::new(db, source);
        let excluded = |scope, value: &Expr| {
            index.is_in_type_checking_block(scope, value.range())
                || skipped.is_some_and(|ranges| {
                    ranges[module]
                        .iter()
                        .any(|r| r.contains_range(value.range()))
                })
        };
        for scope in index.scope_ids() {
            let owner = scope.node(db);
            let scope = scope.file_scope_id(db);
            // Class annotations may be interpreted by a metaclass as schema fields.
            if matches!(owner, NodeWithScopeKind::Class(_)) {
                continue;
            }
            if let NodeWithScopeKind::Function(function) = owner {
                let function = function.node(&parsed);
                if let Some(annotation) = function.returns.as_deref()
                    && !scope.is_generator_function(index)
                {
                    for ret in explicit_returns(function) {
                        if let Some(value) = ret.value.as_deref().filter(|v| !excluded(scope, v)) {
                            self.exact_declared_operand(
                                &model, annotation, &model, value, path, errors,
                            );
                        }
                    }
                }
            }
            let definitions = index.use_def_map(scope);
            for (_, definition, _) in definitions.definitions_with_usage() {
                let (annotation, value) = match definition.kind(db) {
                    DefinitionKind::AnnotatedAssignment(a) if a.target(&parsed).is_name_expr() => {
                        (Some(a.annotation(&parsed)), a.value(&parsed))
                    }
                    DefinitionKind::Assignment(a) if a.unpack().is_none() => {
                        let Some(name) = a.target(&parsed).as_name_expr() else {
                            continue;
                        };
                        let Some(symbol) = index.place_table(scope).symbol_id(name.id.as_str())
                        else {
                            continue;
                        };
                        let mut annotations = definitions
                            .reachable_symbol_declarations(symbol)
                            .filter_map(|d| d.declaration.definition())
                            .filter_map(|d| match d.kind(db) {
                                DefinitionKind::AnnotatedAssignment(a) => {
                                    Some(a.annotation(&parsed))
                                }
                                _ => None,
                            })
                            .filter(|ann| ann.start() < a.value(&parsed).start());
                        let first = annotations.next();
                        (
                            first.filter(|_| annotations.next().is_none()),
                            Some(a.value(&parsed)),
                        )
                    }
                    _ => continue,
                };
                if let (Some(annotation), Some(value)) = (annotation, value)
                    && !excluded(scope, value)
                {
                    self.exact_declared_operand(&model, annotation, &model, value, path, errors);
                }
            }
        }
        // Only a uniquely resolved, undecorated first-party function has a fixed source
        // signature. Rebound or dynamically selected Python callables keep their semantics.
        ruff_python_ast::helpers::any_over_body(parsed.suite(), |expr| {
            let Expr::Call(call) = expr else { return false };
            let Some(scope) = index.try_expression_scope_id(expr) else {
                return false;
            };
            if excluded(scope, expr) {
                return false;
            }
            let Some(name) = call.func.as_name_expr() else {
                return false;
            };
            let Some(definition) =
                resolved_name_definition(&model, name, ImportAliasResolution::ResolveAliases)
            else {
                return false;
            };
            if !declarations.call_shapes(db).contains(&definition)
                || changed_module_binding(db, &model, call, name, definition)
            {
                return false;
            }
            let DefinitionKind::Function(function) = definition.kind(db) else {
                return false;
            };
            let owner = definition.file(db);
            let owner_parsed = self.parsed(owner);
            let function = function.node(&owner_parsed);
            let owner_model = SemanticModel::new(db, db.program_file(owner));
            for argument in imre_ast::evaluated_arguments(&call.arguments) {
                if argument.is_variadic() {
                    break;
                }
                let value = argument.value();
                for (position, parameter) in
                    function.parameters.iter_non_variadic_params().enumerate()
                {
                    if let Some(annotation) = parameter.parameter.annotation.as_deref()
                        && crate::frontend::known_declared_argument(
                            &call.arguments,
                            &function.parameters,
                            position,
                            parameter.parameter.name.as_str(),
                        )
                        .is_some_and(|bound| bound.range() == value.range())
                    {
                        self.exact_declared_operand(
                            &owner_model,
                            annotation,
                            &model,
                            value,
                            path,
                            errors,
                        );
                    }
                }
                // Python evaluates arguments in order. A call or descriptor can exit
                // before a later argument is evaluated, so it cannot prove that site.
                let passive_call = value.as_call_expr().is_some_and(|nested| {
                    let Some(name) = nested.func.as_name_expr() else {
                        return false;
                    };
                    let Some(target) = resolved_name_definition(
                        &model,
                        name,
                        ImportAliasResolution::ResolveAliases,
                    ) else {
                        return false;
                    };
                    let DefinitionKind::Function(function) = target.kind(db) else {
                        return false;
                    };
                    let body = self.parsed(target.file(db));
                    passive_function(function.node(&body))
                        && imre_ast::evaluated_arguments(&nested.arguments).all(|arg| {
                            !arg.is_variadic()
                                && (arg.value().is_literal_expr() || arg.value().is_name_expr())
                        })
                });
                if !value.is_literal_expr() && !value.is_name_expr() && !passive_call {
                    break;
                }
            }
            false
        });
    }

    fn exact_declared_operand(
        &self,
        owner: &SemanticModel<'_>,
        annotation: &Expr,
        model: &SemanticModel<'_>,
        value: &Expr,
        path: &Path,
        errors: &mut Vec<(TextSize, String)>,
    ) {
        let db = &self.db;
        let Some(expected) = annotation_type(db, owner, annotation) else {
            return;
        };
        let Some(actual) = value.inferred_type(model) else {
            return;
        };
        if let Some(ty_python_semantic::types::Type::TypeAlias(alias)) =
            annotation.inferred_type(owner)
        {
            with_alias_value(
                db,
                &owner.program_environment(),
                alias,
                |_, alias_model, expression, _| {
                    self.exact_declared_operand(
                        alias_model,
                        expression,
                        model,
                        value,
                        path,
                        errors,
                    );
                },
            );
            return;
        }
        let env = model.program_environment();
        // ty widens a float annotation to float | int, while Imre promises an
        // exact float representation. Preserve explicitly written union arms first.
        let float = KnownClass::Float.to_instance(db, &env);
        let widened_float = ty_python_semantic::types::UnionType::from_two_elements(
            db,
            &env,
            KnownClass::Int.to_instance(db, &env),
            float,
        );
        let float_definition = float.definition(db, &env);
        let float_contract = expected == widened_float
            || float_definition.is_some() && expected.definition(db, &env) == float_definition;
        let exact_expected = if float_contract { float } else { expected };
        // ty permits numeric promotion at a float declaration even when its raw
        // inferred operand is not assignable to the nominal float instance.
        let promoted_float = float_contract
            && matches!(
                Self::scalar_layout(db, &env, actual, ScalarMatch::Assignable),
                Some(crate::ir::Type::Int | crate::ir::Type::Bool)
            );
        // Other non-assignable types already have a ty declaration diagnostic.
        if (actual.is_assignable_to(db, &env, expected) || promoted_float)
            && !Self::written_scalar_union_accepts(owner, annotation, actual)
            && let Some(error) =
                Self::scalar_contract_error(model, exact_expected, value, actual, || true)
        {
            errors.push((value.start(), error.with_path(path).to_string()));
            return;
        }
        let Expr::Subscript(subscript) = annotation else {
            return;
        };
        let canonical = |class: KnownClass| {
            let class = if class == KnownClass::Tuple {
                class.to_subclass_of(db, &env)
            } else {
                class.to_instance(db, &env)
            };
            let target = class.definition(db, &env);
            target.is_some()
                && (expected.definition(db, &env) == target
                    || subscript
                        .value
                        .inferred_type(owner)
                        .and_then(|base| base.definition(db, &env))
                        == target)
        };
        match value {
            Expr::List(list) if canonical(KnownClass::List) => {
                for item in &list.elts {
                    self.exact_declared_operand(owner, &subscript.slice, model, item, path, errors);
                }
            }
            Expr::Set(set) if canonical(KnownClass::Set) => {
                for item in &set.elts {
                    self.exact_declared_operand(owner, &subscript.slice, model, item, path, errors);
                }
            }
            Expr::Dict(dict) if canonical(KnownClass::Dict) => {
                if let Expr::Tuple(arguments) = subscript.slice.as_ref()
                    && let [key_type, value_type] = arguments.elts.as_slice()
                {
                    for item in &dict.items {
                        if let Some(key) = &item.key {
                            self.exact_declared_operand(owner, key_type, model, key, path, errors);
                            self.exact_declared_operand(
                                owner,
                                value_type,
                                model,
                                &item.value,
                                path,
                                errors,
                            );
                        }
                    }
                }
            }
            Expr::Tuple(tuple) if canonical(KnownClass::Tuple) => match subscript.slice.as_ref() {
                Expr::Tuple(arguments)
                    if matches!(arguments.elts.as_slice(), [_, Expr::EllipsisLiteral(_)]) =>
                {
                    for item in &tuple.elts {
                        self.exact_declared_operand(
                            owner,
                            &arguments.elts[0],
                            model,
                            item,
                            path,
                            errors,
                        );
                    }
                }
                Expr::Tuple(arguments) if arguments.elts.len() == tuple.elts.len() => {
                    for (annotation, item) in arguments.elts.iter().zip(&tuple.elts) {
                        self.exact_declared_operand(owner, annotation, model, item, path, errors);
                    }
                }
                annotation if tuple.elts.len() == 1 => self.exact_declared_operand(
                    owner,
                    annotation,
                    model,
                    &tuple.elts[0],
                    path,
                    errors,
                ),
                _ => {}
            },
            _ => {}
        }
    }
}
