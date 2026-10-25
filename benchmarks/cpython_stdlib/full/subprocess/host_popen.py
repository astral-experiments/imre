"""Run original unittest methods against a persistent, compiled Imre Popen parent."""

import argparse
import ast
from collections import Counter
from functools import cache
import fcntl
import hashlib
import json
import os
from pathlib import Path
import select
import subprocess
import sys
import tempfile
import time
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[1]))
from suite_inventory import load_suite, verify_files, verify_interpreter_tests
from suite_native import run_in_fixtures
PIPE, STDOUT, DEVNULL = subprocess.PIPE, subprocess.STDOUT, subprocess.DEVNULL
# Minimum observed native operations for the assertions in each original.
# Repetitions matter: a success after a timeout must use the same child.
METHODS = {
    "ContextManagerTests": {
        "test_broken_pipe_cleanup": "stdin.write returncode exit:OSError stdin.closed",
        "test_communicate_stdin": "communicate returncode exit",
        "test_invalid_args": "Popen:FileNotFoundError",
        "test_pipe": "stdout.read stderr.read exit stdout.closed stderr.closed",
        "test_returncode": "exit returncode",
    },
    "POSIXProcessTestCase": {
        "test_communicate_repeated_call_after_stdout_close": "communicate:timeout communicate",
        "test_kill": "stdout.read_size kill communicate wait",
        "test_kill_dead": "stdout.read_size kill communicate",
        "test_shell_sequence": "Popen stdout.read exit",
        "test_shell_string": "Popen stdout.read exit",
        "test_args_string": "Popen wait returncode",
        "test_run_abort": "wait returncode",
        "test_terminate": "stdout.read_size terminate communicate wait",
        "test_terminate_dead": "stdout.read_size terminate communicate",
    },
    "ProcessTestCase": {
        "test_invalid_cmd": "Popen:ValueError Popen:ValueError",
        "test_executable": "Popen wait returncode",
        "test_bytes_executable": "Popen wait returncode",
        "test_executable_takes_precedence": "Popen Popen:FileNotFoundError wait returncode",
        "test_executable_replaces_shell": "Popen wait returncode",
        "test_bytes_executable_replaces_shell": "Popen wait returncode",
        "test_cwd_with_relative_executable": "Popen stdout.read wait returncode",
        "test_executable_with_cwd": "Popen stdout.read wait returncode",
        "test_executable_without_cwd": "Popen stdout.read wait returncode",
        "test_communicate": "communicate stdout.close stderr.close stdin.close",
        "test_communicate_epipe": "communicate",
        "test_communicate_epipe_only_stdin": "communicate wait",
        "test_communicate_pipe_buf": "communicate",
        "test_communicate_pipe_fd_leak": "communicate stdin.closed stdout.closed stderr.closed",
        "test_communicate_returns": "communicate",
        "test_communicate_stderr": "communicate",
        "test_communicate_stdin": "communicate returncode",
        "test_communicate_stdin_closed_before_call": "stdin.close communicate returncode exit",
        "test_communicate_stdout": "communicate",
        "test_communicate_timeout_large_output": "communicate:timeout communicate",
        "test_cwd": "stdout.read wait returncode",
        "test_cwd_with_absolute_arg": "Popen:FileNotFoundError stdout.read wait returncode",
        "test_cwd_with_relative_arg": "Popen:FileNotFoundError Popen:FileNotFoundError stdout.read wait returncode",
        "test_post_timeout_communicate_sends_input": "communicate:timeout communicate exit returncode",
        "test_stderr_devnull": "streams wait",
        "test_stderr_none": "streams wait",
        "test_stderr_pipe": "stderr.read exit",
        "test_stdin_devnull": "streams wait",
        "test_stdin_none": "streams wait",
        "test_stdin_pipe": "stdin.write stdin.close wait returncode",
        "test_stdout_devnull": "streams wait",
        "test_stdout_pipe": "stdout.read exit",
        "test_stdout_stderr_pipe": "stdout.read exit",
        "test_wait": "wait wait",
        "test_wait_timeout": "wait:timeout wait",
        "test_writes_before_communicate": "stdin.write communicate",
    },
}


class Unsupported(Exception):
    """A transport limitation must not satisfy upstream's expected API exceptions."""


