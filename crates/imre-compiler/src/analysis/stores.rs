//! Recover written contracts from the selected source bindings. Inferred locals
//! and fields have no such contract, even when ty gives them a concrete type.

use super::{
    Session, annotation_type, concrete_scalar, contract_initializer, resolved_name_definition,
    with_alias_value,
};
use crate::frontend::phase::Diagnostic;
use ruff_db::{files::File, parsed::ParsedModuleRef};
use ruff_python_ast::{AnyNodeRef, Expr, ExprSubscript, find_node::covering_node};
use ruff_text_size::{Ranged, TextRange};
use ty_python_core::{
    SemanticIndex,
    definition::{Definition, DefinitionKind},
    scope::{FileScopeId, NodeWithScopeKind},
};
use ty_python_semantic::{
    Db, HasType, ResolvedDefinition, SemanticModel,
    types::{KnownClass, SpecialFormType, Type, ide_support::ImportAliasResolution},
};

#[derive(Clone, Copy)]
pub(super) struct Store<'db, 'a> {
    binding: Option<Definition<'db>>,
    pub(super) target: &'a Expr,
    /// An augmented store writes the operator's result. Only ty knows its type.
    pub(super) value: Option<&'a Expr>,
}

pub(super) fn for_each_store<'db, 'a>(
    index: &SemanticIndex<'db>,
    node: AnyNodeRef<'a>,
    mut visit: impl FnMut(Store<'db, 'a>),
) {
    match node {
        AnyNodeRef::StmtAnnAssign(a) if a.value.is_some() => visit(Store {
            binding: index.try_definition(a),
            target: &a.target,
            value: a.value.as_deref(),
        }),
        AnyNodeRef::StmtAssign(a) => {
            for target in &a.targets {
                assigned(index, target, &a.value, &mut visit);
            }
        }
        AnyNodeRef::StmtFor(a) => assigned(index, &a.target, &a.target, &mut visit),
        AnyNodeRef::StmtAugAssign(a) => visit(Store {
            binding: index.try_definition(a),
            target: &a.target,
            value: None,
        }),
        AnyNodeRef::ExprNamed(a) => visit(Store {
            binding: index.try_definition(a),
            target: &a.target,
            value: Some(&a.value),
        }),
        _ => {}
    }
}

/// ty records the unpacked value type on each leaf target, even for a
/// nonliteral RHS. Avoid comparing the whole iterable to one binding.
fn assigned<'db, 'a>(
    index: &SemanticIndex<'db>,
    target: &'a Expr,
    value: &'a Expr,
    visit: &mut impl FnMut(Store<'db, 'a>),
) {
    match target {
        Expr::List(ruff_python_ast::ExprList { elts, .. })
        | Expr::Tuple(ruff_python_ast::ExprTuple { elts, .. }) => {
            for leaf in elts {
                assigned(index, leaf, leaf, visit);
            }
        }
        Expr::Starred(star) => assigned(index, &star.value, &star.value, visit),
        _ => visit(Store {
            binding: index
                .try_definitions(target.into())
                .and_then(|defs| unique(defs.iter().copied())),
            target,
            value: Some(value),
        }),
    }
}

fn unique<T>(mut values: impl Iterator<Item = T>) -> Option<T> {
    values.next().filter(|_| values.next().is_none())
}

/// A write in this scope uses its reaching declaration. A deferred write
/// through global or nonlocal needs a unique declaration in the owning scope.
pub(super) fn reaching_declaration<'db>(
    db: &'db dyn Db,
    binding: Definition<'db>,
) -> Option<Definition<'db>> {
    let file = binding.file(db);
    let index = ty_python_core::semantic_index(db, db.program_file(file));
    let scope = binding.file_scope(db);
    let symbol = index
        .place_table(scope)
        .symbol(binding.place(db).as_symbol()?);
    let owner = if !scope.is_global() && symbol.is_global() {
        let global = FileScopeId::global();
        Some((global, index.place_table(global).symbol_id(symbol.name())?))
    } else if symbol.is_nonlocal() {
        // A closer unannotated local or parameter also owns the cell.
        let mut parents = index.ancestor_scopes(scope).skip(1);
        loop {
            let (id, outer) = parents.next()?;
            if outer.kind() == ty_python_core::scope::ScopeKind::Class
                && super::class_cell::implicit_cell(symbol.name())
            {
                return None;
            }
            if !outer.kind().is_function_like() {
                continue;
            }
            let table = index.place_table(id);
            let Some(place) = table.symbol_id(symbol.name()) else {
                continue;
            };
            let entry = table.symbol(place);
            if entry.is_global() {
                return None;
            }
            if entry.is_local() {
                break Some((id, place));
            }
        }
    } else {
        None
    };
    let mut declarations = if let Some((outer, id)) = owner {
        let definitions = index.use_def_map(outer);
        // The write may run before or after a redeclaration. Never select
        // the final annotation alone when an earlier one could apply.
        if definitions
            .reachable_symbol_declarations(id)
            .filter_map(|d| d.declaration.definition())
            .nth(1)
            .is_some()
        {
            return None;
        }
        definitions.end_of_scope_symbol_declarations(id)
    } else {
        index.use_def_map(scope).declarations_at_binding(binding)
    };
    let declaration = declarations.next()?.declaration.definition()?;
    if declarations.next().is_some() || declaration.file(db) != file {
        return None;
    }
    Some(declaration)
}

