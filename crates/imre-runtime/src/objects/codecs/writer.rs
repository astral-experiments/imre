//! The codec class is shared across registry aliases; each instance owns its stream state.
use super::*;
use crate::objects::{acquire_rc, files::close_stream};
use std::cell::Cell;

pub(in crate::objects) struct WriterClass {
    codec: Codec,
    encoding: RefCell<Option<Rc<Str>>>,
}

pub(in crate::objects) struct Writer {
    class: Rc<Object>,
    pub(in crate::objects) stream: Rc<Object>,
    errors: RefCell<Rc<Str>>,
    encoding: RefCell<Option<Rc<Str>>>,
    first: Cell<bool>,
}

thread_local! {
    static CLASSES: RefCell<HashMap<usize, Rc<Object>>> = RefCell::default();
}

/// Writer objects never change their storage variant. Python attributes use interior cells.
fn inspect<T>(owner: &Object, read: impl FnOnce(&Storage) -> T) -> T {
    // SAFETY: only rooted writer classes or writers enter here; neither mutates its outer storage.
    unsafe { owner.0.with(read) }
}

/// # Safety
/// The caller owns a live registry entry throughout the class lookup.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_codec_writer_class(info: *const Object) -> *const Object {
    // SAFETY: the compiler accepts only an entry returned by the registry.
    let codec = unsafe { codec(info) };
    CLASSES.with_borrow_mut(|classes| {
        let class = classes.entry(codec.index()).or_insert_with(|| {
            Rc::new(Object(StorageCell::new(Storage::CodecWriterClass(
                WriterClass {
                    codec,
                    encoding: RefCell::default(),
                },
            ))))
        });
        Rc::into_raw(class.clone())
    })
}

/// # Safety
/// Class, underlying binary file and error handler are rooted by the caller.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_codec_writer_new(
    class: *const Object,
    stream: *const Object,
    errors: *const Str,
) -> *const Object {
    // SAFETY: each input is an Rc of the exact native type indicated by the ABI.
    let (class, stream, errors) = unsafe {
        (
            acquire_rc(class as usize),
            acquire_rc(stream as usize),
            acquire_rc(errors as usize),
        )
    };
    Rc::into_raw(Rc::new(Object(StorageCell::new(Storage::CodecWriter(
        Box::new(Writer {
            class,
            stream,
            errors: RefCell::new(errors),
            encoding: RefCell::default(),
            first: Cell::new(true),
        }),
    )))))
}

impl Writer {
    /// Complete encoding before touching the stream, preserving the previous bytes on failure.
    pub(in crate::objects) fn write(
        &self,
        text: &str,
        error_input: impl FnOnce() -> Rc<Str>,
    ) -> Result<()> {
        let mut codec = inspect(&self.class, |s| match s {
            Storage::CodecWriterClass(class) => class.codec,
            _ => unreachable!(),
        });
        if let Codec::Utf8 { signature: true } = codec {
            codec = Codec::Utf8 {
                signature: self.first.replace(false),
            };
        }
        let bytes = codec.encode(text, Some(&self.errors.borrow()), error_input)?;
        // SAFETY: the wrapped object is a distinct binary file whose write cannot reenter.
        unsafe { self.stream.0.with_mut(|s| s.file_mut().write_bytes(&bytes)) }
    }

    pub(in crate::objects) fn flush(&self) -> Result<()> {
        // SAFETY: the wrapped file cannot call back into the writer during flush.
        unsafe { self.stream.0.with_mut(|s| s.file_mut().flush()) }
    }
}

/// # Safety
/// The generated caller roots the writer and the input string until write finishes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_codec_writer_write(writer: *const Object, text: *const Str) {
    // SAFETY: the generated native types are a writer and a Unicode string.
    let (writer, text) = unsafe { (object(writer), &*text) };
    let result = inspect(writer, |s| match s {
        Storage::CodecWriter(w) => w.write(text, || {
            // SAFETY: the generated caller retains this exact input for the whole write.
            unsafe { acquire_rc(text as *const Str as usize) }
        }),
        _ => unreachable!(),
    });
    if let Err(err) = result {
        err.raise();
    }
}

