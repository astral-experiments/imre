"""Run the original packing callbacks with the cwd and arguments supplied by Imre."""
import ast
from contextlib import ExitStack
import os
import tempfile
from unittest.mock import patch

from archive_pipe import session

METHODS = {
    "test_register_archive_format", "test_make_archive_cwd_default",
    "test_make_archive_cwd", "test_make_archive_cwd_supports_root_dir",
}

SOURCE = r'''import os
import shutil
import sys

def reply(line: str) -> None:
    with open(sys.argv[2], "w") as response:
        response.write(line + "\n")

def nullable(value: str | None) -> str:
    if value is None:
        return "None"
    return repr(value)

def relay(base: str, directory: str, *, dry_run: int, logger: None, owner: str | None, group: str | None, root_dir: str = "\x00absent") -> str:
    reply("callback")
    reply(repr(os.getcwd()))
    reply(repr(base))
    reply(repr(directory))
    reply(repr(dry_run))
    reply(repr(logger))
    reply(nullable(owner))
    reply(nullable(group))
    reply(repr(root_dir))
    with open(sys.argv[1]) as answer:
        if answer.readline() != "returned\n":
            raise RuntimeError()
        return answer.readline().rstrip("\n")

with open(sys.argv[1]) as commands:
    while True:
        command = commands.readline().rstrip("\n")
        if command == "" or command == "quit":
            break
        try:
            if command == "list":
                reply("value " + repr(shutil.get_archive_formats()))
            elif command == "remove":
                shutil.unregister_archive_format(commands.readline().rstrip("\n"))
                reply("value None")
            elif command == "register":
                name = commands.readline().rstrip("\n")
                description = commands.readline().rstrip("\n")
                kind = commands.readline().rstrip("\n")
                if kind == "integer callback":
                    shutil.register_archive_format(name, int(commands.readline()), description=description)
                else:
                    relay.supports_root_dir = commands.readline() == "1\n"
                    if kind == "integer extras":
                        shutil.register_archive_format(name, relay, int(commands.readline()), description)
                    elif kind == "rows":
                        rows: list[list[int]] = []
                        for i in range(int(commands.readline())):
                            row: list[int] = []
                            for j in range(int(commands.readline())):
                                row.append(int(commands.readline()))
                            rows.append(row)
                        shutil.register_archive_format(name, relay, rows, description)
                    else:
                        shutil.register_archive_format(name, relay, description=description)
                reply("value None")
            elif command == "make":
                base = commands.readline().rstrip("\n")
                format = commands.readline().rstrip("\n")
                if commands.readline() == "root\n":
                    result = shutil.make_archive(base, format, root_dir=commands.readline().rstrip("\n"))
                else:
                    result = shutil.make_archive(base, format)
                reply("value " + repr(result))
            else:
                raise AssertionError("unknown archive fixture command")
        except RuntimeError as error:
            reply("RuntimeError " + repr(str(error)))
        except TypeError as error:
            reply("TypeError " + repr(str(error)))
        except KeyError as error:
            reply("KeyError " + repr(str(error)))
'''


def run(module, original, command, guard):
    cwd, real_chdir = os.getcwd(), os.chdir
    callback = None
    counts, errors, visits, changes = {}, [], [], []
    with ExitStack() as stack:
        trace = stack.enter_context(tempfile.TemporaryFile())
        env = dict(os.environ, LD_PRELOAD=str(guard), IMRE_SHUTIL_TRACE_CHDIR="1",
                   IMRE_SHUTIL_TEST_CHDIR_FD=str(trace.fileno()))
        send, receive = stack.enter_context(session(command, env=env, pass_fds=(trace.fileno(),)))
        offset = 0

        def deliver_chdir():
            # Only actual intercepted native chdir calls reach the original hook.
            # pread leaves the child's write offset alone. The original swap_attr
            # really enters the same cwd before its original callback is invoked.
            nonlocal offset
            data = os.pread(trace.fileno(), os.fstat(trace.fileno()).st_size - offset, offset)
            if data:
                assert data.endswith(b"\0"), data
                offset += len(data)
                for raw in data[:-1].split(b"\0"):
                    changes.append(os.fsdecode(raw))
                    module.os.chdir(os.fsdecode(raw))

        def request(operation, op, *fields):
            counts[operation] = counts.get(operation, 0) + 1
            send(op, *fields)
            raised = None
            while True:
                event = receive()
                try:
                    deliver_chdir()
                except BaseException as error:
                    raised = error
                if event == "callback":
                    child_cwd, base, directory, dry, logger, owner, group, root = [
                        ast.literal_eval(receive()) for _ in range(8)]
                    kwargs = dict(dry_run=dry, logger=logger, owner=owner, group=group)
                    if root != "\0absent":
                        kwargs["root_dir"] = root
                    visits.append({"keywords": sorted(kwargs), "in_original_cwd": child_cwd == cwd})
                    try:
                        if raised is not None:
                            raise raised
                        assert child_cwd == os.getcwd(), (child_cwd, os.getcwd())
                        result = callback(base, directory, **kwargs)
                    except BaseException as error:
                        raised = error
                        send("raised")
                    else:
                        assert isinstance(result, str)
                        send("returned", result)
                    continue
                kind, data = event.split(" ", 1)
                value = ast.literal_eval(data)
                if raised is not None:
                    # The three original closures throw RuntimeError() with no args.
                    if isinstance(raised, RuntimeError):
                        assert kind == "RuntimeError" and value == str(raised)
                    raise raised
                if kind == "value":
                    return value
                errors.append((kind, value))
                raise {"RuntimeError": RuntimeError, "TypeError": TypeError, "KeyError": KeyError}[kind](value)

        def register(name, function, extra_args=None, description=""):
            nonlocal callback
            if type(function) is int:
                return request("register_archive_format", "register", name, description, "integer callback", function)
            assert callable(function)
            flag = int(getattr(function, "supports_root_dir", False))
            if type(extra_args) is int:
                fields = ["integer extras", flag, extra_args]
            elif extra_args is None:
                fields = ["default", flag]
            else:
                fields = ["rows", flag, len(extra_args)]
                for row in extra_args:
                    assert isinstance(row, (list, tuple)) and all(type(item) is int for item in row)
                    fields.extend([len(row), *row])
            result = request("register_archive_format", "register", name, description, *fields)
            callback = function
            return result

        def make(base_name, format, root_dir=None):
            return request("make_archive", "make", base_name, format,
                           *(("none",) if root_dir is None else ("root", root_dir)))

        for name, hook in (
            ("register_archive_format", register), ("make_archive", make),
            ("get_archive_formats", lambda: request("get_archive_formats", "list")),
            ("unregister_archive_format", lambda name: request("unregister_archive_format", "remove", name)),
        ):
            stack.enter_context(patch.object(module, name, hook))
        try:
            original()
            assert os.getcwd() == cwd and offset == os.fstat(trace.fileno()).st_size
        finally:
            if os.getcwd() != cwd:
                real_chdir(cwd)
    return dict(calls_by_operation=counts, callback_visits=visits, native_chdir_calls=len(changes),
                native_registration_errors=errors)