fn declaration<'db>(
    model: &SemanticModel<'db>,
    target: &Expr,
    binding: Option<Definition<'db>>,
) -> Option<Definition<'db>> {
    let db = model.db();
    let index = ty_python_core::semantic_index(db, model.program_file());
    let scope = index.try_expression_scope_id(target)?;
    if index.is_in_type_checking_block(scope, target.range()) {
        return None;
    }
    // A base, metaclass or decorator can give a field declaration a different
    // meaning. A plain class namespace follows the ordinary store rules.
    if let NodeWithScopeKind::Class(class) = index.scope(scope).node() {
        let parsed =
            ruff_db::parsed::parsed_module(db, model.program_file().python_file(db)).load(db);
        let class = class.node(&parsed);
        if class.arguments.is_some()
            || !class.decorator_list.is_empty()
            || class.type_params.is_some()
        {
            return None;
        }
    }
    if let Some(binding) = binding
        && matches!(binding.kind(db), DefinitionKind::AnnotatedAssignment(_))
    {
        return Some(binding);
    }
    match target {
        Expr::Name(name) => {
            let binding = binding.or_else(|| {
                resolved_name_definition(model, name, ImportAliasResolution::ResolveAliases)
            })?;
            if matches!(
                binding.kind(db),
                DefinitionKind::AnnotatedAssignment(_) | DefinitionKind::Parameter(_)
            ) {
                return Some(binding);
            }
            reaching_declaration(db, binding)
        }
        Expr::Attribute(attr) => {
            // An annotated member of one arm cannot speak for a different
            // receiver whose field is unannotated.
            if matches!(attr.value.inferred_type(model)?, Type::Union(_)) {
                return None;
            }
            let defs = ty_python_semantic::definitions_for_attribute(model, attr);
            unique(defs.iter().filter_map(|d| match d {
                ResolvedDefinition::Definition(d)
                    if matches!(d.kind(db), DefinitionKind::AnnotatedAssignment(_)) =>
                {
                    Some(*d)
                }
                _ => None,
            }))
        }
        Expr::Subscript(sub) => declaration(model, &sub.value, None),
        _ => None,
    }
}

impl<'db> Store<'db, '_> {
    fn with_annotation<T>(
        self,
        model: &SemanticModel<'db>,
        files: &[File],
        check: impl FnOnce(&SemanticModel<'db>, &Expr) -> Option<T>,
    ) -> Option<T> {
        let db = model.db();
        let def = declaration(model, self.target, self.binding)?;
        if def.file(db) != model.file() && !files.contains(&def.file(db)) {
            return None;
        }
        let parsed = ruff_db::parsed::parsed_module(db, def.python_file(db)).load(db);
        let annotation = match def.kind(db) {
            DefinitionKind::AnnotatedAssignment(a) => a.annotation(&parsed),
            DefinitionKind::Parameter(p) => p.annotation(&parsed)?,
            _ => return None,
        };
        check(&SemanticModel::new(db, def.program_file(db)), annotation)
    }

    pub(super) fn error(self, model: &SemanticModel<'db>, files: &[File]) -> Option<Diagnostic> {
        let value = self.value?;
        self.with_annotation(model, files, |owner, annotation| {
            let mut items = Vec::new();
            if !self.binding.is_some_and(|binding| {
                matches!(
                    binding.kind(model.db()),
                    DefinitionKind::AnnotatedAssignment(_)
                )
            }) {
                let mut target = self.target;
                while let Expr::Subscript(sub) = target {
                    items.push(sub);
                    target = &sub.value;
                }
            }
            value_error(model, owner, annotation, value, &items)
        })
    }
}

