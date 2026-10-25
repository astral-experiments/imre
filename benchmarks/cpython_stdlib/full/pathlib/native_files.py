"""Run whole upstream local-filesystem methods with the shared unittest fixture runner."""

import argparse
from collections import Counter
from contextlib import chdir
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile

BENCHMARK = Path(__file__).resolve().parents[2]
sys.path.append(str(BENCHMARK))
from suite_inventory import load_suite, verify_interpreter_tests
from suite_native import Toolchain, method_bytes, native_method, run_in_fixtures
from upstream import verify_upstream

PACKAGE = BENCHMARK / 'upstream/Lib/test/test_pathlib'
READ = 'test.test_pathlib.test_read.PathReadTest.'
WRITE = 'test.test_pathlib.test_write.PathWriteTest.'

# The exact upstream LocalPathGround.create_file uses a binary stream. This native
# fixture receives the directory made by the original CPython setup for this ID.
def read_fixture(root, equality):
    return f"""from pathlib import Path

class Ground:
    def create_file(self, path: Path, data: bytes) -> None:
        with open(str(path), 'wb') as f:
            f.write(data)

class NativeCase:
    root: Path
    ground: Ground
    def __init__(self) -> None:
        self.root = Path({root!r})
        self.ground = Ground()
    def assertEqual(self, first: {equality}, second: {equality}) -> None:
        if first != second:
            raise AssertionError("values differ")
"""


def native_read(tools, work, name, equality):
    def execute(case, original):
        # Compilation happens inside the original fixture: an arbitrary replacement
        # tree cannot accidentally make read_bytes or read_text pass.
        directory = work / name
        directory.mkdir()
        run = native_method(tools, directory, PACKAGE / 'test_read.py',
                            'ReadTestBase', READ + name,
                            read_fixture(os.path.abspath(os.fspath(case.root)), equality))
        return run(case, original)
    return execute


# The host calls the original write methods, including callable-form assertRaises.
# Every join, write and wrong-type call crosses into this native binary. The
# upstream ground reads the bytes independently through its own ordinary binary file.
DRIVER = b"""from pathlib import Path
import sys

def optional(value: str) -> str | None:
    if value == '0':
        return None
    return value[1:]

def run(args: list[str]) -> None:
    path = Path(args[2])
    command = args[1]
    if command == 'join':
        print(str(path / args[3]))
        return
    with open(args[3], 'rb') as source:
        data = source.read()
    if command == 'write_bytes':
        print(path.write_bytes(data))
    elif command == 'write_bytes_str':
        print(path.write_bytes(data.decode()))
    elif command == 'write_text' or command == 'write_text_bytes':
        encoding = optional(args[4])
        errors = optional(args[5])
        newline = optional(args[6])
        if command == 'write_text':
            print(path.write_text(data.decode(), encoding, errors, newline))
        else:
            print(path.write_text(data, encoding, errors, newline))
    else:
        raise RuntimeError('unknown request')

try:
    run(sys.argv)
except TypeError as error:
    print('TypeError:' + str(error))
"""


class NativePath:
    def __init__(self, spelling, binary, payload, calls):
        self.spelling = spelling
        self.binary = binary
        self.payload = payload
        self.calls = calls

    def __fspath__(self):
        return self.spelling

    def call(self, operation, *args):
        out = subprocess.check_output([self.binary, operation, self.spelling, *args],
                                      timeout=15).decode().removesuffix('\n')
        self.calls[operation] += 1
        if out.startswith('TypeError:'):
            raise TypeError(out.removeprefix('TypeError:'))
        return out

    def __truediv__(self, other):
        return NativePath(self.call('join', other), self.binary, self.payload, self.calls)

    def write_bytes(self, data):
        with open(self.payload, "wb") as scratch:
            scratch.write(data.encode() if isinstance(data, str) else data)
        command = 'write_bytes_str' if isinstance(data, str) else 'write_bytes'
        return int(self.call(command, self.payload))

    def write_text(self, data, encoding=None, errors=None, newline=None):
        with open(self.payload, "wb") as scratch:
            scratch.write(data.encode() if isinstance(data, str) else data)
        command = 'write_text' if isinstance(data, str) else 'write_text_bytes'
        options = ['0' if value is None else '1' + value for value in (encoding, errors, newline)]
        return int(self.call(command, self.payload, *options))


def host_write(binary, payload, name):
    def execute(case, original):
        root = case.root
        calls = Counter()
        case.root = NativePath(os.fspath(root), binary, payload, calls)
        try:
            original()
        finally:
            case.root = root
        if not any(key.startswith('write_') for key in calls):
            raise AssertionError('upstream method executed no native write')
        body = method_bytes(PACKAGE / 'test_write.py', 'WriteTestBase', name)
        return {'execution': 'host-driven-native-operations', 'body': 'CPython',
                'assertions': 'CPython', 'fixture_execution': 'CPython upstream hooks and ground',
                'native_calls': dict(calls), 'method_sha256': hashlib.sha256(body).hexdigest()}
    return execute


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for arg in ('imre', 'runtime', 'clang', 'result'):
        parser.add_argument('--' + arg, required=True, type=Path)
    parser.add_argument('--cpython-source', type=Path)
    parser.add_argument('--opt-level', type=int, choices=(0, 3), required=True)
    args = parser.parse_args()
    if os.name != 'posix' or sys.version_info[:3] != (3, 14, 5):
        parser.error('use the test-enabled POSIX CPython 3.14.5')
    if args.cpython_source:
        sys.path.insert(0, str(args.cpython_source.resolve() / 'Lib'))
    commit = verify_upstream()
    manifest = json.loads((BENCHMARK / 'UPSTREAM.json').read_text())
    verify_interpreter_tests({'files': {name: blob for name, blob in manifest['files'].items()
                                         if name.startswith('Lib/test/test_pathlib/')}})
    tools = Toolchain(args.imre.resolve(), args.runtime.resolve(), args.clang.resolve(), args.opt_level)
    with tempfile.TemporaryDirectory(prefix='pl-') as tmp, chdir(tmp):
        work = Path(tmp)
        native = {READ + name: native_read(tools, work, name, equality) for name, equality in (
            ('test_read_bytes', 'bytes'), ('test_read_text', 'str'), ('test_read_text_with_newlines', 'str'))}
        binary = tools.build(DRIVER, work)
        for name in ('test_write_bytes', 'test_write_text', 'test_write_text_with_newlines'):
            native[WRITE + name] = host_write(binary, work / 'payload.bin', name)
        evidence, failures, skips = run_in_fixtures(load_suite('test.test_pathlib'), native)
    report = {'cpython_commit': commit, 'opt_level': args.opt_level, 'evidence': evidence,
              'failures': failures, 'skips': skips}
    args.result.write_text(json.dumps(report, indent=2) + '\n')
    if failures or skips or set(evidence) != set(native) or any(len(v) != 1 for v in evidence.values()):
        raise AssertionError((failures, skips, set(native) - set(evidence)))
    print(f'{len(evidence)} complete methods; 3 native bodies with host setup, 3 host-driven')


if __name__ == '__main__':
    main()