def source(options, layout, *, select_backend=False):
    keywords = dict(options)
    if layout == "list[str]":
        command = "argv: list[str] = []\nfor argument in sys.argv[4:]:\n    argv.append(Path(argument).read_bytes().decode())"
    else:
        kind, items = layout
        values = []
        for position, member in enumerate(items, 4):
            raw = f"Path(sys.argv[{position}]).read_bytes()"
            values.append(raw if member == "bytes" else f"Path({raw}.decode())" if member == "path" else f"{raw}.decode()")
        value = ("(" + ", ".join(values) + ("," if len(values) == 1 else "") + ")" if kind == "tuple" else
                 "[" + ", ".join(values) + "]" if kind == "list" else values[0])
        command = "argv = " + value
    if keywords.get("env") == "child_env":
        command += """
child_env: dict[str, str] = {}
for i in range(int((payload.parent / "env-count").read_text())):
    child_env[(payload.parent / ("key-" + str(i))).read_bytes().decode()] = (payload.parent / ("val-" + str(i))).read_bytes().decode()
"""
    text = keywords.get("text") == "True" or keywords.get("encoding", "None") != "None"
    opts = "".join(f", {key}={value}" for key, value in options)
    lines = []
    def operation(name, expr):
        lines.extend([f'elif cmd == {name!r}:', f'    send(reply, "ok " + repr({expr}))'])
    for name, expr in {"streams": "(p.stdin is None, p.stdout is None, p.stderr is None)", "pid": "p.pid", "returncode": "p.returncode", "poll": "p.poll()", "wait": "p.wait()",
                       "kill": "p.kill()", "terminate": "p.terminate()"}.items():
        operation(name, expr)
    for index, stream in enumerate(("stdin", "stdout", "stderr")):
        if keywords.get(stream) != "subprocess.PIPE":
            continue
        for method in ("close", "flush", "fileno"):
            operation(stream + "." + method, f"p.{stream}.{method}()")
        operation(stream + ".closed", f"p.{stream}.closed")
        if index:
            operation(stream + ".read", f"p.{stream}.read()")
        if not text and index:
            operation(stream + ".read_size", f"p.{stream}.read(int(payload.read_text()))")
        if not index:
            operation("stdin.write", f"p.stdin.write(payload.read_bytes(){'.decode()' if text else ''})")
    operation("communicate", "p.communicate()")
    operation("communicate_input", f"p.communicate(payload.read_bytes(){'.decode()' if text else ''})")
    # The transport preserves the integer versus float spelling of TimeoutExpired.
    # Imre currently accepts integer literal deadlines and nullable float values.
    for seconds in (0, 15, 30, 60):
        operation("communicate_input_" + str(seconds), f"p.communicate(payload.read_bytes(){'.decode()' if text else ''}, {seconds})")
        operation("communicate_" + str(seconds), f"p.communicate(timeout={seconds})")
        operation("wait_" + str(seconds), f"p.wait(timeout={seconds})")
    operation("wait_float", "p.wait(timeout=float(payload.read_text()))")
    operation("communicate_float", "p.communicate(timeout=float(payload.read_text()))")
    lines.extend(['elif cmd == "exit":', '    with p:', '        pass', '    send(reply, "ok None")',
                  'else:', '    raise ValueError("unknown transport command")'])
    operations = "\n".join("                    " + line for line in lines)
    selector_import = "import selectors\n" if select_backend else ""
    binding = "subprocess._PopenSelector = selectors.SelectSelector\n" if select_backend else ""
    return f"""from pathlib import Path
from typing import TextIO
import subprocess
import sys
{selector_import}
def send(reply: TextIO, message: str) -> None:
    reply.write(message + "\\n")
    reply.flush()

{binding}payload = Path(sys.argv[3])
{command}
with open(sys.argv[1]) as channel, open(sys.argv[2], "w") as reply:
    try:
        p = subprocess.Popen(argv{opts})
        send(reply, "ok " + repr(p.pid))
        try:
            while True:
                cmd = channel.readline().strip()
                if not cmd:
                    break
                try:
                    if cmd == "stop":
                        break
{operations}
                except subprocess.TimeoutExpired as error:
                    send(reply, "timeout " + repr((str(error), error.output, error.stderr)))
                except ValueError as error:
                    send(reply, "ValueError " + repr(str(error)))
                except OSError as error:
                    send(reply, "OSError " + repr(str(error)))
        finally:
            if p.poll() is None:
                p.kill()
            p.wait()
    except ValueError as error:
        send(reply, "ValueError " + repr(str(error)))
    except FileNotFoundError as error:
        send(reply, "FileNotFoundError " + repr(str(error)))
    except OSError as error:
        send(reply, "OSError " + repr(str(error)))
"""


