//! Checked lowering can finish, report a terminal diagnostic, or wait for a signature.
//! In this result type M is the diagnostic payload. A wait has no diagnostic text.

use std::fmt;
use std::path::{Path, PathBuf};

use ruff_text_size::{Ranged, TextRange, TextSize};

use crate::modules::{DefinitionId, ModuleId, SourceGraph};

pub(crate) type Result<T, M = String> = std::result::Result<T, Failure<M>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Ordinary,
    MissingFormatter(DefinitionId),
    UnsupportedRepresentation,
    FrameSuspension,
    UnresolvedParameter(ModuleId, ruff_text_size::TextSize),
    ExecutionOperand(ModuleId),
    RegisteredExecution,
    ExactType,
    DeclaredArgument,
    ReturnType,
    Object,
    ScopedObject,
    ClassLayout,
    ScopedClassLayout,
}

/// The entry operand and physical source containing its checked diagnostic.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct EntryOperand {
    pub(crate) module: ModuleId,
    pub(crate) statement: TextSize,
    pub(crate) operand: TextRange,
}

/// The source annotation that supplied an operand's expected type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct TypeDeclaration {
    pub(super) module: ModuleId,
    pub(super) annotation: TextRange,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum BodyCheck {
    Inference,
    Lowering,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum CheckedSite {
    Entry(EntryOperand),
    Function(ModuleId, TextRange, BodyCheck),
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Position {
    range: TextRange,
    checked_site: Option<Box<CheckedSite>>,
    expected_from: Option<TypeDeclaration>,
    line: usize,
    column: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Diagnostic<M = String> {
    message: M,
    position: Option<Box<Position>>,
    path: Option<PathBuf>,
    pub(crate) kind: Kind,
    following: Box<[Diagnostic<M>]>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Failure<M = String> {
    Diagnostic(Box<Diagnostic<M>>),
    PendingSignature {
        signature: usize,
        missing_callable_parameters: bool,
        module: ModuleId,
        range: TextRange,
    },
}

impl<M> Diagnostic<M> {
    pub(crate) fn plain(message: M) -> Self {
        Self {
            message,
            position: None,
            path: None,
            kind: Kind::Ordinary,
            following: Box::new([]),
        }
    }

    pub(crate) fn at(source: &str, site: &impl Ranged, message: M) -> Self {
        let range = site.range();
        let prefix = &source[..usize::from(range.start())];
        let line = prefix.bytes().filter(|&byte| byte == b'\n').count() + 1;
        let column = prefix.rsplit('\n').next().unwrap_or("").chars().count() + 1;
        Self {
            position: Some(Box::new(Position {
                range,
                checked_site: None,
                expected_from: None,
                line,
                column,
            })),
            ..Self::plain(message)
        }
    }

    pub(crate) fn classified(mut self, kind: Kind) -> Self {
        self.kind = kind;
        self
    }

    pub(crate) fn entry_operand(&self) -> Option<EntryOperand> {
        let CheckedSite::Entry(entry) = self.position.as_ref()?.checked_site.as_deref()? else {
            return None;
        };
        (self.kind == Kind::ExactType).then_some(*entry)
    }

    pub(super) fn exact_operand(&self) -> Option<TextRange> {
        (self.kind == Kind::ExactType).then_some(self.position.as_ref()?.range)
    }

    /// Attach the original declaration to a refused operand in its synchronous body.
    pub(super) fn in_function(
        &mut self,
        graph: &SourceGraph,
        owner: DefinitionId,
        check: BodyCheck,
    ) {
        let module = &graph.modules[owner.module];
        if graph.registered_startup
            && module.is_program_source()
            && let Some(operand) = self.exact_operand()
            && let Some(function) =
                module.parsed.syntax().body[owner.statement].as_function_def_stmt()
            && function.decorator_list.is_empty()
            && !function.is_async
            && function.range().contains_range(operand)
            && graph
                .session
                .unconditional_function_operand(owner.module, function, operand)
            && let Some(position) = &mut self.position
        {
            position.checked_site = Some(Box::new(CheckedSite::Function(
                owner.module,
                function.range(),
                check,
            )));
        }
    }

    pub(crate) fn function_site(&self) -> Option<(ModuleId, TextRange)> {
        let CheckedSite::Function(module, range, _) =
            self.position.as_ref()?.checked_site.as_deref()?
        else {
            return None;
        };
        Some((*module, *range))
    }

    pub(crate) fn function_reports(&self) -> impl Iterator<Item = ((ModuleId, TextRange), &Self)> {
        std::iter::once(self)
            .chain(self.following.iter())
            .filter_map(|report| Some((report.function_site()?, report)))
    }

    pub(crate) fn with_path(mut self, path: &std::path::Path) -> Self {
        self.path = Some(path.to_owned());
        self
    }

    pub(super) fn with_following(mut self, reports: impl Iterator<Item = Self>) -> Self {
        self.following = reports.collect();
        self
    }

    pub(crate) fn check_declared_operand<'p>(
        &self,
        mut selected: impl Iterator<Item = (ModuleId, &'p Path)> + Clone,
    ) -> std::result::Result<(), String>
    where
        M: fmt::Display,
    {
        // Lowering delegates a named body to startup. Inference's existing
        // declaration check still applies when the report also carries an owner.
        let site = self
            .position
            .as_ref()
            .and_then(|p| p.checked_site.as_deref());
        if !matches!(site, Some(CheckedSite::Function(_, _, BodyCheck::Lowering)))
            && self.kind == Kind::ExactType
            && self
                .position
                .as_ref()
                .and_then(|position| position.expected_from)
                .is_some_and(|owner| selected.clone().any(|(id, _)| id == owner.module))
            && selected.any(|(_, path)| self.path.as_deref() == Some(path))
        {
            Err(self.to_string())
        } else {
            Ok(())
        }
    }

    pub(crate) fn earlier_contract(&self, other: &Self) -> bool {
        matches!(
            self.kind,
            Kind::ExactType | Kind::DeclaredArgument | Kind::ReturnType
        ) && self.path == other.path
            && self
                .position
                .as_ref()
                .zip(other.position.as_ref())
                .is_some_and(|(a, b)| a.range.start() < b.range.start())
    }

    pub(crate) fn matching_declared_operand(
        &self,
        path: &std::path::Path,
        operand: TextRange,
        module: ModuleId,
        annotation: TextRange,
    ) -> Option<&Self> {
        std::iter::once(self)
            .chain(self.following.iter())
            .find(|report| {
                report.kind == Kind::ExactType
                    && report.path.as_deref() == Some(path)
                    && report.position.as_ref().is_some_and(|position| {
                        position.range == operand
                            && position.expected_from.is_some_and(|declaration| {
                                declaration.module == module
                                    && annotation.contains_range(declaration.annotation)
                            })
                    })
            })
    }

    pub(super) fn in_module(self, graph: &SourceGraph, module: ModuleId) -> Self {
        self.with_path(&graph.modules[module].path)
    }
}

impl<M: fmt::Display> fmt::Display for Diagnostic<M> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(path) = &self.path {
            write!(f, "{}:", path.display())?;
        }
        if let Some(p) = &self.position {
            write!(f, "{}:{}: Imre: ", p.line, p.column)?;
        }
        self.message.fmt(f)
    }
}

impl From<&str> for Diagnostic {
    fn from(message: &str) -> Self {
        Self::plain(message.to_owned())
    }
}

impl Kind {
    pub(super) fn is_representation(self) -> bool {
        self == Self::UnsupportedRepresentation || self.is_object()
    }

    pub(super) fn is_object(self) -> bool {
        matches!(self, Self::Object | Self::ScopedObject) || self.is_class_layout()
    }

    pub(crate) fn checks_object_calls(self) -> bool {
        matches!(self, Self::ScopedObject | Self::ScopedClassLayout)
    }

    pub(crate) fn is_class_layout(self) -> bool {
        matches!(self, Self::ClassLayout | Self::ScopedClassLayout)
    }
}

impl From<String> for Diagnostic {
    fn from(message: String) -> Self {
        Self::plain(message)
    }
}

impl<M> From<Diagnostic<M>> for Failure<M> {
    fn from(report: Diagnostic<M>) -> Self {
        Self::Diagnostic(Box::new(report))
    }
}

impl From<String> for Failure {
    fn from(message: String) -> Self {
        Diagnostic::plain(message).into()
    }
}

impl From<&str> for Failure {
    fn from(message: &str) -> Self {
        message.to_owned().into()
    }
}

impl Failure<&'static str> {
    pub(super) fn at(self, source: &str, site: &impl Ranged) -> Failure {
        match self {
            Self::PendingSignature {
                signature,
                missing_callable_parameters,
                module,
                range,
            } => Failure::PendingSignature {
                signature,
                missing_callable_parameters,
                module,
                range,
            },
            Self::Diagnostic(report) => Diagnostic::at(source, site, report.message.to_owned())
                .classified(report.kind)
                .into(),
        }
    }
}

impl Failure {
    pub(super) fn in_entry_statement(
        mut self,
        entry: Option<ModuleId>,
        statement: TextSize,
    ) -> Self {
        if let (Some(module), Self::Diagnostic(report)) = (entry, &mut self)
            && report.kind == Kind::ExactType
            && let Some(position) = &mut report.position
        {
            position.checked_site.get_or_insert_with(|| {
                Box::new(CheckedSite::Entry(EntryOperand {
                    module,
                    statement,
                    operand: position.range,
                }))
            });
        }
        self
    }

    pub(super) fn expected_from(
        mut self,
        declaration: Option<TypeDeclaration>,
        operand: TextRange,
    ) -> Self {
        if let Some(declaration) = declaration
            && let Self::Diagnostic(error) = &mut self
            && error.kind == Kind::ExactType
            && let Some(position) = &mut error.position
            && position.range == operand
        {
            position.expected_from.get_or_insert(declaration);
        }
        self
    }

    pub(super) fn at_callable_argument(
        mut self,
        site: TextRange,
        declared: impl FnOnce() -> bool,
    ) -> Self {
        if let Self::Diagnostic(d) = &mut self
            && d.kind == Kind::ExactType
            && d.position.as_ref().is_some_and(|p| p.range == site)
            && declared()
        {
            d.kind = Kind::DeclaredArgument;
        }
        self
    }

    pub(super) fn classify(mut self, kind: Kind) -> Self {
        if let Self::Diagnostic(d) = &mut self {
            d.kind = kind;
        }
        self
    }

    pub(super) fn is_representation(&self) -> bool {
        matches!(self, Self::Diagnostic(d) if d.kind.is_representation())
    }

    pub(super) fn is_object(&self) -> bool {
        matches!(self, Self::Diagnostic(d) if d.kind.is_object())
    }

    pub(super) fn map_message(mut self, f: impl FnOnce(String) -> String) -> Self {
        if let Self::Diagnostic(d) = &mut self {
            d.message = f(std::mem::take(&mut d.message));
        }
        self
    }

    pub(super) fn terminal_in(self, graph: &SourceGraph, module: ModuleId) -> Diagnostic {
        match self {
            Self::Diagnostic(d) => d.in_module(graph, module),
            Self::PendingSignature { module, range, .. } => Diagnostic::at(
                &graph.modules[module].source,
                &range,
                "inference is waiting for a function return type".to_owned(),
            )
            .in_module(graph, module),
        }
    }
}
