//! Copy from the current positions without flushing, closing, or reopening either stream.

use super::super::exceptions::{Error, Kind, Result, finish};
use super::super::{Int, Object, object};

/// # Safety
/// Both handles are rooted exact file objects, with the indicated text or byte
/// element types. The size is a live language integer. Storage borrows end
/// between each read and write, including when both handles alias.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_shutil_copyfileobj(
    src: *const Object,
    dst: *const Object,
    size: usize,
    src_text: u32,
    dst_text: u32,
) {
    // SAFETY: the caller roots both objects and the canonical integer for this loop.
    let result: Result<()> = unsafe {
        (|| {
            let size = Int::borrow(size);
            let default_size = Int::small(256 * 1024).unwrap();
            let size = if size.as_small() == Some(0) {
                &default_size
            } else {
                &size
            };
            loop {
                if src_text == 0 {
                    let buf = object(src).0.with_mut(|s| s.file_mut().read_size(size))?;
                    if buf.is_empty() {
                        return Ok(());
                    }
                    if dst_text != 0 {
                        return Err(Error::new(
                            Kind::TypeError,
                            "write() argument must be str, not bytes",
                        ));
                    }
                    object(dst).0.with_mut(|s| s.file_mut().write_bytes(&buf))?;
                } else {
                    let text = object(src)
                        .0
                        .with_mut(|s| s.file_mut().read_text_size(size))?;
                    if text.is_empty() {
                        return Ok(());
                    }
                    if dst_text == 0 {
                        return Err(Error::new(
                            Kind::TypeError,
                            "a bytes-like object is required, not 'str'",
                        ));
                    }
                    object(dst).0.with_mut(|s| s.file_mut().write(&text))?;
                }
            }
        })()
    };
    finish(result, ());
}
