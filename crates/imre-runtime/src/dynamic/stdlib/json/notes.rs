use super::{Context, Encoder, Error, Id, NONE, Result, Value};
use crate::dynamic::{builtins, copy_ids};

pub(super) enum Location<'a> {
    Index(usize),
    Key(Id, Option<&'a str>),
    Object,
}

impl Encoder {
    /// Attach the location of a child that failed to serialize. A writer
    /// exception comes from the consumer after the generator yielded a chunk.
    pub(super) fn noted(
        &self,
        ctx: &mut Context,
        error: Error,
        owner: Id,
        location: Location<'_>,
    ) -> Error {
        let streaming = self.stream.is_some();
        if self.write_failed
            || streaming && {
                let exit = ctx.builtin("GeneratorExit");
                ctx.raised_error_matches(&error, exit)
            }
        {
            return error;
        }
        let exception = ctx.raised_instance(error);
        // Reserve the exception, its suspended handler and any replacement.
        if let Err(mut failure) = ctx.heap.reserve_temporaries(3) {
            failure.context = exception;
            return failure;
        }
        let previous = ctx.heap.active_exception();
        // Both the suspended handler and the exception being annotated must
        // survive repr, descriptor lookup and arbitrary add_note callbacks.
        ctx.heap.keep(exception);
        if streaming {
            ctx.heap.keep(previous);
            ctx.heap.replace_active(exception);
        }
        let mark = ctx.heap.temporary_mark();
        let raised = if let Err(mut failure) = self.add_location(ctx, exception, owner, location) {
            // The C encoder chains a formatting failure to the original even
            // when a repr hook raises that same exception again.
            if !streaming
                || failure.context == NONE
                    && failure.trace.as_ref().and_then(|trace| trace.instance) != Some(exception)
            {
                failure.context = exception;
            }
            ctx.raised_instance(failure)
        } else {
            exception
        };
        // A callable deleted by repr can finalize before the enclosing
        // container looks up add_note again. Keep any replacement exception.
        ctx.finish_call_operands(mark, raised);
        ctx.heap.release_temporaries_since(mark);
        if raised != exception {
            ctx.heap.keep(raised);
        }
        if streaming {
            ctx.heap.replace_active(previous);
        }
        let Value::Raised(error) = ctx.heap.get(raised) else {
            unreachable!("annotated exception")
        };
        let mut error = error.clone();
        error.tracing().instance = Some(raised);
        error
    }

    fn add_location(
        &self,
        ctx: &mut Context,
        exception: Id,
        owner: Id,
        location: Location<'_>,
    ) -> Result<()> {
        // Python resolves the bound method before it evaluates its f-string argument.
        let method = if self.stream.is_some() {
            let method = ctx.lookup_attr(exception, "add_note")?;
            // Formatting the key can delete a callable stored on the exception.
            ctx.heap.reserve_temporaries(1)?;
            ctx.heap.keep(method);
            Some(method)
        } else {
            None
        };
        let class = ctx.exact_type(owner)?;
        let name = if method.is_some() {
            let name = ctx.lookup_attr(class, "__name__")?;
            builtins::format_value(ctx, name, None)?
        } else {
            super::super::qualified_type(ctx, class)?
        };
        let prefix = ctx.text("when serializing ");
        let prefix = ctx.concat_strings(prefix, name)?;
        let suffix = match location {
            Location::Index(index) => ctx.text(format!(" item {index}")),
            Location::Object => ctx.text(" object"),
            Location::Key(key, normalized) => {
                let key = normalized.map_or(key, |text| ctx.text(text));
                let rendered = builtins::call(ctx, "repr", &[key], &[])?;
                let rendered = if method.is_some() {
                    builtins::format_value(ctx, rendered, None)?
                } else {
                    rendered
                };
                let item = ctx.text(" item ");
                ctx.concat_strings(item, rendered)?
            }
        };
        let note = ctx.concat_strings(prefix, suffix)?;
        if let Some(method) = method {
            ctx.call(method, copy_ids(&[note])?, Vec::new())?;
            Ok(())
        } else {
            ctx.add_exception_note(exception, note)
        }
    }
}
