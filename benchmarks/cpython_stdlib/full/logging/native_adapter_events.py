"""Run unchanged LoggerAdapter context and handler methods inside their CPython fixtures."""
import argparse
import ast
import hashlib
import json
from pathlib import Path
from functools import cache
import re
import select
import subprocess
import sys
import tempfile
from types import SimpleNamespace
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
STDLIB = HERE.parents[1]
SOURCE = STDLIB / 'upstream/Lib/test/test_logging.py'
sys.path[:0] = [str(HERE), str(STDLIB)]
from suite_evidence import roles
from suite_inventory import cases, load_suite, verify_files
from suite_native import NativeCompileError, Toolchain, method_bytes, native_method, run_in_fixtures, testcase_fixture
from native_manager import manager_cases

OWNER = 'LoggerAdapterTest'
PREFIX = 'test.test_logging.' + OWNER + '.'
NAMES = ('test_extra_in_records', 'test_extra_not_merged_by_default', 'test_extra_merged',
         'test_extra_merged_log_call_has_precedence', 'test_extra_merged_without_extra', 'test_has_handlers')
FIXTURE = ('Original CPython LoggerAdapterTest setUp, root recording handler, shutdown and cleanups; '
           'fresh native root with matching threshold and default adapter. Its native Handler.emit stores the original records; CPython RecordingHandler stores them in handle.')
OPERATION = 'Native adapter creation, process, record forwarding and live record attributes'
CAPTURE = (HERE / 'adapter_event_driver.py').read_text().split('def context()', 1)[0]
CAPTURE += """
def install(recording: Recording, level: int) -> logging.Logger:
    logger = logging.root
    logger.setLevel(level)
    logger.addHandler(recording)
    return logger
"""

SETUP = '''
    logger: logging.Logger
    recording: Recording
    adapter: logging.LoggerAdapter

    def __init__(self) -> None:
        super().__init__()
        self.recording = Recording()
        self.logger = install(self.recording, THRESHOLD)
        self.adapter = logging.LoggerAdapter(logger=self.logger)
'''
# The concrete instance branch of CPython TestCase's two attribute assertions.
# Always try the stock testcase_fixture first; the original test body never changes.
# Keep the shared TestCase for equality, message formatting and failure. A plain
# fixture can add both LogRecord assertions without narrowing TestCase's module assertion.
RECORD_CASE = """import unittest

class NativeCase:
    checks: unittest.TestCase

    def assertEqual(self, first: int, second: int) -> None:
        self.checks.assertEqual(first, second)

    def assertHasAttr(self, obj: logging.LogRecord, name: str, msg: str | None = None) -> None:
        if not hasattr(obj, name):
            self.checks.fail(self.checks._formatMessage(msg, "'LogRecord' object has no attribute " + repr(name)))

    def assertNotHasAttr(self, obj: logging.LogRecord, name: str, msg: str | None = None) -> None:
        if hasattr(obj, name):
            self.checks.fail(self.checks._formatMessage(msg, "'LogRecord' object has unexpected attribute " + repr(name)))
"""


def record_fixture(fixture):
    stock = testcase_fixture()
    initialize = "super().__init__()"
    before, boundary, setup = fixture.partition(stock)
    if not boundary or stock in setup or setup.count(initialize) != 1:
        raise ValueError("unexpected adapter fixture")
    return before + RECORD_CASE + setup.replace(initialize, "self.checks = unittest.TestCase()")


@cache
def helper_classes(name):
    """Keep the actual prefix/style helper definitions when the original method needs them."""
    classes = {'test_nested': ('PrefixAdapter',), 'test_styled_adapter': ('Message', 'StyleAdapter'),
               'test_nested_styled_adapter': ('PrefixAdapter', 'Message', 'StyleAdapter')}.get(name, ())
    if not classes:
        return ()
    original = SOURCE.read_bytes()
    lines = original.splitlines(keepends=True)
    nodes = {c.name: c for c in ast.parse(original).body if isinstance(c, ast.ClassDef)}
    return tuple((nodes[c].lineno, b''.join(lines[nodes[c].lineno-1:nodes[c].end_lineno]).decode()) for c in classes)


