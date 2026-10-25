"""Run unchanged CPython timed handler routines against persistent native handlers."""
import argparse
import ast
import os
from contextlib import ExitStack
from pathlib import Path
import subprocess
import sys
import tempfile
from types import SimpleNamespace
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
STDLIB = HERE.parents[1]
sys.path.insert(0, str(STDLIB))
from upstream import verify_upstream


DAILY_FIXTURE = '''import datetime
import logging.handlers
import sys

class Case:
    fn: str

    def __init__(self, filename: str) -> None:
        self.fn = filename

    def assertEqual(self, actual: int, expected: int) -> None:
        if actual != expected:
            raise AssertionError("rollover time differs")

'''


class Driver:
    def __init__(self, binary, stack):
        self.process = None
        self.binary = binary
        self.stack = stack
        self.operations = 0

    def send(self, command, *args):
        if self.process is None:
            # Timezone decorators set TZ inside the unchanged method. Start
            # the native process under precisely that environment.
            self.process = self.stack.enter_context(subprocess.Popen(
                [self.binary], stdin=subprocess.PIPE, stdout=subprocess.PIPE, encoding="utf-8"))
            self.stack.callback(self.stop)
        self.operations += 1
        lines = [str(arg) for arg in (command, *args)]
        if any("\n" in line or "\r" in line for line in lines):
            raise ValueError("unexpected multiline argument in test transport")
        self.process.stdin.write("\n".join(lines) + "\n")
        self.process.stdin.flush()
        reply = self.process.stdout.readline()
        for kind in (ValueError, OverflowError):
            if reply.startswith(kind.__name__ + ":"):
                raise kind(ast.literal_eval(reply.split(":", 1)[1]))
        return ast.literal_eval(reply)

    def stop(self):
        if self.process.poll() is None:
            self.process.stdin.write("stop\n")
            self.process.stdin.close()
        if self.process.wait(timeout=10):
            raise RuntimeError(f"native handler driver exited {self.process.returncode}")

    def new(self, filename, when="h", interval=1, backupCount=0, encoding=None,
            delay=False, utc=False, atTime=None, errors=None):
        if encoding not in (None, "utf-8") or errors is not None:
            raise ValueError("unsupported test transport encoding")
        clock = [0] if atTime is None else [
            1, atTime.hour, atTime.minute, atTime.second, atTime.microsecond, atTime.fold]
        handle = self.send("new", filename, when, interval, backupCount, int(delay), int(utc), *clock)
        return SimpleNamespace(
            computeRollover=lambda now: self.send("int" if isinstance(now, int) else "float", handle, now),
            getFilesToDelete=lambda: self.send("deletions", handle),
            close=lambda: self.send("close", handle))


def main():
    if sys.implementation.name != "cpython" or sys.version_info[:3] != (3, 14, 5) or not __debug__:
        raise SystemExit("run with CPython 3.14.5 and assertions enabled")
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("imre", "runtime", "clang"):
        parser.add_argument("--" + name, required=True, type=Path)
    parser.add_argument("--opt-level", choices=("0", "1", "2", "3"), default="3")
    parser.add_argument("--cpython-source", type=Path)
    parser.add_argument("--sanitize", action="store_true")
    args = parser.parse_args()
    verify_upstream()
    if args.cpython_source:
        sys.path.insert(0, str(args.cpython_source.resolve() / "Lib"))
    import test
    vendored = STDLIB / "upstream/Lib/test"
    test.__path__.insert(0, str(vendored))
    from test import test_logging
    assert Path(test_logging.__file__).resolve() == vendored / "test_logging.py"
    cls = test_logging.TimedRotatingFileHandlerTest
    names = sorted(n for n in dir(cls) if n.startswith("test_compute_rollover_"))
    names += ["test_invalid", "test_compute_files_to_delete_same_filename_different_extensions"]
    with tempfile.TemporaryDirectory() as work:
        def build(source):
            binary = Path(work) / source.stem
            subprocess.run([args.imre.resolve(), "build", source, "--no-cache",
                            "--opt-level", args.opt_level, "--runtime", args.runtime.resolve(),
                            "--clang", args.clang.resolve(), "-o", binary,
                            *(["--sanitize"] if args.sanitize else [])], cwd=work, check=True, timeout=120)
            return binary

        lifecycle = HERE / "timed_lifecycle.py"
        logdir = Path(work) / "files"
        logdir.mkdir()
        logfile = logdir / "application.log"
        def rotate(command):
            logfile.write_bytes(b"previous contents\n")
            os.utime(logfile, (42, 42))
            logfile.with_suffix(".log.1969-12-31_23-59-59").touch()
            run = subprocess.run([*command, logfile], capture_output=True, timeout=30)
            if run.returncode:
                raise RuntimeError(run.stderr.decode())
            files = {p.name: p.read_bytes() for p in logdir.iterdir()}
            for p in logdir.iterdir():
                p.unlink()
            return run.stdout, run.stderr, files
        assert rotate([build(lifecycle)]) == rotate([sys.executable, lifecycle])
        binary = build(HERE / "timed_driver.py")
        # This complete body only needs a filename and integer equality. Compile
        # it directly as well as running it with the original host fixture.
        source = (vendored / "test_logging.py").read_text()
        node = next(n for n in ast.parse(source).body if isinstance(n, ast.ClassDef)
                    and n.name == cls.__name__)
        name = "test_compute_rollover_daily_attime"
        node = next(n for n in node.body if isinstance(n, ast.FunctionDef) and n.name == name)
        body = "".join(source.splitlines(keepends=True)[node.lineno - 1:node.end_lineno])
        daily = Path(work) / "daily.py"
        daily.write_text(DAILY_FIXTURE + body + f"\nCase(sys.argv[1]).{name}()\n")
        subprocess.run([build(daily), Path(work) / "daily.log"], cwd=work, check=True, timeout=30)
        print("native body and operations", f"test.test_logging.{cls.__name__}.{name}")
        for name in names:
            with ExitStack() as stack:
                driver = Driver(binary, stack)
                native = SimpleNamespace(handlers=SimpleNamespace(TimedRotatingFileHandler=driver.new))
                case = cls(name)
                method = getattr(case, name)
                def check():
                    with patch.object(test_logging, "logging", native):
                        method()
                setattr(case, name, check)
                result = case.run()
                if not result.wasSuccessful() or result.skipped or not driver.operations:
                    raise AssertionError((case.id(), result.errors, result.failures, result.skipped))
                print("host-driven native operations", case.id(), driver.operations)


if __name__ == "__main__":
    main()
