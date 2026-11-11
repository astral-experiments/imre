//! Transfer the result of each reachable normal edge into one checked value.
use super::{CheckedValue, FunctionBody, Type, Value};

#[derive(Default)]
pub(crate) struct ValueJoin {
    incoming: Vec<(CheckedValue, String)>,
    owner: Option<usize>,
}

/// Emit an ordered choice with one continuation for its reachable edges. An
/// unconditional arm ends the chain. A missing else falls through normally.
pub(crate) fn choose<C, T, A>(
    context: &mut C,
    output: fn(&mut C) -> &mut FunctionBody,
    arms: impl IntoIterator<Item = (Option<T>, A)>,
    mut branch: impl FnMut(&mut C, T, &str, &str),
    mut emit: impl FnMut(&mut C, A),
    resumable: bool,
) {
    let done = output(context).name("select_end");
    let mut reachable = false;
    let mut finish = |out: &mut FunctionBody| {
        if !out.terminated {
            out.jump(&done);
            reachable = true;
        }
    };
    for (test, arm) in arms {
        let next = test.map(|test| {
            let out = output(context);
            let yes = out.name("select_yes");
            let no = out.name("select_no");
            branch(context, test, &yes, &no);
            output(context).start(&yes);
            no
        });
        emit(context, arm);
        finish(output(context));
        let Some(next) = next else { break };
        output(context).start(&next);
    }
    let out = output(context);
    finish(out);
    out.start(&done);
    if !reachable && !resumable {
        out.end("unreachable");
    }
}

impl ValueJoin {
    /// The producer decides the condition and evaluates only the selected arm at
    /// runtime. Accessing the output publishes the normal edge after an ABI check.
    pub(crate) fn select<C>(
        context: &mut C,
        output: fn(&mut C) -> &mut FunctionBody,
        branch: impl FnOnce(&mut C, &str, &str),
        on_true: impl FnOnce(&mut C) -> CheckedValue,
        on_false: impl FnOnce(&mut C) -> CheckedValue,
    ) -> Option<CheckedValue> {
        let mut join = Self::default();
        choose(
            context,
            output,
            [(Some(branch), Ok(on_true)), (None, Err(on_false))],
            |code, test, yes, no| test(code, yes, no),
            |code, arm| {
                let value = match arm {
                    Ok(yes) => yes(code),
                    Err(no) => no(code),
                };
                join.incoming(output(code), value);
            },
            false,
        );
        join.finish(output(context))
    }

    /// Record an arm after its error check and before its normal terminator.
    /// Ownership transfer belongs on an edge exclusive to this join. A shared
    /// predecessor may contribute a rooted handle without changing either edge.
    /// A returning or raising arm contributes no result.
    pub(crate) fn incoming(&mut self, out: &mut FunctionBody, value: CheckedValue) {
        if out.terminated {
            return;
        }
        if let Some(source) = value.frame_owner() {
            let destination = *self.owner.get_or_insert(source);
            if source != destination {
                // Only one arm executes. Both slots start at tagged zero, including
                // when a previous guard selected its Python result.
                let word = out.instruction(format!("load i64, ptr %int_owner{source}"));
                out.line(format!("store i64 1, ptr %int_owner{source}"));
                out.line(format!("store i64 {word}, ptr %int_owner{destination}"));
            }
        }
        self.incoming.push((value, out.current.clone()));
    }

    pub(crate) fn jump(&mut self, out: &mut FunctionBody, done: &str, value: CheckedValue) {
        if !out.terminated {
            self.incoming(out, value);
            out.jump(done);
        }
    }

    fn phi(&self, out: &mut FunctionBody, ty: &str, operand: fn(&CheckedValue) -> &str) -> String {
        let first = operand(&self.incoming[0].0);
        if self
            .incoming
            .iter()
            .all(|(value, _)| operand(value) == first)
        {
            return first.to_owned();
        }
        let edges = self
            .incoming
            .iter()
            .map(|(value, block)| format!("[ {}, %{block} ]", operand(value)))
            .collect::<Vec<_>>()
            .join(", ");
        out.instruction(format!("phi {ty} {edges}"))
    }

