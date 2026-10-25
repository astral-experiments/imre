import subprocess
import sys

python = sys.argv[1]
print(subprocess.call([python, "-c", "import sys; sys.exit(47)"]))
print(subprocess.check_call([python, "-c", "pass"], stdout=subprocess.PIPE))
cmd = [python, "-c", "import sys; print('lost'); sys.exit(23)"]
try:
    subprocess.check_call(cmd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
except subprocess.CalledProcessError as failure:
    print(failure.returncode)
    print(failure.stdout is None)
    print(failure.stderr is None)
    cmd.append("after the call")
    print(repr(failure.cmd))

# A call with a pipe waits without draining it. The child records its PID,
# fills the pipe, and blocks; an implementation using run() would complete.
blocked = [python, "-c", "import os; open('pid', 'w').write(str(os.getpid())); os.write(1, b'x' * (4 * 1024 * 1024))"]
try:
    subprocess.call(blocked, stdout=subprocess.PIPE, timeout=1)
except subprocess.TimeoutExpired as expired:
    print(expired.timeout)
    print(expired.output is None)
    print(expired.stderr is None)
print(subprocess.check_call([python, "-c", """import os
pid = int(open('pid').read())
os.unlink('pid')
try:
    os.kill(pid, 0)
except ProcessLookupError:
    pass
else:
    raise AssertionError('timed-out child is still present')
"""]))
