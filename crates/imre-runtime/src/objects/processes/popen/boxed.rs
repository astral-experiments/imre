//! Boxed callers share the typed process control, communication cursor and file owners.
use super::*;
use crate::objects::Words;

#[derive(Clone)]
pub(crate) struct BoxedPopen(triomphe::Arc<RefCell<Popen>>);

#[derive(Clone)]
pub(crate) struct BoxedPipe(crate::ObjectOwner);

/// The constructor retains every owned descriptor until the virtual launch hook runs.
pub(crate) struct BoxedPreparation {
    pub(crate) launch: Option<BoxedLaunch>,
    pipes: Pipes,
}

pub(crate) struct BoxedLaunch {
    pub(crate) streams: [(u32, Option<Int>); 3],
    pub(crate) handles: PreparedHandles,
    pub(crate) bufsize: Int,
}

impl BoxedPreparation {
    pub(crate) fn new(
        streams: [(u32, Option<Int>); 3],
        mut handles: PreparedHandles,
        bufsize: Int,
    ) -> Result<(Self, [Int; 6])> {
        if handles.parents.iter().any(Option::is_some) {
            let size = bufsize
                .to_i64()
                .and_then(|n| i32::try_from(n).ok())
                .ok_or_else(c_int_overflow)?;
            handles.wrap_parents(false, size)?;
        }

        let mut child = handles.child_fds.map(|fd| Int::from(i64::from(fd)));
        for (index, (_, redirect)) in streams.iter().enumerate() {
            if let Some(fd) = redirect {
                child[index] = fd.clone();
            }
        }
        if streams[2].0 == 5 {
            child[2] = if child[1].as_small() == Some(-1) {
                Int::from(1)
            } else {
                child[1].clone()
            };
        }
        let mut parent = [-1; 3];
        for (index, file) in handles.pipes.iter().enumerate() {
            if let Some(file) = file {
                parent[index] = BoxedPipe(file.clone()).fileno()?;
            }
        }
        let [stdin, stdout, stderr] = child;
        let [write, read, err] = parent.map(|fd| Int::from(i64::from(fd)));
        let fds = [stdin, write, read, stdout, err, stderr];
        let pipes = handles.pipes.clone();
        let launch = Some(BoxedLaunch {
            streams,
            handles,
            bufsize,
        });
        Ok((Self { launch, pipes }, fds))
    }

    pub(crate) fn pipe(&self, index: usize) -> Option<crate::ObjectOwner> {
        self.pipes[index].clone()
    }

    pub(crate) fn release(&mut self, failed: bool) {
        // A hook can retain or replace a public wrapper or raise after calling super.
        for file in std::mem::take(&mut self.pipes).into_iter().flatten() {
            if failed {
                let _ = BoxedPipe(file).close();
            }
        }
        self.launch = None;
    }
}

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
    pub(crate) fn inherit_stderr(&mut self, stream: &mut (u32, Option<Int>), stdout: Int) {
        if stdout.as_small() == Some(1) {
            return;
        }
        self.child_fds[2] = redirect_number(&stdout).0;
        *stream = if stdout.as_small() == Some(-1) {
            (0, None)
        } else {
            (6, Some(stdout))
        };
    }

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

    pub(crate) fn spawn_closes_fds() -> bool {
        launch_error::posix_spawn_capabilities().1
    }

    /// Distinct Python objects can convert to the same descriptor after set deduplication.
    pub(crate) fn push_pass_fd(fds: &mut Vec<RawFd>, number: &Int) -> Result<()> {
        let previous = fds.last().copied();
        super::pass_fds::push(fds, number)?;
        if previous == fds.last().copied() {
            return Err(Error::new(Kind::ValueError, "bad value(s) in fds_to_keep"));
        }
        Ok(())
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
        handles: PreparedHandles,
        close_fds: bool,
        restore_signals: bool,
        cwd: Option<Vec<u8>>,
        env: Option<ByteEnvironment>,
        bufsize: Int,
        start_new_session: bool,
        process_group: i32,
        umask: i32,
        keep: Vec<RawFd>,
        extra_groups: Option<Vec<libc::gid_t>>,
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
            prepared: Some(handles),
            failure_site: Some(&mut site),
            direct,
            search_path,
            environment: env,
            boxed_child: Some(ChildAttributes {
                new_session: start_new_session,
                group: process_group,
                extra_groups,
                mask: (umask >= 0).then_some(umask as libc::mode_t),
                keep,
            }),
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
        let mut process = unsafe { spawn(&config, extras) }.map_err(|error| BoxedLaunchError {
            error,
            directory: site.map(|(site, _)| matches!(site, launch_error::Site::Directory)),
            direct: site.is_some_and(|(_, direct)| direct),
        })?;
        // The boxed Python owner emits the warning. Drop still reaps or defers
        // the child if that owner's finalizer raises or releases a live handle.
        process.native_finalizer_pending = false;
        owner.write(RefCell::new(process));
        // SAFETY: write initialized the unique allocation; sharing it performs no allocation.
        Ok(Self(
            unsafe { triomphe::UniqueArc::assume_init(owner) }.shareable(),
        ))
    }

    pub(crate) fn pid(&self) -> i32 {
        self.0.borrow().control.pid
    }

    pub(crate) fn poll(&self) -> Result<Option<i32>> {
        Ok(self.0.borrow().control.poll()?.map(ReturnCode::kernel))
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
    pub(crate) fn cleanup() {
        cleanup()
    }

    pub(crate) fn begin_communication(
        &self,
        input: &[u8],
        seconds: Option<f64>,
        select: bool,
    ) -> Result<communicate::PendingExchange> {
        self.0.borrow_mut().begin_exchange(
            input,
            seconds.map(Deadline::Float).as_ref(),
            false,
            select,
        )
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
