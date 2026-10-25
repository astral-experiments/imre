import subprocess
import sys
from pathlib import Path

echo = (sys.argv[1], "-c", "import sys; sys.stdout.buffer.write(sys.stdin.buffer.read())")
print(subprocess.check_output(echo, input=b"a\x00\xff"))
print(subprocess.check_output(echo))
print(subprocess.check_output(echo, input=None))
print(repr(subprocess.check_output(echo, input="café\r\n", universal_newlines=True)))
print(subprocess.check_output("printf shell", shell=True))
print(subprocess.check_output([Path("/bin/sh"), "-c", "printf out; printf err >&2"], stderr=subprocess.STDOUT))
raw = [sys.argv[1], "-c", "import os; os.write(1, b'a\\xffb\\r\\n\\xc3')"]
print(repr(subprocess.check_output(raw, errors="ignore")))
print(repr(subprocess.check_output(raw, errors="replace")))

try:
    subprocess.check_output(("/bin/sh", "-c", "printf output; printf error >&2; exit 7"), stderr=subprocess.PIPE)
except subprocess.CalledProcessError as checked:
    print(checked.returncode)
    print(repr(checked.output))
    print(repr(checked.stderr))
    print(repr(checked.cmd))

for option in (0, 1, 2, 3):
    try:
        if option == 0:
            subprocess.check_output([], stdout=None)
        elif option == 1:
            subprocess.check_output([], check=False)
        elif option == 2:
            subprocess.check_output([], stdin=subprocess.PIPE, input=None)
        else:
            subprocess.check_output([], capture_output=True)
    except ValueError as error:
        print(str(error))

try:
    subprocess.check_output([], text=True, universal_newlines=False)
except subprocess.SubprocessError as error:
    print(str(error))

pidfile = Path("checked-capture.pid")
try:
    subprocess.check_output(["/bin/sh", "-c", "echo $$ > checked-capture.pid; printf ready; exec sleep 10"],
                            timeout=0.5, text=True)
except subprocess.TimeoutExpired as timeout:
    print(repr(timeout.output))
    print(repr(timeout.stderr))
    # kill -0 also succeeds for a zombie. The parent must have reaped it
    # before delivering TimeoutExpired to this handler.
    print(subprocess.call(["/bin/sh", "-c", 'kill -0 "$1" 2>/dev/null', "probe", pidfile.read_text().strip()]))
    pidfile.unlink()
