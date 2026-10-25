"""No child may run when constructing a Python pipe buffer fails."""

import subprocess
import sys

args = [sys.argv[1], "-c", "open('child-ran', 'w').close()"]
try:
    print(subprocess.Popen(args, stdin=subprocess.PIPE, bufsize=1 << 30).wait())
except MemoryError:
    print("stdin: MemoryError")

try:
    print(subprocess.Popen(args, stdout=subprocess.PIPE, text=True, bufsize=1 << 30).wait())
except MemoryError:
    print("stdout: MemoryError")