/// Keep ty's assignability diagnostics only where the selected binding carries
/// a declaration. In particular, ty checks the computed result of augmented stores.
pub(super) fn assignment_declaration(
    db: &dyn Db,
    file: File,
    parsed: &ParsedModuleRef,
    range: TextRange,
    files: &[File],
) -> Option<(File, TextRange)> {
    if let Some(annotation) = contract_initializer(db, file, parsed, range) {
        return Some((file, annotation));
    }
    let source = db.program_file(file);
    let model = SemanticModel::new(db, source);
    let index = ty_python_core::semantic_index(db, source);
    let nodes = covering_node(parsed.syntax().into(), range);
    nodes.ancestors().find_map(|node| {
        let mut result = None;
        for_each_store(index, node, |store| {
            if range == node.range()
                || range == store.target.range()
                || store.value.is_some_and(|value| range == value.range())
            {
                // ty locates an invalid indexed write on the entire statement.
                // Keep the more precise key or value error when we have one.
                if store.target.is_subscript_expr() && store.error(&model, files).is_some() {
                    return;
                }
                result = result.or_else(|| {
                    store.with_annotation(&model, files, |owner, annotation| {
                        // Preserve the existing exact scalar diagnostic for name reassignments.
                        let declared = annotation_type(db, owner, annotation)?;
                        if store.value.is_some()
                            && store.target.is_name_expr()
                            && concrete_scalar(db, &owner.program_environment(), declared)
                        {
                            return None;
                        }
                        Some((owner.file(), annotation.range()))
                    })
                });
            }
        });
        result
    })
}

/// ty can infer a display using its expected type (e.g. [True] as list[int]).
/// Project written builtin slots onto their operands before that widening. All
/// type resolution and scalar comparisons still use the same semantic models.
pub(super) fn display_operands<'a>(
    model: &SemanticModel<'_>,
    annotation: &'a Expr,
    value: &'a Expr,
) -> Option<Vec<(&'a Expr, &'a Expr)>> {
    let generic = annotation.as_subscript_expr()?;
    let class = match value {
        Expr::List(_) => KnownClass::List,
        Expr::Set(_) => KnownClass::Set,
        Expr::Tuple(_) => KnownClass::Tuple,
        Expr::Dict(_) => KnownClass::Dict,
        _ => return None,
    };
    let db = model.db();
    let env = model.program_environment();
    let origin = annotation_type(db, model, annotation)?.definition(db, &env)?;
    if class.to_subclass_of(db, &env).definition(db, &env)? != origin {
        return None;
    }
    let slots = match generic.slice.as_ref() {
        Expr::Tuple(t) => t.elts.as_slice(),
        one => std::slice::from_ref(one),
    };
    let (items, element) = match (value, slots) {
        (Expr::List(list), [slot]) => (&list.elts, slot),
        (Expr::Set(set), [slot]) => (&set.elts, slot),
        (Expr::Tuple(tuple), [slot, Expr::EllipsisLiteral(_)]) => (&tuple.elts, slot),
        (Expr::Tuple(tuple), slots)
            if slots.len() == tuple.elts.len() && !tuple.elts.iter().any(Expr::is_starred_expr) =>
        {
            return Some(tuple.elts.iter().zip(slots).collect());
        }
        (Expr::Dict(dict), [key, item]) => {
            let mut pairs = Vec::new();
            for entry in &dict.items {
                pairs.extend([(entry.key.as_ref()?, key), (&entry.value, item)]);
            }
            return Some(pairs);
        }
        _ => return None,
    };
    Some(items.iter().map(|item| (item, element)).collect())
}

