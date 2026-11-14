//! Apply Python's lexical class-name mangling before lowering boxed modules.

/// CPython 3.14.5's pyc magic number followed by CRLF in little-endian wire order.
pub const PYC_MAGIC_NUMBER_TOKEN: u32 = 3627 | ((b'\r' as u32) << 16) | ((b'\n' as u32) << 24);

/// Default wire format shared by Python 3.14 marshal writers and the module export.
pub const MARSHAL_VERSION: i32 = 5;

/// Native archives in import order. The first filename carries the wheel ABI tag.
pub const NATIVE_ARCHIVE_EXTENSIONS: [&str; 2] = ["imre1.a", "imre.a"];

/// Python language version used by the compiler and the runtime import machinery.
pub const PYTHON_VERSION: ruff_python_ast::PythonVersion = ruff_python_ast::PythonVersion::PY314;

/// Filesystem module suffixes in discovery order; match compound suffixes before ".so".
pub fn module_suffixes() -> [String; 7] {
    let version = PYTHON_VERSION;
    let abi = if cfg!(target_os = "macos") {
        "darwin".to_owned()
    } else {
        let libc = if cfg!(target_env = "musl") {
            "musl"
        } else {
            "gnu"
        };
        format!("{}-linux-{libc}", std::env::consts::ARCH)
    };
    let tagged = format!(".cpython-{}{}-{abi}.so", version.major, version.minor);
    let [versioned, legacy] = NATIVE_ARCHIVE_EXTENSIONS.map(|ext| format!(".{ext}"));
    [
        tagged,
        ".abi3.so".into(),
        versioned,
        legacy,
        ".so".into(),
        ".py".into(),
        ".pyc".into(),
    ]
}

/// Unicode Character Database version shipped with the target CPython 3.14 release.
pub const UNICODE_DATA_VERSION: &str = "16.0.0";

pub mod comprehension;
#[cfg(target_os = "linux")]
pub mod epoll;
pub mod exceptions;
pub mod executable;
pub mod forward_refs;
pub mod indentation;
pub mod layout;
pub mod lzma;
pub mod numbers;
pub mod posixpath;
pub mod providers;
pub mod python_nodes;
pub mod source_encoding;
pub mod startup;
pub mod strings;
mod thread_entry;
pub use thread_entry::{ThreadEntry, captured_thread_locks, thread_entry};
mod type_aliases;
pub mod zlib;
pub use type_aliases::annotation_scope_error as type_alias_error;
pub use type_aliases::{TypeParameterField, annotation_scope_error, generic_class_arguments_error};

use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap, HashSet};

use ruff_python_ast::name::Name;
use ruff_python_ast::visitor::transformer::{self, Transformer};
use ruff_python_ast::{self as ast, Identifier};
use ruff_python_stdlib::identifiers::is_mangled_private;
use ruff_text_size::{Ranged, TextSize};

/// Interactive input accepts one compound statement or one logical line of simple statements.
pub fn check_single_input(
    body: &[ast::Stmt],
    tokens: &ast::token::Tokens,
) -> Result<(), &'static str> {
    use ast::token::TokenKind::{Dedent, Newline};

    let Some(first) = body.first() else {
        return Err("invalid syntax");
    };
    let mut trailing = tokens
        .iter()
        .skip_while(|token| token.start() < first.end());
    if ast::helpers::is_compound_statement(first) {
        // An indented suite ends in DEDENT. An inline suite needs a real closing newline.
        if body.len() != 1
            || !trailing.any(|token| {
                token.kind() == Dedent || (token.kind() == Newline && !token.range().is_empty())
            })
        {
            return Err("invalid syntax");
        }
    } else if let Some(last) = body.last()
        && trailing
            .take_while(|token| token.start() < last.start())
            .any(|token| token.kind() == Newline)
    {
        return Err("multiple statements found while compiling a single statement");
    }
    Ok(())
}

/// A valueless parenthesized name in an annotation neither binds nor reads the name.
pub fn annotation_visits_target(assignment: &ast::StmtAnnAssign) -> bool {
    assignment.simple || assignment.value.is_some() || !assignment.target.is_name_expr()
}

pub fn parameter_defaults(parameters: &ast::Parameters) -> impl Iterator<Item = &ast::Expr> {
    parameters
        .iter_non_variadic_params()
        .filter_map(|parameter| parameter.default.as_deref())
}

