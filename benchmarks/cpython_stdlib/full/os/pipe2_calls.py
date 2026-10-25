"""Calls made in the unchanged POSIX test process's descriptor table."""
import os
import sys


def invoke() -> None:
    op = sys.argv[1]
    if op == "close":
        os.close(int(sys.argv[2]))
    elif op == "get_inheritable":
        print(os.get_inheritable(int(sys.argv[2])))
    elif op == "get_blocking":
        print(os.get_blocking(int(sys.argv[2])))
    else:
        if op == "pipe2_str":
            pair = os.pipe2(sys.argv[2])
        elif op == "pipe2_two":
            pair = os.pipe2(int(sys.argv[2]), int(sys.argv[3]))
        elif op == "pipe2_int":
            pair = os.pipe2(int(sys.argv[2]))
        else:
            raise ValueError("unknown descriptor operation")
        reader, writer = pair
        print(reader)
        print(writer)


try:
    invoke()
except TypeError as error:
    print("TypeError")
    print(str(error))
except OverflowError as error:
    print("OverflowError")
    print(str(error))
except OSError as error:
    code = error.errno
    reason = error.strerror
    if code is None or reason is None:
        raise ValueError("native OSError has no errno")
    print("OSError")
    print(code)
    print(reason)
