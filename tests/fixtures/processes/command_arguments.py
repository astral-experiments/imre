from pathlib import Path
import subprocess
import sys

python = Path(sys.argv[1])
# The OS sees the bytes intact. Popen.args and an exception retain the tuple,
# including its Path and bytes elements.
argv = (python, "-c", "import os,sys; print(os.fsencode(sys.argv[1]).hex())", b"\xff")
p = subprocess.Popen(argv, stdout=subprocess.PIPE)
print(repr(p.args))
print(repr(p.communicate()[0]))
print(subprocess.call(("/bin/sh", "-c", "exit 18")))
print(subprocess.check_call((b"/bin/true",)))
print(subprocess.call(b"/bin/true"))
print(subprocess.call(Path("/bin/true")))
try:
    subprocess.check_call("/bin/false")
except subprocess.CalledProcessError as string_error:
    print(str(string_error))
    print(string_error.cmd)
try:
    subprocess.check_call((Path("/bin/false"),))
except subprocess.CalledProcessError as tuple_error:
    print(str(tuple_error))
    print(repr(tuple_error.cmd))
try:
    subprocess.call(("/bin/sleep", b"15"), timeout=0)
except subprocess.TimeoutExpired as timeout_error:
    print(str(timeout_error))
    print(repr(timeout_error.cmd))
aliased = [b"/bin/true"]
child = subprocess.Popen(aliased)
aliased.append(b"after launch")
print(repr(child.args))
print(child.wait())
