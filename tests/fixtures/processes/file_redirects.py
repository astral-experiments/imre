from pathlib import Path
import subprocess
from typing import BinaryIO


def evaluated(file: BinaryIO) -> BinaryIO:
    print("evaluated stdout")
    return file


def same_filename(error: Exception, path: Path) -> bool:
    assert isinstance(error, OSError)
    name = error.filename
    if isinstance(name, Path):
        return name is path
    if isinstance(name, str):
        assert name == str(path)
        return False
    raise AssertionError("missing filename")


Path("redirect-input").write_bytes(b"012345")
with open("redirect-input", "rb") as source, open("redirect-output", "wb") as destination:
    source.seek(2)
    destination.write(b"before:")
    destination.flush()
    p = subprocess.Popen(
        ["/bin/sh", "-c", "cat; printf ':error:' >&2"],
        stdin=source, stdout=destination, stderr=destination.fileno(),
    )
    print(p.wait())
    print(p.stdin is None)
    print(p.stdout is None)
    print(p.stderr is None)
    destination.write(b"after")
    print(source.tell())
print(repr(Path("redirect-output").read_bytes()))

# The two descriptors refer to the same open file description; stderr=STDOUT
# follows the child's redirected stdout and the parent's file remains usable.
with open("redirect-output", "wb") as target:
    result = subprocess.run(["/bin/sh", "-c", "printf A; printf B >&2"],
                            stdout=target, stderr=-2)
    print(result.stdout is None)
    print(result.stderr is None)
    target.write(b"C")
print(repr(Path("redirect-output").read_bytes()))

with open("redirect-output", "w") as text:
    print(subprocess.call(["/bin/sh", "-c", "printf 'text' >&2"], stderr=text))
print(Path("redirect-output").read_text())
print(repr(subprocess.check_output(["/bin/sh", "-c", "printf ok; printf ignored >&2"], stderr=-3)))
print(repr(subprocess.run(["/bin/sh", "-c", "printf bytes"], stdout=-1).stdout))
print(subprocess.run(["/bin/sh", "-c", "exit 0"], stdin=0).returncode)

try:
    subprocess.run(["/bin/sh"], stdout=evaluated(destination), capture_output=True)
except ValueError as conflict:
    print(str(conflict))
try:
    subprocess.Popen(["/bin/sh"], stdout=destination)
except ValueError as closed:
    print(str(closed))
try:
    subprocess.Popen(["/bin/sh"], stdout=12345)
except OSError:
    print("closed descriptor")
try:
    subprocess.Popen(["/bin/sh"], stdin=subprocess.STDOUT)
except OSError:
    print("invalid stdin merge")

# The caller's low fd forces fork semantics. The parent duplicates this fd for
# launch, but that higher duplicate cannot change the reported filename type.
missing = Path("directory/missing-redirect-child")
try:
    subprocess.Popen([missing], stdin=0)
except FileNotFoundError as failure:
    print(same_filename(failure, missing))
try:
    subprocess.Popen([missing], stdout=1)
except FileNotFoundError as failure:
    print(same_filename(failure, missing))
try:
    subprocess.Popen([missing], stderr=2)
except FileNotFoundError as failure:
    print(same_filename(failure, missing))
with open("redirect-output", "wb") as higher:
    try:
        subprocess.Popen([missing], stdout=higher)
    except FileNotFoundError as failure:
        print(same_filename(failure, missing))

large_buffer = 2**65
exit_zero = ["/bin/sh", "-c", "exit 0"]
print(subprocess.Popen(exit_zero, bufsize=large_buffer).wait())
print(subprocess.Popen(exit_zero, stdout=subprocess.DEVNULL, bufsize=-large_buffer).wait())
try:
    subprocess.Popen([missing], stdout=subprocess.PIPE, bufsize=large_buffer)
except OverflowError as overflow:
    print(str(overflow))
try:
    subprocess.Popen(exit_zero, stdout=destination, stderr=subprocess.PIPE, bufsize=large_buffer)
except ValueError as closed:
    print(str(closed))
try:
    subprocess.Popen(["/bin/bad\x00command"], stdin=subprocess.STDOUT)
except ValueError:
    print("embedded NUL in the command")

try:
    subprocess.Popen(exit_zero, stdin=36893488147419103232, stdout=destination)
except ValueError as closed:
    print(str(closed))
for executable in ("/bin/sh", "/bin/bad\x00command"):
    try:
        subprocess.Popen([executable], stdin=36893488147419103232)
    except OverflowError:
        print("oversized descriptor")
    except ValueError:
        print("invalid executable")
    try:
        subprocess.Popen([executable], stdin=36893488147419103232, cwd=".")
    except OverflowError:
        print("oversized descriptor with cwd")

empty: list[str] = []
try:
    print(subprocess.Popen(empty, executable="/bin/sh", stdin=subprocess.DEVNULL).wait())
except ValueError as invalid:
    print(str(invalid))
print(subprocess.Popen(empty, executable="/bin/sh", stdin=subprocess.DEVNULL, cwd=".").wait())
