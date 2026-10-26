//! A timeout suspends pipe multiplexing, not the child. Retain the input cursor,
//! raw output chunks, and observed EOF across calls, including in text mode.

use super::*;

#[derive(Default)]
pub(super) struct Communication {
    started: bool,
    input: Vec<u8>,
    offset: usize,
    output: [Vec<u8>; 2],
    observed: [bool; 2],
}

enum Exchange {
    Done([Vec<u8>; 2]),
    Timeout([Option<Vec<u8>>; 2], Option<f64>),
}

/// Decode the rooted input using the stream mode chosen at the call site.
unsafe fn input_bytes<'a>(input: *const (), text: bool) -> &'a [u8] {
    if input.is_null() {
        &[]
    } else if text {
        // SAFETY: the caller roots an exact native string for this borrow.
        unsafe { (&*input.cast::<Str>()).as_bytes() }
    } else {
        // SAFETY: the caller roots an exact byte buffer for this borrow.
        unsafe { (&*input.cast::<Bytes>()).as_slice() }
    }
}

/// Capture stdout, keep checked failures' streams, and reap the direct child
/// before exposing a timeout. Descendants may still hold the far pipe ends.
///
/// # Safety
/// The launch configuration, deadline word and optional input have the layouts
/// selected by the compiler and stay rooted until the call returns.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_check_output_config(
    config: *const LaunchConfig,
    deadline_kind: u32,
    deadline: usize,
    input: usize,
    validation: u32,
    errors: u32,
) -> *const () {
    // SAFETY: the legacy API supplies the same rooted parameters; a null
    // descriptor asks for the stdout object instead of a CompletedProcess.
    unsafe {
        imre_run_config(
            config,
            deadline_kind,
            deadline,
            input,
            validation,
            errors,
            true,
            std::ptr::null(),
        )
    }
}

/// Run a native command and return either an exact CompletedProcess or checked stdout.
///
/// # Safety
/// Launch fields (including a trailer when a stream is borrowed), the deadline
/// and input must remain rooted and have the stated types. A present result
/// descriptor has the original command's exact layout.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_run_config(
    config: *const LaunchConfig,
    deadline_kind: u32,
    deadline: usize,
    input: usize,
    validation: u32,
    errors: u32,
    check: bool,
    descriptor: *const RecordDescriptor,
) -> *const () {
    // SAFETY: the caller roots this immutable configuration throughout launch.
    let (mut config, extras) = unsafe { (*config, LaunchConfig::extras(config)) };
    let mut validation = validation;
    if !descriptor.is_null() && input != 0 {
        // run(input=None) inherits stdin; a present value, even empty, demands a pipe.
        if (config.streams >> 12) & 15 != 0 {
            validation = 3;
        } else {
            config.streams |= 1 << 12;
        }
    }
    let invalid = match validation {
        0 => None,
        1 => Some("stdout argument not allowed, it will be overridden."),
        2 => Some("check argument not allowed, it will be overridden."),
        3 => Some("stdin and input arguments may not both be used."),
        4 => Some("stdout and stderr arguments may not be used with capture_output."),
        5 => return Error::new(Kind::SubprocessError, "Cannot disambiguate when both text and universal_newlines are supplied but different. Pass one or the other.").returning(std::ptr::null()),
        _ => fail("SystemError", "unknown check_output validation"),
    };
    if let Some(message) = invalid {
        return Error::new(Kind::ValueError, message).returning(std::ptr::null());
    }
    // SAFETY: the caller roots the configuration and its typed launch fields;
    // the timeout representation and input type were selected at the same call site.
    let (timeout, input, process) = unsafe {
        (
            Deadline::from_word(deadline_kind, deadline),
            input_bytes(input as *const (), config.streams & (1 << 16) != 0),
            spawn(&config, extras),
        )
    };
    let kind = if config.streams & (1 << 16) != 0 {
        3
    } else {
        8
    };
    let stream = |bytes: Vec<u8>, kind| {
        let bytes = if kind == 3 && errors != 0 {
            match errors {
                1 => bytes
                    .utf8_chunks()
                    .flat_map(|chunk| chunk.valid().bytes())
                    .collect(),
                2 => String::from_utf8_lossy(&bytes).into_owned().into_bytes(),
                _ => fail("SystemError", "unknown process decoding policy"),
            }
        } else {
            bytes
        };
        completed_stream(bytes, WordKind::new(kind))
    };
    let result = process.and_then(|mut p| {
        // No caller can have consumed the freshly opened streams. Decode them
        // once after communication so timeouts keep their original raw bytes.
        let exchange = p.communicate(input, timeout.as_ref(), false);
        let killed = if matches!(exchange, Ok(Exchange::Done(_))) {
            Ok(())
        } else {
            p.signal(|| Ok(libc::SIGKILL))
        };
        p.close_wait()?;
        killed?;
        match exchange? {
            Exchange::Done([stdout, stderr]) => {
                let stdout_kind = if config.streams & 15 == 1 { kind } else { 0 };
                let stdout = stream(stdout, stdout_kind)?;
                let stderr_kind = if (config.streams >> 4) & 15 == 1 {
                    kind
                } else {
                    0
                };
                let stderr = stream(stderr, stderr_kind)?;
                let code = p
                    .control
                    .status()
                    .expect("communicate reaped the child")
                    .kernel();
                if check && code != 0 {
                    raise_process(ProcessError {
                        positional_streams: false,
                        cmd: p.args.clone(),
                        returncode: code,
                        stdout,
                        stderr,
                    });
                    Ok(std::ptr::null())
                } else if descriptor.is_null() {
                    Ok(stdout.into_word() as *const ())
                } else {
                    // SAFETY: the compiler's descriptor uses this same command and capture layout.
                    Ok(unsafe {
                        completed_record(descriptor, p.args.clone(), code, stdout, stderr)
                    }
                    .cast())
                }
            }
            Exchange::Timeout([stdout, stderr], wait_budget) => {
                raise_timeout(TimeoutError {
                    cmd: p.args.clone(),
                    timeout: timeout.expect("timed communication"),
                    wait_budget,
                    stdout: stdout.map(crate::BytesOwner::new),
                    stderr: stderr.map(crate::BytesOwner::new),
                });
                Ok(std::ptr::null())
            }
        }
    });
    finish(result, std::ptr::null())
}

