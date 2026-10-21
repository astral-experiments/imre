import subprocess
from sys import argv


command = [argv[1], "-I", argv[2], "echo", "", "space separated", "éλ🙂", "$HOME;*|>", "-flag"]
result = subprocess.run(command, capture_output=True, encoding="utf-8")
print(result.returncode)
print(repr(result.stdout))
print(repr(result.stderr))
print(repr(result.args) == repr(command))
command.append("changed after execution")
print(result.args[-1])
print(result.check_returncode())

empty = subprocess.run([argv[1], "-I", argv[2], "empty"], capture_output=True, encoding="utf-8", text=True)
print(f"{empty.stdout!r} {empty.stderr!r} {empty.returncode}")
print(bool(empty))
newlines = subprocess.run([argv[1], "-I", argv[2], "newlines"], capture_output=True, encoding="utf-8")
print(repr(newlines.stdout))
print(repr(newlines.stderr))

large = subprocess.run([argv[1], "-I", argv[2], "large"], capture_output=True, encoding="utf-8")
assert len(large.stdout) == 262144 and large.stdout.replace("O", "") == ""
assert len(large.stderr) == 262144 and large.stderr.replace("E", "") == ""
print(f"{len(large.stdout)} {len(large.stderr)}")
