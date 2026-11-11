//! Resolve a demanded member in the selected runtime source, using the current ty database.
use super::*;
use ruff_db::files::FileRange;
use ruff_python_ast::name::Name;
use ty_python_core::definition::{Definition, DefinitionKind, DefinitionState};

impl Session {
    pub(crate) fn class_declaration(&self, id: crate::modules::DefinitionId) -> FileRange {
        let file = self.files[id.module];
        let parsed = self.parsed(file);
        let class = parsed.suite()[id.statement]
            .as_class_def_stmt()
            .expect("registered class declaration");
        FileRange::new(file, class.name.range())
    }

    pub(crate) fn prepare_shipped_source(&mut self, owner: &str) -> Result<(), String> {
        let Some((filename, body)) = crate::dynamic::bundled_module_source(owner, false) else {
            return Ok(());
        };
        let root = Path::new(crate::dynamic::BUNDLED_SOURCE_ROOT);
        let path = root.join(filename);
        if system_path_to_file(&self.db, path.to_str().expect("SDK filename")).is_ok() {
            return Ok(());
        }
        self.seed(&path, &body)?;
        if filename.ends_with("/__init__.py") {
            let (file, _, _) = self.selected_file(&path)?;
            for (_, child) in star_imports(&self.db, file, Name::new(owner)).clone() {
                // The initializer imports these files. Install their bytes in the same
                // session; member resolution happens at a from-import or attribute read.
                if let Some((filename, body)) =
                    crate::dynamic::bundled_module_source(child.as_str(), false)
                {
                    let path = root.join(filename);
                    if system_path_to_file(&self.db, path.to_str().expect("SDK filename")).is_err()
                    {
                        self.seed(&path, &body)?;
                    }
                }
            }
        }
        Ok(())
    }

    pub(super) fn final_module_binding(&self, file: File, name: &str) -> Option<Definition<'_>> {
        let mut defs = bindings(&self.db, file, name);
        let def = defs.next()?.definition()?;
        (defs.next().is_none() && def.file(&self.db) == file).then_some(def)
    }

    pub(crate) fn shipped_class(&self, owner: &str, name: &str) -> Option<FileRange> {
        let (filename, _) = crate::dynamic::bundled_module_source(owner, false)?;
        let file = shipped_file(&self.db, filename)?;
        *selected_class(&self.db, file, Name::new(owner), Name::new(name))
    }
}

fn shipped_file(db: &dyn Db, filename: &str) -> Option<File> {
    system_path_to_file(
        db,
        Path::new(crate::dynamic::BUNDLED_SOURCE_ROOT)
            .join(filename)
            .to_str()?,
    )
    .ok()
}

#[salsa::tracked(returns(ref), no_eq)]
fn star_imports(db: &dyn Db, file: File, owner: Name) -> Vec<(TextRange, Name)> {
    let parsed = ruff_db::parsed::parsed_module(db, db.program_file(file).python_file(db)).load(db);
    parsed
        .suite()
        .iter()
        .filter_map(Stmt::as_import_from_stmt)
        .filter_map(|import| {
            if import.level != 1 || !import.names.iter().any(|alias| alias.name.as_str() == "*") {
                return None;
            }
            Some((
                import.range(),
                Name::new(crate::modules::resolve_import(
                    owner.as_str(),
                    import.level,
                    import.module.as_ref()?.as_str(),
                )?),
            ))
        })
        .collect()
}

fn bindings<'db>(
    db: &'db dyn Db,
    file: File,
    name: &str,
) -> impl Iterator<Item = DefinitionState<'db>> + 'db {
    let index = ty_python_core::semantic_index(db, db.program_file(file));
    let scope = FileScopeId::global();
    index
        .place_table(scope)
        .symbol_id(name)
        .into_iter()
        .flat_map(move |symbol| {
            index
                .use_def_map(scope)
                .end_of_scope_symbol_bindings(symbol)
                .map(|b| b.binding)
        })
}

