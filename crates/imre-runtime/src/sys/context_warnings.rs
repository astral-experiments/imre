//! Process startup snapshot for context-aware warnings on a GIL-enabled build.
use std::sync::OnceLock;

static CONTEXT_AWARE_WARNINGS: OnceLock<bool> = OnceLock::new();

/// A Python launcher passes its validated setting; a standalone ELF selects it at startup.
#[unsafe(no_mangle)]
pub extern "C" fn imre_context_warnings_init(selected: i32) {
    CONTEXT_AWARE_WARNINGS.get_or_init(|| {
        if selected >= 0 {
            return selected != 0;
        }
        std::env::var_os("PYTHON_CONTEXT_AWARE_WARNINGS").is_some_and(|raw| {
            imre_ast::startup::parse_context_flag(&raw).unwrap_or_else(|| {
                eprintln!("Fatal Python error: config_init_context_aware_warnings: PYTHON_CONTEXT_AWARE_WARNINGS=N: N is missing or invalid\nPython runtime state: preinitialized\n");
                std::process::exit(1)
            })
        })
    });
}

/// Return the captured integer flag independently of later changes to os.environ.
#[unsafe(no_mangle)]
pub extern "C" fn imre_sys_context_aware_warnings() -> i64 {
    i64::from(CONTEXT_AWARE_WARNINGS.get().copied().unwrap_or(false))
}
