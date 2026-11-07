//! Checked lowering can finish, report a terminal diagnostic, or wait for a signature.
//! In this result type M is the diagnostic payload. A wait has no diagnostic text.

use std::fmt;
use std::path::PathBuf;

use ruff_text_size::{Ranged, TextRange};

use crate::modules::{ModuleId, SourceGraph};

pub(crate) type Result<T, M = String> = std::result::Result<T, Failure<M>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Ordinary,
    ExactType,
    ReturnType,
    Object,
    ScopedObject,
    ClassLayout,
    ScopedClassLayout,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Position {
    range: TextRange,
    line: usize,
    column: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Diagnostic<M = String> {
    message: M,
    position: Option<Position>,
    path: Option<PathBuf>,
    pub(crate) kind: Kind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Failure<M = String> {
    Diagnostic(Diagnostic<M>),
    PendingSignature {
        signature: usize,
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
        }
    }

    pub(crate) fn at(source: &str, site: &impl Ranged, message: M) -> Self {
        let range = site.range();
        let prefix = &source[..usize::from(range.start())];
        let line = prefix.bytes().filter(|&byte| byte == b'\n').count() + 1;
        let column = prefix.rsplit('\n').next().unwrap_or("").chars().count() + 1;
        Self {
            position: Some(Position {
                range,
                line,
                column,
            }),
            ..Self::plain(message)
        }
    }

    pub(crate) fn classified(mut self, kind: Kind) -> Self {
        self.kind = kind;
        self
    }

    pub(crate) fn with_path(mut self, path: &std::path::Path) -> Self {
        self.path = Some(path.to_owned());
        self
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

impl From<String> for Failure {
    fn from(message: String) -> Self {
        Self::Diagnostic(message.into())
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
                module,
                range,
            } => Failure::PendingSignature {
                signature,
                module,
                range,
            },
            Self::Diagnostic(report) => {
                Failure::Diagnostic(Diagnostic::at(source, site, report.message.to_owned()))
            }
        }
    }
}

impl Failure {
    pub(super) fn classify(mut self, kind: Kind) -> Self {
        if let Self::Diagnostic(d) = &mut self {
            d.kind = kind;
        }
        self
    }

    pub(super) fn is_object(&self) -> bool {
        matches!(self, Self::Diagnostic(d) if matches!(d.kind, Kind::Object | Kind::ScopedObject | Kind::ClassLayout | Kind::ScopedClassLayout))
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
