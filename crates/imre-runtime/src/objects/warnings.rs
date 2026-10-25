//! Native warning filtering and display for explicit calls, subprocess launch, and finalizers.
//! A warning raised during finalization must preserve the exception being unwound.
mod display;
use super::{Error, Int, Kind, Str, exceptions, finish};
use num_bigint::BigInt;
use num_traits::{Signed, ToPrimitive};
use regex_automata::{Anchored, Input, meta::Regex};
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

struct Pattern {
    original: String,
    regex: Regex,
}

impl Pattern {
    fn compile(original: &str, translated: &str, insensitive: bool) -> crate::Result<Option<Self>> {
        if original.is_empty() {
            return Ok(None);
        }
        let regex = Regex::builder()
            .syntax(regex_automata::util::syntax::Config::new().case_insensitive(insensitive))
            .build(translated)
            .map_err(|_| {
                Error::new(
                    Kind::MemoryError,
                    "warning filter pattern exceeds native regex limits",
                )
            })?;
        Ok(Some(Self {
            original: original.into(),
            regex,
        }))
    }
}

struct Rule {
    name: String,
    action: Action,
    category: u32,
    message: Option<Pattern>,
    module: Option<Pattern>,
    lineno: BigInt,
}

impl Rule {
    fn same(&self, other: &Self) -> bool {
        fn source(pattern: &Option<Pattern>) -> Option<&str> {
            pattern.as_ref().map(|p| p.original.as_str())
        }
        self.category == other.category
            && self.name == other.name
            && self.lineno == other.lineno
            && source(&self.message) == source(&other.message)
            && source(&self.module) == source(&other.module)
    }

    fn matches(&self, text: &str, category: u32, module: &str, lineno: isize) -> Option<bool> {
        fn matches(pattern: &Option<Pattern>, text: &str) -> bool {
            pattern
                .as_ref()
                .is_none_or(|p| p.regex.is_match(Input::new(text).anchored(Anchored::Yes)))
        }
        // CPython converts every visited rule's line to Py_ssize_t during
        // emission, even when that rule's category or regex would not match.
        let line = self.lineno.to_isize()?;
        Some(
            (self.category == 0 || self.category == category)
                && (line == 0 || line == lineno)
                && matches(&self.message, text)
                && matches(&self.module, module),
        )
    }
}

#[derive(Eq, Hash, PartialEq)]
enum Scope {
    Line(&'static str, isize),
    Module(&'static str),
    Process,
}

struct Filters {
    rules: Vec<Rule>,
    displayed: HashSet<(u32, String, Scope)>,
}

// Release defaults, in CPython order. These are ordinary rules: reset removes
// them, and appending a rule cannot supersede an earlier match.
static FILTERS: LazyLock<Mutex<Filters>> = LazyLock::new(|| {
    let mut rules = vec![Rule {
        name: "default".into(),
        action: Action::Default,
        category: 4,
        message: None,
        module: Some(Pattern {
            original: "__main__".into(),
            regex: Regex::new(r"__main__\z").expect("fixed main module name"),
        }),
        lineno: BigInt::ZERO,
    }];
    rules.extend([4, 5, 8, 1].map(|category| Rule {
        name: "ignore".into(),
        action: Action::Ignore,
        category,
        message: None,
        module: None,
        lineno: BigInt::ZERO,
    }));
    Mutex::new(Filters {
        rules,
        displayed: HashSet::new(),
    })
});

/// Install a checked warning filter; expressions were evaluated in Python order.
///
/// # Safety
/// The caller roots five string arguments and one integer until the call finishes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_warning_filter(
    action: *const Str,
    message: *const Str,
    message_re: *const Str,
    module: *const Str,
    module_re: *const Str,
    lineno: usize,
    append: u32,
    category: u32,
    invalid: u32,
) {
    // SAFETY: the named arguments remain rooted by the native frame.
    let [name, message, message_re, module, module_re] =
        [action, message, message_re, module, module_re].map(|s| unsafe { &*s }.as_str());
    // SAFETY: lineno is a retained integer handle for the duration of this call.
    let integer = unsafe { Int::borrow(lineno) };
    let lineno = integer.big();
    finish(
        (|| {
            let action = Action::parse(name).ok_or_else(|| {
                Error::new(
                    Kind::ValueError,
                    format!("invalid action: {}", exceptions::string_repr(name).as_str()),
                )
            })?;
            for (bit, reason) in [
                (1, "message must be a string"),
                (2, "category must be a Warning subclass"),
                (4, "module must be a string"),
                (8, "lineno must be an int"),
            ] {
                if invalid & bit != 0 {
                    return Err(Error::new(Kind::TypeError, reason));
                }
            }
            if lineno.is_negative() {
                return Err(Error::new(Kind::ValueError, "lineno must be an int >= 0"));
            }
            let rule = Rule {
                name: name.into(),
                action,
                category,
                lineno: lineno.into_owned(),
                message: Pattern::compile(message, message_re, true)?,
                module: Pattern::compile(module, module_re, false)?,
            };
            let mut filters = FILTERS.lock().unwrap_or_else(|poison| poison.into_inner());
            if append == 0 {
                filters.rules.retain(|old| !old.same(&rule));
                filters.rules.insert(0, rule);
            } else if !filters.rules.iter().any(|old| old.same(&rule)) {
                filters.rules.push(rule);
            }
            filters.displayed.clear();
            Ok(())
        })(),
        (),
    );
}

/// Clear filters, including the initial ResourceWarning ignore rule.
#[unsafe(no_mangle)]
pub extern "C" fn imre_warning_reset() {
    let mut filters = FILTERS.lock().unwrap_or_else(|poison| poison.into_inner());
    filters.rules.clear();
    filters.displayed.clear();
}

/// Preserve the runtime ABI used by previously compiled two-argument simplefilter calls.
///
/// # Safety
/// The caller holds an exact action string until this function returns.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_warning_simplefilter(action: *const Str, category: u32) {
    let empty = Str::new("");
    // SAFETY: the caller roots action; all four empty patterns outlive the
    // call. Zero is an immediate integer and does not own an allocation.
    unsafe {
        imre_warning_filter(
            action,
            &empty,
            &empty,
            &empty,
            &empty,
            Int::from_usize(0).into_raw(),
            0,
            category,
            0,
        );
    }
}

