import os
import sys


class Recorder:
    def write(self, text):
        self.text = text
        return len(text)


original = sys.stderr
assert original is sys.__stderr__
assert sys.stdout is sys.__stdout__
assert sys.stdout is not original
assert isinstance(sys.stdout.isatty(), bool)
assert isinstance(sys.stderr.isatty(), bool)
assert sys.stdout.write("stdout π\n") == 9
assert sys.stdout.flush() is None
assert original.write("stderr 🌍\n") == 9
assert original.flush() is None

recorder = Recorder()
sys.stderr = recorder
from sys import stderr

assert stderr is recorder
assert stderr.write("redirected") == 10
assert recorder.text == "redirected"
assert sys.__stderr__ is original
sys.stderr = original
assert sys.stderr.write("restored\n") == 9

buffer = sys.stdout.buffer
assert buffer is sys.__stdout__.buffer
assert buffer is not sys.stderr.buffer
assert buffer.fileno() == 1 and sys.stderr.buffer.fileno() == 2
assert buffer.isatty() == os.isatty(buffer.fileno())
assert buffer.writable() and not buffer.readable()
assert buffer.name == "<stdout>" and buffer.mode == "wb"
assert sys.stdout.write("text") == 4
assert buffer.write(memoryview(b"raw:")) == 4
assert buffer.flush() is None
assert buffer.write(b"then:") == 5
assert sys.stdout.write("\n") == 1
assert sys.stdout.flush() is None

err_buffer = original.buffer
assert original.flush() is None
assert original.write("pending") == 7
assert err_buffer.write(b"binary stderr\n") == 14
assert err_buffer.close() is None
assert err_buffer.closed and original.closed
try:
    err_buffer.isatty()
except ValueError:
    pass
else:
    raise AssertionError("closed standard buffer returned terminal status")
assert sys.__stderr__.buffer is err_buffer
assert err_buffer.close() is None
try:
    err_buffer.write(b"x")
except ValueError:
    pass
else:
    raise AssertionError("closed standard buffer accepted a write")

# A failed close must not replay these bytes when the process flushes at exit.
saved = os.dup(1)
assert buffer.write(b"never replay") == 12
os.close(1)
try:
    if sys.argv[1:] == ["close-text"]:
        sys.stdout.close()
    else:
        buffer.close()
except OSError as error:
    assert error.errno == 9
else:
    raise AssertionError("closed descriptor accepted a flush")
finally:
    os.dup2(saved, 1)
    os.close(saved)
assert sys.stdout.closed and buffer.closed
