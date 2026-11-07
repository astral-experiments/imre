"""POSIX process handles for runtime-loaded Imre modules."""
import _imre_subprocess as _process
from _weakref import ref as _ref
from time import monotonic as _time
from types import GenericAlias as _GenericAlias
from warnings import warn as _warn
import io
import selectors
import sys
import time

__all__ = ["Popen", "PIPE", "STDOUT", "call", "check_call", "getstatusoutput",
           "getoutput", "check_output", "run", "CalledProcessError", "DEVNULL",
           "SubprocessError", "TimeoutExpired", "CompletedProcess"]

_PopenSelector = selectors.PollSelector

PIPE = -1
STDOUT = -2
DEVNULL = -3

_active = []


def _cleanup():
    if _active is not None:
        for child in _active[:]:
            if child._internal_poll(_deadstate=sys.maxsize) is not None:
                try:
                    _active.remove(child)
                except ValueError:
                    # A concurrent cleanup may already have removed this child.
                    pass
    _process.cleanup()


# From CPython 3.14.5, Lib/subprocess.py. See stdlib/docstrings/LICENSE.
def list2cmdline(seq):
    """Translate arguments into a command line using the Microsoft C runtime's rules."""
    from os import fsdecode

    result = []
    for arg in map(fsdecode, seq):
        if result:
            result.append(' ')
        quoted = " " in arg or "\t" in arg or not arg
        if quoted:
            result.append('"')

        # Backslashes are literal unless followed by a quote, including a closing quote.
        backslashes = []
        for char in arg:
            if char == '\\':
                backslashes.append(char)
            elif char == '"':
                result.append('\\' * (2 * len(backslashes)))
                backslashes = []
                result.append('\\"')
            else:
                if backslashes:
                    result.extend(backslashes)
                    backslashes = []
                result.append(char)

        result.extend(backslashes)
        if quoted:
            result.extend(backslashes)
            result.append('"')
    return ''.join(result)


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


def _pipe_replaced(stream, original_ref):
    if stream is None:
        return original_ref is not None
    return original_ref is None or original_ref() is not stream


def _flush_stdin(stream):
    try:
        stream.flush()
    except BrokenPipeError:
        pass
    except ValueError:
        if not stream.closed:
            raise


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


def _get_group_id(group):
    import os
    if not hasattr(os, "setregid"):
        raise ValueError("The 'group' parameter is not supported on the current platform")
    if isinstance(group, str):
        try:
            import grp
        except ImportError:
            raise ValueError("The group parameter cannot be a string on systems without the grp module")
        group = grp.getgrnam(group).gr_gid
    elif not isinstance(group, int):
        raise TypeError(f"Group must be a string or an integer, not {type(group)}")
    if group < 0:
        raise ValueError(f"Group ID cannot be negative, got {group}")
    return group


def _get_extra_group_ids(extra_groups):
    if isinstance(extra_groups, str):
        raise ValueError("Groups must be a list, not a string")
    gids = []
    for group in extra_groups:
        if isinstance(group, str):
            try:
                import grp
            except ImportError:
                raise ValueError("Items in extra_groups cannot be strings on systems without the grp module")
            group = grp.getgrnam(group).gr_gid
        elif not isinstance(group, int):
            raise TypeError(f"Items in extra_groups must be a string or integer, not {type(group)}")
        gids.append(group)
    for gid in gids:
        if gid < 0:
            raise ValueError(f"Group ID cannot be negative, got {gid}")
    return gids


def _get_user_id(user):
    import os
    if not hasattr(os, "setreuid"):
        raise ValueError("The 'user' parameter is not supported on the current platform")
    if isinstance(user, str):
        try:
            import pwd
        except ImportError:
            raise ValueError("The user parameter cannot be a string on systems without the pwd module")
        user = pwd.getpwnam(user).pw_uid
    elif not isinstance(user, int):
        raise TypeError("User must be a string or an integer")
    if user < 0:
        raise ValueError(f"User ID cannot be negative, got {user}")
    return user


