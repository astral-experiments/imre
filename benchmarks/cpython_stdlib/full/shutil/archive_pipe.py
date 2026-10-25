"""Drive a single native archive registry using inherited Linux pipe descriptors."""
from contextlib import contextmanager, ExitStack
import os
import select
import subprocess


@contextmanager
def session(command, *, env=None, pass_fds=()):
    with ExitStack() as stack:
        incoming, writer = os.pipe()
        reader, outgoing = os.pipe()
        tx = stack.enter_context(os.fdopen(writer, "wb", buffering=0))
        rx = stack.enter_context(os.fdopen(reader, "rb", buffering=0))
        try:
            proc = subprocess.Popen(
                [*command, f"/proc/self/fd/{incoming}", f"/proc/self/fd/{outgoing}"],
                env=env, pass_fds=(*pass_fds, incoming, outgoing),
                stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        finally:
            os.close(incoming)
            os.close(outgoing)

        def send(*fields):
            lines = [str(x) for x in fields]
            if any("\n" in line for line in lines):
                raise ValueError("unrepresentable newline in archive fixture command")
            tx.write(("\n".join(lines) + "\n").encode())

        def receive():
            if not select.select([rx], [], [], 30)[0]:
                raise TimeoutError("the archive process did not reply")
            data = rx.readline()
            if not data:
                stdout, stderr = proc.communicate(timeout=10)
                raise AssertionError((proc.returncode, stdout, stderr))
            return data.decode().removesuffix("\n")

        try:
            yield send, receive
            send("quit")
            stdout, stderr = proc.communicate(timeout=30)
            assert (proc.returncode, stdout, stderr) == (0, b"", b""), (proc.returncode, stdout, stderr)
        finally:
            if proc.poll() is None:
                proc.kill()
            proc.communicate()
