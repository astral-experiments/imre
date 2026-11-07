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
    /// Keep both accepted spellings distinct when deduplicating filter tuples.
    All,
    Error,
    Default,
    Module,
    Once,
}

impl Action {
    fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "ignore" => Self::Ignore,
            "always" => Self::Always,
            "all" => Self::All,
            "error" => Self::Error,
            "default" => Self::Default,
            "module" => Self::Module,
            "once" => Self::Once,
            _ => return None,
        })
    }
}

#[derive(Clone)]
struct Pattern {
    original: String,
    regex: Option<Regex>,
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
            regex: Some(regex),
        }))
    }

    /// A startup option stores a literal message prefix or an exact module name.
    fn literal(original: &str) -> Option<Self> {
        (!original.is_empty()).then(|| Self {
            original: original.into(),
            regex: None,
        })
    }

    fn matches(&self, text: &str, message_prefix: bool) -> bool {
        if let Some(regex) = &self.regex {
            return regex.is_match(Input::new(text).anchored(Anchored::Yes));
        }
        if !message_prefix {
            return self.original == text;
        }
        let mut chars = text.chars();
        self.original.chars().all(|expected| {
            chars.next().is_some_and(|actual| {
                if expected.is_ascii() && actual.is_ascii() {
                    expected.eq_ignore_ascii_case(&actual)
                } else {
                    expected == actual || literal_fold(expected) == literal_fold(actual)
                }
            })
        })
    }
}

/// Unicode IGNORECASE also groups dotted and dotless I with ASCII i.
fn literal_fold(ch: char) -> u32 {
    match ch {
        'İ' | 'ı' => u32::from('i'),
        _ => unicode_case_mapping::case_folded(ch).map_or(u32::from(ch), |n| n.get()),
    }
}

#[derive(Clone)]
struct Rule {
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
            && self.action == other.action
            && self.lineno == other.lineno
            && source(&self.message) == source(&other.message)
            && source(&self.module) == source(&other.module)
    }

    fn matches(&self, text: &str, category: u32, module: &str, lineno: isize) -> Option<bool> {
        fn matches(pattern: &Option<Pattern>, text: &str, message_prefix: bool) -> bool {
            pattern
                .as_ref()
                .is_none_or(|p| p.matches(text, message_prefix))
        }
        // CPython converts every visited rule's line to Py_ssize_t during
        // emission, even when that rule's category or regex would not match.
        let line = self.lineno.to_isize()?;
        Some(
            (self.category == 0 || self.category == category)
                && (line == 0 || line == lineno)
                && matches(&self.message, text, true)
                && matches(&self.module, module, false),
        )
    }
}

#[derive(Eq, Hash, PartialEq)]
enum Scope {
    Line(String, isize),
    Module(String),
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
        action: Action::Default,
        category: 4,
        message: None,
        module: Pattern::literal("__main__"),
        lineno: BigInt::ZERO,
    }];
    rules.extend([4, 5, 8, 1].map(|category| Rule {
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
                action,
                category,
                lineno: lineno.into_owned(),
                message: Pattern::compile(message, message_re, true)?,
                module: Pattern::compile(module, module_re, false)?,
            };
            insert_rule(rule, append != 0);
            Ok(())
        })(),
        (),
    );
}

fn insert_rule(rule: Rule, append: bool) {
    let mut filters = FILTERS.lock().unwrap_or_else(|poison| poison.into_inner());
    if !append {
        filters.rules.retain(|old| !old.same(&rule));
        filters.rules.insert(0, rule);
    } else if !filters.rules.iter().any(|old| old.same(&rule)) {
        filters.rules.push(rule);
    }
    filters.displayed.clear();
}

fn category_number(name: &str) -> Option<u32> {
    imre_ast::exceptions::WARNING_CATEGORIES
        .iter()
        .position(|kind| kind.name() == name)
        .map(|n| n as u32)
}

