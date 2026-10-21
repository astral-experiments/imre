import subprocess
from sys import argv


print("inherited input and descriptors")
result = subprocess.run([argv[1], "-I", argv[2], "stdin"], capture_output=True, encoding="utf-8")
print(repr(result.stdout))
closed = subprocess.run([argv[1], "-I", argv[2], "descriptor", argv[3]], capture_output=True, encoding="utf-8")
print(closed.stdout)
print(subprocess.run([argv[4]], capture_output=True, encoding="utf-8").returncode)