pub fn decorator_expressions(decorators: &[ast::Decorator]) -> impl Iterator<Item = &ast::Expr> {
    decorators.iter().map(|decorator| &decorator.expression)
}

/// Expressions evaluated in the enclosing scope when a function is defined, in order.
pub fn function_definition_expressions(
    function: &ast::StmtFunctionDef,
) -> impl Iterator<Item = &ast::Expr> {
    decorator_expressions(&function.decorator_list).chain(parameter_defaults(&function.parameters))
}

/// Positional and starred operands are evaluated before keyword operands.
pub fn evaluated_arguments(args: &ast::Arguments) -> impl Iterator<Item = ast::ArgOrKeyword<'_>> {
    args.args
        .iter()
        .map(ast::ArgOrKeyword::Arg)
        .chain(args.keywords.iter().map(ast::ArgOrKeyword::Keyword))
}

/// Values supplied to the call, including the operands of argument expansion.
pub fn argument_values(args: &ast::Arguments) -> impl Iterator<Item = &ast::Expr> {
    evaluated_arguments(args).map(|arg| match arg {
        ast::ArgOrKeyword::Arg(ast::Expr::Starred(star)) => star.value.as_ref(),
        _ => arg.value(),
    })
}

/// Generic class arguments run in the type-parameter scope. Decorators always run outside it.
pub fn class_definition_expressions(class: &ast::StmtClassDef) -> impl Iterator<Item = &ast::Expr> {
    let arguments = class
        .arguments
        .as_deref()
        .filter(|_| class.type_params.is_none());
    decorator_expressions(&class.decorator_list)
        .chain(arguments.into_iter().flat_map(argument_values))
}

/// Diagnose the positional-only range constructor before converting any bounds.
pub fn range_constructor_error(positional: usize, keywords: bool) -> Option<String> {
    if keywords {
        Some("range() takes no keyword arguments".into())
    } else {
        match positional {
            0 => Some("range expected at least 1 argument, got 0".into()),
            1..=3 => None,
            n => Some(format!("range expected at most 3 arguments, got {n}")),
        }
    }
}

/// Context.run takes its callable positionally and forwards the remaining arguments.
pub fn context_run_arguments<T>(positional: &[T]) -> Result<(&T, &[T]), &'static str> {
    positional
        .split_first()
        .ok_or("run() missing 1 required positional argument")
}

/// A pattern that neither inspects nor captures its operand.
pub fn wildcard_pattern(pattern: &ast::Pattern) -> bool {
    matches!(pattern, ast::Pattern::MatchAs(p) if p.pattern.is_none() && p.name.is_none())
        || matches!(pattern, ast::Pattern::MatchStar(p) if p.name.is_none())
}

/// The sequence protocol operations required by one validated sequence pattern.
#[derive(Clone)]
pub struct SequencePatternPlan {
    /// A sole star does not call len. Otherwise require exactly or at least this many items.
    pub length: Option<(ast::CmpOp, usize)>,
    pub items: SequencePatternItems,
}

#[derive(Clone)]
pub enum SequencePatternItems {
    /// Every item is a wildcard. The subject need not be indexed or iterated.
    Ignore,
    /// Fetch only inspected items on either side of this ignored star.
    IndexAround(usize),
    /// Unpack in iteration order, collecting a list at the optional capture.
    Unpack(Option<usize>),
}

/// Use the same observable length, indexing and iteration policy in both boxed backends.
pub fn sequence_pattern_plan(patterns: &[ast::Pattern]) -> SequencePatternPlan {
    let star = patterns
        .iter()
        .position(|p| matches!(p, ast::Pattern::MatchStar(_)));
    let length = match star {
        None => Some((ast::CmpOp::Eq, patterns.len())),
        Some(_) if patterns.len() > 1 => Some((ast::CmpOp::GtE, patterns.len() - 1)),
        _ => None,
    };
    let items = if patterns.iter().all(wildcard_pattern) {
        SequencePatternItems::Ignore
    } else if let Some(index) = star
        && wildcard_pattern(&patterns[index])
    {
        SequencePatternItems::IndexAround(index)
    } else {
        SequencePatternItems::Unpack(star)
    };
    SequencePatternPlan { length, items }
}

