//! Boxed callers share the typed process control, communication cursor and file owners.
use super::*;
use crate::objects::Words;

#[derive(Clone)]
pub(crate) struct BoxedPopen(triomphe::Arc<RefCell<Popen>>);

#[derive(Clone)]
pub(crate) struct BoxedPipe(crate::ObjectOwner);

pub(crate) struct BoxedLaunchError {
    pub(crate) error: Error,
    // Only a tagged chdir/exec failure identifies an original Python path object.
    pub(crate) directory: Option<bool>,
    pub(crate) direct: bool,
}

impl From<Error> for BoxedLaunchError {
    fn from(error: Error) -> Self {
        Self {
            error,
            directory: None,
            direct: false,
        }
    }
}

pub(crate) struct BoxedCommunication {
    pub(crate) streams: [Option<Vec<u8>>; 2],
    pub(crate) timed_out: bool,
}

impl PreparedHandles {
    pub(crate) fn uses_posix_spawn(
        &self,
        directory: bool,
        cwd: bool,
        close_fds: bool,
        streams: &[(u32, Option<Int>); 3],
    ) -> bool {
        let modes = streams.each_ref().map(|(mode, _)| *mode);
        launch_error::uses_posix_spawn(directory, cwd, modes, self.child_fds, close_fds)
    }
}

impl BoxedPopen {
    pub(crate) fn default_shell() -> &'static str {
        DEFAULT_SHELL.to_str().expect("ASCII shell path")
    }

    pub(crate) fn probes_executable() -> bool {
        launch_error::posix_spawn_capabilities().0
    }

    /// Reserve the actual child and parent fds before any executable-path callbacks.
    pub(crate) fn prepare_handles(
        streams: &[(u32, Option<Int>); 3],
        pipesize: &Int,
    ) -> Result<PreparedHandles> {
        let modes = streams.each_ref().map(|(mode, _)| *mode);
        let fds = streams
            .each_ref()
            .map(|(_, fd)| fd.as_ref().map_or(-1, |fd| redirect_number(fd).0));
        // SAFETY: pipesize is the live Python int designated by kind 2.
        let capacity = unsafe { super::pipe_size::capacity(pipesize.0, WordKind::new(2)) }?;
        ChildPipes::open(&modes, fds, capacity).map_err(errno_error)
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn launch(
        argv: Vec<Vec<u8>>,
        executable: Option<Vec<u8>>,
        direct: Option<bool>,
        search_path: Option<Vec<u8>>,
        streams: [(u32, Option<Int>); 3],
        handles: Option<PreparedHandles>,
        close_fds: bool,
        restore_signals: bool,
        cwd: Option<Vec<u8>>,
        env: Option<ByteEnvironment>,
        bufsize: Int,
        pipesize: Int,
        start_new_session: bool,
        warn_line_buffering: &mut dyn FnMut() -> Result<()>,
    ) -> std::result::Result<Self, BoxedLaunchError> {
        let mut words = Words::new(8);
        words
            .values
            .try_reserve_exact(argv.len())
            .map_err(|_| Error::allocation())?;
        for arg in argv {
            let owner = crate::BytesOwner::try_new(arg).map_err(|_| Error::allocation())?;
            words
                .values
                .push(crate::BytesOwner::into_raw(owner) as usize);
        }
        let argv =
            crate::ObjectOwner::try_new(Object(crate::StorageCell::new(words.into_storage())))
                .map_err(|_| Error::allocation())?;
        let executable = executable
            .map(crate::BytesOwner::try_new)
            .transpose()
            .map_err(|_| Error::allocation())?;
        let cwd = cwd
            .map(crate::BytesOwner::try_new)
            .transpose()
            .map_err(|_| Error::allocation())?;
        let mut config = LaunchConfig::basic(
            crate::ObjectOwner::as_ptr(&argv).cast(),
            5,
            std::ptr::null(),
            std::ptr::null(),
            u32::from(!restore_signals) << 20,
            bufsize.0,
        );
        config.close_fds = close_fds;
        if let Some(program) = &executable {
            config.executable = crate::BytesOwner::as_ptr(program).cast();
            config.executable_kind = 8;
        }
        if let Some(path) = &cwd {
            config.cwd = crate::BytesOwner::as_ptr(path).cast();
            config.cwd_kind = 8;
        }
        let mut site = None;
        let mut extras = LaunchExtras {
            pipe_size: handles.is_none().then_some((pipesize.0, 2)),
            prepared: handles,
            failure_site: Some(&mut site),
            direct,
            search_path,
            warn_line_buffering: Some(warn_line_buffering),
            environment: env,
            // A new session uses the ordinary child-settings path with no group or umask change.
            child: start_new_session.then_some((0, Int::small(-1).unwrap().0, true)),
            ..Default::default()
        };
        for (i, ((mode, fd), shift)) in streams.iter().zip([12, 0, 4]).enumerate() {
            match (mode, fd) {
                (0 | 1 | 2 | 5, None) => {}
                (6, Some(fd)) => extras.redirects[i] = fd.0,
                _ => {
                    return Err(Error::new(
                        Kind::SystemError,
                        "invalid subprocess stream configuration",
                    )
                    .into());
                }
            }
            config.streams |= *mode << shift;
        }
        // Reserve the public handle before a successful spawn can leave a child to reap.
        let mut owner = triomphe::UniqueArc::try_new_uninit().map_err(|_| Error::allocation())?;
        // SAFETY: argv, executable, cwd, sizes and descriptor integers stay rooted through spawn.
        let process = unsafe { spawn(&config, extras) }.map_err(|error| BoxedLaunchError {
            error,
            directory: site.map(|(site, _)| matches!(site, launch_error::Site::Directory)),
            direct: site.is_some_and(|(_, direct)| direct),
        })?;
        owner.write(RefCell::new(process));
        // SAFETY: write initialized the unique allocation; sharing it performs no allocation.
        Ok(Self(
            unsafe { triomphe::UniqueArc::assume_init(owner) }.shareable(),
        ))
    }

    pub(crate) fn pipe(&self, index: usize) -> Option<crate::ObjectOwner> {
        self.0.borrow().pipes.get(index)?.clone()
    }

    pub(crate) fn pid(&self) -> i32 {
        self.0.borrow().control.pid
    }

    pub(crate) fn status(&self, poll: bool) -> Result<Option<i32>> {
        let process = self.0.borrow();
        let status = if poll {
            process.control.poll()?
        } else {
            process.control.status()
        };
        Ok(status.map(ReturnCode::kernel))
    }

    pub(crate) fn wait<E: From<Error>>(
        &self,
        seconds: Option<f64>,
        check: impl FnMut() -> std::result::Result<(), E>,
    ) -> std::result::Result<Option<i32>, E> {
        let control = self.0.borrow().control.clone();
        Ok(control
            .wait_with(seconds.map(Deadline::Float).as_ref(), check)?
            .map(ReturnCode::kernel))
    }

    pub(crate) fn signal(&self, number: i32) -> Result<()> {
        self.0.borrow().signal(|| Ok(number))
    }

    pub(crate) fn close_wait(&self) -> Result<()> {
        self.0.borrow_mut().close_wait()
    }
    pub(crate) fn finalize(&self) -> bool {
        self.0.borrow_mut().finalize()
    }
    pub(crate) fn cleanup() {
        cleanup()
    }

    pub(crate) fn begin_communication(
        &self,
        input: &[u8],
        seconds: Option<f64>,
        select: bool,
    ) -> Result<communicate::PendingExchange> {
        self.0
            .borrow_mut()
            .begin_exchange(input, seconds, false, select)
    }

    pub(crate) fn finish_communication<E: From<Error>>(
        &self,
        pending: communicate::PendingExchange,
        check: impl FnMut() -> std::result::Result<(), E>,
    ) -> std::result::Result<BoxedCommunication, E> {
        let exchange = pending.drain(
            |timeout, select| {
                self.0
                    .borrow_mut()
                    .exchange_pipes(&[], timeout, false, select)
            },
            check,
        )?;
        let process = self.0.borrow();
        let (streams, timed_out) = match exchange {
            communicate::Exchange::Done([out, err]) => (
                [
                    process.pipes[1].as_ref().map(|_| out),
                    process.pipes[2].as_ref().map(|_| err),
                ],
                false,
            ),
            communicate::Exchange::Timeout(streams, _) => (streams, true),
        };
        Ok(BoxedCommunication { streams, timed_out })
    }
}

