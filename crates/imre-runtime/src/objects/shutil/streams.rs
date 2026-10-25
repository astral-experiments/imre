//! Copy from the current positions without flushing, closing, or reopening either stream.

use super::super::exceptions::{Result, finish};
use super::super::{Int, Object, object};

/// # Safety
/// The caller roots both file handles and the length. Each read releases its
/// storage borrow before the write, including when the two objects alias.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_shutil_copyfileobj(
    src: *const Object,
    dst: *const Object,
    size: usize,
) {
    // SAFETY: all three arguments remain live throughout this synchronous call.
    let result: Result<()> = unsafe {
        (|| {
            let size = Int::borrow(size);
            let default_size = Int::from_usize(super::COPY_BUFSIZE);
            let size = if size.as_small() == Some(0) {
                &default_size
            } else {
                &size
            };
            while let Some(chunk) = object(src).0.with_mut(|s| s.file_mut().read_chunk(size))? {
                object(dst)
                    .0
                    .with_mut(|s| s.file_mut().write_chunk(chunk))?;
            }
            Ok(())
        })()
    };
    finish(result, ());
}
