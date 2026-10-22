"""Deadline snapshots remain bytes even when normal results are UTF-8 text."""

import subprocess
from subprocess import CalledProcessError, SubprocessError, TimeoutExpired as Expired
from sys import argv


def command(code: str) -> list[str]:
    return [argv[1], "-I", "-c", code]


def same_bytes(first: bytes | None, second: bytes | None) -> bool:
    if first is None:
        return second is None
    if second is None:
        return False
    return first == second


def expire(code: str, seconds: float) -> None:
    args = command(code)
    try:
        subprocess.run(args, capture_output=True, encoding="utf-8", timeout=seconds, check=True)
    except CalledProcessError:
        raise AssertionError("timeout was treated as an exit status")
    except Expired as error:
        print(f"{error.timeout} {error.stdout!r} {error.output!r} {error.stderr!r}")
        print(f"{isinstance(error, Expired)} {isinstance(error, SubprocessError)} {isinstance(error, CalledProcessError)}")
        print(repr(error.cmd) == repr(args))
        output = error.stdout
        if output is not None:
            print(len(output))
        args.append("mutated after timeout")
        print(error.cmd[-1])
        print(str(error).endswith(" seconds"))
        saved: Exception = error
        if isinstance(saved, Expired):
            print(same_bytes(saved.stdout, error.stdout))
        maybe: Exception | None = error
        if isinstance(maybe, Expired):
            print(maybe.timeout == seconds)
        try:
            raise error
        except subprocess.TimeoutExpired as again:
            print(same_bytes(again.stdout, error.stdout))
    else:
        raise AssertionError("child unexpectedly completed")


expire("import time; time.sleep(10)", 0.0)
expire("import time; time.sleep(10)", -0.0)
expire("import time; time.sleep(10)", -0.5)
expire("import os,time; os.write(1,b'out\\r\\n\\xff'); os.write(2,b'err\\xe2\\x82'); time.sleep(10)", 1.0)
expire("import os,time; os.write(1,b'only stdout'); time.sleep(10)", 1.0)
# A closed stream is an observed empty byte string while another pipe stays open.
expire("import os,time; os.close(1); os.write(2,b'only stderr'); time.sleep(10)", 1.0)
# Once both pipes close, CPython's wait timeout carries no stream snapshots.
expire("import os,time; os.write(1,b'discarded'); os.close(1); os.close(2); time.sleep(10)", 1.0)

duration: float | None = None
result = subprocess.run(command("print('no deadline')"), capture_output=True, encoding="utf-8", timeout=duration)
print(result.stdout)
duration = 5.0
print(subprocess.run(command("print('within deadline')"), capture_output=True, encoding="utf-8", timeout=duration).stdout)
try:
    subprocess.run(command("raise SystemExit(7)"), capture_output=True, encoding="utf-8", timeout=5.0, check=True)
except CalledProcessError as error:
    print(error.returncode)
try:
    subprocess.run(command("import os; os.write(1,b'\\xff')"), capture_output=True, encoding="utf-8", timeout=5.0)
except UnicodeDecodeError:
    print("completed output still decodes strictly")

for value in [float("nan"), float("inf"), 2147484.0, 1e20]:
    try:
        subprocess.run(command("import time; time.sleep(10)"), capture_output=True, encoding="utf-8", timeout=value)
    except ValueError as error:
        print("ValueError: " + str(error))
    except OverflowError as error:
        print("OverflowError: " + str(error))
expire("import time; time.sleep(10)", -float("inf"))

# Launch failures precede deadline validation.
try:
    subprocess.run(["./missing-executable"], capture_output=True, encoding="utf-8", timeout=float("nan"))
except FileNotFoundError:
    print("launch precedes timeout")

for turn in range(40):
    try:
        subprocess.run(command("import time; time.sleep(10)"), capture_output=True, encoding="utf-8", timeout=0.0)
    except Expired:
        pass
print(subprocess.run(command("print('recovered')"), capture_output=True, encoding="utf-8").stdout)
