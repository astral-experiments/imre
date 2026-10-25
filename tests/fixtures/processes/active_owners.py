import subprocess
import sys
import time
from typing import BinaryIO


def drop_local(argv: list[str]) -> tuple[int, int, BinaryIO]:
    process = subprocess.Popen(argv, stdout=subprocess.PIPE)
    return id(process), process.pid, process.stdout


def drop_sequence(argv: list[str]) -> tuple[int, int, BinaryIO]:
    processes = [subprocess.Popen(argv, stdout=subprocess.PIPE)]
    return id(processes[0]), processes[0].pid, processes[0].stdout


def unchanged_argument(id: int) -> int:
    return id


argv = [sys.argv[1], "-c", "import time; time.sleep(30)"]
active = subprocess._active
assert active is subprocess._active
assert not active
assert unchanged_argument(7) == 7
first_id, first_pid, first_pipe = drop_local(argv)
second_id, second_pid, second_pipe = drop_sequence(argv)
assert active is subprocess._active
assert [id(item) for item in active] == [first_id, second_id]
assert first_id != second_id
first = active[0]
second = active[1]
assert first is active[0] and first is not second
assert first.pid == first_pid and second.pid == second_pid

# Killing and observing pipe EOF leaves the kernel status for the next launch.
first.kill()
assert first_pipe.read() == b""
assert first.returncode is None
for attempt in range(100):
    try:
        subprocess.Popen(["./missing-program-for-active-cleanup"])
    except FileNotFoundError:
        pass
    if len(active) == 1:
        break
assert [id(item) for item in active] == [second_id]
assert first.returncode == -9 and first.wait() == -9
assert not first_pipe.closed
first_pipe.close()

second.kill()
assert second_pipe.read() == b""
for attempt in range(100):
    subprocess.run([sys.argv[1], "-c", "pass"], check=True)
    if not active:
        break
assert not active
assert second.returncode == -9
second_pipe.close()

third_id, third_pid, third_pipe = drop_local(argv)
third = active[0]
assert id(third) == third_id and third.pid == third_pid
third.kill()
assert third_pipe.read() == b""
for attempt in range(100):
    try:
        subprocess.Popen(argv, stdout=subprocess.STDOUT)
    except ValueError:
        pass
    if not active:
        break
    time.sleep(0.001)
assert not active and third.returncode == -9
third_pipe.close()
print("retained original Popen objects, reaped on failed and successful launches")
