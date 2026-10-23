import subprocess
import sys


def run_checked(command: list[str], capture: bool) -> None:
    if capture:
        subprocess.run(command, capture_output=True, text=True, check=True)
    else:
        subprocess.run(command, stdout=sys.stderr, check=True)


command = [sys.argv[1], "-I", sys.argv[2], "status", "3"]
try:
    subprocess.run(command, capture_output=True, text=True, check=True)
except subprocess.CalledProcessError as error:
    print(f"{len(error.stdout)} {len(error.stderr)} {len(error.output)}")

result = subprocess.run(command, capture_output=True, text=True)
try:
    result.check_returncode()
except subprocess.SubprocessError as error:
    print(len(error.stderr))

for capture in [True, False]:
    child = command if capture else [sys.argv[1], "-Ic", "raise SystemExit(3)"]
    try:
        run_checked(child, capture)
    except subprocess.CalledProcessError as error:
        print(f"{error.stdout!r} {error.stderr!r} {error.output!r}")
        try:
            raise
        except subprocess.CalledProcessError as again:
            print(repr(again.stderr))
    try:
        if capture:
            subprocess.run(child, capture_output=True, text=True, check=True)
        else:
            subprocess.run(child, stdout=sys.stderr, check=True)
    except subprocess.CalledProcessError as error:
        print(repr(error.stderr))
