"""Check closed module graphs against CPython and exercise rejected import boundaries."""

import argparse
import os
from pathlib import Path
from zipfile import ZIP_DEFLATED, ZIP_STORED, ZipFile
import shutil
import subprocess
import sys
import tempfile


FIXTURES = Path(__file__).resolve().parent / "fixtures" / "imports"
HELPER = "def f(value: int) -> int:\n    return value + 1\n"
IMPORT = "from helper import f\nprint(f(2))\n"


REJECTED = {
    "unittest_method_skip_preserves_typed_call": ({
        "main.py": (
            "import unittest\n"
            "class Case(unittest.TestCase):\n"
            "    @unittest.skipUnless(False, 'feature required')\n"
            "    def test_body(self): pass\n"
            "def typed(n: int) -> int: return n\n"
            "typed('wrong')\n"
        ),
    }, "invalid-argument-type"),
    "second_base_has_storage": ({
        "main.py": (
            "class Left:\n    def __init__(self, x: int): self.left = x\n"
            "class Right:\n    def __init__(self, y: int): self.right = y\n"
            "class Both(Left, Right): pass\n"
        ),
    }, "multiple independent base classes"),
    "inspect_isdatadescriptor_preserves_typed_call": ({
        "main.py": (
            "from inspect import isdatadescriptor\n"
            "def typed(n: int) -> int: return n\n"
            "print(isdatadescriptor(property()))\n"
            "typed('wrong')\n"
        ),
    }, "expected int, got str"),
    "optional_import_keeps_typed_call_check": ({
        "main.py": (
            "import http.client\n"
            "try:\n    import ssl\nexcept ImportError:\n    ssl = None\n"
            "from urllib.request import urlopen\n"
            "def typed(n: int) -> int: return n\n"
            "print(urlopen('data:,hello').read(), typed('wrong'))\n"
        ),
    }, "invalid-argument-type"),
    "optional_presence_keeps_later_call_check": ({
        "main.py": (
            "try:\n    import sys as available\nexcept ImportError:\n    available = None\n"
            "import helper\nassert available is not None\n"
            "getattr(helper, 'hidd' + 'en')()\n"
        ),
        "helper.py": HELPER,
    }, "unknown function or unsupported builtin arguments"),
    "urlopen_keeps_typed_call_check": ({
        "main.py": (
            "from urllib.request import urlopen\n"
            "def typed(n: int) -> int: return n\n"
            "print(urlopen('data:,hello').read())\n"
            "print(typed('wrong'))\n"
        ),
    }, "expected int, got str"),
    "unused_warn_preserves_typed_arguments": ({
        "main.py": (
            "from warnings import warn\n"
            "def identity(n: int) -> int: return n\n"
            "identity('wrong')\n"
        ),
    }, "expected int, got str"),
    "codeop_import_keeps_return_check": ({
        "main.py": 'import codeop\ndef wrong() -> int:\n    return "invalid return"\nwrong()\n',
    }, "invalid-return-type"),
    "function_wildcard": ({
        "main.py": "def invalid():\n    from helper import *\n", "helper.py": HELPER,
    }, "SyntaxError: import * only allowed at module level"),
    "warning_scope_keeps_call_contracts": ({
        "main.py": (
            "import warnings\n"
            "def identity(n: int) -> int: return n\n"
            "with warnings.catch_warnings(record=True):\n    identity('wrong')\n"
        ),
    }, "invalid-argument-type"),
    "class_wildcard": ({
        "main.py": "class Invalid:\n    from helper import *\n", "helper.py": HELPER,
    }, "SyntaxError: import * only allowed at module level"),
    "configured_mock_checks_arguments": ({
        "main.py": "from unittest.mock import Mock\n"
                   "def identity(value: int) -> int: return value\n"
                   "m = Mock(custom=identity('wrong'))\n",
    }, "expected int, got str"),
    "unused_resource_reader_keeps_return_check": ({
        "main.py": "from importlib.readers import MultiplexedPath\ndef wrong() -> int: return 'bad'\nwrong()\n",
    }, "int"),
    "wildcard_keeps_return_check": ({
        "main.py": "from helper import *\ndef wrong() -> int:\n    return 'bad'\nwrong()\n",
        "helper.py": HELPER,
    }, "int"),
    "wildcard_keeps_missing_import": ({
        "main.py": "from helper import *\nimport absent\n", "helper.py": HELPER,
    }, "cannot resolve first-party"),
    "type_checking_only_export": ({
        "main.py": "from helper import token\n",
        "helper.py": "from typing import TYPE_CHECKING\nif TYPE_CHECKING:\n    token = 3\n",
    }, 'has no static binding "token"'),
    "weakref_of_imported_value_struct": ({
        "records.py": "from imre import struct as layout\n@layout\nclass Point:\n    x: float\n",
        "main.py": "from records import Point\nfrom weakref import ref\nref(Point(1.0))\n",
    }, "weakref.ref requires native Python object operations"),
    "deque_repr_checks_receiver_expression": ({
        "main.py": (
            "from collections import deque\ndef queue(n: int) -> int:\n    return n\n"
            "deque.__repr__(queue('wrong'))\n"
        ),
    }, "expected int, got str"),
    "saved_deque_repr_keeps_checked_calls": ({
        "main.py": (
            "from collections import deque\ndef identity(n: int) -> int:\n    return n\n"
            "render = deque.__repr__\nrender(deque([1]))\nidentity('wrong')\n"
        ),
    }, "invalid-argument-type"),
    "patch_object_checks_replacement": ({
        "main.py": (
            "from unittest.mock import patch\nclass Owner:\n    value = 0\n"
            "def replacement(number: int) -> int:\n    return number\n"
            "with patch.object(Owner, 'value', replacement('wrong')):\n    pass\n"
        ),
    }, "expected int, got str"),
    "unused_chdir_preserves_checked_calls": ({
        "main.py": "from os import chdir\ndef path(n: int) -> int:\n    return n\npath('wrong')\n",
    }, "expected int, got str"),
    "posix_chdir_checks_path": ({
        "main.py": "from posix import chdir as cd\ndef path(n: int) -> int:\n    return n\ncd(path=path('wrong'))\n",
    }, "expected int, got str"),
    "captured_chdir_keeps_callback_checks": ({
        "main.py": (
            "from os import chdir\ndef path(n: int) -> int:\n    return n\n"
            "move = chdir\nmove('.')\npath('wrong')\n"
        ),
    }, "invalid-argument-type"),
    "captured_chdir_checks_nested_missing_argument": ({
        "main.py": (
            "from os import chdir\ndef path(n: int) -> str:\n    return '.'\n"
            "move = chdir\nmove(path())\n"
        ),
    }, "missing-argument"),
    "captured_chdir_checks_user_overload": ({
        "main.py": (
            "from os import chdir\nfrom typing import overload\n"
            "@overload\ndef path(n: int) -> str: return '.'\n"
            "@overload\ndef path(n: int, m: int) -> str: return '.'\n"
            "def path(n: int, m: int = 0) -> str: return '.'\n"
            "move = chdir\nmove(path())\n"
        ),
    }, "no-matching-overload"),
    "captured_chdir_checks_user_keyword": ({
        "main.py": (
            "from os import chdir\ndef path(n: int = 0) -> int:\n    return n\n"
            "move = chdir\nmove('.')\npath(typo=1)\n"
        ),
    }, "unknown-argument"),
    "captured_chdir_preserves_callable_parameter": ({
        "main.py": (
            "from os import chdir\nfrom typing import Callable\n"
            "def invoke(cb: Callable[[int], int]) -> int:\n"
            "    move = chdir\n    move('.')\n    return cb('wrong')\n"
            "def identity(n: int) -> int:\n    return n\nprint(invoke(identity))\n"
        ),
    }, "invalid-argument-type"),
    "chdir_keeps_subsequent_checks": ({
        "main.py": "import os\ndef path(n: int) -> int:\n    return n\nos.chdir('.')\npath('wrong')\n",
    }, "expected int, got str"),
    "unused_struct_keeps_return_checks": ({
        "main.py": "from struct import Struct as Format\ndef wrong() -> int:\n    return 'bad'\nwrong()\n",
    }, "expected int, got str"),
    "serialization_import_preserves_annotation": ({
        "main.py": "import helper\n",
        "helper.py": (
            "import marshal as serializer\nfrom pickle import dumps as encode\n"
            "def result() -> int:\n    return 'wrong'\nresult()\n"
        ),
    }, "expected int, got str"),
    "unready_partial_function": ({
        "main.py": "import pkg.parent\n",
        "pkg/__init__.py": "",
        "pkg/parent.py": "def ready() -> int:\n    return 1\nfrom . import child\ndef later() -> int:\n    return 42\n",
        "pkg/child.py": "from .parent import later\nprint(later())\n",
    }, "has no static binding \"later\""),
    "unready_partial_attribute": ({
        "main.py": "import pkg.parent\n", "pkg/__init__.py": "",
        "pkg/parent.py": "from . import child\ndef later(n: int) -> int:\n    return n + 1\n",
        "pkg/child.py": "from . import parent\nprint(parent.later(41))\n",
    }, "references a function before it is available"),
    "partial_child_custom_getattr": ({
        "main.py": "import pkg.http\n",
        "pkg/__init__.py": "def __getattr__(name: str) -> str:\n    print(name)\n    return \"custom\"\n",
        "pkg/http.py": "import pkg.child as child\n",
        "pkg/child.py": "from . import http as partial\nprint(\"child finished\")\n",
    }, "cyclic import through \"pkg.http\""),
    "decorated_partial_function": ({
        "main.py": "import pkg.parent\n",
        "pkg/__init__.py": "",
        "pkg/parent.py": "from functools import cache\n@cache\ndef earlier() -> int:\n    return 42\nfrom . import child\n",
        "pkg/child.py": "from .parent import earlier\nprint(earlier())\n",
    }, "cyclic import through \"pkg.parent\""),
    "partial_source_value": ({
        "main.py": "import pkg.parent\n",
        "pkg/__init__.py": "",
        "pkg/parent.py": "answer = 42\nfrom . import child\n",
        "pkg/child.py": "from .parent import answer\nprint(answer)\n",
    }, "cyclic import through \"pkg.parent\""),
    "private_textwrap_module": ({"main.py": "import __imre_textwrap_wrapper__\n"}, "private textwrap implementation"),
    "private_textwrap_helper": ({"main.py": "from textwrap import _chunk_text\n"}, "unsupported sealed standard-library export textwrap._chunk_text"),
    "unsupported_stdlib": ({"main.py": "import turtle\n"}, 'standard-library imports and shadowing are unsupported: "turtle"'),
    "missing": ({"main.py": "import missing_dependency\n"}, "cannot resolve first-party"),
    "dynamic": ({"main.py": '__import__("helper")\n', "helper.py": HELPER}, "unsupported"),
    "nested": ({"main.py": "def f() -> None:\n    import math\nf()\n"}, "unsupported"),
    "conditional": ({"main.py": "if False:\n    import math\n"}, "unsupported"),
    "conditional_missing_export": ({
        "main.py": "from helper import chosen\n",
        "helper.py": "if False:\n    def chosen():\n        return 1\n",
    }, "no static binding"),
    "conditional_uninitialized_export": ({
        "main.py": "from helper import selected\n",
        "helper.py": "if True:\n    selected: int\nelse:\n    selected = 1\n",
    }, 'no static binding "selected"'),
    "unbound_alias": ({"main.py": "import helper\n", "helper.py": "Pair = tuple[Missing, int]\n"}, "name is not a local value"),
    "unused_bad_type": ({"main.py": IMPORT, "helper.py": HELPER + 'def unused() -> int:\n    return "wrong"\n'}, "helper.py"),
    "unused_resource_function": ({
        "main.py": "from importlib.resources import files\nx: int = 'wrong'\n",
    }, "expected int"),
    "unused_traceback_import": ({
        "main.py": "import helper\n",
        "helper.py": "import traceback as tb\ndef bad() -> int:\n    return 'wrong'\n",
    }, "expected int, got str"),
    "unused_traceback_formatter": ({
        "main.py": "from helper import bad\nprint(bad())\n",
        "helper.py": "from traceback import format_exc\ndef bad() -> int:\n    return 'wrong'\n",
    }, "expected int, got str"),
    "contextmanager_wrong_generator_return": ({
        "main.py": "import helper\nimport sys\nsys.exit(0)\n",
        "helper.py": (
            "import collections.abc\nfrom contextlib import contextmanager\n"
            "@contextmanager\ndef values() -> collections.abc.Generator[int, None, int]:\n"
            "    yield 1\n    return 'wrong'\n"
        ),
    }, 'expected `int`, found `Literal["wrong"]`'),
    "contextmanager_generator_missing_return": ({
        "main.py": "import helper\nimport sys\nsys.exit(0)\n",
        "helper.py": (
            "from collections.abc import Generator\nfrom contextlib import contextmanager\n"
            "@contextmanager\ndef values() -> Generator[int, None, int]:\n    yield 1\n"
        ),
    }, "always implicitly returns `None`"),
    "contextmanager_generator_can_fall_through": ({
        "main.py": "import helper\nimport sys\nsys.exit(0)\n",
        "helper.py": (
            "from collections.abc import Generator as Gen\nfrom contextlib import contextmanager as cm\n"
            "@cm\ndef values(flag: bool) -> Gen[int, None, int]:\n"
            "    yield 1\n    if flag:\n        return 2\n"
        ),
    }, "can implicitly return `None`"),
    "live_traceback_and_invalid_return": ({
        "main.py": "from helper import bad, render\nprint(render())\nprint(bad())\n",
        "helper.py": "import traceback as tb\ndef render():\n    return tb.format_exc()\ndef bad() -> int:\n    return 'wrong'\n",
    }, "invalid-return-type"),
    "unused_unsupported": ({"main.py": IMPORT, "helper.py": HELPER + 'def unused() -> int:\n    return eval("1")\n'}, "unsupported"),
    "inactive_guard": ({"main.py": IMPORT, "helper.py": HELPER + 'if __name__ == "__main__":\n    eval("1")\n'}, "unsupported"),
    "wrong_imported_argument": ({"main.py": 'from helper import f\nprint(f("wrong"))\n', "helper.py": HELPER}, "int"),
    "cycle": ({"main.py": "import first\n", "first.py": "import second\n", "second.py": "import first\n"}, "cyclic import"),
    "relative_beyond_root": ({"main.py": "from .helper import f\n", "helper.py": HELPER}, "relative import"),
    "parameter_module_shadow": ({"main.py": "import helper\ndef f(helper: int) -> int:\n    return helper\n", "helper.py": HELPER}, "rebinding"),
    "parameter_function_shadow": ({"main.py": "from helper import f\ndef g(f: int) -> int:\n    return f\n", "helper.py": HELPER}, "rebinding"),
    "local_module_shadow": ({"main.py": "import helper\ndef g() -> None:\n    helper = 1\n", "helper.py": HELPER}, "rebinding"),
    "local_function_shadow": ({"main.py": "from helper import f\ndef g() -> None:\n    f = 1\n", "helper.py": HELPER}, "rebinding"),
    "module_rebinding": ({"main.py": "from helper import f\ndef f(value: int) -> int:\n    return value\n", "helper.py": HELPER}, "cannot be rebound"),
    "cached_target_early_alias": ({
        "main.py": "import pkg.parent\n", "pkg/__init__.py": "", "pkg/child.py": HELPER,
        "pkg/parent.py": "from . import child\nvalue = late(1)\nfrom .child import f as late\n",
    }, "import before its binding is available"),
    "cached_target_early_alias_through_function": ({
        "main.py": "import pkg.parent\n", "pkg/__init__.py": "", "pkg/child.py": HELPER,
        "pkg/parent.py": ("from . import child\ndef read() -> int:\n    return late(1)\n"
                          "value = read()\nfrom .child import f as late\n"),
    }, "import before its binding is available"),
    "unpublished_child_attribute": ({
        "main.py": "import pkg.child\n", "pkg/__init__.py": "", "pkg/other.py": "",
        "pkg/child.py": "import pkg\n" + HELPER + "observed = pkg.child.f(1)\nfrom . import other\n",
    }, "import before its binding is available"),
    "imports_after_execution": ({"main.py": "print(1)\nimport helper\n", "helper.py": HELPER}, "imports must precede"),
    "guard_reads_later_import": ({"main.py": "import helper\n", "helper.py": 'if __name__ == "helper":\n    raise ValueError(message)\nfrom child import message\n', "child.py": 'message = "late"\n'}, "imports must precede"),
    "module_computed_before_import": ({
        "main.py": "import helper\n", "helper.py": "labels = str(7)\nimport os\n",
    }, "imports must precede"),
    "module_rebound_before_import": ({
        "main.py": "import helper\n", "helper.py": "labels = [1]\nlabels = [2]\nimport os\n",
    }, "imports must precede"),
    "library_computed_before_import": ({"main.py": "import package\n", "package/__init__.py": "metadata = str(7)\nfrom .child import f\n", "package/child.py": HELPER}, "imports must precede"),
    "module_attribute_guard_keeps_argument_checks": ({
        "feature.py": "available = True\n",
        "main.py": (
            "import http.client\nimport urllib.request\nimport feature\n"
            "if not feature.available:\n    raise RuntimeError('missing feature')\n"
            "def typed(value: int) -> int: return value\ntyped('wrong')\n"
        ),
    }, "invalid-argument-type"),
    "definition_after_execution": ({"main.py": "print(1)\n" + HELPER}, "definitions must precede"),
    "missing_namespace_child": ({"main.py": "import package.missing\n", "package/child.py": HELPER}, "cannot resolve first-party module"),
    "namespace_preserves_annotation": ({
        "main.py": "import helper\n",
        "space/child.py": "value = 3\n",
        "helper.py": "from space.child import value\ndef result() -> int:\n    return 'wrong'\nresult()\n",
    }, "expected int, got str"),
    "child_overwrites_definition": ({"main.py": "import package.child\n", "package/__init__.py": "def child() -> int:\n    return 1\n", "package/child.py": HELPER}, "overwrite"),
    "entry_self_import": ({"main.py": "import main\n"}, "entry module"),
    "reserved_alias": ({"main.py": "from helper import f as int\n", "helper.py": HELPER}, "reserved"),
    "normalized_reserved_alias": ({"main.py": "from helper import f as ℕone\n", "helper.py": HELPER}, "reserved"),
    "stdlib_shadow": ({"main.py": "import sys\n", "sys.py": HELPER}, "standard-library"),
    "py_compile_shadow": ({"main.py": "import py_compile\n", "py_compile.py": HELPER}, 'shadowing is unsupported: "py_compile"'),
    "token_shadow": ({"main.py": "import token\n", "token.py": HELPER}, 'shadowing is unsupported: "token"'),
    "email_address_parser_unknown_name": ({
        "main.py": "from email._header_value_parser import missing_parser\n",
    }, "unsupported sealed standard-library export"),
    "unused_email_address_parser_keeps_types": ({
        "main.py": "from email._header_value_parser import get_addr_spec\n"
                   "def identity(n: int) -> int: return n\nidentity('wrong')\n",
    }, "expected int, got str"),
    "opcode_shadow": ({"main.py": "import _opcode\n", "_opcode.py": HELPER}, "shadowing"),
    "opcode_unknown_operation": ({"main.py": "from _opcode import not_an_opcode_operation\n"}, "not_an_opcode_operation"),
    "opcode_typed_argument": ({"main.py": "import _opcode as op\ndef text(x: str) -> None:\n    print(x)\ntext(op.ENABLE_SPECIALIZATION)\n"}, "expected str, got int"),
    "unstubbed_builtin_shadow": ({"main.py": "import _types\n", "_types.py": HELPER}, "standard-library"),
    "frozen_shadow": ({"main.py": "import __hello__\n", "__hello__.py": HELPER}, "standard-library"),
    "imported_surrogate": ({"main.py": IMPORT, "helper.py": '"\\ud800"\n' + HELPER}, "surrogate"),
    "imported_encoding": ({"main.py": IMPORT, "helper.py": "# coding: latin1\n" + HELPER}, "encoding"),
    "imported_tabs": ({"main.py": IMPORT, "helper.py": "def f(value: int) -> int:\n\treturn value\n"}, "indentation"),
}

