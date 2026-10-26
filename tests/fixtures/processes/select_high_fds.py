import selectors
import subprocess
import sys

child = [sys.argv[1], "-c", "print('above FD_SETSIZE')"]
original = subprocess._PopenSelector
subprocess._PopenSelector = selectors.SelectSelector
try:
    with subprocess.Popen(child, stdout=subprocess.PIPE, stderr=subprocess.PIPE) as high:
        assert high.stdout.fileno() >= 1024
        try:
            high.communicate()
        except ValueError as exc:
            print(str(exc))
finally:
    subprocess._PopenSelector = original

with subprocess.Popen(child, stdout=subprocess.PIPE, stderr=subprocess.PIPE) as restored:
    assert restored.stdout.fileno() >= 1024
    out, err = restored.communicate()
    print(repr((out, err, restored.returncode)))
