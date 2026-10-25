"""Fixtures for two unchanged BaseSelectorTestCase methods using integer fds."""
import ast
from contextlib import contextmanager
from hashlib import sha256
from pathlib import Path
import socket
import sys


FIXTURE = '''from typing import Callable
import os
import selectors
import sys

class Descriptor:
    number: int
    def __init__(self, fd: int) -> None:
        self.number = fd
    def fileno(self) -> int:
        return self.number
    def close(self) -> None:
        os.close(self.number)

class NativeCase:
    selector: selectors.PollSelector
    pending: bool
    def __init__(self) -> None:
        self.selector = selectors.CONSTRUCTOR()
        self.pending = False
    def SELECTOR(self) -> selectors.PollSelector:
        return self.selector
    def addCleanup(self, action: Callable[[], None]) -> None:
        self.pending = True
    def finish(self) -> None:
        assert self.pending
        self.selector.close()
    def make_socketpair(self) -> tuple[Descriptor, Descriptor]:
        return Descriptor(int(sys.argv[1])), Descriptor(int(sys.argv[2]))
    def assertEqual(self, first: list[tuple[selectors.SelectorKey, int]], second: list[tuple[selectors.SelectorKey, int]]) -> None:
        if len(first) != 0 or len(second) != 0:
            raise AssertionError("expected empty selector results")
'''


@contextmanager
def descriptors():
    left, right = socket.socketpair()
    with left, right:
        yield (left.fileno(), right.fileno())


def exercise(root, work, level, build, require, run):
    data = (root / 'upstream/Lib/test/test_selectors.py').read_bytes()
    cls, = (n for n in ast.parse(data).body if isinstance(n, ast.ClassDef) and n.name == 'BaseSelectorTestCase')
    observations = {}
    for constructor in ('PollSelector', 'DefaultSelector'):
        for method, before, after, failure in (
            ('test_empty_select', 's.select(timeout=0)', '[(s.register(1, selectors.EVENT_WRITE), selectors.EVENT_WRITE)]', 'expected empty selector results'),
            ('test_unregister_after_fd_close', 's.unregister(w)', 's.unregister(r)', 'KeyError'),
        ):
            node, = (n for n in cls.body if isinstance(n, ast.FunctionDef) and n.name == method)
            # The empty-select method's guard excludes Windows; the parent test
            # class guard for PollSelector requires this actual POSIX backend.
            assert sys.platform in ('linux', 'darwin')
            body = b''.join(data.splitlines(keepends=True)[node.lineno-1:node.end_lineno])
            case = f'test.test_selectors.{constructor}TestCase.{method}'
            file = work / 'selector_method.py'
            fixture = FIXTURE.replace('CONSTRUCTOR', constructor).encode()
            end = f'\nc = NativeCase()\nc.{method}()\nc.finish()\nprint("passed {case}")\n'.encode()
            file.write_bytes(fixture + body + end)
            with descriptors() as fds:
                args = list(map(str, fds))
                assert require([build(file, level), *args], pass_fds=fds) == f'passed {case}\n'
                assert require([sys.executable, file, *args], pass_fds=fds) == f'passed {case}\n'
                assert body.count(before.encode()) == 1
                file.write_bytes(fixture + body.replace(before.encode(), after.encode()) + end)
                for cmd in ([build(file, level)], [sys.executable, file]):
                    mutant = run([*cmd, *args], pass_fds=fds)
                    assert mutant.returncode and failure in mutant.stderr, (case, mutant)
            observations[case] = (
                {'body': 'imre', 'assertions': 'imre', 'operation': 'imre', 'fixture': 'mixed'},
                {'operation': f'native {constructor} registration, close/unregister or zero-timeout wait',
                 'fixture': 'host extracts the unchanged base-class method, checks its POSIX guard and provides a real socketpair; native fixture selects the upstream constructor, defers its close, and specializes the empty-list assertion',
                 'method_sha256': sha256(body).hexdigest(), 'negative_control': failure})
            print(f'O{level} {case}: passed; control detected', flush=True)
    return observations