pub(super) fn binding_at_use<'db>(
    db: &'db dyn Db,
    file: File,
    expression: &Expr,
) -> Option<Definition<'db>> {
    let index = ty_python_core::semantic_index(db, db.program_file(file));
    let scope = index.try_expression_scope_id(expression)?;
    let use_id = index.try_expression_use_id(expression.into())?;
    let mut definitions = index.use_def_map(scope).bindings_at_use(use_id);
    let first = definitions.next()?.binding.definition()?;
    definitions.next().is_none().then_some(first)
}

/// A known literal __all__ determines the names copied by this wildcard.
fn star_contains(db: &dyn Db, file: File, parsed: &ParsedModuleRef, name: &str) -> Option<bool> {
    let mut defs = bindings(db, file, "__all__");
    let Some(first) = defs.next() else {
        // Without __all__, star import copies the public module dictionary.
        return Some(
            !name.starts_with('_') && bindings(db, file, name).any(|s| s.definition().is_some()),
        );
    };
    let first = first.definition()?;
    let DefinitionKind::Assignment(assign) = first.kind(db) else {
        return None;
    };
    if defs.next().is_some() {
        return None;
    }
    // Mutable lists can be changed without a second assignment to __all__.
    let items = &assign.value(parsed).as_tuple_expr()?.elts;
    items.iter().all(Expr::is_string_literal_expr).then(|| {
        items.iter().any(|item| {
            item.as_string_literal_expr()
                .is_some_and(|s| s.value.to_str() == name)
        })
    })
}

/// Keep the ordinary class when the initializer may install its optional accelerator.
fn local_declaration(
    db: &dyn Db,
    file: File,
    parsed: &ParsedModuleRef,
    name: &str,
) -> (TextSize, Option<FileRange>) {
    let mut result = None;
    let mut last = TextSize::default();
    let mut valid = true;
    for state in bindings(db, file, name) {
        let Some(def) = state.definition() else {
            return (TextSize::new(u32::MAX), None);
        };
        if matches!(def.kind(db), DefinitionKind::StarImport(_)) {
            continue;
        }
        last = last.max(def.kind(db).full_range(parsed).end());
        let source = match def.kind(db) {
            DefinitionKind::Assignment(assign) if assign.value(parsed).is_name_expr() => {
                binding_at_use(db, file, assign.value(parsed))
            }
            DefinitionKind::Assignment(assign)
                if assign
                    .value(parsed)
                    .as_attribute_expr()
                    .is_some_and(|attr| {
                        attr.attr.as_str() == name
                            && binding_at_use(db, file, &attr.value)
                                .is_some_and(|d| matches!(d.kind(db), DefinitionKind::Import(_)))
                    }) =>
            {
                continue;
            }
            _ => Some(def),
        };
        let selected = source
            .filter(|source| matches!(source.kind(db), DefinitionKind::Class(_)))
            .map(|source| source.focus_range(db, parsed));
        valid &= result.is_none() && selected.is_some();
        result = selected;
    }
    (last, result.filter(|_| valid))
}

#[salsa::tracked(returns(ref), no_eq)]
fn selected_class(db: &dyn Db, file: File, owner: Name, name: Name) -> Option<FileRange> {
    let (filename, _) = crate::dynamic::bundled_module_source(owner.as_str(), false)?;
    let parsed = ruff_db::parsed::parsed_module(db, db.program_file(file).python_file(db)).load(db);
    let (last, mut selected) = local_declaration(db, file, &parsed, name.as_str());
    if filename.ends_with("/__init__.py") {
        // A subsequent star can replace a local binding. Walk in reverse source
        // order and stop at the last provider that actually exports this name.
        for (_, child) in star_imports(db, file, owner)
            .iter()
            .rev()
            .take_while(|(r, _)| r.start() >= last)
        {
            let (path, _) = crate::dynamic::bundled_module_source(child.as_str(), false)?;
            let child = shipped_file(db, path)?;
            let parsed =
                ruff_db::parsed::parsed_module(db, db.program_file(child).python_file(db)).load(db);
            match star_contains(db, child, &parsed, name.as_str()) {
                Some(false) => continue,
                None if bindings(db, child, name.as_str()).next().is_none() => continue,
                None => return None,
                Some(true) => {}
            }
            (_, selected) = local_declaration(db, child, &parsed, name.as_str());
            break;
        }
    }
    selected
}