class NativeTimeout(subprocess.TimeoutExpired):
    def __str__(self):
        return self.native_message


class Stream:
    def __init__(self, parent, name):
        self.parent, self.name = parent, name

    def read(self, size=-1):
        return self.parent.request(self.name + (".read" if size == -1 else ".read_size"),
                                   None if size == -1 else str(size).encode())

    def write(self, data):
        return self.parent.request(self.name + ".write", self.parent.payload(data))

    def flush(self):
        return self.parent.request(self.name + ".flush")

    def close(self):
        return self.parent.request(self.name + ".close")

    @property
    def closed(self):
        return self.parent.request(self.name + ".closed")


class Remote:
    def __init__(self, harness, argv, options):
        self.harness, self.argv = harness, argv
        names = {"stdout", "stderr", "stdin", "text", "encoding", "close_fds", "cwd", "bufsize", "pipesize", "shell", "executable", "env"}
        elements = argv if type(argv) in (list, tuple) else (argv,)
        members = tuple("str" if type(v) is str else "bytes" if type(v) is bytes else "path" if isinstance(v, Path) else None for v in elements)
        if None in members:
            raise Unsupported("Popen representation for this custom argument is not available")
        layout = "list[str]" if type(argv) is list and all(m=="str" for m in members) else (
            "tuple" if type(argv) is tuple else "list" if type(argv) is list else "scalar", members)
        if unknown := set(options) - names:
            raise Unsupported("Popen options: " + ", ".join(sorted(unknown)))
        keywords = {}
        for key, val in options.items():
            if key in ("stdin", "stdout", "stderr"):
                if val not in (None, PIPE, DEVNULL, STDOUT):
                    raise Unsupported("stream transport does not admit external descriptors yet")
                keywords[key] = {None: "None", PIPE: "subprocess.PIPE", DEVNULL: "subprocess.DEVNULL", STDOUT: "subprocess.STDOUT"}[val]
            elif key == "env" and val is not None:
                if type(val) is not dict or any(type(k) is not str or type(v) is not str for k,v in val.items()):
                    raise Unsupported("environment representation requires a dict[str, str]")
                keywords[key] = "child_env"
            elif key in ("cwd", "executable") and val is not None:
                if type(val) is str:
                    keywords[key] = f'(payload.parent / "{key}").read_bytes().decode()'
                elif type(val) is bytes:
                    keywords[key] = f'(payload.parent / "{key}").read_bytes()'
                elif isinstance(val, Path):
                    keywords[key] = f'Path((payload.parent / "{key}").read_bytes().decode())'
                else:
                    raise Unsupported("custom path-like protocol needs a native __fspath__ invocation")
            else:
                if val is not None and type(val) not in (str,int,bool):
                    raise Unsupported("unsupported option representation: " + key)
                keywords[key] = repr(val)
        self.text = bool(options.get("text") or options.get("encoding"))
        binary = harness.actor(tuple(sorted(keywords.items())), layout)
        self.folder = tempfile.TemporaryDirectory(dir=harness.work)
        self.file = Path(self.folder.name, "payload")
        # The host exec API rejects NUL before a native parent could inspect it.
        # Pass original argument bytes through files; the native launch validates them.
        arguments = []
        for key in ("cwd", "executable"):
            if options.get(key) is not None:
                (self.file.parent / key).write_bytes(os.fsencode(options[key]))
        if options.get("env") is not None:
            env = options["env"]
            (self.file.parent / "env-count").write_text(str(len(env)))
            for i, (key, value) in enumerate(env.items()):
                (self.file.parent / f"key-{i}").write_bytes(key.encode())
                (self.file.parent / f"val-{i}").write_bytes(value.encode())
        for position, argument in enumerate(elements):
            path = Path(self.folder.name, str(position))
            path.write_bytes(os.fsencode(argument))
            arguments.append(path)
        control, cmd = os.pipe()
        reply, response = os.pipe()
        try:
            self.actor = subprocess.Popen([binary, f"/dev/fd/{control}", f"/dev/fd/{response}", self.file, *arguments],
                                          pass_fds=(control, response))
        finally:
            os.close(control)
            os.close(response)
        self.cmd = os.fdopen(cmd, "w")
        self.reply = os.fdopen(reply, "rb", buffering=0)
        self.buffer = bytearray()
        harness.parents.append(self)
        try:
            self.pid = self.receive("Popen", None)
        except BaseException:
            harness.parents.remove(self)
            self.finish()
            raise
        for name, absent in zip(("stdin", "stdout", "stderr"), self.request("streams"), strict=True):
            setattr(self, name, None if absent else Stream(self, name))

    def payload(self, value):
        if type(value) is (str if self.text else bytes):
            return value.encode() if self.text else value
        raise Unsupported("unsupported communication input representation")

    def receive(self, name, timeout):
        deadline = time.monotonic() + 45
        while b"\n" not in self.buffer:
            remaining = deadline - time.monotonic()
            if remaining <= 0 or not select.select([self.reply], [], [], remaining)[0]:
                raise RuntimeError(f"native parent hung during {name}")
            data = os.read(self.reply.fileno(), 65536)
            if not data:
                raise RuntimeError(f"native parent exited during {name}: {self.actor.poll()}")
            self.buffer.extend(data)
        line, _, self.buffer = self.buffer.partition(b"\n")
        status, raw = line.decode().split(" ", 1)
        value = ast.literal_eval(raw)
        self.harness.calls.append(name if status == "ok" else name + ":" + status)
        if status == "ok":
            return value
        if status == "timeout":
            message, out, err = value
            exc = NativeTimeout(self.argv, timeout, output=out, stderr=err)
            exc.native_message = message
            raise exc
        raise {"ValueError": ValueError, "FileNotFoundError": FileNotFoundError, "OSError": OSError}[status](value)

    def request(self, name, data=None, timeout=None):
        cmd = name
        if timeout is not None:
            if type(timeout) is int and timeout in (0, 15, 30, 60):
                cmd += "_" + str(timeout)
            elif name in ("communicate", "wait"):
                # Other integer timeouts are not converted on an exceptional path.
                if type(timeout) is not float:
                    raise Unsupported(f"unsupported integer timeout: {timeout}")
                data, cmd = repr(timeout).encode(), cmd + "_float"
            else:
                raise Unsupported("combined nonliteral timeout and communication input")
        if data is not None:
            self.file.write_bytes(data)
        self.cmd.write(cmd + "\n")
        self.cmd.flush()
        return self.receive(name.removesuffix("_input"), timeout)

    def wait(self, timeout=None):
        return self.request("wait", timeout=timeout)

    def poll(self):
        return self.request("poll")

    def kill(self):
        return self.request("kill")

    def terminate(self):
        return self.request("terminate")

    @property
    def returncode(self):
        return self.request("returncode")

    def communicate(self, input=None, timeout=None):
        return self.request("communicate" if input is None else "communicate_input",
                            None if input is None else self.payload(input), timeout)

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        self.request("exit")

    def finish(self):
        self.cmd.close()
        self.reply.close()
        try:
            if self.actor.wait(timeout=5):
                raise RuntimeError(f"native parent exited with {self.actor.returncode}")
        finally:
            if self.actor.poll() is None:
                self.actor.kill()
                self.actor.wait()
            self.folder.cleanup()


