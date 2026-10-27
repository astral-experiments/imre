import sys


class Recorder:
    def write(self, text):
        self.text = text
        return len(text)


original = sys.stderr
assert original is sys.__stderr__
assert sys.stdout is sys.__stdout__
assert sys.stdout is not original
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
