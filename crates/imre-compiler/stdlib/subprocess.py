"""POSIX process handles for runtime-loaded Imre modules."""
import _imre_subprocess as _process
import selectors

_PopenSelector = selectors.PollSelector

PIPE = -1
STDOUT = -2
DEVNULL = -3

_active = []


def _cleanup():
    for child in _active[:]:
        if child.poll() is not None:
            _active.remove(child)
    _process.cleanup()


def _decode(data, encoding, errors):
    if data is None:
        return None
    return data.decode(encoding, errors).replace("\r\n", "\n").replace("\r", "\n")


class Popen:
    _handle = None

    def __init__(self, args, bufsize=-1, executable=None, stdin=None, stdout=None,
                 stderr=None, *, close_fds=True, cwd=None, env=None, restore_signals=True,
                 pipesize=-1):
        if executable is not None:
            raise NotImplementedError("an alternate subprocess executable is unsupported")
        if bufsize is None:
            bufsize = -1
        if not isinstance(bufsize, int):
            raise TypeError("bufsize must be an integer")
        # Boxed exact ints may not share CPython's cached -2 object.
        if stdout is STDOUT or (type(stdout) is int and stdout == STDOUT):
            raise ValueError("STDOUT can only be used for stderr")
        if pipesize is None:
            pipesize = -1
        if not isinstance(pipesize, int):
            raise TypeError("pipesize must be an integer")
        _cleanup()
        self.args = args
        self.pipesize = pipesize
        self._sigint_wait_secs = 0.25
        self._handle = _process.launch(args, stdin, stdout, stderr, close_fds,
                                       restore_signals, cwd, env, bufsize, pipesize)
        try:
            self.stdin = _process.pipe(self._handle, 0)
            self.stdout = _process.pipe(self._handle, 1)
            self.stderr = _process.pipe(self._handle, 2)
        except BaseException:
            self.kill()
            _process.close_wait(self._handle)
            raise

    def __del__(self):
        if self._handle is not None and _process.finalize(self._handle) and _active is not None:
            _active.append(self)

    @property
    def pid(self):
        return _process.pid(self._handle)

    @property
    def returncode(self):
        return _process.status(self._handle, False)

    def poll(self):
        return _process.status(self._handle, True)

    def wait(self, timeout=None):
        result = _process.wait(self._handle, timeout)
        if result is None:
            raise TimeoutExpired(self.args, timeout)
        return result

    def communicate(self, input=None, timeout=None):
        use_select = _PopenSelector is not selectors.PollSelector
        if use_select and _PopenSelector is not getattr(selectors, "SelectSelector", None):
            raise NotImplementedError("unsupported subprocess selector")
        out, err, expired, remaining = _process.communicate(self._handle, input, timeout, use_select)
        if expired:
            error = TimeoutExpired(self.args, timeout if remaining is None else remaining,
                                   output=out, stderr=err)
            error.timeout = timeout
            raise error
        return out, err

    def send_signal(self, sig):
        _process.signal(self._handle, sig)

    def terminate(self):
        self.send_signal(15)

    def kill(self):
        self.send_signal(9)

    def __enter__(self):
        return self

    def __exit__(self, exc_type, exc_value, traceback):
        if self.stdout:
            self.stdout.close()
        if self.stderr:
            self.stderr.close()
        try:
            if self.stdin:
                self.stdin.close()
        finally:
            if exc_type == KeyboardInterrupt:
                # The child may already have received SIGINT. Give it a brief chance
                # to exit without holding the caller in an unbounded wait.
                if self._sigint_wait_secs > 0:
                    _process.wait(self._handle, self._sigint_wait_secs)
                self._sigint_wait_secs = 0
            else:
                self.wait()


class SubprocessError(Exception):
    pass


class TimeoutExpired(SubprocessError):
    def __init__(self, cmd, timeout, output=None, stderr=None):
        self.cmd = cmd
        self.timeout = timeout
        self.output = output
        self.stderr = stderr

    @property
    def stdout(self):
        return self.output

    @stdout.setter
    def stdout(self, value):
        self.output = value

    def __str__(self):
        return "Command '%s' timed out after %s seconds" % (self.cmd, self.timeout)


class CalledProcessError(SubprocessError):
    def __init__(self, returncode, cmd, output=None, stderr=None):
        self.returncode = returncode
        self.cmd = cmd
        self.output = output
        self.stderr = stderr

    @property
    def stdout(self):
        return self.output

    @stdout.setter
    def stdout(self, value):
        self.output = value

    def __str__(self):
        if self.returncode < 0:
            number = -self.returncode
            try:
                from signal import Signals
                name = repr(Signals(number))
            except (ImportError, ValueError):
                name = f"unknown signal {number}"
            return f"Command '{self.cmd}' died with {name}."
        return f"Command '{self.cmd}' returned non-zero exit status {self.returncode}."


def call(*popenargs, timeout=None, **kwargs):
    with Popen(*popenargs, **kwargs) as process:
        try:
            return process.wait(timeout=timeout)
        except BaseException:
            process.kill()
            raise


def check_call(*popenargs, **kwargs):
    code = call(*popenargs, **kwargs)
    if code:
        cmd = kwargs.get("args")
        if cmd is None:
            cmd = popenargs[0]
        raise CalledProcessError(code, cmd)
    return 0


class CompletedProcess:
    def __init__(self, args, returncode, stdout=None, stderr=None):
        self.args = args
        self.returncode = returncode
        self.stdout = stdout
        self.stderr = stderr

    def check_returncode(self):
        if self.returncode:
            raise CalledProcessError(self.returncode, self.args, self.stdout, self.stderr)


def run(*popenargs, input=None, capture_output=False, timeout=None, check=False, **kwargs):
    if input is not None:
        if kwargs.get("stdin") is not None:
            raise ValueError("stdin and input arguments may not both be used.")
        kwargs["stdin"] = PIPE
    if capture_output:
        if kwargs.get("stdout") is not None or kwargs.get("stderr") is not None:
            raise ValueError("stdout and stderr arguments may not be used with capture_output.")
        kwargs["stdout"] = PIPE
        kwargs["stderr"] = PIPE
    text = kwargs.pop("text", None)
    newlines = kwargs.pop("universal_newlines", None)
    if text is not None and newlines is not None and bool(text) != bool(newlines):
        raise SubprocessError("Cannot disambiguate when both text and universal_newlines are supplied but different. Pass one or the other.")
    encoding = kwargs.pop("encoding", None)
    errors = kwargs.pop("errors", None)
    text_mode = text or newlines or encoding or errors
    encoding = encoding or "utf-8"
    errors = errors or "strict"
    # run owns its streams. Only the resulting data is exposed to the caller.
    if len(popenargs) < 2:
        kwargs.setdefault("bufsize", 0)
    with Popen(*popenargs, **kwargs) as process:
        try:
            if text_mode and input is not None:
                input = input.encode(encoding, errors)
            stdout, stderr = process.communicate(input, timeout=timeout)
            if text_mode:
                stdout = _decode(stdout, encoding, errors)
                stderr = _decode(stderr, encoding, errors)
        except BaseException:
            process.kill()
            raise
        code = process.poll()
        if check and code:
            raise CalledProcessError(code, process.args, stdout, stderr)
    return CompletedProcess(process.args, code, stdout, stderr)
