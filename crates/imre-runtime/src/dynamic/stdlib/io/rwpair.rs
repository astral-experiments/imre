//! A BufferedRWPair owns separate buffered streams of its native module generation.

use super::{Context, Error, FALSE, Id, MemoryStream, NONE, NativeClass, Result, TRUE, Value};

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
    pair.writer = new_writer.as_ref().ok().copied();
    if new_writer.is_err() {
        pair.reader = None;
    }
    store(ctx, receiver, pair);
    new_writer.map(|_| NONE)
}

fn forward(ctx: &mut Context, stream: Option<Id>, name: &str, args: &[Id]) -> Result<Id> {
    let stream =
        stream.ok_or_else(|| Error::new("ValueError", "I/O operation on uninitialized object"))?;
    match ctx.lookup_attr(stream, name) {
        Ok(method) => ctx.call(method, args.to_vec(), Vec::new()),
        Err(error) if name == "peek" && error.kind == "AttributeError" => {
            // The native reader has FileIO storage but does not expose its own peek descriptor yet.
            let handle = super::descriptor_buffer::backing(ctx, stream).and_then(|backing| {
                if let Value::File(handle) = ctx.heap.get(backing) {
                    Some(handle.clone())
                } else {
                    None
                }
            });
            if let Some(handle) = handle
                && let super::super::files::BinaryFile::Descriptor(fd) = handle.as_ref()
            {
                return fd.peek(ctx, stream, args);
            }
            Err(error)
        }
        Err(error) => Err(error),
    }
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
    if !kwargs.is_empty() {
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
