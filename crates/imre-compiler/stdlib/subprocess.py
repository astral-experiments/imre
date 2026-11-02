"""POSIX process handles for runtime-loaded Imre modules."""
import _imre_subprocess as _process
from _weakref import ref as _ref
from time import monotonic as _time
import io
import selectors
import sys

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


def _optim_args_from_interpreter_flags():
    """Return a list of command-line arguments reproducing the current
    optimization settings in sys.flags."""
    args = []
    value = sys.flags.optimize
    if value > 0:
        args.append('-' + 'O' * value)
    return args


def _args_from_interpreter_flags():
    """Return a list of command-line arguments reproducing the current
    settings in sys.flags, sys.warnoptions and sys._xoptions."""
    flag_opt_map = {
        'debug': 'd',
        # 'inspect': 'i',
        # 'interactive': 'i',
        'dont_write_bytecode': 'B',
        'no_site': 'S',
        'verbose': 'v',
        'bytes_warning': 'b',
        'quiet': 'q',
        # -O is handled in _optim_args_from_interpreter_flags()
    }
    args = _optim_args_from_interpreter_flags()
    for flag, opt in flag_opt_map.items():
        v = getattr(sys.flags, flag)
        if v > 0:
            args.append('-' + opt * v)

    if sys.flags.isolated:
        args.append('-I')
    else:
        if sys.flags.ignore_environment:
            args.append('-E')
        if sys.flags.no_user_site:
            args.append('-s')
        if sys.flags.safe_path:
            args.append('-P')

    # -W options
    warnopts = sys.warnoptions[:]
    xoptions = getattr(sys, '_xoptions', {})
    bytes_warning = sys.flags.bytes_warning
    dev_mode = sys.flags.dev_mode

    if bytes_warning > 1:
        warnopts.remove("error::BytesWarning")
    elif bytes_warning:
        warnopts.remove("default::BytesWarning")
    if dev_mode:
        warnopts.remove('default')
    for opt in warnopts:
        args.append('-W' + opt)

    # -X options
    if dev_mode:
        args.extend(('-X', 'dev'))
    for opt in sorted(xoptions):
        if opt == 'dev':
            # handled above via sys.flags.dev_mode
            continue
        value = xoptions[opt]
        if value is True:
            arg = opt
        else:
            arg = '%s=%s' % (opt, value)
        args.extend(('-X', arg))

    return args


