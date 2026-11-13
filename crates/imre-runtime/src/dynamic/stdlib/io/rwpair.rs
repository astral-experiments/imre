//! A BufferedRWPair owns separate buffered streams of its native module generation.

use super::{
    Context, Error, FALSE, Id, MemoryStream, NONE, NativeClass, Result, TRUE, Value, release_child,
};

pub(super) const METHODS: &[&str] = &[
    "__init__",
    "read",
    "peek",
    "read1",
    "readinto",
    "readinto1",
    "write",
    "flush",
    "readable",
    "writable",
    "close",
    "isatty",
    "closed",
];

#[derive(Clone, Copy, Default)]
pub(in crate::dynamic) struct Pair {
    pub(in crate::dynamic) reader: Option<Id>,
    pub(in crate::dynamic) writer: Option<Id>,
}

fn state(ctx: &Context, receiver: Id) -> Pair {
    match ctx.heap.get(receiver) {
        Value::Instance {
            memory_stream: Some(MemoryStream::Pair(pair)),
            ..
        } => *pair,
        _ => Pair::default(),
    }
}

fn store(ctx: &mut Context, receiver: Id, pair: Pair) {
    let Value::Instance { memory_stream, .. } = ctx.heap.get_mut(receiver) else {
        unreachable!("validated BufferedRWPair receiver")
    };
    *memory_stream = Some(MemoryStream::Pair(pair));
}

/// CPython's exact pair deallocator clears its weakrefs, reader, then writer.
/// Subclasses and cyclic isolates use the inherited IOBase finalizer instead.
pub(in crate::dynamic) fn deallocate_exact_pair(ctx: &mut Context, owner: Id) -> Result<bool> {
    let Value::Instance { class, .. } = ctx.heap.get(owner) else {
        return Ok(false);
    };
    if !matches!(
        ctx.heap.get(*class),
        Value::Class {
            io_kind: Some(NativeClass::Pair { .. }),
            ..
        }
    ) {
        return Ok(false);
    }
    ctx.heap.take_finalizer(owner, true);
    let mark = ctx.heap.temporary_mark();
    ctx.heap.keep(owner);
    let pending = ctx.heap.execution.take_rooted_error();
    let result = (|| {
        let callback_roots = ctx.heap.temporary_mark();
        let (heap, roots) = ctx.heap_and_roots();
        let callbacks = heap.clear_dealloc_weakrefs(owner, roots)?;
        ctx.run_weak_callbacks(callbacks);
        ctx.heap.release_temporaries_since(callback_roots);

        let pair = state(ctx, owner);
        store(
            ctx,
            owner,
            Pair {
                reader: None,
                ..pair
            },
        );
        release_child(ctx, pair.reader)?;
        // A reader finalizer can run Python; keep the writer and owner alive until now.
        let pair = state(ctx, owner);
        store(
            ctx,
            owner,
            Pair {
                writer: None,
                ..pair
            },
        );
        release_child(ctx, pair.writer)
    })();
    ctx.heap.execution.pending = pending;
    ctx.heap.release_temporaries_since(mark);
    result.map(|()| true)
}

fn initialize(ctx: &mut Context, owner: Id, receiver: Id, args: &[Id]) -> Result<Id> {
    if !(2..=3).contains(&args.len()) {
        let bound = if args.len() < 2 {
            "at least 2"
        } else {
            "at most 3"
        };
        return Err(Error::new(
            "TypeError",
            format!(
                "BufferedRWPair expected {bound} arguments, got {}",
                args.len()
            ),
        ));
    }
    let capacity = args
        .get(2)
        .map(|&n| super::descriptor_buffer::ssize(ctx, n))
        .transpose()?
        .unwrap_or(super::DEFAULT_BUFFER_SIZE);
    for (raw, capability) in [(args[0], "readable"), (args[1], "writable")] {
        let method = ctx.lookup_attr(raw, capability)?;
        if ctx.call(method, Vec::new(), Vec::new())? != TRUE {
            return Err(Error::new(
                "UnsupportedOperation",
                format!("File or stream is not {capability}."),
            ));
        }
    }
    let Value::Class {
        io_kind: Some(NativeClass::Pair { reader, writer }),
        ..
    } = *ctx.heap.get(owner)
    else {
        unreachable!("BufferedRWPair initializer owner")
    };
    let size = ctx.int(capacity);
    ctx.heap.keep(size);
    let new_reader = ctx.call(reader, vec![args[0], size], Vec::new());
    let mut pair = state(ctx, receiver);
    pair.reader = new_reader.as_ref().ok().copied();
    store(ctx, receiver, pair);
    new_reader?;

    let new_writer = ctx.call(writer, vec![args[1], size], Vec::new());
    let mut pair = state(ctx, receiver);
    match new_writer {
        Ok(writer) => {
            pair.writer = Some(writer);
            store(ctx, receiver, pair);
            Ok(NONE)
        }
        Err(error) => {
            for id in error.roots() {
                ctx.heap.keep(id);
            }
            let reader = pair.reader.take();
            pair.writer = None;
            store(ctx, receiver, pair);
            // The pair owns this private reader. Run its ordinary, once-only file
            // finalizer just as clearing the last C reference does during rollback.
            if let Some(reader) = reader {
                ctx.dispatch_finalizer(reader);
            }
            Err(error)
        }
    }
}

