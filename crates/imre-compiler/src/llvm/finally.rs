use super::operations::CheckedBody;
use super::{Cleanup, Completion, Emitter, Statement, Type, Value, body::FunctionBody};

/// The LLVM predecessors of one lexical finalbody. The pending action and its
/// owners remain distinct even when several protected exits share a destination.
pub(super) struct Finalbody<V = Value> {
    pub(super) entry: String,
    edges: Vec<Edge<V>>,
}

pub(super) struct Edge<V> {
    from: String,
    pub(super) completion: Completion<V>,
    pub(super) exception: Option<(V, V)>,
}

impl<T> Completion<T> {
    pub(super) fn map_return<U>(&self, f: impl FnOnce(&T) -> U) -> Completion<U> {
        match self {
            Self::Return(value) => Completion::Return(f(value)),
            Self::Jump { label, depth } => Completion::Jump {
                label: label.clone(),
                depth: *depth,
            },
            Self::Error => Completion::Error,
        }
    }
}

impl<V> Finalbody<V> {
    pub(super) fn new(entry: String) -> Self {
        Self {
            entry,
            edges: Vec::new(),
        }
    }

    pub(super) fn is_empty(&self) -> bool {
        self.edges.is_empty()
    }

    /// Save this predecessor's completion before transferring to the single cleanup.
    pub(super) fn enter(
        &mut self,
        body: &mut FunctionBody,
        completion: Completion<V>,
        exception: Option<(V, V)>,
    ) {
        self.edges.push(Edge {
            from: body.current.clone(),
            completion,
            exception,
        });
        body.jump(&self.entry);
    }

    pub(super) fn completions<T: PartialEq>(
        &self,
        out: &mut FunctionBody,
        pending_error: Option<&str>,
        returned: impl Fn(&V) -> T,
    ) -> (Vec<Completion<T>>, String) {
        // A captured Python exception is None on the ordinary edge. Reuse that
        // handle for the dispatch instead of carrying another tag across cleanup.
        if let Some(error) = pending_error
            && let [ordinary, failed] = self.edges.as_slice()
            && ordinary.exception.is_none()
            && failed.exception.is_some()
        {
            return (
                vec![ordinary.completion.map_return(&returned), Completion::Error],
                format!("i64 {error}"),
            );
        }
        let mut targets = Vec::new();
        let tag = self.phi(out, "i32", |edge| {
            let target = edge.completion.map_return(&returned);
            let index = targets
                .iter()
                .position(|seen| *seen == target)
                .unwrap_or_else(|| {
                    let index = targets.len();
                    targets.push(target);
                    index
                });
            index.to_string()
        });
        (targets, format!("i32 {tag}"))
    }

    pub(super) fn phi(
        &self,
        out: &mut FunctionBody,
        ty: &str,
        mut operand: impl FnMut(&Edge<V>) -> String,
    ) -> String {
        if let [edge] = self.edges.as_slice() {
            return operand(edge);
        }
        let incoming = self
            .edges
            .iter()
            .map(|edge| format!("[ {}, %{} ]", operand(edge), edge.from))
            .collect::<Vec<_>>()
            .join(", ");
        out.instruction(format!("phi {ty} {incoming}"))
    }
}

impl Finalbody {
    fn owner(
        &self,
        out: &mut Emitter<'_, '_>,
        ty: Type,
        get: impl Fn(&Edge<Value>) -> Option<&Value>,
    ) -> Value {
        let operand = self.phi(&mut out.body, ty.llvm(), |edge| {
            get(edge).map_or_else(|| ty.initial().to_owned(), |value| value.operand.clone())
        });
        Value {
            ty,
            operand,
            small_integer: false,
        }
    }
}

impl Emitter<'_, '_> {
    pub(super) fn push_finalbody(&mut self) {
        let entry = self.label();
        self.cleanups.push(Cleanup::Finally(self.finalbodies.len()));
        self.finalbodies.push(Finalbody::new(entry));
    }

    pub(super) fn enter_finalbody(&mut self, region: usize, completion: Completion) {
        let exception = if matches!(completion, Completion::Error) {
            let error = self.call("imre_error_take", Type::Exception, &[]);
            let previous = self.call("imre_exception_swap_active", Type::Exception, &[&error]);
            self.forget(&error);
            self.forget(&previous);
            Some((error, previous))
        } else {
            None
        };
        self.finalbodies[region].enter(&mut self.body, completion, exception);
    }

    /// A transfer out of the cleanup replaces the pending exception only on
    /// the incoming error edge. Other edges inherit the surrounding handler.
    pub(super) fn discard_pending_exception(&mut self, error: &Value, previous: &Value) {
        let pending = self.instruction(format!("icmp ne ptr {}, null", error.operand));
        let (restore, done) = (self.label(), self.label());
        self.branch(&pending, &restore, &done);
        self.start(&restore);
        self.restore_active(previous, None);
        self.release(error);
        self.jump(&done);
        self.start(&done);
    }

    pub(super) fn emit_finalbody(&mut self, statements: &[Statement]) {
        let region = self.finalbodies.pop().expect("active finally");
        if region.is_empty() {
            return;
        }
        self.start(&region.entry);
        let (targets, tag) = region.completions(&mut self.body, None, |value| value.ty);
        let returned = targets.iter().find_map(|target| match target {
            Completion::Return(ty) => {
                Some(region.owner(self, *ty, |edge| match &edge.completion {
                    Completion::Return(value) => Some(value),
                    _ => None,
                }))
            }
            _ => None,
        });
        let exception = targets.contains(&Completion::Error).then(|| {
            let error = region.owner(self, Type::Exception, |edge| {
                edge.exception.as_ref().map(|(e, _)| e)
            });
            let previous = region.owner(self, Type::Exception, |edge| {
                edge.exception.as_ref().map(|(_, p)| p)
            });
            (error, previous)
        });
        // A replacement return, error or loop transfer owns its own value.
        // Unwind any displaced owners at this lexical depth before proceeding.
        let depth = self.cleanups.len();
        if let Some(value) = &returned {
            self.cleanups.push(Cleanup::Owner(value.clone()));
        }
        if let Some((error, previous)) = &exception {
            self.cleanups.push(Cleanup::PendingException {
                error: error.clone(),
                previous: previous.clone(),
            });
        }
        self.block(statements);
        self.cleanups.truncate(depth);
        if !self.body.terminated {
            CheckedBody::native(self).resume_completions(&targets, &tag, |body, target| {
                let code = body.fixed();
                if matches!(target, Completion::Error) {
                    let (error, previous) = exception.as_ref().unwrap();
                    code.restore_active(previous, None);
                    code.void_call("imre_error_set", &[error]);
                    code.release(error);
                }
                code.route(target.map_return(|_| returned.as_ref().unwrap().clone()));
            });
        }
    }
}
