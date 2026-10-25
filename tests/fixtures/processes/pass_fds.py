import os
from pathlib import Path
import subprocess
import sys


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
        print(exc.filename is missing, str(exc.filename))
    try:
        subprocess.Popen([missing], pass_fds=())
    except FileNotFoundError as exc:
        print(exc.filename is missing, str(exc.filename))
finally:
    os.close(first)
    os.close(second)

try:
    subprocess.Popen(["/bin/true"], pass_fds=(654321,))
except OSError:
    print("closed descriptor rejected")
try:
    subprocess.Popen(["/bin/true"], pass_fds=(-1,))
except ValueError as error:
    print(str(error))
try:
    subprocess.Popen(["/bin/true"], pass_fds=(1 << 34,))
except ValueError as error:
    print(str(error))