class Harness:
    PIPE, STDOUT, DEVNULL = PIPE, STDOUT, DEVNULL
    TimeoutExpired = subprocess.TimeoutExpired

    def __init__(self, args, work):
        self.args, self.work = args, work
        self.parents, self.calls, self.mixed = [], [], []

    @cache
    def actor(self, keywords, layout, *, select_backend=False):
        code = source(keywords, layout, select_backend=select_backend)
        # Workers share only complete artifacts within this invocation and opt level.
        directory = Path(getattr(self.args, "actors", None) or self.work)
        stem = hashlib.sha256(code.encode()).hexdigest()[:24]
        entry, binary = directory / (stem + ".py"), directory / stem
        with (directory / (stem + ".lock")).open("w") as lock:
            fcntl.flock(lock, fcntl.LOCK_EX)
            if not binary.exists():
                entry.write_text(code)
                compile_result = subprocess.run([self.args.imre, "build", entry, "--runtime", self.args.runtime,
                                                 "--clang", self.args.clang, "--opt-level", self.args.opt_level,
                                                 "-o", binary], capture_output=True, text=True, timeout=120)
                if compile_result.returncode:
                    raise Unsupported("native compilation: " + compile_result.stderr.splitlines()[-1])
        return binary

    def Popen(self, *pos, **options):
        if len(pos) == 1 and "args" not in options:
            argv = pos[0]
        elif not pos and "args" in options:
            argv = options.pop("args")
        else:
            raise Unsupported("Popen positional binding")
        if isinstance(argv, (tuple, list)) and any(isinstance(arg, str) and
                ("subprocess" in arg or "sigchild_ignore.py" in arg)
                for arg in argv):
            self.mixed.append("child program exercises subprocess")
        return Remote(self, argv, options)

    def __getattr__(self, name):
        raise Unsupported("subprocess." + name)


