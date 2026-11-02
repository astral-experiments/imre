//! Boxed signal handlers are heap roots and are called only by the main interpreter thread.
use super::{Context, Error, Id, NONE, Result, Value, stdlib};
use crate::signals::{self, Disposition};

impl Context {
    fn signal_handler(&mut self, number: i32, disposition: Option<Disposition>) -> Id {
        match disposition {
            Some(Disposition::Default) => self.int(libc::SIG_DFL as i64),
            Some(Disposition::Ignore) => self.int(libc::SIG_IGN as i64),
            Some(Disposition::Python) => self.python_handler(number),
            None => NONE,
        }
    }

    fn python_handler(&mut self, number: i32) -> Id {
        if let Some(&handler) = self.signal_handlers.get(&number) {
            handler
        } else if number == libc::SIGINT {
            self.builtin("_signal.default_int_handler")
        } else {
            NONE
        }
    }

    pub(super) fn dispatch_signals(&mut self) -> Result<()> {
        if !signals::begin_dispatch() { return Ok(()); }
        for number in 1..signals::nsig() {
            if signals::take(number) && matches!(signals::disposition(number), Ok(Some(Disposition::Python))) {
                let handler = self.python_handler(number);
                if handler != NONE {
                    let result = self.call_signal_handler(number, handler);
                    if result.is_err() {
                        // A raising handler leaves later pending signals for the next checkpoint.
                        signals::resume_dispatch();
                    }
                    result?;
                }
            }
        }
        Ok(())
    }

    fn call_signal_handler(&mut self, number: i32, handler: Id) -> Result<()> {
        let number = self.int(i64::from(number));
        let frame = self.active_frame(0)?.map_or(NONE, |(_, frame)| frame);
        self.call(handler, vec![number, frame], vec![])?;
        Ok(())
    }

    pub(super) fn signal_call(&mut self, name: &str, args: &[Id], kwargs: &[(String, Id)]) -> Result<Id> {
        if !kwargs.is_empty() {
            return Err(Error::new("TypeError", format!("{name}() takes no keyword arguments")));
        }
        let expected = if name == "_signal.signal" { 2 } else { 1 };
        if args.len() != expected {
            return Err(Error::new("TypeError", format!("{name} expected {expected} arguments, got {}", args.len())));
        }
        let number = stdlib::index_c_int(self, args[0])?;
        if expected == 2 && !signals::is_main() {
            return Err(Error::new("ValueError", "signal only works in main thread of the main interpreter"));
        }
        if !(1..signals::nsig()).contains(&number) {
            return Err(Error::new("ValueError", "signal number out of range"));
        }
        if expected == 1 {
            let existing = match signals::disposition(number) {
                Ok(existing) => existing,
                // Reserved numbers inside NSIG have no Python disposition.
                Err(e) if e.raw_os_error() == Some(libc::EINVAL) => None,
                Err(e) => return Err(stdlib::os_error(e)),
            };
            return Ok(self.signal_handler(number, existing));
        }
        let handler = args[1];
        let action = if self.callable(handler) {
            Disposition::Python
        } else {
            match self.heap.get(handler) {
                Value::Int(i) if i.to_i64() == Some(libc::SIG_DFL as i64) => Disposition::Default,
                Value::Int(i) if i.to_i64() == Some(libc::SIG_IGN as i64) => Disposition::Ignore,
                _ => return Err(Error::new("TypeError", "signal handler must be signal.SIG_IGN, signal.SIG_DFL, or a callable object")),
            }
        };
        self.dispatch_signals()?;
        self.signal_handlers.try_reserve(1).map_err(|_| Error::new("MemoryError", ""))?;
        let old = signals::install(number, action).map_err(stdlib::os_error)?;
        let old = self.signal_handler(number, old);
        if action == Disposition::Python {
            self.signal_handlers.insert(number, handler);
        } else {
            self.signal_handlers.remove(&number);
        }
        Ok(old)
    }
}
