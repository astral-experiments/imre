//! The noninteractive input protocol shared by native and boxed values.

#[derive(Clone, Copy)]
pub(crate) enum StreamName {
    Stdin,
    Stdout,
    Stderr,
}

impl StreamName {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Stdin => "stdin",
            Self::Stdout => "stdout",
            Self::Stderr => "stderr",
        }
    }
}

pub(crate) trait Backend {
    type Stream;
    type Prompt;
    type Line;
    type Error;

    /// Capture a stream whose identity survives callbacks until this input call finishes.
    fn stream(&mut self, name: StreamName) -> Result<Self::Stream, Self::Error>;
    /// Native typed execution has no interpreter-local Python audit hooks.
    fn audit(&mut self, _prompt: Option<&Self::Prompt>) -> Result<(), Self::Error> {
        Ok(())
    }
    /// Only Python streams can supply a user-defined fileno or __index__.
    fn check_descriptors(
        &mut self,
        _stdin: &Self::Stream,
        _stdout: &Self::Stream,
    ) -> Result<(), Self::Error> {
        Ok(())
    }
    fn flush_ignoring_errors(&mut self, stream: &Self::Stream);
    fn write_prompt(
        &mut self,
        stream: &Self::Stream,
        prompt: Self::Prompt,
    ) -> Result<(), Self::Error>;
    fn read_line(&mut self, stream: &Self::Stream) -> Result<Self::Line, Self::Error>;
    fn last_unit(&self, line: &Self::Line) -> Result<Option<u32>, Self::Error>;
    fn trim_linefeed(&mut self, line: Self::Line) -> Result<Self::Line, Self::Error>;
    fn eof(&self) -> Self::Error;
}

pub(crate) struct Streams<S> {
    pub(crate) stdin: S,
    pub(crate) stdout: S,
    // CPython retains stderr even after the first flush, until the call returns.
    _stderr: S,
}

/// Capture the three bindings before running any Python-level conversion.
pub(crate) fn prepare<B: Backend>(
    backend: &mut B,
    prompt: Option<&B::Prompt>,
) -> Result<Streams<B::Stream>, B::Error> {
    let stdin = backend.stream(StreamName::Stdin)?;
    let stdout = backend.stream(StreamName::Stdout)?;
    let stderr = backend.stream(StreamName::Stderr)?;
    backend.audit(prompt)?;
    backend.flush_ignoring_errors(&stderr);
    backend.check_descriptors(&stdin, &stdout)?;
    Ok(Streams {
        stdin,
        stdout,
        _stderr: stderr,
    })
}

pub(crate) fn finish<B: Backend>(
    backend: &mut B,
    stdin: &B::Stream,
    stdout: &B::Stream,
    prompt: Option<B::Prompt>,
) -> Result<B::Line, B::Error> {
    if let Some(prompt) = prompt {
        backend.write_prompt(stdout, prompt)?;
    }
    backend.flush_ignoring_errors(stdout);
    let line = backend.read_line(stdin)?;
    match backend.last_unit(&line)? {
        None => Err(backend.eof()),
        Some(10) => backend.trim_linefeed(line),
        Some(_) => Ok(line),
    }
}

pub(crate) fn read<B: Backend>(
    backend: &mut B,
    prompt: Option<B::Prompt>,
) -> Result<B::Line, B::Error> {
    let streams = prepare(backend, prompt.as_ref())?;
    finish(backend, &streams.stdin, &streams.stdout, prompt)
}