    /// Use the live predecessor list supplied by the control-flow producer.
    pub(crate) fn finish(mut self, out: &mut FunctionBody) -> Option<CheckedValue> {
        if self.incoming.len() < 2 {
            return self.incoming.pop().map(|(value, _)| value);
        }
        if self
            .incoming
            .iter()
            .all(|(v, _)| matches!(v, CheckedValue::Handle { .. }))
        {
            let id = self.phi(out, "i64", python_handle);
            let exact_word = self
                .incoming
                .iter()
                .all(|(v, _)| {
                    matches!(
                        v,
                        CheckedValue::Handle {
                            exact_word: Some(_),
                            ..
                        }
                    )
                })
                .then(|| self.phi(out, "i64", exact_handle_word));
            return Some(CheckedValue::Handle { id, exact_word });
        }
        if let CheckedValue::Native(first, _) = &self.incoming[0].0
            && self
                .incoming
                .iter()
                .all(|(v, _)| matches!(v, CheckedValue::Native(..)))
        {
            let ty = first.value.ty;
            debug_assert!(self.incoming.iter().all(|(v, _)| {
                let CheckedValue::Native(input, owner) = v else {
                    return false;
                };
                input.value.ty == ty
                    && owner.is_some() == self.owner.is_some()
                    && matches!(input.ownership, super::Ownership::Owned)
            }));
            let small_integer = self.incoming.iter().all(
                |(v, _)| matches!(v, CheckedValue::Native(input, _) if input.value.small_integer),
            );
            let value = Value {
                ty,
                small_integer,
                operand: self.phi(out, ty.llvm(), native_operand),
            };
            return Some(CheckedValue::Native(
                super::Operand {
                    value,
                    ownership: super::Ownership::Owned,
                },
                self.owner,
            ));
        }
        // A native word entering a Python join already has a frame cleanup slot.
        // A rooted object retains its identity even when it represents an exact int.
        debug_assert!(self.incoming.iter().all(|(value, _)| !matches!(value,
            CheckedValue::Native(input, slot) if input.value.ty != Type::Int || slot.is_none())));
        Some(CheckedValue::IntOrHandle {
            word: self.phi(out, "i64", native_operand),
            fallback: self.phi(out, "i64", python_handle),
            owner: self.owner.expect("registered integer owner"),
        })
    }
}

fn native_operand(value: &CheckedValue) -> &str {
    match value {
        CheckedValue::Native(v, _) => &v.value.operand,
        CheckedValue::IntOrHandle { word, .. } => word,
        CheckedValue::Handle { .. } => "0",
        CheckedValue::IntegerLiteral(_) => {
            unreachable!("a literal operand belongs to its operation")
        }
    }
}

fn python_handle(value: &CheckedValue) -> &str {
    match value {
        CheckedValue::Handle { id, .. } => id,
        CheckedValue::IntOrHandle { fallback, .. } => fallback,
        CheckedValue::Native(..) | CheckedValue::IntegerLiteral(_) => "0",
    }
}

fn exact_handle_word(value: &CheckedValue) -> &str {
    match value {
        CheckedValue::Handle {
            exact_word: Some(word),
            ..
        } => word,
        _ => unreachable!("exact rooted integer"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_normal_successor_terminates_the_join() {
        let mut out = FunctionBody::new("entry", "failed");
        let exit = |out: &mut FunctionBody| {
            out.end("ret i64 0");
            CheckedValue::handle("%dead".into())
        };
        let result = ValueJoin::select(
            &mut out,
            |out| out,
            |out, yes, no| out.branch("%test", yes, no),
            exit,
            exit,
        );
        assert!(result.is_none());
        assert!(out.terminated);
        assert!(out.code.ends_with("  unreachable\n"));
        assert!(!out.code.contains("phi "));
    }

    #[test]
    fn terminated_arms_supply_neither_phi_inputs_nor_owners() {
        let mut join = ValueJoin::default();
        let mut out = FunctionBody::new("left", "failed");
        out.end("ret i64 0");
        join.jump(&mut out, "end", CheckedValue::handle("%dead".into()));
        assert!(join.finish(&mut out).is_none());

        let mut join = ValueJoin::default();
        out.start("only");
        join.jump(&mut out, "end", CheckedValue::handle("%live".into()));
        out.start("end");
        let Some(CheckedValue::Handle { id, .. }) = join.finish(&mut out) else {
            panic!("sole reachable result")
        };
        assert_eq!(id, "%live");
        assert!(!out.code.contains("%dead"));
        assert!(!out.code.contains("phi "));
    }
}
