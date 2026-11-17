//! First-party scalar storage contracts, independent of the selected representation.

use super::*;
use ty_python_core::definition::DefinitionKind;
use ty_python_core::scope::ScopeKind;

/// One indexed name store and the written declaration that constrains it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ScalarStore {
    pub(super) value: TextRange,
    pub(super) annotation: TextRange,
    pub(super) exact_error: Option<String>,
}

/// Read declarations at each binding from ty's semantic index. A clean ty body check
/// does not prove the stricter representation contract (notably bool versus int).
#[salsa::tracked(returns(ref))]
pub(super) fn scalar_stores(db: &dyn LiteralSourceDb, file: File) -> Vec<ScalarStore> {
    let source = db.program_file(file);
    let index = ty_python_core::semantic_index(db, source);
    let parsed = ruff_db::parsed::parsed_module(db, source.python_file(db)).load(db);
    let model = SemanticModel::new(db, source);
    let env = model.program_environment();
    let path = Path::new(file.path(db).as_str());
    let mut stores = Vec::new();
    for scope in index.scope_ids() {
        let scope = scope.file_scope_id(db);
        // Class namespace annotations can be interpreted by a metaclass as schema fields.
        if !matches!(
            index.scope(scope).kind(),
            ScopeKind::Module | ScopeKind::Function
        ) {
            continue;
        }
        let map = index.use_def_map(scope);
        for (_, definition, _) in map.definitions_with_usage() {
            let (target, value, annotation) = match definition.kind(db) {
                DefinitionKind::AnnotatedAssignment(a) => {
                    let Some(value) = a.value(&parsed) else {
                        continue;
                    };
                    (a.target(&parsed), value, Some(a.annotation(&parsed)))
                }
                DefinitionKind::Assignment(a) if a.unpack().is_none() => {
                    let annotation = map.declarations_at_binding(definition).find_map(|entry| {
                        let declaration = entry.declaration.definition()?;
                        if declaration.file(db) != file {
                            return None;
                        }
                        match declaration.kind(db) {
                            DefinitionKind::AnnotatedAssignment(a) => Some(a.annotation(&parsed)),
                            DefinitionKind::Parameter(p) => p.annotation(&parsed),
                            _ => None,
                        }
                    });
                    (a.target(&parsed), a.value(&parsed), annotation)
                }
                _ => continue,
            };
            let Some(annotation) = annotation else {
                continue;
            };
            if !target.is_name_expr() || index.is_in_type_checking_block(scope, value.range()) {
                continue;
            }
            let Some(expected) = annotation_type(db, &model, annotation) else {
                continue;
            };
            if !scalar_store_type(db, &env, expected) {
                continue;
            }
            let exact_error = value.inferred_type(&model).and_then(|actual| {
                // The diagnostic merge prefers a real ty failure at the same operand.
                if Session::written_scalar_union_accepts(&model, annotation, actual) {
                    return None;
                }
                let error = Session::scalar_store_contract_error(&model, expected, value, actual)?;
                Some(error.with_path(path).to_string())
            });
            stores.push(ScalarStore {
                value: value.range(),
                annotation: annotation.range(),
                exact_error,
            });
        }
    }
    stores.sort_by_key(|store| store.value.start());
    stores.dedup_by(|a, b| a.value == b.value && a.annotation == b.annotation);
    stores
}