ACCEPTED = {
    "imported_method_mixin": {
        "helper.py": (
            "OFFSET = 7\n"
            "class Methods(object):\n"
            "    def common(self) -> int: return 99\n"
            "    def extra(self) -> int: return OFFSET\n"
        ),
        "main.py": (
            "from helper import Methods\n"
            "OFFSET = 200\n"
            "class Storage:\n"
            "    def __init__(self, value: int): self.value = value\n"
            "    def common(self) -> int: return self.value\n"
            "class Child(Storage, Methods): pass\n"
            "class Override(Child):\n"
            "    def extra(self) -> int: return 12\n"
            "def via(m: Methods) -> int: return m.common() + m.extra()\n"
            "print(via(Child(4)))\n"
            "print(via(Override(4)))\n"
            "print(issubclass(Override, Methods))\n"
        ),
    },
    "urllib_request_and_error": {
        "main.py": (
            "import urllib.request\n"
            "from urllib.error import URLError\n"
            "from urllib.request import Request\n"
            "with urllib.request.urlopen(Request('data:,one%20two')) as response:\n"
            "    print(response.geturl(), response.read())\n"
            "try: urllib.request.urlopen('nohandler://host')\n"
            "except URLError as error: print(error.reason)\n"
        ),
    },
    "token_named_helper": {
        "main.py": "from helper import read\nprint(read())\n",
        "helper.py": "from token import EXACT_TOKEN_TYPES, tok_name\ndef read():\n    return tok_name[EXACT_TOKEN_TYPES[':=']]\n",
    },
    "token_qualified_helper": {
        "main.py": "from helper import read\nprint(read())\n",
        "helper.py": "import token\ndef read():\n    return token.tok_name[token.EXACT_TOKEN_TYPES[':=']]\n",
    },
    "codeop_module": {
        "main.py": (
            "import codeop\n"
            "from codeop import Compile, CommandCompiler\n"
            "assert codeop.Compile is Compile\n"
            "compiler = CommandCompiler().compiler\n"
            "assert type(compiler) is Compile\n"
            "assert compiler.flags == codeop.PyCF_DONT_IMPLY_DEDENT | codeop.PyCF_ALLOW_INCOMPLETE_INPUT\n"
        ),
    },
    "warn_named_helper": {
        "main.py": "from helper import current\ncurrent()\n",
        "helper.py": (FIXTURES.parent / "boxed_warnings.py").read_text(),
    },
    "codeop_named_helper": {
        "main.py": "from helper import flags\nprint(flags())\n",
        "helper.py": (
            "from codeop import Compile, CommandCompiler\n"
            "def flags():\n"
            "    compiler = CommandCompiler().compiler\n"
            "    assert type(compiler) is Compile\n"
            "    return compiler.flags\n"
        ),
    },
    "opcode_named_helper": {
        "main.py": "from helper import effect\nprint(effect())\n",
        "helper.py": "from _opcode import stack_effect\ndef effect():\n    return stack_effect(31)\n",
    },
    "opcode_qualified_helper": {
        "main.py": "from helper import effect\nprint(effect())\n",
        "helper.py": "import _opcode\ndef effect():\n    return _opcode.stack_effect(31)\n",
    },
    "opcode_star_helper": {
        "main.py": "from helper import effect\nprint(effect())\n",
        "helper.py": "from _opcode import *\ndef effect():\n    return stack_effect(31)\n",
    },
    "atexit_in_dependency": {
        "main.py": "from cleanup import prepare\nprepare()\n",
        "cleanup.py": (
            "from atexit import register, unregister\n"
            "def removed(): print('unexpected')\n"
            "def prepare():\n"
            "    register(print, 'shutdown')\n"
            "    register(removed)\n"
            "    unregister(removed)\n"
        ),
    },
    "weakref_finalize_import": {
        "main.py": (
            "from weakref import finalize\n"
            "class Owner: pass\n"
            "owner = Owner()\n"
            "cleanup = finalize(owner, print, 'finalized')\n"
            "cleanup()\n"
            "assert not cleanup.alive\n"
        ),
    },
    "patch_object_as_value": {
        "main.py": (
            "from unittest.mock import patch\nclass Owner:\n    value = 1\n"
            "patcher = patch.object\nwith patcher(Owner, 'value', 2):\n    assert Owner.value == 2\n"
            "assert Owner.value == 1\n"
        ),
    },
    "entry_module_attribute_guard": {
        "feature.py": "import sys\navailable = 'deny' not in sys.argv\nprint('feature loaded')\n",
        "main.py": (
            "import feature\nif not feature.available:\n    raise ValueError('guard fired')\n"
            "def value() -> int:\n    print('initializer')\n    return 17\n"
            "after = value()\ndef read() -> int:\n    return after\nprint(read())\n"
        ),
    },
    "optional_stdlib_entry_import": {
        "main.py": (
            "try:\n    import sys as available\nexcept ImportError:\n    available = None\n"
            f"try:\n    import {'pwd' if os.name == 'nt' else 'winreg'} as missing\n"
            "except ImportError:\n    missing = None\n"
            "import after\nassert available is not None\nassert missing is None\n"
            "print(available is after.sys, missing is None)\n"
        ),
        "after.py": "import sys\nprint('after')\n",
    },
    "optional_import_error_alternatives": {
        "main.py": "from pkg.adapter import chosen, available, recovered\nprint(chosen, available, recovered)\n",
        "pkg/__init__.py": "",
        "pkg/adapter.py": (
            "try:\n    import absent.plugin as chosen\n"
            "except ImportError:\n    from .fallback import value as chosen\n"
            "try:\n    from .preferred import value as available\n"
            "except ImportError:\n    from .unexpected import value as available\n"
            "try:\n    from .broken import value as recovered\n"
            "except ImportError:\n    from .fallback import value as recovered\n"
            "from . import after\n"
        ),
        "pkg/preferred.py": "print('preferred')\nvalue = 23\n",
        "pkg/broken.py": "print('broken')\nraise ImportError('provider unavailable')\n",
        "pkg/fallback.py": "print('fallback')\nvalue = 17\n",
        "pkg/unexpected.py": "raise AssertionError('unselected fallback executed')\n",
        "pkg/after.py": "print('after')\n",
    },
    "optional_platform_import": {
        "main.py": "import configure\nfrom adapter import present, absent\nprint(present, absent)\n",
        "configure.py": "import sys\nsys.platform = 'test-platform'\nprint('configure')\n",
        "adapter.py": (
            "import sys\n"
            "if sys.platform in {'test-platform'}:\n"
            "    from provider import value as present\n"
            "else:\n"
            "    present = None\n"
            "if sys.platform == 'missing-platform':\n"
            "    from unavailable import value as absent\n"
            "else:\n"
            "    absent = None\n"
            "import after\n"
        ),
        "provider.py": "print('provider')\nvalue = 42\n",
        "after.py": "print('after')\n",
    },
    "wildcard_native_namespaces": {
        "main.py": "import paths\nimport prepare\nimport codes\nimport computed\n",
        "paths.py": (
            "import genericpath as _path\nfrom genericpath import *\n"
            "assert [n for n in globals() if not n.startswith('_')] == _path.__all__\n"
            "assert ALLOW_MISSING is _path.ALLOW_MISSING and samefile is _path.samefile\n"
        ),
        "prepare.py": (
            "import errno\n"
            "def resolve(name):\n    if name == '_computed': return 19\n    raise AttributeError(name)\n"
            "errno.custom = 37\nerrno._private = 42\n"
        ),
        "codes.py": (
            "import errno as _errno\nimport prepare as _prepare\nfrom errno import *\n"
            "assert [n for n in globals() if not n.startswith('_')] == "
            "[n for n in _errno.__dict__ if not n.startswith('_')]\n"
            "assert '_private' not in globals() and custom == 37\n"
            "assert ENOENT == _errno.ENOENT and errorcode is _errno.errorcode\n"
            "assert dir(_errno) == sorted(_errno.__dict__)\n"
            "del _errno.EINVAL\n_errno.ENOENT = 'rebound'\n"
            "_errno.__all__ = ['_private', 'ENOENT', 'EINVAL']\n"
            "try:\n    from errno import *\n"
            "except AttributeError:\n    assert _private == 42 and ENOENT == 'rebound'\n"
            "else:\n    raise AssertionError('deleted errno constant was resurrected')\n"
            "_errno.__getattr__ = _prepare.resolve\n_errno.__all__ = ['_computed']\n"
        ),
        "computed.py": "from errno import *\nassert _computed == 19\n",
    },
    "wildcard_dict_subclass_all": {
        "main.py": (
            "from custom_names import *\nprint(custom)\n"
            "try:\n    from dict_names import *\n"
            "except KeyError as error:\n    print(error.args, globals()['partial'])\n"
            "else:\n    raise AssertionError('missing KeyError from __all__')\n"
        ),
        "custom_names.py": (
            "class Names(dict):\n    def __getitem__(self, index):\n"
            "        if index == 0: return 'custom'\n        raise IndexError(index)\n"
            "custom = 47\n__all__ = Names()\n"
        ),
        "dict_names.py": "class Names(dict): pass\npartial = 48\n__all__ = Names({0: 'partial'})\n",
    },
    "wildcard": {
        "main.py": "import consumer\n",
        "consumer.py": (
            "from public import *\nfrom explicit import *\n"
            "print(token, _private, 'ignored' in globals(), '_hidden' in globals())\n"
            "try:\n    from broken import *\nexcept AttributeError:\n    print(earlier)\n"
            "try:\n    from invalid import *\nexcept TypeError as error:\n    print(str(error))\n"
        ),
        "public.py": "token = 42\n_hidden = False\n",
        "explicit.py": "_private = 7\nignored = True\n__all__ = ['_' + 'private']\n",
        "broken.py": "earlier = 1\n__all__ = ['earlier', 'absent']\n",
        "invalid.py": "__all__ = [3]\n",
    },
    "type_checking_runtime_export": {
        "main.py": (
            "import helper\nfrom helper import token as selected\n"
            "print(selected is helper.token, selected[0])\n"
        ),
        "helper.py": (
            "import typing as t\n"
            "def initialize():\n    print('initialized')\n    return [41]\n"
            "if t.TYPE_CHECKING:\n    from unavailable_at_runtime import token\n"
            "else:\n    token = initialize()\n"
        ),
    },
    "record_warnings": {
        "main.py": (
            "import warnings\n"
            "with warnings.catch_warnings(record=True) as caught:\n    assert caught == []\n"
        ),
    },
    "contextmanager_generator_annotation": {
        "helper.py": (
            "from contextlib import contextmanager as manager\n"
            "@manager\ndef values(value: int) -> str:\n"
            "    yield str(value)\n    return 'done'\n"
        ),
        "main.py": (
            "import sys\nfrom helper import values\n"
            "raw = sys.modules[values.__module__].values.__wrapped__\n"
            "assert raw.__annotations__ == {'value': int, 'return': str}\n"
            "with values(3) as value:\n    print(value)\n"
            "generator = raw(4)\nprint(next(generator))\n"
            "try:\n    next(generator)\nexcept StopIteration as exc:\n    print(exc.value)\n"
        ),
    },
    "weak_method_export": {
        "main.py": (
            "from weakref import WeakMethod\n"
            "class Receiver:\n    def accept(self, value): return value\n"
            "receiver = Receiver()\nsaved = WeakMethod(receiver.accept)\n"
            "print(saved().__self__ is receiver, saved()(42))\n"
            "del receiver\nprint(saved() is None)\n"
        ),
    },
    "weak_key_dictionary_export": {
        "main.py": (
            "from weakref import WeakKeyDictionary, ref\n"
            "class Stream: pass\n"
            "stream = Stream()\ncache = WeakKeyDictionary()\n"
            "cache[stream] = 'cached'\n"
            "print(ref(stream)() is stream, cache.get(stream))\n"
        ),
    },
    "imported_struct_timestamp": {
        "main.py": (
            "from wire import encode, decode\nimport struct\n"
            "wire = encode(2**64 - 1)\n"
            "print(struct.calcsize('>Q'), wire, decode(wire), struct.unpack('>Q', wire))\n"
        ),
        "wire.py": (
            "import struct\nfrom struct import Struct as Format\n"
            "layout = Format('>Q')\nassert Format is struct.Struct\n"
            "encode = layout.pack\ndecode = layout.unpack\n"
        ),
    },
    "imported_bytecode_header": {
        "main.py": "from header import magic, decode, encode\nprint(list(magic), decode, encode)\n",
        "header.py": (
            "import marshal\nimport pickle\nimport sys\n"
            "from marshal import loads\nfrom pickle import dumps\n"
            "version = (sys.version_info[0] << 24) | sys.version_info[1]\n"
            "magic = b'j2' + pickle.dumps(5, 2) + dumps(version, 2)\n"
            "decode = loads(b'i\\x03\\x00\\x00\\x00')\n"
            "encode = pickle.dumps is dumps\n"
        ),
    },
    "namespace_package": {
        "main.py": "import package.child\nfrom package.nested import sibling\nprint(package.child.f())\nprint(sibling.read())\n",
        "package/child.py": "def f() -> int:\n    return 41\n",
        "package/nested/sibling.py": "from ..child import f\ndef read() -> int:\n    return f() + 1\n",
    },
    "imported_traceback_handler": {
        "main.py": "from handler import capture\nprint(capture())\n",
        "handler.py": (
            "import traceback as tb\nfrom io import StringIO\n"
            "class Writer:\n    def __init__(self):\n        self.stream = StringIO()\n"
            "def capture():\n"
            "    try:\n        raise ValueError('app factory')\n"
            "    except ValueError:\n"
            "        stream = Writer().stream\n"
            "        tb.print_exc(limit=0, file=stream)\n"
            "        return stream.getvalue() == tb.format_exc(0), stream.getvalue()\n"
        ),
    },
    "native_ast_import": {
        "main.py": (
            "import ast as trees\nfrom ast import parse, walk\n"
            "tree = parse('value', mode='eval')\n"
            "print(trees.parse is parse, trees.walk is walk)\n"
            "print([type(node).__name__ for node in walk(tree)])\n"
        ),
    },
    "registered_import_module": {
        "main.py": (
            "from consumer import ensure, with_local\nfrom importlib import import_module as load\n"
            "import importlib\nprint('before')\nensure('.'.join(['orchard', 'leaf']))\n"
            "leaf = load('orchard.leaf')\nprint(leaf.marker, load is importlib.import_module)\n"
            "print(leaf is load('.leaf', 'orchard'), leaf is load(name='..leaf', package='orchard.bud'))\n"
            "print(with_local(str))\n"
            "try:\n    load('absent' + '_package.child')\nexcept ModuleNotFoundError as e:\n    print(e.name)\n"
        ),
        "consumer.py": (
            "from importlib import import_module\n"
            "def ensure(package_name: str) -> None:\n    import_module(package_name)\n"
            "def with_local(import_module):\n    return import_module('unrelated')\n"
        ),
        "orchard/__init__.py": "print('package init')\n",
        "orchard/leaf.py": "print('leaf init')\nmarker = 47\n",
        "unrelated.py": "invalid syntax for an unrelated source file!\n",
    },
    "boxed_unittest_package": {
        "main.py": (
            "from importlib import import_module\nimport sys\nimport unittest\n"
            "from unittest import TestCase, mock\n"
            "assert sys.path and mock is import_module('unittest.mock')\n"
            "mock.Mock().assert_not_called()\n"
            "print(unittest.__name__, unittest.__package__, hasattr(unittest, '__path__'))\n"
            "print(import_module('unittest') is unittest, TestCase is unittest.TestCase)\n"
            "TestCase().assertTrue(True)\n"
        ),
    },
    "frozen_import_override": {
        "main.py": (
            "import _imp\nfrom _imp import _override_frozen_modules_for_tests as override\n"
            "print(_imp.is_frozen('__hello__'), _imp.is_frozen('json'))\n"
            "print(override(-1), _imp.is_frozen('__hello__'), _imp.is_frozen('_frozen_importlib'))\n"
            "import __hello__ as local\nprint(local.initialized)\n"
            "print(override(1), _imp.is_frozen('__hello__'))\n"
            "import __hello__ as cached\nprint(cached is local)\n"
            "print(override(0), _imp.is_frozen('__hello__'))\n"
            "try:\n    override(1 << 70)\nexcept OverflowError:\n    print('overflow')\n"
            "try:\n    _imp._override_multi_interp_extensions_check(1)\nexcept RuntimeError as e:\n    print(e)\n"
        ),
        "__hello__.py": "initialized = 'local'\n",
    },
    "boxed_stat_shadow": {
        "main.py": (
            "import sys\nsys.path = sys.path\n"
            "from importlib import import_module\n"
            "loaded = import_module(''.join(['st', 'at']))\n"
            "import stat\nfrom stat import S_IRWXU\nassert loaded is stat\n"
            "print(loaded.S_ISDIR(loaded.S_IFDIR), S_IRWXU)\n"
        ),
        "stat.py": "invalid syntax in a shadow of frozen stat!\n",
        "stat/__init__.py": "invalid shadow package!\n",
        "stat/S_IRWXU.py": "invalid shadow member!\n",
    },
    "email_address_parsers": {
        "main.py": (
            "from email._header_value_parser import get_addr_spec, get_angle_addr as angle\n"
            "import email._header_value_parser as parser\n"
            "assert angle is parser.get_angle_addr\n"
            "for parse, value in ((get_addr_spec, 'user@example.org SIZE=32'), (angle, '<>')):\n"
            "    address, rest = parse(value)\n"
            "    print(address.addr_spec, rest)\n"
        ),
    },
    "zip_source_spec": {
        "main.py": (
            "import sys\nimport zipimport\nfrom importlib import import_module, invalidate_caches\n"
            "from importlib.util import find_spec\nfrom witness import seen\n"
            "def refresh():\n    assert sys.exception() is None\n    refresh_finder()\n"
            "for name in ('stored', 'deflated'):\n"
            "    sys.path.insert(0, __file__.rsplit('/', 1)[0] + '/' + name + '.zip')\n"
            "    assert find_spec(name) is not None and name not in seen\n"
            "    package = import_module(name)\n    spec = find_spec(name)\n"
            "    assert package is import_module(name) and package.__spec__ is spec\n"
            "    assert spec.loader is package.__loader__ and type(spec.loader) is zipimport.zipimporter\n"
            "    assert spec.submodule_search_locations is package.__path__\n"
            "    print(seen, spec.loader.get_data(package.__path__[0] + '/message.txt'))\n"
            "finder = spec.loader\n"
            "with open(finder.archive, \"rb\") as archive:\n"
            "    original = archive.read()\n"
            "with open(finder.archive.rsplit(\"/\", 1)[0] + \"/stored.zip\", \"rb\") as archive:\n"
            "    replacement = archive.read()\n"
            "refresh_finder = finder.invalidate_caches\n"
            "finder.invalidate_caches = refresh\n"
            "try:\n"
            "    with open(finder.archive, \"wb\") as archive:\n"
            "        archive.write(replacement)\n"
            "    assert finder.find_spec(\"stored\") is None\n"
            "    names = ('importlib', 'importlib._bootstrap', 'importlib._bootstrap_external',\n"
            "             '_frozen_importlib', '_frozen_importlib_external')\n"
            "    saved = {key: sys.modules.pop(key) for key in names}\n"
            "    sys.modules['_frozen_importlib'] = sys.modules['_frozen_importlib_external'] = None\n"
            "    try:\n"
            "        source = import_module('importlib')\n"
            "        assert source._bootstrap_external is not saved['_frozen_importlib_external']\n"
            "        assert source.invalidate_caches() is None\n"
            "    finally:\n"
            "        sys.modules.update(saved)\n"
            "    assert finder.find_spec(\"deflated\") is None\n"
            "    assert finder.find_spec(\"stored\").loader is finder\n"
            "    assert finder.get_data(\"stored/message.txt\") == b\"hello\"\n"
            "    assert invalidate_caches() is None\n"
            "finally:\n"
            "    del finder.invalidate_caches\n"
            "    with open(finder.archive, \"wb\") as archive:\n"
            "        archive.write(original)\n"
            "    finder.invalidate_caches()\n"
        ),
        "witness.py": "seen = []\n",
    },
    "registered_typevar_frame": {
        "main.py": "from importlib import import_module\nimport_module('maker')\n",
        "maker.py": (
            "from typing import TypeVar\n"
            "print(TypeVar('Imported').__module__)\n"
            "first, second = [], []\n"
            "scope = {'__name__': first, 'second': second, 'T': TypeVar}\n"
            "exec(\"__name__ = second\\nstable = T('Stable')\\n"
            "def make():\\n    before = T('Before')\\n    make.__module__ = None\\n"
            "    return before, T('After')\", scope)\n"
            "make = scope['make']\nscope['__name__'] = 'rebound'\n"
            "before, after = make()\n"
            "print(scope['stable'].__module__ is first, before.__module__ is second, "
            "after.__module__ is None)\n"
        ),
    },
    "nested_importlib_abc": {
        "main.py": "import helper\nhelper.check()\n",
        "helper.py": (
            "from importlib import abc\n"
            "import importlib.abc as protocols\n"
            "from importlib.abc import FileLoader\n"
            "from importlib._abc import Loader\n"
            "from importlib.util import Loader as UtilLoader\n"
            "def check():\n"
            "    assert abc is protocols and FileLoader is abc.FileLoader\n"
            "    assert Loader is UtilLoader is abc.Loader\n"
            "    assert abc.MetaPathFinder().invalidate_caches() is None\n"
        ),
    },
    "late_importlib_abc": {
        "main.py": (
            "import importlib\nimport os\n"
            "name = 'installed_loader_plugin'\n"
            "path = os.path.join(os.path.dirname(__file__), name + '.py')\n"
            "with open(path, 'w') as file:\n"
            "    file.write('def check():\\n    from importlib import abc, util\\n'\n"
            "               '    from importlib._abc import Loader\\n'\n"
            "               '    assert abc.Loader is util.Loader is Loader\\n')\n"
            "try:\n    importlib.import_module(name).check()\n"
            "finally:\n    os.remove(path)\n"
        ),
    },
    "unregistered_source_loader": {
        "main.py": (
            "from importlib.machinery import SourceFileLoader\n"
            "from types import ModuleType\n"
            "path = __file__ + '.plugin.py'\n"
            "with open(path, 'wb') as out:\n"
            "    out.write(b\"# coding: latin-1\\ntext = 'caf\\xe9'\\n\")\n"
            "module = ModuleType('plugin')\n"
            "SourceFileLoader('plugin', path).exec_module(module)\n"
            "print(module.text)\n"
            "try:\n    SourceFileLoader('other', path + '.missing').get_code('other')\n"
            "except FileNotFoundError as exc:\n    print(exc.filename == path + '.missing')\n"
        ),
    },
    "namespace_resource_spec": {
        "main.py": (FIXTURES.parent / "boxed_namespace_resource_spec.py").read_text(),
        "first/acme/data.txt": "first",
        "second/acme/data.txt": "second",
        "second/acme/only.txt": "second",
    },
    "multiplexed_resources": {
        "main.py": (FIXTURES.parent / "boxed_multiplexed_resources.py").read_text(),
        "first/same.txt": "first",
        "first/shared/one": "1",
        "first/mixed/one": "1",
        "second/same.txt": "second",
        "second/shared/two": "2",
        "second/mixed": "file",
    },
    "filesystem_resources": {
        "main.py": (FIXTURES.parent / "boxed_filesystem_resources.py").read_text(),
        "data/__init__.py": (
            "from importlib.resources import files\n"
            "def wrapper(): return files(None)\n"
            "def regular(): return files(None)\n"
        ),
        "data/child.py": "def call(function): return function()\n",
        "data/nested/message.txt": "hello\n",
    },
    "materialize_resources": {
        "main.py": (FIXTURES.parent / "boxed_resources_as_file.py").read_text(),
        "data/__init__.py": "",
        "data/message.txt": "hello\n",
        "data/directory-payload.txt": "directory payload\n",
    },
    "source_loader_compile_override": {
        "main.py": (
            "from importlib.machinery import SourceFileLoader\n"
            "from types import ModuleType\n"
            "class PluginLoader(SourceFileLoader):\n"
            "    def get_data(self, path):\n"
            "        data = super().get_data(path)\n"
            "        self.source_to_code = self.compile_plugin\n"
            "        return data\n"
            "    def compile_plugin(self, data, path):\n"
            "        assert data == b'value = 1' and path == self.path\n"
            "        return compile('value = 2', path, 'exec')\n"
            "path = __file__ + '.plugin.py'\n"
            "with open(path, 'wb') as out:\n"
            "    out.write(b'value = 1')\n"
            "module = ModuleType('plugin')\n"
            "PluginLoader('plugin', path).exec_module(module)\n"
            "assert module.value == 2\n"
        ),
    },
    "source_loader_path_stats": {
        "main.py": (
            "import os\nfrom importlib.machinery import SourceFileLoader\n"
            "class PathLike:\n"
            "    def __init__(self, path): self.path = path\n"
            "    def __fspath__(self): return self.path\n"
            "loader = SourceFileLoader('example', __file__)\n"
            "stats = loader.path_stats(__file__)\n"
            "print(stats, type(stats['mtime']).__name__, type(stats['size']).__name__)\n"
            "assert stats == loader.path_stats(path=PathLike(__file__.encode()))\n"
            "fd = os.open(__file__, os.O_RDONLY)\n"
            "try:\n    assert stats == loader.path_stats(fd)\n"
            "finally:\n    os.close(fd)\n"
            "missing = b'absent-source-file'\n"
            "try:\n    loader.path_stats(PathLike(missing))\n"
            "except FileNotFoundError as error:\n    print(error.filename is missing)\n"
            "else:\n    raise AssertionError('accepted a missing path')\n"
        ),
    },
    "py_compile_prechecks": {
        "main.py": "from consumer import check\ncheck()\n",
        "consumer.py": (
            "import py_compile\n"
            "def check():\n"
            "    print(py_compile.PycInvalidationMode.CHECKED_HASH.name)\n"
            "    for output in ('.', 'absent.pyc'):\n"
            "        try:\n"
            "            py_compile.compile('absent.py', output, doraise=True)\n"
            "        except (FileExistsError, FileNotFoundError) as error:\n"
            "            print(type(error).__name__, str(error))\n"
            "        else:\n"
            "            raise AssertionError(output)\n"
        ),
    },
    "frozen_external_alias": {
        "main.py": (
            "import _frozen_importlib_external as frozen\n"
            "import importlib._bootstrap_external as external\n"
            "from importlib import _bootstrap_external\n"
            "from importlib.machinery import SourceFileLoader\n"
            "print(external is frozen, external is _bootstrap_external)\n"
            "print(external.__name__, external.__package__, SourceFileLoader.__module__)\n"
            "print(external.SourceFileLoader is SourceFileLoader)\n"
            "loader = external.SourceFileLoader('example', __file__)\n"
            "print(loader.get_filename('example') == __file__, bool(loader.get_data(__file__)))\n"
        ),
    },
    "loader_source_to_code": {
        "main.py": (
            "from importlib.machinery import SourceFileLoader\n"
            "loader = SourceFileLoader('module', 'unread.py')\n"
            "for source, level in [('value = 41', -1), (b'value = 42', 0)]:\n"
            "    code = loader.source_to_code(source, b'display.py', _optimize=level)\n"
            "    scope = {}\n"
            "    exec(code, scope)\n"
            "    print(code.co_filename, scope['value'])\n"
            "try:\n"
            "    loader.source_to_code(b'# coding: missing-codec', 'bad.py')\n"
            "except SyntaxError as error:\n"
            "    print(error.msg, error.filename, error.lineno)\n"
            "else:\n"
            "    raise AssertionError('invalid coding cookie accepted')\n"
            "try:\n"
            "    loader.source_to_code(None, b'bad\\x00name', _optimize='no index')\n"
            "except ValueError as error:\n"
            "    print(str(error))\n"
        ),
    },
    "registered_source_spec": {
        "main.py": (
            "from importlib.util import find_spec\nfrom importlib import import_module\n"
            "first = find_spec('orchard.leaf')\nsecond = find_spec('.leaf', 'orchard')\n"
            "print(first == second, first is second, first.parent, first.loader.is_package(first.name))\n"
            "leaf = import_module('orchard.leaf')\npackage = import_module('orchard')\n"
            "print(leaf.marker, find_spec(first.name) is leaf.__spec__, leaf.__spec__.loader is leaf.__loader__)\n"
            "print(package.__path__ is package.__spec__.submodule_search_locations, "
            "leaf.__file__ == first.origin, find_spec('orchard.missing') is None)\n"
            "leaf.__spec__ = 'app spec'\nprint(find_spec('.leaf', 'orchard'))\n"
        ),
        "orchard/__init__.py": "print('parent init')\n",
        "orchard/leaf.py": "print('leaf init')\nmarker = __name__\n",
    },
    "conditional_function_export": {
        "main.py": (
            "from helper import selected as chosen\nimport helper\n"
            "print(chosen(), helper.selected is chosen, helper.other())\n"
        ),
        "helper.py": (
            "import sys\n"
            "def guard(prefix):\n    print(prefix)\n    return sys.platform.startswith(prefix)\n"
            "if guard('win'):\n    def selected():\n        return 'windows'\n"
            "elif guard('linux'):\n    def selected():\n        return 'linux'\n"
            "else:\n    def selected():\n        return 'other'\n"
            "if guard(''):\n    def other():\n        return 'always'\n"
            "else:\n    def other():\n        return 'unreachable'\n"
        ),
    },
    "conditional_package_value": {
        "main.py": "from helper import selected as chosen\nimport helper\nprint(chosen, helper.selected)\n",
        "helper/__init__.py": (
            "import sys\n"
            "if sys.platform == 'win32':\n    selected = 'windows'\n"
            "elif sys.platform == 'linux':\n    selected = 'linux'\n"
            "else:\n    selected = None\n"
        ),
    },
    "conditional_function_then_definition": {
        "main.py": "from helper import selected\nprint(selected())\n",
        "helper.py": (
            "if True:\n    def selected():\n        return 'branch'\n"
            "else:\n    def selected():\n        return 'other'\n"
            "def selected():\n    return 'final'\n"
        ),
    },
    "ready_function_in_cycle": {
        "main.py": "from pkg.parent import observed\nprint(observed)\n",
        "pkg/__init__.py": "",
        "pkg/parent.py": "def increment(n: int) -> int:\n    return n + 1\nfrom . import child\nobserved = child.result\n",
        "pkg/child.py": "from .parent import increment\nresult = increment(41)\n",
    },
    "partial_child_shared_storage": {
        "main.py": "import pkg.parent\n",
        "pkg/__init__.py": "",
        "pkg/parent.py": "shared = [1]\nfrom . import child\nprint(shared[-1])\n",
        "pkg/child.py": "from . import parent as first\nfrom . import parent as second\nprint(first is second)\nfirst.shared.append(42)\nprint(second.shared[-1])\n",
    },
    "cast_mutable": {"main.py": 'from typing import cast as view\ndef fresh() -> list[int]:\n    print("create")\n    return [1]\nvalues = view("list[int]", fresh())\nview("list[int]", values).append(7)\nprint(values[1])\n'},
    "type_aliases": {
        "types_used.py": (
            "from typing import NewType, TYPE_CHECKING\n"
            "if TYPE_CHECKING:\n    import pathlib as p\n    from typing import Literal\n"
            '    Mode = Literal["read", "write"]\n'
            'Name = NewType("Name", str)\nPair = tuple[Name, int]\n'
            'def named(value: Name) -> Pair:\n    return value, 7\n'
            'def mode(value: "Mode") -> str:\n    return value\n'
            'def leaf(value: "p.Path") -> str:\n    return value.name\n'
        ),
        "main.py": (
            "from pathlib import Path\nfrom types_used import Name, named, mode, leaf\n"
            'data = named(Name("alias"))\nprint(data[0])\nprint(data[1])\nprint(mode("read"))\nprint(leaf(Path("one/two")))\n'
        ),
    },
    "inferred_signature": {"main.py": IMPORT, "helper.py": "def f(value):\n    return value\n"},
    "library_effect": {"main.py": IMPORT, "helper.py": "print(7)\n" + HELPER},
    "library_state": {"main.py": "from helper import f, values\nprint(f(values[0]))\n", "helper.py": "values = [1]\n" + HELPER},
    "package_effect": {"main.py": "import package\n", "package/__init__.py": "print(1)\n"},
    "active_library_else": {"main.py": IMPORT, "helper.py": HELPER + 'if __name__ == "__main__":\n    print(1)\nelse:\n    print(2)\n'},
    "module_literal_before_import": {
        "main.py": ('from parentlib import labels\nfrom visitorlib import shared\n'
                    'print(labels is shared)\nprint(labels[-1])\n'),
        "parentlib.py": ('labels = ["one", "two", "three", "four"]\n'
                         'def expose() -> list[str]:\n    return labels\nimport visitorlib\n'),
        "visitorlib.py": ('from parentlib import expose\nshared = expose()\n'
                          'shared.append("visited")\n'),
    },
    "ordered_library_effects": {
        "main.py": "from pkg.parent import value\nprint(value)\n", "pkg/__init__.py": "",
        "pkg/parent.py": 'print("parent prefix")\nfrom . import child\nprint("parent suffix")\nvalue = child.value\n',
        "pkg/child.py": 'print("child init")\nvalue = 42\n',
    },
    "delayed_cached_alias": {
        "main.py": "from pkg.parent import read\nprint(read())\n", "pkg/__init__.py": "",
        "pkg/child.py": 'print("child once")\n' + HELPER,
        "pkg/parent.py": ('from . import child\ndef read() -> int:\n    return late(41)\n'
                          'print("before alias")\nfrom .child import f as late\n'),
    },
    "guard_before_import": {
        "main.py": "import pkg.guard\n",
        "pkg/__init__.py": "",
        "pkg/guard.py": 'if __name__ != "pkg.guard":\n    raise ValueError("wrong module")\nfrom . import child\n',
        "pkg/child.py": 'print("child initialized")\n',
    },
}


