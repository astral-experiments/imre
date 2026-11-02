//! Linux system-wide CPU count and the Python startup override.
static OVERRIDE: std::sync::OnceLock<i32> = std::sync::OnceLock::new();

/// A command provides its resolved setting; a standalone binary reads its own startup environment.
pub(super) fn initialize(selected: i32) {
    OVERRIDE.get_or_init(|| {
        if selected >= 0 {
            return selected;
        }
        imre_ast::startup::environment_cpu_count().unwrap_or_else(|| {
            eprintln!("{}", imre_ast::startup::CPU_COUNT_ERROR);
            std::process::exit(1)
        })
    });
}

/// Query the online logical processors just as CPython does, independently of process affinity.
pub(crate) fn online() -> Option<i32> {
    if let Some(&n) = OVERRIDE.get()
        && n > 0
    {
        return Some(n);
    }
    // SAFETY: sysconf takes only the standard name of the requested host limit.
    i32::try_from(unsafe { libc::sysconf(libc::_SC_NPROCESSORS_ONLN) })
        .ok()
        .filter(|n| *n > 0)
}
