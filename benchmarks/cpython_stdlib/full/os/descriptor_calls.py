"""Entry point for upstream tests that inspect a native fd from the host process."""
import os
import sys


def invoke() -> None:
    op = sys.argv[1]
    if op == "open":
        print(os.open(sys.argv[2], int(sys.argv[3]), int(sys.argv[4])))
    elif op == "close":
        os.close(int(sys.argv[2]))
    elif op == "dup":
        print(os.dup(int(sys.argv[2])))
    elif op == "dup2":
        print(os.dup2(int(sys.argv[2]), int(sys.argv[3]), int(sys.argv[4])))
    elif op == "get_blocking":
        print(os.get_blocking(int(sys.argv[2])))
    elif op == "set_blocking":
        os.set_blocking(int(sys.argv[2]), int(sys.argv[3]))
    elif op == "get_inheritable":
        print(os.get_inheritable(int(sys.argv[2])))
    elif op == "set_inheritable":
        os.set_inheritable(int(sys.argv[2]), int(sys.argv[3]))
    else:
        raise ValueError("unknown operation")

try:
    invoke()
except OSError as error:
    code = error.errno
    reason = error.strerror
    if code is None or reason is None:
        raise ValueError("missing structured OS error")
    if isinstance(error, FileNotFoundError):
        print("FileNotFoundError")
    elif isinstance(error, PermissionError):
        print("PermissionError")
    else:
        print("OSError")
    print(code)
    print(reason)
