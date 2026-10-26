"""Run both sides of the original plain UTF-8 FileHandler test under its real BaseTest."""
import argparse
import ast
from contextlib import ExitStack, contextmanager
from functools import cache
import hashlib
import json
from pathlib import Path
import re
import select
import subprocess
import sys
import tempfile
from types import SimpleNamespace
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
sys.path[:0] = [str(HERE), str(HERE.parents[1])]
from native_encoding import FIXTURE, SOURCE, base_fixture, stock_case
from suite_evidence import roles
from suite_inventory import load_suite, verify_files
from suite_native import NativeCompileError, Toolchain, method_bytes, native_method, run_in_fixtures

CASE = 'test.test_logging.EncodingTest.test_encoding_plain_file'
BODY = method_bytes(SOURCE, 'EncodingTest', 'test_encoding_plain_file')
OP = 'Native named logger, UTF-8 FileHandler construction, attachment, warning, removal and close'
TEMP_REQUEST = '''
def make_temp_file(suffix: str, prefix: str) -> str:
    print("make_temp_file")
    print(suffix)
    print(prefix)
    sys.stdout.flush()
    return input()
'''


@cache
def helper():
    data = SOURCE.read_bytes()
    node, = (n for n in ast.parse(data).body if isinstance(n, ast.FunctionDef) and n.name == 'make_temp_file')
    return b''.join(data.splitlines(keepends=True)[node.lineno-1:node.end_lineno]).decode()


class TemporaryFile:
    """Retain a read descriptor to the actual upstream inode after its name is removed."""
    def __init__(self, stack, factory):
        self.stack = stack
        self.factory = factory
        self.path = self.reader = None

    def create(self, *args, **kwargs):
        if self.path is not None:
            raise AssertionError('unexpected second temporary file')
        name = self.factory(*args, **kwargs)
        self.path = Path(name)
        self.stack.callback(self.path.unlink, missing_ok=True)
        self.reader = self.stack.enter_context(self.path.open('rb'))
        return name

    def contents(self):
        if self.reader is None or self.path.exists():
            raise AssertionError('the original test did not create and remove its file')
        return self.reader.read()


@cache
def reference_bytes():
    from test import test_logging

    def run(case, method):
        with ExitStack() as stack:
            file = TemporaryFile(stack, test_logging.make_temp_file)
            with patch.object(test_logging, 'make_temp_file', file.create):
                method()
            return file.contents().hex()

    found, failures, skips = run_in_fixtures(load_suite('test.test_logging'), {CASE:run})
    if failures or skips:
        raise AssertionError((failures, skips))
    return bytes.fromhex(found[CASE][0])


@contextmanager
def child_process(binary):
    with subprocess.Popen([binary], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                          stderr=subprocess.PIPE, encoding='utf-8') as child:
        try:
            yield child
        finally:
            if child.poll() is None:
                child.kill()


def send(child, *lines):
    if any('\n' in line or '\r' in line for line in lines):
        raise ValueError('a fixture argument is not a single line')
    child.stdin.write('\n'.join(lines) + '\n')
    child.stdin.flush()


def read_line(child):
    if not select.select([child.stdout], [], [], 30)[0]:
        raise AssertionError('timed out waiting for native file logging')
    answer = child.stdout.readline()
    if not answer:
        raise AssertionError((child.poll(), child.stderr.read()))
    return answer.removesuffix('\n')


def location(exc, folder):
    match = re.search(r'case\.py:(\d+):(\d+): Imre: ([^\n]+)', str(exc))
    if not match:
        raise exc
    line = (folder/'case.py').read_text().splitlines()[int(match[1])-1].strip()
    origins = [i for i, text in enumerate(SOURCE.read_text().splitlines(), 1)
               if text.strip() == line and (594 <= i <= 597 or 2276 <= i <= 2299)]
    if len(origins) != 1:
        raise NativeCompileError(f'{exc}\nGenerated adapter statement: {line}') from exc
    origin, = origins
    return {'source': f'Lib/test/test_logging.py:{origin}:{match[2]}',
            'statement': line, 'diagnostic': match[3]}