def run(command: list[str], cwd: Path, *, timeout: int = 60) -> subprocess.CompletedProcess[str]:
    return subprocess.run(command, cwd=cwd, text=True, capture_output=True, timeout=timeout,
                          env={**os.environ, "ASAN_OPTIONS": "detect_leaks=1:halt_on_error=1"})


def write_files(root: Path, files: dict[str, str]) -> None:
    for name, source in files.items():
        path = root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(source)


def require_success(result: subprocess.CompletedProcess[str]) -> None:
    assert result.returncode == 0, (result.args, result.stdout, result.stderr)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--compiler", type=Path, required=True)
    parser.add_argument("--runtime", type=Path, required=True)
    parser.add_argument("--python", default=sys.executable)
    parser.add_argument("--clang", default="clang")
    parser.add_argument("--lto-linker", type=Path)
    parser.add_argument("--target")
    parser.add_argument("--opt-levels", type=int, nargs="+", default=[0, 3])
    parser.add_argument("--sanitize", action="store_true")
    args = parser.parse_args()
    build_flags = []
    if args.lto_linker is not None:
        build_flags += ["--lto-linker", str(args.lto_linker.absolute())]
    if args.target is not None:
        build_flags += ["--target", args.target]
    compiler, runtime = str(args.compiler.resolve()), str(args.runtime.resolve())
    checks = 0
    with tempfile.TemporaryDirectory(prefix="imre-imports-") as directory:
        temporary = Path(directory)
        root = temporary / "project"
        shutil.copytree(FIXTURES, root)
        for entry, module in [("entry.py", "entry"), ("package/program.py", "package.program"),
                              ("package/__main__.py", "package")]:
            baseline = run([args.python, "-E", "-m", module], root)
            require_success(baseline)
            require_success(run([compiler, "check", str(root / entry)], temporary))
            for level in args.opt_levels:
                executable = temporary / f"program-{module}-{level}"
                command = [compiler, "build", str(root / entry), "-o", str(executable), "--runtime", runtime,
                           "--opt-level", str(level), "--clang", args.clang]
                command += build_flags
                # Exercise explicit and inferred roots independently of the current directory.
                if level == args.opt_levels[-1]:
                    command += ["--module-root", str(root)]
                if args.sanitize:
                    command += ["--sanitize"]
                require_success(run(command, temporary))
                result = run([str(executable)], temporary)
                require_success(result)
                assert result.stdout == baseline.stdout, (entry, level, result.stdout, baseline.stdout)
                checks += 1

        build_timeout = 120 if args.lto_linker and not args.sanitize else 60
        for name, files in ACCEPTED.items():
            if name == "boxed_stat_shadow" and sys.platform not in ("linux", "darwin"):
                continue
            case = temporary / name
            write_files(case, files)
            if name == "namespace_resource_spec" and os.name == "posix":
                with ZipFile(case / "portions.zip", "w") as archive:
                    archive.writestr("acme/", b"")
                    archive.writestr("only_archive/", b"")
            if name == "zip_source_spec":
                if os.name != "posix":
                    continue
                for pkg, method in [("stored", ZIP_STORED), ("deflated", ZIP_DEFLATED)]:
                    with ZipFile(case / f"{pkg}.zip", "w", compression=method) as archive:
                        archive.writestr(f"{pkg}/__init__.py", "# coding: (ordinary comment)\nfrom witness import seen\nseen.append(__name__)\n")
                        archive.writestr(f"{pkg}/message.txt", b"hello")
            baseline = run([args.python, "-E", "-m", "main"], case)
            require_success(baseline)
            if name in {"codeop_module", "codeop_named_helper"}:
                # Run these checked modules with the late-import executable below.
                require_success(run([compiler, "check", str(case / "main.py")], temporary))
                checks += 1
                continue
            for level in args.opt_levels:
                executable = temporary / f"accepted-{name}-{level}"
                command = [compiler, "build", str(case / "main.py"), "-o", str(executable),
                           "--runtime", runtime, "--clang", args.clang, "--opt-level", str(level),
                           *build_flags]
                if args.sanitize:
                    command.append("--sanitize")
                # Optimizing the bundled address parser and warnings modules can take several minutes.
                slow_optimized = name in {"email_address_parsers", "record_warnings", "warn_named_helper"}
                timeout = 300 if slow_optimized and level > 0 else build_timeout
                require_success(run(command, temporary, timeout=timeout))
                result = run([str(executable)], temporary)
                require_success(result)
                assert result.stdout == baseline.stdout, (name, level, result.stdout, baseline.stdout)
                if name == "entry_module_attribute_guard":
                    denied = run([str(executable), "deny"], temporary)
                    expected = run([args.python, "-E", "-m", "main", "deny"], case)
                    assert denied.returncode == expected.returncode == 1
                    assert denied.stdout == expected.stdout == "feature loaded\n"
                    assert denied.stderr.splitlines()[-1] == expected.stderr.splitlines()[-1]
                checks += 1

        late_import = temporary / "late_import.py"
        late_import.write_text("import importlib, sys\nimportlib.import_module(sys.argv[1]).run()\n")
        for level in args.opt_levels:
            executable = temporary / f"late-import-{level}"
            command = [compiler, "build", str(late_import), "-o", str(executable),
                       "--runtime", runtime, "--clang", args.clang, "--opt-level", str(level),
                       *build_flags, "--runtime-imports", "--no-cache"]
            if args.sanitize:
                command.append("--sanitize")
            require_success(run(command, temporary, timeout=build_timeout))
            # Load the probe from disk so the executable cannot resolve its imports at build time.
            probe = temporary / f"late_import_probe_{level}.py"
            probe.write_text(
                "import builtins, sys\n"
                "def run():\n"
                "    sys.path.insert(0, sys.argv[4])\n"
                "    module = builtins.__import__(sys.argv[2])\n"
                "    if sys.argv[3] == 'flags':\n"
                "        print(module.CommandCompiler().compiler.flags)\n"
                "    elif sys.argv[3] == 'lookup':\n"
                "        print(module.opmap['POP_TOP'])\n"
                "    elif sys.argv[3] == 'current':\n"
                "        module.current()\n"
            )
            for module, operation, path, expected in [
                ("opcode", "lookup", temporary, "31\n"), ("_opcode_metadata", "lookup", temporary, "31\n"),
                ("code", "flags", temporary, "16896\n"), ("codeop", "flags", temporary, "16896\n"),
                ("main", "import", temporary / "codeop_module", ""),
                ("main", "import", temporary / "codeop_named_helper", "16896\n"),
                ("boxed_interactive_interpreter", "current", FIXTURES.parent, ""),
            ]:
                result = run([str(executable), probe.stem, module, operation, str(path)], temporary)
                require_success(result)
                assert result.stdout == expected, (module, level, result.stdout)
                checks += 1

            absent = "no_such_console_guard_module"
            result = run([str(executable), probe.stem, absent, "flags", str(temporary)], temporary)
            assert result.returncode == 1 and f"ModuleNotFoundError: No module named '{absent}'" in result.stderr, result
            checks += 1

        # These are build capabilities: CPython's flags depend on its build; Imre emits native code.
        for level in args.opt_levels:
            for fixture, route, output in [
                ("opcode_flags", [], "0\n0\nno bytecode specialization\n"),
                ("opcode_stack_effect", [], "-1\n"),
                ("opcode_module", [], "opcode module OK\n"),
                ("opcode_operations", [], "opcode operations OK\n"),
                ("opcode_operations", ["--runtime-imports"], "opcode operations OK\n"),
            ]:
                executable = temporary / f"{fixture}-{level}"
                command = [compiler, "build", str(root / f"{fixture}.py"), "-o", str(executable),
                           "--runtime", runtime, "--clang", args.clang, "--opt-level", str(level),
                           *build_flags, *route]
                if args.sanitize:
                    command.append("--sanitize")
                require_success(run(command, temporary))
                result = run([str(executable)], temporary)
                require_success(result)
                assert result.stdout == output, result
                checks += 1

        # A failing guard must run before the next import initializes its child.
        guarded = temporary / "wrong_name"
        write_files(guarded, {"main.py": "import helper\n", "child.py": 'print("child initialized")\n',
                              "helper.py": 'if "helper" == __name__:\n    raise ValueError("wrong module")\nimport child\n'})
        baseline = run([args.python, "-E", "-m", "main"], guarded)
        assert baseline.returncode and not baseline.stdout
        for level in args.opt_levels:
            executable = temporary / f"wrong-name-{level}"
            require_success(run([compiler, "build", str(guarded / "main.py"), "-o", str(executable),
                                 "--runtime", runtime, "--clang", args.clang, "--opt-level", str(level),
                                 *(["--sanitize"] if args.sanitize else []), *build_flags], temporary))
            result = run([str(executable)], temporary)
            assert result.returncode and result.stdout == baseline.stdout, result
            assert result.stderr.splitlines()[-1] == baseline.stderr.splitlines()[-1], result
            checks += 1

        chain = temporary / "deep_imports"
        files = {f"m{i}.py": f"import m{i + 1}\n" for i in range(97)}
        write_files(chain, {**files, "m97.py": "pass\n"})
        require_success(run([compiler, "check", str(chain / "m1.py")], temporary))
        result = run([compiler, "check", str(chain / "m0.py")], temporary)
        assert result.returncode == 1 and "import nesting exceeds 96" in result.stderr, result
        checks += 2

        for name, (files, expected) in REJECTED.items():
            case = temporary / name
            write_files(case, files)
            result = run([compiler, "check", str(case / "main.py")], temporary)
            assert result.returncode != 0 and expected in result.stderr, (name, result.stdout, result.stderr)
            checks += 1

        # The imported object operation precedes the call that supplies flag's type.
        for name, function, diagnostic in [
            ("annotated", "def choose(flag: bool) -> int:\n    return flag\n", "expected int, got bool"),
            ("inferred", "def choose(flag):\n    if flag:\n        return 1\n    return 'one'\n", "inferred returns require"),
            ("unused_object", "def choose(flag):\n    parsed = ast.parse('1')\n    if flag:\n        return 1\n    return 'one'\n", "inferred returns require"),
            ("live_object", "def choose(flag) -> int:\n    parsed = ast.parse('1')\n    print(parsed)\n    return 'one'\n", "invalid-return-type"),
        ]:
            case = temporary / ("object_return_" + name)
            write_files(case, {
                "main.py": "import helper\n",
                "helper.py": "import ast\ntree = ast.parse('1')\n" + function + "choose(True)\n",
            })
            command = [compiler, "check", str(case / "main.py")]
            for flags in ([], ["--reachable-only"]):
                result = run(command + flags, temporary)
                assert result.returncode == 1 and diagnostic in result.stderr, (name, result)
            require_success(run(command + ["--runtime-imports"], temporary))
            checks += 3

        # A TypeError raised while evaluating a return operand can still be caught.
        caught = temporary / "object_return_operand"
        write_files(caught, {
            "main.py": "from helper import wrapper\ntry:\n    wrapper()\nexcept TypeError:\n    print('caught')\n",
            "helper.py": "import ast\nast.parse('1')\ndef take(value: int) -> int:\n    return value\n"
                         "def wrapper() -> int:\n    return take('argument')\n",
        })
        for level in args.opt_levels:
            executable = temporary / f"operand-{level}"
            for flags in ([], ["--reachable-only"]):
                command = [compiler, "build", str(caught / "main.py"), "-o", str(executable),
                           "--runtime", runtime, "--clang", args.clang, "--opt-level", str(level), *build_flags, *flags]
                if args.sanitize:
                    command += ["--sanitize"]
                require_success(run(command, temporary))
                result = run([str(executable)], temporary)
                require_success(result)
                assert result.stdout == "caught\n", result
                checks += 1

        collision = temporary / "collisions"
        write_files(collision, {"main.py": IMPORT, "helper.py": HELPER})
        dependency = collision / "helper.py"
        for alias in [dependency, collision / "symlink.py", collision / "hardlink.py"]:
            if alias.name == "symlink.py":
                alias.symlink_to(dependency)
            elif alias.name == "hardlink.py":
                os.link(dependency, alias)
            for output_flag in ["-o", "--emit-llvm"]:
                command = [compiler, "build", str(collision / "main.py"), "--runtime", runtime,
                           "-o", str(temporary / "collision-program"), output_flag, str(alias), "--clang", args.clang]
                command += build_flags
                result = run(command, temporary)
                assert result.returncode != 0 and "distinct paths" in result.stderr, result.stderr
                assert dependency.read_text() == HELPER
                checks += 1

        alias_case = temporary / "module_alias"
        write_files(alias_case, {"main.py": "import helper\nimport alias\n",
                                "helper.py": HELPER + "def label() -> str:\n    return __name__\n"})
        (alias_case / "alias.py").symlink_to(alias_case / "helper.py")
        result = run([compiler, "check", str(alias_case / "main.py")], temporary)
        assert result.returncode != 0 and "multiple module identities" in result.stderr, result.stderr
        checks += 1

        # A single internal symlink keeps its logical import name in both ty and codegen.
        (alias_case / "main.py").write_text("import alias\nprint(alias.label())\nprint(alias.f(4))\n")
        baseline = run([args.python, "-E", "-m", "main"], alias_case)
        require_success(baseline)
        for level in args.opt_levels:
            executable = temporary / f"symlink-program-{level}"
            command = [compiler, "build", str(alias_case / "main.py"), "-o", str(executable),
                       "--runtime", runtime, "--opt-level", str(level), "--clang", args.clang]
            command += build_flags
            if args.sanitize:
                command += ["--sanitize"]
            require_success(run(command, temporary))
            result = run([str(executable)], temporary)
            require_success(result)
            assert result.stdout == baseline.stdout, (result.stdout, baseline.stdout)
            checks += 1

        escape = temporary / "escape"
        write_files(escape, {"main.py": "import helper\n"})
        (escape / "helper.py").symlink_to(dependency)
        result = run([compiler, "check", str(escape / "main.py")], temporary)
        assert result.returncode != 0 and "outside" in result.stderr, result.stderr
        checks += 1
    print(f"{checks} import checks passed")


if __name__ == "__main__":
    main()
