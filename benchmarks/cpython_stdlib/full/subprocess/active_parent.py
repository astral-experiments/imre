"""Compiled Popen parent for the two original POSIX finalizer methods."""

import fixture_active_os
import subprocess
import sys
import warnings
from typing import BinaryIO


def hold(argv: list[str]) -> tuple[int, BinaryIO, BinaryIO]:
    process = subprocess.Popen(argv, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    print(process.pid)
    print(process.stdout.fileno())
    print(process.stderr.fileno())
    command = input()
    while command != "drop":
        assert command == "identity"
        print(id(process))
        command = input()
    warnings.simplefilter(input(), ResourceWarning)
    return process.pid, process.stdout, process.stderr


def report_active() -> None:
    print(len(subprocess._active))
    for child in subprocess._active:
        print(id(child))
        print(child.pid)


def wait_active(identity: int) -> int:
    for child in subprocess._active:
        if id(child) == identity:
            return child.wait()
    raise RuntimeError("fixture lost its active child")


pid, out, err = hold(sys.argv[1:])
print("dropped")
command = input()
while command != "quit":
    if command == "active":
        report_active()
    elif command == "wait-active":
        print(wait_active(int(input())))
    elif command == "cleanup":
        subprocess._cleanup()
        print("cleaned")
    elif command == "kill":
        print(fixture_active_os.send_signal(pid, int(input())))
    elif command == "waitpid":
        print(fixture_active_os.wait_status(pid, int(input())))
    elif command == "missing":
        try:
            unexpected = subprocess.Popen(
                (input(),), stdout=subprocess.PIPE, stderr=subprocess.PIPE
            )
        except OSError as error:
            number = error.errno
            name = error.filename
            if number is None or not isinstance(name, str):
                raise AssertionError("launch error lost its errno or original filename")
            print(number)
            print(name)
        else:
            unexpected.kill()
            unexpected.wait()
            raise AssertionError("fixture's nonexistent executable ran")
    elif command == "close-out":
        out.close()
        print("closed")
    elif command == "close-err":
        err.close()
        print("closed")
    else:
        raise RuntimeError("unknown fixture command")
    command = input()