/// Wildcard imports can only publish names in a module body, including its conditional suites.
pub fn misplaced_star_import(body: &[ast::Stmt]) -> Option<ruff_text_size::TextRange> {
    #[derive(Default)]
    struct Scopes {
        nested: bool,
        invalid: Option<ruff_text_size::TextRange>,
    }
    impl<'a> ast::visitor::Visitor<'a> for Scopes {
        fn visit_stmt(&mut self, stmt: &'a ast::Stmt) {
            if self.invalid.is_some() {
                return;
            }
            if self.nested
                && let ast::Stmt::ImportFrom(import) = stmt
                && let Some(alias) = import.names.iter().find(|alias| alias.name.as_str() == "*")
            {
                self.invalid = Some(alias.range());
                return;
            }
            let outer = self.nested;
            self.nested |= matches!(stmt, ast::Stmt::ClassDef(_) | ast::Stmt::FunctionDef(_));
            stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || {
                ast::visitor::walk_stmt(self, stmt)
            });
            self.nested = outer;
        }
        fn visit_expr(&mut self, _: &'a ast::Expr) {}
    }
    let mut scopes = Scopes::default();
    use ast::visitor::Visitor;
    scopes.visit_body(body);
    scopes.invalid
}

/// A display of literal values is held by the code's constant table in CPython.
pub use ruff_python_ast::helpers::is_constant as literal_tuple_element;

pub mod annotations;
pub mod async_generators;
pub mod class_attributes;
pub mod class_cell;
pub mod code_flags;

/// Names imported by the module scope disable method binding even after reassignment.
pub fn module_imports(body: &[ast::Stmt]) -> BTreeSet<String> {
    use ast::visitor::{Visitor, walk_stmt};

    struct Imports(BTreeSet<String>);
    impl<'a> Visitor<'a> for Imports {
        fn visit_stmt(&mut self, stmt: &'a ast::Stmt) {
            let names = match stmt {
                ast::Stmt::Import(node) => &node.names,
                ast::Stmt::ImportFrom(node) => &node.names,
                ast::Stmt::FunctionDef(_) | ast::Stmt::ClassDef(_) => return,
                _ => return walk_stmt(self, stmt),
            };
            for alias in names {
                let name = alias.asname.as_ref().map_or_else(
                    || alias.name.as_str().split('.').next().unwrap(),
                    |asname| asname.as_str(),
                );
                self.0.insert(name.to_owned());
            }
        }

        fn visit_expr(&mut self, _: &'a ast::Expr) {}
    }

    let mut imports = Imports(BTreeSet::new());
    imports.visit_body(body);
    imports.0
}

/// Return the attribute for calls where CPython defers a method descriptor's receiver check.
pub fn direct_method<'a>(
    call: &'a ast::ExprCall,
    imports: &BTreeSet<String>,
    spellings: Option<&Names>,
) -> Option<&'a ast::ExprAttribute> {
    let ast::Expr::Attribute(field) = call.func.as_ref() else {
        return None;
    };
    let args = &call.arguments;
    // CPython's method call path is limited to fewer than 30 stack entries.
    (args.args.len() + args.keywords.len() + usize::from(!args.keywords.is_empty()) < 30
        && !matches!(field.value.as_ref(), ast::Expr::Name(name) if imports.contains(
            spellings.and_then(|names| names.original.get(&name.start())).unwrap_or(&name.id).as_str()
        ))
        && !args.args.iter().any(ast::Expr::is_starred_expr)
        && args.keywords.iter().all(|arg| arg.arg.is_some()))
    .then_some(field)
}

/// CPython omits the caret for the whole call in these assignment and return statements.
pub fn unhighlighted_call(stmt: &ast::Stmt) -> Option<ruff_text_size::TextRange> {
    match stmt {
        ast::Stmt::Assign(v)
            if matches!(v.targets.as_slice(), [ast::Expr::Name(_)])
                && matches!(&*v.value, ast::Expr::Call(_)) =>
        {
            Some(v.value.range())
        }
        ast::Stmt::Return(v) => v.value.as_deref().and_then(|e| match e {
            ast::Expr::Call(c) if matches!(&*c.func, ast::Expr::Name(_)) => Some(e.range()),
            _ => None,
        }),
        _ => None,
    }
}

