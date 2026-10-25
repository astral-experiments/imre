import shutil
import subprocess
import sys
from pathlib import Path

def nullable_group(args: list[str], group: int | None) -> bytes:
    return subprocess.check_output(args, process_group=group)

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

python = sys.argv[1]
identity = [python, "-c", "import os; print(os.getpid() == os.getsid(0), os.getpid() == os.getpgrp())"]
print(subprocess.check_output(identity, start_new_session=True))
print(subprocess.run(identity, process_group=0, capture_output=True).stdout)
with subprocess.Popen(identity, process_group=None, stdout=subprocess.PIPE) as inherited:
    print(inherited.communicate()[0])
print(subprocess.check_output(identity, process_group=False))

print(nullable_group(identity, None))
print(nullable_group(identity, 0))
for group in [-2, -1, 0]:
    print(subprocess.check_output(identity, process_group=group))

# Even a negative group that doesn't call setpgid affects the original
# filename object: only exactly -1 permits the direct POSIX spawn interface.
missing = Path("absent-child-dir/program")
for group in [-2, -1, 0]:
    try:
        subprocess.Popen([missing], process_group=group)
    except FileNotFoundError as exc:
        report_filename(exc, missing)
try:
    subprocess.Popen([missing], start_new_session=True)
except FileNotFoundError as exc:
    report_filename(exc, missing)
try:
    subprocess.Popen([missing], umask=0o053)
except FileNotFoundError as exc:
    report_filename(exc, missing)

with open("parent-before", "wb") as before:
    before.write(b"")
make_file = [python, "-c", "import os; open('child-mode','wb').close(); print(oct(os.stat('child-mode').st_mode & 0o777))"]
with open("mode-report", "wb") as destination:
    subprocess.check_call(make_file, stdout=destination, umask=0o053, start_new_session=True)
with open("mode-report", "rb") as report:
    print(report.read())
with open("parent-after", "wb") as after:
    after.write(b"")
print(subprocess.check_output([python, "-c", "import os; print(os.stat('parent-before').st_mode == os.stat('parent-after').st_mode)"]))

# setsid makes the child a session leader; the following setpgid must fail
# before exec. The parent is still usable after the failed launch.
try:
    subprocess.call([python, "-c", "open('unreachable','wb').close()"], start_new_session=True, process_group=0)
except PermissionError:
    print("session leader cannot change groups")
for large in [1099511627776, 1267650600228229401496703205376]:
    try:
        subprocess.run(identity, process_group=large)
    except OverflowError as exc:
        print(str(exc))
    try:
        subprocess.run(identity, umask=large)
    except OverflowError as exc:
        print(str(exc))

# A direct launch can ignore even an unbounded negative umask. Looking up a
# bare command or starting a new session forces conversion in fork/exec.
true_path = shutil.which("true")
assert true_path is not None
for program in [true_path, "true"]:
    try:
        print(subprocess.call([program], umask=-(2**90), close_fds=False))
    except OverflowError as exc:
        print(str(exc))
for session in [False, True]:
    try:
        subprocess.call([true_path + "\x00"], stdin=1237940039285380274899124224, start_new_session=session, close_fds=False)
    except ValueError:
        print("invalid executable first")
    except OverflowError:
        print("invalid descriptor first")
    empty: list[str] = []
    try:
        print(subprocess.call(empty, executable=true_path, start_new_session=session, close_fds=False))
    except ValueError as exc:
        print(str(exc))

# The parent resolves stream handles before fork converts the child attributes.
try:
    subprocess.Popen(identity, stdout=report, process_group=2**65)
except ValueError as exc:
    print(str(exc))
try:
    subprocess.Popen(identity, stdout=subprocess.STDOUT, umask=2**65)
except ValueError as exc:
    print(str(exc))
try:
    subprocess.Popen(identity, env={"invalid=name": "val"}, umask=2**65)
except ValueError as exc:
    print(str(exc))
try:
    subprocess.Popen(["bad\x00program"], process_group=2**65)
except OverflowError as exc:
    print(str(exc))
