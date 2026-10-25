import subprocess
import sys
argv = [sys.argv[1], "-c", "import time; time.sleep(15)"]
proc = subprocess.Popen(argv)
alias = proc
argv.append("extra")
print(repr(proc.args))
print(proc.pid > 0)
print(proc.returncode is None)
print(alias.poll() is None)
try:
    proc.wait(0)
except subprocess.TimeoutExpired as error:
    print(str(error))
    print(error.timeout)
print(repr(proc.poll()))
alias.terminate()
print(proc.wait())
proc.kill()
print(repr(proc.poll()))
print(repr(alias.returncode))
done = subprocess.Popen([sys.argv[1], "-c", "import sys; sys.exit(47)"])
print(done.wait(timeout=9))
print(done.wait())

with subprocess.Popen([sys.argv[1], "-c", "import sys; sys.exit(100)"]) as scoped:
    print(scoped.returncode is None)
print(repr(scoped.returncode))
