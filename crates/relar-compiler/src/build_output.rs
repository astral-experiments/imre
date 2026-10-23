use std::env;
use std::ffi::OsStr;
use std::fmt::{self, Display};
use std::io::{self, IsTerminal, Write};
use std::path::Path;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Default, Eq, PartialEq)]
pub(crate) enum Verbosity {
    Quiet,
    #[default]
    Normal,
    Verbose,
}

/// Human-readable build events share stderr with the compiler's diagnostics.
pub(crate) struct BuildOutput {
    verbosity: Verbosity,
    color: bool,
    started: Instant,
    compiled: usize,
    reused: usize,
    first_compiled: String,
}

impl BuildOutput {
    pub(crate) fn new(verbosity: Verbosity) -> Self {
        Self {
            verbosity,
            color: io::stderr().is_terminal()
                && env::var_os("NO_COLOR").is_none()
                && env::var_os("TERM").as_deref() != Some(OsStr::new("dumb")),
            started: Instant::now(),
            compiled: 0,
            reused: 0,
            first_compiled: String::new(),
        }
    }

    pub(crate) fn compiling(&self, source: &Path, optimization: &OsStr) {
        self.line(
            "Compiling",
            format_args!(
                "{} (-O{})",
                Label(&source.to_string_lossy()),
                optimization.display()
            ),
        );
    }

    pub(crate) fn unit(&mut self, name: &str, hit: bool) {
        if self.verbosity == Verbosity::Quiet {
            return;
        }
        if hit {
            self.reused += 1;
        } else {
            self.compiled += 1;
            if self.compiled == 1 {
                self.first_compiled.push_str(name);
            }
        }
        if self.verbosity == Verbosity::Verbose {
            self.line(if hit { "Reusing" } else { "Compiling" }, Label(name));
        }
    }

    /// All unit compilations have succeeded by the time the native link starts.
    pub(crate) fn linking(&self) {
        if self.verbosity != Verbosity::Quiet {
            self.line("Linking", self.object_summary());
        }
    }

    pub(crate) fn finished(&self, target: &Path) {
        self.timed("Finished", target);
    }

    pub(crate) fn cached(&self, target: &Path) {
        self.timed("Cached", target);
    }

    pub(crate) fn running(&self, source: &Path) {
        self.line("Running", Label(&source.to_string_lossy()));
    }

    fn timed(&self, action: &str, target: &Path) {
        if self.verbosity != Verbosity::Quiet {
            self.line(
                action,
                format_args!(
                    "{} in {}",
                    Label(&target.to_string_lossy()),
                    elapsed(self.started.elapsed())
                ),
            );
        }
    }

    fn object_summary(&self) -> String {
        let compiled = match self.compiled {
            0 => String::new(),
            1 => format!("1 object compiled ({})", Label(&self.first_compiled)),
            n => format!("{n} objects compiled"),
        };
        match (self.compiled, self.reused) {
            (_, 0) => compiled,
            (0, 1) => "1 object reused".to_owned(),
            (0, n) => format!("{n} objects reused"),
            (_, n) => format!("{compiled}, {n} reused"),
        }
    }

    fn line(&self, action: &str, detail: impl Display) {
        if self.verbosity != Verbosity::Quiet {
            // A closed stderr must not turn a successful build into a failure.
            let _ = write_line(&mut io::stderr().lock(), self.color, action, detail);
        }
    }
}

fn write_line(
    out: &mut impl Write,
    color: bool,
    action: &str,
    detail: impl Display,
) -> io::Result<()> {
    if color {
        let style = match action {
            "Cached" | "Reusing" | "Running" => "\x1b[1;36m",
            _ => "\x1b[1;32m",
        };
        writeln!(out, "{style}{action:>10}\x1b[0m {detail}")
    } else {
        writeln!(out, "{action:>10} {detail}")
    }
}

fn elapsed(duration: Duration) -> String {
    match duration.as_millis() {
        0 => "<1 ms".to_owned(),
        ms @ 1..1000 => format!("{ms} ms"),
        _ => format!("{:.2} s", duration.as_secs_f64()),
    }
}

// Paths can contain tabs, newlines and terminal controls. Keep each event on one line.
struct Label<'a>(&'a str);

impl Display for Label<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if !self.0.chars().any(char::is_control) {
            return f.write_str(self.0);
        }
        for c in self.0.chars() {
            if c.is_control() {
                write!(f, "{}", c.escape_default())?;
            } else {
                fmt::Write::write_char(f, c)?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_changed_module_is_identified_in_the_default_summary() {
        let mut status = BuildOutput::new(Verbosity::Normal);
        status.unit("report.model", true);
        status.unit("report.render", false);
        status.unit("report.parse", true);
        assert_eq!(
            status.object_summary(),
            "1 object compiled (report.render), 2 reused"
        );
        status.unit("report.parse", false);
        assert_eq!(status.object_summary(), "2 objects compiled, 2 reused");
    }

    #[test]
    fn plain_logs_and_terminal_labels_escape_user_controls() {
        let mut plain = Vec::new();
        write_line(&mut plain, false, "Running", Label("my dir/é\t\n\x1b.py")).unwrap();
        assert_eq!(
            String::from_utf8(plain).unwrap(),
            "   Running my dir/é\\t\\n\\u{1b}.py\n"
        );
        let mut terminal = Vec::new();
        write_line(&mut terminal, true, "Cached", "reporter").unwrap();
        assert_eq!(terminal, b"\x1b[1;36m    Cached\x1b[0m reporter\n");
    }

    #[test]
    fn short_hits_and_longer_builds_show_readable_times() {
        assert_eq!(elapsed(Duration::from_micros(950)), "<1 ms");
        assert_eq!(elapsed(Duration::from_millis(17)), "17 ms");
        assert_eq!(elapsed(Duration::from_millis(1234)), "1.23 s");
    }
}
