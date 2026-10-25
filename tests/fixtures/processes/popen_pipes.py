import subprocess
import sys

python = sys.argv[1]
child = [python, "-c", "import sys; sys.stderr.buffer.write(b'e'*200000); data=sys.stdin.buffer.read(); sys.stdout.buffer.write(data)"]
with subprocess.Popen(child, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE) as p:
    # Fill more than an OS pipe before consuming output: write and both reads
    # must progress together. A zero deadline leaves input for the next call.
    try:
        p.communicate(b'a' * 200000, timeout=0)
    except subprocess.TimeoutExpired as exc:
        print(repr((exc.output, exc.stderr)))
    try:
        p.communicate(b'extra')
    except ValueError as exc:
        print(str(exc))
    out, err = p.communicate(timeout=10)
    print(repr((len(out), out[:4], len(err), err[:4])))
    print(repr(p.returncode))
    print(repr((p.stdin.closed, p.stdout.closed, p.stderr.closed)))
    again, again_err = p.communicate()
    print(repr((len(again), len(again_err))))

with subprocess.Popen([python, "-c", "import sys; sys.stdout.buffer.write(b'a\\r\\nb\\r')"], stdout=subprocess.PIPE, text=True) as single:
    alias = single.stdout
    result, unused = single.communicate()
    print(repr((result, unused, alias.closed)))
    try:
        single.communicate()
    except ValueError:
        print("closed reader")

with subprocess.Popen([python, "-c", "import os; os.write(1,b'x'); os.close(1)"], stdout=subprocess.PIPE) as p2:
    print(p2.stdout.fileno() >= 0)
    print(p2.stdout.read(1))
    print(p2.stdout.closed)
print(p2.stdout.closed)