impl BoxedPipe {
    pub(crate) fn from_owner(owner: crate::ObjectOwner) -> Self {
        Self(owner)
    }

    fn with_file<T>(&self, f: impl FnOnce(&mut FileState) -> T) -> T {
        // SAFETY: the rooted pipe file is only borrowed during a sealed operation.
        unsafe { self.0.0.with_mut(|storage| f(storage.file_mut())) }
    }

    pub(crate) fn is_raw(&self) -> bool {
        self.with_file(|file| file.is_raw())
    }

    pub(crate) fn close_raw(&self) -> Result<()> {
        self.with_file(|file| file.close_raw_pipe().map_err(errno_error))
    }

    pub(crate) fn read(&self, size: Option<usize>, line: bool) -> Result<Vec<u8>> {
        self.with_file(|file| {
            if line {
                file.read_pipe_line(size)
            } else {
                file.read_bytes(size)
            }
        })
    }

    pub(crate) fn take_read_ahead(&self) -> Result<Vec<u8>> {
        self.with_file(FileState::take_pipe_read_ahead)
    }

    pub(crate) fn peek(&self) -> Result<Vec<u8>> {
        self.with_file(|file| file.peek_pipe())
    }

    pub(crate) fn read1(&self, size: Option<usize>) -> Result<Vec<u8>> {
        self.with_file(|file| file.read_pipe_once(size))
    }

    pub(crate) fn write(&self, data: &[u8]) -> Result<()> {
        self.with_file(|file| file.write_bytes(data))
    }

    pub(crate) fn flush(&self) -> Result<()> {
        self.with_file(|file| file.flush())
    }

    pub(crate) fn closed(&self) -> bool {
        self.with_file(|file| file.closed())
    }
    pub(crate) fn fileno(&self) -> Result<i32> {
        self.with_file(|file| file.fileno())
    }
    pub(crate) fn close(&self) -> Result<()> {
        self.with_file(|file| file.close().map_err(errno_error))
    }
}
