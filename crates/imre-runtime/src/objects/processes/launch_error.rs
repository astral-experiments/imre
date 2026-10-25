//! Report the original Python filename of the syscall that actually failed.

use super::*;
use crate::objects::exceptions::Filename;
use crate::objects::files::errno_with_filenames;
use crate::objects::records::descriptor;

#[derive(Clone, Copy)]
pub(super) enum Site {
    Directory,
    Executable,
}

/// Command propagates a pre_exec closure's raw i32 through its close-on-exec
/// error channel. Unix errno values are positive. Encode only these two child
/// failures as negative numbers, so a pipe, fork, or descriptor error cannot
/// acquire an unrelated filename. No second pipe is needed under RLIMIT_NOFILE.
pub(super) fn child_errno(site: Site, errno: i32) -> io::Error {
    io::Error::from_raw_os_error(-(errno * 2 + i32::from(matches!(site, Site::Executable))))
}

pub(super) fn decode(error: io::Error) -> (Option<Site>, io::Error) {
    match error.raw_os_error().and_then(i32::checked_neg) {
        Some(tag) if tag >= 2 => (
            Some(if tag % 2 == 0 {
                Site::Directory
            } else {
                Site::Executable
            }),
            io::Error::from_raw_os_error(tag / 2),
        ),
        _ => (None, error),
    }
}

/// # Safety
/// The compiler roots the command and the exact cwd/executable owners in config.
pub(super) unsafe fn spawn(
    cmd: &OwnedWord,
    config: &popen::LaunchConfig,
    streams: u32,
) -> Result<Child> {
    // SAFETY: validation and copying complete before the command may fork.
    let (launch, cwd) = unsafe { prepare_command(cmd, config)? };
    launch.spawn_tagged(cwd.as_ref(), streams).map_err(|err| {
        let (site, err) = decode(err);
        // SAFETY: failure does not release the original command or its arguments.
        let name = site.map(|site| unsafe {
            match site {
                Site::Directory => {
                    filename(config.cwd as usize, WordKind::new(config.cwd_kind), true)
                }
                Site::Executable if !config.executable.is_null() => filename(
                    config.executable as usize,
                    WordKind::new(config.executable_kind),
                    false,
                ),
                Site::Executable if config.shell => {
                    Filename::Str(Rc::new(Str::from(if cfg!(target_os = "android") {
                        "/system/bin/sh"
                    } else {
                        "/bin/sh"
                    })))
                }
                Site::Executable => {
                    let (word, kind) = match cmd.kind.raw() {
                        5 => object(cmd.word as *const Object).0.with(|s| match s {
                            Storage::Sequence(list) => (list.values[0], list.kind),
                            _ => (cmd.word, cmd.kind),
                        }),
                        6 => {
                            let tuple = cmd.word as *const usize;
                            let kind = WordKind::new(*descriptor(tuple).kinds);
                            (kind.read_record_field(tuple), kind)
                        }
                        _ => (cmd.word, cmd.kind),
                    };
                    filename(word, kind, false)
                }
            }
        });
        errno_with_filenames(err, name, None)
    })
}

/// cwd retains its original object; the executable passes through os.fspath.
unsafe fn filename(word: usize, kind: WordKind, keep_path: bool) -> Filename {
    let (word, kind) = if kind.raw() == 9 {
        crate::objects::unions::member(word)
    } else {
        (word, kind)
    };
    // SAFETY: argv validation admitted only rooted str, bytes, and path values.
    unsafe {
        match kind.raw() {
            3 => Filename::Str(crate::objects::acquire_rc(word)),
            8 => Filename::Bytes(crate::objects::acquire_rc(word)),
            5 if keep_path => Filename::Path(crate::objects::acquire_rc(word)),
            5 => object(word as *const Object)
                .0
                .with(|s| Filename::Str(Rc::new(Str::from(s.path())))),
            _ => fail("SystemError", "unexpected subprocess error filename"),
        }
    }
}