fn file<T>(owner: &Object, f: impl FnOnce(&mut FileState) -> T) -> T {
    // SAFETY: the rooted pipe is a sealed file, and no callback reenters a borrow.
    unsafe { owner.0.with_mut(|s| f(s.file_mut())) }
}

fn copy(bytes: &[u8]) -> Result<Vec<u8>> {
    let mut result = Vec::new();
    append_output(&mut result, bytes).map_err(capture_error)?;
    Ok(result)
}

impl Communication {
    fn timeout(&self) -> Result<Exchange> {
        let snapshot = |i: usize| self.observed[i].then(|| copy(&self.output[i])).transpose();
        Ok(Exchange::Timeout([snapshot(0)?, snapshot(1)?], None))
    }
}

impl Popen {
    fn communicate(
        &mut self,
        input: &[u8],
        deadline: Option<&Deadline>,
        text: bool,
    ) -> Result<Exchange> {
        let data = &mut self.communication;
        if data.started && !input.is_empty() {
            return Err(Error::new(
                Kind::ValueError,
                "Cannot send input after starting communication",
            ));
        }
        let mut out = [Vec::new(), Vec::new()];
        if deadline.is_none() && !data.started && self.pipes.iter().flatten().count() <= 1 {
            if let Some(stdin) = &self.pipes[0] {
                file(stdin, |f| f.communicate_stdin(input, true))?;
            } else {
                for (stream, output) in self.pipes[1..].iter().zip(&mut out) {
                    if let Some(stream) = stream {
                        *output = file(stream, |f| -> Result<_> {
                            let bytes = if text {
                                copy(f.read()?.as_bytes())?
                            } else {
                                f.read_bytes(None)?
                            };
                            f.close().map_err(errno_error)?;
                            Ok(bytes)
                        })?;
                    }
                }
            }
            self.wait(None)?;
            return Ok(Exchange::Done(out));
        }

        let start = Instant::now();
        let timeout = deadline.map(Deadline::seconds).transpose()?;
        if !data.started {
            data.started = true;
            if let Some(stdin) = &self.pipes[0] {
                file(stdin, |f| f.communicate_stdin(&[], input.is_empty()))?;
                data.input = copy(input)?;
            }
        }

        let mut pollfds = [libc::pollfd {
            fd: -1,
            events: 0,
            revents: 0,
        }; 3];
        for (index, (stream, poll)) in self.pipes.iter().zip(&mut pollfds).enumerate() {
            if let Some(stream) = stream
                && (index != 0 || data.offset < data.input.len())
            {
                poll.fd = file(stream, |f| if f.closed() { Ok(-1) } else { f.fileno() })?;
                poll.events = if index == 0 {
                    libc::POLLOUT
                } else {
                    libc::POLLIN
                };
            }
        }
        let select = selector::current();
        while pollfds.iter().any(|poll| poll.fd >= 0) {
            let remaining = timeout.map(|t| t - start.elapsed().as_secs_f64());
            if remaining.is_some_and(|t| t <= 0.0) {
                return data.timeout();
            }
            if !selector::wait(select, &mut pollfds, remaining)? {
                continue;
            }
            if timeout.is_some_and(|t| start.elapsed().as_secs_f64() >= t) {
                return data.timeout();
            }
            for (index, poll) in pollfds.iter_mut().enumerate() {
                if poll.revents & libc::POLLNVAL != 0 {
                    return Err(errno_error(io::Error::from_raw_os_error(libc::EBADF)));
                }
                if poll.revents & (poll.events | libc::POLLERR | libc::POLLHUP) == 0 {
                    continue;
                }
                let finish = if index == 0 {
                    let pending = &data.input[data.offset..];
                    let length = pending.len().min(libc::PIPE_BUF);
                    // SAFETY: the slice is live; a write of at most PIPE_BUF after
                    // readiness cannot deadlock behind the child's full output pipes.
                    let n = unsafe { libc::write(poll.fd, pending.as_ptr().cast(), length) };
                    if n < 0 {
                        let error = io::Error::last_os_error();
                        match error.kind() {
                            io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock => continue,
                            io::ErrorKind::BrokenPipe => true,
                            _ => return Err(errno_error(error)),
                        }
                    } else {
                        data.offset += n as usize;
                        data.offset == data.input.len()
                    }
                } else {
                    let mut bytes = [0u8; 32768];
                    // SAFETY: read fills at most the length of this writable buffer.
                    let n = unsafe { libc::read(poll.fd, bytes.as_mut_ptr().cast(), bytes.len()) };
                    if n < 0 {
                        let error = io::Error::last_os_error();
                        if matches!(
                            error.kind(),
                            io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock
                        ) {
                            continue;
                        }
                        return Err(errno_error(error));
                    }
                    data.observed[index - 1] = true;
                    append_output(&mut data.output[index - 1], &bytes[..n as usize])
                        .map_err(capture_error)?;
                    n == 0
                };
                if finish {
                    file(self.pipes[index].as_ref().unwrap(), |f| f.close())
                        .map_err(errno_error)?;
                    poll.fd = -1;
                }
            }
        }
        let remaining = timeout.map(|t| t - start.elapsed().as_secs_f64());
        if self
            .wait(remaining.map(Deadline::Float).as_ref())?
            .is_none()
        {
            // A timeout raised by wait itself has no captured streams.
            return Ok(Exchange::Timeout([None, None], remaining));
        }
        Ok(Exchange::Done([
            copy(&self.communication.output[0])?,
            copy(&self.communication.output[1])?,
        ]))
    }
}