/// # Safety
/// The rooted writer retains its underlying stream through flush, close or reset.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_codec_writer_control(writer: *const Object, mode: u32) {
    // SAFETY: the receiver is a rooted codec writer.
    let result = inspect(unsafe { object(writer) }, |s| match s {
        Storage::CodecWriter(w) => match mode {
            0 => w.flush(),
            1 => close_stream(&w.stream),
            2 => {
                w.first.set(true);
                Ok(())
            }
            _ => unreachable!(),
        },
        _ => unreachable!(),
    });
    if let Err(err) = result {
        err.raise();
    }
}

/// # Safety
/// The receiver is a live writer instance or its exact codec class.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_codec_writer_encoding(owner: *const Object) -> *const Str {
    // SAFETY: the generated caller roots its receiver through the attribute read.
    let result = inspect(unsafe { object(owner) }, |s| match s {
        Storage::CodecWriterClass(c) => c
            .encoding
            .borrow()
            .clone()
            .ok_or_else(|| "type object 'StreamWriter' has no attribute 'encoding'".to_owned()),
        Storage::CodecWriter(w) => w
            .encoding
            .borrow()
            .clone()
            .or_else(|| {
                inspect(&w.class, |s| match s {
                    Storage::CodecWriterClass(c) => c.encoding.borrow().clone(),
                    _ => unreachable!(),
                })
            })
            .ok_or_else(|| {
                // SAFETY: the separately rooted binary stream is inspected without callbacks.
                let name = unsafe { w.stream.0.with(|s| s.file().binary_type_name()) };
                format!("'{name}' object has no attribute 'encoding'")
            }),
        _ => unreachable!(),
    });
    finish(
        result
            .map(Rc::into_raw)
            .map_err(|msg| Error::new(Kind::AttributeError, msg)),
        std::ptr::null(),
    )
}

/// # Safety
/// The generated caller roots the attribute value before evaluating the receiver.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_codec_writer_set_encoding(text: *const Str, owner: *const Object) {
    // SAFETY: the owned string outlives the store; the receiver stays rooted.
    let (text, owner) = unsafe { (acquire_rc(text as usize), object(owner)) };
    inspect(owner, |s| match s {
        Storage::CodecWriterClass(c) => *c.encoding.borrow_mut() = Some(text),
        Storage::CodecWriter(w) => *w.encoding.borrow_mut() = Some(text),
        _ => unreachable!(),
    });
}

/// # Safety
/// The caller roots the writer while acquiring the underlying binary file.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_codec_writer_stream(writer: *const Object) -> *const Object {
    // SAFETY: this exact native writer owns a live file.
    inspect(unsafe { object(writer) }, |s| match s {
        Storage::CodecWriter(w) => Rc::into_raw(w.stream.clone()),
        _ => unreachable!(),
    })
}

/// # Safety
/// The writer stays rooted while its error-handler string is acquired.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_codec_writer_errors(writer: *const Object) -> *const Str {
    // SAFETY: the type checker only emits this read for a writer instance.
    inspect(unsafe { object(writer) }, |s| match s {
        Storage::CodecWriter(w) => Rc::into_raw(w.errors.borrow().clone()),
        _ => unreachable!(),
    })
}

/// # Safety
/// The string and writer are borrowed through an instance attribute assignment.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_codec_writer_set_errors(text: *const Str, owner: *const Object) {
    // SAFETY: acquire the RHS before replacing the receiver's previous value.
    let (text, owner) = unsafe { (acquire_rc(text as usize), object(owner)) };
    inspect(owner, |s| match s {
        Storage::CodecWriter(w) => *w.errors.borrow_mut() = text,
        _ => unreachable!(),
    });
}
