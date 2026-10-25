"""Drive CPython's original stored and deflated CRC tests with native ZIP operations."""
import argparse
import ast
import io
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest import mock
import zipfile

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
from upstream import verify_upstream

DRIVER = '''import io
import sys
from pathlib import Path
from zipfile import BadZipFile, ZipFile

with io.BytesIO(Path(sys.argv[1]).read_bytes()) as buffer:
    with ZipFile(buffer) as archive:
        try:
            if sys.argv[2] == "testzip":
                result = archive.testzip()
                print("name " + repr(result))
            elif sys.argv[2] == "read":
                print("data " + repr(archive.read(sys.argv[3])))
            else:
                with archive.open(sys.argv[3], "r") as member:
                    if sys.argv[2] == "all":
                        print("data " + repr(member.read()))
                    else:
                        member.MIN_READ_SIZE = 2
                        while True:
                            part = member.read(2)
                            print("data " + repr(part))
                            if not part:
                                break
        except BadZipFile as error:
            print("bad " + str(error))
'''

# This also tests independent handles after archive.close(), direct use of
# ZipInfo for duplicate names, and that ZipFile does not own a passed BinaryIO.
CONTROL = '''import io
import sys
from pathlib import Path
from zipfile import ZipFile

def check(archive: ZipFile) -> None:
    entries = archive.infolist()
    for item in entries:
        print(repr((item.filename, item.file_size, item.compress_size, item.CRC, item.compress_type, item.external_attr, item.is_dir())))
    print(repr(archive.read(entries[0])))
    print(repr(archive.read(entries[1])))
    print(repr(archive.read("duplicate")))
    print(repr(archive.testzip()))
    with archive.open(entries[0]) as first:
        with archive.open(archive.getinfo("duplicate")) as second:
            print(repr(first.read(2)))
            print(repr(second.read(1)))
            archive.close()
            print(repr(first.read()))
            print(repr(second.read(3)))

path = Path(sys.argv[1])
with ZipFile(path) as disk:
    print(repr(disk.filename))
    check(disk)
with open(sys.argv[1], "rb") as file:
    with ZipFile(file) as wrapped:
        check(wrapped)
    print(file.seek(0))
    print(repr(file.read(4)))
    print(file.tell())
with io.BytesIO(path.read_bytes()) as mem:
    with ZipFile(mem) as buffered:
        print(repr(buffered.filename))
        check(buffered)
    print(mem.seek(-3, 2))
    print(repr(mem.read()))
    print(mem.tell())
with io.BytesIO(path.read_bytes()) as source:
    with ZipFile(source) as archive:
        with archive.open(archive.infolist()[0]) as member:
            member.MIN_READ_SIZE = 2
            source.close()
            for _ in range(2):
                try:
                    print(repr(member.read(1)))
                except ValueError:
                    print("closed source " + str(member.tell()))
'''