/// Install a -W option. Messages and module names here are literal.
#[cold]
pub(crate) fn startup_filter(option: &str) -> Result<(), String> {
    let repr = |s: &str| exceptions::string_repr(s).as_str().to_owned();
    let mut fields = option.split(':').map(str::trim);
    let [action, message, category, module, line] =
        std::array::from_fn(|_| fields.next().unwrap_or(""));
    if fields.next().is_some() {
        return Err(format!("too many fields (max 5): {}", repr(option)));
    }
    let action = [
        "default", "always", "all", "ignore", "module", "once", "error",
    ]
    .into_iter()
    .find(|name| name.starts_with(action))
    .ok_or_else(|| format!("invalid action: {}", repr(action)))?;
    let category = if category.is_empty() {
        0
    } else {
        category_number(category.strip_prefix("builtins.").unwrap_or(category))
            .ok_or_else(|| format!("unknown warning category: {}", repr(category)))?
    };
    let lineno = if line.is_empty() {
        BigInt::ZERO
    } else {
        line.parse::<BigInt>()
            .ok()
            .filter(|n| !n.is_negative())
            .ok_or_else(|| format!("invalid lineno {}", repr(line)))?
    };
    let message = Pattern::literal(message);
    let module = Pattern::literal(module);
    insert_rule(
        Rule {
            action: Action::parse(action).unwrap(),
            category,
            message,
            module,
            lineno,
        },
        false,
    );
    Ok(())
}

thread_local! {
    static SAVED_RULES: std::cell::RefCell<Vec<Vec<Rule>>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Enter a non-recording catch_warnings block with a copy of the current filters.
#[unsafe(no_mangle)]
pub extern "C" fn imre_warning_scope_enter() {
    let mut filters = FILTERS.lock().unwrap_or_else(|poison| poison.into_inner());
    SAVED_RULES.with_borrow_mut(|stack| stack.push(filters.rules.clone()));
    filters.displayed.clear();
}

/// The compiler emits this in a finally block for every successful entry.
#[unsafe(no_mangle)]
pub extern "C" fn imre_warning_scope_exit() {
    let mut filters = FILTERS.lock().unwrap_or_else(|poison| poison.into_inner());
    filters.rules =
        SAVED_RULES.with_borrow_mut(|stack| stack.pop().expect("entered warning scope"));
    filters.displayed.clear();
}

/// Clear filters, including the initial ResourceWarning ignore rule.
#[unsafe(no_mangle)]
pub extern "C" fn imre_warning_reset() {
    let mut filters = FILTERS.lock().unwrap_or_else(|poison| poison.into_inner());
    filters.rules.clear();
    filters.displayed.clear();
}

/// Decide whether to display or raise a warning. Every visited rule converts
/// its line number to Py_ssize_t, even when the category would not match.
fn filtered(
    category: u32,
    module: &str,
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
        Action::Default => Some(Scope::Line(module.to_owned(), line)),
        Action::Module => Some(Scope::Module(module.to_owned())),
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
    runtime_warning(
        "subprocess",
        "subprocess.py",
        862,
        "pass_fds overriding close_fds.",
    )
}

/// Emit a checked RuntimeWarning from an internal standard-library frame.
pub(crate) fn runtime_warning(
    module: &str,
    filename: &str,
    line: isize,
    message: &str,
) -> crate::Result<()> {
    internal_warning(Kind::RuntimeWarning, module, filename, line, message)
}

/// A checked non-finalizer warning issued from an internal standard-library frame.
pub(super) fn deprecation(
    module: &'static str,
    filename: &str,
    line: isize,
    message: &str,
) -> crate::Result<()> {
    internal_warning(Kind::DeprecationWarning, module, filename, line, message)
}

/// Apply the module and line filters at the warning site before displaying it.
pub(super) fn internal_warning(
    kind: Kind,
    module: &str,
    filename: &str,
    line: isize,
    message: &str,
) -> crate::Result<()> {
    let category = category_number(kind.name()).expect("expected a warning category");
    match filtered(category, module, line, || message.into())
        .map_err(|()| Error::new(Kind::OverflowError, LINE_OVERFLOW))?
    {
        Some((Action::Error, message)) => Err(Error::new(kind, message)),
        Some((_, message)) => {
            let text = format!("{filename}:{line}: {}: {message}", kind.name());
            let _ = crate::sys::write_stderr(&text, true);
            Ok(())
        }
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::Pattern;

    #[test]
    fn startup_message_and_module_patterns_follow_python_literal_matching() {
        let message = Pattern::literal("i.σ").unwrap();
        assert!(message.matches("İ.ς suffix", true));
        assert!(message.matches("ı.Σ", true));
        assert!(!message.matches("iXσ", true));
        assert!(!message.matches("i.", true));

        let module = Pattern::literal("pkg.[x]").unwrap();
        assert!(module.matches("pkg.[x]", false));
        assert!(!module.matches("pkg.[x].child", false));
        assert!(!module.matches("pkg.x", false));
        assert!(!module.matches("PKG.[x]", false));
    }
}
