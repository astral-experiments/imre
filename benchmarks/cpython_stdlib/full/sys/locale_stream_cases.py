"""Exercise the unmodified C/POSIX stream methods through each subject's real -c command."""
import ast
import hashlib
from pathlib import Path
import re
import subprocess
import sys
from types import SimpleNamespace
from unittest.mock import patch

from suite_evidence import roles
from suite_inventory import load_suite
from suite_native import run_in_fixtures

MODULE = 'test.test_sys'
IDS = {f'{MODULE}.SysModuleTest.test_{locale}_locale_surrogateescape': locale.upper()
       for locale in ('c', 'posix')}


def normalized(text):
    return text.replace(str(Path.cwd() / '<string>.py'), '<string>.py')


def subject(module, executable, level_flags=()):
    def execute(case, original):
        children = []

        class Child(subprocess.Popen):
            def __init__(self, argv, **kwargs):
                if (argv[:4] != [executable, '-X', 'utf8=0', '-c'] or argv[5:] not in ([], ['-I'])
                        or not kwargs['universal_newlines'] or kwargs['stderr'] != subprocess.STDOUT):
                    raise ValueError(f'unexpected locale child: {argv!r}')
                env = kwargs['env'].copy()
                if env['LC_ALL'] != IDS[case.id()] or env['PYTHONCOERCECLOCALE'] != '0':
                    raise ValueError('original locale fixture changed')
                self.row = {'args': argv[1:], 'source_sha256': hashlib.sha256(argv[4].encode()).hexdigest(),
                            'LC_ALL': env['LC_ALL'], 'PYTHONCOERCECLOCALE': env['PYTHONCOERCECLOCALE'],
                            'PYTHONIOENCODING': env.get('PYTHONIOENCODING')}
                children.append(self.row)
                if level_flags:
                    env['IMRE_ENCODED_FLAGS'] = '\x1f'.join(level_flags)
                super().__init__(argv, **(kwargs | {'env': env}))

            def communicate(self, *args, **kwargs):
                stdout, stderr = super().communicate(*args, **kwargs)
                if stderr is not None:
                    raise ValueError('the original child merges stderr into stdout')
                self.row.update(status=self.returncode, stdout_text=normalized(stdout))
                return stdout, stderr

        with (patch.object(module, 'sys', SimpleNamespace(executable=executable)),
              patch.object(module, 'subprocess', SimpleNamespace(Popen=Child, PIPE=subprocess.PIPE,
                                                                 STDOUT=subprocess.STDOUT))):
            try:
                original()
            except AssertionError as exc:
                outcome, detail = 'failure', normalized(str(exc))
            else:
                outcome, detail = 'pass', 'all six original stream comparisons passed'
        if not children or any('status' not in child for child in children):
            raise AssertionError(children)
        return {'outcome': outcome, 'reason': detail, 'children': children}

    results, failures, skips = run_in_fixtures(load_suite(MODULE), dict.fromkeys(IDS, execute))
    if failures or skips or set(results) != IDS.keys() or any(len(items) != 1 for items in results.values()):
        raise AssertionError((failures, skips, results.keys()))
    return {key: items[0] for key, items in results.items()}


def observe(module, toolchain, source, preflight):
    stock = subject(module, sys.executable)
    flags = ['--quiet', '--no-cache', '--runtime', str(toolchain.runtime.resolve()),
             '--clang', str(toolchain.clang.resolve()), '--opt-level', str(toolchain.level)]
    native = subject(module, str(toolchain.imre.resolve()), flags)
    lines = source.read_bytes().splitlines(keepends=True)
    cls, = [c for c in ast.parse(b''.join(lines)).body if isinstance(c, ast.ClassDef) and c.name == 'SysModuleTest']
    hashes = {n.name: hashlib.sha256(b''.join(lines[min([n.lineno, *(d.lineno for d in n.decorator_list)]) - 1:n.end_lineno])).hexdigest()
              for n in cls.body if isinstance(n, ast.FunctionDef)}
    for case_id in IDS:
        control, actual = stock[case_id], native[case_id]
        if control['outcome'] != 'pass' or len(control['children']) != 6:
            raise AssertionError(control)
        if (len(actual['children']) > len(control['children'])
                or (actual['outcome'] == 'pass' and len(actual['children']) != 6)
                or any(a['args'] != b['args'] or a['PYTHONIOENCODING'] != b['PYTHONIOENCODING']
                       for a, b in zip(actual['children'], control['children']))):
            raise AssertionError('subjects received different original calls')
        last = actual['children'][-1]
        compile_error = re.search(r'^<string>\.py:(\d+):(\d+): Imre: (.+)$', last['stdout_text'], re.M)
        reached = sum(c['stdout_text'].startswith('stdin: ') for c in actual['children'])
        if actual['outcome'] == 'failure' and last['status'] and compile_error:
            outcome = 'blocked'
            reason = f'Original child {len(actual["children"])} stopped at {compile_error.group(0)}'
        else:
            outcome, reason = actual['outcome'], actual['reason']
        record = preflight[case_id, 1]
        original_attempt = record['evidence']
        record.update(outcome=outcome, reason=reason,
                      roles=roles(body='cpython', assertions='cpython', fixture='cpython',
                                  operation='imre' if reached else 'unobserved'),
                      evidence={'operation': 'the original children request stdin, stdout and stderr error policies',
                                'fixture': 'original CPython locale helpers, subprocess pipes and TestCase hooks',
                                'assertions': 'unchanged CPython stream comparisons', 'opt_level': toolchain.level,
                                'method_sha256': hashes[case_id.rsplit('.', 1)[1]],
                                'helper_sha256': {k: hashes[k] for k in ('check_locale_surrogateescape', 'c_locale_get_error_handler')},
                                'children_with_stream_output': reached, 'complete_native_method_attempt': original_attempt,
                                'stock_cpython_control': control, 'original_method_with_imre': actual})
