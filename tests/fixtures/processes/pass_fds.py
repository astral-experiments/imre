import os
from pathlib import Path
import shutil
import subprocess
import sys


def report_filename(error: Exception, original: Path) -> None:
    assert isinstance(error, OSError)
    name = error.filename
    if isinstance(name, Path):
        print(name is original)
        print(str(name))
    elif isinstance(name, str):
        print(False)
        print(name)
    else:
        raise AssertionError("launch error lost the executable filename")


# Query only the supplied fds, so the observation works on Linux and macOS.
probe = """import os, sys
for arg in sys.argv[1:]:
    try:
        os.fstat(int(arg))
    except OSError:
        print('closed')
    else:
        print('open')
"""
first, second = os.pipe()
try:
    os.set_inheritable(first, True)
    os.set_inheritable(second, False)
    argv = [sys.argv[1], "-c", probe, str(first), str(second)]
    print(repr(subprocess.check_output(argv, close_fds=False)))
    print(repr(subprocess.check_output(argv, close_fds=True)))
    print(repr(subprocess.check_output(argv, pass_fds=(second,))))
    print(repr(subprocess.check_output(argv, pass_fds=[second, first, second])))
    print(repr(subprocess.check_output(argv, pass_fds={first, second})))
    print(os.get_inheritable(first))
    print(os.get_inheritable(second))

    missing = Path("missing-pass-fds-dir/program")
    try:
        subprocess.Popen([missing], pass_fds=(second,))
    except FileNotFoundError as exc:
        report_filename(exc, missing)
    try:
        subprocess.Popen([missing], pass_fds=())
    except FileNotFoundError as exc:
        report_filename(exc, missing)
finally:
    os.close(first)
    os.close(second)

true_path = shutil.which("true")
assert true_path is not None
try:
    subprocess.Popen([true_path], pass_fds=(654321,))
except OSError:
    print("closed descriptor rejected")
try:
    subprocess.Popen([true_path], pass_fds=(-1,))
except ValueError as error:
    print(str(error))
try:
    subprocess.Popen([true_path], pass_fds=(1 << 34,))
except ValueError as error:
    print(str(error))

try:
    subprocess.Popen([true_path], pass_fds=(-1,), umask=2**65)
except OverflowError as error:
    print(str(error))
try:
    subprocess.Popen(["bad\x00program"], pass_fds=(-1,))
except ValueError as error:
    print(str(error))

# Only the fork path iterates an explicit None, before C narrows child settings.
for program in [true_path, "true"]:
    try:
        print(subprocess.call([program], pass_fds=None, close_fds=False))
    except TypeError as error:
        print(str(error))
try:
    subprocess.call([true_path], pass_fds=None, umask=2**65)
except TypeError as error:
    print(str(error))
try:
    subprocess.call([true_path], pass_fds=(1,), umask=-(2**90))
except OverflowError as error:
    print(str(error))
try:
    print(subprocess.call([true_path], pass_fds=(), umask=-(2**90), close_fds=False))
except OverflowError as error:
    print(str(error))
