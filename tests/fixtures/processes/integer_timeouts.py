from subprocess import TimeoutExpired, run
from sys import argv

command = [argv[1], "-I", "-c", "import time; time.sleep(10)"]
try:
    run(command, capture_output=True, text=True, timeout=-9007199254740993)
except TimeoutExpired as error:
    value: int = error.timeout
    print(f"{value} {value % 10} {isinstance(value, int)}")
    print(str(error))
try:
    run(command, capture_output=True, text=True, timeout=0)
except TimeoutExpired as error:
    print(f"{error.timeout} {error.stdout!r} {error.stderr!r}")
try:
    run(command, capture_output=True, text=True, timeout=2147484)
except OverflowError as error:
    print(str(error))
try:
    run(["./missing-executable"], capture_output=True, text=True, timeout=2147484)
except FileNotFoundError:
    print("launch first")
print(run([argv[1], "-I", "-c", "print('implicit UTF-8 λ')"], capture_output=True, text=True, timeout=15).stdout)
