import selectors
import subprocess
import sys

python = sys.argv[1]
original = subprocess._PopenSelector
assert original is selectors.PollSelector
subprocess._PopenSelector = selectors.SelectSelector
try:
    assert subprocess._PopenSelector is selectors.SelectSelector
    assert subprocess._PopenSelector != original
    child = [python, "-c", "import sys; sys.stderr.buffer.write(b'e'*200000); sys.stdout.buffer.write(sys.stdin.buffer.read())"]
    with subprocess.Popen(child, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE) as process:
        out, err = process.communicate(b'x' * 200000, timeout=10)
        print(repr((len(out), out[:4], len(err), err[:4], process.returncode)))

    writer = [python, "-c", "import os,time; os.write(1,b'part'); os.write(2,b'!'); time.sleep(30)"]
    with subprocess.Popen(writer, stdout=subprocess.PIPE, stderr=subprocess.PIPE) as timed:
        try:
            assert timed.stderr.read(1) == b'!'
            try:
                timed.communicate(timeout=0.025)
            except subprocess.TimeoutExpired as exc:
                print(repr((exc.output, exc.stderr)))
        finally:
            timed.kill()
        tail, errors = timed.communicate()
        print(repr((tail, errors, timed.returncode)))

    with subprocess.Popen([python, "-c", "print('range')"], stdout=subprocess.PIPE, stderr=subprocess.PIPE) as large:
        try:
            large.communicate(timeout=1e11)
        except OverflowError as exc:
            print(str(exc))
        output, _ = large.communicate()
        print(repr(output))
finally:
    subprocess._PopenSelector = original

assert subprocess._PopenSelector is original
with subprocess.Popen([python, "-c", "print('restored')"], stdout=subprocess.PIPE, stderr=subprocess.PIPE) as restored:
    result, _ = restored.communicate(timeout=10)
    print(repr(result))
