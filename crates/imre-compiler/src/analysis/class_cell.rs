//! Class scopes bind these cells for their children (CPython's analyze_block).
use ruff_db::files::File;
use ruff_python_ast::{AnyNodeRef, find_node::covering_node};
use ruff_python_parser::semantic_errors::{SemanticSyntaxError, SemanticSyntaxErrorKind};
use ty_python_core::scope::{NodeWithScopeRef, ScopeKind};
use ty_python_semantic::Db;

pub(super) fn binds_nonlocal(db: &dyn Db, file: File, error: &SemanticSyntaxError) -> bool {
    let SemanticSyntaxErrorKind::NonlocalWithoutBinding(name) = &error.kind else {
        return false;
    };
    if !matches!(
        name.as_str(),
        "__class__" | "__classdict__" | "__conditional_annotations__"
    ) {
        return false;
    }
    let program = db.program_file(file);
    let parsed = ruff_db::parsed::parsed_module(db, program.python_file(db)).load(db);
    let nodes = covering_node(parsed.syntax().into(), error.range);
    let Some(owner) = nodes.ancestors().find_map(|node| match node {
        AnyNodeRef::StmtFunctionDef(f) => Some(NodeWithScopeRef::Function(f)),
        AnyNodeRef::StmtClassDef(c) => Some(NodeWithScopeRef::Class(c)),
        _ => None,
    }) else {
        return false;
    };
    let index = ty_python_core::semantic_index(db, program);
    for (id, scope) in index.ancestor_scopes(index.node_scope(owner)).skip(1) {
        if scope.kind() == ScopeKind::Class {
            return true;
        }
        if let Some(symbol) = index.place_table(id).symbol_by_name(name)
            && (symbol.is_global() || (scope.kind() == ScopeKind::TypeParams && symbol.is_bound()))
        {
            return false;
        }
    }
    false
}