def fixture_for(name, threshold):
    helpers = ''.join(source + '\n' for _, source in helper_classes(name))
    return CAPTURE + '\n' + helpers + testcase_fixture() + SETUP.replace('THRESHOLD', str(threshold))


def helper_site(name, fixture, line):
    """Map a diagnostic in an extracted helper back to its unchanged CPython line."""
    for start, source in helper_classes(name):
        before, _, _ = fixture.partition(source)
        offset = line - len(before.splitlines()) - 1
        lines = source.splitlines()
        if 0 <= offset < len(lines):
            return start + offset, lines[offset].strip()
    return None


def assertion_blocker(exc, directory, original):
    text = str(exc)
    pos = re.search(r'case.py:(\d+):(\d+): Imre: ([^\n]+)', text)
    if not pos:
        raise exc
    statement = (directory / 'case.py').read_text().splitlines()[int(pos[1]) - 1].strip()
    if statement not in original or not statement.startswith(('self.assertHasAttr(', 'self.assertNotHasAttr(', 'self.assertEqual(')):
        raise exc
    return {'statement': statement, 'diagnostic': pos[3]}


def host_method(binary, case, original_method):
    from test import test_logging
    calls = []
    with subprocess.Popen([binary], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                          text=True, encoding='utf-8') as child:
        child.stdin.write(str(case.logger.getEffectiveLevel()) + '\n')
        def send(op, *args):
            lines = [op, *map(str, args)]
            if any('\n' in item for item in lines):
                raise ValueError('fixture used a multiline protocol operand')
            child.stdin.write('\n'.join(lines) + '\n')
            child.stdin.flush()
            if not select.select([child.stdout], [], [], 20)[0]:
                child.kill()
                raise AssertionError(f'native adapter did not answer {op}')
            value = child.stdout.readline()
            if not value:
                raise AssertionError(f'native {op}: {child.stderr.read()}')
            calls.append(op)
            return value.rstrip('\n')

        def encode(ctx):
            if ctx is None:
                return [-1]
            return [len(ctx), *(x for pair in ctx.items() for x in pair)]

        class Adapter:
            def __init__(self, logger, extra=None, *, merge_extra=False):
                assert logger is case.logger
                assert send('adapter', merge_extra, *encode(extra)) == 'ready'

            def critical(self, message, **kw):
                assert kw.keys() <= {'extra'}
                supplied = 'extra' in kw
                inputs = ['present', *encode(kw['extra'])] if supplied else ['absent']
                assert send('critical', message, *inputs) == 'logged'

        class Record:
            def __init__(self, i):
                self._i = i

            def __getattr__(self, key):
                value = send('attribute', self._i, key)
                if value == 'missing':
                    raise AttributeError(key)
                assert value.startswith('value ')
                return ast.literal_eval(value[6:])

        class Records:
            def __len__(self):
                return int(send('length'))

            def __getitem__(self, i):
                reply = send('index', i)
                if reply == 'missing':
                    raise IndexError(i)
                return Record(int(reply))

        try:
            with patch.object(test_logging, 'logging', SimpleNamespace(LoggerAdapter=Adapter)), \
                 patch.object(case, 'adapter', Adapter(case.logger)), \
                 patch.object(case.recording, 'records', Records()):
                original_method()
        finally:
            if child.poll() is None:
                child.stdin.write('stop\n')
                child.stdin.flush()
            _, err = child.communicate(timeout=30)
            if child.returncode or err:
                raise AssertionError((child.returncode, err))
    assert 'critical' in calls and 'attribute' in calls
    return calls


