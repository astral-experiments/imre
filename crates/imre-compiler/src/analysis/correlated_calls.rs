//! Preserve the relationship between a selected callable and arguments assigned
//! beside it in the same match arm. Ty widens those local variables separately.

use super::{OperandTypes, parameter_labels, primary_range};
use ruff_db::{diagnostic::Diagnostic, files::File};
use ruff_python_ast::{
    AnyNodeRef, Expr, ExprContext, Stmt,
    find_node::covering_node,
    visitor::{Visitor, walk_expr, walk_stmt},
};
use ruff_text_size::{Ranged, TextRange};
use ty_python_core::definition::DefinitionKind;
use ty_python_semantic::{
    Db, HasType, SemanticModel,
    types::{Type, TypeDefinition},
};

pub(super) fn explains(db: &dyn Db, file: File, diagnostic: &Diagnostic) -> bool {
    if !diagnostic.id().is_lint_named("invalid-argument-type") {
        return false;
    }
    let Some(range) = primary_range(diagnostic, file) else {
        return false;
    };
    let parsed = ruff_db::parsed::parsed_module(db, db.program_file(file).python_file(db)).load(db);
    let nodes = covering_node(parsed.syntax().into(), range);
    let Some(call) = nodes.ancestors().find_map(|node| match node {
        AnyNodeRef::ExprCall(call) if call.arguments.range().contains_range(range) => Some(call),
        _ => None,
    }) else {
        return false;
    };
    let Some(callee) = call.func.as_name_expr() else {
        return false;
    };
    let model = SemanticModel::new(db, db.program_file(file));
    let Some(Type::Union(union)) = call.func.inferred_type(&model) else {
        return false;
    };
    if !union
        .elements(db)
        .iter()
        .all(|ty| matches!(ty, Type::FunctionLiteral(_)))
    {
        return false;
    }
    let argument = call
        .arguments
        .args
        .iter()
        .find(|arg| arg.range().contains_range(range))
        .and_then(Expr::as_name_expr)
        .map(|name| (name.id.as_str(), false))
        .or_else(|| {
            call.arguments
                .keywords
                .iter()
                .find(|keyword| keyword.arg.is_none() && keyword.range().contains_range(range))
                .and_then(|keyword| keyword.value.as_name_expr())
                .map(|name| (name.id.as_str(), true))
        });
    let Some((argument, spread)) = argument else {
        return false;
    };
    let Some(caller) = nodes.ancestors().find_map(|node| match node {
        AnyNodeRef::StmtFunctionDef(function) => Some(function),
        _ => None,
    }) else {
        return false;
    };

    parameter_labels(diagnostic).any(|(owner, label)| {
        let source = ruff_db::parsed::parsed_module(db, db.program_file(owner).python_file(db)).load(db);
        let nodes = covering_node(source.syntax().into(), label);
        let Some(parameter) = nodes.ancestors().find_map(|node| match node {
            AnyNodeRef::Parameter(parameter) => Some(parameter),
            _ => None,
        }) else { return false };
        let Some(function) = nodes.ancestors().find_map(|node| match node {
            AnyNodeRef::StmtFunctionDef(function) => Some(function),
            _ => None,
        }) else { return false };
        let owner_model = SemanticModel::new(db, db.program_file(owner));
        let Some(expected) = parameter.inferred_type(&owner_model) else { return false };
        caller.body.iter().filter_map(Stmt::as_match_stmt).any(|selection| {
            if selection.end() >= call.start() || overwritten(caller, selection.range(), call.range(), callee.id.as_str(), argument) {
                return false;
            }
            if !selection.cases.last().is_some_and(|case| {
                case.guard.is_none()
                    && case.pattern.is_irrefutable()
                    && matches!(case.body.as_slice(), [Stmt::Raise(_)])
            }) || selection.cases.iter().any(|case| case.guard.is_some()) {
                return false;
            }
            let mut matched = false;
            for case in &selection.cases {
                if matches!(case.body.as_slice(), [Stmt::Raise(_)]) { continue }
                let [Stmt::Assign(assign)] = case.body.as_slice() else { return false };
                let [Expr::Tuple(targets)] = assign.targets.as_slice() else { return false };
                let Expr::Tuple(values) = assign.value.as_ref() else { return false };
                if targets.elts.len() != values.elts.len() { return false }
                let binding = |name: &str| targets.elts.iter().position(|target| target.as_name_expr().is_some_and(|n| n.id.as_str() == name));
                let Some(selected) = binding(callee.id.as_str()).and_then(|i| values.elts.get(i)) else { return false };
                let Some(TypeDefinition::Function(definition)) = selected.inferred_type(&model).and_then(|ty| ty.definition(db, &model.program_environment())) else { return false };
                let DefinitionKind::Function(definition_node) = definition.kind(db) else { return false };
                if definition.file(db) != owner || definition_node.node(&source).range() != function.range() { continue }
                matched = true;
                let Some(value) = binding(argument).and_then(|i| values.elts.get(i)) else { return false };
                let value = if spread {
                    let Expr::Dict(dict) = value else { return false };
                    let Some(item) = dict.items.iter().find(|item| matches!(item.key.as_ref(), Some(Expr::StringLiteral(key)) if key.value.to_str() == parameter.name.as_str())) else { return false };
                    &item.value
                } else { value };
                if !value.inferred_type(&model).is_some_and(|actual| match actual {
                    Type::Union(union) => union.elements(db).iter().copied().all(|ty| {
                        OperandTypes::accepts_type(&owner_model, ty, expected)
                            || ty.is_none(db) && rejects_none(db, owner, &owner_model, function, parameter.name.as_str(), expected)
                    }),
                    actual => OperandTypes::accepts_type(&owner_model, actual, expected),
                }) { return false }
            }
            matched
        })
    })
}

