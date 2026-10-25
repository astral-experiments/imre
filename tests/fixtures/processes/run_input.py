import subprocess
import sys
from pathlib import Path

child = (sys.argv[1], "-c", "import sys; sys.stdout.buffer.write(sys.stdin.buffer.read())")
print(repr(subprocess.run(child, input=b"\x00\xff", stdout=subprocess.PIPE).stdout))
print(subprocess.run(child, input=None, stdin=subprocess.DEVNULL, capture_output=True).stdout)
print(repr(subprocess.run(child, input="text\r\n", capture_output=True, universal_newlines=True).stdout))
print(subprocess.run(Path("directory/exit-zero"), cwd=b".", close_fds=False).args)
print(subprocess.run(b"directory/exit-zero").args)
print(repr(subprocess.run([Path("directory/exit-zero")]).args))
print(subprocess.run("printf shell", shell=True, executable=Path("/bin/sh"), capture_output=True).stdout)
print(subprocess.run("directory/exit-zero", input="text", text=True).returncode)
print(subprocess.run([sys.argv[1], "-c", "import os; os.write(1,b'a\\xffb')"], errors="replace", stdout=subprocess.PIPE).stdout)

command = ["/bin/sh", "-c", "printf out; printf err >&2; exit 7"]
result = subprocess.run(command, capture_output=True)
command.append("after launch")
print(repr(result.args))
print(repr(result))
try:
    result.check_returncode()
except subprocess.CalledProcessError as error:
    print(repr(error.cmd))
    print(repr(error.stdout))
    print(repr(error.stderr))

try:
    subprocess.run((b"/bin/sh", "-c", "exit 11"), check=True)
except subprocess.CalledProcessError as checked:
    print(repr(checked.cmd))
    print(checked.returncode)
    print(checked.stdout is None)

try:
    subprocess.run(["directory/exit-one"], input=b"", stdin=subprocess.DEVNULL, capture_output=True, stderr=subprocess.PIPE)
except ValueError as conflict:
    print(str(conflict))