def adapter_cases(toolchain, work):
    from test import test_logging

    def root_fixture(case, name):
        if (case.logger is not test_logging.logging.root or case.adapter.logger is not case.logger
                or case.recording not in case.logger.handlers or case.recording.records
                or case.logger.disabled or case.logger.manager.disable != 0 or case.adapter.extra is not None):
            raise AssertionError('unexpected original LoggerAdapterTest root or recording fixture')
        return fixture_for(name, case.logger.getEffectiveLevel())

    result = {}
    binary = None
    for name in NAMES:
        key = PREFIX + name
        folder = work / name
        folder.mkdir()
        def execute(case, original, key=key, name=name, folder=folder):
            nonlocal binary
            fixture = root_fixture(case, name)
            body = method_bytes(SOURCE, OWNER, name)
            statements = {line.strip() for line in body.decode().splitlines()}
            if name == 'test_has_handlers':
                if case.logger.handlers != [case.recording]:
                    raise AssertionError('handler removal needs the actual single-handler fixture')
                observed = native_method(toolchain, folder, SOURCE, OWNER, key, fixture,
                    operation=('imre', 'Native adapter and logger handler search before and after live removal'))(case, original)
                observed['observation']['evidence'].update(fixture=FIXTURE, assertion_fixture='stock native TestCase')
                return observed
            blockers = []
            for label, attempt in [('stock native TestCase', fixture),
                                   ('native assertions for concrete LogRecord', record_fixture(fixture))]:
                try:
                    run = native_method(toolchain, folder, SOURCE, OWNER, key, attempt,
                                        operation=('imre', OPERATION))
                except NativeCompileError as exc:
                    blockers.append({'fixture': label, **assertion_blocker(exc, folder, statements)})
                else:
                    observed = run(case, original)
                    observed['observation']['evidence'].update(fixture=FIXTURE, assertion_fixture=label,
                                                              native_assertion_blockers=blockers)
                    return observed
            if binary is None:
                binary = toolchain.build((HERE / 'adapter_event_driver.py').read_bytes(), work)
            calls = host_method(binary, case, original)
            return {'observation': {'roles': roles(body='cpython', assertions='cpython', operation='imre', fixture='mixed'),
                    'evidence': {'operation': OPERATION, 'fixture': FIXTURE, 'opt_level': toolchain.level,
                                 'method_sha256': hashlib.sha256(body).hexdigest(), 'native_calls': calls,
                                 'native_assertion_blockers': blockers}}}
        result[key] = execute
    result.update(manager_cases(toolchain, work, adapter_fixture=lambda case: root_fixture(case, 'test_is_enabled_for')))
    return result


def wrong_handler_controls(toolchain, directory, module):
    """Make the unchanged default-extra method reject two deliberately bad handlers."""
    name = 'test_extra_not_merged_by_default'
    key = PREFIX + name
    body = method_bytes(SOURCE, OWNER, name)
    captured = '            self.records.append(record)'
    controls = {}
    for fault in ('drop_record', 'insert_extra'):
        def execute(case, original):
            handle = case.recording.handle

            def wrong(record):
                if fault == 'insert_extra':
                    record.__dict__['foo'] = 'wrong'
                    handle(record)

            with patch.object(case.recording, 'handle', wrong):
                try:
                    original()
                except AssertionError as error:
                    message = str(error)
                else:
                    raise AssertionError(f'CPython accepted {fault}')

            fixture = record_fixture(fixture_for(name, case.logger.getEffectiveLevel()))
            if fixture.count(captured) != 1:
                raise ValueError('expected one native Recording.emit capture')
            change = ("            record.__dict__['foo'] = 'wrong'\n" + captured
                      if fault == 'insert_extra' else '            pass')
            source = fixture.replace(captured, change).encode() + b'\n' + body
            source += (f'\ntry:\n    NativeCase().{name}()\n'
                       'except AssertionError as error:\n    print(str(error))\n'
                       'else:\n    raise RuntimeError("accepted broken handler")\n').encode()
            binary = toolchain.build(source, directory)
            child = subprocess.run([binary], capture_output=True, text=True, timeout=60)
            if child.returncode or child.stderr or child.stdout != message + '\n':
                raise AssertionError((fault, message, child.returncode, child.stdout, child.stderr))
            return {'assertion_error': message, 'method_sha256': hashlib.sha256(body).hexdigest()}

        results, failures, skips = run_in_fixtures(load_suite(module), {key: execute})
        if failures or skips or set(results) != {key}:
            raise AssertionError((fault, failures, skips, results))
        controls[fault], = results[key]
    return controls


