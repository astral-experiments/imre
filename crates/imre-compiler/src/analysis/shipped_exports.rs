//! Resolve a demanded member in the selected runtime source, using the current ty database.
use super::*;
use crate::modules::Binding;
use ruff_db::files::FileRange;
use ruff_python_ast::name::Name;
use ty_python_core::definition::{Definition, DefinitionKind, DefinitionState};
use ty_python_semantic::types::TypeDefinition;

impl Session {
    /// Check the class bound by the selected runtime declaration.
    pub(crate) fn is_exception_declaration(&self, site: FileRange) -> bool {
        use ruff_python_ast::{AnyNodeRef, find_node::covering_node};
        use ty_python_semantic::types::{KnownClass, Type};

        let parsed = self.parsed(site.file());
        let name = covering_node(parsed.syntax().into(), site.range());
        let Some(AnyNodeRef::StmtClassDef(class)) = name.parent() else {
            return false;
        };
        let db = &self.db;
        let model = SemanticModel::new(db, db.program_file(site.file()));
        let env = model.program_environment();
        class.name.range() == site.range()
            && matches!(class.inferred_type(&model), Some(ty @ Type::ClassLiteral(_))
                if ty.is_assignable_to(db, &env, KnownClass::BaseException.to_subclass_of(db, &env)))
    }

    pub(crate) fn class_declaration(&self, id: crate::modules::DefinitionId) -> FileRange {
        let file = self.files[id.module];
        let parsed = self.parsed(file);
        let class = parsed.suite()[id.statement]
            .as_class_def_stmt()
            .expect("registered class declaration");
        FileRange::new(file, class.name.range())
    }

    pub(crate) fn prepare_shipped_source(&mut self, owner: &str) -> Result<Option<File>, String> {
        let Some((filename, body)) = crate::dynamic::bundled_module_source(owner, false) else {
            return Ok(None);
        };
        self.prepare_shipped_file(owner, filename, body).map(Some)
    }

    fn prepare_shipped_file(
        &mut self,
        owner: &str,
        filename: &str,
        body: &str,
    ) -> Result<File, String> {
        let root = Path::new(crate::dynamic::BUNDLED_SOURCE_ROOT);
        let path = root.join(filename);
        let origin = path.to_str().expect("SDK filename");
        if let Ok(file) = system_path_to_file(&self.db, origin) {
            return Ok(file);
        }
        self.seed(&path, body)?;
        let file = system_path_to_file(&self.db, origin).map_err(|error| error.to_string())?;
        if filename.ends_with("/__init__.py") {
            for (_, child) in star_imports(&self.db, file, Name::new(owner)).clone() {
                // The initializer imports these files. Install their bytes in the same
                // session; member resolution happens at a from-import or attribute read.
                if let Some((filename, body)) =
                    crate::dynamic::bundled_module_source(child.as_str(), false)
                {
                    let path = root.join(filename);
                    if system_path_to_file(&self.db, path.to_str().expect("SDK filename")).is_err()
                    {
                        self.seed(&path, body)?;
                    }
                }
            }
        }
        Ok(file)
    }

    /// The checked ABI uses the leading declarations of the same module that boxed callers import.
    pub(crate) fn checked_shipped_prefix(
        &mut self,
        owner: &str,
        last: &str,
    ) -> Result<&'static str, String> {
        let (filename, source) = crate::modules::bundled_runtime_source(owner)
            .ok_or_else(|| format!("no checked runtime provider selected for {owner:?}"))?;
        let file = self.prepare_shipped_file(owner, filename, source)?;
        // A user file at the SDK spelling still owns its declarations. Select the
        // immutable provider in a separate file before using offsets in its bytes.
        let file = if self.file_matches_parser_input(file, source) {
            file
        } else {
            self.virtual_source(&format!("imre/stdlib/{filename}"), source)
        };
        let definition = self
            .final_module_binding(file, last)
            .ok_or_else(|| format!("missing checked definition {owner}.{last}"))?;
        let end = definition
            .kind(&self.db)
            .full_range(&self.parsed(file))
            .end()
            .to_usize();
        let newline = source.as_bytes().get(end) == Some(&b'\n');
        source
            .get(..end + usize::from(newline))
            .ok_or_else(|| format!("invalid checked definition range for {owner}.{last}"))
    }

    pub(super) fn final_module_binding(&self, file: File, name: &str) -> Option<Definition<'_>> {
        let mut defs = bindings(&self.db, file, name);
        let def = defs.next()?.definition()?;
        (defs.next().is_none() && def.file(&self.db) == file).then_some(def)
    }

    pub(crate) fn demanded_shipped_declaration(
        &mut self,
        owner: &str,
        name: &str,
    ) -> Result<Option<Binding>, String> {
        Ok(self
            .prepare_shipped_source(owner)?
            .and_then(|file| self.shipped_declaration_at(file, owner, name)))
    }

    /// Read a package's docstring from the same parsed source as its exports.
    pub(crate) fn shipped_module_docstring(&self, owner: &str) -> Option<String> {
        let (filename, _) = crate::dynamic::bundled_module_source(owner, false)?;
        let file = shipped_file(&self.db, filename)?;
        crate::frontend::docstring(self.parsed(file).suite())
    }

    pub(crate) fn shipped_declaration(&self, owner: &str, name: &str) -> Option<Binding> {
        let (filename, _) = crate::dynamic::bundled_module_source(owner, false)?;
        let file = shipped_file(&self.db, filename)?;
        self.shipped_declaration_at(file, owner, name)
    }

    pub(crate) fn shipped_declaration_at(
        &self,
        file: File,
        owner: &str,
        name: &str,
    ) -> Option<Binding> {
        *selected_declaration(&self.db, file, Name::new(owner), Name::new(name))
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
) -> (TextSize, Option<Binding>) {
    let mut result = None;
    let mut last = TextSize::default();
    let mut valid = true;
    let mut may_use_accelerator = false;
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
                may_use_accelerator = true;
                continue;
            }
            _ => Some(def),
        };
        let selected = source.and_then(|source| match source.kind(db) {
            DefinitionKind::Class(_) => {
                Some(Binding::RegisteredClass(source.focus_range(db, parsed)))
            }
            DefinitionKind::Function(_) => {
                Some(Binding::RegisteredValue(source.focus_range(db, parsed)))
            }
            DefinitionKind::Assignment(assign)
                if !name.starts_with('_') && assign.value(parsed).is_call_expr() =>
            {
                // A type variable created here has its own declaration. Import its live value.
                let model = SemanticModel::new(db, db.program_file(file));
                let ty = assign.value(parsed).inferred_type(&model)?;
                let declaration = ty.definition(db, &model.program_environment());
                (declaration == Some(TypeDefinition::TypeVar(source)))
                    .then(|| Binding::RegisteredValue(source.focus_range(db, parsed)))
            }
            _ => None,
        });
        valid &= result.is_none() && selected.is_some();
        result = selected;
    }
    (
        last,
        result.filter(|binding| {
            valid && (!may_use_accelerator || matches!(binding, Binding::RegisteredClass(_)))
        }),
    )
}

#[salsa::tracked(returns(ref), no_eq)]
fn selected_declaration(db: &dyn Db, file: File, owner: Name, name: Name) -> Option<Binding> {
    let parsed = ruff_db::parsed::parsed_module(db, db.program_file(file).python_file(db)).load(db);
    let (last, mut selected) = local_declaration(db, file, &parsed, name.as_str());
    if file.path(db).as_str().ends_with("/__init__.py") {
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
