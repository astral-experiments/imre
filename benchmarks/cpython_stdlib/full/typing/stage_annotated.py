"""Stage the recorded AnnotatedTests and GetTypeHintsTests drivers from pinned fixtures."""
import argparse
import hashlib
from pathlib import Path
import shutil
from string import Template

SUITE = Path(__file__).resolve().parent
SHARED = SUITE.parent / 'shared/upstream/Lib/test'
TYPING = SUITE / 'upstream/Lib/test'
ANNOTATED_IMPORTS = '''import annotationlib
import itertools
import operator
import pickle
import types
import typing
from functools import reduce
from typing import Annotated, ClassVar, Final, Generic, KT, List, Optional, T, Tuple
from typing import TypeVar, TypeVarTuple, Union, Unpack, VT, get_args, get_origin, get_type_hints
from unittest import TestCase
from test.support import EqualToForwardRef'''
CLIENT_IMPORTS = '''import sys
import typing
from typing import Annotated, ForwardRef, Generic, List, T, Tuple, get_type_hints
from unittest import TestCase'''
CLIENTS = ['test_get_type_hints_annotated' + suffix for suffix in ('', '_in_union', '_refs', '_with_none_default')]
DIAGNOSTIC = 'test_get_type_hints_annotated_bad_module'


def pinned(path, digest):
    data = path.read_bytes()
    if hashlib.sha256(data).hexdigest() != digest:
        raise ValueError(f'Fixture checksum mismatch for {path}')
    return data


DISPATCHER = """import sys

if len(sys.argv) > 1 and sys.argv[1] == 'clients':
    del sys.argv[1]
    import client_runner
else:
    import annotated_runner
"""


def stage_runner(root, clients):
    modes = "'enumerate','class','selected'"
    selection = ''
    if clients:
        modes += ",'diagnostic'"
        selection = (
            f"permitted = [{DIAGNOSTIC!r}] if mode == 'diagnostic' else {CLIENTS!r}\n"
            "suite = unittest.TestSuite([c for c in suite if c.id().rsplit('.',1)[-1] in permitted])\n"
        )
    template = Template((SUITE / 'annotated_runner.py.in').read_text())
    runner = template.substitute(owner='GetTypeHintsTests' if clients else 'AnnotatedTests', modes=modes, selection=selection)
    (root / ('client_runner.py' if clients else 'annotated_runner.py')).write_text(runner)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('route', choices=['isolated', 'intact', 'clients', 'clients-intact', 'intact-both'])
    parser.add_argument('output', type=Path, help='New module root')
    args = parser.parse_args()
    root = args.output
    root.mkdir(parents=True)
    test = root / 'test'
    test.mkdir()
    clients = args.route.startswith('clients')
    module = pinned(TYPING / 'test_typing.py', '28d17772f523800f02a5bf1e0cc012779d7df1f616fec7cc34d922dcba7ee1cc')
    shutil.copyfile(SHARED / '__init__.py', test / '__init__.py')
    if args.route in ('intact', 'clients-intact', 'intact-both'):
        (test / 'test_typing.py').write_bytes(module)
        shutil.copytree(SHARED / 'support', test / 'support')
        shutil.copytree(TYPING / 'typinganndata', test / 'typinganndata')
    else:
        lines = module.splitlines(keepends=True)
        base = b''.join(lines[66:71])  # BaseTestCase
        if clients:
            imports = CLIENT_IMPORTS
            owner = b''.join(lines[6672:7207])  # GetTypeHintsTests
        else:
            imports = ANNOTATED_IMPORTS
            owner = b''.join(lines[9453:9926])  # AnnotatedTests
            support = pinned(SHARED / 'support/__init__.py', 'ab77f9f5d2a4cd07f21550980fc84a224553a8d7c7ffdda409ebd7f3ee1941c4')
            helper = b''.join(support.splitlines(keepends=True)[3149:3188])  # EqualToForwardRef
            (test / 'support').mkdir()
            (test / 'support/__init__.py').write_bytes(b'import annotationlib\n\n\n' + helper)
        (test / 'test_typing.py').write_bytes(imports.encode() + b'\n\n\n' + base + b'\n\n' + owner)

    stage_runner(root, clients)
    if args.route == 'intact-both':
        stage_runner(root, True)
        (root / 'full_entry.py').write_text(DISPATCHER)
    shutil.copyfile(SUITE / 'annotated_control.py', root / 'annotated_control.py')


if __name__ == '__main__':
    main()