ERRORS = '''import sys
from zipfile import ZipFile, BadZipFile
from zlib import error as ZlibError

try:
    with ZipFile(sys.argv[1]) as archive:
        if sys.argv[2] == "missing":
            archive.getinfo(name="absent")
        elif sys.argv[2] == "closed":
            archive.close()
            archive.read(name="data")
        else:
            with archive.open(mode="r", name="data") as member:
                print(repr(member.read()))
except BadZipFile as error:
    print("BadZipFile " + str(error))
except KeyError as error:
    print("KeyError " + str(error))
except ValueError as error:
    print("ValueError " + str(error))
except EOFError as error:
    print("EOFError " + str(error))
except ZlibError as error:
    print("zlib.error " + str(error))
'''


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("imre", "runtime", "clang", "result"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--cpython-source", type=Path)
    parser.add_argument("--opt-level", choices=("0", "3"), default="3")
    args = parser.parse_args()
    if sys.implementation.name != "cpython" or sys.version_info[:3] != (3, 14, 5):
        parser.error("this gate requires CPython 3.14.5")
    if args.cpython_source:
        sys.path.insert(0, str(args.cpython_source.resolve() / "Lib"))
    import test
    test.__path__.insert(0, str(HERE / "upstream/Lib/test"))
    from test.test_zipfile import test_core as suite
    assert Path(suite.__file__).resolve() == HERE / "upstream/Lib/test/test_zipfile/test_core.py"
    report = {"cpython_commit": verify_upstream(), "opt_level": args.opt_level, "tests": {}}
    with tempfile.TemporaryDirectory() as directory:
        work = Path(directory)
        def compile(name, source):
            path, executable = work / (name + ".py"), work / name
            path.write_text(source)
            subprocess.run([args.imre.resolve(), "build", path, "--opt-level", args.opt_level, "--no-cache",
                            "--runtime", args.runtime.resolve(), "--clang", args.clang.resolve(), "-o", executable],
                           check=True, timeout=120)
            return path, executable

        _, executable = compile("crc", DRIVER)
        diagnostic = []
        def invoke(data, mode, name=""):
            payload = work / "input"
            payload.write_bytes(data)
            child = subprocess.run([executable, payload, mode, name], check=True, text=True, capture_output=True, timeout=30)
            if child.stderr:
                raise AssertionError(child.stderr)
            events = [line.split(" ", 1) for line in child.stdout.splitlines()]
            diagnostic.append((mode, [event[0] for event in events]))
            return iter(events)

        def decode(event, tag):
            kind, detail = event
            if kind == "bad":
                raise zipfile.BadZipFile(detail)
            assert kind == tag, event
            return ast.literal_eval(detail)

        class Context:
            def __enter__(self):
                return self
            def __exit__(self, *args):
                return False

        class Member(Context):
            MIN_READ_SIZE = None
            def __init__(self, data, name):
                self.data, self.name, self.events = data, name, None
            def read(self, size=None):
                if self.events is None:
                    mode = "all" if size is None else "chunks"
                    assert size is None or (size, self.MIN_READ_SIZE) == (2, 2)
                    self.events = invoke(self.data, mode, self.name)
                return decode(next(self.events), "data")

        class Archive(Context):
            def __init__(self, buffer, mode):
                assert isinstance(buffer, io.BytesIO) and mode == "r"
                self.data = buffer.getvalue()
            def testzip(self):
                return decode(next(invoke(self.data, "testzip")), "name")
            def read(self, name):
                return decode(next(invoke(self.data, "read", name)), "data")
            def open(self, name, mode):
                assert mode == "r"
                return Member(self.data, name)

        for cls in (suite.StoredBadCrcTests, suite.DeflateBadCrcTests):
            for method in ("test_testzip_with_bad_crc", "test_read_with_bad_crc"):
                diagnostic.clear()
                case, outcome = cls(method), unittest.TestResult()
                with mock.patch.object(suite.zipfile, "ZipFile", Archive):
                    case.run(outcome)
                expected = ["testzip"] if method.startswith("test_testzip") else ["read", "all", "chunks"]
                if not outcome.wasSuccessful() or outcome.skipped or [m for m, _ in diagnostic] != expected:
                    raise AssertionError((case.id(), outcome.errors, outcome.failures, outcome.skipped, diagnostic))
                report["tests"][case.id()] = list(diagnostic)
                print("pass", case.id())
        control, binary = compile("control", CONTROL)
        fixture = work / "control.zip"
        import warnings
        with warnings.catch_warnings(), zipfile.ZipFile(fixture, "w") as writer:
            warnings.simplefilter("ignore", UserWarning)
            writer.writestr("duplicate", b"first-stored", compress_type=zipfile.ZIP_STORED)
            writer.writestr("duplicate", b"second-deflated", compress_type=zipfile.ZIP_DEFLATED)
            writer.writestr("subdir/", b"")
        for cmd in ([sys.executable, control], [binary]):
            result = subprocess.run([*cmd, fixture], capture_output=True, timeout=30)
            if len(cmd) == 2:
                reference = result
                assert reference.returncode == 0 and not reference.stderr, reference
            else:
                assert (result.stdout, result.stderr, result.returncode) == (reference.stdout, reference.stderr, 0), result
        errors, errors_binary = compile("errors", ERRORS)
        good = work / "deflated.zip"
        with zipfile.ZipFile(good, "w", compression=zipfile.ZIP_DEFLATED) as writer:
            writer.writestr("data", b"test member" * 256)
        original = good.read_bytes()
        corrupt = work / "corrupt.zip"
        offset = 30 + len("data")  # first local header has no extra field
        corrupt.write_bytes(original[:offset] + b"\x07" + original[offset + 1:])
        truncated = work / "truncated.zip"
        truncated.write_bytes(original[:-10])
        for file, action, tag in [(good, "missing", "KeyError"), (good, "closed", "ValueError"),
                                  (truncated, "read", "BadZipFile"), (corrupt, "read", "zlib.error")]:
            def check(command):
                proc = subprocess.run([*command, file, action], capture_output=True, text=True, timeout=30)
                assert not proc.returncode and not proc.stderr, proc
                return proc.stdout
            expected = check([sys.executable, errors])
            assert expected.startswith(tag + " ") and check([errors_binary]) == expected, (action, expected)
        report["error_controls"] = ["missing member", "closed archive", "truncated directory", "damaged DEFLATE payload"]
        report["controls"] = ["stored and deflated duplicates", "independent readers survive parent close",
                              "path, open file and BytesIO", "seek/tell on caller-owned binary inputs"]
    args.result.write_text(json.dumps(report, indent=2) + "\n")


if __name__ == "__main__":
    main()
