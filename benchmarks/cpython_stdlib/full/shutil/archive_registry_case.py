"""Keep one extractor registry alive while the original CPython callback runs."""

import ast
from contextlib import ExitStack
import os
import select
import subprocess
from unittest.mock import patch

SOURCE = '''import shutil
import sys

def reply(value: str) -> None:
    with open(sys.argv[2], "w") as outgoing:
        outgoing.write(value + "\\n")

def extract(filename: str, extract_dir: str, extra: int) -> None:
    reply("callback " + repr((filename, extract_dir, extra)))
    with open(sys.argv[1]) as incoming:
        if incoming.readline() != "ok\\n":
            raise RuntimeError("original archive callback raised")

with open(sys.argv[1]) as commands:
    while True:
        command = commands.readline().rstrip("\\n")
        if command == "quit" or command == "":
            break
        try:
            if command == "list":
                reply("value " + repr(shutil.get_unpack_formats()))
            elif command == "register":
                name = commands.readline().rstrip("\\n")
                description = commands.readline().rstrip("\\n")
                suffixes: list[str] = []
                for i in range(int(commands.readline())):
                    suffixes.append(commands.readline().rstrip("\\n"))
                count = int(commands.readline())
                if count < 0:
                    shutil.register_unpack_format(name, suffixes, extract, description=description)
                else:
                    extras: list[tuple[str, int]] = []
                    for i in range(count):
                        key = commands.readline().rstrip("\\n")
                        value = int(commands.readline())
                        extras.append((key, value))
                    shutil.register_unpack_format(name, suffixes, extract, extras, description)
                reply("value None")
            elif command == "remove":
                shutil.unregister_unpack_format(commands.readline().rstrip("\\n"))
                reply("value None")
            elif command == "unpack":
                filename = commands.readline().rstrip("\\n")
                dst = commands.readline().rstrip("\\n")
                shutil.unpack_archive(filename, dst)
                reply("value None")
            else:
                raise AssertionError("unknown registry request")
        except shutil.RegistryError as error:
            reply("RegistryError " + repr(str(error)))
        except RuntimeError as error:
            reply("RuntimeError " + repr(str(error)))
'''


def run(module, original, command):
    counts = {}
    callback = None
    callback_types = []
    with ExitStack() as stack:
        # Reopening inherited pipe descriptors works for native regular-file IO.
        # Keep commands and replies separate: a live registry spans all calls.
        incoming, parent_writer = os.pipe()
        parent_reader, outgoing = os.pipe()
        rx = stack.enter_context(os.fdopen(parent_reader, "rb", buffering=0))
        tx = stack.enter_context(os.fdopen(parent_writer, "wb", buffering=0))
        try:
            proc = subprocess.Popen([*command, f"/proc/self/fd/{incoming}", f"/proc/self/fd/{outgoing}"],
                                    pass_fds=(incoming, outgoing), stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        finally:
            os.close(incoming)
            os.close(outgoing)
        def cleanup():
            if proc.poll() is None:
                proc.kill()
            proc.communicate()
        stack.callback(cleanup)

        def send(*fields):
            lines = [str(field) for field in fields]
            if any("\n" in line for line in lines):
                raise ValueError("the line transport cannot represent embedded newlines")
            tx.write(("\n".join(lines) + "\n").encode())

        def request(operation, op, *fields):
            counts[operation] = counts.get(operation, 0) + 1
            send(op, *fields)
            raised = None
            while True:
                if not select.select([rx], [], [], 30)[0]:
                    raise TimeoutError(f"archive registry stopped responding during {operation}")
                line = rx.readline()
                if not line:
                    stdout, stderr = proc.communicate(timeout=10)
                    raise AssertionError((proc.returncode, stdout, stderr))
                event, data = line.decode().rstrip("\n").split(" ", 1)
                value = ast.literal_eval(data)
                if event == "callback":
                    filename, dst, extra = value
                    callback_types.append([type(v).__name__ for v in value])
                    try:
                        callback(filename, dst, extra=extra)
                    except BaseException as error:
                        raised = error
                        send("raised")
                    else:
                        send("ok")
                elif event == "RuntimeError" and raised is not None:
                    assert value == "original archive callback raised"
                    raise raised
                elif event == "RegistryError":
                    raise module.RegistryError(value)
                elif event == "value" and raised is None:
                    return value
                else:
                    raise AssertionError((event, value, raised))

        def register(name, extensions, function, extra_args=None, description=""):
            nonlocal callback
            # The unchanged test uses one closure across both registrations.
            assert callback is None or callback is function
            callback = function
            fields = [name, description, len(extensions), *extensions]
            fields.append(-1 if extra_args is None else len(extra_args))
            for key, value in extra_args or ():
                assert isinstance(key, str) and type(value) is int
                fields.extend((key, value))
            return request("register_unpack_format", "register", *fields)

        stack.enter_context(patch.object(module, "get_unpack_formats",
                                         lambda: request("get_unpack_formats", "list")))
        stack.enter_context(patch.object(module, "register_unpack_format", register))
        stack.enter_context(patch.object(module, "unregister_unpack_format",
                                         lambda name: request("unregister_unpack_format", "remove", name)))
        stack.enter_context(patch.object(module, "unpack_archive",
                                         lambda filename, dst: request("unpack_archive", "unpack", filename, dst)))
        original()
        send("quit")
        stdout, stderr = proc.communicate(timeout=30)
        assert (proc.returncode, stdout, stderr) == (0, b"", b""), (proc.returncode, stdout, stderr)
    return counts, callback_types
