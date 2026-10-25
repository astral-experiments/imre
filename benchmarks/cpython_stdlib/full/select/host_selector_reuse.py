"""Run the unchanged fd-reuse method against one live native selector per case."""
from contextlib import ExitStack
import errno
import os
from pathlib import Path
import select
import socket
import subprocess
import sys
from test import test_selectors
from types import SimpleNamespace
import unittest
from unittest.mock import patch


METHOD = 'test_unregister_after_fd_close_and_reuse'


def exercise(stack, native, constructor, control):
    # The worker must inherit both descriptions before the first register call.
    pairs = [tuple(stack.enter_context(s) for s in socket.socketpair()) for _ in range(2)]
    r, w, r2, _ = fds = [s.fileno() for pair in pairs for s in pair]
    assert pairs[1][1].send(b'x') == 1
    worker = stack.enter_context(subprocess.Popen([native, constructor], pass_fds=fds,
                                 stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                 stderr=subprocess.PIPE, text=True, bufsize=1))
    stack.callback(lambda: worker.kill() if worker.poll() is None else None)

    def rpc(action, a=0, b=0):
        worker.stdin.write(f'{action}\n{a}\n{b}\n')
        worker.stdin.flush()
        assert select.select([worker.stdout], [], [], 5)[0], ('worker stalled', action)
        reply = worker.stdout.readline().strip()
        if reply == 'key':
            raise KeyError('native duplicate unregister')
        if reply == 'badfd':
            raise OSError(errno.EBADF, 'native fd not duplicated')
        assert reply == 'ok', (action, reply, worker.stderr.read() if worker.poll() is not None else '')

    class SharedSocket:
        def __init__(self, stream):
            self.stream = stream

        def fileno(self):
            return self.stream.fileno()

        def close(self):
            if self.fileno() >= 0:
                rpc('close', self.fileno())
                self.stream.close()

    streams = iter(tuple(SharedSocket(s) for s in pair) for pair in pairs)

    def dup2(source, target):
        if control != 'omit-dup2' or target != r:
            rpc('dup2', source, target)
        return os.dup2(source, target)

    def close(fd):
        rpc('close', fd)
        os.close(fd)

    witnessed = []

    class NativeSelector:
        def register(self, fd, events):
            rpc('register', fd, events)
            if fd == w:
                rpc('before', w)
                witnessed.append('before')

        def unregister(self, fd):
            rpc('unregister', r if control == 'double-unregister' and fd == w else fd)
            if fd == w:
                try:
                    rpc('after', r, w)
                    witnessed.append('after')
                finally:
                    if control == 'omit-dup2':
                        rpc('dup2', r2, r)  # Restore the fd before unittest cleanups.

        def close(self):
            rpc('stop')

    case = getattr(test_selectors, constructor + 'TestCase')(METHOD)
    with patch.object(case, 'SELECTOR', NativeSelector), \
         patch.object(test_selectors, 'socketpair', lambda: next(streams)), \
         patch.object(test_selectors, 'os', SimpleNamespace(dup2=dup2, close=close)):
        result = unittest.TestResult()
        case.run(result)
    problems = result.errors + result.failures
    assert not result.skipped, result.skipped
    if control is None:
        assert result.wasSuccessful() and witnessed == ['before', 'after'], (problems, witnessed)
    else:
        expected = 'native duplicate unregister' if control == 'double-unregister' else 'native fd not duplicated'
        assert len(problems) == 1 and expected in problems[0][1], problems
    assert worker.wait(timeout=5) == 0 and not worker.stderr.read()
    print(case.id(), 'passed' if control is None else f'{control} control detected', flush=True)


if __name__ == '__main__':
    root = Path(__file__).resolve().parent
    assert Path(test_selectors.__file__).read_bytes() == (root / 'upstream/Lib/test/test_selectors.py').read_bytes()
    for name in ('PollSelector', 'DefaultSelector'):
        for control in (None, 'double-unregister', 'omit-dup2'):
            with ExitStack() as resources:
                exercise(resources, sys.argv[1], name, control)
