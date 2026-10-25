"""The persistent native side of EncodingTest.test_encoding_plain_file."""
import io
import logging
import sys


def expect(op: str) -> None:
    if input() != op:
        raise ValueError("unexpected file logging operation: " + op)


def acknowledge() -> None:
    print("ok")
    sys.stdout.flush()


def main() -> None:
    root = logging.getLogger("")
    root.setLevel(int(input()))
    capture = logging.StreamHandler(io.StringIO())
    capture.setFormatter(logging.Formatter(input()))
    root.addHandler(capture)
    expect("getLogger")
    logger = logging.getLogger(input())
    acknowledge()
    expect("FileHandler")
    filename = input()
    encoding = input()
    handler = logging.FileHandler(filename, encoding=encoding)
    acknowledge()
    expect("addHandler")
    logger.addHandler(handler)
    acknowledge()
    expect("warning")
    logger.warning(input())
    acknowledge()
    expect("removeHandler")
    logger.removeHandler(handler)
    acknowledge()
    expect("close")
    handler.close()
    acknowledge()
    root.removeHandler(capture)
    capture.close()


main()