def _text_encoding():
    import sys

    if sys.flags.warn_default_encoding:
        import warnings
        frame = sys._getframe()
        filename = frame.f_code.co_filename
        stacklevel = 2
        frame = frame.f_back
        while frame is not None and frame.f_code.co_filename == filename:
            stacklevel += 1
            frame = frame.f_back
        warnings.warn("'encoding' argument not specified.", EncodingWarning, stacklevel)
    if sys.flags.utf8_mode:
        return "utf-8"
    from _locale import getencoding
    return getencoding()


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
                 universal_newlines=None, encoding=None, errors=None, text=None, start_new_session=False, umask=-1, pipesize=-1):
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
            self.encoding = _text_encoding()
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
                                       errors if self.text_mode else None, shell, start_new_session, umask)
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
            self._stdin_ref = None if self.stdin is None else _ref(self.stdin)
            self._replacement_output = None
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
        endtime = None if timeout is None else _time() + timeout
        try:
            return self._wait(timeout=timeout)
        except KeyboardInterrupt:
            self._wait_after_keyboard_interrupt(endtime)
            raise

    def _wait(self, timeout):
        result = _process.wait(self._handle, timeout)
        if result is None:
            raise TimeoutExpired(self.args, timeout)
        return result

    def _wait_after_keyboard_interrupt(self, endtime):
        # The child may also have received SIGINT. Share one brief wait with
        # __exit__ and respect the caller's original deadline.
        grace = self._sigint_wait_secs
        self._sigint_wait_secs = 0
        if endtime is not None:
            grace = min(grace, endtime - _time())
        try:
            self._wait(timeout=grace)
        except TimeoutExpired:
            pass

    def communicate(self, input=None, timeout=None):
        if self._communication_started and input:
            raise ValueError("Cannot send input after starting communication")
        if (timeout is None and not self._communication_started
                and (self.stdin is None) + (self.stdout is None) + (self.stderr is None) >= 2):
            out = err = None
            if self.stdin:
                if input:
                    try:
                        self.stdin.write(input)
                    except BrokenPipeError:
                        pass
                try:
                    self.stdin.close()
                except BrokenPipeError:
                    pass
            elif self.stdout:
                out = self.stdout.read()
                self.stdout.close()
            elif self.stderr:
                err = self.stderr.read()
                self.stderr.close()
            self.wait()
            return out, err

        endtime = None if timeout is None else _time() + timeout
        try:
            stdout, stderr = self._communicate(input, endtime, timeout)
        except KeyboardInterrupt:
            self._wait_after_keyboard_interrupt(endtime)
            raise
        finally:
            self._communication_started = True
        try:
            self.wait(timeout=None if endtime is None else endtime - _time())
        except TimeoutExpired as exc:
            exc.timeout = timeout
            raise
        return stdout, stderr

    def _communicate(self, input, endtime, orig_timeout):
        use_select = _PopenSelector is not selectors.PollSelector
        if use_select and _PopenSelector is not getattr(selectors, "SelectSelector", None):
            raise NotImplementedError("unsupported subprocess selector")
        same_stdin = self._stdin_ref is None if self.stdin is None else (
            self._stdin_ref is not None and self._stdin_ref() is self.stdin)
        if self._replacement_output is not None or (not self._communication_started and not same_stdin):
            return self._communicate_replaced_stdin(input, endtime, orig_timeout)
        if self.text_mode:
            if self.stdin is None or self._communication_started:
                input = None
            if input is not None:
                # A false input closes stdin before encoding. A BOM produced
                # from an empty string is still validated, but the pipe is closed.
                if not input:
                    try:
                        self.stdin.close()
                    except BrokenPipeError:
                        pass
                input = input.encode(self.stdin.encoding, self.stdin.errors)
                if not input:
                    input = None
                elif not isinstance(input, (bytes, bytearray)):
                    input = memoryview(input)
        timeout = None if endtime is None else endtime - _time()
        out, err, expired = _process.communicate(self._handle, input, timeout, use_select)
        if expired:
            raise TimeoutExpired(self.args, orig_timeout, output=out, stderr=err)
        try:
            self.wait(timeout=None if endtime is None else endtime - _time())
        except TimeoutExpired as exc:
            exc.timeout = orig_timeout
            raise
        if self.text_mode:
            out, err = _decode(out, self.stdout), _decode(err, self.stderr)
        return out, err

    def _communicate_replaced_stdin(self, input, deadline, timeout):
        # The process handle still owns its original pipe. Use the public stream's
        # descriptor for readiness and writes, and leave the original pipe alone.
        import os
        import select

        if self._replacement_output is None:
            self._replacement_output = [[], []]
            self._replacement_input = b""
            self._replacement_offset = 0
            self._communication_started = True
            if self.stdin:
                try:
                    self.stdin.flush()
                except BrokenPipeError:
                    pass
                except ValueError:
                    if not self.stdin.closed:
                        raise
                if input:
                    if self.text_mode:
                        input = input.encode(self.stdin.encoding, self.stdin.errors)
                    self._replacement_input = input
                else:
                    try:
                        self.stdin.close()
                    except BrokenPipeError:
                        pass
        elif input:
            raise ValueError("Cannot send input after starting communication")

        # A caller may mutate the buffer before retrying a timed-out exchange.
        # Snapshot non-byte-format views in byte order for this call's offset.
        input_bytes = self._replacement_input
        if not isinstance(input_bytes, (bytes, bytearray)):
            input_bytes = bytes(memoryview(input_bytes))
        output = self._replacement_output
        with _PopenSelector() as selector:
            pending = 0
            if (self.stdin and not self.stdin.closed and
                    self._replacement_offset < len(input_bytes)):
                selector.register(self.stdin.fileno(), selectors.EVENT_WRITE, self.stdin)
                pending += 1
            for stream in (self.stdout, self.stderr):
                if stream and not stream.closed:
                    selector.register(stream.fileno(), selectors.EVENT_READ, stream)
                    pending += 1
            while pending:
                remaining = None if deadline is None else deadline - _time()
                ready = selector.select(remaining) if remaining is None or remaining > 0 else []
                if deadline is not None and _time() >= deadline:
                    raise TimeoutExpired(self.args, timeout,
                                         output=b"".join(output[0]) if output[0] else None,
                                         stderr=b"".join(output[1]) if output[1] else None)
                for key, _ in ready:
                    if key.data is self.stdin:
                        offset = self._replacement_offset
                        chunk = input_bytes[offset:offset + getattr(select, "PIPE_BUF", 512)]
                        try:
                            self._replacement_offset += os.write(key.fd, chunk)
                        except BrokenPipeError:
                            pass
                        else:
                            if self._replacement_offset < len(input_bytes):
                                continue
                    else:
                        data = os.read(key.fd, 32768)
                        output[0 if key.data is self.stdout else 1].append(data)
                        if data:
                            continue
                    selector.unregister(key.fd)
                    key.data.close()
                    pending -= 1
        try:
            self.wait(timeout=None if deadline is None else deadline - _time())
        except TimeoutExpired as exc:
            exc.timeout = timeout
            raise
        stdout = None if self.stdout is None else b"".join(output[0])
        stderr = None if self.stderr is None else b"".join(output[1])
        if self.text_mode:
            return _decode(stdout, self.stdout), _decode(stderr, self.stderr)
        return stdout, stderr

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
                    try:
                        self._wait(timeout=self._sigint_wait_secs)
                    except TimeoutExpired:
                        pass
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