/// Contextual inference can widen a fresh container to its annotated type,
/// erasing distinctions such as bool versus int. Project the written element
/// contracts only for the resolved builtin containers, then check each operand
/// with the same scalar rule as a top-level store or return.
pub(super) fn value_error<'db>(
    model: &SemanticModel<'db>,
    owner: &SemanticModel<'db>,
    annotation: &Expr,
    value: &Expr,
    items: &[&ExprSubscript],
) -> Option<Diagnostic> {
    let db = model.db();
    let env = owner.program_environment();
    // Joining the alternatives can erase a scalar distinction before it
    // reaches a declared destination (for example, True and 1 join to int).
    if let Expr::If(branch) = value {
        let unreachable =
            ty_python_semantic::types::ide_support::unreachable_ranges(db, model.program_file());
        return [&*branch.body, &*branch.orelse]
            .into_iter()
            .filter(|arm| {
                !unreachable
                    .iter()
                    .any(|r| r.range.contains_range(arm.range()))
            })
            .find_map(|arm| value_error(model, owner, annotation, arm, items));
    }
    let expected = annotation_type(db, owner, annotation)?;
    if let Some(Type::TypeAlias(alias)) = annotation.inferred_type(owner) {
        return with_alias_value(db, &env, alias, |_, owner, rhs, _| {
            value_error(model, owner, rhs, value, items)
        })?;
    }
    if let Expr::StringLiteral(text) = annotation
        && let Some((parsed, inner)) = owner.enter_string_annotation(text)
    {
        return value_error(model, &inner, &parsed.syntax().body, value, items);
    }
    // Check a nullable scalar only when the operand is known to be in
    // its non-null arm. Keep explicit scalar unions that ty simplified.
    let mut scalar = expected;
    let mut written = annotation;
    if let Type::Union(union) = expected
        && union.elements(db).iter().any(|ty| ty.is_none(db))
    {
        let mut arms = union.elements(db).iter().copied();
        scalar = arms.find(|ty| !ty.is_none(db))?;
        if arms.any(|ty| !ty.is_none(db))
            || !value
                .inferred_type(model)?
                .is_assignable_to(db, &env, scalar)
        {
            return None;
        }
        if let Expr::Subscript(sub) = annotation
            && sub.value.inferred_type(owner) == Some(Type::SpecialForm(SpecialFormType::Optional))
        {
            written = &sub.slice;
        }
    }
    if items.is_empty() && concrete_scalar(db, &env, scalar) {
        let actual = value.inferred_type(model)?;
        if Session::written_scalar_union_accepts(owner, written, actual) {
            return None;
        }
        return Session::scalar_contract_error(model, scalar, value, actual, || true);
    }
    let Expr::Subscript(sub) = annotation else {
        return None;
    };
    let class = expected.definition(db, &env)?;
    let builtin = |kind: KnownClass| {
        kind.to_subclass_of(db, &env).definition(db, &env).as_ref() == Some(&class)
    };
    let args = match sub.slice.as_ref() {
        Expr::Tuple(tuple) => tuple.elts.as_slice(),
        arg => std::slice::from_ref(arg),
    };
    let check = |annotation: &Expr, value: &Expr| value_error(model, owner, annotation, value, &[]);
    if let Some((item, tail)) = items.split_last() {
        if item.slice.is_slice_expr() {
            return None;
        }
        return match args {
            [element] if builtin(KnownClass::List) => {
                let index_error = || {
                    let actual = item.slice.inferred_type(model)?;
                    let int = KnownClass::Int.to_instance(db, &env);
                    // Other index protocols are ty's responsibility.
                    if !tail.is_empty() || !actual.is_assignable_to(db, &env, int) {
                        return None;
                    }
                    Session::scalar_contract_error(model, int, &*item.slice, actual, || true)
                };
                let element_error = || value_error(model, owner, element, value, tail);
                // An unpacked target carries its projected type. Its index is
                // checked before that projection, unlike an explicit RHS.
                if value.range() == item.range() {
                    index_error().or_else(element_error)
                } else {
                    element_error().or_else(index_error)
                }
            }
            [key, element] if builtin(KnownClass::Dict) => {
                if tail.is_empty()
                    && let Some(error) = check(key, &item.slice)
                {
                    return Some(error);
                }
                value_error(model, owner, element, value, tail)
            }
            _ => None,
        };
    }
    match (args, value) {
        ([element], Expr::ListComp(comp)) if builtin(KnownClass::List) => check(element, &comp.elt),
        ([element], Expr::SetComp(comp)) if builtin(KnownClass::Set) => check(element, &comp.elt),
        ([key, element], Expr::DictComp(comp)) if builtin(KnownClass::Dict) => {
            check(key, comp.key.as_deref()?).or_else(|| check(element, &comp.value))
        }
        _ => display_operands(owner, annotation, value)?
            .into_iter()
            .filter(|(value, _)| !value.is_starred_expr())
            .find_map(|(value, annotation)| check(annotation, value)),
    }
}