def _fork_fds(pass_fds):
    return sorted(map(int, set(pass_fds)))


class Popen:
    __class_getitem__ = classmethod(_GenericAlias)
    _handle = None
    _child_created = False

    def __init__(self, args, bufsize=-1, executable=None, stdin=None, stdout=None,
                 stderr=None, preexec_fn=None, *, close_fds=True, shell=False, cwd=None, env=None, restore_signals=True, pass_fds=(),
                 universal_newlines=None, encoding=None, errors=None, text=None, start_new_session=False,
                 umask=-1, pipesize=-1, process_group=None, startupinfo=None, creationflags=0, group=None, extra_groups=None, user=None):
        _cleanup()
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
        if pass_fds and not close_fds:
            import warnings
            warnings.warn("pass_fds overriding close_fds.", RuntimeWarning)
            close_fds = True
        if startupinfo is not None:
            raise ValueError("startupinfo is only supported on Windows platforms")
        if creationflags != 0:
            raise ValueError("creationflags is only supported on Windows platforms")
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
        self.args = args
        self.returncode = None
        self.pipesize = pipesize
        self._sigint_wait_secs = 0.25
        self.stdin = self.stdout = self.stderr = None
        gid = None if group is None else _get_group_id(group)
        gids = None if extra_groups is None else _get_extra_group_ids(extra_groups)
        uid = None if user is None else _get_user_id(user)
        prepared, fds = _process.prepare(stdin, stdout, stderr, bufsize, pipesize)
        failed = True
        try:
            self._prepared = prepared
            self._pipe_fds = fds
            self.stdin = _process.pipe(prepared, 0)
            self.stdout = _process.pipe(prepared, 1)
            self.stderr = _process.pipe(prepared, 2)
            if self.text_mode:
                if self.stdin is not None:
                    self.stdin = io.TextIOWrapper(self.stdin, encoding=self.encoding, errors=errors,
                                                 write_through=True, line_buffering=line_buffering)
                if self.stdout is not None:
                    self.stdout = io.TextIOWrapper(self.stdout, encoding=self.encoding, errors=errors)
                if self.stderr is not None:
                    self.stderr = io.TextIOWrapper(self.stderr, encoding=self.encoding, errors=errors)
            self._stdin_ref = None if self.stdin is None else _ref(self.stdin)
            self._stdout_ref = None if self.stdout is None else _ref(self.stdout)
            self._stderr_ref = None if self.stderr is None else _ref(self.stderr)
            self._replacement_output = None
            self._input = None
            self._execute_child(args, executable, preexec_fn, close_fds, pass_fds, cwd, env,
                                startupinfo, creationflags, shell, *fds, restore_signals,
                                gid, gids, uid, umask, start_new_session,
                                -1 if process_group is None else process_group)
            failed = False
        except BaseException:
            for pipe in (self.stdin, self.stdout, self.stderr):
                if pipe is not None:
                    try:
                        pipe.close()
                    except OSError:
                        pass
            raise
        finally:
            _process.release(prepared, failed)

    def _execute_child(self, args, executable, preexec_fn, close_fds, pass_fds, cwd, env,
                       startupinfo, creationflags, shell, p2cread, p2cwrite, c2pread, c2pwrite,
                       errread, errwrite, restore_signals, gid, gids, uid, umask,
                       start_new_session, process_group):
        if (p2cread, p2cwrite, c2pread, c2pwrite, errread, errwrite) != self._pipe_fds:
            raise NotImplementedError("replacing prepared subprocess descriptors is unsupported")
        slot = None
        if preexec_fn is not None or gid is not None or gids is not None or uid is not None:
            # Child setup can fail after fork. Keep its real pid and wait status.
            slot = self._handle = _process.reserve_handle()
        handle = _process.launch(self._prepared, args, executable, close_fds, restore_signals,
                                 cwd, env, shell, start_new_session, process_group, umask, pass_fds,
                                 gid, gids, uid, preexec_fn, slot, self)
        if slot is None:
            self._handle = handle
            self._child_created = True

    def __repr__(self):
        result = f"<{self.__class__.__name__}: returncode: {self.returncode} args: {self.args!r}>"
        if len(result) > 80:
            return result[:76] + "...>"
        return result

    def __del__(self, _maxsize=sys.maxsize, _warn=_warn):
        handle = self._handle
        if handle is None or not _process.started(handle):
            return
        if self.returncode is None:
            _warn("subprocess %s is still running" % self.pid, ResourceWarning, source=self)
        self._internal_poll(_deadstate=_maxsize)
        if self.returncode is None and _active is not None:
            _active.append(self)
        _process.finalized(handle)

    @property
    def universal_newlines(self):
        return self.text_mode

    @universal_newlines.setter
    def universal_newlines(self, value):
        self.text_mode = bool(value)

    @property
    def pid(self):
        return _process.pid(self._handle)

    def _internal_poll(self, _deadstate=None):
        if self.returncode is None:
            try:
                status = _process.poll(self._handle, _deadstate)
            except OSError:
                status = _deadstate
            if status is not None:
                self.returncode = status
        return self.returncode

    def poll(self):
        return self._internal_poll()

    def wait(self, timeout=None):
        endtime = None if timeout is None else _time() + timeout
        try:
            return self._wait(timeout=timeout)
        except KeyboardInterrupt:
            self._wait_after_keyboard_interrupt(endtime, timeout)
            raise

    def _remaining_time(self, endtime):
        return None if endtime is None else endtime - _time()

    def _wait(self, timeout):
        if self.returncode is not None:
            return self.returncode
        if timeout is None:
            self.returncode = _process.wait(self._handle)
            return self.returncode

        endtime = _time() + timeout
        delay = 0.0005
        while self.returncode is None:
            # Reap under the native PID lock before checking the deadline.
            status = _process.poll(self._handle)
            if status is not None:
                self.returncode = status
                break
            remaining = self._remaining_time(endtime)
            if remaining <= 0:
                raise TimeoutExpired(self.args, timeout)
            delay = min(delay * 2, remaining, 0.05)
            time.sleep(delay)
        return self.returncode

    def _wait_after_keyboard_interrupt(self, endtime, timeout):
        # The child may also have received SIGINT. Share one brief wait with
        # __exit__ and respect the caller's original deadline.
        grace = self._sigint_wait_secs
        if timeout is not None:
            grace = min(grace, self._remaining_time(endtime))
        self._sigint_wait_secs = 0
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
            self._wait_after_keyboard_interrupt(endtime, timeout)
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
        if self._replacement_output is not None or (not self._communication_started and (
                _pipe_replaced(self.stdin, self._stdin_ref)
                or _pipe_replaced(self.stdout, self._stdout_ref)
                or _pipe_replaced(self.stderr, self._stderr_ref))):
            return self._communicate_replaced_pipes(input, endtime, orig_timeout)
        if not self._communication_started and self.stdin is not None:
            # Publish earlier writes even if converting the new input raises.
            _flush_stdin(self.stdin)
        if self.stdin is not None and self._input is None:
            if input is not None and self.text_mode:
                # A false input closes stdin before encoding, even for a codec
                # that produces a BOM from the empty string.
                if not input:
                    try:
                        self.stdin.close()
                    except BrokenPipeError:
                        pass
                input = input.encode(self.stdin.encoding, self.stdin.errors)
            self._input = input

        # Retain the exporter for retries, but only pin its storage while this
        # call is using it. The native pipe loop owns the byte offset.
        input = self._input
        if not input:
            input = None
        elif isinstance(input, memoryview):
            input = input.cast("B")
        elif type(input) is not bytes:
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

    def _communicate_replaced_pipes(self, input, deadline, timeout):
        # The native handle knows only the original pipes. Select on the public
        # streams and retain their output buffers by file object across retries.
        import os
        import select

        if self._replacement_output is None:
            self._replacement_output = {stream: [] for stream in (self.stdout, self.stderr) if stream}
            self._replacement_input = b""
            self._replacement_offset = 0
            self._communication_started = True
            if self.stdin:
                _flush_stdin(self.stdin)
                if not input:
                    try:
                        self.stdin.close()
                    except BrokenPipeError:
                        pass
                if input is not None and self.text_mode:
                    input = input.encode(self.stdin.encoding, self.stdin.errors)
                if input:
                    self._replacement_input = input
        elif input:
            raise ValueError("Cannot send input after starting communication")

        output = self._replacement_output
        stdout = output[self.stdout] if self.stdout else None
        stderr = output[self.stderr] if self.stderr else None
        input_bytes = self._replacement_input
        if isinstance(input_bytes, memoryview):
            # The write offset is in bytes. A strided view cannot be cast.
            input_bytes = input_bytes.cast("B")
        elif not isinstance(input_bytes, (bytes, bytearray)):
            input_bytes = bytes(memoryview(input_bytes))
        with _PopenSelector() as selector:
            pending = 0
            if (self.stdin and not self.stdin.closed and
                    self._replacement_offset < len(input_bytes)):
                selector.register(self.stdin, selectors.EVENT_WRITE)
                pending += 1
            for stream in (self.stdout, self.stderr):
                if stream and not stream.closed:
                    selector.register(stream, selectors.EVENT_READ)
                    pending += 1
            while pending:
                remaining = None if deadline is None else deadline - _time()
                ready = selector.select(remaining) if remaining is None or remaining > 0 else []
                if deadline is not None and _time() >= deadline:
                    raise TimeoutExpired(self.args, timeout,
                                         output=b"".join(stdout) if stdout else None,
                                         stderr=b"".join(stderr) if stderr else None)
                for key, _ in ready:
                    if key.fileobj is self.stdin:
                        offset = self._replacement_offset
                        chunk = input_bytes[offset:offset + getattr(select, "PIPE_BUF", 512)]
                        try:
                            self._replacement_offset += os.write(key.fd, chunk)
                        except BrokenPipeError:
                            pass
                        else:
                            if self._replacement_offset < len(input_bytes):
                                continue
                    elif key.fileobj in (self.stdout, self.stderr):
                        data = os.read(key.fd, 32768)
                        output[key.fileobj].append(data)
                        if data:
                            continue
                    else:
                        continue
                    selector.unregister(key.fileobj)
                    key.fileobj.close()
                    pending -= 1
        try:
            self.wait(timeout=None if deadline is None else deadline - _time())
        except TimeoutExpired as exc:
            exc.timeout = timeout
            raise
        if stdout is not None:
            stdout = b"".join(stdout)
        if stderr is not None:
            stderr = b"".join(stderr)
        if self.text_mode:
            return _decode(stdout, self.stdout), _decode(stderr, self.stderr)
        return stdout, stderr

    def send_signal(self, sig):
        self.poll()
        if self.returncode is None:
            try:
                _process.signal(self._handle, sig)
            except ProcessLookupError:
                pass

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
    __class_getitem__ = classmethod(_GenericAlias)

    def __init__(self, args, returncode, stdout=None, stderr=None):
        self.args = args
        self.returncode = returncode
        self.stdout = stdout
        self.stderr = stderr

    def __repr__(self):
        fields = [f"args={self.args!r}", f"returncode={self.returncode!r}"]
        if self.stdout is not None:
            fields.append(f"stdout={self.stdout!r}")
        if self.stderr is not None:
            fields.append(f"stderr={self.stderr!r}")
        return f"{type(self).__name__}({', '.join(fields)})"

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
            raise CalledProcessError(code, process.args, output=stdout, stderr=stderr)
    return CompletedProcess(process.args, code, stdout, stderr)