fn rejects_none<'db>(
    db: &'db dyn Db,
    owner: File,
    model: &SemanticModel<'db>,
    function: &ruff_python_ast::StmtFunctionDef,
    parameter: &str,
    expected: Type<'db>,
) -> bool {
    for statement in &function.body {
        if let Stmt::Match(selection) = statement
            && matches!(selection.subject.as_ref(), Expr::Name(name) if name.id.as_str() == parameter)
        {
            let Some((fallback, cases)) = selection.cases.split_last() else {
                return false;
            };
            return !cases.is_empty()
                && fallback.guard.is_none()
                && fallback.pattern.is_irrefutable()
                && matches!(fallback.body.as_slice(), [Stmt::Raise(_)])
                && super::DiagnosticRules::get(db)
                    .profile_exclusions(db)
                    .get(owner.path(db).as_str())
                    .is_some_and(|ranges| {
                        ranges
                            .iter()
                            .any(|range| range.contains_range(fallback.body[0].range()))
                    })
                && cases.iter().all(|case| {
                    if case.guard.is_some() {
                        return false;
                    }
                    let ruff_python_ast::Pattern::MatchValue(pattern) = &case.pattern else {
                        return false;
                    };
                    pattern.value.inferred_type(model).is_some_and(|ty| {
                        matches!(ty, Type::LiteralValue(literal) if literal.is_enum())
                            && OperandTypes::accepts_type(model, ty, expected)
                    })
                });
        }
        struct Reads<'n> {
            name: &'n str,
            found: bool,
        }
        impl<'a> Visitor<'a> for Reads<'_> {
            fn visit_expr(&mut self, expr: &'a Expr) {
                if matches!(expr, Expr::Name(name) if name.id.as_str() == self.name && name.ctx == ExprContext::Load)
                {
                    self.found = true;
                }
                walk_expr(self, expr);
            }
        }
        let mut reads = Reads {
            name: parameter,
            found: false,
        };
        reads.visit_stmt(statement);
        if reads.found {
            return false;
        }
    }
    false
}

fn overwritten(
    caller: &ruff_python_ast::StmtFunctionDef,
    selection: TextRange,
    call: TextRange,
    callee: &str,
    argument: &str,
) -> bool {
    struct Writes<'a> {
        selection: TextRange,
        call: TextRange,
        callee: &'a str,
        argument: &'a str,
        found: bool,
    }
    impl<'a> Visitor<'a> for Writes<'_> {
        fn visit_stmt(&mut self, stmt: &'a Stmt) {
            if stmt.end() <= self.selection.end() || stmt.start() >= self.call.start() {
                return;
            }
            if !matches!(stmt, Stmt::FunctionDef(_) | Stmt::ClassDef(_)) {
                walk_stmt(self, stmt);
            }
        }
        fn visit_expr(&mut self, expr: &'a Expr) {
            if expr.start() >= self.call.start() {
                return;
            }
            // Even a read can expose or alias a mutable spread dictionary before the call.
            if matches!(expr, Expr::Name(name) if name.id.as_str() == self.callee || name.id.as_str() == self.argument)
            {
                self.found = true
            }
            walk_expr(self, expr);
        }
    }
    let mut writes = Writes {
        selection,
        call,
        callee,
        argument,
        found: false,
    };
    for stmt in &caller.body {
        writes.visit_stmt(stmt);
    }
    writes.found
}