/// Locate the delimiter after a call target or subscription receiver on a traceback line.
pub fn traceback_delimiter(
    source: &[u8],
    prefix_end: usize,
    end: usize,
    opener: u8,
) -> Option<usize> {
    let suffix = source.get(prefix_end..end)?;
    Some(prefix_end + suffix.iter().position(|&b| b == opener)?)
}

/// Python locates attribute loads and optimized method calls at the attribute name.
pub fn expression_position(
    expr: &ast::Expr,
    imports: &BTreeSet<String>,
    spellings: Option<&Names>,
) -> TextSize {
    match expr {
        ast::Expr::Attribute(field) => field.attr.start(),
        ast::Expr::Call(call) => direct_method(call, imports, spellings)
            .map_or_else(|| expr.start(), |field| field.attr.start()),
        _ => expr.start(),
    }
}

/// Check explicit keyword names before lowering can mangle class-private names.
pub fn repeated_keyword(node: ast::AnyNodeRef<'_>) -> Option<&ast::Keyword> {
    use ast::visitor::source_order::{SourceOrderVisitor, TraversalSignal};

    struct Children<'a>(Vec<ast::AnyNodeRef<'a>>);
    impl<'a> SourceOrderVisitor<'a> for Children<'a> {
        fn enter_node(&mut self, node: ast::AnyNodeRef<'a>) -> TraversalSignal {
            self.0.push(node);
            TraversalSignal::Skip
        }
    }

    let mut pending = Children(vec![node]);
    while let Some(node) = pending.0.pop() {
        let args = match node {
            // A call's own keyword list is validated before its callable is compiled.
            ast::AnyNodeRef::ExprCall(call) => Some(&call.arguments),
            ast::AnyNodeRef::StmtClassDef(class) => class.arguments.as_deref(),
            _ => None,
        };
        if let Some(args) = args {
            for (index, keyword) in args.keywords.iter().enumerate() {
                if let Some(name) = &keyword.arg
                    && let Some(repeated) = args.keywords[index + 1..].iter().find(|other| {
                        other
                            .arg
                            .as_ref()
                            .is_some_and(|id| id.as_str() == name.as_str())
                    })
                {
                    return Some(repeated);
                }
            }
        }
        let start = pending.0.len();
        node.visit_source_order(&mut pending);
        pending.0[start..].reverse();
    }
    None
}

#[derive(Default)]
pub struct Names {
    pub string_annotations: bool,
    pub duplicate_type_parameter: Option<Identifier>,
    pub nonlocal_in_generic_class: bool,
    /// The mangler changed at least one identifier in the syntax tree.
    pub rewritten: bool,
    original: HashMap<TextSize, Name>,
    import_roots: HashSet<TextSize>,
}

impl Names {
    /// A private definition keeps its source name for introspection.
    pub fn definition<'a>(&'a self, name: &'a Identifier) -> &'a str {
        self.source_name(name.start(), name.as_str())
    }

    /// Return the original spelling of a potentially mangled name.
    pub fn source_name<'a>(&'a self, start: TextSize, binding: &'a str) -> &'a str {
        self.original.get(&start).map_or(binding, Name::as_str)
    }

    /// A dotted import binds the top-level module under a possibly private name.
    pub fn imports_root(&self, name: &Identifier) -> bool {
        self.import_roots.contains(&name.start())
    }
}

/// Whether source opts in to stringized annotations via a future import.
pub fn future_annotations(body: &[ast::Stmt]) -> bool {
    future_import(body, "annotations")
}

/// Whether the module imports a given compiler feature from __future__.
pub fn future_import(body: &[ast::Stmt], feature: &str) -> bool {
    future_features(body).any(|name| name == feature)
}

/// Return the code flags declared in the module's leading future imports.
pub fn future_flags(body: &[ast::Stmt]) -> u32 {
    let mut flags = 0;
    for name in future_features(body) {
        flags |= match name {
            "annotations" => code_flags::FUTURE_ANNOTATIONS,
            "barry_as_FLUFL" => code_flags::FUTURE_BARRY_AS_BDFL,
            _ => 0,
        };
    }
    flags
}

/// Future directives form one contiguous prefix after the optional module docstring.
fn future_features(body: &[ast::Stmt]) -> impl Iterator<Item = &str> {
    ast::helpers::body_without_leading_docstring(body)
        .iter()
        .map_while(future_statement)
        .flat_map(|import| import.names.iter().map(|alias| alias.name.as_str()))
}

