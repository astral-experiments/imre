//! Python source function binding shared by checked calls and object frames.
use smallvec::SmallVec;

pub(crate) struct Shape<'a, N> {
    pub positional: &'a [N],
    pub positional_only: usize,
    pub keyword_only: &'a [N],
    pub varargs: bool,
    pub kwargs: bool,
}

pub(crate) struct Binding {
    /// Each evaluated keyword binds this numbered formal, or goes into **kwargs.
    pub keywords: SmallVec<[Option<usize>; 8]>,
    /// Omitted formals whose defaults belong to the function.
    pub defaults: SmallVec<[usize; 8]>,
}

pub(crate) enum Error {
    Memory,
    Type(String),
}

impl<N: AsRef<str>> Shape<'_, N> {
    pub(crate) fn parameter(&self, slot: usize) -> &str {
        if slot < self.positional.len() {
            self.positional[slot].as_ref()
        } else {
            self.keyword_only[slot - self.positional.len()].as_ref()
        }
    }

    pub(crate) fn bind<'k>(
        &self,
        name: &str,
        positional: usize,
        mut keywords: impl ExactSizeIterator<Item = &'k str> + Clone,
        positional_defaults: usize,
        has_keyword_default: impl Fn(&str) -> bool,
    ) -> Result<Binding, Error> {
        let npos = self.positional.len();
        let count = npos + self.keyword_only.len();
        let mut supplied = SmallVec::<[bool; 8]>::new();
        supplied.try_reserve(count).map_err(|_| Error::Memory)?;
        supplied.resize(count, false);
        supplied[..positional.min(npos)].fill(true);
        let mut targets = SmallVec::new();
        targets
            .try_reserve(keywords.len())
            .map_err(|_| Error::Memory)?;
        while let Some(key) = keywords.next() {
            let slot = (self.positional_only..count).find(|&i| self.parameter(i) == key);
            if let Some(slot) = slot {
                if std::mem::replace(&mut supplied[slot], true) {
                    return Err(Error::Type(format!(
                        "{name}() got multiple values for argument '{key}'"
                    )));
                }
            } else if !self.kwargs {
                let conflicts: Vec<_> = self.positional[..self.positional_only]
                    .iter()
                    .map(AsRef::as_ref)
                    .filter(|p| *p == key || keywords.clone().any(|remaining| remaining == *p))
                    .collect();
                if !conflicts.is_empty() {
                    return Err(Error::Type(format!(
                        "{name}() got some positional-only arguments passed as keyword arguments: '{}'",
                        conflicts.join(", ")
                    )));
                }
                let candidates: Vec<_> = (self.positional_only..count)
                    .map(|i| self.parameter(i))
                    .collect();
                return Err(Error::Type(unexpected_keyword_message(
                    name,
                    key,
                    &candidates,
                )));
            }
            targets.push(slot);
        }
        if positional > npos && !self.varargs {
            let named = supplied[npos..].iter().filter(|v| **v).count();
            return Err(Error::Type(excess_positional_message(
                name,
                npos,
                positional_defaults,
                positional,
                named,
            )));
        }
        let mut defaults = SmallVec::new();
        let mut missing = SmallVec::<[&str; 4]>::new();
        for (kind, slots) in [("positional", 0..npos), ("keyword-only", npos..count)] {
            for slot in slots.filter(|&slot| !supplied[slot]) {
                let formal = self.parameter(slot);
                let has_default = if slot < npos {
                    slot >= npos.saturating_sub(positional_defaults)
                } else {
                    has_keyword_default(formal)
                };
                if has_default {
                    defaults.try_reserve(1).map_err(|_| Error::Memory)?;
                    defaults.push(slot);
                } else {
                    missing.try_reserve(1).map_err(|_| Error::Memory)?;
                    missing.push(formal);
                }
            }
            if !missing.is_empty() {
                return Err(Error::Type(missing_arguments_message(name, kind, &missing)));
            }
        }
        Ok(Binding {
            keywords: targets,
            defaults,
        })
    }
}

/// Format a Python call's unexpected keyword and any spelling suggestion.
pub(crate) fn unexpected_keyword_message(name: &str, key: &str, params: &[&str]) -> String {
    let mut message = format!("{name}() got an unexpected keyword argument '{key}'");
    if let Some(hint) = crate::objects::exceptions::keyword_suggestion(key, params) {
        message.push_str(&format!(". Did you mean '{hint}'?"));
    }
    message
}

pub(crate) fn excess_positional_message(
    name: &str,
    allowed: usize,
    defaults: usize,
    given: usize,
    keywords: usize,
) -> String {
    let limit = if defaults == 0 {
        allowed.to_string()
    } else {
        format!("from {} to {allowed}", allowed as isize - defaults as isize)
    };
    let unit = if allowed == 1 && defaults == 0 {
        "argument"
    } else {
        "arguments"
    };
    let (count, verb) = if keywords == 0 {
        (given.to_string(), if given == 1 { "was" } else { "were" })
    } else {
        let positional_unit = if given == 1 { "argument" } else { "arguments" };
        let keyword_unit = if keywords == 1 {
            "argument"
        } else {
            "arguments"
        };
        (
            format!(
                "{given} positional {positional_unit} (and {keywords} keyword-only {keyword_unit})"
            ),
            "were",
        )
    };
    format!("{name}() takes {limit} positional {unit} but {count} {verb} given")
}

pub(crate) fn missing_arguments_message(name: &str, kind: &str, missing: &[&str]) -> String {
    let count = missing.len();
    let unit = if count == 1 { "argument" } else { "arguments" };
    let names = match missing {
        [one] => format!("'{one}'"),
        [one, two] => format!("'{one}' and '{two}'"),
        [rest @ .., last] => format!("'{}', and '{last}'", rest.join("', '")),
        [] => unreachable!(),
    };
    format!("{name}() missing {count} required {kind} {unit}: {names}")
}
