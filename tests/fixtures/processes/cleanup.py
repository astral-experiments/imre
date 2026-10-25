import subprocess
from errno import EMFILE
from pathlib import Path
from sys import argv
from typing import BinaryIO


def descriptors() -> int:
    return sum(1 for path in Path("/dev/fd").iterdir())


def check_pipe_exhaustion() -> None:
    held: list[BinaryIO] = []
    while True:
        try:
            held.append(open("sentinel.txt", "rb"))
        except OSError as error:
            assert error.errno == EMFILE
            break

    # Three pipes need six descriptors. Leave five available so setup fails
    # after the first two pipes, before the parent can validate bufsize.
    for _ in range(5):
        held.pop().close()
    before = descriptors()
    for _ in range(3):
        try:
            subprocess.Popen(
                [argv[1]], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                stderr=subprocess.PIPE, bufsize=2**65,
            )
        except OSError as error:
            assert error.errno == EMFILE
        else:
            raise AssertionError("expected pipe exhaustion")
    assert descriptors() == before
    # Leave three slots: the second configured pipe fails regardless of launch API.
    for _ in range(2):
        held.append(open("sentinel.txt", "rb"))
    empty: list[str] = []
    closed = open("sentinel.txt", "rb")
    closed.close()
    before = descriptors()
    for _ in range(3):
        try:
            subprocess.Popen(empty, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        except OSError as error:
            assert error.errno == EMFILE
        else:
            raise AssertionError("expected pipe exhaustion before argv lookup")
        try:
            subprocess.Popen(
                [argv[1]], stdin=36893488147419103232, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            )
        except OSError as error:
            assert error.errno == EMFILE
        else:
            raise AssertionError("expected pipe exhaustion before descriptor conversion")
        try:
            subprocess.Popen([argv[1]], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=closed)
        except OSError as error:
            assert error.errno == EMFILE
        else:
            raise AssertionError("expected pipe exhaustion before a later closed file")
    assert descriptors() == before
    for file in held:
        file.close()


before = descriptors()
check_pipe_exhaustion()
assert descriptors() == before
for turn in range(16):
    try:
        subprocess.run([argv[1], "-I", argv[2], "status", "4"], capture_output=True, encoding="utf-8", check=True)
    except subprocess.CalledProcessError as error:
        assert error.returncode == 4
    try:
        subprocess.run(["./missing-executable"], capture_output=True, encoding="utf-8")
    except FileNotFoundError:
        pass
    try:
        subprocess.run([argv[1], "-I", argv[2], "invalid", "1"], capture_output=True, encoding="utf-8")
    except UnicodeDecodeError:
        pass
    with open("sentinel.txt", "r", encoding="utf-8") as stream:
        assert stream.read() == "preserve this file"
    result = subprocess.run([argv[1], "-I", argv[2], "empty"], capture_output=True, encoding="utf-8")
    assert result.returncode == 0
print(descriptors() == before)
print(Path("completed.txt").read_text(encoding="utf-8"))