fn future_statement(stmt: &ast::Stmt) -> Option<&ast::StmtImportFrom> {
    let ast::Stmt::ImportFrom(import) = stmt else {
        return None;
    };
    (import.level == 0 && import.module.as_deref() == Some("__future__")).then_some(import)
}

/// Check future statement placement after the module's optional docstring.
pub fn future_import_error(
    body: &[ast::Stmt],
) -> Option<(ruff_text_size::TextRange, &'static str)> {
    use ast::visitor::Visitor;
    struct Check(Option<ruff_text_size::TextRange>);
    impl<'a> Visitor<'a> for Check {
        fn visit_stmt(&mut self, stmt: &'a ast::Stmt) {
            if self.0.is_some() {
                return;
            }
            if future_statement(stmt).is_some() {
                self.0 = Some(stmt.range());
            } else if ast::helpers::is_compound_statement(stmt) {
                stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || {
                    ast::visitor::walk_stmt(self, stmt)
                });
            }
        }
        fn visit_expr(&mut self, _: &'a ast::Expr) {}
    }
    let mut check = Check(None);
    for stmt in ast::helpers::body_without_leading_docstring(body)
        .iter()
        .skip_while(|stmt| future_statement(stmt).is_some())
    {
        check.visit_stmt(stmt);
        if let Some(range) = check.0 {
            // The symbol table visits the imported aliases before checking
            // future placement, including a star in this same statement.
            if let Some(star) = misplaced_star_import(body)
                && star.start() < range.end()
            {
                return Some((star, "import * only allowed at module level"));
            }
            return Some((
                range,
                "from __future__ imports must occur at the beginning of the file",
            ));
        }
    }
    None
}

pub fn lower(body: &mut [ast::Stmt]) -> Names {
    lower_in(body, "", future_annotations(body))
}

/// Apply the same lexical names when reparsing a generator's original suite.
pub fn lower_in(body: &mut [ast::Stmt], owner: &str, string_annotations: bool) -> Names {
    let names = RefCell::new(Names {
        string_annotations,
        ..Default::default()
    });
    Mangler {
        owner,
        names: &names,
        string_annotations,
        only_parameters: None,
        in_generic_class: false,
    }
    .visit_body(body);
    names.into_inner()
}

struct Mangler<'a> {
    owner: &'a str,
    names: &'a RefCell<Names>,
    string_annotations: bool,
    only_parameters: Option<&'a HashSet<Name>>,
    in_generic_class: bool,
}

fn private_name(owner: &str, name: &str) -> Option<Name> {
    let owner = owner.trim_start_matches('_');
    (is_mangled_private(name) && !name.contains('.') && !owner.is_empty())
        .then(|| Name::new(format!("_{owner}{name}")))
}

impl Mangler<'_> {
    fn replacement(&self, name: &str) -> Option<Name> {
        if self
            .only_parameters
            .is_some_and(|params| !params.contains(name))
        {
            return None;
        }
        private_name(self.owner, name).inspect(|_| self.names.borrow_mut().rewritten = true)
    }

    fn rename(&self, name: &mut Name) {
        if let Some(replacement) = self.replacement(name.as_str()) {
            *name = replacement;
        }
    }

    fn definition(&self, name: &mut Identifier) {
        self.remember(name.start(), &mut name.id);
    }

    fn type_parameter_bindings(&self, params: &mut ast::TypeParams) {
        let mut bindings = HashSet::new();
        for param in &mut params.type_params {
            let name = match param {
                ast::TypeParam::TypeVar(p) => &mut p.name,
                ast::TypeParam::TypeVarTuple(p) => &mut p.name,
                ast::TypeParam::ParamSpec(p) => &mut p.name,
            };
            self.definition(name);
            if !bindings.insert(name.id.clone()) {
                self.names
                    .borrow_mut()
                    .duplicate_type_parameter
                    .get_or_insert_with(|| name.clone());
            }
        }
    }

    /// Keep the source spelling for symbol-table queries after private-name mangling.
    fn remember(&self, start: TextSize, name: &mut Name) {
        if let Some(replacement) = self.replacement(name.as_str()) {
            let original = std::mem::replace(name, replacement);
            self.names.borrow_mut().original.insert(start, original);
        }
    }
}

