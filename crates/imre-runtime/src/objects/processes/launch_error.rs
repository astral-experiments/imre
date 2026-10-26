//! Report the original Python filename of the syscall that actually failed.

use super::*;
use crate::StrOwner;
use crate::objects::exceptions::Filename;
use crate::objects::files::errno_with_filenames;

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

/// The direct POSIX spawn interface reports its converted filesystem path.
/// The fork/exec interface retains the caller's original executable object.
/// This is observable even though both execute the same native program.
fn uses_posix_spawn(has_directory: bool, cwd: bool, streams: u32, fds: [RawFd; 3]) -> bool {
    if !has_directory || cwd {
        return false;
    }
    let [stdin, stdout, stderr] = [12, 0, 4].map(|shift| (streams >> shift) & 15);
    // Direct spawn requires each redirected child end to be absent or above
    // stderr. Use the original fd, before creating our CLOEXEC duplicates.
    if [stdin, stdout, stderr]
        .into_iter()
        .zip(fds)
        .any(|(mode, fd)| matches!(mode, 3 | 4 | 8) || matches!(mode, 6 | 7) && fd != -1 && fd <= 2)
        || stdin == 5
        || stderr == 5 && stdout == 0
    {
        return false;
    }
    static CAPABILITIES: std::sync::OnceLock<(bool, bool)> = std::sync::OnceLock::new();
    let &(spawn, closefrom) = CAPABILITIES.get_or_init(|| {
        let spawn = match std::env::var_os("_PYTHON_SUBPROCESS_USE_POSIX_SPAWN").as_deref() {
            Some(value) if value == "0" => false,
            Some(value) if value == "1" => true,
            _ => spawn_reports_exec_errors(),
        };
        // SAFETY: query an optional libc symbol without calling it. Older glibc
        // and Darwin lack this file action even when posix_spawn itself works.
        let closefrom = unsafe {
            !libc::dlsym(
                libc::RTLD_DEFAULT,
                c"posix_spawn_file_actions_addclosefrom_np".as_ptr(),
            )
            .is_null()
        };
        (spawn, closefrom)
    });
    spawn && (closefrom || streams & (1 << 17) != 0)
}

#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn spawn_reports_exec_errors() -> bool {
    // SAFETY: glibc returns a permanent NUL-terminated version string.
    let version = unsafe { std::ffi::CStr::from_ptr(libc::gnu_get_libc_version()) };
    let mut parts = version.to_str().unwrap_or_default().split('.');
    let mut next = || {
        parts
            .next()
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(0)
    };
    (next(), next()) >= (2, 24)
}

#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
fn spawn_reports_exec_errors() -> bool {
    cfg!(any(target_os = "macos", target_os = "solaris"))
}

/// Inspect the executable for launch selection before a C API validates its bytes.
unsafe fn has_directory(cmd: &OwnedWord, config: &popen::LaunchConfig) -> bool {
    let program = if config.executable.is_null() {
        if config.shell { None } else { argv::first(cmd) }
    } else {
        Some((
            config.executable as usize,
            WordKind::new(config.executable_kind),
        ))
    };
    program.map_or(config.shell, |(word, kind)| {
        // SAFETY: the rooted original program may contain NUL at this stage.
        unsafe { argv::with_bytes(word, kind, |bytes| bytes.contains(&b'/')) }
    })
}

/// # Safety
/// The compiler roots the command and the exact cwd/executable owners in config.
pub(super) unsafe fn spawn(
    cmd: &OwnedWord,
    config: &popen::LaunchConfig,
    streams: u32,
    fds: [RawFd; 3],
    oversized_fd: bool,
    child_attributes: impl FnOnce(bool) -> Result<ChildAttributes>,
) -> Result<Child> {
    // SAFETY: the caller roots the command until both launch APIs finish validation.
    let may_spawn = uses_posix_spawn(
        unsafe { has_directory(cmd, config) },
        !config.cwd.is_null(),
        streams,
        fds,
    );
    // SAFETY: validation and copying complete before the command may fork.
    let (launch, cwd, attributes) = unsafe {
        prepare_command(cmd, config, || {
            let attributes = child_attributes(may_spawn)?;
            if oversized_fd && (!may_spawn || attributes.requires_fork()) {
                return Err(c_int_overflow());
            }
            Ok(attributes)
        })?
    };
    let direct = may_spawn && !attributes.requires_fork();
    if launch.argv.strings.is_empty() && direct {
        return Err(Error::new(
            Kind::ValueError,
            "posix_spawn: argv must not be empty",
        ));
    }
    if oversized_fd {
        return Err(c_int_overflow());
    }
    let streams = if attributes.keep.is_empty() {
        streams
    } else {
        streams & !(1 << 17) // pass_fds closes all other inherited descriptors
    };
    launch
        .spawn_tagged(cwd.as_ref(), streams, fds, attributes)
        .map_err(|err| {
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
                        !direct,
                    ),
                    Site::Executable if config.shell => {
                        Filename::Str(StrOwner::new(Str::from(if cfg!(target_os = "android") {
                            "/system/bin/sh"
                        } else {
                            "/bin/sh"
                        })))
                    }
                    Site::Executable => {
                        let (word, kind) = argv::first(cmd).expect("validated nonempty argv");
                        filename(word, kind, !direct)
                    }
                }
            });
            errno_with_filenames(err, name, None)
        })
}

/// Only the direct spawn interface converts an executable with os.fspath.
unsafe fn filename(word: usize, kind: WordKind, keep_path: bool) -> Filename {
    let (word, kind) = if kind.raw() == 9 {
        crate::objects::unions::member(word)
    } else {
        (word, kind)
    };
    // SAFETY: argv validation admitted only rooted str, bytes, and path values.
    unsafe {
        match kind.raw() {
            3 => Filename::Str(StrOwner::acquire_word(word)),
            8 => Filename::Bytes(crate::acquire_bytes(word)),
            5 if keep_path => Filename::Path(crate::ObjectOwner::acquire_word(word)),
            5 => object(word as *const Object)
                .0
                .with(|s| Filename::Str(StrOwner::new(Str::from(s.path())))),
            _ => fail("SystemError", "unexpected subprocess error filename"),
        }
    }
}
