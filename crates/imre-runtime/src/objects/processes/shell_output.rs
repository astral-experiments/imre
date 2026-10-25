//! Shell commands return combined stdout and stderr even after a nonzero exit.

use super::*;
use crate::objects::codecs::Codec;

/// Run the native shell, retain its signed exit status, and remove one final newline.
///
/// # Safety
/// The caller roots the command and optional strings. A non-null status slot is
/// the first word of a fresh `(int, str)` tuple and is written only on success.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_subprocess_shell_output(
    args: *const (),
    kind: u32,
    encoding: usize,
    errors: usize,
    status_slot: *mut usize,
) -> *const Str {
    let run = || -> Result<*const Str> {
        // SAFETY: a nonzero optional str word points to its rooted, immutable owner.
        let (encoding, errors) = unsafe {
            (
                (encoding as *const Str).as_ref(),
                (errors as *const Str).as_ref(),
            )
        };
        let codec = Codec::for_stream(encoding.map(Str::as_str))?;
        // stdout=PIPE and stderr=STDOUT merge both writes into one kernel pipe.
        let streams = 1 | (5 << 4);
        let mut config = popen::LaunchConfig::basic(
            args,
            kind,
            std::ptr::null(),
            std::ptr::null(),
            streams,
            0,
            true,
        );
        config.shell = true;
        // SAFETY: the command uses the same rooted scalar/list/tuple ABI as Popen.
        let command = unsafe { WordKind::new(kind).acquire(args as usize) };
        // SAFETY: the borrowed config points only to the same rooted command.
        let child = unsafe {
            launch_error::spawn(&command, &config, streams, [-1; 3], false, |_| {
                Ok(ChildAttributes::default())
            })
        }?;
        let (status, bytes, _) = capture(child).map_err(capture_error)?;
        let mut output = codec.decode_stream(&bytes, errors.map(Str::as_str), true)?;
        if output.ends_with('\n') {
            output.pop();
        }
        let output = StrOwner::try_new(Str::from(output)).map_err(|_| Error::allocation())?;
        if !status_slot.is_null() {
            // SAFETY: the caller owns this uninitialized integer field.
            unsafe { status_slot.write(exit_word(returncode(status))) };
        }
        Ok(StrOwner::into_raw(output))
    };
    finish(run(), std::ptr::null())
}