impl Transformer for Mangler<'_> {
    fn visit_stmt(&self, stmt: &mut ast::Stmt) {
        match stmt {
            ast::Stmt::ClassDef(class) => {
                let owner = class.name.id.clone();
                self.definition(&mut class.name);
                for decorator in &mut class.decorator_list {
                    self.visit_decorator(decorator);
                }
                let body = Mangler {
                    owner: owner.as_str(),
                    only_parameters: None,
                    in_generic_class: self.in_generic_class || class.type_params.is_some(),
                    ..*self
                };
                if let Some(params) = &mut class.type_params {
                    // Only its declared private type parameters are mangled in a
                    // generic class's header. The body uses normal class mangling.
                    let names = params.iter().map(|param| param.name().id.clone()).collect();
                    let header = Mangler {
                        only_parameters: Some(&names),
                        ..body
                    };
                    header.type_parameter_bindings(params);
                    header.visit_type_params(params);
                    if let Some(arguments) = &mut class.arguments {
                        header.visit_arguments(arguments);
                    }
                } else if let Some(arguments) = &mut class.arguments {
                    self.visit_arguments(arguments);
                }
                body.visit_body(&mut class.body);
                return;
            }
            ast::Stmt::FunctionDef(function) => {
                self.definition(&mut function.name);
                if let Some(params) = &mut function.type_params {
                    self.type_parameter_bindings(params);
                }
            }

            ast::Stmt::TypeAlias(alias) => {
                if let Some(params) = &mut alias.type_params {
                    self.type_parameter_bindings(params);
                }
            }
            ast::Stmt::Global(global) => {
                for name in &mut global.names {
                    self.rename(&mut name.id);
                }
            }
            ast::Stmt::Nonlocal(nonlocal) => {
                if self.in_generic_class {
                    self.names.borrow_mut().nonlocal_in_generic_class = true;
                }
                for name in &mut nonlocal.names {
                    self.rename(&mut name.id);
                }
            }
            ast::Stmt::ImportFrom(import) => {
                if let Some(module) = &mut import.module {
                    self.rename(&mut module.id);
                }
            }
            _ => {}
        }
        transformer::walk_stmt(self, stmt);
    }

    fn visit_expr(&self, expr: &mut ast::Expr) {
        match expr {
            ast::Expr::Name(name) => self.remember(name.start(), &mut name.id),
            ast::Expr::Attribute(attr) => self.definition(&mut attr.attr),
            _ => {}
        }
        stacker::maybe_grow(128 * 1024, 2 * 1024 * 1024, || {
            transformer::walk_expr(self, expr)
        });
    }

    fn visit_annotation(&self, expression: &mut ast::Expr) {
        if !self.string_annotations {
            transformer::walk_annotation(self, expression);
        }
    }

    fn visit_parameter(&self, parameter: &mut ast::Parameter) {
        self.rename(&mut parameter.name.id);
        transformer::walk_parameter(self, parameter);
    }

    fn visit_pattern(&self, pattern: &mut ast::Pattern) {
        let binding = match pattern {
            ast::Pattern::MatchAs(p) => p.name.as_mut(),
            ast::Pattern::MatchStar(p) => p.name.as_mut(),
            ast::Pattern::MatchMapping(p) => p.rest.as_mut(),
            _ => None,
        };
        if let Some(name) = binding {
            self.definition(name);
        }
        transformer::walk_pattern(self, pattern);
    }

    fn visit_except_handler(&self, handler: &mut ast::ExceptHandler) {
        let ast::ExceptHandler::ExceptHandler(data) = handler;
        if let Some(name) = &mut data.name {
            self.rename(&mut name.id);
        }
        transformer::walk_except_handler(self, handler);
    }

    fn visit_alias(&self, alias: &mut ast::Alias) {
        if let Some(binding) = &mut alias.asname {
            self.rename(&mut binding.id);
        } else if let Some((root, _)) = alias.name.as_str().split_once('.')
            && let Some(binding) = self.replacement(root)
        {
            alias.asname = Some(Identifier::new(binding, alias.name.range()));
            self.names
                .borrow_mut()
                .import_roots
                .insert(alias.name.start());
        }
        self.rename(&mut alias.name.id);
    }
}
