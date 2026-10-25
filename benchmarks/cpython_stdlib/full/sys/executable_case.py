"""Observe the original child with a nonexistent argv[0] and a separate real executable."""
import ast
import hashlib
import os
import subprocess
import sys
from types import SimpleNamespace
from unittest.mock import patch

from argv_case import native_command_env
from suite_evidence import roles

CASE_ID = "test.test_sys.SysModuleTest.test_executable"


def observe_subject(module, original, exe, env):
    # The parent only sees transported native strings; its path operations and assertions stay on CPython.
    reads = []
    children = []

    class Subject:
        @property
        def executable(self):
            if env is None:
                value = sys.executable
            else:
                run = subprocess.run([exe, "-c", "import sys; print(sys.executable)"], env=env,
                                     capture_output=True, check=True, timeout=45)
                if run.stderr or not run.stdout.endswith(b"\n"):
                    raise AssertionError((run.stdout, run.stderr))
                value = os.fsdecode(run.stdout[:-1])
            if value != exe:
                raise AssertionError((value, exe))
            reads.append(value)
            return value

    class ObservedPopen(subprocess.Popen):
        def __init__(self, argv, **kwargs):
            argv0, command_flag, code = argv
            if (argv0, command_flag, kwargs['executable'], kwargs['cwd']) != (
                    'nonexistent', '-c', exe, os.path.dirname(os.path.realpath(exe))):
                raise AssertionError((argv, kwargs))
            self.child = {'argv0': argv0, 'command_flag': command_flag,
                          'executable': '<subject-executable>', 'cwd': '<subject-executable-directory>',
                          'source_sha256': hashlib.sha256(code.encode()).hexdigest()}
            children.append(self.child)
            if env is not None:
                kwargs['env'] = env
            super().__init__(argv, **kwargs)

        def communicate(self, *args, **kwargs):
            out, err = super().communicate(*args, **kwargs)
            encoded = ast.literal_eval(out.strip().decode('ascii'))
            if self.returncode or encoded not in (b'', exe.encode('ascii', 'backslashreplace')):
                raise AssertionError((self.returncode, out, err))
            self.child.update(returncode=self.returncode, encoded_path='own executable' if encoded else 'empty')
            return out, err

    with (patch.object(module, 'sys', Subject()),
          patch.object(module, 'subprocess', SimpleNamespace(Popen=ObservedPopen, PIPE=subprocess.PIPE))):
        original()
    child, = children
    if len(reads) != 5:
        raise AssertionError(reads)
    return {'parent_reads': len(reads), 'child': child}


def observer(module, toolchain, method_sha256):
    def execute(case, original):
        stock = observe_subject(module, original, sys.executable, None)
        native = observe_subject(module, original, str(toolchain.imre.resolve()), native_command_env(toolchain))
        if stock['child']['source_sha256'] != native['child']['source_sha256']:
            raise AssertionError((stock, native))
        return {'execution': 'host-driven-native', 'body': 'CPython; original child: Imre',
                'assertions': 'CPython', 'fixture_execution': 'CPython decorators and SysModuleTest hooks',
                'calls': ['sys.executable'] * (native['parent_reads'] + 1),
                'observation': {'roles': roles(body='cpython', assertions='cpython', operation='imre', fixture='cpython'),
                                'evidence': {'operation': 'five native parent executable reads and the native child executable read and ASCII encoding',
                                             'assertions': 'original CPython path and accepted-output assertions; parent path operations and encoding are on CPython',
                                             'fixture': 'CPython decorators, SysModuleTest lifecycle and real Popen with a separate executable',
                                             'method_sha256': method_sha256, 'opt_level': toolchain.level,
                                             'native_calls': native['parent_reads'] + 2,
                                             'original_child': native, 'stock_cpython_control': stock}}}
    return execute
