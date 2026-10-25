"""Run the five unchanged LoggerAdapter context methods inside their CPython fixtures."""
import argparse
import ast
import hashlib
import json
from pathlib import Path
import re
import subprocess
import sys
import tempfile
from types import SimpleNamespace
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
STDLIB = HERE.parents[1]
SOURCE = STDLIB / 'upstream/Lib/test/test_logging.py'
sys.path.insert(0, str(STDLIB))
from suite_evidence import roles
from suite_inventory import cases, load_suite, verify_files
from suite_native import NativeCompileError, Toolchain, method_bytes, native_method, run_in_fixtures, testcase_fixture

OWNER = 'LoggerAdapterTest'
PREFIX = 'test.test_logging.' + OWNER + '.'
NAMES = ('test_extra_in_records', 'test_extra_not_merged_by_default', 'test_extra_merged',
         'test_extra_merged_log_call_has_precedence', 'test_extra_merged_without_extra')
FIXTURE = ('Original CPython LoggerAdapterTest setUp, root recording handler, shutdown and cleanups; '
           'fresh native root with matching threshold, recording handler and default adapter.')
OPERATION = 'Native adapter creation, process, record forwarding and live record attributes'
CAPTURE = (HERE / 'adapter_event_driver.py').read_text().split('def context()', 1)[0]
SETUP = '''
    logger: logging.Logger
    recording: Recording
    adapter: logging.LoggerAdapter

    def __init__(self) -> None:
        super().__init__()
        self.logger = logging.root
        self.logger.setLevel(THRESHOLD)
        self.recording = Recording()
        self.logger.addHandler(self.recording)
        self.adapter = logging.LoggerAdapter(logger=self.logger)
'''
# The concrete instance branch of CPython TestCase's two attribute assertions.
# Always try the stock testcase_fixture first; the original test body never changes.
PRESENCE = '''
    def assertHasAttr(self, obj: logging.LogRecord, name: str, msg: str | None = None) -> None:
        if not hasattr(obj, name):
            self.fail(self._formatMessage(msg, "'LogRecord' object has no attribute " + repr(name)))

    def assertNotHasAttr(self, obj: logging.LogRecord, name: str, msg: str | None = None) -> None:
        if hasattr(obj, name):
            self.fail(self._formatMessage(msg, "'LogRecord' object has unexpected attribute " + repr(name)))
'''


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
    result = {}
    binary = None
    for name in NAMES:
        key = PREFIX + name
        folder = work / name
        folder.mkdir()
        def execute(case, original, key=key, name=name, folder=folder):
            nonlocal binary
            if (case.logger is not test_logging.logging.root or case.adapter.logger is not case.logger
                    or case.recording not in case.logger.handlers or case.recording.records
                    or case.logger.disabled or case.logger.manager.disable != 0 or case.adapter.extra is not None):
                raise AssertionError('unexpected original LoggerAdapterTest root or recording fixture')
            body = method_bytes(SOURCE, OWNER, name)
            statements = {line.strip() for line in body.decode().splitlines()}
            fixture = CAPTURE + '\n' + testcase_fixture() + SETUP.replace('THRESHOLD', str(case.logger.getEffectiveLevel()))
            blockers = []
            for label, added in [('stock native TestCase', ''), ('native assertions for concrete LogRecord', PRESENCE)]:
                try:
                    run = native_method(toolchain, folder, SOURCE, OWNER, key, fixture + added,
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
    return result


def main():
    p = argparse.ArgumentParser(description=__doc__)
    for name in ('imre', 'runtime', 'clang', 'cpython-source', 'result'):
        p.add_argument('--' + name, type=Path, required=True)
    args = p.parse_args()
    if sys.implementation.name != 'cpython' or sys.version_info[:3] != (3, 14, 5):
        p.error('requires the pinned CPython 3.14.5')
    sys.path.insert(0, str(args.cpython_source.resolve() / 'Lib'))
    manifest = json.loads((HERE / 'UPSTREAM.json').read_bytes())
    verify_files(STDLIB / 'upstream', manifest['files'])
    verify_files(args.cpython_source, manifest['files'] | manifest['reference_files'])
    report = {'cpython_commit': manifest['commit'], 'denominator': 272, 'levels': {}}
    for level in (0, 3):
        suite = load_suite(manifest['module'])
        assert len(list(cases(suite))) == 272
        with tempfile.TemporaryDirectory() as tmp:
            tools = Toolchain(args.imre, args.runtime, args.clang, level)
            selected = adapter_cases(tools, Path(tmp))
            evidence, failures, skips = run_in_fixtures(suite, selected)
            if failures or skips or set(evidence) != set(selected):
                raise AssertionError((failures, skips, evidence))
            report['levels'][str(level)] = {k:v[0]['observation'] for k,v in evidence.items()}
    args.result.write_text(json.dumps(report, indent=2) + '\n')
    print('Five original LoggerAdapter methods completed at O0 and O3; see each execution role.')


if __name__ == '__main__':
    main()
