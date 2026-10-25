"""Run unchanged CStringIOTest bodies against a live native stream in a child."""

import ast
from collections import Counter
from contextlib import ExitStack
import hashlib
from select import select
import subprocess
from unittest.mock import patch

from suite_native import method_bytes

CASES = {**dict.fromkeys(("test_newline_default", "test_newline_none", "test_newline_empty",
                          "test_newline_lf", "test_newline_cr", "test_newline_crlf",
                          "test_newlines_property", "test_relative_seek"), "TextIOTestMixin"),
         **dict.fromkeys(("test_overseek", "test_tell", "test_getvalue", "test_flags"), "MemoryTestMixin")}


def text_fields(value):
    return [len(value), *map(ord, value)]


class Stream:
    def __init__(self, binary, stack, calls, initial_value="", newline="\n"):
        self.child = subprocess.Popen([binary], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                      stderr=subprocess.PIPE, text=True, encoding="utf-8")
        self.calls = calls
        stack.callback(self.finish)
        self.send(*text_fields(initial_value), *(('none',) if newline is None else ('text', *text_fields(newline))))
        if self.receive() != 'ready':
            raise AssertionError('native StringIO construction failed')
        calls['StringIO'] += 1

    def send(self, *fields):
        self.child.stdin.write(''.join(f'{field}\n' for field in fields))
        self.child.stdin.flush()

    def receive(self):
        if not select([self.child.stdout], [], [], 10)[0]:
            raise TimeoutError('native StringIO did not answer')
        line = self.child.stdout.readline()
        if not line:
            raise AssertionError(f'native StringIO exited: {self.child.poll()}')
        return line.removesuffix('\n')

    def finish(self):
        try:
            if self.child.poll() is None:
                self.send('stop')
            stdout, stderr = self.child.communicate(timeout=10)
        except BaseException:
            self.child.kill()
            self.child.communicate()
            raise
        if (self.child.returncode, stdout, stderr) != (0, '', ''):
            raise AssertionError((self.child.returncode, stdout, stderr))

    def call(self, operation, *fields):
        self.send(operation, *fields)
        line = self.receive()
        self.calls[operation] += 1
        if line == 'stop-iteration':
            raise StopIteration
        for error in (ValueError, OSError):
            prefix = error.__name__ + ' '
            if line.startswith(prefix):
                raise error(ast.literal_eval(line[len(prefix):]))
        return ast.literal_eval(line)

    def getvalue(self): return self.call('getvalue')
    def read(self, size=-1): return self.call('read', size)
    def readlines(self): return self.call('readlines')
    def seek(self, offset, whence=0): return self.call('seek', offset, whence)
    def tell(self): return self.call('tell')
    def writable(self): return self.call('writable')
    def readable(self): return self.call('readable')
    def seekable(self): return self.call('seekable')
    def isatty(self): return self.call('isatty')
    def write(self, text): return self.call('write', *text_fields(text))
    def close(self): return self.call('close')
    def __next__(self): return self.call('next')

    @property
    def newlines(self): return self.call('newlines')

    @property
    def closed(self): return self.call('closed')

    def __iter__(self):
        if self.call('iter') is not True:
            raise AssertionError('a native StringIO iterator did not return itself')
        return self


def executor(binary, source, owner, method, level):
    def execute(case, original):
        calls = Counter()
        with ExitStack() as stack:
            def factory(*args, **kwargs):
                return Stream(binary, stack, calls, *args, **kwargs)
            with patch.object(case, 'ioclass', factory):
                original()
        if not calls['StringIO'] or len(calls) == 1:
            raise AssertionError(f'no stream operations in {case.id()}')
        return {'observation': {'roles': {'body': 'cpython', 'assertions': 'cpython',
                                          'operation': 'imre', 'fixture': 'mixed'},
                'evidence': {'operation': 'io.StringIO held by the native child', 'native_calls': dict(sorted(calls.items())),
                             'fixture': 'original CPython module, class, instance and method; native stream factory',
                             'opt_level': level, 'method_sha256': hashlib.sha256(method_bytes(source, owner, method)).hexdigest()}}}
    return execute