def main():
    p = argparse.ArgumentParser(description=__doc__)
    for name in ('imre', 'runtime', 'clang', 'cpython-source', 'result'):
        p.add_argument('--' + name, type=Path, required=True)
    args = p.parse_args()
    if sys.implementation.name != 'cpython' or sys.version_info[:3] != (3, 14, 5) or not __debug__:
        p.error('requires pinned CPython 3.14.5 with assertions enabled')
    sys.path.insert(0, str(args.cpython_source.resolve() / 'Lib'))
    manifest = json.loads((HERE / 'UPSTREAM.json').read_bytes())
    verify_files(STDLIB / 'upstream', manifest['files'])
    verify_files(args.cpython_source, manifest['files'] | manifest['reference_files'])
    public = (HERE.parents[3] / 'tests/fixtures/files/logging_adapter_events.py').read_bytes()
    checks = '''record = logging.LogRecord('name', 20, '', 1, 'hello', (), None)
case.assertHasAttr(record, 'name')
case.assertNotHasAttr(record, 'absent')
print('present and absent')
try:
    case.assertHasAttr(record, 'absent', 'detail')
except AssertionError as error:
    print(str(error))
settings.longMessage = False
try:
    case.assertNotHasAttr(record, 'name', 'detail')
except AssertionError as error:
    print(str(error))
'''
    host_checks = ('import logging, unittest\ncase = unittest.TestCase()\nsettings = case\n' + checks).encode()
    initialize = "    def __init__(self) -> None:\n        self.checks = unittest.TestCase()\n"
    native_checks = ('import logging\n' + RECORD_CASE + initialize
                     + 'case = NativeCase()\nsettings = case.checks\n' + checks).encode()
    report = {'cpython_commit': manifest['commit'], 'denominator': 272, 'levels': {}, 'controls': {}}
    for level in (0, 3):
        suite = load_suite(manifest['module'])
        assert len(list(cases(suite))) == 272
        with tempfile.TemporaryDirectory() as tmp:
            tools = Toolchain(args.imre, args.runtime, args.clang, level)
            selected = adapter_cases(tools, Path(tmp))
            evidence, failures, skips = run_in_fixtures(suite, selected)
            if failures or skips or set(evidence) != set(selected):
                raise AssertionError((failures, skips, evidence))
            report['levels'][str(level)] = {k:v[0].get('observation', v[0]) for k,v in evidence.items()}
            report['controls'][str(level)] = controls = {}
            for label, host, native in [('public_adapter_events', public, public),
                                        ('concrete_attribute_assertions', host_checks, native_checks)]:
                reference = subprocess.run([sys.executable, '-I', '-B', '-'], input=host, capture_output=True, check=True, timeout=60)
                binary = tools.build(native, Path(tmp))
                actual = subprocess.run([binary], capture_output=True, check=True, timeout=60)
                if reference.stderr or actual.stderr or reference.stdout != actual.stdout:
                    raise AssertionError((label, reference.stdout, actual.stdout, reference.stderr, actual.stderr))
                controls[label] = {'observations': len(actual.stdout.splitlines()),
                                   'stdout_sha256': hashlib.sha256(actual.stdout).hexdigest()}

            controls['corrupted_recording_handler'] = wrong_handler_controls(tools, Path(tmp), manifest['module'])
    args.result.write_text(json.dumps(report, indent=2) + '\n')
    print(f"{len(report['levels']['0'])} original LoggerAdapter methods completed at O0 and O3; see each execution role.")


if __name__ == '__main__':
    main()
