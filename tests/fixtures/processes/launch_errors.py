from pathlib import Path
import subprocess


def describe(error: Exception) -> None:
    assert isinstance(error, OSError)
    code = error.errno
    reason = error.strerror
    if code is not None:
        print(code)
    else:
        raise AssertionError("missing errno")
    if reason is not None:
        print(reason)
    else:
        raise AssertionError("missing strerror")
    name = error.filename
    if isinstance(name, str):
        print(repr(name))
    elif isinstance(name, bytes):
        print(repr(name))
    elif isinstance(name, Path):
        print(repr(name))
    else:
        raise AssertionError("missing failure filename")
    print(str(error))
    print(error.filename2 is None)


try:
    subprocess.Popen(["no-imre-program", "arg"], cwd="/no-imre-directory")
except FileNotFoundError as error:
    describe(error)
try:
    subprocess.call(["/no-imre-program", "arg"], cwd="/")
except FileNotFoundError as error:
    describe(error)
try:
    subprocess.Popen((b"/no-imre-\xff", "-c", "pass"))
except FileNotFoundError as error:
    describe(error)
try:
    subprocess.Popen(["/bin/true"], cwd=b"/no-imre-\xff")
except FileNotFoundError as error:
    describe(error)
try:
    subprocess.Popen(Path("/no-imre-program"))
except FileNotFoundError as error:
    describe(error)
try:
    subprocess.Popen(["/bin/true"], executable=Path("/no-imre-program"))
except FileNotFoundError as error:
    describe(error)
directory = Path("/no-imre-directory")
try:
    subprocess.Popen(["/bin/true"], cwd=directory)
except FileNotFoundError as error:
    describe(error)
    name = error.filename
    if isinstance(name, Path):
        assert name is directory
    else:
        raise AssertionError("cwd object was replaced")
try:
    subprocess.Popen("exit 0", shell=True, executable=b"/no-imre-shell")
except FileNotFoundError as error:
    describe(error)
# A real directory exists at cwd; exec failure must still blame the executable.
try:
    subprocess.Popen(["/bin/true"], executable="/no-imre-program", cwd=".")
except FileNotFoundError as error:
    describe(error)
try:
    subprocess.Popen(["/bin/true"], cwd="/dev/null")
except OSError as error:
    describe(error)