fn forward(ctx: &mut Context, stream: Option<Id>, name: &str, args: &[Id]) -> Result<Id> {
    let stream =
        stream.ok_or_else(|| Error::new("ValueError", "I/O operation on uninitialized object"))?;
    let method = ctx.lookup_attr(stream, name)?;
    ctx.call(method, args.to_vec(), Vec::new())
}

pub(super) fn call(
    ctx: &mut Context,
    owner: Id,
    receiver: Id,
    name: &str,
    args: &[Id],
    kwargs: &[(String, Id)],
) -> Result<Id> {
    let transfer = matches!(
        name,
        "read" | "read1" | "peek" | "readinto" | "readinto1" | "write"
    );
    if !kwargs.is_empty()
        && !(name == "__init__"
            && ctx
                .slot(receiver, "__new__")
                .is_some_and(|new| Some(&new) != ctx.builtins.get("object.__new__")))
    {
        let prefix = if name == "__init__" {
            "BufferedRWPair".into()
        } else if transfer {
            name.into()
        } else {
            format!("BufferedRWPair.{name}")
        };
        return Err(Error::new(
            "TypeError",
            format!("{prefix}() takes no keyword arguments"),
        ));
    }
    if name == "__init__" {
        return initialize(ctx, owner, receiver, args);
    }
    if !transfer && name != "__exit__" && !args.is_empty() {
        return Err(Error::new(
            "TypeError",
            format!(
                "BufferedRWPair.{name}() takes no arguments ({} given)",
                args.len()
            ),
        ));
    }
    let pair = state(ctx, receiver);
    match name {
        "closed" => {
            let writer = pair.writer.ok_or_else(|| {
                Error::new(
                    "RuntimeError",
                    "the BufferedRWPair object is being garbage-collected",
                )
            })?;
            ctx.lookup_attr(writer, "closed")
        }
        "close" => {
            let writer = forward(ctx, pair.writer, "close", &[]);
            if let Err(error) = &writer {
                for id in error.roots() {
                    ctx.heap.keep(id);
                }
            }
            let reader = state(ctx, receiver).reader;
            match (writer, forward(ctx, reader, "close", &[])) {
                (Err(first), Err(mut second)) => {
                    second.context = ctx.raised_instance(first);
                    Err(second)
                }
                (Err(first), _) => Err(first),
                (_, result) => result,
            }
        }
        "isatty" => {
            let result = forward(ctx, pair.writer, name, &[])?;
            if result == FALSE {
                let reader = state(ctx, receiver).reader;
                return forward(ctx, reader, name, &[]);
            }
            Ok(result)
        }
        "__enter__" => {
            let closed = ctx.lookup_attr(receiver, "closed")?;
            if ctx.truth(closed)? {
                return Err(Error::new("ValueError", "I/O operation on closed file."));
            }
            Ok(receiver)
        }
        "__exit__" => {
            let close = ctx.lookup_attr(receiver, "close")?;
            ctx.call(close, Vec::new(), Vec::new())
        }
        "flush" | "writable" | "write" => forward(ctx, pair.writer, name, args),
        _ => forward(ctx, pair.reader, name, args),
    }
}