/// # Safety
/// The handle, optional input, and deadline word are rooted; the fixed tuple
/// descriptor describes exactly the output pipes as bytes, UTF-8 text, or None.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn imre_popen_communicate(
    handle: *const Object,
    input: *const (),
    text: bool,
    deadline_kind: u32,
    deadline: usize,
    descriptor: *const RecordDescriptor,
) -> *mut usize {
    // SAFETY: all storage types follow the call site's sealed Popen signature.
    unsafe {
        let timeout = Deadline::from_word(deadline_kind, deadline);
        let bytes = input_bytes(input, text);
        let result = with_process(handle, |p| -> Result<_> {
            match p.communicate(bytes, timeout.as_ref(), text)? {
                Exchange::Done([stdout, stderr]) => {
                    let stream = |bytes, index| {
                        completed_stream(bytes, WordKind::new(*(*descriptor).kinds.add(index)))
                    };
                    let stdout = stream(stdout, 0)?;
                    let stderr = stream(stderr, 1)?;
                    let tuple = imre_record_new(descriptor);
                    for (index, owner) in [stdout, stderr].into_iter().enumerate() {
                        tuple
                            .add(index)
                            .cast::<*const ()>()
                            .write(owner.into_word() as *const ());
                    }
                    Ok(tuple)
                }
                Exchange::Timeout([stdout, stderr], wait_budget) => {
                    raise_timeout(TimeoutError {
                        cmd: p.args.clone(),
                        timeout: timeout.expect("timed communication"),
                        wait_budget,
                        stdout: stdout.map(crate::BytesOwner::new),
                        stderr: stderr.map(crate::BytesOwner::new),
                    });
                    Ok(std::ptr::null_mut())
                }
            }
        });
        finish(result, std::ptr::null_mut())
    }
}
