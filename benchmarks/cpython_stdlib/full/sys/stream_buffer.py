"""Observe which write reaches a failing descriptor and what a later flush retains."""

import os
import sys


def main() -> None:
    fd = int(sys.argv[1])
    saved = os.dup(fd)
    full = os.open("/dev/full", os.O_WRONLY)
    os.dup2(full, fd)
    os.close(full)
    stage = "flush"
    failed = "none"
    try:
        for i in range(2, len(sys.argv)):
            stage = "write " + str(i - 1)
            word = sys.argv[i]
            if word == "LF":
                text = "\n"
            elif word == "CR":
                text = "\r"
            elif word.startswith("u"):
                text = "é" * int(word[1:])
            else:
                text = "x" * int(word)
            if fd == 1:
                sys.stdout.write(text)
            else:
                sys.stderr.write(text)
        stage = "flush"
        if fd == 1:
            sys.stdout.flush()
        else:
            sys.stderr.flush()
    except OSError:
        failed = stage
    finally:
        os.dup2(saved, fd)
        os.close(saved)
    if fd == 1:
        sys.stdout.flush()
        sys.stderr.write(failed + "\n")
    else:
        sys.stderr.flush()
        print(failed)


main()
