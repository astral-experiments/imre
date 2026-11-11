use super::{Cleanup, Completion, Emitter, Statement, Type, Value};

/// The LLVM predecessors of one lexical finalbody. The pending action and its
/// owners remain distinct even when several protected exits share a destination.
pub(super) struct Finalbody {
    entry: String,
    edges: Vec<Edge>,
}

struct Edge {
    from: String,
    completion: Completion,
    exception: Option<(Value, Value)>,
}

impl<T> Completion<T> {
    fn map_return<U>(&self, f: impl FnOnce(&T) -> U) -> Completion<U> {
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

impl Finalbody {
    fn phi(
        &self,
        out: &mut Emitter<'_, '_>,
        ty: &str,
        operand: impl Fn(&Edge) -> String,
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

    fn owner(
        &self,
        out: &mut Emitter<'_, '_>,
        ty: Type,
        get: impl Fn(&Edge) -> Option<&Value>,
    ) -> Value {
        let operand = self.phi(out, ty.llvm(), |edge| {
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
        self.finalbodies.push(Finalbody {
            entry,
            edges: Vec::new(),
        });
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
        let region = &mut self.finalbodies[region];
        region.edges.push(Edge {
            from: self.body.current.clone(),
            completion,
            exception,
        });
        self.body.jump(&region.entry);
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
        const MAX_CLEANUP_BYTES: usize = 8 * 1024 * 1024;
        let region = self.finalbodies.pop().expect("active finally");
        if region.edges.is_empty() {
            return;
        }
        self.start(&region.entry);
        if self.cleanup_limit_hit || self.cleanup_bytes >= MAX_CLEANUP_BYTES {
            self.cleanup_limit_hit = true;
            self.body.end("unreachable");
            return;
        }
        let before = self.body.code.len();
        let previous_bytes = self.cleanup_bytes;
        let mut targets = Vec::new();
        for edge in &region.edges {
            let target = edge.completion.map_return(|value| value.ty);
            if !targets.contains(&target) {
                targets.push(target);
            }
        }
        let tag = region.phi(self, "i32", |edge| {
            let target = edge.completion.map_return(|value| value.ty);
            targets
                .iter()
                .position(|t| *t == target)
                .unwrap()
                .to_string()
        });
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
            let last = targets.len() - 1;
            for (index, target) in targets.into_iter().enumerate() {
                let next = (index != last).then(|| {
                    let selected = self.instruction(format!("icmp eq i32 {tag}, {index}"));
                    let (resume, next) = (self.label(), self.label());
                    self.branch(&selected, &resume, &next);
                    self.start(&resume);
                    next
                });
                if matches!(target, Completion::Error) {
                    let (error, previous) = exception.as_ref().unwrap();
                    self.restore_active(previous, None);
                    self.void_call("imre_error_set", &[error]);
                    self.release(error);
                }
                self.route(target.map_return(|_| returned.as_ref().unwrap().clone()));
                if let Some(next) = next {
                    self.start(&next);
                }
            }
        }
        // A nested finalbody is included once in its enclosing byte range.
        self.cleanup_bytes = previous_bytes + (self.body.code.len() - before);
        self.cleanup_limit_hit |= self.cleanup_bytes >= MAX_CLEANUP_BYTES;
    }
}
