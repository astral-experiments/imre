"""Fail one write or flush on /dev/full, restore the descriptor, and retry."""

import builtins
import os
import sys


def main() -> None:
    mode = sys.argv[1]
    fd = 1 if mode.startswith("stdout") else 2
    saved = os.dup(fd)
    full = os.open("/dev/full", os.O_WRONLY)
    os.dup2(full, fd)
    os.close(full)
    message = ""
    try:
        if mode == "stdout-write":
            # Bypass both subjects' pipe buffers so the write reaches /dev/full.
            sys.stdout.write("x" * (256 * 1024))
        elif mode == "stderr-write":
            sys.stderr.write("line\n")
        elif mode == "stdout-flush":
            sys.stdout.write("buffered")
            sys.stdout.flush()
        elif mode == "stderr-flush":
            sys.stderr.write("buffered")
            sys.stderr.flush()
        elif mode == "stdout-displayhook":
            builtins._ = 42
            sys.__displayhook__("x" * (256 * 1024))
        else:
            raise ValueError("unknown stream operation")
    except OSError as error:
        number = error.errno
        reason = error.strerror
        if number is None or reason is None:
            raise AssertionError("stream error lost its errno or strerror")
        message = f"{number}: {reason}; {str(error)}"
    finally:
        os.dup2(saved, fd)
        os.close(saved)
    if not message:
        raise AssertionError("expected the stream operation to fail")
    if mode == "stdout-displayhook" and builtins._ is not None:
        raise AssertionError("a failed display left a stale value in builtins._")
    sys.stdout.flush()
    sys.stderr.flush()
    print(message)


main()
