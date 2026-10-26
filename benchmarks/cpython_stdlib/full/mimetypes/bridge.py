"""Call an already-built native MIME database from CPython test fixtures."""
from ast import literal_eval
from contextlib import contextmanager
import subprocess


@contextmanager
def worker(binary, mode):
    with subprocess.Popen([binary, mode], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                          stderr=subprocess.PIPE, text=True, encoding="utf-8") as proc:
        def call(op, arg="", strict=True, *extra):
            fields = [op, arg, str(int(strict)), *extra]
            if any("\n" in field for field in fields):
                raise ValueError("newline outside this fixture's request domain")
            proc.stdin.write("\n".join(fields) + "\n")
            proc.stdin.flush()
            reply = proc.stdout.readline()
            if not reply:
                raise RuntimeError(f"native MIME child exited: {proc.stderr.read()}")
            return literal_eval(reply)
        try:
            yield call
        finally:
            if proc.poll() is None:
                proc.stdin.write("quit\n")
                proc.stdin.flush()
            proc.stdin.close()
            if proc.wait(timeout=10):
                raise RuntimeError(proc.stderr.read())