def verify():
    manifest = json.loads((HERE / "UPSTREAM.json").read_text())
    if sys.version_info[:3] != (3, 14, 5):
        raise RuntimeError("use the pinned CPython 3.14.5 build")
    verify_files(HERE / "upstream", manifest["files"])
    verify_interpreter_tests(manifest)
    return manifest


def one(args, name, work):
    from test import support
    support.use_resources = ["cpu"]
    from test import test_subprocess as suite
    harness = Harness(args, work)
    in_method = False
    def audit(event, data):
        if in_method and event == "subprocess.Popen":
            program = os.fsdecode(data[0])
            if not str(program).startswith(str(args.actors or work)) and program != str(args.imre):
                harness.mixed.append("host launched another command during the method")
    # This hook lives for one isolated test worker. No argument or environment
    # contents are persisted, even if the upstream test launches its own child.
    sys.addaudithook(audit)

    def execute(case, method):
        nonlocal in_method
        in_method = True
        try:
            with patch.object(suite, "subprocess", harness):
                method()
        finally:
            in_method = False
        return {"operation_calls": harness.calls, "mixed": harness.mixed}

    try:
        evidence, failures, skips = run_in_fixtures(load_suite("test.test_subprocess"), {name: execute})
        if failures:
            message = failures[0][1].strip().splitlines()[-1]
            return {"status": "blocked" if "Unsupported:" in message or "AttributeError:" in message else "failed",
                    "reason": message, "operation_calls": harness.calls}
        if skips:
            return {"status": "blocked", "reason": "worker fixture: " + skips[0][1]}
        if not evidence.get(name):
            return {"status": "failed", "reason": "original method not executed"}
        if not harness.calls:
            return {"status": "reference-only", "reason": "no native subprocess operation was observed"}
        if harness.mixed:
            return {"status": "mixed", "reason": "; ".join(sorted(set(harness.mixed))), "operation_calls": harness.calls}
        method = name.rsplit(".", 1)[1]
        if method in ("test_double_close_on_error", "test_small_errpipe_write_fd", "test_leaking_fds_on_error"):
            return {"status": "blocked", "reason": {
                "test_double_close_on_error": "the race opens descriptors on the host rather than in the native launcher's descriptor table",
                "test_small_errpipe_write_fd": "the control pipes occupy descriptors that the upstream fixture closed",
                "test_leaking_fds_on_error": "the 1024 failure attempts use separate native parents; descriptor exhaustion must be observed in one parent",
            }[method], "operation_calls": harness.calls}
        if ".ProcessTestCaseNoPoll." in name:
            return {"status": "blocked", "reason": "the fixture selects SelectSelector on the host; the native parent still uses poll",
                    "operation_calls": harness.calls}
        cls = name.rsplit(".", 2)[1]
        required = METHODS.get(cls, {}).get(method)
        if not required or not Counter(required.split()) <= Counter(harness.calls):
            return {"status": "blocked", "reason": "native trace did not verify the operations asserted by this ID",
                    "operation_calls": harness.calls}
        return {"status": "host-driven-native", "operation_calls": harness.calls}
    finally:
        for parent in harness.parents:
            parent.finish()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("imre", "runtime", "clang", "result"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--opt-level", choices=["0", "3"], required=True)
    parser.add_argument("--case")
    parser.add_argument("--actors", type=Path)
    args = parser.parse_args()
    verify()
    if args.case:
        with tempfile.TemporaryDirectory() as work:
            result = one(args, args.case, work)
        args.result.write_text(json.dumps(result) + "\n")
        return
    traces = {}
    with tempfile.TemporaryDirectory() as work:
        args.actors = Path(work)
        for cls, methods in METHODS.items():
            for method, required in methods.items():
                name = f"test.test_subprocess.{cls}.{method}"
                result = one(args, name, work)
                if result["status"] != "host-driven-native":
                    raise AssertionError((name, required, result))
                traces[name] = result["operation_calls"]
    args.result.write_text(json.dumps({"opt_level": args.opt_level, "host_driven_native": traces}, indent=2) + "\n")


if __name__ == "__main__":
    main()
