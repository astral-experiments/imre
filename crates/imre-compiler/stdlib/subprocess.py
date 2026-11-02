"""POSIX process handles for runtime-loaded Imre modules."""
import _imre_subprocess as _process
import io
import selectors

__all__ = ["Popen", "PIPE", "STDOUT", "call", "check_call", "getstatusoutput",
           "getoutput", "check_output", "run", "CalledProcessError", "DEVNULL",
           "SubprocessError", "TimeoutExpired", "CompletedProcess"]

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


def _decode(data, stream):
    if data is None:
        return None
    return data.decode(stream.encoding, stream.errors).replace("\r\n", "\n").replace("\r", "\n")


def _exec_search_path(env):
    """Read the POSIX search path independently of the child's saved environment."""
    from os import environ, fsdecode, fsencode
    if env is None:
        env = environ
    try:
        path = env.get("PATH")
    except TypeError:
        path = None
    try:
        byte_path = env[b"PATH"]
    except (KeyError, TypeError):
        pass
    else:
        if path is not None:
            raise ValueError("env cannot contain 'PATH' and b'PATH' keys")
        path = byte_path
    if path is None:
        return b"/bin:/usr/bin"
    if isinstance(path, bytes):
        path = fsdecode(path)
    return b":".join(fsencode(part) for part in path.split(":"))


class Popen:
    _handle = None

    def __init__(self, args, bufsize=-1, executable=None, stdin=None, stdout=None,
                 stderr=None, *, close_fds=True, shell=False, cwd=None, env=None, restore_signals=True,
                 universal_newlines=None, encoding=None, errors=None, text=None, pipesize=-1):
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
        if (text is not None and universal_newlines is not None
                and bool(text) != bool(universal_newlines)):
            raise SubprocessError("Cannot disambiguate when both text and universal_newlines "
                                  "are supplied but different. Pass one or the other.")
        self.encoding = encoding
        self.errors = errors
        self.text_mode = encoding or errors or text or universal_newlines
        if self.text_mode and encoding is None:
            self.encoding = io.text_encoding(None, 2)
            if self.encoding == "locale":
                from _locale import getencoding
                self.encoding = getencoding()
        line_buffering = self.text_mode and bufsize == 1
        if line_buffering:
            bufsize = -1
        self._communication_started = False
        _cleanup()
        self.args = args
        self.pipesize = pipesize
        self._sigint_wait_secs = 0.25
        self._handle = _process.launch(args, executable, stdin, stdout, stderr, close_fds,
                                       restore_signals, cwd, env, bufsize, pipesize,
                                       encoding if self.text_mode else None,
                                       errors if self.text_mode else None, shell)
        try:
            self.stdin = _process.pipe(self._handle, 0)
            self.stdout = _process.pipe(self._handle, 1)
            self.stderr = _process.pipe(self._handle, 2)
            if self.text_mode:
                if self.stdin is not None:
                    self.stdin = io.TextIOWrapper(self.stdin, encoding=self.encoding, errors=errors,
                                                 write_through=True, line_buffering=line_buffering)
                if self.stdout is not None:
                    self.stdout = io.TextIOWrapper(self.stdout, encoding=self.encoding, errors=errors)
                if self.stderr is not None:
                    self.stderr = io.TextIOWrapper(self.stderr, encoding=self.encoding, errors=errors)
        except BaseException:
            self.kill()
            _process.close_wait(self._handle)
            raise

    def __del__(self):
        if self._handle is not None and _process.finalize(self._handle) and _active is not None:
            _active.append(self)

    @property
    def universal_newlines(self):
        return self.text_mode

    @universal_newlines.setter
    def universal_newlines(self, value):
        self.text_mode = bool(value)

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
        if self.text_mode:
            if self._communication_started and input:
                raise ValueError("Cannot send input after starting communication")
            if self.stdin is None or self._communication_started:
                input = None
        single_pipe = (timeout is None and not self._communication_started
                       and (self.stdin is None) + (self.stdout is None) + (self.stderr is None) >= 2)
        if single_pipe:
            reader = self.stdout or self.stderr
            if isinstance(reader, io.TextIOWrapper):
                # A direct read may already have decoded characters in this wrapper.
                data = reader.read()
                reader.close()
                self.wait()
                return (data, None) if reader is self.stdout else (None, data)
            if isinstance(self.stdin, io.TextIOWrapper):
                if input:
                    try:
                        self.stdin.write(input)
                    except BrokenPipeError:
                        pass
                # The native single-pipe path flushes and closes the shared binary pipe.
                input = None
        else:
            self._communication_started = True
            if self.text_mode and input is not None:
                input = input.encode(self.stdin.encoding, self.stdin.errors)
        out, err, expired, remaining = _process.communicate(self._handle, input, timeout, use_select)
        if expired:
            error = TimeoutExpired(self.args, timeout if remaining is None else remaining,
                                   output=out, stderr=err)
            error.timeout = timeout
            raise error
        if self.text_mode and not single_pipe:
            out, err = _decode(out, self.stdout), _decode(err, self.stderr)
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


def check_output(*popenargs, timeout=None, **kwargs):
    for key in ("stdout", "check"):
        if key in kwargs:
            raise ValueError(f"{key} argument not allowed, it will be overridden.")
    if "input" in kwargs and kwargs["input"] is None:
        # Explicit None supplies EOF, like an empty input in the corresponding mode.
        text = (kwargs.get("text") or kwargs.get("universal_newlines")
                or kwargs.get("encoding") or kwargs.get("errors"))
        kwargs["input"] = "" if text else b""
    return run(*popenargs, stdout=PIPE, timeout=timeout, check=True, **kwargs).stdout


def getstatusoutput(cmd, *, encoding=None, errors=None):
    try:
        output = check_output(cmd, shell=True, text=True, stderr=STDOUT,
                              encoding=encoding, errors=errors)
        status = 0
    except CalledProcessError as exc:
        output = exc.output
        status = exc.returncode
    if output.endswith("\n"):
        output = output[:-1]
    return status, output


def getoutput(cmd, *, encoding=None, errors=None):
    return getstatusoutput(cmd, encoding=encoding, errors=errors)[1]


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
    # run owns its streams. Only the resulting data is exposed to the caller.
    if len(popenargs) < 2:
        kwargs.setdefault("bufsize", 0)
    with Popen(*popenargs, **kwargs) as process:
        try:
            stdout, stderr = process.communicate(input, timeout=timeout)
        except BaseException:
            process.kill()
            raise
        code = process.poll()
        if check and code:
            raise CalledProcessError(code, process.args, stdout, stderr)
    return CompletedProcess(process.args, code, stdout, stderr)
