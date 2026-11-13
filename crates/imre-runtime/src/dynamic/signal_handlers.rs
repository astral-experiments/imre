//! Boxed signal handlers are heap roots and are called only by the main interpreter thread.
use super::calls::RegisteredCall;
use super::{Context, Error, Id, NONE, Result, Value, stdlib};
use crate::signals::{self, Disposition};

impl Context {
    pub(super) fn signal_sentinel(&mut self, key: &str, value: libc::sighandler_t) -> Id {
        if let Some(&id) = self.builtins.get(key) {
            return id;
        }
        let id = self.int(value as i64);
        self.builtins.insert(key.to_owned(), id);
        id
    }

    /// A native library can replace a disposition without changing Python's handler table.
    pub(super) fn initialize_signal_handlers(&mut self) {
        if !self.signal_handlers.is_empty() {
            return;
        }
        let default = self.signal_sentinel("_signal.SIG_DFL", libc::SIG_DFL);
        let ignore = self.signal_sentinel("_signal.SIG_IGN", libc::SIG_IGN);
        let interrupt = self.builtin("_signal.default_int_handler");
        self.signal_handlers = vec![NONE; signals::nsig() as usize];
        let main = signals::is_main();
        for (number, slot) in self.signal_handlers.iter_mut().enumerate().skip(1) {
            let disposition = signals::disposition(number as i32).ok().flatten();
            if main {
                signals::record_handler(number, disposition);
            }
            *slot = match disposition {
                Some(Disposition::Default) => default,
                Some(Disposition::Ignore) => ignore,
                Some(Disposition::Python) if number == libc::SIGINT as usize => interrupt,
                _ => NONE,
            };
        }
    }

    pub(super) fn dispatch_signals(&mut self) -> Result<()> {
        // SAFETY: synchronous delivery returns to the thread that owns this context.
        unsafe { Self::dispatch_pending_signals(self) }
    }

    /// # Safety
    /// The caller owns this context and its active frame throughout the checkpoint.
    pub(super) unsafe fn dispatch_pending_calls(context: *mut Self) -> Result<()> {
        if signals::begin_dispatch() {
            // SAFETY: a registered handler returns to the interrupted caller on this thread.
            unsafe { Self::dispatch_pending_signals(context)? };
            // SAFETY: delivery has completed, including any nested callbacks.
            unsafe { &mut *context }.report_wakeup_errors();
        }
        Ok(())
    }

    fn report_wakeup_errors(&mut self) {
        if !self.reporting_wakeup_errors && signals::has_wakeup_errors() {
            self.reporting_wakeup_errors = true;
            for error in signals::wakeup_errors() {
                super::unraisable::report(
                    self,
                    stdlib::os_error(error),
                    true,
                    None,
                    "Exception ignored while trying to write to the signal wakeup fd",
                );
            }
            self.reporting_wakeup_errors = false;
            if signals::has_wakeup_errors() {
                signals::resume_dispatch();
            }
        }
    }

    /// # Safety
    /// The caller owns the context on this thread with the interrupted frame still active.
    pub(super) unsafe fn dispatch_pending_signals(context: *mut Self) -> Result<()> {
        if !signals::begin_signal_dispatch() {
            return Ok(());
        }
        // A queued wakeup-fd report belongs to the next Python checkpoint, even inside a handler.
        if signals::has_wakeup_errors() {
            signals::resume_dispatch();
        }
        // SAFETY: the thread owns its handler table and initialization finishes before delivery.
        let ctx = unsafe { &mut *context };
        ctx.initialize_signal_handlers();
        for number in (1..signals::nsig()).filter(|&number| signals::take(number)) {
            // SAFETY: the preceding delivery has returned to this thread.
            let ctx = unsafe { &mut *context };
            let handler = ctx.signal_handlers[number as usize];
            if ctx.callable(handler) {
                let call = ctx.prepare_signal_handler(number, handler);
                // SAFETY: preparation has returned and the interrupted caller stays on the heap.
                if let Err(error) = call.and_then(|call| unsafe { call.run(context) }) {
                    // A raising handler leaves later pending signals for the next checkpoint.
                    signals::resume_signals();
                    return Err(error);
                }
            } else {
                super::unraisable::report(
                    ctx,
                    Error::new(
                        "OSError",
                        format!("Signal {number} ignored due to race condition"),
                    ),
                    true,
                    None,
                    "Exception ignored while calling signal handler",
                );
            }
        }
        Ok(())
    }

    fn prepare_signal_handler(&mut self, number: i32, handler: Id) -> Result<RegisteredCall> {
        let number = self.int(i64::from(number));
        let frame = self.active_frame(0)?.map_or(NONE, |(_, frame)| frame);
        self.prepare_call(handler, vec![number, frame], vec![])
    }

    pub(super) fn signal_call(
        &mut self,
        name: &str,
        args: &[Id],
        kwargs: &[(String, Id)],
    ) -> Result<Id> {
        if !kwargs.is_empty() {
            return Err(Error::new(
                "TypeError",
                format!("{name}() takes no keyword arguments"),
            ));
        }
        let registering = name == "_signal.signal";
        let expected = if registering { 2 } else { 1 };
        if args.len() != expected {
            return Err(Error::new(
                "TypeError",
                format!("{name} expected {expected} arguments, got {}", args.len()),
            ));
        }
        let number = stdlib::index_c_int(self, args[0])?;
        if registering && !signals::is_main() {
            return Err(Error::new(
                "ValueError",
                "signal only works in main thread of the main interpreter",
            ));
        }
        if !(1..signals::nsig()).contains(&number) {
            return Err(Error::new("ValueError", "signal number out of range"));
        }
        self.initialize_signal_handlers();
        if !registering {
            return Ok(self.signal_handlers[number as usize]);
        }
        let handler = args[1];
        let action = if self.callable(handler) {
            Disposition::Python
        } else {
            match self.heap.get(handler) {
                Value::Int(i) if i.to_i64() == Some(libc::SIG_DFL as i64) => Disposition::Default,
                Value::Int(i) if i.to_i64() == Some(libc::SIG_IGN as i64) => Disposition::Ignore,
                _ => {
                    return Err(Error::new(
                        "TypeError",
                        "signal handler must be signal.SIG_IGN, signal.SIG_DFL, or a callable object",
                    ));
                }
            }
        };
        self.dispatch_signals()?;
        signals::install(number, action).map_err(stdlib::os_error)?;
        let value = match action {
            Disposition::Default => self.signal_sentinel("_signal.SIG_DFL", libc::SIG_DFL),
            Disposition::Ignore => self.signal_sentinel("_signal.SIG_IGN", libc::SIG_IGN),
            Disposition::Python => handler,
        };
        let old = std::mem::replace(&mut self.signal_handlers[number as usize], value);
        Ok(old)
    }
}