/// Decide whether to display or raise a warning. Every visited rule converts
/// its line number to Py_ssize_t, even when the category would not match.
fn filtered(
    category: u32,
    module: &'static str,
    line: isize,
    describe: impl Fn() -> String,
) -> Result<Option<(Action, String)>, ()> {
    let mut message = None;
    let mut filters = FILTERS.lock().unwrap_or_else(|poison| poison.into_inner());
    let mut action = Action::Default;
    for rule in &filters.rules {
        let text = if rule.message.is_some() {
            message.get_or_insert_with(&describe).as_str()
        } else {
            ""
        };
        if rule.matches(text, category, module, line).ok_or(())? {
            action = rule.action;
            break;
        }
    }
    if action == Action::Ignore {
        return Ok(None);
    }
    let message = message.unwrap_or_else(describe);
    let scope = match action {
        Action::Default => Some(Scope::Line(module, line)),
        Action::Module => Some(Scope::Module(module)),
        Action::Once => Some(Scope::Process),
        _ => None,
    };
    if let Some(scope) = scope
        && !filters.displayed.insert((category, message.clone(), scope))
    {
        return Ok(None);
    }
    Ok(Some((action, message)))
}

const LINE_OVERFLOW: &str = "Python int too large to convert to C ssize_t";

/// Report the unreaped handle before the destructor performs its own poll.
pub(super) fn unobserved_child(pid: libc::pid_t) {
    const LINE: isize = 1139; // subprocess.Popen.__del__
    let diagnostic = match filtered(1, "subprocess", LINE, || {
        format!("subprocess {pid} is still running")
    }) {
        Ok(None) => return,
        Ok(Some((Action::Error, message))) => {
            format!("Exception ignored in Popen.__del__:\nResourceWarning: {message}")
        }
        Ok(Some((_, message))) => format!("subprocess.py:{LINE}: ResourceWarning: {message}"),
        Err(()) => format!("Exception ignored in Popen.__del__:\nOverflowError: {LINE_OVERFLOW}"),
    };
    let _ = crate::sys::write_stderr(&diagnostic, true);
}

/// CPython checks this argument combination before obtaining the stdio handles.
pub(super) fn pass_fds_override() -> crate::Result<()> {
    const LINE: isize = 862; // subprocess.Popen.__init__
    match filtered(2, "subprocess", LINE, || {
        "pass_fds overriding close_fds.".into()
    })
    .map_err(|()| Error::new(Kind::OverflowError, LINE_OVERFLOW))?
    {
        Some((Action::Error, message)) => return Err(Error::new(Kind::RuntimeWarning, message)),
        Some((_, message)) => {
            let _ = crate::sys::write_stderr(
                &format!("subprocess.py:{LINE}: RuntimeWarning: {message}"),
                true,
            );
        }
        None => {}
    }
    Ok(())
}

/// A checked non-finalizer warning issued from an internal standard-library frame.
pub(super) fn deprecation(
    module: &'static str,
    filename: &str,
    line: isize,
    message: &str,
) -> crate::Result<()> {
    match filtered(4, module, line, || message.into())
        .map_err(|()| Error::new(Kind::OverflowError, LINE_OVERFLOW))?
    {
        Some((Action::Error, message)) => Err(Error::new(Kind::DeprecationWarning, message)),
        Some((_, message)) => {
            let _ = crate::sys::write_stderr(
                &format!("{filename}:{line}: DeprecationWarning: {message}"),
                true,
            );
            Ok(())
        }
        None => Ok(()),
    }
}
