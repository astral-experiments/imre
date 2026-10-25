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
identities = [id(item) for item in active]
assert len(identities) == 2 and identities[0] == first_id and identities[1] == second_id
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
assert len(active) == 1 and id(active[0]) == second_id
first_status = first.returncode
assert first_status is not None and first_status == -9 and first.wait() == -9
assert not first_pipe.closed
first_pipe.close()

second.kill()
assert second_pipe.read() == b""
for attempt in range(100):
    subprocess.run([sys.argv[1], "-c", "pass"], check=True)
    if not active:
        break
assert not active
second_status = second.returncode
assert second_status is not None and second_status == -9
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
third_status = third.returncode
assert not active and third_status is not None and third_status == -9
third_pipe.close()

fourth_id, fourth_pid, fourth_pipe = drop_sequence(argv)
fourth = active[0]
assert id(fourth) == fourth_id and fourth.pid == fourth_pid
fourth.kill()
assert fourth.wait() == -9
assert active[0] is fourth
subprocess._cleanup()
assert not active
fourth_pipe.close()
print("retained original Popen objects, reaped on failed and successful launches")
