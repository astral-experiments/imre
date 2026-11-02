use super::{Context, Error, Id, Result, Value};
use crate::dynamic::builtins;

pub(super) enum Location {
    Index(usize),
    Key(Id),
    Object,
}

/// Attach the location of a child that failed to serialize.
pub(super) fn noted(ctx: &mut Context, error: Error, owner: Id, location: Location) -> Error {
    let exception = ctx.raised_instance(error);
    noted_instance(ctx, exception, owner, location)
}

/// Annotate the live exception, including changes a replacement's finalizer made to it.
pub(super) fn noted_instance(
    ctx: &mut Context,
    exception: Id,
    owner: Id,
    location: Location,
) -> Error {
    // Reserve the exception and any replacement produced while formatting its note.
    if let Err(mut failure) = ctx.heap.reserve_temporaries(2) {
        failure.context = exception;
        return failure;
    }
    ctx.heap.keep(exception);
    let mark = ctx.heap.temporary_mark();
    let raised = if let Err(mut failure) = add_location(ctx, exception, owner, location) {
        // The C encoder chains a formatting failure to the original even
        // when a repr hook raises that same exception again.
        failure.context = exception;
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
    live_error(ctx, raised)
}

/// Propagate the exception's current state after running user callbacks.
pub(super) fn live_error(ctx: &Context, raised: Id) -> Error {
    let Value::Raised(error) = ctx.heap.get(raised) else {
        unreachable!("annotated exception")
    };
    let mut error = error.clone();
    error.tracing().instance = Some(raised);
    error
}

fn add_location(ctx: &mut Context, exception: Id, owner: Id, location: Location) -> Result<()> {
    let class = ctx.exact_type(owner)?;
    let name = super::super::qualified_type(ctx, class)?;
    let prefix = ctx.text("when serializing ");
    let prefix = ctx.concat_strings(prefix, name)?;
    let suffix = match location {
        Location::Index(index) => ctx.text(format!(" item {index}")),
        Location::Object => ctx.text(" object"),
        Location::Key(key) => {
            let rendered = builtins::call(ctx, "repr", &[key], &[])?;
            let item = ctx.text(" item ");
            ctx.concat_strings(item, rendered)?
        }
    };
    let note = ctx.concat_strings(prefix, suffix)?;
    ctx.add_exception_note(exception, note)
}
