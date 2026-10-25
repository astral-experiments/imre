"""Check NullHandler and its real RecordingHandler override from test_logging."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile

HERE = Path(__file__).resolve().parent
STDLIB = HERE.parents[1]
sys.path.insert(0, str(STDLIB))
from suite_inventory import verify_files
from suite_native import Toolchain, method_bytes


def sample():
    manifest = json.loads((HERE / "UPSTREAM.json").read_text())
    verify_files(STDLIB / "upstream", manifest["files"])
    method = method_bytes(STDLIB / "upstream/Lib/test/test_logging.py", "RecordingHandler", "handle")
    imports = b"from logging import Filter, Handler, LogRecord, NullHandler, getHandlerByName, makeLogRecord\n"
    records = b"""
first = makeLogRecord({"name": "first", "msg": "one"})
second = makeLogRecord({"name": "second", "msg": "two"})
"""
    recording = imports + b"""

class RecordingHandler(NullHandler):
    records: list[LogRecord]

    def __init__(self) -> None:
        super().__init__()
        self.records = []

""" + method + records + b"""
recording = RecordingHandler()
saved = recording.records
recording.handle(second)
recording.handle(first)
print(saved is recording.records)
print(len(saved))
print(saved[0] is second)
print(saved[1] is first)
"""
    base = imports + b"""
class CountingEmitter(NullHandler):
    calls: int

    def __init__(self) -> None:
        super().__init__()
        self.calls = 0

    def emit(self, record: LogRecord | None) -> None:
        self.calls += 1

created: list[int] = []

class ObservedLock(NullHandler):
    def createLock(self) -> None:
        created.append(self.level)
        if self.level == 9:
            raise RuntimeError("lock")
        super().createLock()

observed = ObservedLock("WARNING")
print(observed.lock is None)
try:
    ObservedLock(9)
except RuntimeError as error:
    print(str(error))
print(repr(created))

handler = NullHandler("WARNING")
print(isinstance(handler, Handler))
print(handler.level)
print(handler.lock is None)
print(handler.createLock() is None)
print(handler.lock is None)
print(handler._at_fork_reinit() is None)
handler.name = "library"
print(getHandlerByName("library") is handler)
""" + records + b"""
handler.filters.append(Filter("different"))
print(handler.handle(first) is None)
print(handler.emit(second) is None)
print(handler.handle(None) is None)
print(handler.format(first))
handler.close()
print(handler._closed)
print(getHandlerByName("library") is None)
emitter = CountingEmitter()
emitter.handle(first)
print(emitter.calls)
emitter.emit(first)
print(emitter.calls)
"""
    return {"null": base, "recording": recording, "locks": (HERE / "lock_values.py").read_bytes(),
            "buffers": (HERE / "buffer_values.py").read_bytes()}, method


def output(command, cwd):
    child = subprocess.run(command, cwd=cwd, capture_output=True, timeout=30, check=True)
    return child.stdout, child.stderr


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("imre", "runtime", "clang", "result"):
        parser.add_argument("--" + name, required=True, type=Path)
    args = parser.parse_args()
    if sys.implementation.name != "cpython" or sys.version_info[:3] != (3, 14, 5) or not __debug__:
        parser.error("requires CPython 3.14.5 with assertions enabled")
    programs, method = sample()
    levels = {"0": {}, "3": {}}
    with tempfile.TemporaryDirectory() as temp:
        work = Path(temp)
        reference = work / "reference.py"
        for name, entry in programs.items():
            reference.write_bytes(entry)
            expected = output([sys.executable, "-I", "-B", reference], work)
            for level in (0, 3):
                tool = Toolchain(args.imre, args.runtime, args.clang, level)
                actual = output([tool.build(entry, work)], work)
                if actual != expected:
                    raise AssertionError((name, level, expected, actual))
                levels[str(level)][name] = actual[0].decode().splitlines()
    report = {"source": "Lib/test/test_logging.py:RecordingHandler.handle",
              "method_sha256": hashlib.sha256(method).hexdigest(), "levels": levels,
              "logging_occurrences": 0}
    args.result.write_text(json.dumps(report, indent=2) + "\n")
    print("Handler locks, buffers, NullHandler and unchanged RecordingHandler.handle match at O0/O3; zero test IDs")


if __name__ == "__main__":
    main()
