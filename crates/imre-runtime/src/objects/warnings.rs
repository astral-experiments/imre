//! Native warning filters for resource finalizers. Display and filtering do
//! not run Python or interfere with the exception already being unwound.
use super::{Error, Kind, Str, exceptions, finish};
use std::collections::HashSet;
use std::sync::{LazyLock, Mutex};

#[derive(Clone, Copy, Eq, PartialEq)]
enum Action {
    Ignore,
    Always,
    Error,
    Default,
    Module,
    Once,
}

impl Action {
    fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "ignore" => Self::Ignore,
            "always" | "all" => Self::Always,
            "error" => Self::Error,
            "default" => Self::Default,
            "module" => Self::Module,
            "once" => Self::Once,
            _ => return None,
        })
    }
}

#[derive(Default)]
struct Filters {
    rules: Vec<(Action, u32)>,
    displayed: HashSet<String>,
}

static FILTERS: LazyLock<Mutex<Filters>> = LazyLock::new(|| Mutex::new(Filters::default()));

/// Install a native catch-all or category filter before existing rules.
///
/// # Safety
/// The caller roots the action string and supplies a statically known category.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_warning_simplefilter(action: *const Str, category: u32) {
    // SAFETY: the native compiler roots an exact string through this call.
    let action = unsafe { &*action }.as_str();
    let Some(action) = Action::parse(action) else {
        let repr = exceptions::string_repr(action);
        return finish(
            Err(Error::new(
                Kind::ValueError,
                format!("invalid action: {}", repr.as_str()),
            )),
            (),
        );
    };
    {
        let mut filters = FILTERS.lock().unwrap_or_else(|poison| poison.into_inner());
        filters.rules.retain(|rule| *rule != (action, category));
        filters.rules.insert(0, (action, category));
        filters.displayed.clear();
    }
}

/// Report the unreaped handle before the destructor performs its own poll.
pub(super) fn unobserved_child(pid: libc::pid_t) {
    let mut filters = FILTERS.lock().unwrap_or_else(|poison| poison.into_inner());
    let action = filters
        .rules
        .iter()
        .find(|(_, category)| matches!(category, 0 | 1))
        .map_or(Action::Ignore, |(action, _)| *action);
    if action == Action::Ignore {
        return;
    }
    let message = format!("subprocess {pid} is still running");
    // These warnings come from one module, category and line. Only the text
    // distinguishes locations for the default, module and once filters here.
    if matches!(action, Action::Default | Action::Module | Action::Once)
        && !filters.displayed.insert(message.clone())
    {
        return;
    }
    drop(filters);
    let diagnostic = if action == Action::Error {
        format!("Exception ignored in Popen.__del__:\nResourceWarning: {message}")
    } else {
        format!("{}:{}: ResourceWarning: {message}", file!(), line!())
    };
    let _ = crate::sys::write_stderr(&diagnostic, true);
}
