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
        self.prepare_shipped_file(
            Path::new(crate::dynamic::BUNDLED_SOURCE_ROOT),
            owner,
            filename,
            |session, path| session.seed(path, body),
        )
        .map(Some)
    }

    /// Make the checked child visible beside its source package in the project.
    pub(crate) fn project_shipped_child(&mut self, root: &Path, owner: &str) -> Result<(), String> {
        let Some((filename, _)) = crate::modules::bundled_runtime_source(owner) else {
            return Ok(());
        };
        self.prepare_shipped_file(root, owner, filename, |session, path| {
            let (_, file) = session.checked_shipped_file(owner, None)?;
            let input = ruff_db::source::source_text(&session.db, file);
            session.seed_parser_input(path, &input)
        })?;
        Ok(())
    }

    fn prepare_shipped_file(
        &mut self,
        root: &Path,
        owner: &str,
        filename: &str,
        seed: impl FnOnce(&mut Self, &Path) -> Result<(), String>,
    ) -> Result<File, String> {
        let path = root.join(filename);
        let origin = path.to_str().expect("SDK filename");
        if let Ok(file) = system_path_to_file(&self.db, origin) {
            return Ok(file);
        }
        seed(self, &path)?;
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

    pub(crate) fn checked_shipped_file(
        &mut self,
        owner: &str,
        private_path: Option<&Path>,
    ) -> Result<(&'static str, File), String> {
        let (filename, source) = crate::modules::bundled_runtime_source(owner)
            .ok_or_else(|| format!("no checked runtime provider selected for {owner:?}"))?;
        let input = shipped_parser_input(source)?;
        let file = self.prepare_shipped_file(
            Path::new(crate::dynamic::BUNDLED_SOURCE_ROOT),
            owner,
            filename,
            |session, path| session.seed_parser_input(path, &input),
        )?;
        // A user file at the SDK spelling still owns its declarations. Select the
        // immutable provider in a separate file before using offsets in its bytes.
        let file = if self.file_matches_parser_input(file, &input) {
            file
        } else if let Some(path) = private_path {
            let path = path.to_str().ok_or("module paths must be valid UTF-8")?;
            self.write_file(path, &input)?
        } else {
            self.virtual_shipped_file(filename, || Ok(input))?
        };
        Ok((source, file))
    }

    pub(crate) fn checked_shipped_function(
        &mut self,
        owner: &str,
        name: &str,
        private_path: &Path,
    ) -> Result<(&'static str, FileRange), String> {
        let (source, file) = self.checked_shipped_file(owner, Some(private_path))?;
        let Some(Binding::RegisteredValue(site)) = self.shipped_declaration_at(file, owner, name)
        else {
            return Err(format!("missing checked definition {owner}.{name}"));
        };
        if site.file() != file {
            return Err(format!(
                "checked function {owner}.{name} belongs to another module"
            ));
        }
        Ok((source, site))
    }

    /// Register the immutable module under the original top-level statement index.
    pub(crate) fn add_shipped_declaration(
        &mut self,
        site: FileRange,
        path: &Path,
        source: &str,
    ) -> Result<(ParsedModuleRef, usize), String> {
        Self::with_checked_input(path, source, |path, input| {
            let (parsed, spreads) = Self::checked_parse(
                self.parsed(site.file()),
                path,
                source,
                input,
                crate::SourceKind::File,
            )?;
            let suite = parsed.suite();
            let index = suite.partition_point(|stmt| stmt.end() <= site.range().start());
            let name = suite.get(index).and_then(crate::modules::declaration_name);
            if name.is_none_or(|name| name.range() != site.range()) {
                return Err(format!(
                    "{path}: checked binding is not a module-level declaration"
                ));
            }
            self.files.push(site.file());
            self.tuple_spreads.push(spreads);
            Ok((parsed, index))
        })
    }

    /// Resolve the unannotated positional inputs of a shipped implementation in its public stub.
    pub(crate) fn positional_contract(
        &self,
        interface: &str,
        function: &ruff_python_ast::StmtFunctionDef,
        types: &crate::types::Types,
    ) -> Option<Vec<crate::ir::Type>> {
        let db = &self.db;
        let source = &function.parameters;
        let positional = source.posonlyargs.iter().chain(&source.args);
        if positional.clone().all(|p| p.annotation().is_some()) {
            return None;
        }
        let file = ruff_db::files::vendored_path_to_file(db, interface).ok()?;
        let definition = self.final_module_binding(file, function.name.as_str())?;
        let DefinitionKind::Function(declaration) = definition.kind(db) else {
            return None;
        };
        let parsed = self.parsed(file);
        let stub = &declaration.node(&parsed).parameters;
        if source.posonlyargs.len() != stub.posonlyargs.len()
            || source.args.len() != stub.args.len()
        {
            return None;
        }
        let model = SemanticModel::new(db, db.program_file(file));
        positional
            .zip(stub.posonlyargs.iter().chain(&stub.args))
            .map(|(parameter, declared)| {
                if parameter.name().as_str() != declared.name().as_str() {
                    return None;
                }
                Self::contract_layout(&model, declared.annotation()?, types)
            })
            .collect()
    }

    fn contract_layout(
        model: &SemanticModel<'_>,
        expression: &Expr,
        types: &crate::types::Types,
    ) -> Option<crate::ir::Type> {
        use ty_python_semantic::types::Type;

        let db = model.db();
        let ty = annotation_type(db, model, expression)?;
        let nested = |expr: &Expr| Self::contract_layout(model, expr, types).ok_or(());
        match (ty, expression) {
            (Type::Callable(_), Expr::Subscript(subscript)) => {
                crate::frontend::callable_annotation(types, &subscript.slice, nested, |_| ()).ok()
            }
            (Type::Union(_), Expr::BinOp(union))
                if union.op == ruff_python_ast::Operator::BitOr =>
            {
                crate::frontend::union_annotation(types, union, nested, || ()).ok()
            }
            _ => Self::scalar_layout(db, &model.program_environment(), ty, ScalarMatch::Exact),
        }
    }

    pub(super) fn final_module_binding(&self, file: File, name: &str) -> Option<Definition<'_>> {
        let mut defs = bindings(&self.db, file, name);
        let def = defs.next()?.definition()?;
        (defs.next().is_none() && def.file(&self.db) == file).then_some(def)
    }

    /// The initializer selects the object when every binding at normal exit is an import.
    pub(crate) fn imports_module_member(&self, module: usize, name: &str) -> bool {
        let mut reaching = bindings(&self.db, self.files[module], name).peekable();
        reaching.peek().is_some()
            && reaching.all(|state| {
                state.is_defined_and(|definition| {
                    matches!(
                        definition.kind(&self.db),
                        DefinitionKind::Import(_) | DefinitionKind::ImportFrom(_)
                    )
                })
            })
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

    fn virtual_shipped_file<'a>(
        &self,
        filename: &str,
        input: impl FnOnce() -> Result<Cow<'a, str>, String>,
    ) -> Result<File, String> {
        let key = format!("imre/stdlib/{filename}");
        let path = ruff_db::system::SystemVirtualPath::new(&key);
        if let Some(file) = self.db.files.try_virtual_file(path) {
            return Ok(file.file());
        }
        Ok(self.db.virtual_source(&key, &input()?))
    }

    /// Only a unique unconditional module class supplies a fixed declaration site.
    pub(crate) fn shipped_runtime_export(
        &self,
        filename: &str,
        source: &str,
        name: &str,
    ) -> Option<Binding> {
        let file = self
            .virtual_shipped_file(filename, || shipped_parser_input(source))
            .ok()?;
        let db = &self.db;
        let mut reaching = None;
        let mut definite = true;
        for state in bindings(db, file, name) {
            if let Some(definition) = state.definition() {
                definite &= reaching.replace(definition).is_none();
            } else {
                definite = false;
            }
        }
        let definition = reaching?;
        if !definite || !matches!(definition.kind(db), DefinitionKind::Class(_)) {
            return Some(Binding::ConditionalValue);
        }
        let parsed = self.parsed(file);
        let site = definition.focus_range(db, &parsed);
        let suite = parsed.suite();
        let stmt = suite.get(suite.partition_point(|stmt| stmt.end() <= site.range().start()))?;
        Some(
            if stmt
                .as_class_def_stmt()
                .is_some_and(|class| class.name.range() == site.range())
            {
                Binding::RegisteredClass(site)
            } else {
                Binding::ConditionalValue
            },
        )
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

fn shipped_parser_input(source: &str) -> Result<Cow<'_, str>, String> {
    Ok(
        match crate::normalize_source(source, crate::SourceKind::File)? {
            Cow::Borrowed(text) => imre_ast::indentation::prepare(text).text,
            Cow::Owned(text) => Cow::Owned(imre_ast::indentation::prepare(&text).text.into_owned()),
        },
    )
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
    if Path::new(file.path(db).as_str()).ends_with("__init__.py") {
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
