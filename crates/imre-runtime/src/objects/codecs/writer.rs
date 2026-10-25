//! Codec writer classes are shared by encoding; each instance owns its stream and encoder state.
use super::{Codec, Error, Kind, Object, Rc, Storage, StorageCell, Str, codec, finish, object};
use crate::objects::acquire_rc;
use std::cell::RefCell;
use std::collections::HashMap;

thread_local! {
    static CLASSES: RefCell<HashMap<Codec, Rc<Object>>> = RefCell::default();
}

pub(in crate::objects) struct WriterClass {
    codec: Codec,
    encoding: Option<Rc<Str>>,
}

pub(in crate::objects) struct Writer {
    class: Rc<Object>,
    stream: Rc<Object>,
    errors: Rc<Str>,
    first: bool,
}

fn class_of(storage: &Storage) -> &WriterClass {
    let Storage::CodecWriterClass(class) = storage else {
        unreachable!("expected a codec stream writer class")
    };
    class
}

fn writer_of(storage: &Storage) -> &Writer {
    let Storage::CodecWriter(writer) = storage else {
        unreachable!("expected a codec stream writer")
    };
    writer
}

/// # Safety
/// The entry is a rooted CodecInfo. Aliases share the writer class for their encoding.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_codec_writer_class(info: *const Object) -> *const Object {
    // SAFETY: the caller supplies an actual registry entry.
    let codec = unsafe { codec(info) };
    CLASSES.with_borrow_mut(|classes| {
        let class = classes.entry(codec).or_insert_with(|| {
            Rc::new(Object(StorageCell::new(Storage::CodecWriterClass(
                WriterClass {
                    codec,
                    encoding: None,
                },
            ))))
        });
        Rc::into_raw(class.clone())
    })
}

/// # Safety
/// The class, underlying binary stream and error policy stay rooted through construction.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_codec_writer_new(
    class: *const Object,
    stream: *const Object,
    errors: *const Str,
) -> *const Object {
    // SAFETY: each input points to the allocation named by the native signature.
    let (class, stream, errors) = unsafe {
        (
            acquire_rc(class as usize),
            acquire_rc(stream as usize),
            acquire_rc(errors as usize),
        )
    };
    Rc::into_raw(Rc::new(Object(StorageCell::new(Storage::CodecWriter(
        Writer {
            class,
            stream,
            errors,
            first: true,
        },
    )))))
}

/// # Safety
/// The class and assigned text stay rooted until the new class attribute owns its value.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_codec_writer_set_encoding(value: *const Str, class: *const Object) {
    // SAFETY: generated code keeps both operands alive during assignment.
    unsafe {
        let value = acquire_rc(value as usize);
        let old = object(class).0.with_mut(|s| {
            let Storage::CodecWriterClass(class) = s else {
                unreachable!("expected a codec stream writer class")
            };
            class.encoding.replace(value)
        });
        drop(old);
    }
}

/// # Safety
/// The operand is a rooted writer class.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_codec_writer_encoding(value: *const Object) -> *const Str {
    // SAFETY: the class owns the attribute if a caller set it.
    let text = unsafe { object(value).0.with(|s| class_of(s).encoding.clone()) };
    finish(
        text.map(Rc::into_raw).ok_or_else(|| {
            Error::new(
                Kind::AttributeError,
                "type object 'StreamWriter' has no attribute 'encoding'",
            )
        }),
        std::ptr::null(),
    )
}

/// # Safety
/// The writer stays rooted while returning an owner for the original binary stream.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_codec_writer_stream(writer: *const Object) -> *const Object {
    // SAFETY: the writer owns this exact stream.
    unsafe {
        object(writer)
            .0
            .with(|s| Rc::into_raw(writer_of(s).stream.clone()))
    }
}

/// # Safety
/// The writer stays rooted while returning its stored error policy.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_codec_writer_errors(writer: *const Object) -> *const Str {
    // SAFETY: the writer owns the stored string.
    unsafe {
        object(writer)
            .0
            .with(|s| Rc::into_raw(writer_of(s).errors.clone()))
    }
}

/// # Safety
/// The writer and text are rooted. Encode before writing; a closed stream can still reject the bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_codec_writer_write(writer: *const Object, text: *const Str) -> u32 {
    // SAFETY: the caller roots both operands until the stream operation completes.
    let (writer, text) = unsafe { (object(writer), &*text) };
    // SAFETY: the writer, its class, and its binary stream are distinct objects.
    // Encoding and file I/O cannot call back into the writer.
    let result = unsafe {
        writer.0.with_mut(|s| {
            let Storage::CodecWriter(writer) = s else {
                unreachable!("expected a codec stream writer")
            };
            let mut codec = writer.class.0.with(|s| class_of(s).codec);
            if let Codec::Utf8 { signature } = &mut codec {
                *signature &= writer.first;
            }
            let bytes = codec.encode(text, Some(&writer.errors))?;
            writer.first = false;
            writer
                .stream
                .0
                .with_mut(|s| s.file_mut().write_bytes(&bytes))
        })
    };
    finish(result.map(|()| 1), 0)
}

/// # Safety
/// The writer is rooted and no references to its storage escape.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_codec_writer_reset(writer: *const Object) {
    // SAFETY: the caller supplies the exact native writer type.
    unsafe {
        object(writer).0.with_mut(|s| {
            let Storage::CodecWriter(writer) = s else {
                unreachable!("expected a codec stream writer")
            };
            writer.first = true;
        })
    };
}
