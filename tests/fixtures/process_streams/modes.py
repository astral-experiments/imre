import subprocess
import sys
from subprocess import CalledProcessError, CompletedProcess, DEVNULL, PIPE, STDOUT, run

def selected() -> list[str]:
    print("evaluated before stream conflict")
    return ["/not-run"]


def show_streams(error: Exception) -> None:
    print(repr(error.stdout))
    print(repr(error.stderr))
    print(repr(error.output))


# Use raw writes so stderr=STDOUT observes the child's original write order.
command = [sys.argv[1], "-c", "import os; os.write(1,b'O\\r\\n'); os.write(2,b'E\\r'); os.write(1,b'X')"]
inherited = run(command)
print(repr(inherited))
print(inherited.stdout is None)
print(inherited.stderr is None)
print(isinstance(inherited, CompletedProcess))
redirected = run(command, stdout=sys.stderr, stderr=sys.stdout)
print(repr(redirected))
out = run(command, stdout=PIPE, stderr=DEVNULL)
print(repr(out.stdout))
print(out.stderr is None)
err = run(command, stdout=DEVNULL, stderr=PIPE, encoding="UTF8")
print(err.stdout is None)
print(repr(err.stderr))
merged = run(command, stdout=PIPE, stderr=STDOUT, text=True)
print(repr(merged))
raw: CompletedProcess[bytes] = run(command, capture_output=True, stdout=None)
print(repr(raw.stdout))
print(repr(raw.stderr))
raw_invalid = run([sys.argv[1], "-c", "import os; os.write(1,b'\\xff\\x00')"], stdout=PIPE)
print(repr(raw_invalid.stdout))

for index in range(3):
    fail = [sys.argv[1], "-c", "import os; os.write(1,b'O'); os.write(2,b'E'); raise SystemExit(37)"]
    try:
        if index == 0:
            run(fail, capture_output=True, check=True)
        elif index == 1:
            run(fail, stdout=DEVNULL, stderr=PIPE, text=True).check_returncode()
        else:
            run(fail, stdout=DEVNULL, stderr=DEVNULL, check=True)
    except CalledProcessError as exc:
        print(exc.returncode)
        print(repr(exc.output))
        print(repr(exc.stdout))
        print(repr(exc.stderr))

try:
    run(selected(), stdout=PIPE, capture_output=True)
except ValueError as exc:
    print(str(exc))

try:
    run(command, stdout=STDOUT)
except ValueError as exc:
    print(str(exc))

try:
    run([sys.argv[1], "-c", "import time; time.sleep(10)"], timeout=0.01, stderr=DEVNULL)
except subprocess.TimeoutExpired as exc:
    print(exc.output is None)
    print(exc.stderr is None)

# The explicit fd is from the parent, even when the other child fd is a pipe.
explicit = run(command, stdout=PIPE, stderr=sys.stdout)
print(repr(explicit.stdout))

# A direct checked call keeps each concrete field type inside the handler,
# even though other failures in this program carry different stream types.
try:
    run([sys.argv[1], "-c", "import sys; print('byte'); sys.exit(7)"],
        stdout=PIPE, stderr=DEVNULL, check=True)
except CalledProcessError as exc:
    print(repr(exc.output + b"!"))
    print(exc.stderr is None)


for deadline in [float("inf"), float("nan"), 1e12]:
    print(run(["/bin/sh", "-c", "exit 0"], timeout=deadline).returncode)


# A deadline may leave raw bytes where a checked failure from the same run
# carries decoded text. The common exception view must preserve both layouts.
for expire in ["no", "yes"]:
    argv = [sys.argv[1], "-Ic",
            "import os,sys,time; os.write(1,b'O\\r\\n'); os.write(2,b'E\\r'); "
            "time.sleep(10) if sys.argv[1] == 'yes' else sys.exit(7)", expire]
    try:
        run(argv, capture_output=True, text=True, check=True, timeout=0.5)
    except subprocess.SubprocessError as error:
        print(repr(error.stdout))
        print(repr(error.stderr))
        print(repr(error.output))
        show_streams(error)

try:
    run([sys.argv[1], "-Ic", "import time; time.sleep(10)"], stdout=PIPE, text=True, timeout=0.1)
except subprocess.SubprocessError as error:
    show_streams(error)

try:
    run([sys.argv[1], "-Ic", "print('captured'); raise SystemExit(1)"],
        capture_output=True, text=True, check=True, timeout=5)
except subprocess.CalledProcessError as error:
    print(error.stdout + "text")
