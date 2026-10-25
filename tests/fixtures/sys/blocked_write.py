import os
import sys

saved = os.dup(1)
os.dup2(int(sys.argv[1]), 1)
try:
    if sys.argv[2] == "write":
        sys.stdout.write("x" * 262144)
    else:
        sys.stdout.write("x")
        sys.stdout.flush()
except BlockingIOError as error:
    diagnostic = repr((error.characters_written, error.errno, error.strerror, repr(error)))
else:
    diagnostic = "no blocked write"
finally:
    os.dup2(saved, 1)
    os.close(saved)
sys.stdout.flush()
print(diagnostic, file=sys.stderr)
