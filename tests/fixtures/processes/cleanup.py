import subprocess
from pathlib import Path
from sys import argv


def descriptors() -> int:
    return sum(1 for path in Path("/proc/self/fd").iterdir())


before = descriptors()
for turn in range(16):
    try:
        subprocess.run([argv[1], "-I", argv[2], "status", "4"], capture_output=True, encoding="utf-8", check=True)
    except subprocess.CalledProcessError as error:
        assert error.returncode == 4
    try:
        subprocess.run(["./missing-executable"], capture_output=True, encoding="utf-8")
    except FileNotFoundError:
        pass
    try:
        subprocess.run([argv[1], "-I", argv[2], "invalid", "1"], capture_output=True, encoding="utf-8")
    except UnicodeDecodeError:
        pass
    with open("sentinel.txt", "r", encoding="utf-8") as stream:
        assert stream.read() == "preserve this file"
    result = subprocess.run([argv[1], "-I", argv[2], "empty"], capture_output=True, encoding="utf-8")
    assert result.returncode == 0
print(descriptors() == before)
print(Path("completed.txt").read_text(encoding="utf-8"))