def plain_case(tools, folder):
    # Finish the independent oracle run before the native case enters BaseTest.
    expected = reference_bytes()

    def execute(case, original):
        from test import test_logging
        fixture = base_fixture(case) + 'import os\n'
        body_sha = hashlib.sha256(BODY).hexdigest()
        blockers = []
        try:
            complete = native_method(tools, folder, SOURCE, 'EncodingTest', CASE,
                                     fixture+'import tempfile\n'+helper()+'\n'+stock_case(),
                                     operation=('imre', OP))
        except NativeCompileError as exc:
            where = location(exc, folder)
            if where['statement'] not in {'def make_temp_file(*args, **kwargs):',
                                          'fd, fn = tempfile.mkstemp(*args, **kwargs)'}:
                raise
            blockers.append(where)
        else:
            result = complete(case, original)
            result['observation']['evidence'].update(fixture=FIXTURE, reference_bytes_hex=expected.hex())
            return result

        # The native body may request the real upstream mkstemp/close from the host.
        # Keep its original file read, assertion and final removal in that attempt.
        prefix = (fixture+'import sys\n'+TEMP_REQUEST+'\n'+stock_case()).encode()
        entry = prefix+b'\n'+BODY+b'\ncase = NativeCase()\ncase.test_encoding_plain_file()\nprint("completed")\n'
        try:
            binary = tools.build(entry, folder)
        except NativeCompileError as exc:
            where = location(exc, folder)
            if where['statement'] not in {'if os.path.isfile(fn):', 'self.assertEqual(f.read().rstrip(), data)'}:
                raise
            blockers.append(where)
        else:
            with ExitStack() as stack, child_process(binary) as child:
                file = TemporaryFile(stack, test_logging.make_temp_file)
                # The three request lines can already be in Python's text buffer.
                if read_line(child) != 'make_temp_file':
                    raise AssertionError('native method did not request the original file helper')
                suffix, prefix = (child.stdout.readline().removesuffix('\n') for _ in range(2))
                send(child, file.create(suffix,prefix))
                out, err = child.communicate(timeout=30)
                if child.returncode or err or out != 'completed\n':
                    raise AssertionError((child.returncode,out,err))
                actual = file.contents()
            if actual != expected:
                raise AssertionError((actual, expected))
            return {'observation': {'roles': roles(body='imre', assertions='imre', operation='imre', fixture='mixed'),
                    'evidence': {'operation': OP, 'fixture': FIXTURE+'; host ran the original temporary-file helper on request',
                                 'opt_level': tools.level, 'method_sha256': body_sha, 'file_hex': actual.hex(),
                                 'reference_bytes_hex': expected.hex(), 'native_body_blockers': blockers}}}

        binary = tools.build((HERE/'plain_file_driver.py').read_bytes(), folder)
        calls = []
        with ExitStack() as stack, child_process(binary) as child:
            file = TemporaryFile(stack, test_logging.make_temp_file)
            send(child, str(case.root_logger.getEffectiveLevel()), case.log_format)

            def call(op, *args):
                send(child, op, *args)
                if read_line(child) != 'ok':
                    raise AssertionError(op)
                calls.append(op)

            handle = SimpleNamespace(close=lambda: call('close'))
            def get_logger(name):
                call('getLogger', name)
                def change(op, target):
                    if target is not handle:
                        raise AssertionError('unknown file handle')
                    call(op)
                return SimpleNamespace(addHandler=lambda h: change('addHandler',h),
                                       warning=lambda message: call('warning',message),
                                       removeHandler=lambda h: change('removeHandler',h))

            def file_handler(filename, *, encoding):
                call('FileHandler', filename, encoding)
                return handle

            with patch.object(test_logging, 'make_temp_file', file.create), \
                 patch.object(test_logging, 'logging', SimpleNamespace(getLogger=get_logger, FileHandler=file_handler)):
                original()
            out, err = child.communicate(timeout=30)
            if child.returncode or out or err:
                raise AssertionError((child.returncode,out,err))
            actual = file.contents()
        if actual != expected:
            raise AssertionError((actual,expected))
        return {'observation': {'roles': roles(body='cpython', assertions='cpython', operation='imre', fixture='mixed'),
                'evidence': {'operation': OP, 'fixture': FIXTURE+'; CPython ran the original temporary-file helper, read, assertion and cleanup',
                             'opt_level':tools.level, 'method_sha256':body_sha, 'native_calls':calls,
                             'file_hex':actual.hex(), 'reference_bytes_hex':expected.hex(), 'native_body_blockers':blockers}}}
    return execute


def main():
    p = argparse.ArgumentParser(description=__doc__)
    for key in ('imre','runtime','clang','cpython-source','result'):
        p.add_argument('--'+key, type=Path, required=True)
    args = p.parse_args()
    if sys.implementation.name != 'cpython' or sys.version_info[:3] != (3,14,5) or not __debug__:
        p.error('use the pinned CPython 3.14.5 test interpreter')
    sys.path.insert(0,str(args.cpython_source.resolve()/'Lib'))
    manifest = json.loads((HERE/'UPSTREAM.json').read_text())
    verify_files(HERE.parents[1]/'upstream', manifest['files'])
    verify_files(args.cpython_source, manifest['files'] | manifest['reference_files'])
    from test import test_logging
    if Path(test_logging.__file__).read_bytes() != SOURCE.read_bytes():
        raise AssertionError('loaded test_logging differs from pinned source')
    report = {'cpython_commit':manifest['commit'], 'denominator':272, 'reference_bytes_hex':reference_bytes().hex(), 'levels':{}}
    for level in (0,3):
        with tempfile.TemporaryDirectory() as tmp:
            tools = Toolchain(args.imre,args.runtime,args.clang,level)
            records, errors, skips = run_in_fixtures(load_suite(manifest['module']),{CASE:plain_case(tools,Path(tmp))})
            if errors or skips or set(records)!={CASE}:
                raise AssertionError((errors,skips))
            report['levels'][str(level)] = records[CASE][0]['observation']
    args.result.write_text(json.dumps(report,indent=2)+'\n')
    print(CASE, report['reference_bytes_hex'], 'at O0 and O3')


if __name__ == '__main__':
    main()
